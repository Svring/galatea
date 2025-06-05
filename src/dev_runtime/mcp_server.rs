use anyhow::{Context, Result};
use std::fs;
use std::process::Stdio;
use tokio::process::Command;
use tracing;
use crate::terminal::port::{is_port_available, ensure_port_is_free};
use crate::dev_runtime::util; // Still needed for spawn_background_command_in_dir
use crate::terminal::npm; // Import the npm module
use crate::dev_runtime::types::McpServiceDefinition; // Import the definition
use tokio::time::{timeout, Duration};

const STARTING_MCP_PORT: u16 = 3060;
const MCP_OPENAPI_SPEC_PATH: &str = "/openapi.json"; // Assumed path on the MCP server

/// Launches MCP (Model-Centric Proxy) servers for each OpenAPI specification file found.
/// Each server is first generated, then built, and finally run as a separate process.
/// Returns a list of definitions for successfully initiated servers.
pub async fn create_mcp_servers(use_sudo: bool) -> Result<Vec<McpServiceDefinition>> {
    tracing::info!(target: "dev_runtime::mcp_server", "Initiating MCP server launch sequence...");

    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path.parent().context("Failed to get executable directory")?;
    let galatea_files_dir = exe_dir.join("galatea_files");
    let openapi_spec_dir = galatea_files_dir.join("openapi_specification");
    let mcp_servers_base_dir = galatea_files_dir.join("mcp_servers");

    if !openapi_spec_dir.exists() || !openapi_spec_dir.is_dir() {
        tracing::warn!(target: "dev_runtime::mcp_server", path = %openapi_spec_dir.display(), "OpenAPI specification directory not found. Skipping MCP server launch.");
        return Ok(Vec::new()); // Return empty list if no dir
    }

    // Count how many OpenAPI specs we have to determine how many ports we need
    let spec_count = fs::read_dir(&openapi_spec_dir)
        .context(format!("Failed to read OpenAPI specification directory at {}", openapi_spec_dir.display()))?
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                let path = e.path();
                if path.is_file() {
                    let extension = path.extension().and_then(|s| s.to_str());
                    if extension == Some("json") || extension == Some("yaml") || extension == Some("yml") {
                        Some(())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
        .count();

    if spec_count == 0 {
        tracing::info!(target: "dev_runtime::mcp_server", "No valid OpenAPI specifications found. Skipping MCP server launch.");
        return Ok(Vec::new());
    }

    // Only clean up the ports we actually need, plus a small buffer
    let ports_to_clean = std::cmp::min(spec_count + 2, 10); // Clean at most 10 ports
    tracing::info!(target: "dev_runtime::mcp_server", "Found {} OpenAPI specs. Cleaning up {} ports ({}-{}) before launching servers...", 
        spec_count, ports_to_clean, STARTING_MCP_PORT, STARTING_MCP_PORT + ports_to_clean as u16 - 1);
    
    for i in 0..ports_to_clean {
        let port = STARTING_MCP_PORT + i as u16;
        
        // Quick check: if port is already available, skip cleanup
        if is_port_available(port).await {
            tracing::debug!(target: "dev_runtime::mcp_server", port, "Port already available, skipping cleanup.");
            continue;
        }
        
        // Port is in use, try to free it with a shorter timeout
        let cleanup_result = timeout(Duration::from_millis(1500), ensure_port_is_free(port, "MCP server pre-launch cleanup")).await;
        match cleanup_result {
            Ok(Ok(_)) => {
                tracing::debug!(target: "dev_runtime::mcp_server", port, "Port successfully freed.");
            }
            Ok(Err(e)) => {
                tracing::warn!(target: "dev_runtime::mcp_server", port, error = ?e, "Failed to ensure port is free during MCP pre-launch cleanup. Continuing anyway.");
            }
            Err(_) => {
                tracing::warn!(target: "dev_runtime::mcp_server", port, "Timeout while trying to free port during MCP pre-launch cleanup. Continuing anyway.");
            }
        }
    }
    tracing::info!(target: "dev_runtime::mcp_server", "MCP port range cleanup complete.");

    if !mcp_servers_base_dir.exists() {
        fs::create_dir_all(&mcp_servers_base_dir)
            .context(format!("Failed to create mcp_servers directory at {}", mcp_servers_base_dir.display()))?;
        tracing::info!(target: "dev_runtime::mcp_server", path = %mcp_servers_base_dir.display(), "Created mcp_servers directory.");
    }

    let mut current_port = STARTING_MCP_PORT;
    let mut mcp_definitions = Vec::new();

    for entry in fs::read_dir(&openapi_spec_dir).context(format!("Failed to read OpenAPI specification directory at {}", openapi_spec_dir.display()))? {
        let entry = entry.context("Failed to read directory entry in openapi_specification")?;
        let spec_file_path = entry.path();
        
        tracing::debug!(target: "dev_runtime::mcp_server", path = %spec_file_path.display(), "Found file in openapi_specification directory.");

        if spec_file_path.is_file() {
            let extension = spec_file_path.extension().and_then(|s| s.to_str());
            if !(extension == Some("json") || extension == Some("yaml") || extension == Some("yml")) {
                tracing::debug!(target: "dev_runtime::mcp_server", path = %spec_file_path.display(), "Skipping non-JSON/YAML file in openapi_specification directory.");
                continue;
            }
            
            tracing::info!(target: "dev_runtime::mcp_server", path = %spec_file_path.display(), "Processing OpenAPI specification file.");

            let file_stem = spec_file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
            // Convert "project_api.json" to "project_mcp"
            let server_name = if file_stem.ends_with("_api") {
                format!("{}_mcp", &file_stem[..file_stem.len() - 4])
            } else {
                format!("{}_mcp", file_stem)
            };
            
            // The ID used for routing (e.g., "project" for project_api.json)
            let server_id = if file_stem.ends_with("_api") {
                file_stem[..file_stem.len() - 4].to_string()
            } else {
                file_stem.to_string()
            };
            
            let dedicated_project_path = mcp_servers_base_dir.join(&server_name);

            let assigned_port = loop {
                if is_port_available(current_port).await {
                    break current_port;
                }
                tracing::warn!(target: "dev_runtime::mcp_server", port = current_port, "Port already in use, trying next.");
                current_port += 1;
                if current_port > STARTING_MCP_PORT + 50 { // Reduced safety break
                    let err_msg = format!("Could not find an available port after 50 attempts for MCP server {}", server_name);
                    tracing::error!(target: "dev_runtime::mcp_server", "{}", err_msg);
                    return Err(anyhow::anyhow!(err_msg)); 
                }
            };
            current_port += 1; 

            let need_generate;
            let spec_metadata = match fs::metadata(&spec_file_path) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    tracing::info!(target: "dev_runtime::mcp_server", path = %spec_file_path.display(), error = ?e, "Failed to get metadata for spec file. Skipping regeneration check.");
                    if let Err(remove_err) = fs::remove_dir_all(&dedicated_project_path) {
                        if remove_err.kind() != std::io::ErrorKind::NotFound {
                            tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?remove_err, "Failed to delete old server directory before regeneration.");
                        }
                    }
                    None
                }
            };
            let server_metadata = match fs::metadata(&dedicated_project_path) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    tracing::info!(target: "dev_runtime::mcp_server", path = %dedicated_project_path.display(), error = ?e, "Failed to get metadata for server directory. Forcing regeneration.");
                    if let Err(remove_err) = fs::remove_dir_all(&dedicated_project_path) {
                        if remove_err.kind() != std::io::ErrorKind::NotFound {
                            tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?remove_err, "Failed to delete old server directory before regeneration.");
                        }
                    }
                    None
                }
            };
            let spec_modified = spec_metadata.as_ref().and_then(|m| m.modified().ok());
            let server_modified = server_metadata.as_ref().and_then(|m| m.modified().ok());
            if let (Some(spec_time), Some(server_time)) = (spec_modified, server_modified) {
                if spec_time > server_time {
                    tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "Spec file is newer than server directory. Deleting and regenerating server.");
                    if let Err(e) = fs::remove_dir_all(&dedicated_project_path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?e, "Failed to delete old server directory before regeneration.");
                            continue;
                        }
                    }
                    need_generate = true;
                } else {
                    tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "Project directory already exists and is up to date, skipping openapi-mcp-generator step.");
                    need_generate = false;
                }
            } else {
                // If we can't get modification times, force regeneration
                tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "Could not determine modification times. Forcing regeneration.");
                if let Err(e) = fs::remove_dir_all(&dedicated_project_path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?e, "Failed to delete old server directory before regeneration.");
                        continue;
                    }
                }
                need_generate = true;
            }

            if need_generate {
                let spec_file_path_str = spec_file_path.to_string_lossy().to_string();
                
                if use_sudo {
                    // Use sudo to run as root
                    let generator_command_str = format!(
                        "sudo openapi-mcp-generator --input '{}' --output '{}' --transport=streamable-http --port={}",
                        spec_file_path_str,
                        dedicated_project_path.to_string_lossy(),
                        assigned_port
                    );
                    let mut generator_cmd = Command::new("bash");
                    generator_cmd.arg("-c").arg(&generator_command_str)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, command = %generator_command_str, "Running openapi-mcp-generator as root (sudo)...");
                    match generator_cmd.output().await {
                        Ok(generator_output) => {
                            if !generator_output.status.success() {
                                tracing::error!(target: "dev_runtime::mcp_server", 
                                    server_name = %server_name, 
                                    status = %generator_output.status,
                                    stdout = %String::from_utf8_lossy(&generator_output.stdout),
                                    stderr = %String::from_utf8_lossy(&generator_output.stderr),
                                    "openapi-mcp-generator failed for {}. Skipping server launch.", server_name);
                                continue; 
                            }
                            tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "openapi-mcp-generator completed successfully.");
                        }
                        Err(e) => {
                            tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?e, "Failed to execute openapi-mcp-generator. Skipping server launch.");
                            continue;
                        }
                    }
                } else {
                    // Run openapi-mcp-generator normally (without sudo to avoid password prompt)
                    let mut generator_cmd = Command::new("openapi-mcp-generator");
                    generator_cmd.arg("--input")
                       .arg(&spec_file_path_str)
                       .arg("--output")
                       .arg(dedicated_project_path.to_string_lossy().as_ref())
                       .arg("--transport=streamable-http")
                       .arg(format!("--port={}", assigned_port))
                       .stdout(Stdio::piped())
                       .stderr(Stdio::piped());
                    
                    tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "Running openapi-mcp-generator...");
                    match generator_cmd.output().await {
                        Ok(generator_output) => {
                            if !generator_output.status.success() {
                                tracing::error!(target: "dev_runtime::mcp_server", 
                                    server_name = %server_name, 
                                    status = %generator_output.status,
                                    stdout = %String::from_utf8_lossy(&generator_output.stdout),
                                    stderr = %String::from_utf8_lossy(&generator_output.stderr),
                                    "openapi-mcp-generator failed for {}. Skipping server launch.", server_name);
                                continue; 
                            }
                            tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "openapi-mcp-generator completed successfully.");
                        }
                        Err(e) => {
                            tracing::error!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?e, "Failed to execute openapi-mcp-generator. Skipping server launch.");
                            continue;
                        }
                    }
                }
                
                // Fix permissions on the generated directory to ensure npm can write to it
                let chmod_command = if use_sudo {
                    format!("sudo chmod -R 777 {}", dedicated_project_path.to_string_lossy())
                } else {
                    format!("chmod -R 777 {}", dedicated_project_path.to_string_lossy())
                };
                
                tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, path = %dedicated_project_path.display(), command = %chmod_command, "Setting permissions on generated MCP server directory...");
                let chmod_status = Command::new("bash")
                    .arg("-c")
                    .arg(&chmod_command)
                    .status()
                    .await;
                match chmod_status {
                    Ok(status) if status.success() => {
                        tracing::info!(target: "dev_runtime::mcp_server", server_name = %server_name, "Permissions set successfully.");
                    }
                    Ok(status) => {
                        tracing::warn!(target: "dev_runtime::mcp_server", server_name = %server_name, status = %status, "Failed to set permissions, but continuing anyway.");
                    }
                    Err(e) => {
                        tracing::warn!(target: "dev_runtime::mcp_server", server_name = %server_name, error = ?e, "Failed to execute chmod command, but continuing anyway.");
                    }
                }
            }

            // Always spawn a task to build and run this specific server
            let dedicated_project_path_clone = dedicated_project_path.clone();
            let server_id_clone = server_id.clone();
            let server_name_clone = server_name.clone();
            let assigned_port_clone = assigned_port;
            let use_sudo_clone = use_sudo;
            tokio::spawn(async move {
                let proj_path = dedicated_project_path_clone;
                let s_id = server_id_clone;
                let s_name = server_name_clone;

                if use_sudo_clone {
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, path = %proj_path.display(), "Running npm install with sudo...");
                    if let Err(e) = npm::run_npm_command_with_sudo(&proj_path, &["install"], false).await {
                        tracing::error!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, error = ?e, "npm install with sudo failed. Aborting launch for this server.");
                        return;
                    }
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, "npm install completed.");

                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, path = %proj_path.display(), "Running npm run build with sudo...");
                    if let Err(e) = npm::run_npm_command_with_sudo(&proj_path, &["run", "build"], false).await {
                        tracing::error!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, error = ?e, "npm run build with sudo failed. Aborting launch for this server.");
                        return; 
                    }
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, "npm run build completed.");
                } else {
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, path = %proj_path.display(), "Running npm install...");
                    if let Err(e) = npm::run_npm_command(&proj_path, &["install"], false).await {
                        tracing::error!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, error = ?e, "npm install failed. Aborting launch for this server.");
                        return;
                    }
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, "npm install completed.");

                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, path = %proj_path.display(), "Running npm run build...");
                    if let Err(e) = npm::run_npm_command(&proj_path, &["run", "build"], false).await {
                        tracing::error!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, error = ?e, "npm run build failed. Aborting launch for this server.");
                        return; 
                    }
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, "npm run build completed.");
                }

                tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, path = %proj_path.display(), port = assigned_port_clone, "Running npm run start:http...");
                if let Err(e) = util::spawn_background_command_in_dir(&proj_path, "npm", &["run", "start:http"], &format!("MCP Server {} ({})", s_name, s_id), None).await {
                    tracing::error!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, error = ?e, "Failed to spawn 'npm run start:http'.");
                } else {
                    tracing::info!(target: "dev_runtime::mcp_server::lifecycle", server_id = %s_id, server_name = %s_name, port = assigned_port_clone, "MCP server '{}' ({}) initiated on port {}.", s_name, s_id, assigned_port_clone);
                }
            });
            
            // Add definition after successfully initiating the generation and spawning the launch task
            mcp_definitions.push(McpServiceDefinition {
                id: server_id,
                name: server_name,
                port: assigned_port,
                openapi_spec_path_on_mcp: MCP_OPENAPI_SPEC_PATH.to_string(),
            });
        }
    }

    if mcp_definitions.is_empty() {
        tracing::info!(target: "dev_runtime::mcp_server", "No valid OpenAPI specifications found to generate and launch MCP servers.");
    } else {
        tracing::info!(target: "dev_runtime::mcp_server", count = mcp_definitions.len(), "All requested MCP server generation and launch tasks have been initiated and definitions collected.");
    }
    
    Ok(mcp_definitions)
}
