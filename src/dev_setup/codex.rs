use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing;
use crate::terminal;
use tokio::process::Command;
use std::process::Stdio;

const NODE_VERSION: &str = "22";

async fn verify_node_version(project_root: &Path) -> Result<bool> {
    // Run a command to check the current node version
    let mut cmd = Command::new("bash");
    cmd.arg("-c");
    cmd.arg("node --version");
    cmd.current_dir(project_root);
    cmd.stdout(Stdio::piped());
    
    let output = cmd.output().await.context("Failed to execute node --version command")?;
    
    if !output.status.success() {
        tracing::warn!(target: "dev_setup::codex", "Failed to check node version");
        return Ok(false);
    }
    
    let version_str = String::from_utf8_lossy(&output.stdout);
    let version_str = version_str.trim();
    
    // Check if version starts with v22
    let is_correct_version = version_str.starts_with("v22");
    
    if !is_correct_version {
        tracing::warn!(
            target: "dev_setup::codex", 
            current_version = %version_str, 
            expected_version = %NODE_VERSION,
            "Node.js version mismatch"
        );
    } else {
        tracing::info!(
            target: "dev_setup::codex", 
            version = %version_str,
            "Verified Node.js version"
        );
    }
    
    Ok(is_correct_version)
}

pub async fn ensure_codex_cli_installed(project_root_for_context: &Path) -> Result<()> {
    tracing::info!(target: "dev_setup::codex", "Setting up Node.js environment for codex...");
    
    // First ensure we're using Node.js 22
    terminal::nvm::ensure_node_version(project_root_for_context, NODE_VERSION)
        .await
        .context(format!("Failed to set up Node.js version {} for codex", NODE_VERSION))?;
    
    // Verify that the node version is actually set correctly
    let version_verified = verify_node_version(project_root_for_context).await
        .context("Failed to verify Node.js version")?;
    
    if !version_verified {
        tracing::warn!(
            target: "dev_setup::codex",
            "Node.js version verification failed. This may cause issues with codex. Will proceed with installation anyway."
        );
    }
    
    tracing::info!(target: "dev_setup::codex", "Ensuring @openai/codex CLI is installed globally...");

    // Use the bash command with nvm to ensure Node.js 22 is used for npm install
    let mut cmd = Command::new("bash");
    cmd.arg("-c");
    cmd.arg("source ~/.nvm/nvm.sh && nvm use 22 && npm install -g @openai/codex");
    cmd.current_dir(project_root_for_context);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    
    let output = cmd.output().await.context("Failed to execute npm install command")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!(target: "dev_setup::codex", stderr = %stderr, "Failed to install @openai/codex CLI");
        return Err(anyhow::anyhow!("Failed to install @openai/codex CLI: {}", stderr));
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        tracing::info!(target: "dev_setup::codex::npm::stdout", "{}", stdout.trim_end());
    }
    
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        tracing::warn!(target: "dev_setup::codex::npm::stderr", "{}", stderr.trim_end());
    }

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