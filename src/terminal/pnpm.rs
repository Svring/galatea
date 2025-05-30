use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Runs a pnpm command in the specified directory
pub async fn run_pnpm_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let mut cmd = Command::new("pnpm");
    cmd.current_dir(project_dir);
    cmd.args(args);

    if suppress_output {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    tracing::debug!(target: "terminal::pnpm", command = format!("pnpm {}", args.join(" ")), cwd = %project_dir.display(), "Spawning pnpm command");

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "terminal::pnpm: Failed to spawn pnpm command (pnpm {}). Ensure pnpm is installed and in PATH.",
            args.join(" ")
        )
    })?;

    if !suppress_output {
        let stdout = child.stdout.take().context("terminal::pnpm: Failed to capture stdout from pnpm command")?;
        let stderr = child.stderr.take().context("terminal::pnpm: Failed to capture stderr from pnpm command")?;

        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::info!(target: "terminal::pnpm::stdout", "{}", line);
            }
        });

        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::warn!(target: "terminal::pnpm::stderr", "{}", line);
            }
        });

        let status = child.wait().await.with_context(|| {
            format!(
                "terminal::pnpm: Failed to wait for pnpm command: pnpm {}",
                args.join(" ")
            )
        })?;
        
        // Ensure logger tasks complete
        let _ = tokio::try_join!(stdout_task, stderr_task);

        if status.success() {
            Ok(())
        } else {
            tracing::error!(target: "terminal::pnpm", command = format!("pnpm {}", args.join(" ")), status = %status, "pnpm command failed");
            Err(anyhow!(
                "terminal::pnpm: pnpm command failed with status: {}.\nCommand: pnpm {}",
                status,
                args.join(" ")
            ))
        }
    } else {
        // If output is suppressed, just wait for completion and check status
        let output = child.wait_with_output().await.with_context(|| {
            format!(
                "terminal::pnpm: Failed to wait for pnpm command (output suppressed): pnpm {}",
                args.join(" ")
            )
        })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr_text = String::from_utf8_lossy(&output.stderr);
            let stdout_text = String::from_utf8_lossy(&output.stdout);
            tracing::error!(target: "terminal::pnpm", command = format!("pnpm {}", args.join(" ")), status = %output.status, stderr = %stderr_text, stdout = %stdout_text, "pnpm command failed (output suppressed)");
            Err(anyhow!(
                "terminal::pnpm: pnpm command failed with status: {}.\nCommand: pnpm {}\nStderr: {}\nStdout: {}",
                output.status,
                args.join(" "),
                stderr_text,
                stdout_text
            ))
        }
    }
} 