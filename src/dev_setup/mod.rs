pub mod nextjs_project;
pub mod codex_rules;

use anyhow::Result;
use std::path::Path;
use tracing;

// This function will orchestrate the setup checks
pub async fn ensure_development_environment(project_root: &Path) -> Result<()> {
    tracing::info!(target: "dev_setup", "Ensuring Next.js project dependencies and scripts...");
    nextjs_project::ensure_project_dependencies_and_scripts(project_root).await?;
    tracing::info!(target: "dev_setup", "Next.js project dependencies and scripts checked.");

    tracing::info!(target: "dev_setup", "Ensuring Next.js project configuration (rewrites)...");
    nextjs_project::ensure_next_config_rewrites(project_root).await?;
    tracing::info!(target: "dev_setup", "Next.js project configuration (rewrites) checked.");

    tracing::info!(target: "dev_setup", "Ensuring .codex directory structure...");
    codex_rules::ensure_codex_directory_exists(project_root).await?;
    tracing::info!(target: "dev_setup", ".codex directory structure checked.");

    Ok(())
} 