use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

// Use modules
use galatea::{embedder, hoarder, parser_mod, processing, wanderer, editor};

// Add Poem imports
use poem::{
    endpoint::StaticFilesEndpoint,
    get, handler, post, 
    listener::TcpListener, 
    middleware::Cors,
    web::Json, 
    EndpointExt, Route, Server,
    http::Method,
    web::Data,
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

#[handler]
async fn health() -> &'static str {
    "Galatea API is running"
}

#[handler]
async fn find_files(Json(req): Json<FindFilesRequest>) -> Result<Json<FindFilesResponse>, poem::Error> {
    let dir = PathBuf::from(&req.dir);
    let suffixes_ref: Vec<&str> = req.suffixes.iter().map(|s| s.as_str()).collect();
    let exclude_dirs = req.exclude_dirs.unwrap_or_else(|| vec![
        "node_modules".to_string(), "target".to_string(), "dist".to_string(),
        "build".to_string(), ".git".to_string(), ".vscode".to_string(), ".idea".to_string()
    ]);
    let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
    
    match wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(found_files) => {
            let file_paths = found_files.iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            Ok(Json(FindFilesResponse { files: file_paths }))
        }
        Err(e) => Err(poem::Error::from_string(format!("Error searching directory: {}", e), poem::http::StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[handler]
async fn parse_file(Json(req): Json<ParseFileRequest>) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    let file_path = PathBuf::from(&req.file_path);
    
    if !file_path.exists() {
        return Err(poem::Error::from_string(
            format!("File not found: {}", file_path.display()),
            poem::http::StatusCode::NOT_FOUND
        ));
    }
    
    let extension = file_path.extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| poem::Error::from_string(
            "File has no extension", 
            poem::http::StatusCode::BAD_REQUEST
        ))?;
        
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
            poem::http::StatusCode::INTERNAL_SERVER_ERROR
        ))
    }
}

#[handler]
async fn parse_directory(Json(req): Json<ParseDirectoryRequest>) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    let dir = PathBuf::from(&req.dir);
    let suffixes_ref: Vec<&str> = req.suffixes.iter().map(|s| s.as_str()).collect();
    let exclude_dirs = req.exclude_dirs.unwrap_or_else(|| vec![
        "node_modules".to_string(), "target".to_string(), "dist".to_string(),
        "build".to_string(), ".git".to_string(), ".vscode".to_string(), ".idea".to_string()
    ]);
    let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
    
    let granularity = match req.granularity.as_deref() {
        Some("coarse") => processing::Granularity::Coarse,
        Some("medium") => processing::Granularity::Medium,
        _ => processing::Granularity::Fine,
    };
    
    let files_to_parse = match wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(files) => files,
        Err(e) => return Err(poem::Error::from_string(
            format!("Error finding files: {}", e),
            poem::http::StatusCode::INTERNAL_SERVER_ERROR
        ))
    };
    
    if files_to_parse.is_empty() {
        return Ok(Json(Vec::new()));
    }
    
    let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
    for file_path in files_to_parse {
        let extension = file_path.extension().and_then(|ext| ext.to_str());
        let parse_result = match extension {
            Some("rs") => parser_mod::extract_rust_entities_from_file(&file_path, req.max_snippet_size),
            Some("ts") => parser_mod::extract_ts_entities(&file_path, false, req.max_snippet_size),
            Some("tsx") => parser_mod::extract_ts_entities(&file_path, true, req.max_snippet_size),
            _ => continue,
        };
        
        if let Ok(entities) = parse_result {
            all_entities.extend(entities);
        }
    }
    
    let final_entities = processing::post_process_entities(all_entities, granularity, req.max_snippet_size);
    Ok(Json(final_entities))
}

#[handler]
async fn query_collection(Json(req): Json<QueryRequest>) -> Result<Json<Vec<parser_mod::CodeEntity>>, poem::Error> {
    println!("API query request for collection '{}': {}", req.collection_name, req.query_text);
    
    match hoarder::query(&req.collection_name, &req.query_text, req.model, req.api_key, req.api_base).await {
        Ok(entities) => Ok(Json(entities)),
        Err(e) => {
            eprintln!("Error in API query_collection: {}", e);
            Err(poem::Error::from_string(
                format!("Error querying collection: {}", e),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR
            ))
        }
    }
}

#[handler]
async fn generate_embeddings_api(Json(req): Json<GenerateEmbeddingsRequest>) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!("API request to generate embeddings: input='{}', output='{}'", req.input_file, req.output_file);
    let input_path = PathBuf::from(&req.input_file);
    let output_path = PathBuf::from(&req.output_file);

    if !input_path.exists() {
        return Err(poem::Error::from_string(
            format!("Input file not found: {}", req.input_file),
            poem::http::StatusCode::BAD_REQUEST
        ));
    }

    match embedder::generate_embeddings_for_index(
        &input_path, 
        &output_path, 
        req.model, 
        req.api_key, 
        req.api_base
    ).await {
        Ok(_) => Ok(Json(GenericApiResponse {
            message: "Embeddings generated successfully.".to_string(),
            details: Some(format!("Output written to {}", req.output_file)),
        })),
        Err(e) => {
            eprintln!("Error in API generate_embeddings_api: {}", e);
            Err(poem::Error::from_string(
                format!("Failed to generate embeddings: {}", e),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR
            ))
        }
    }
}

#[handler]
async fn upsert_embeddings_api(Json(req): Json<UpsertEmbeddingsRequest>) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!(
        "API request to upsert embeddings: input='{}', collection='{}'", 
        req.input_file, 
        req.collection_name
    );
    let input_path = PathBuf::from(&req.input_file);

    if !input_path.exists() {
        return Err(poem::Error::from_string(
            format!("Input file not found: {}", req.input_file),
            poem::http::StatusCode::BAD_REQUEST
        ));
    }

    // Ensure collection exists (optional, create_collection is idempotent)
    if let Err(e) = hoarder::create_collection(&req.collection_name).await {
        eprintln!("Error creating collection '{}' (it might already exist or Qdrant is down): {}", req.collection_name, e);
        // Decide if this is a hard error or if we can proceed assuming it might exist.
        // For now, let's proceed, as upsert will fail if collection doesn't exist and couldn't be created.
    }

    match hoarder::upsert_embeddings(&req.collection_name, &input_path).await {
        Ok(_) => Ok(Json(GenericApiResponse {
            message: "Embeddings upserted successfully.".to_string(),
            details: Some(format!("Upserted from {} to collection {}", req.input_file, req.collection_name)),
        })),
        Err(e) => {
            eprintln!("Error in API upsert_embeddings_api: {}", e);
            Err(poem::Error::from_string(
                format!("Failed to upsert embeddings: {}", e),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR
            ))
        }
    }
}

#[handler]
async fn build_index_api(Json(req): Json<BuildIndexRequest>) -> Result<Json<GenericApiResponse>, poem::Error> {
    println!(
        "API request to build index: dir='{}', collection='{}'",
        req.dir,
        req.collection_name
    );

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
        let dir_path = PathBuf::from(dir_clone); // Use cloned data
        let suffixes_ref: Vec<&str> = suffixes_clone.iter().map(|s| s.as_str()).collect();
        
        let default_exclude_dirs = vec!["node_modules".to_string(), "target".to_string(), "dist".to_string(), "build".to_string(), ".git".to_string(), ".vscode".to_string(), ".idea".to_string()];
        let exclude_dirs_owned = exclude_dirs_clone.unwrap_or(default_exclude_dirs);
        let exclude_dirs_ref: Vec<&str> = exclude_dirs_owned.iter().map(|s| s.as_str()).collect();
        
        let granularity = match granularity_str_clone.as_deref() { // Use cloned data
            Some("coarse") => processing::Granularity::Coarse,
            Some("medium") => processing::Granularity::Medium,
            Some("fine") => processing::Granularity::Fine,
            _ => processing::Granularity::Fine, // Default to fine
        };

        println!("--- Starting Full Index Build (API Triggered) ---");

        println!("[1/4] Finding files...");
        let files_to_parse = match wanderer::find_files_by_suffix(&dir_path, &suffixes_ref, &exclude_dirs_ref) {
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
                Some("rs") => parser_mod::extract_rust_entities_from_file(&file_path, max_snippet_size_clone),
                Some("ts") => parser_mod::extract_ts_entities(&file_path, false, max_snippet_size_clone),
                Some("tsx") => parser_mod::extract_ts_entities(&file_path, true, max_snippet_size_clone),
                _ => { continue; }
            };
            match parse_result {
                Ok(entities) => all_entities.extend(entities),
                Err(e) => eprintln!("BuildIndex API: Error parsing {}: {}. Skipping.", file_path.display(), e),
            }
        }
        println!("BuildIndex API: Parsed {} initial entities.", all_entities.len());

        println!("[2b/4] Post-processing entities (granularity: {:?})...", granularity);
        let processed_entities = processing::post_process_entities(all_entities, granularity, max_snippet_size_clone);
        println!("BuildIndex API: {} entities after post-processing.", processed_entities.len());
        if processed_entities.is_empty() { 
            println!("BuildIndex API: No entities after processing. Index build cancelled."); 
            return; 
        }

        println!("[3/4] Generating embeddings...");
        let entities_with_embeddings = match embedder::generate_embeddings_for_vec(
            processed_entities,
            embedding_model_clone, // Use cloned data
            api_key_clone,       // Use cloned data
            api_base_clone,      // Use cloned data
        ).await {
            Ok(entities) => entities,
            Err(e) => {
                eprintln!("BuildIndex API: Embedding step failed: {}", e);
                return;
            }
        };
        println!("BuildIndex API: Embeddings generated.");
        if entities_with_embeddings.iter().all(|e| e.embedding.is_none()) {
            println!("BuildIndex API: Warning: No entities had embeddings generated successfully.");
        }

        println!("[4/4] Storing embeddings in Qdrant collection '{}'...", collection_name_clone); // Use cloned data
        if let Err(e) = hoarder::create_collection(&collection_name_clone).await { // Use cloned data
            eprintln!("BuildIndex API: Failed to ensure Qdrant collection exists: {}", e);
            return;
        }
        if let Err(e) = hoarder::upsert_entities_from_vec(&collection_name_clone, entities_with_embeddings).await { // Use cloned data
            eprintln!("BuildIndex API: Upserting embeddings to Qdrant failed: {}", e);
            return;
        }

        println!("--- Index Build Complete (API Triggered) ---");
    });

    Ok(Json(GenericApiResponse {
        message: "Build index process started in the background.".to_string(),
        details: Some(format!("Building index for dir '{}' into collection '{}'. Check server logs for progress.", req.dir, req.collection_name)),
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
        Err(e) => Err(poem::Error::from_string(e, poem::http::StatusCode::BAD_REQUEST)),
    }
}

async fn start_server(host: String, port: u16) -> Result<()> {
    let editor_state = Arc::new(Mutex::new(editor::Editor::new()));

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
        .with(Cors::new()
            .allow_credentials(true)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(["Content-Type", "Authorization"])
        );

    // Static files endpoint that serves the React app from ./dist
    // Use correct path configuration to handle absolute paths in the built React app
    let static_files = StaticFilesEndpoint::new("./dist")
        .index_file("index.html");

    let app = Route::new()
        .nest("/api", api_app)
        // Serve static files directly from the root path
        .nest("/", static_files);

    println!("Starting Galatea server on {}:{}", host, port);
    println!("Serving API at /api and static files from ./dist at / ");
    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        .run(app.data(editor_state))
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
            Commands::FindFiles { dir, suffixes, exclude_dirs } => {
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
                println!(
                    "Searching for files with suffixes [{}] in: {} (excluding: [{}])",
                    suffixes.join(", "), dir.display(), exclude_dirs.join(", ")
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
            Commands::ParseDirectory { dir, suffixes, exclude_dirs, max_snippet_size, granularity } => {
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();

                println!("Starting parsing in '{}' for suffixes: {:?} (excluding: {:?})", dir.display(), suffixes, exclude_dirs);
                let files_to_parse = wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref)?;
                if files_to_parse.is_empty() { println!("No matching files found to parse."); return Ok(()); }
                println!("Found {} files to process.", files_to_parse.len());

                let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
                for file_path in files_to_parse {
                    println!("  Parsing: {}", file_path.display());
                    let extension = file_path.extension().and_then(|ext| ext.to_str());
                    let parse_result = match extension {
                        Some("rs") => parser_mod::extract_rust_entities_from_file(&file_path, max_snippet_size),
                        Some("ts") => parser_mod::extract_ts_entities(&file_path, false, max_snippet_size),
                        Some("tsx") => parser_mod::extract_ts_entities(&file_path, true, max_snippet_size),
                        _ => { println!("  -> Skipping file with unsupported extension."); continue; }
                    };
                    match parse_result {
                        Ok(entities) => {
                            println!("    -> Extracted {} entities.", entities.len());
                            all_entities.extend(entities);
                        }
                        Err(e) => { eprintln!("    -> Error parsing {}: {}. Skipping file.", file_path.display(), e); }
                    }
                }
                println!("Total entities extracted before post-processing: {}", all_entities.len());

                let final_entities = processing::post_process_entities(all_entities, granularity, max_snippet_size);
                println!("Total entities after post-processing: {}", final_entities.len());

                let json_output = serde_json::to_string_pretty(&final_entities)?;
                println!("\n--- Start JSON Output ---");
                println!("{}", json_output);
                println!("--- End JSON Output ---");
            }
            Commands::GenerateEmbeddings { input_file, output_file, model, api_key, api_base } => {
                if let Err(e) = embedder::generate_embeddings_for_index(
                    &input_file, &output_file, model, api_key, api_base,
                ).await { eprintln!("Failed to generate embeddings: {}", e); return Err(e); }
            }
            Commands::UpsertEmbeddings { input_file, collection_name } => {
                hoarder::create_collection(&collection_name).await?;
                if let Err(e) = hoarder::upsert_embeddings(&collection_name, &input_file).await { 
                    eprintln!("Failed to upsert embeddings from file: {}", e);
                    return Err(e);
                }
                println!("Upsert from file complete.");
            }
            Commands::Query { collection_name, query_text, model, api_key, api_base } => {
                println!("Querying collection '{}' with: \"{}\"", collection_name, query_text);
                if let Err(e) = hoarder::query(&collection_name, &query_text, model, api_key, api_base).await { eprintln!("Failed to execute query: {}", e); return Err(e); }
            }
            Commands::BuildIndex { dir, suffixes, exclude_dirs, granularity, max_snippet_size, embedding_model, api_key, api_base, collection_name } => {
                println!("--- Starting Full Index Build ---");

                println!("[1/4] Finding files...");
                let suffixes_ref: Vec<&str> = suffixes.iter().map(|s| s.as_str()).collect();
                let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
                let files_to_parse = wanderer::find_files_by_suffix(&dir, &suffixes_ref, &exclude_dirs_ref)
                    .with_context(|| format!("Wander step failed in dir '{}'", dir.display()))?;
                if files_to_parse.is_empty() { println!("No matching files found. Index build cancelled."); return Ok(()); }
                println!("Found {} files.", files_to_parse.len());

                println!("[2/4] Parsing files...");
                let mut all_entities: Vec<parser_mod::CodeEntity> = Vec::new();
                for file_path in files_to_parse {
                    let extension = file_path.extension().and_then(|ext| ext.to_str());
                    let parse_result = match extension {
                        Some("rs") => parser_mod::extract_rust_entities_from_file(&file_path, max_snippet_size),
                        Some("ts") => parser_mod::extract_ts_entities(&file_path, false, max_snippet_size),
                        Some("tsx") => parser_mod::extract_ts_entities(&file_path, true, max_snippet_size),
                        _ => { continue; }
                    };
                    match parse_result {
                        Ok(entities) => all_entities.extend(entities),
                        Err(e) => eprintln!("    -> Error parsing {}: {}. Skipping.", file_path.display(), e),
                    }
                }
                println!("Parsed {} initial entities.", all_entities.len());

                println!("[2b/4] Post-processing entities (granularity: {:?})...", granularity);
                let processed_entities = processing::post_process_entities(all_entities, granularity, max_snippet_size);
                println!("{} entities after post-processing.", processed_entities.len());
                if processed_entities.is_empty() { println!("No entities after processing. Index build cancelled."); return Ok(()); }

                println!("[3/4] Generating embeddings...");
                let entities_with_embeddings = embedder::generate_embeddings_for_vec(
                    processed_entities,
                    embedding_model.clone(),
                    api_key.clone(),
                    api_base.clone(),
                ).await.context("Embedding step failed")?;
                println!("Embeddings generated.");
                if entities_with_embeddings.iter().all(|e| e.embedding.is_none()) {
                    println!("Warning: No entities had embeddings generated successfully. Check API key/quota/connectivity.");
                    println!("Index build finished without storing to Qdrant.");
                    return Ok(());
                }

                println!("[4/4] Storing embeddings in Qdrant collection '{}'...", collection_name);
                hoarder::create_collection(&collection_name).await.context("Failed to ensure Qdrant collection exists")?;
                hoarder::upsert_entities_from_vec(
                    &collection_name,
                    entities_with_embeddings
                ).await.context("Upserting embeddings to Qdrant failed")?;

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
