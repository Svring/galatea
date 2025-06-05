use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing;

// Originally from project_tooling::nodejs, now a generic utility
pub async fn run_npm_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let mut cmd = Command::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(args);

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

    tracing::debug!(target: "terminal::npm", command = format!("npm {}", args.join(" ")), cwd = %project_dir.display(), "Spawning npm command");

    let child = cmd.spawn().with_context(|| {
        format!(
            "terminal::npm: Failed to spawn npm command (npm {}). Ensure npm is installed and in PATH.",
            args.join(" ")
        )
    })?;

    let output = child.wait_with_output().await.with_context(|| {
        format!(
            "terminal::npm: Failed to wait for npm command: npm {}",
            args.join(" ")
        )
    })?;

    if output.status.success() {
        if !suppress_output {
            let stdout_data = String::from_utf8_lossy(&output.stdout);
            if !stdout_data.is_empty() {
                tracing::info!(target: "terminal::npm::stdout", "{}", stdout_data.trim_end());
            }
            let stderr_data = String::from_utf8_lossy(&output.stderr);
            if !stderr_data.is_empty() {
                tracing::warn!(target: "terminal::npm::stderr", "{}", stderr_data.trim_end());
            }
        }
        Ok(())
    } else {
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        let stdout_text = String::from_utf8_lossy(&output.stdout);
        tracing::error!(target: "terminal::npm", command = format!("npm {}", args.join(" ")), status = %output.status, stderr = %stderr_text, stdout = %stdout_text, "npm command failed");
        Err(anyhow!(
            "terminal::npm: npm command failed with status: {}.\nCommand: npm {}\nStderr: {}\nStdout: {}",
            output.status,
            args.join(" "),
            stderr_text,
            stdout_text
        ))
    }
}

/// Runs an npm command with sudo in the specified directory
pub async fn run_npm_command_with_sudo(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let npm_command = format!("sudo npm {}", args.join(" "));
    let mut cmd = Command::new("bash");
    cmd.current_dir(project_dir);
    cmd.arg("-c").arg(&npm_command);

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

    tracing::debug!(target: "terminal::npm", command = %npm_command, cwd = %project_dir.display(), "Spawning npm command with sudo");

    let child = cmd.spawn().with_context(|| {
        format!(
            "terminal::npm: Failed to spawn npm command with sudo ({}). Ensure npm is installed and in PATH.",
            npm_command
        )
    })?;

    let output = child.wait_with_output().await.with_context(|| {
        format!(
            "terminal::npm: Failed to wait for npm command with sudo: {}",
            npm_command
        )
    })?;

    if output.status.success() {
        if !suppress_output {
            let stdout_data = String::from_utf8_lossy(&output.stdout);
            if !stdout_data.is_empty() {
                tracing::info!(target: "terminal::npm::stdout", "{}", stdout_data.trim_end());
            }
            let stderr_data = String::from_utf8_lossy(&output.stderr);
            if !stderr_data.is_empty() {
                tracing::warn!(target: "terminal::npm::stderr", "{}", stderr_data.trim_end());
            }
        }
        Ok(())
    } else {
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        let stdout_text = String::from_utf8_lossy(&output.stdout);
        tracing::error!(target: "terminal::npm", command = %npm_command, status = %output.status, stderr = %stderr_text, stdout = %stdout_text, "npm command with sudo failed");
        Err(anyhow!(
            "terminal::npm: npm command with sudo failed with status: {}.\nCommand: {}\nStderr: {}\nStdout: {}",
            output.status,
            npm_command,
            stderr_text,
            stdout_text
        ))
    }
} 