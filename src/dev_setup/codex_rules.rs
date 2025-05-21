use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing;

pub async fn ensure_codex_directory_exists(project_root: &Path) -> Result<()> {
    let codex_path = project_root.join(".codex");
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
    // Future: Add more validation for specific files/structures within .codex if needed.
    Ok(())
} 