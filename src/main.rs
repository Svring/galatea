use anyhow::{Context, Result};
use clap::Parser; // Added for command-line argument parsing
use std::time::Instant;
use tracing::info;

// Tracing subscriber imports for layered logging
use tracing_subscriber::EnvFilter;

// Use modules
use galatea::dev_runtime; // Existing, contains logging, nextjs
use galatea::dev_setup;
use galatea::terminal; // Added for port utilities

// Add Poem imports
use poem::{
    http::Method,
    listener::TcpListener,
    middleware::Cors,
    EndpointExt, Route, Server,
};
use poem_openapi::{OpenApi, OpenApiService};

// Import the individual API structs
use galatea::api::routes::project::ProjectApi;
use galatea::api::routes::editor_api::EditorApi;

// Import for MCP proxy functionality
use poem::{handler, web::Path as PoemPath, Response};
use poem::http::StatusCode;

// Define command-line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(long)]
    token: Option<String>,
    #[clap(long, default_value = "nextjs")]
    template: Option<String>,
    #[clap(long, default_value_t = false)]
    mcp_enabled: bool,
}

// Combined API struct
struct GalateaApi;

#[OpenApi]
impl GalateaApi {
    /// Health check endpoint for the main API
    #[oai(path = "/health", method = "get")]
    async fn health(&self) -> poem_openapi::payload::PlainText<String> {
        poem_openapi::payload::PlainText("Galatea is online.".to_string())
    }
}

// MCP Proxy handler
#[handler]
async fn mcp_proxy(
    req: &poem::Request,
    body: poem::Body,
) -> poem::Result<Response> {
    // Extract the path manually
    let path = req.uri().path();
    
    // Parse the path to extract api_type and subpath
    // Expected format: /api/{api_type}/mcp[/{subpath}]
    let path_parts: Vec<&str> = path.split('/').collect();
    if path_parts.len() < 4 || path_parts[1] != "api" || path_parts[3] != "mcp" {
        return Err(poem::Error::from_string("Invalid MCP proxy path", StatusCode::BAD_REQUEST));
    }
    
    let api_type = path_parts[2];
    let subpath = if path_parts.len() > 4 {
        path_parts[4..].join("/")
    } else {
        String::new()
    };
    
    // Get the MCP definitions from app data
    let mcp_definitions = req.data::<Vec<galatea::dev_runtime::types::McpServiceDefinition>>()
        .ok_or_else(|| poem::Error::from_string("MCP definitions not found", StatusCode::INTERNAL_SERVER_ERROR))?;
    
    // Find the matching MCP server
    let mcp_def = mcp_definitions.iter()
        .find(|def| def.id == api_type)
        .ok_or_else(|| poem::Error::from_string(format!("MCP server '{}' not found", api_type), StatusCode::NOT_FOUND))?;
    
    // Build the target URL
    let target_url = if subpath.is_empty() {
        format!("http://127.0.0.1:{}/mcp", mcp_def.port)
    } else {
        format!("http://127.0.0.1:{}/mcp/{}", mcp_def.port, subpath)
    };
    
    // Create HTTP client
    let client = reqwest::Client::new();
    
    // Forward the request
    let mut proxy_req = client.request(req.method().clone(), &target_url);
    
    // Copy headers
    for (key, value) in req.headers() {
        if key != "host" {
            proxy_req = proxy_req.header(key, value);
        }
    }
    
    // Forward body
    let body_bytes = body.into_bytes().await?;
    proxy_req = proxy_req.body(body_bytes);
    
    // Send request
    let resp = proxy_req.send().await
        .map_err(|e| poem::Error::from_string(format!("Proxy error: {}", e), StatusCode::BAD_GATEWAY))?;
    
    // Build response
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await
        .map_err(|e| poem::Error::from_string(format!("Failed to read response body: {}", e), StatusCode::BAD_GATEWAY))?;
    
    let mut response = Response::builder().status(status);
    
    // Copy response headers
    for (key, value) in headers {
        if let Some(key) = key {
            response = response.header(key, value);
        }
    }
    
    Ok(response.body(body))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing with a default filter if RUST_LOG is not set
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")); // Default to info level for all targets
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!(target: "galatea::main", "Galatea application starting...");

    let cli = Cli::parse();

    let now_init_env = Instant::now();
    let project_directory = dev_setup::ensure_development_environment(cli.template.clone())
        .await
        .map_err(|e| {
            eprintln!(
                "[ERROR] Failed to verify and set up project environment (duration: {}ms): {:?}. Server will not start.",
                now_init_env.elapsed().as_millis(),
                e
            );
            e
        })?;

    // Write CLI arguments to config.toml (after galatea_files is created)
    if let Some(token) = &cli.token {
        galatea::dev_setup::config_files::set_config_value("token", token)?;
    }
    if let Some(template) = &cli.template {
        galatea::dev_setup::config_files::set_config_value("template", template)?;
    }

    info!(target: "galatea::main", source_component = "bootstrap", path = %project_directory.display(), duration_ms = now_init_env.elapsed().as_millis(), "Project environment verified and set up successfully.");

    info!(target: "galatea::main", "Phase 2: Launching runtime services (Next.js and MCP servers if enabled)...");
    
    // Launch runtime services and get MCP definitions
    let mcp_definitions = dev_runtime::launch_runtime_services(project_directory.clone(), cli.mcp_enabled)
        .await
        .context("Failed to launch runtime services")?;
    
    if !mcp_definitions.is_empty() {
        info!(target: "galatea::main", count = mcp_definitions.len(), "MCP servers initiated: {:?}", mcp_definitions);
        // Give MCP servers time to start up
        info!(target: "galatea::main", "Waiting 3 seconds for MCP servers to initialize...");
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    }

    let host = "0.0.0.0";
    let port = 3051;
    let _span = tracing::info_span!(target: "galatea::main", "start_server", host, port).entered();

    // --- OpenAPI Services ---
    let main_api_service = OpenApiService::new(GalateaApi, "Galatea API", "1.0")
        .server(format!("http://localhost:{}/api", port));
    let project_api_service = OpenApiService::new(ProjectApi, "Project API", "1.0")
        .server(format!("http://localhost:{}/api/project", port));
    let editor_api_service = OpenApiService::new(EditorApi, "Editor API", "1.0")
        .server(format!("http://localhost:{}/api/editor", port));

    // --- Scalar UI & Spec Endpoints ---
    let main_api_scalar = main_api_service.scalar();
    let main_api_spec = main_api_service.spec_endpoint();
    let project_api_scalar = project_api_service.scalar();
    let project_api_spec = project_api_service.spec_endpoint();
    let editor_api_scalar = editor_api_service.scalar();
    let editor_api_spec = editor_api_service.spec_endpoint();

    // --- Route Setup ---
    let mut app = Route::new()
        // Main API
        .nest("/api", main_api_service)
        .nest("/api/scalar", main_api_scalar)
        .at("/api/spec", main_api_spec)
        // Project API
        .nest("/api/project", project_api_service)
        .nest("/api/project/scalar", project_api_scalar)
        .at("/api/project/spec", project_api_spec)
        // Editor API
        .nest("/api/editor", editor_api_service)
        .nest("/api/editor/scalar", editor_api_scalar)
        .at("/api/editor/spec", editor_api_spec);
    
    // Add MCP proxy routes dynamically based on definitions
    for mcp_def in &mcp_definitions {
        let route_pattern = format!("/api/{}/mcp", mcp_def.id);
        let route_pattern_with_path = format!("/api/{}/mcp/*", mcp_def.id);
        info!(target: "galatea::main", "Adding MCP proxy routes: {} and {} -> http://127.0.0.1:{}/mcp", route_pattern, route_pattern_with_path, mcp_def.port);
        app = app.at(&route_pattern, mcp_proxy);
        app = app.at(&route_pattern_with_path, mcp_proxy);
    }
    
    // Build final app with data and middleware
    let app = app
        .data(mcp_definitions)
        .with(
            Cors::new()
                .allow_credentials(true)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
                .allow_headers(["Content-Type", "Authorization"])
                .allow_origin("*"),
        );

    terminal::port::ensure_port_is_free(port, "Galatea main server (pre-bind check)")
        .await
        .context("Failed to ensure Galatea server port was free immediately before binding")?;

    info!(target: "galatea::main", source_component = "server_startup", host, port, "Starting Galatea server with OpenAPI documentation at http://{}:{}/", host, port);

    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        .run(app)
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

    info!(target: "galatea::main", "Galatea application shutdown.");
    Ok(())
}
