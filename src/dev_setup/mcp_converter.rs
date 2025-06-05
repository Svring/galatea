use crate::terminal::npm::run_npm_command;
use anyhow::{Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tracing;

/// Ensures that the 'openapi-mcp-generator' CLI is installed globally. Installs it with npm if not present.
pub async fn ensure_openapi_mcp_generator_installed(use_sudo: bool) -> Result<()> {
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
            let install_command = if use_sudo {
                "sudo npm install -g openapi-mcp-generator"
            } else {
                "npm install -g openapi-mcp-generator"
            };
            
            tracing::info!(target: "dev_setup::mcp_converter", command = %install_command, "'openapi-mcp-generator' not found. Installing globally with npm...");
            
            let install_status = Command::new("bash")
                .arg("-c")
                .arg(install_command)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .await
                .context(format!("Failed to run '{}'", install_command))?;
            if !install_status.success() {
                return Err(anyhow::anyhow!("'{}' failed with status: {}", install_command, install_status));
            }
            tracing::info!(target: "dev_setup::mcp_converter", "Successfully installed 'openapi-mcp-generator' globally.");

            // Try to find and set permissions on global npm directories (optional, don't fail if this doesn't work)
            let npm_bin_dir_result = Command::new("bash")
                .arg("-c")
                .arg("npm bin -g")
                .output()
                .await;
            let npm_lib_dir_result = Command::new("bash")
                .arg("-c")
                .arg("npm root -g")
                .output()
                .await;

            if let (Ok(bin_output), Ok(lib_output)) = (npm_bin_dir_result, npm_lib_dir_result) {
                let npm_bin_dir = String::from_utf8_lossy(&bin_output.stdout).trim().to_string();
                let npm_lib_dir = String::from_utf8_lossy(&lib_output.stdout).trim().to_string();

                // Try to set permissions (don't fail if this doesn't work)
                for dir in [&npm_bin_dir, &npm_lib_dir] {
                    let chmod_command = if use_sudo {
                        format!("sudo chmod -R 777 {}", dir)
                    } else {
                        format!("chmod -R 755 {}", dir)
                    };
                    
                    tracing::info!(target: "dev_setup::mcp_converter", dir = %dir, command = %chmod_command, "Attempting to set permissions on {}...", dir);
                    let chmod_status = Command::new("bash")
                        .arg("-c")
                        .arg(&chmod_command)
                        .status()
                        .await;
                    match chmod_status {
                        Ok(status) if status.success() => {
                            tracing::info!(target: "dev_setup::mcp_converter", dir = %dir, "Permissions set successfully.");
                        }
                        _ => {
                            tracing::warn!(target: "dev_setup::mcp_converter", dir = %dir, "Could not set permissions, but continuing anyway.");
                        }
                    }
                }
            } else {
                tracing::warn!(target: "dev_setup::mcp_converter", "Could not determine npm global directories, but continuing anyway.");
            }
            Ok(())
        }
    }
}
