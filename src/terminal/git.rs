use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Runs a git command in the specified directory
pub async fn run_git_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.current_dir(project_dir);
    cmd.args(args);

    if suppress_output {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    tracing::debug!(target: "terminal::git", command = format!("git {}", args.join(" ")), cwd = %project_dir.display(), "Spawning git command");

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "terminal::git: Failed to spawn git command (git {}). Ensure git is installed and in PATH.",
            args.join(" ")
        )
    })?;

    if !suppress_output {
        let stdout = child.stdout.take().context("terminal::git: Failed to capture stdout from git command")?;
        let stderr = child.stderr.take().context("terminal::git: Failed to capture stderr from git command")?;

        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::info!(target: "terminal::git::stdout", "{}", line);
            }
        });

        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::warn!(target: "terminal::git::stderr", "{}", line);
            }
        });

        let status = child.wait().await.with_context(|| {
            format!(
                "terminal::git: Failed to wait for git command: git {}",
                args.join(" ")
            )
        })?;

        // Ensure logger tasks complete
        let _ = tokio::try_join!(stdout_task, stderr_task);


        if status.success() {
            Ok(())
        } else {
            tracing::error!(target: "terminal::git", command = format!("git {}", args.join(" ")), status = %status, "git command failed");
            Err(anyhow!(
                "terminal::git: git command failed with status: {}.\nCommand: git {}",
                status,
                args.join(" ")
            ))
        }
    } else {
        // If output is suppressed, just wait for completion and check status
        let output = child.wait_with_output().await.with_context(|| {
            format!(
                "terminal::git: Failed to wait for git command (output suppressed): git {}",
                args.join(" ")
            )
        })?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr_text = String::from_utf8_lossy(&output.stderr);
            let stdout_text = String::from_utf8_lossy(&output.stdout);
            tracing::error!(target: "terminal::git", command = format!("git {}", args.join(" ")), status = %output.status, stderr = %stderr_text, stdout = %stdout_text, "git command failed (output suppressed)");
            Err(anyhow!(
                "terminal::git: git command failed with status: {}.\nCommand: git {}\nStderr: {}\nStdout: {}",
                output.status,
                args.join(" "),
                stderr_text,
                stdout_text
            ))
        }
    }
}

/// Clone a git repository to the specified directory
pub async fn clone_repository(repo_url: &str, target_dir: &Path) -> Result<()> {
    tracing::info!(target: "terminal::git", repo_url = repo_url, target_dir = %target_dir.display(), "Cloning git repository");
    
    // Get the parent directory for cloning
    let parent_dir = target_dir.parent().unwrap_or_else(|| Path::new("."));
    
    // Clone the repository
    run_git_command(parent_dir, &["clone", "--verbose", "--progress", repo_url, &target_dir.file_name().unwrap().to_string_lossy()], false).await
        .context(format!("Failed to clone repository {} to {}", repo_url, target_dir.display()))?;
    
    tracing::info!(target: "terminal::git", repo_url = repo_url, target_dir = %target_dir.display(), "Repository cloned successfully");
    Ok(())
} 