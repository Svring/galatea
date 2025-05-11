use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

// Use modules
use galatea::{editor, embedder, hoarder, parser_mod, processing, wanderer, watcher};

// Add Poem imports
use poem::{
    endpoint::StaticFilesEndpoint, get, handler, http::Method, listener::TcpListener,
    middleware::Cors, post, web::Data, web::Json, EndpointExt, Route, Server,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Find files recursively in a directory by suffix
    FindFiles {
        #[arg(long, required = true)]
        dir: PathBuf,
        #[arg(long, required = true, value_delimiter = ',')]
        suffixes: Vec<String>,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "node_modules,target,dist,build,.git,.vscode,.idea"
        )]
        exclude_dirs: Vec<String>,
    },
    /// Parse files in a directory and print results as JSON to stdout
    ParseDirectory {
        #[arg(long, required = true, default_value = ".")]
        dir: PathBuf,
        #[arg(long, required = true, value_delimiter = ',')]
        suffixes: Vec<String>,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "node_modules,target,dist,build,.git,.vscode,.idea"
        )]
        exclude_dirs: Vec<String>,
        #[arg(long)]
        max_snippet_size: Option<usize>,
        #[arg(long, value_enum, default_value_t = processing::Granularity::Fine)]
        granularity: processing::Granularity,
    },
    /// Generate embeddings for entities in an index JSON file
    GenerateEmbeddings {
        #[arg(long, required = true)]
        input_file: PathBuf,
        #[arg(long, required = true)]
        output_file: PathBuf,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_base: Option<String>,
    },
    /// Upsert embeddings from an index JSON file into a Qdrant collection
    UpsertEmbeddings {
        #[arg(long, required = true)]
        input_file: PathBuf,
        #[arg(long, required = true)]
        collection_name: String,
        #[arg(long, default_value = "http://localhost:6334")]
        qdrant_url: String,
    },
    /// Query a Qdrant collection with a text query
    Query {
        #[arg(long, required = true)]
        collection_name: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_base: Option<String>,
        #[arg(long, default_value = "http://localhost:6334")]
        qdrant_url: String,
        query_text: String,
    },
    /// Build a full index: Find files -> Parse -> Embed -> Store in Qdrant
    BuildIndex {
        // Wanderer args
        #[arg(long, required = true, default_value = ".")]
        dir: PathBuf,
        #[arg(long, required = true, value_delimiter = ',')]
        suffixes: Vec<String>,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "node_modules,target,dist,build,.git,.vscode,.idea"
        )]
        exclude_dirs: Vec<String>,
        // Processing args
        #[arg(long)]
        max_snippet_size: Option<usize>,
        #[arg(long, value_enum, default_value_t = processing::Granularity::Fine)]
        granularity: processing::Granularity,
        // Embedder args
        #[arg(long)]
        embedding_model: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_base: Option<String>,
        // Hoarder args
        #[arg(long, required = true)]
        collection_name: String,
        #[arg(long, default_value = "http://localhost:6334")]
        qdrant_url: String,
    },
    /// Start the Galatea web API server
    StartServer {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value = "3051")]
        port: u16,
    },
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>, // For view command
}

// Watcher API Payloads

// ESLint
#[derive(Debug, Serialize, Deserialize)]
struct LintRequest {
    paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LintResponse {
    results: Vec<watcher::EslintResult>,
}

// Prettier Check
#[derive(Debug, Serialize, Deserialize)]
struct FormatCheckRequest {
    patterns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FormatCheckResponse {
    unformatted_files: Vec<String>,
}

// Prettier Write
#[derive(Debug, Serialize, Deserialize)]
struct FormatWriteRequest {
    patterns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FormatWriteResponse {
    formatted_files: Vec<String>,
}

// LSP - Goto Definition
#[derive(Debug, Serialize, Deserialize)]
struct GotoDefinitionApiRequest {
    uri: String,    // e.g., "file:///path/to/project/file.ts"
    line: u32,      // 0-indexed
    character: u32, // 0-indexed
}

#[derive(Debug, Serialize, Deserialize)]
struct GotoDefinitionApiResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    locations: Option<lsp_types::GotoDefinitionResponse>,
}

#[handler]
async fn health() -> &'static str {
    "Galatea API is running"
}

#[handler]
async fn find_files(
    Json(req): Json<FindFilesRequest>,
) -> Result<Json<FindFilesResponse>, poem::Error> {
    let dir = PathBuf::from(&req.dir);
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

    match wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(found_files) => {
            let file_paths = found_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            Ok(Json(FindFilesResponse { files: file_paths }))
        }
        Err(e) => Err(poem::Error::from_string(
            format!("Error searching directory: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn parse_file(
    Json(req): Json<ParseFileRequest>,
) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    let file_path = PathBuf::from(&req.file_path);

    if !file_path.exists() {
        return Err(poem::Error::from_string(
            format!("File not found: {}", file_path.display()),
            poem::http::StatusCode::NOT_FOUND,
        ));
    }

    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| {
            poem::Error::from_string("File has no extension", poem::http::StatusCode::BAD_REQUEST)
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
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn parse_directory(
    Json(req): Json<ParseDirectoryRequest>,
) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    let dir = PathBuf::from(&req.dir);
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
        match wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref) {
            Ok(files) => files,
            Err(e) => {
                return Err(poem::Error::from_string(
                    format!("Error finding files: {}", e),
                    poem::http::StatusCode::INTERNAL_SERVER_ERROR,
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
                poem::http::StatusCode::INTERNAL_SERVER_ERROR,
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
    let input_path = PathBuf::from(&req.input_file);
    let output_path = PathBuf::from(&req.output_file);

    if !input_path.exists() {
        return Err(poem::Error::from_string(
            format!("Input file not found: {}", req.input_file),
            poem::http::StatusCode::BAD_REQUEST,
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
            message: "Embeddings generated successfully.".to_string(),
            details: Some(format!("Output written to {}", req.output_file)),
        })),
        Err(e) => {
            eprintln!("Error in API generate_embeddings_api: {}", e);
            Err(poem::Error::from_string(
                format!("Failed to generate embeddings: {}", e),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR,
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
    let input_path = PathBuf::from(&req.input_file);
    let qdrant_url = req.qdrant_url.as_deref().unwrap_or("http://localhost:6334");

    if !input_path.exists() {
        return Err(poem::Error::from_string(
            format!("Input file not found: {}", req.input_file),
            poem::http::StatusCode::BAD_REQUEST,
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
                poem::http::StatusCode::INTERNAL_SERVER_ERROR,
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
        let dir_path = PathBuf::from(dir_clone); // Use cloned data
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
            match wanderer::find_files_by_suffix(&dir_path, &suffixes_ref, &exclude_dirs_ref) {
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
                poem::http::StatusCode::BAD_REQUEST,
            ))
        }
    };

    // Path is required by EditorArgs and API schema
    let editor_args = editor::EditorArgs {
        command: command_type,
        path: req.path.clone(), // path is always required by schema
        file_text: req.file_text.clone(),
        insert_line: req.insert_line,
        new_str: req.new_str.clone(),
        old_str: req.old_str.clone(),
        view_range: req.view_range.clone(),
    };

    let mut editor_guard = editor_data.0.lock().await;
    match editor::handle_command(&mut editor_guard, editor_args) {
        Ok(Some(content)) => Ok(Json(EditorCommandResponse {
            message: None,
            content: Some(content),
        })),
        Ok(None) => Ok(Json(EditorCommandResponse {
            message: Some(format!("Command '{}' executed successfully.", req.command)),
            content: None,
        })),
        Err(e) => Err(poem::Error::from_string(
            e,
            poem::http::StatusCode::BAD_REQUEST,
        )),
    }
}

#[handler]
async fn lint_files_api(Json(req): Json<LintRequest>) -> Result<Json<LintResponse>, poem::Error> {
    match watcher::run_eslint(&req.paths).await {
        Ok(results) => Ok(Json(LintResponse { results })),
        Err(e) => Err(poem::Error::from_string(
            format!("Error running ESLint: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn format_check_api(
    Json(req): Json<FormatCheckRequest>,
) -> Result<Json<FormatCheckResponse>, poem::Error> {
    match watcher::check_prettier(&req.patterns).await {
        Ok(unformatted_files) => Ok(Json(FormatCheckResponse { unformatted_files })),
        Err(e) => Err(poem::Error::from_string(
            format!("Error checking with Prettier: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn format_write_api(
    Json(req): Json<FormatWriteRequest>,
) -> Result<Json<FormatWriteResponse>, poem::Error> {
    match watcher::format_with_prettier(&req.patterns).await {
        Ok(formatted_files) => Ok(Json(FormatWriteResponse { formatted_files })),
        Err(e) => Err(poem::Error::from_string(
            format!("Error formatting with Prettier: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn lsp_goto_definition_api(
    lsp_client_data: Data<&Arc<Mutex<watcher::LspClient>>>,
    Json(req): Json<GotoDefinitionApiRequest>,
) -> Result<Json<GotoDefinitionApiResponse>, poem::Error> {
    let file_uri = match req.uri.parse::<lsp_types::Uri>() {
        Ok(uri) => uri,
        Err(e) => {
            return Err(poem::Error::from_string(
                format!("Invalid URI format '{}': {}", req.uri, e),
                poem::http::StatusCode::BAD_REQUEST,
            ));
        }
    };

    let mut file_path_str = file_uri.path().to_string(); // Convert URI path component to String
    #[cfg(windows)]
    {
        // On Windows, Uri path might start with a leading `/` for a path like `C:\...`,
        // e.g., `/c:/Users/...`. We need to strip it if present for PathBuf.
        if file_path_str.starts_with('/') && file_path_str.chars().nth(2) == Some(':') {
            file_path_str = file_path_str[1..].to_string();
        }
    }
    let file_path = PathBuf::from(file_path_str);

    let file_content = match std::fs::read_to_string(&file_path) {
        Ok(content) => content,
        Err(e) => {
            return Err(poem::Error::from_string(
                format!(
                    "Failed to read file for LSP didOpen '{}': {}",
                    file_path.display(),
                    e
                ),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    };

    let position = lsp_types::Position {
        line: req.line,
        character: req.character,
    };

    let mut client_guard = lsp_client_data.0.lock().await;

    // Notify didOpen before gotoDefinition
    // Determine language ID based on extension - simplistic, could be improved
    let language_id = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or_else(
            || "plaintext".to_string(), // Default if no extension
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
        // Log this error but proceed to goto_definition anyway, as some servers might allow it
        // or the file might have been opened by another means.
        eprintln!(
            "LSP notify_did_open failed (continuing to goto_definition): {}",
            e
        );
    }

    match client_guard.goto_definition(file_uri, position).await {
        Ok(locations) => Ok(Json(GotoDefinitionApiResponse { locations })),
        Err(e) => Err(poem::Error::from_string(
            format!("LSP goto_definition failed: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

async fn start_server(host: String, port: u16) -> Result<()> {
    let editor_state = Arc::new(Mutex::new(editor::Editor::new()));

    // Initialize LSP Client
    // This will attempt to install typescript-language-server if not present in ./project
    // and then spawn it.
    let lsp_client = match watcher::LspClient::new().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!(
                "Failed to initialize LSP client: {}. LSP features will be unavailable.",
                e
            );
            // We could panic here, or proceed without LSP. For now, proceed.
            // A more robust solution might involve a placeholder client or retries.
            // This is a simplified approach for now.
            // Creating a dummy or non-functional client if new() fails would be complex
            // for just satisfying type constraints if it was an Option or Result.
            // For now, if it fails, the server starts but /api/lsp calls would likely fail
            // if the client isn't properly shared or if this unwrap is hit.
            // A better way is to make lsp_client_state an Option<Arc<Mutex<...>>>
            // For now, let's assume it must succeed or we panic.
            panic!("LSP Client initialization failed: {}", e);
        }
    };
    // Initialize the LSP server with client capabilities
    // This needs to be done once after the LspClient is created.
    // The root URI should point to the './project' directory.
    let project_root_path =
        watcher::get_project_root() // Using watcher's helper
            .map_err(|e| anyhow::anyhow!("Failed to get project root for LSP: {}", e))?;
    let root_uri_str = format!("file://{}", project_root_path.to_string_lossy());
    let root_uri = root_uri_str
        .parse::<lsp_types::Uri>() // Use FromStr trait
        .map_err(|e| anyhow::anyhow!("Failed to parse project root URI for LSP: {}", e))?;

    // Define basic client capabilities
    let client_capabilities = lsp_types::ClientCapabilities::default(); // Basic capabilities

    // Perform initialization
    // We need mutable access to lsp_client here.
    // If we wrap it in Arc<Mutex<>> too early, this becomes tricky.
    // Let's initialize it before wrapping for server state.

    let mut lsp_client_instance = lsp_client; // Take ownership to make it mutable

    if let Err(e) = lsp_client_instance
        .initialize(root_uri.clone(), client_capabilities.clone())
        .await
    {
        eprintln!(
            "LSP server initialization failed: {}. GotoDefinition might not work.",
            e
        );
        // Decide if server should still start. For now, it will.
    }

    // Now wrap it for sharing across API handlers
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
        .at("/lint", post(lint_files_api))
        .at("/format-check", post(format_check_api))
        .at("/format-write", post(format_write_api))
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
    let args = Args::parse();

    // Use a default command if none was provided
    match args.command {
        Some(cmd) => match cmd {
            Commands::FindFiles {
                dir,
                suffixes,
                exclude_dirs,
            } => {
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
                println!(
                    "Searching for files with suffixes [{}] in: {} (excluding: [{}])",
                    suffixes.join(", "),
                    dir.display(),
                    exclude_dirs.join(", ")
                );
                match wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref) {
                    Ok(found_files) => {
                        if found_files.is_empty() {
                            println!("No matching files found.");
                        } else {
                            println!("Found files:");
                            for file_path in found_files {
                                println!("  {}", file_path.display());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error searching directory {}: {}", dir.display(), e);
                        return Err(e);
                    }
                }
            }
            Commands::ParseDirectory {
                dir,
                suffixes,
                exclude_dirs,
                max_snippet_size,
                granularity,
            } => {
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();

                println!(
                    "Starting parsing in '{}' for suffixes: {:?} (excluding: {:?})",
                    dir.display(),
                    suffixes,
                    exclude_dirs
                );
                let files_to_parse =
                    wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref)?;
                if files_to_parse.is_empty() {
                    println!("No matching files found to parse.");
                    return Ok(());
                }
                println!("Found {} files to process.", files_to_parse.len());

                let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
                for file_path in files_to_parse {
                    println!("  Parsing: {}", file_path.display());
                    let extension = file_path.extension().and_then(|ext| ext.to_str());
                    let parse_result = match extension {
                        Some("rs") => parser_mod::extract_rust_entities_from_file(
                            &file_path,
                            max_snippet_size,
                        ),
                        Some("ts") => {
                            parser_mod::extract_ts_entities(&file_path, false, max_snippet_size)
                        }
                        Some("tsx") => {
                            parser_mod::extract_ts_entities(&file_path, true, max_snippet_size)
                        }
                        _ => {
                            println!("  -> Skipping file with unsupported extension.");
                            continue;
                        }
                    };
                    match parse_result {
                        Ok(entities) => {
                            println!("    -> Extracted {} entities.", entities.len());
                            all_entities.extend(entities);
                        }
                        Err(e) => {
                            eprintln!(
                                "    -> Error parsing {}: {}. Skipping file.",
                                file_path.display(),
                                e
                            );
                        }
                    }
                }
                println!(
                    "Total entities extracted before post-processing: {}",
                    all_entities.len()
                );

                let final_entities =
                    processing::post_process_entities(all_entities, granularity, max_snippet_size);
                println!(
                    "Total entities after post-processing: {}",
                    final_entities.len()
                );

                let json_output = serde_json::to_string_pretty(&final_entities)?;
                println!("\n--- Start JSON Output ---");
                println!("{}", json_output);
                println!("--- End JSON Output ---");
            }
            Commands::GenerateEmbeddings {
                input_file,
                output_file,
                model,
                api_key,
                api_base,
            } => {
                if let Err(e) = embedder::generate_embeddings_for_index(
                    &input_file,
                    &output_file,
                    model,
                    api_key,
                    api_base,
                )
                .await
                {
                    eprintln!("Failed to generate embeddings: {}", e);
                    return Err(e);
                }
            }
            Commands::UpsertEmbeddings {
                input_file,
                collection_name,
                qdrant_url,
            } => {
                hoarder::create_collection(&collection_name, &qdrant_url).await?;
                if let Err(e) =
                    hoarder::upsert_embeddings(&collection_name, &input_file, &qdrant_url).await
                {
                    eprintln!("Failed to upsert embeddings from file: {}", e);
                    return Err(e);
                }
                println!("Upsert from file complete.");
            }
            Commands::Query {
                collection_name,
                query_text,
                model,
                api_key,
                api_base,
                qdrant_url,
            } => {
                println!(
                    "Querying collection '{}' with: \"{}\" using Qdrant at {}",
                    collection_name, query_text, qdrant_url
                );
                if let Err(e) = hoarder::query(
                    &collection_name,
                    &query_text,
                    model,
                    api_key,
                    api_base,
                    &qdrant_url,
                )
                .await
                {
                    eprintln!("Failed to execute query: {}", e);
                    return Err(e);
                }
            }
            Commands::BuildIndex {
                dir,
                suffixes,
                exclude_dirs,
                granularity,
                max_snippet_size,
                embedding_model,
                api_key,
                api_base,
                collection_name,
                qdrant_url,
            } => {
                println!(
                    "--- Starting Full Index Build (Qdrant at: {}) ---",
                    qdrant_url
                );

                println!("[1/4] Finding files...");
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
                let files_to_parse =
                    wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref)
                        .with_context(|| {
                            format!("Wander step failed in dir '{}'", dir.display())
                        })?;
                if files_to_parse.is_empty() {
                    println!("No matching files found. Index build cancelled.");
                    return Ok(());
                }
                println!("Found {} files.", files_to_parse.len());

                println!("[2/4] Parsing files...");
                let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
                for file_path in files_to_parse {
                    let extension = file_path.extension().and_then(|ext| ext.to_str());
                    let parse_result = match extension {
                        Some("rs") => parser_mod::extract_rust_entities_from_file(
                            &file_path,
                            max_snippet_size,
                        ),
                        Some("ts") => {
                            parser_mod::extract_ts_entities(&file_path, false, max_snippet_size)
                        }
                        Some("tsx") => {
                            parser_mod::extract_ts_entities(&file_path, true, max_snippet_size)
                        }
                        _ => {
                            continue;
                        }
                    };
                    match parse_result {
                        Ok(entities) => all_entities.extend(entities),
                        Err(e) => eprintln!(
                            "    -> Error parsing {}: {}. Skipping.",
                            file_path.display(),
                            e
                        ),
                    }
                }
                println!("Parsed {} initial entities.", all_entities.len());

                println!(
                    "[2b/4] Post-processing entities (granularity: {:?})...",
                    granularity
                );
                let processed_entities =
                    processing::post_process_entities(all_entities, granularity, max_snippet_size);
                println!(
                    "{} entities after post-processing.",
                    processed_entities.len()
                );
                if processed_entities.is_empty() {
                    println!("No entities after processing. Index build cancelled.");
                    return Ok(());
                }

                println!("[3/4] Generating embeddings...");
                let entities_with_embeddings = embedder::generate_embeddings_for_vec(
                    processed_entities,
                    embedding_model.clone(),
                    api_key.clone(),
                    api_base.clone(),
                )
                .await
                .context("Embedding step failed")?;
                println!("Embeddings generated.");
                if entities_with_embeddings
                    .iter()
                    .all(|e| e.embedding.is_none())
                {
                    println!("Warning: No entities had embeddings generated successfully. Check API key/quota/connectivity.");
                    println!("Index build finished without storing to Qdrant.");
                    return Ok(());
                }

                println!(
                    "[4/4] Storing embeddings in Qdrant collection '{}'...",
                    collection_name
                );
                hoarder::create_collection(&collection_name, &qdrant_url)
                    .await
                    .context("Failed to ensure Qdrant collection exists")?;
                hoarder::upsert_entities_from_vec(
                    &collection_name,
                    entities_with_embeddings,
                    &qdrant_url,
                )
                .await
                .context("Upserting embeddings to Qdrant failed")?;

                println!("--- Index Build Complete ---");
            }
            Commands::StartServer { host, port } => {
                start_server(host, port).await?;
            }
        },
        None => {
            // Default to starting the server with default parameters when no command is provided
            println!("No command specified, starting server with default settings on port 3051...");
            // Use default host and new default port 3051
            start_server("0.0.0.0".to_string(), 3051).await?;
        }
    }

    Ok(())
}
