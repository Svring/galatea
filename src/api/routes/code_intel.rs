use poem::{Route, get, handler, post, web::Json, http::StatusCode, Error as PoemError, web::Data};
use anyhow::{Result, Context};
use crate::api::models::*;
use crate::codebase_indexing::parser::{self, CodeEntity};
use crate::codebase_indexing::postprocessor;
use crate::codebase_indexing::embedding as embedder;
use crate::codebase_indexing::vector_db as hoarder;
use crate::codebase_indexing::pipeline;
use crate::file_system;
use tracing::{error, info, warn};
use tokio;

#[handler]
async fn code_intel_health() -> &'static str {
    "Code Intel API route is healthy"
}

#[handler]
async fn parse_file_handler(
    Json(req): Json<ParseFileRequest>,
) -> Result<Json<Vec<CodeEntity>>, PoemError> {
    let file_path = match file_system::resolve_path(&req.file_path) {
        Ok(p) => p,
        Err(e) => return Err(PoemError::from_string(e.to_string(), StatusCode::BAD_REQUEST)),
    };
    
    if !file_path.exists() {
        return Err(PoemError::from_string(
            format!("File not found: {}", file_path.display()),
            StatusCode::NOT_FOUND,
        ));
    }
    
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| {
            PoemError::from_string("File has no extension", StatusCode::BAD_REQUEST)
        })?;
        
    let parse_result = match extension {
        "rs" => parser::extract_rust_entities_from_file(&file_path, req.max_snippet_size),
        "ts" => parser::extract_ts_entities(&file_path, false, req.max_snippet_size),
        "tsx" => parser::extract_ts_entities(&file_path, true, req.max_snippet_size),
        _ => Err(anyhow::anyhow!("Unsupported file extension: {}", extension)),
    };
    
    match parse_result {
        Ok(entities) => Ok(Json(entities)),
        Err(e) => Err(PoemError::from_string(
            format!("Error parsing file: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn parse_directory_handler(
    Json(req): Json<ParseDirectoryRequest>,
) -> Result<Json<Vec<CodeEntity>>, PoemError> {
    let dir = std::path::PathBuf::from(&req.dir);
    let suffixes_ref: Vec<&str> = req.suffixes.iter().map(|s| s.as_str()).collect();
    let exclude_dirs = req.exclude_dirs.unwrap_or_else(|| {
        vec![
            "node_modules".to_string(),
            "target".to_string(),
            "dist".to_string(),
            "build".to_string(),
            ".git".to_string(),
            ".vscode".to_string(),
            ".idea".to_string(),
        ]
    });
    let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
    
    let granularity = match req.granularity.as_deref() {
        Some("coarse") => postprocessor::Granularity::Coarse,
        Some("medium") => postprocessor::Granularity::Medium,
        _ => postprocessor::Granularity::Fine,
    };
    
    let files_to_parse =
        match file_system::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(files) => files,
            Err(e) => {
                return Err(PoemError::from_string(
            format!("Error finding files: {}", e),
                    StatusCode::INTERNAL_SERVER_ERROR,
        ))
            }
    };
    
    if files_to_parse.is_empty() {
        return Ok(Json(Vec::new()));
    }
    
    let mut all_entities: Vec<CodeEntity> = Vec::new();
    for file_path in files_to_parse {
        let extension = file_path.extension().and_then(|ext| ext.to_str());
        let parse_result = match extension {
            Some("rs") => {
                parser::extract_rust_entities_from_file(&file_path, req.max_snippet_size)
            }
            Some("ts") => parser::extract_ts_entities(&file_path, false, req.max_snippet_size),
            Some("tsx") => parser::extract_ts_entities(&file_path, true, req.max_snippet_size),
            _ => continue,
        };
        
        if let Ok(entities) = parse_result {
            all_entities.extend(entities);
        }
    }
    
    let final_entities =
        postprocessor::post_process_entities(all_entities, granularity, req.max_snippet_size);
    Ok(Json(final_entities))
}

#[handler]
async fn query_collection_handler(
    Json(req): Json<QueryRequest>,
) -> Result<Json<Vec<CodeEntity>>, PoemError> {
    info!(target: "galatea::api::code_intel", collection_name = %req.collection_name, query_text = %req.query_text, "API query request");

    let qdrant_url = req.qdrant_url.as_deref().unwrap_or("http://localhost:6334");

    match hoarder::query(
        &req.collection_name,
        &req.query_text,
        req.model,
        req.api_key,
        req.api_base,
        qdrant_url,
    )
    .await
    {
        Ok(entities) => Ok(Json(entities)),
        Err(e) => {
            error!(target: "galatea::api::code_intel", error = ?e, collection_name = %req.collection_name, "Error in API query_collection");
            Err(PoemError::from_string(
                format!("Error querying collection: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn generate_embeddings_api_handler(
    Json(req): Json<GenerateEmbeddingsRequest>,
) -> Result<Json<GenericApiResponse>, PoemError> {
    info!(target: "galatea::api::code_intel", input_file = %req.input_file, output_file = %req.output_file, "API request to generate embeddings");

    let input_path = std::path::PathBuf::from(&req.input_file);
    let output_path = std::path::PathBuf::from(&req.output_file);

    if !input_path.exists() {
        return Err(PoemError::from_string(
            format!("Input file not found: {}", req.input_file),
            StatusCode::BAD_REQUEST,
        ));
    }

    match embedder::generate_embeddings_for_index(
        &input_path, 
        &output_path, 
        req.model, 
        req.api_key, 
        req.api_base,
    )
    .await
    {
        Ok(_) => Ok(Json(GenericApiResponse {
            success: true,
            message: "Embeddings generated successfully.".to_string(),
            details: Some(format!("Output written to {}", req.output_file)),
        })),
        Err(e) => {
            error!(target: "galatea::api::code_intel", error = ?e, input_file = %req.input_file, "Error in API generate_embeddings_api");
            Err(PoemError::from_string(
                format!("Failed to generate embeddings: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn upsert_embeddings_api_handler(
    Json(req): Json<UpsertEmbeddingsRequest>,
) -> Result<Json<GenericApiResponse>, PoemError> {
    info!(target: "galatea::api::code_intel", input_file = %req.input_file, collection_name = %req.collection_name, "API request to upsert embeddings");

    let input_path = std::path::PathBuf::from(&req.input_file);
    let qdrant_url = req.qdrant_url.as_deref().unwrap_or("http://localhost:6334");

    if !input_path.exists() {
        return Err(PoemError::from_string(
            format!("Input file not found: {}", req.input_file),
            StatusCode::BAD_REQUEST,
        ));
    }

    if let Err(e) = hoarder::create_collection(&req.collection_name, qdrant_url).await {
        warn!(target: "galatea::api::code_intel", error = ?e, collection_name = %req.collection_name, "Error creating collection");
    }

    match hoarder::upsert_embeddings(&req.collection_name, &input_path, qdrant_url).await {
        Ok(_) => Ok(Json(GenericApiResponse {
            success: true,
            message: "Embeddings upserted successfully.".to_string(),
            details: Some(format!(
                "Upserted from {} to collection {}",
                req.input_file, req.collection_name
            )),
        })),
        Err(e) => {
            error!(target: "galatea::api::code_intel", error = ?e, input_file = %req.input_file, collection_name = %req.collection_name, "Error in API upsert_embeddings_api");
            Err(PoemError::from_string(
                format!("Failed to upsert embeddings: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn build_index_api_handler(
    Json(req): Json<BuildIndexRequest>,
) -> Result<Json<GenericApiResponse>, PoemError> {
    info!(target: "galatea::api::code_intel", directory = %req.dir, collection_name = %req.collection_name, "API request to build index (background task)");

    let qdrant_url_for_spawn = req
        .qdrant_url
        .clone()
        .unwrap_or_else(|| "http://localhost:6334".to_string());

    let dir_clone = req.dir.clone();
    let suffixes_clone: Vec<String> = req.suffixes.clone();
    let exclude_dirs_clone = req.exclude_dirs.clone();
    let max_snippet_size_clone = req.max_snippet_size;
    let granularity_str_clone = req.granularity.clone();
    let embedding_model_clone = req.embedding_model.clone();
    let api_key_clone = req.api_key.clone();
    let api_base_clone = req.api_base.clone();
    let collection_name_clone = req.collection_name.clone();

    tokio::spawn(async move {
        let qdrant_url_inner = qdrant_url_for_spawn;
        let dir_path = std::path::PathBuf::from(dir_clone);
        let suffixes_ref: Vec<&str> = suffixes_clone.iter().map(|s| s.as_str()).collect();
        
        let default_exclude_dirs = vec![
            "node_modules".to_string(), "target".to_string(), "dist".to_string(),
            "build".to_string(), ".git".to_string(), ".vscode".to_string(), ".idea".to_string(),
        ];
        let exclude_dirs_owned = exclude_dirs_clone.unwrap_or(default_exclude_dirs);
        let exclude_dirs_ref: Vec<&str> = exclude_dirs_owned.iter().map(|s| s.as_str()).collect();
        
        let granularity = match granularity_str_clone.as_deref() {
            Some("coarse") => postprocessor::Granularity::Coarse,
            Some("medium") => postprocessor::Granularity::Medium,
            _ => postprocessor::Granularity::Fine,
        };

        info!(target: "galatea::build_index_task", "Starting Full Index Build (API Triggered)");

        info!(target: "galatea::build_index_task", "[1/4] Finding files...");
        let files_to_parse =
            match file_system::find_files_by_extensions(&dir_path, &suffixes_ref, &exclude_dirs_ref) {
            Ok(files) => files,
            Err(e) => {
                error!(target: "galatea::build_index_task", error = ?e, "Wander step failed");
                return;
            }
        };
        if files_to_parse.is_empty() { 
            info!(target: "galatea::build_index_task", "No matching files found. Index build cancelled.");
            return; 
        }
        info!(target: "galatea::build_index_task", count = files_to_parse.len(), "Found files.");

        info!(target: "galatea::build_index_task", "[2/4] Parsing files...");
        let mut all_entities: Vec<CodeEntity> = Vec::new();
        for file_path in files_to_parse {
            let extension = file_path.extension().and_then(|ext| ext.to_str());
            let parse_result = match extension {
                Some("rs") => parser::extract_rust_entities_from_file(&file_path, max_snippet_size_clone),
                Some("ts") => parser::extract_ts_entities(&file_path, false, max_snippet_size_clone),
                Some("tsx") => parser::extract_ts_entities(&file_path, true, max_snippet_size_clone),
                _ => continue,
            };
            match parse_result {
                Ok(entities) => all_entities.extend(entities),
                Err(e) => error!(target: "galatea::build_index_task", error = ?e, file_path = %file_path.display(), "Error parsing file. Skipping."),
            }
        }
        info!(target: "galatea::build_index_task", count = all_entities.len(), "Parsed initial entities.");

        info!(target: "galatea::build_index_task", ?granularity, "[2b/4] Post-processing entities...");
        let processed_entities = postprocessor::post_process_entities(all_entities, granularity, max_snippet_size_clone);
        info!(target: "galatea::build_index_task", count = processed_entities.len(), "Entities after post-processing.");
        if processed_entities.is_empty() { 
            info!(target: "galatea::build_index_task", "No entities after processing. Index build cancelled.");
            return; 
        }

        info!(target: "galatea::build_index_task", "[3/4] Generating embeddings...");
        let entities_with_embeddings = match embedder::generate_embeddings_for_vec(
            processed_entities, embedding_model_clone, api_key_clone, api_base_clone).await {
            Ok(entities) => entities,
            Err(e) => {
                error!(target: "galatea::build_index_task", error = ?e, "Embedding step failed");
                return;
            }
        };
        info!(target: "galatea::build_index_task", "Embeddings generated.");
        if entities_with_embeddings.iter().all(|e| e.embedding.is_none()) {
            warn!(target: "galatea::build_index_task", "Warning: No entities had embeddings generated successfully.");
        }

        info!(target: "galatea::build_index_task", collection_name = %collection_name_clone, "[4/4] Storing embeddings...");
        if let Err(e) = hoarder::create_collection(&collection_name_clone, &qdrant_url_inner).await {
            error!(target: "galatea::build_index_task", error = ?e, "Failed to ensure Qdrant collection exists");
            return;
        }
        if let Err(e) = hoarder::upsert_entities_from_vec(&collection_name_clone, entities_with_embeddings, &qdrant_url_inner).await {
            error!(target: "galatea::build_index_task", error = ?e, "Upserting embeddings to Qdrant failed");
            return;
        }
        info!(target: "galatea::build_index_task", "--- Index Build Complete (API Triggered) ---");
    });

    Ok(Json(GenericApiResponse {
        success: true,
        message: "Build index process started in the background.".to_string(),
        details: Some(format!(
            "Building index for dir '{}' into collection '{}'. Check server logs for progress.",
            req.dir, req.collection_name
        )),
    }))
}

pub fn code_intel_routes() -> Route {
    Route::new()
        .at("/health", get(code_intel_health))
        .at("/parse-file", post(parse_file_handler))
        .at("/parse-directory", post(parse_directory_handler))
        .at("/query", post(query_collection_handler))
        .at("/generate-embeddings", post(generate_embeddings_api_handler))
        .at("/upsert-embeddings", post(upsert_embeddings_api_handler))
        .at("/build-index", post(build_index_api_handler))
} 