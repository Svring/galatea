use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tracing;

use crate::terminal;

pub async fn start_dev_server(project_dir: &Path) -> Result<()> {
    terminal::port::ensure_port_is_free(3000, "Next.js dev server")
        .await
        .context("Failed to ensure Next.js dev server port (3000) is free before starting")?;

    tracing::info!(
        target: "dev_runtime::nextjs",
        project_dir = %project_dir.display(),
        "Attempting to start 'npm run dev'"
    );

    let mut cmd = TokioCommand::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(&["run", "dev"]);
    cmd.stdout(Stdio::piped()); 
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "dev_runtime::nextjs: Failed to spawn 'npm run dev' in {}. Ensure npm is installed and the script exists.",
            project_dir.display()
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .context("dev_runtime::nextjs: Failed to capture stdout from 'npm run dev'")?;
    let stderr = child
        .stderr
        .take()
        .context("dev_runtime::nextjs: Failed to capture stderr from 'npm run dev'")?;

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::info!(target: "dev_runtime::nextjs::npm_stdout", source_process = "next_dev_server", "{}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::warn!(target: "dev_runtime::nextjs::npm_stderr", source_process = "next_dev_server", "{}", line);
        }
    });

    let status = child
        .wait()
        .await
        .with_context(|| "dev_runtime::nextjs: 'npm run dev' process failed to wait")?;

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        let success_msg = format!("'npm run dev' completed successfully (status: {}).", status);
        tracing::info!(target: "dev_runtime::nextjs", source_process = "next_dev_server", "{}", success_msg);
        Ok(())
    } else {
        let err_msg = format!(
            "dev_runtime::nextjs: 'npm run dev' exited with status: {}. Check output above for details.",
            status
        );
        tracing::error!(target: "dev_runtime::nextjs", source_process = "next_dev_server", "{}", err_msg);
        Err(anyhow!(
            "{}",
            err_msg
        ))
    }
}
