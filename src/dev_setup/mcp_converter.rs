use crate::terminal::npm::run_npm_command;
use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tracing;

/// Ensures that the 'openapi-mcp-generator' CLI is installed globally. Installs it with npm if not present.
pub async fn ensure_openapi_mcp_generator_installed() -> Result<()> {
    // Check if the CLI is available
    let check_cmd = Command::new("bash")
        .arg("-c")
        .arg("openapi-mcp-generator --version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    match check_cmd {
        Ok(status) if status.success() => {
            tracing::info!(target: "dev_setup::mcp_converter", "'openapi-mcp-generator' is already installed.");
            Ok(())
        }
        _ => {
            tracing::info!(target: "dev_setup::mcp_converter", "'openapi-mcp-generator' not found. Installing globally with npm...");
            let cwd = std::env::current_dir()
                .context("Failed to get current directory for npm install")?;
            run_npm_command(&cwd, &["install", "-g", "openapi-mcp-generator"], false).await?;
            tracing::info!(target: "dev_setup::mcp_converter", "Successfully installed 'openapi-mcp-generator' globally.");
            Ok(())
        }
    }
}
