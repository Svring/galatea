use anyhow::{Context, Result};
use clap::Parser; // Added for command-line argument parsing
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration; // Added for http_request_timeout
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::sleep as tokio_sleep; // Alias to avoid conflict if Duration::sleep is used
use tracing::{error, info, warn}; // For return type of initialize_environment // Added for timing // For codex task state

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
    token: Option<String>,
    #[clap(long, default_value = "nextjs")]
    template: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing with a default filter if RUST_LOG is not set
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info")); // Default to info level for all targets
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

    let app = Route::new().nest("/api", api::api_routes()).with(
        Cors::new()
            .allow_credentials(true)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(["Content-Type", "Authorization"])
            .allow_origin("*"),
    );

    terminal::port::ensure_port_is_free(port, "Galatea main server (pre-bind check)")
        .await
        .context("Failed to ensure Galatea server port was free immediately before binding")?;

    info!(target: "galatea::main", source_component = "server_startup", host, port, "Starting Galatea server");

    Server::new(TcpListener::bind(format!("{}:{}", host, port)))
        .run(app)
        .await
        .map_err(|e| anyhow::anyhow!("Server error: {}", e))?;

    info!(target: "galatea::main", "Galatea application shutdown.");
    Ok(())
}
