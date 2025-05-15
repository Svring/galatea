use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

// Use modules
use galatea::{editor, embedder, hoarder, parser_mod, processing, wanderer, watcher, resolver, debugger};

// Add Poem imports
use poem::{
    endpoint::StaticFilesEndpoint, get, handler, http::{Method, StatusCode}, listener::TcpListener,
    middleware::Cors, post, web::Data, web::Json, EndpointExt, Route, Server,
};
use serde::{Deserialize, Serialize};
use lsp_types::Uri;

// API request/response types
#[derive(Debug, Serialize, Deserialize)]
struct FindFilesRequest {
    dir: String,
    suffixes: Vec<String>,
    exclude_dirs: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FindFilesResponse {
    files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParseFileRequest {
    file_path: String,
    max_snippet_size: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParseDirectoryRequest {
    dir: String,
    suffixes: Vec<String>,
    exclude_dirs: Option<Vec<String>>,
    max_snippet_size: Option<usize>,
    granularity: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct QueryRequest {
    collection_name: String,
    query_text: String,
    model: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
    qdrant_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenerateEmbeddingsRequest {
    input_file: String,
    output_file: String,
    model: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenericApiResponse {
    success: bool,
    message: String,
    details: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpsertEmbeddingsRequest {
    input_file: String,
    collection_name: String,
    qdrant_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BuildIndexRequest {
    dir: String,
    suffixes: Vec<String>,
    exclude_dirs: Option<Vec<String>>,
    max_snippet_size: Option<usize>,
    granularity: Option<String>, // Will be parsed to enum processing::Granularity
    embedding_model: Option<String>,
    api_key: Option<String>,
    api_base: Option<String>,
    collection_name: String,
    qdrant_url: Option<String>,
}

// Editor API Request Structure
#[derive(Debug, Serialize, Deserialize)]
struct EditorCommandRequest {
    command: String, // "view", "create", "str_replace", "insert", "undo_edit"
    path: String,    // Required by schema, value might be ignored for 'undo_edit'
    file_text: Option<String>,
    insert_line: Option<usize>, // 1-indexed
    new_str: Option<String>,
    old_str: Option<String>,
    view_range: Option<Vec<isize>>, // [start_line, end_line], 1-indexed, end_line = -1 for to_end
}

// Editor API Response Structure
#[derive(Debug, Serialize, Deserialize)]
struct EditorCommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>, // For view command and now for all file modification operations
    #[serde(skip_serializing_if = "Option::is_none")]
    file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_at: Option<String>, // ISO timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_lines: Option<Vec<usize>>, // Line numbers affected
}

// Watcher API Payloads

// ESLint
#[derive(Debug, Serialize, Deserialize)]
struct LintRequest {
    // paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LintResponse {
    results: Vec<watcher::EslintResult>,
}

// New response struct for lint status
#[derive(Debug, Serialize, Deserialize)]
struct LintStatusResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    results: Option<Vec<watcher::EslintResult>>,
}

// Prettier Write
#[derive(Debug, Serialize, Deserialize)]
struct FormatWriteRequest {
}

// New response struct for format status
#[derive(Debug, Serialize, Deserialize)]
struct FormatStatusResponse {
    success: bool,
    message: String,
    // Optionally, could add a field for details if watcher::run_format ever returns more info
    // formatted_files: Option<Vec<String>>, 
}

// LSP - Goto Definition
#[derive(Debug, Serialize, Deserialize)]
struct GotoDefinitionApiRequest {
    uri: String,    // e.g., "file:///path/to/project/file.ts" or a partial path
    line: u32,      // 0-indexed
    character: u32, // 0-indexed
}

#[derive(Debug, Serialize, Deserialize)]
struct GotoDefinitionApiResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    locations: Option<lsp_types::GotoDefinitionResponse>,
}

// package.json API Response - uses watcher::PackageJsonData directly

#[handler]
async fn health() -> &'static str {
    "Galatea API is running"
}

#[handler]
async fn find_files(
    Json(req): Json<FindFilesRequest>,
) -> Result<Json<FindFilesResponse>, poem::Error> {
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
    
    match wanderer::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(found_files) => {
            let file_paths = found_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            Ok(Json(FindFilesResponse { files: file_paths }))
        }
        Err(e) => Err(poem::Error::from_string(
            format!("Error searching directory: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn parse_file(
    Json(req): Json<ParseFileRequest>,
) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    let file_path = match resolver::resolve_path(&req.file_path) {
        Ok(p) => p,
        Err(e) => return Err(poem::Error::from_string(e.to_string(), StatusCode::BAD_REQUEST)),
    };
    
    if !file_path.exists() {
        return Err(poem::Error::from_string(
            format!("File not found: {}", file_path.display()),
            StatusCode::NOT_FOUND,
        ));
    }
    
    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| {
            poem::Error::from_string("File has no extension", StatusCode::BAD_REQUEST)
        })?;
        
    let parse_result = match extension {
        "rs" => parser_mod::extract_rust_entities_from_file(&file_path, req.max_snippet_size),
        "ts" => parser_mod::extract_ts_entities(&file_path, false, req.max_snippet_size),
        "tsx" => parser_mod::extract_ts_entities(&file_path, true, req.max_snippet_size),
        _ => Err(anyhow::anyhow!("Unsupported file extension: {}", extension)),
    };
    
    match parse_result {
        Ok(entities) => Ok(Json(entities)),
        Err(e) => Err(poem::Error::from_string(
            format!("Error parsing file: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn parse_directory(
    Json(req): Json<ParseDirectoryRequest>,
) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
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
        Some("coarse") => processing::Granularity::Coarse,
        Some("medium") => processing::Granularity::Medium,
        _ => processing::Granularity::Fine,
    };
    
    let files_to_parse =
        match wanderer::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(files) => files,
            Err(e) => {
                return Err(poem::Error::from_string(
            format!("Error finding files: {}", e),
                    StatusCode::INTERNAL_SERVER_ERROR,
        ))
            }
    };
    
    if files_to_parse.is_empty() {
        return Ok(Json(Vec::new()));
    }
    
    let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
    for file_path in files_to_parse {
        let extension = file_path.extension().and_then(|ext| ext.to_str());
        let parse_result = match extension {
            Some("rs") => {
                parser_mod::extract_rust_entities_from_file(&file_path, req.max_snippet_size)
            }
            Some("ts") => parser_mod::extract_ts_entities(&file_path, false, req.max_snippet_size),
            Some("tsx") => parser_mod::extract_ts_entities(&file_path, true, req.max_snippet_size),
            _ => continue,
        };
        
        if let Ok(entities) = parse_result {
            all_entities.extend(entities);
        }
    }
    
    let final_entities =
        processing::post_process_entities(all_entities, granularity, req.max_snippet_size);
    Ok(Json(final_entities))
}

#[handler]
async fn query_collection(
    Json(req): Json<QueryRequest>,
) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    println!(
        "API query request for collection '{}': {}",
        req.collection_name, req.query_text
    );
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
            eprintln!("Error in API query_collection: {}", e);
            Err(poem::Error::from_string(
                format!("Error querying collection: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn generate_embeddings_api(
    Json(req): Json<GenerateEmbeddingsRequest>,
) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!(
        "API request to generate embeddings: input='{}', output='{}'",
        req.input_file, req.output_file
    );
    let input_path = std::path::PathBuf::from(&req.input_file);
    let output_path = std::path::PathBuf::from(&req.output_file);

    if !input_path.exists() {
        return Err(poem::Error::from_string(
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
            eprintln!("Error in API generate_embeddings_api: {}", e);
            Err(poem::Error::from_string(
                format!("Failed to generate embeddings: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn upsert_embeddings_api(
    Json(req): Json<UpsertEmbeddingsRequest>,
) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!(
        "API request to upsert embeddings: input='{}', collection='{}'", 
        req.input_file, req.collection_name
    );
    let input_path = std::path::PathBuf::from(&req.input_file);
    let qdrant_url = req.qdrant_url.as_deref().unwrap_or("http://localhost:6334");

    if !input_path.exists() {
        return Err(poem::Error::from_string(
            format!("Input file not found: {}", req.input_file),
            StatusCode::BAD_REQUEST,
        ));
    }

    // Ensure collection exists (optional, create_collection is idempotent)
    if let Err(e) = hoarder::create_collection(&req.collection_name, qdrant_url).await {
        eprintln!(
            "Error creating collection '{}' (it might already exist or Qdrant is down): {}",
            req.collection_name, e
        );
        // Decide if this is a hard error or if we can proceed assuming it might exist.
        // For now, let's proceed, as upsert will fail if collection doesn't exist and couldn't be created.
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
            eprintln!("Error in API upsert_embeddings_api: {}", e);
            Err(poem::Error::from_string(
                format!("Failed to upsert embeddings: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn build_index_api(
    Json(req): Json<BuildIndexRequest>,
) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!(
        "API request to build index: dir='{}', collection='{}'",
        req.dir, req.collection_name
    );
    let qdrant_url_for_spawn = req
        .qdrant_url
        .clone()
        .unwrap_or_else(|| "http://localhost:6334".to_string());

    // Clone all necessary data from req to move into the spawned task
    let dir_clone = req.dir.clone();
    let suffixes_clone: Vec<String> = req.suffixes.clone(); // Clone into Vec<String>
    let exclude_dirs_clone = req.exclude_dirs.clone();
    let max_snippet_size_clone = req.max_snippet_size;
    let granularity_str_clone = req.granularity.clone();
    let embedding_model_clone = req.embedding_model.clone();
    let api_key_clone = req.api_key.clone();
    let api_base_clone = req.api_base.clone();
    let collection_name_clone = req.collection_name.clone();

    // For long-running tasks like this, it's better to spawn a new task
    // so the HTTP request can return quickly.
    tokio::spawn(async move {
        let qdrant_url_inner = qdrant_url_for_spawn; // Use the URL captured for the spawn
        let dir_path = std::path::PathBuf::from(dir_clone); // Use cloned data
        let suffixes_ref: Vec<&str> = suffixes_clone.iter().map(|s| s.as_str()).collect();
        
        let default_exclude_dirs = vec![
            "node_modules".to_string(),
            "target".to_string(),
            "dist".to_string(),
            "build".to_string(),
            ".git".to_string(),
            ".vscode".to_string(),
            ".idea".to_string(),
        ];
        let exclude_dirs_owned = exclude_dirs_clone.unwrap_or(default_exclude_dirs);
        let exclude_dirs_ref: Vec<&str> = exclude_dirs_owned.iter().map(|s| s.as_str()).collect();
        
        let granularity = match granularity_str_clone.as_deref() {
            // Use cloned data
            Some("coarse") => processing::Granularity::Coarse,
            Some("medium") => processing::Granularity::Medium,
            Some("fine") => processing::Granularity::Fine,
            _ => processing::Granularity::Fine, // Default to fine
        };

        println!("--- Starting Full Index Build (API Triggered) ---");

        println!("[1/4] Finding files...");
        let files_to_parse =
            match wanderer::find_files_by_extensions(&dir_path, &suffixes_ref, &exclude_dirs_ref) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("BuildIndex API: Wander step failed: {}", e);
                return;
            }
        };
        if files_to_parse.is_empty() { 
            println!("BuildIndex API: No matching files found. Index build cancelled."); 
            return; 
        }
        println!("BuildIndex API: Found {} files.", files_to_parse.len());

        println!("[2/4] Parsing files...");
        let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
        for file_path in files_to_parse {
            let extension = file_path.extension().and_then(|ext| ext.to_str());
            let parse_result = match extension {
                Some("rs") => {
                    parser_mod::extract_rust_entities_from_file(&file_path, max_snippet_size_clone)
                }
                Some("ts") => {
                    parser_mod::extract_ts_entities(&file_path, false, max_snippet_size_clone)
                }
                Some("tsx") => {
                    parser_mod::extract_ts_entities(&file_path, true, max_snippet_size_clone)
                }
                _ => {
                    continue;
                }
            };
            match parse_result {
                Ok(entities) => all_entities.extend(entities),
                Err(e) => eprintln!(
                    "BuildIndex API: Error parsing {}: {}. Skipping.",
                    file_path.display(),
                    e
                ),
            }
        }
        println!(
            "BuildIndex API: Parsed {} initial entities.",
            all_entities.len()
        );

        println!(
            "[2b/4] Post-processing entities (granularity: {:?})...",
            granularity
        );
        let processed_entities =
            processing::post_process_entities(all_entities, granularity, max_snippet_size_clone);
        println!(
            "BuildIndex API: {} entities after post-processing.",
            processed_entities.len()
        );
        if processed_entities.is_empty() { 
            println!("BuildIndex API: No entities after processing. Index build cancelled."); 
            return; 
        }

        println!("[3/4] Generating embeddings...");
        let entities_with_embeddings = match embedder::generate_embeddings_for_vec(
            processed_entities,
            embedding_model_clone, // Use cloned data
            api_key_clone,         // Use cloned data
            api_base_clone,        // Use cloned data
        )
        .await
        {
            Ok(entities) => entities,
            Err(e) => {
                eprintln!("BuildIndex API: Embedding step failed: {}", e);
                return;
            }
        };
        println!("BuildIndex API: Embeddings generated.");
        if entities_with_embeddings
            .iter()
            .all(|e| e.embedding.is_none())
        {
            println!("BuildIndex API: Warning: No entities had embeddings generated successfully.");
        }

        println!(
            "[4/4] Storing embeddings in Qdrant collection '{}'...",
            collection_name_clone
        ); // Use cloned data
        if let Err(e) = hoarder::create_collection(&collection_name_clone, &qdrant_url_inner).await
        {
            // Use cloned data
            eprintln!(
                "BuildIndex API: Failed to ensure Qdrant collection exists: {}",
                e
            );
            return;
        }
        if let Err(e) = hoarder::upsert_entities_from_vec(
            &collection_name_clone,
            entities_with_embeddings,
            &qdrant_url_inner,
        )
        .await
        {
            // Use cloned data
            eprintln!(
                "BuildIndex API: Upserting embeddings to Qdrant failed: {}",
                e
            );
            return;
        }

        println!("--- Index Build Complete (API Triggered) ---");
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

#[handler]
async fn editor_command_api(
    editor_data: Data<&Arc<Mutex<editor::Editor>>>,
    Json(req): Json<EditorCommandRequest>,
) -> Result<Json<EditorCommandResponse>, poem::Error> {
    let command_type = match req.command.as_str() {
        "view" => editor::CommandType::View,
        "create" => editor::CommandType::Create,
        "str_replace" => editor::CommandType::StrReplace,
        "insert" => editor::CommandType::Insert,
        "undo_edit" => editor::CommandType::UndoEdit,
        _ => {
            return Err(poem::Error::from_string(
                format!("Invalid command type: {}", req.command),
                StatusCode::BAD_REQUEST,
            ))
        }
    };

    // Resolve the path, except for 'undo_edit' which might not need a path
    // or its path meaning is different (e.g. last edited file, handled by Editor state).
    // For simplicity, we'll resolve it. If Editor can handle relative/unresolved paths for undo context, that's fine.
    let resolved_req_path = match resolver::resolve_path(&req.path) {
        Ok(p) => p,
        Err(e) => return Err(poem::Error::from_string(e.to_string(), StatusCode::BAD_REQUEST)),
    };

    // Ensure the path exists for commands that operate on existing files, if necessary.
    // 'create' doesn't need it to exist. 'view', 'str_replace', 'insert' likely do.
    // 'undo_edit' might operate on a path that was just deleted, or a conceptual path.
    if command_type != editor::CommandType::Create && 
       command_type != editor::CommandType::UndoEdit && // Assuming undo might not need path to exist right now
       !resolved_req_path.exists() {
        return Err(poem::Error::from_string(
            format!("File not found at resolved path: {}", resolved_req_path.display()),
            StatusCode::NOT_FOUND,
        ));
    }

    // Get current timestamp
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    // Path is required by EditorArgs and API schema
    let editor_args = editor::EditorArgs {
        command: command_type,
        path: resolved_req_path.to_string_lossy().into_owned(), // Use resolved path
        file_text: req.file_text.clone(),
        insert_line: req.insert_line,
        new_str: req.new_str.clone(),
        old_str: req.old_str.clone(),
        view_range: req.view_range.clone(),
    };

    let mut editor_guard = editor_data.0.lock().await;
    match editor::handle_command(&mut editor_guard, editor_args) {
        Ok(Some(content)) => {
            // View command already returns content
            Ok(Json(EditorCommandResponse {
                success: true,
                message: Some(format!("Command '{}' executed successfully.", req.command)),
                content: Some(content.clone()),
                file_path: Some(req.path.clone()),
                operation: Some(req.command.clone()),
                line_count: Some(content.lines().count()),
                modified_at: Some(timestamp),
                modified_lines: None,
            }))
        },
        Ok(None) => {
            // Non-view commands now need to fetch content to return it
            let mut response = EditorCommandResponse {
                success: true,
                message: Some(format!("Command '{}' executed successfully.", req.command)),
                content: None,
                file_path: Some(req.path.clone()),
                operation: Some(req.command.clone()),
                modified_at: Some(timestamp),
                line_count: Some(0), // Default to 0 lines until we get actual content
                modified_lines: None,
            };
            
            // For create, str_replace, insert - fetch the updated content
            if req.command == "create" || req.command == "str_replace" || req.command == "insert" || req.command == "undo_edit" {
                // Create a view command to fetch the content, using the resolved path of the original request
                let view_args = editor::EditorArgs {
                    command: editor::CommandType::View,
                    path: resolved_req_path.to_string_lossy().into_owned(), // Use resolved path
                    file_text: None,
                    insert_line: None,
                    new_str: None,
                    old_str: None,
                    view_range: None, // view the whole file
                };
                
                if let Ok(Some(updated_content)) = editor::handle_command(&mut editor_guard, view_args) {
                    response.content = Some(updated_content.clone());
                    response.line_count = Some(updated_content.lines().count());
                    
                    // For str_replace, try to estimate affected lines
                    if req.command == "str_replace" && req.old_str.is_some() {
                        // Simple approach: count line breaks in old_str to estimate affected lines
                        // For a more precise approach, we would need to track changes in the editor module
                        if let Some(old_str) = &req.old_str {
                            let line_count = old_str.lines().count();
                            if line_count > 0 && line_count < 100 { // Reasonable limit
                                response.modified_lines = Some((1..=line_count).collect());
                            }
                        }
                    }
                    
                    // For insert, we know the affected line
                    if req.command == "insert" && req.insert_line.is_some() {
                        response.modified_lines = Some(vec![req.insert_line.unwrap()]);
                    }
                }
            }
            
            Ok(Json(response))
        },
        Err(e) => Err(poem::Error::from_string(
            e,
            StatusCode::BAD_REQUEST,
        )),
    }
}

#[handler]
async fn lint_api(Json(_req): Json<LintRequest>) -> Result<Json<LintStatusResponse>, poem::Error> {
    match watcher::run_eslint().await {
        Ok(results) => {
            if results.is_empty() {
                Ok(Json(LintStatusResponse {
                    success: true,
                    message: Some("No lint errors found.".to_string()),
                    results: None,
                }))
            } else {
                Ok(Json(LintStatusResponse {
                    success: true,
                    message: Some(format!("{} lint issue(s) found.", results.len())),
                    results: Some(results),
                }))
            }
        }
        Err(e) => Err(poem::Error::from_string(
            format!("Error running ESLint: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn format_api(
    Json(_req): Json<FormatWriteRequest>,
) -> Result<Json<FormatStatusResponse>, poem::Error> {
    match watcher::run_format().await {
        Ok(_formatted_files) => {
            Ok(Json(FormatStatusResponse {
                success: true,
                message: "Formatting process completed successfully.".to_string(),
            }))
        }
        Err(e) => Err(poem::Error::from_string(
            format!("Error formatting with Prettier: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn lsp_goto_definition_api(
    lsp_client_data: Data<&Arc<Mutex<watcher::LspClient>>>,
    Json(req): Json<GotoDefinitionApiRequest>,
) -> Result<Json<GotoDefinitionApiResponse>, poem::Error> {
    // 1. Resolve the input string (which could be a URI string or a partial path)
    //    to a canonical PathBuf using the resolver module.
    let resolved_file_path: std::path::PathBuf = match resolver::resolve_path(&req.uri) {
        Ok(p) => p,
        Err(e) => { // e is anyhow::Error
            return Err(poem::Error::from_string(
                format!(
                    "Failed to resolve input path/URI '{}' to a project file: {}",
                    req.uri,
                    e.to_string()
                ),
                StatusCode::BAD_REQUEST,
            ));
        }
    };

    let file_uri = match resolver::resolve_path_to_uri(&req.uri) {
        Ok(uri) => uri,
        Err(e) => {
            return Err(poem::Error::from_string(
                format!("Failed to resolve input path/URI '{}' to a project file: {}", req.uri, e.to_string()),
                StatusCode::BAD_REQUEST,
            ));
        }
    };
    
    let file_content = match std::fs::read_to_string(&resolved_file_path) {
        Ok(content) => content,
        Err(e) => {
            return Err(poem::Error::from_string(
                format!(
                    "Failed to read file for LSP didOpen '{}': {}",
                    resolved_file_path.display(),
                    e
                ),
                StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    };

    let position = lsp_types::Position {
        line: req.line,
        character: req.character,
    };

    let mut client_guard = lsp_client_data.0.lock().await;

    let language_id = resolved_file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or_else(
            || "plaintext".to_string(),
            |ext| match ext {
                "ts" => "typescript".to_string(),
                "tsx" => "typescriptreact".to_string(),
                "js" => "javascript".to_string(),
                "jsx" => "javascriptreact".to_string(),
                "json" => "json".to_string(),
                _ => "plaintext".to_string(),
            },
        );

    if let Err(e) = client_guard
        .notify_did_open(file_uri.clone(), &language_id, 0, file_content)
        .await
    {
        eprintln!(
            "LSP notify_did_open failed (continuing to goto_definition): {}",
            e
        );
    }

    match client_guard.goto_definition(file_uri, position).await {
        Ok(locations) => Ok(Json(GotoDefinitionApiResponse { locations })),
        Err(e) => Err(poem::Error::from_string(
            format!("LSP goto_definition failed: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

async fn start_server(host: &str, port: u16) -> Result<()> {
    let editor_state = Arc::new(Mutex::new(editor::Editor::new()));

    let lsp_client = match watcher::LspClient::new().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!(
                "Failed to initialize LSP client: {}. LSP features will be unavailable.",
                e
            );
            panic!("LSP Client initialization failed: {}", e);
        }
    };

    let project_root_path =
        resolver::get_project_root() // Use resolver::get_project_root
            .map_err(|e| anyhow::anyhow!("Failed to get project root for LSP: {}", e))?;
    
    // Convert project_root_path to an LspUri for the LSP client initialize method
    let root_uri: Uri = resolver::resolve_path_to_uri(&project_root_path)
        .map_err(|e| anyhow::anyhow!("Failed to resolve project root path {} to a URI: {}", project_root_path.display(), e))?;

    let client_capabilities = lsp_types::ClientCapabilities::default();
    let mut lsp_client_instance = lsp_client;

    if let Err(e) = lsp_client_instance
        .initialize(root_uri.clone(), client_capabilities.clone())
        .await
    {
        eprintln!(
            "LSP server initialization failed: {}. GotoDefinition might not work.",
            e
        );
    }

    let lsp_client_state = Arc::new(Mutex::new(lsp_client_instance));

    let api_app = Route::new()
        .at("/health", get(health))
        .at("/find-files", post(find_files))
        .at("/parse-file", post(parse_file))
        .at("/parse-directory", post(parse_directory))
        .at("/query", post(query_collection))
        .at("/generate-embeddings", post(generate_embeddings_api))
        .at("/upsert-embeddings", post(upsert_embeddings_api))
        .at("/build-index", post(build_index_api))
        .at("/editor", post(editor_command_api))
        .at("/lint", post(lint_api))
        .at("/format-write", post(format_api))
        .at("/lsp/goto-definition", post(lsp_goto_definition_api))
        .with(
            Cors::new()
            .allow_credentials(true)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers(["Content-Type", "Authorization"]),
        );

    // Static files endpoint that serves the React app from ./dist
    // Use correct path configuration to handle absolute paths in the built React app
    let static_files = StaticFilesEndpoint::new("./dist").index_file("index.html");

    let app = Route::new()
        .nest("/api", api_app)
        // Serve static files directly from the root path
        .nest("/", static_files);

    println!("Starting Galatea server on {}:{}", host, port);
    println!("Serving API at /api and static files from ./dist at / ");
    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        .run(app.data(editor_state).data(lsp_client_state)) // Add lsp_client_state
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    // Verify and set up the project environment using the debugger module
    if let Err(e) = debugger::verify_and_setup_project().await {
        eprintln!("Failed to verify and set up the project environment: {:?}", e);
        eprintln!("Please check the errors above. Server will not start.");
        return Err(e); 
    }
    println!("Project environment verified and set up successfully.");
    
    // Default server settings
    let host = "0.0.0.0";
    let port = 3051;
    
    println!("Starting server with default settings on {}:{}...", host, port);
    start_server(host, port).await
}
