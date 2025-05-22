use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing;
use crate::terminal;

const NODE_VERSION: &str = "22";

pub async fn ensure_codex_cli_installed(project_root_for_context: &Path) -> Result<()> {
    tracing::info!(target: "dev_setup::codex", "Setting up Node.js environment for codex...");
    
    // First ensure we're using Node.js 22
    terminal::nvm::ensure_node_version(project_root_for_context, NODE_VERSION)
        .await
        .context(format!("Failed to set up Node.js version {} for codex", NODE_VERSION))?;
    
    tracing::info!(target: "dev_setup::codex", "Ensuring @openai/codex CLI is installed globally...");

    let install_args = ["install", "-g", "@openai/codex"];

    terminal::npm::run_npm_command(project_root_for_context, &install_args, false)
        .await
        .context("Failed to install @openai/codex CLI globally. Please check npm and network connectivity.")?;

    tracing::info!(target: "dev_setup::codex", "@openai/codex CLI global installation command executed. (If it wasn't already installed, it should be now).");
    Ok(())
}

pub async fn ensure_codex_config(project_root: &Path) -> Result<()> {
    let parent_dir = project_root.parent().ok_or_else(|| {
        let msg = format!("Failed to get parent directory of project_root: {}", project_root.display());
        tracing::error!(target: "dev_setup::codex", details = msg);
        anyhow::anyhow!(msg)
    })?;
    let codex_path = parent_dir.join(".codex");

    if !codex_path.exists() {
        tracing::warn!(
            target: "dev_setup::codex",
            path = %codex_path.display(),
            ".codex directory not found. Creating it."
        );
        fs::create_dir_all(&codex_path).map_err(|e| {
            tracing::error!(target: "dev_setup::codex", path = %codex_path.display(), error = %e, "Failed to create .codex directory");
            e
        }).context(format!("Failed to create .codex directory at {}", codex_path.display()))?;
        tracing::info!(
            target: "dev_setup::codex",
            path = %codex_path.display(),
            ".codex directory created."
        );
    } else if !codex_path.is_dir() {
        tracing::error!(
            target: "dev_setup::codex",
            path = %codex_path.display(),
            ".codex exists but is not a directory. Please remove or rename it."
        );
        return Err(anyhow::anyhow!(
            ".codex exists at {} but is not a directory",
            codex_path.display()
        ));
    } else {
        tracing::debug!(
            target: "dev_setup::codex",
            path = %codex_path.display(),
            ".codex directory already exists and is valid."
        );
    }

    let config_file_path = codex_path.join("config.json");
    if !config_file_path.exists() {
        tracing::info!(
            target: "dev_setup::codex",
            path = %config_file_path.display(),
            "config.json not found in .codex directory. Creating it with default content."
        );
        let default_config_content = r#"{
          "model": "o3",
          "approvalMode": "full-auto",
          "provider": "sealos",
          "providers": {
            "sealos": {
              "name": "sealos",
              "baseURL": "https://aiproxy.usw.sealos.io/v1",
              "envKey": "OPENAI_API_KEY"
            }
          },
          "history": {
            "maxSize": 1000,
            "saveHistory": true,
            "sensitivePatterns": []
          }
        }"#;

        fs::write(&config_file_path, default_config_content).map_err(|e| {
            tracing::error!(target: "dev_setup::codex", path = %config_file_path.display(), error = %e, "Failed to create config.json");
            e
        }).context(format!("Failed to create config.json at {}", config_file_path.display()))?;
        tracing::info!(
            target: "dev_setup::codex",
            path = %config_file_path.display(),
            "config.json created in .codex directory."
        );
    } else if !config_file_path.is_file() {
        tracing::error!(
            target: "dev_setup::codex",
            path = %config_file_path.display(),
            "config.json exists in .codex directory but is not a file. Please remove or rename it."
        );
        return Err(anyhow::anyhow!(
            "config.json exists at {} but is not a file",
            config_file_path.display()
        ));
    } else {
        tracing::debug!(
            target: "dev_setup::codex",
            path = %config_file_path.display(),
            "config.json already exists and is a file in .codex directory."
        );
    }
    // Future: Add more validation for specific files/structures within .codex if needed.
    Ok(())
} 