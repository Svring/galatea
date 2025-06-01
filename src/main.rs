use anyhow::{Context, Result};
use clap::Parser; // Added for command-line argument parsing
use std::time::Instant;
use tracing::{error, info}; // For return type of initialize_environment

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

// Define command-line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(long)]
    token: Option<String>,
    #[clap(long, default_value = "nextjs")]
    template: Option<String>,
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

    info!(target: "galatea::main", "Phase 2: Launching background services (Next.js)...");
    let nextjs_project_dir = project_directory.clone();
    tokio::spawn(async move {
        info!(target: "galatea::main", source_component = "next_dev_server_supervisor", path = %nextjs_project_dir.display(), "Attempting to start the Next.js development server...");
        match dev_runtime::nextjs_dev_server::launch_dev_server(&nextjs_project_dir).await {
            Ok(_) => {
                info!(target: "galatea::main", source_component = "next_dev_server_supervisor", "Next.js development server process has finished.")
            }
            Err(e) => {
                error!(target: "galatea::main", source_component = "next_dev_server_supervisor", error = ?e, "Failed to start or monitor the Next.js development server.")
            }
        }
    });

    let host = "0.0.0.0";
    let port = 3051;
    let _span = tracing::info_span!(target: "galatea::main", "start_server", host, port).entered();

    // Create OpenAPI services for each API module
    let main_api_service = OpenApiService::new(GalateaApi, "Galatea API", "1.0")
        .server(format!("http://localhost:{}/api", port));
    
    let project_api_service = OpenApiService::new(ProjectApi, "Project API", "1.0")
        .server(format!("http://localhost:{}/api/project", port));
    
    let editor_api_service = OpenApiService::new(EditorApi, "Editor API", "1.0")
        .server(format!("http://localhost:{}/api/editor", port));

    // Create Scalar UI for each API
    let scalar_ui = main_api_service.scalar();
    let project_scalar_ui = project_api_service.scalar();
    let editor_scalar_ui = editor_api_service.scalar();

    // Build the application routes
    let app = Route::new()
        .nest("/api", main_api_service)
        .nest("/api/project", project_api_service)
        .nest("/api/editor", editor_api_service)
        .nest("/api/scalar", scalar_ui)
        .nest("/api/project/scalar", project_scalar_ui)
        .nest("/api/editor/scalar", editor_scalar_ui)
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
