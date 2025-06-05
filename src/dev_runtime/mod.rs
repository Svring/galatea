pub mod log;
pub mod lsp_client;
pub mod mcp_server;
pub mod nextjs_dev_server;
pub mod types;
pub mod util;

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing;
use types::McpServiceDefinition;

/// Launches the primary development runtime services.
///
/// This includes:
/// - The Next.js development server (launched as a detached task).
/// - MCP (Model-Centric Proxy) servers, if `mcp_enabled` is true.
///
/// Returns a list of McpServiceDefinitions if MCP servers are launched.
pub async fn launch_runtime_services(
    project_dir: PathBuf, // The root directory of the Next.js project
    mcp_enabled: bool,
    use_sudo: bool,
) -> Result<Vec<McpServiceDefinition>> {
    tracing::info!(target: "dev_runtime", "Starting runtime services...");

    // Launch Next.js dev server as a detached task
    let nextjs_project_dir_clone = project_dir.clone();
    tokio::spawn(async move {
        tracing::info!(target: "dev_runtime", path = %nextjs_project_dir_clone.display(), "Attempting to start the Next.js development server in a background task...");
        match nextjs_dev_server::launch_dev_server(&nextjs_project_dir_clone).await {
            Ok(_) => {
                tracing::info!(target: "dev_runtime", "Next.js development server process has finished or was fully spawned.")
            }
            Err(e) => {
                tracing::error!(target: "dev_runtime", error = ?e, "Failed to start or monitor the Next.js development server.")
            }
        }
    });

    let mut mcp_definitions = Vec::new();

    if mcp_enabled {
        tracing::info!(target: "dev_runtime", "MCP flag is enabled. Attempting to launch MCP servers...");

        // Ensure openapi-mcp-generator is installed
        match crate::dev_setup::mcp_converter::ensure_openapi_mcp_generator_installed(use_sudo).await {
            Ok(_) => {
                tracing::info!(target: "dev_runtime", "openapi-mcp-generator is available.");
            }
            Err(e) => {
                tracing::error!(target: "dev_runtime", error = ?e, "Failed to ensure openapi-mcp-generator is installed.");
                return Err(e).context("Failed to ensure openapi-mcp-generator is installed");
            }
        }

        // Await MCP server creation to get their definitions
        match mcp_server::create_mcp_servers(use_sudo).await {
            Ok(definitions) => {
                tracing::info!(target: "dev_runtime", count = definitions.len(), "MCP server creation process completed.");
                mcp_definitions = definitions;
            }
            Err(e) => {
                tracing::error!(target: "dev_runtime", error = ?e, "Failed to complete MCP server creation.");
                // Depending on desired behavior, you might want to propagate this error
            }
        }
    } else {
        tracing::info!(target: "dev_runtime", "MCP flag is not enabled. Skipping MCP server launch.");
    }

    Ok(mcp_definitions)
}
