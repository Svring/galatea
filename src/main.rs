use anyhow::{Context, Result};
use clap::Parser; // Added for command-line argument parsing
use std::env; // Added for std::env::current_dir
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use std::time::Duration; // Added for http_request_timeout
use tokio::sync::Mutex;
use tokio::time::sleep as tokio_sleep; // Alias to avoid conflict if Duration::sleep is used
use tracing::{error, info, warn}; // For return type of initialize_environment // Added for timing
use dashmap::DashMap; // For codex task state

// Tracing subscriber imports for layered logging
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

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
                .allow_headers(["Content-Type", "Authorization"])
                .allow_origin("*") // Allow all origins for CORS
        );

    // Use ensure_port_is_free from the terminal module
    terminal::port::ensure_port_is_free(port, "Galatea main server (pre-bind check)")
        .await
        .context("Failed to ensure Galatea server port was free immediately before binding")?;

    info!(target: "galatea::main", source_component = "server_startup", host, port, "Starting Galatea server");
    info!(target: "galatea::main", source_component = "server_startup", "Serving API at /api and static files from ./dist at / ");

    // Codex task state
    let codex_tasks_state = Arc::new(DashMap::<String, api::routes::codex_api::CodexTaskStatus>::new());

    // Spawn background task for cleaning up old codex tasks
    let codex_tasks_state_for_cleanup = Arc::clone(&codex_tasks_state);
    tokio::spawn(async move {
        let cleanup_interval = Duration::from_secs(60 * 10); // e.g., every 10 minutes
        loop {
            tokio_sleep(cleanup_interval).await;
            info!(target: "galatea::main", "Running cleanup for old codex tasks...");
            api::routes::codex_api::cleanup_old_tasks(&codex_tasks_state_for_cleanup);
        }
    });

    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        .idle_timeout(Duration::from_secs(300)) // Set idle timeout to 300 seconds
        // Pass necessary states.
        .run(app.data(editor_state).data(lsp_client_state).data(codex_tasks_state))
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))
}

#[tokio::main]
async fn main() -> Result<()> {
    //tracing_subscriber::fmt::init(); // This will be replaced
    //info!(target: "galatea::main", "Galatea application starting..."); // Logged after subscriber setup

    let cli = Cli::parse(); // Parse command-line arguments

    // --- Phase 1: Initializing environment (determine project_directory first) ---
    let now_init_env = Instant::now();
    let initialize_result = initialize_environment(cli.api_key.clone()).await; // Clone api_key if needed later
    let elapsed_init_env = now_init_env.elapsed();

    let project_directory = match initialize_result {
        Ok(dir) => {
            // Log this after subscriber is set up
            dir
        }
        Err(e) => {
            // Log this after subscriber is set up, but we need to print to stderr if subscriber fails
            eprintln!(
                "[ERROR] Failed to verify and set up project environment (duration: {}ms): {:?}. Server will not start.",
                elapsed_init_env.as_millis(),
                e
            );
            return Err(e);
        }
    };

    // --- Initialize Logging (File and Console) ---
    let file_log_guard = match dev_runtime::log::init_file_logger(&project_directory) {
        Ok((file_writer, guard)) => {
            let file_layer = fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false); // No ANSI colors in file logs

            let console_layer = fmt::layer()
                .with_writer(std::io::stdout); // Or std::io::stderr based on preference

            tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(console_layer)
                .with(file_layer)
                .init();
            
            info!(target: "galatea::main", "File and console logging initialized.");
            Some(guard) // Store the guard to keep it alive
        }
        Err(e) => {
            eprintln!(
                "[WARN] Failed to initialize file logger: {}. Falling back to console-only logging.",
                e
            );
            // Fallback to basic console logging if file logging fails
            tracing_subscriber::fmt::init();
            None
        }
    };

    // Now that logging is initialized, log the initial messages
    info!(target: "galatea::main", "Galatea application starting...");
    info!(target: "galatea::main", "Phase 1: Initializing environment...");

    if project_directory.exists() { // Check if project_directory was successfully determined before logging success
        info!(target: "galatea::main", source_component = "bootstrap", path = %project_directory.display(), duration_ms = elapsed_init_env.as_millis(), "Project environment verified and set up successfully.");
    } // Error case was handled above and returned

    info!(target: "galatea::main", "Phase 2: Launching background services (Next.js)..." );
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

    // Keep the guard in scope until main exits, if it was created
    if let Some(_guard) = file_log_guard {
        // _guard is kept alive here
    }

    Ok(())
}
