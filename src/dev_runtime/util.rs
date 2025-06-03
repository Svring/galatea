use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tracing;

/// Executes a command in the specified directory, waits for it to complete, and logs its output.
/// This function is intended for commands that need to finish before proceeding (e.g., build steps).
pub async fn run_command_in_dir(
    dir: &Path,
    program: &str,
    args: &[&str],
    command_description: &str,
    port_env: Option<u16>, // For passing PORT environment variable if needed by the command
) -> Result<()> {
    tracing::info!(
        target: "dev_runtime::util::run",
        cwd = %dir.display(),
        command = %program,
        args = ?args,
        description = %command_description,
        "Executing command and waiting for completion"
    );

    let mut cmd = TokioCommand::new(program);
    cmd.current_dir(dir);
    cmd.args(args);
    if let Some(port) = port_env {
        cmd.env("PORT", port.to_string());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "dev_runtime::util::run: Failed to spawn '{}' in {}",
            command_description,
            dir.display()
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .context(format!("dev_runtime::util::run: Failed to capture stdout from '{}'", command_description))?;
    let stderr = child
        .stderr
        .take()
        .context(format!("dev_runtime::util::run: Failed to capture stderr from '{}'", command_description))?;

    let log_target_stdout = format!("dev_runtime::run_stdout::{}", command_description.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "_"));
    let log_target_stderr = format!("dev_runtime::run_stderr::{}", command_description.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "_"));

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::info!(target: "dev_runtime::run_stdout", command_log_target = %log_target_stdout, "{}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::info!(target: "dev_runtime::run_stderr", command_log_target = %log_target_stderr, "{}", line);
        }
    });

    let status = child
        .wait()
        .await
        .with_context(|| format!("dev_runtime::util::run: '{}' process failed to wait", command_description))?;

    stdout_task.await.context("Stdout logging task failed")?;
    stderr_task.await.context("Stderr logging task failed")?;

    if status.success() {
        tracing::info!(
            target: "dev_runtime::util::run",
            description = %command_description,
            status = %status,
            "Command completed successfully."
        );
        Ok(())
    } else {
        let err_msg = format!(
            "dev_runtime::util::run: '{}' exited with status: {}. Check logs for details.",
            command_description, status
        );
        tracing::error!(target: "dev_runtime::util::run", "{}", err_msg);
        Err(anyhow!(err_msg))
    }
}

/// Spawns a command in the specified directory to run in the background.
/// Its output will be logged. This is for long-running processes like servers.
pub async fn spawn_background_command_in_dir(
    dir: &Path,
    program: &str,
    args: &[&str],
    command_description: &str,
    port_env: Option<u16>, // For passing PORT environment variable
) -> Result<()> {
    tracing::info!(
        target: "dev_runtime::util::spawn",
        cwd = %dir.display(),
        command = %program,
        args = ?args,
        description = %command_description,
        "Spawning background command"
    );

    let mut cmd = TokioCommand::new(program);
    cmd.current_dir(dir);
    cmd.args(args);
    if let Some(port) = port_env {
        cmd.env("PORT", port.to_string());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let dir_display = dir.display().to_string(); // Clone for async block
    let command_description_clone = command_description.to_string(); // Clone for async block
    let program_name_clone = program.to_string(); // Clone for async block

    tokio::spawn(async move {
        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id().map_or(0, |id| id);
                tracing::info!(target: "dev_runtime::util::spawned_process", description = %command_description_clone, pid, "Background process started.");

                let stdout = child.stdout.take().expect("Failed to capture stdout for spawned command");
                let stderr = child.stderr.take().expect("Failed to capture stderr for spawned command");

                let log_target_stdout = format!("dev_runtime::spawn_stdout::{}", command_description_clone.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "_"));
                let log_target_stderr = format!("dev_runtime::spawn_stderr::{}", command_description_clone.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "_"));

                let stdout_task = tokio::spawn(async move {
                    let mut reader = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        tracing::info!(target: "dev_runtime::spawn_stdout", command_log_target = %log_target_stdout, "{}", line);
                    }
                });

                let stderr_task = tokio::spawn(async move {
                    let mut reader = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        tracing::info!(target: "dev_runtime::spawn_stderr", command_log_target = %log_target_stderr, "{}", line);
                    }
                });

                let status_result = child.wait().await;

                // Ensure logging tasks complete, handling potential errors
                if let Err(e) = stdout_task.await {
                    tracing::error!(target: "dev_runtime::util::spawned_process", description = %command_description_clone, "Stdout logging task failed: {:?}", e);
                }
                if let Err(e) = stderr_task.await {
                    tracing::error!(target: "dev_runtime::util::spawned_process", description = %command_description_clone, "Stderr logging task failed: {:?}", e);
                }

                match status_result {
                    Ok(status) => {
                        tracing::info!(target: "dev_runtime::util::spawned_process", description = %command_description_clone, status = %status, "Background process exited.");
                    }
                    Err(e) => {
                        tracing::error!(target: "dev_runtime::util::spawned_process", description = %command_description_clone, error = %e, "Background process failed to wait or crashed.");
                    }
                }
            }
            Err(e) => {
                tracing::error!(target: "dev_runtime::util::spawn", description = %command_description_clone, error = %e, path = %dir_display, "Failed to spawn background command '{}'.", program_name_clone);
            }
        }
    });
    Ok(())
} 