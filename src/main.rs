use anyhow::{Context, Result};
use clap::Parser; // Added for command-line argument parsing
use std::env; // Added for std::env::current_dir
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{error, info, warn}; // For return type of initialize_environment // Added for timing

// Use modules
use galatea::api; // New api module
use galatea::dev_operation; // Existing module, may need internal updates
use galatea::dev_runtime; // Existing, contains logging, nextjs
use galatea::dev_setup;
use galatea::file_system; // Existing, contains wanderer, resolver
use galatea::terminal; // Added for port utilities

// Add Poem imports
use poem::{
    endpoint::StaticFilesEndpoint,
    /* get, handler, */ http::{Method /* StatusCode */},
    listener::TcpListener,
    middleware::Cors,
    EndpointExt, Route, Server,
};
// Serde is not used directly here anymore
// use serde::{Deserialize, Serialize};
use lsp_types::Uri;

// API request/response types are now in api::models

// Define command-line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(long)]
    api_key: Option<String>,
}

// Uncommented: Moved from debugger.rs and made private, but still used in main
async fn initialize_environment(api_key: Option<String>) -> Result<PathBuf> {
    let span = tracing::info_span!(target: "galatea::bootstrap", "initialize_environment");
    let _enter = span.enter();

    tracing::info!(target: "galatea::bootstrap", "Attempting to determine project root...");

    let project_dir_path = match file_system::get_project_root() {
        Ok(dir) => {
            tracing::info!(target: "galatea::bootstrap", path = %dir.display(), "Project root found.");
            dir
        }
        Err(e) => {
            warn!(
                target: "galatea::bootstrap",
                error = %e,
                "Failed to get project root. Will attempt to initialize in default location './project'."
            );
            // Define a default project directory if get_project_root fails
            let current_dir = env::current_dir().context(
                "Failed to get current working directory to create default project path.",
            )?;
            current_dir.join("project")
        }
    };

    tracing::info!(target: "galatea::bootstrap", project_dir = %project_dir_path.display(), "Ensuring full development environment setup...");
    dev_setup::ensure_development_environment(&project_dir_path, api_key)
        .await
        .context("Bootstrap: Failed to ensure development environment setup (Next.js deps/scripts/config, .codex folder).")?;
    tracing::info!(target: "galatea::bootstrap", project_dir = %project_dir_path.display(), "Development environment setup ensured.");

    tracing::info!(target: "galatea::bootstrap", "Project verification and setup completed successfully.");
    Ok(project_dir_path)
}

// New function to encapsulate the Next.js server spawning logic
async fn launch_nextjs_dev_server_task(project_directory: PathBuf) {
    let span = tracing::info_span!(target: "galatea::main", "nextjs_dev_server_supervisor");
    let _enter = span.enter();
    info!(target: "galatea::main", source_component = "next_dev_server_supervisor", path = %project_directory.display(), "Attempting to start the Next.js development server...");
    if let Err(e) = dev_runtime::nextjs_dev_server::launch_dev_server(&project_directory).await {
        error!(target: "galatea::main", source_component = "next_dev_server_supervisor", error = ?e, "Failed to start or monitor the Next.js development server.");
    } else {
        info!(target: "galatea::main", source_component = "next_dev_server_supervisor", "Next.js development server process has finished.");
    }
}

async fn launch_api_server(host: &str, port: u16) -> Result<()> {
    let span = tracing::info_span!(target: "galatea::main", "start_server", %host, %port);
    let _enter = span.enter();

    // Editor state is now managed by editor_api routes if needed, passed via api_routes
    // let editor_state = Arc::new(Mutex::new(dev_operation::editor::Editor::new()));

    // LSP Client Setup - passed via api_routes
    let lsp_client = match dev_runtime::lsp_client::LspClient::new().await {
        Ok(client) => client,
        Err(e) => {
            error!(target: "galatea::main", source_component = "lsp_client_setup", error = ?e, "Failed to initialize LSP client. LSP features will be unavailable.");
            panic!("LSP Client initialization failed: {}", e);
        }
    };

    let project_root_path = file_system::get_project_root()
        .map_err(|e| anyhow::anyhow!("Failed to get project root for LSP: {}", e))?;

    let root_uri: Uri = file_system::resolve_path_to_uri(&project_root_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to resolve project root path {} to a URI: {}",
            project_root_path.display(),
            e
        )
    })?;

    let client_capabilities = lsp_types::ClientCapabilities::default();
    let mut lsp_client_instance = lsp_client;

    if let Err(e) = lsp_client_instance
        .initialize(root_uri.clone(), client_capabilities.clone())
        .await
    {
        warn!(target: "galatea::main", source_component = "lsp_client_setup", error = ?e, "LSP server initialization failed. GotoDefinition might not work.");
    }

    let lsp_client_state = Arc::new(Mutex::new(lsp_client_instance));
    let editor_state = Arc::new(Mutex::new(dev_operation::editor::Editor::new()));

    // Static files endpoint that serves the React app from ./dist
    let static_files = StaticFilesEndpoint::new("./dist").index_file("index.html");

    let app = Route::new()
        .nest("/api", api::api_routes()) // Use the new api_routes from the api module
        .nest("/", static_files)
        .with(
            Cors::new()
                .allow_credentials(true)
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers(["Content-Type", "Authorization"]),
        );

    // Use ensure_port_is_free from the terminal module
    terminal::port::ensure_port_is_free(port, "Galatea main server (pre-bind check)")
        .await
        .context("Failed to ensure Galatea server port was free immediately before binding")?;

    info!(target: "galatea::main", source_component = "server_startup", host, port, "Starting Galatea server");
    info!(target: "galatea::main", source_component = "server_startup", "Serving API at /api and static files from ./dist at / ");
    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        // Pass necessary states. Editor state might only be needed by editor_api routes.
        // LspClient state is needed by lsp_api routes.
        .run(app.data(editor_state).data(lsp_client_state))
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    info!(target: "galatea::main", "Galatea application starting...");

    let cli = Cli::parse(); // Parse command-line arguments

    info!(target: "galatea::main", "Phase 1: Initializing environment...");

    let now = Instant::now(); // Start timer
    let initialize_result = initialize_environment(cli.api_key).await;
    let elapsed = now.elapsed();

    let project_directory = match initialize_result {
        Ok(dir) => {
            info!(target: "galatea::main", source_component = "bootstrap", path = %dir.display(), duration_ms = elapsed.as_millis(), "Project environment verified and set up successfully.");
            dir
        }
        Err(e) => {
            error!(target: "galatea::main", source_component = "bootstrap", error = ?e, duration_ms = elapsed.as_millis(), "Failed to verify and set up project environment. Server will not start.");
            return Err(e);
        }
    };
    // The following log is a bit redundant if success is logged above with duration, but kept for consistency if initialize_result itself was not logged.
    // info!(target: "galatea::main", "Phase 1: Environment initialized successfully.");

    info!(target: "galatea::main", "Phase 2: Launching background services (Next.js)...");
    // Call the new function within tokio::spawn
    tokio::spawn(launch_nextjs_dev_server_task(project_directory.clone()));

    // Default server settings
    let host = "0.0.0.0";
    let port = 3051;

    info!(target: "galatea::main", "Phase 3: Starting main API server...");
    if let Err(e) = launch_api_server(host, port).await {
        error!(target: "galatea::main", error = ?e, "Failed to start API server. Application will exit.");
        return Err(e);
    }
    info!(target: "galatea::main", "Galatea application shutdown.");
    Ok(())
}
