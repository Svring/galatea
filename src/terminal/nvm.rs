use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing;

/// Runs an nvm command in the specified directory
pub async fn run_nvm_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    // NVM is typically a shell function, not a standalone executable
    // We need to source nvm and then run the command in the same shell
    
    // Construct the command: source nvm and then run the specified command
    let nvm_command = format!("source ~/.nvm/nvm.sh && nvm {}", args.join(" "));
    
    let mut cmd = Command::new("bash");
    cmd.current_dir(project_dir);
    cmd.arg("-c");
    cmd.arg(&nvm_command);

    match suppress_output {
        true => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
        false => {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }
    }

    tracing::debug!(
        target: "terminal::nvm", 
        command = format!("nvm {}", args.join(" ")), 
        cwd = %project_dir.display(), 
        "Running nvm command"
    );

    let child = cmd.spawn().with_context(|| {
        format!(
            "terminal::nvm: Failed to spawn nvm command (nvm {}). Ensure nvm is installed.",
            args.join(" ")
        )
    })?;

    let output = child.wait_with_output().await.with_context(|| {
        format!(
            "terminal::nvm: Failed to wait for nvm command: nvm {}",
            args.join(" ")
        )
    })?;

    if output.status.success() {
        if !suppress_output {
            let stdout_data = String::from_utf8_lossy(&output.stdout);
            if !stdout_data.is_empty() {
                tracing::info!(target: "terminal::nvm::stdout", "{}", stdout_data.trim_end());
            }
            let stderr_data = String::from_utf8_lossy(&output.stderr);
            if !stderr_data.is_empty() {
                tracing::warn!(target: "terminal::nvm::stderr", "{}", stderr_data.trim_end());
            }
        }
        Ok(())
    } else {
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        let stdout_text = String::from_utf8_lossy(&output.stdout);
        tracing::error!(
            target: "terminal::nvm", 
            command = format!("nvm {}", args.join(" ")), 
            status = %output.status, 
            stderr = %stderr_text, 
            stdout = %stdout_text, 
            "nvm command failed"
        );
        Err(anyhow!(
            "terminal::nvm: nvm command failed with status: {}.\nCommand: nvm {}\nStderr: {}\nStdout: {}",
            output.status,
            args.join(" "),
            stderr_text,
            stdout_text
        ))
    }
}

/// Ensures that the specified Node.js version is installed and set as active
pub async fn ensure_node_version(project_dir: &Path, version: &str) -> Result<()> {
    tracing::info!(target: "terminal::nvm", version = version, "Ensuring Node.js version is installed and active");
    
    // First try to use the version (which will fail if it's not installed)
    let use_result = run_nvm_command(project_dir, &["use", version], false).await;
    
    if use_result.is_err() {
        // If using the version failed, try to install it
        tracing::info!(target: "terminal::nvm", version = version, "Node.js version not available, attempting to install");
        run_nvm_command(project_dir, &["install", version], false).await
            .context(format!("Failed to install Node.js version {} using nvm", version))?;
        
        // Now try using it again
        run_nvm_command(project_dir, &["use", version], false).await
            .context(format!("Failed to use Node.js version {} after installation", version))?;
    }
    
    tracing::info!(target: "terminal::nvm", version = version, "Node.js version is now active");
    Ok(())
}
