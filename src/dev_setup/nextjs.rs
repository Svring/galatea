use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::Path;
use tracing;
use crate::terminal;

pub async fn scaffold_nextjs_project(project_root: &Path, template_url: &str) -> Result<()> {
    tracing::info!(
        target: "dev_setup::nextjs",
        path = %project_root.display(),
        template_url = template_url,
        "Scaffolding Next.js project: Cloning template to desired project location."
    );

    // Only create the project directory if it does not exist
    if !project_root.exists() {
        // Ensure the parent directory exists
        if let Some(parent) = project_root.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create parent directory for project at {}",
                    parent.display()
                )
            })?;
        }
        tracing::info!(
            target: "dev_setup::nextjs",
            path = %project_root.display(),
            template_url = template_url,
            "Cloning Next.js project template from GitHub..."
        );
        tracing::info!("Cloning template repo...");
        terminal::git::clone_repository(template_url, project_root).await?;
        tracing::info!("Clone complete. Installing dependencies...");
    } else {
        tracing::info!(target: "dev_setup::nextjs", path = %project_root.display(), "Project directory already exists. Skipping clone.");
    }

    // Change to the project directory and run pnpm install
    tracing::info!(
        target: "dev_setup::nextjs",
        path = %project_root.display(),
        "Installing dependencies with pnpm..."
    );

    terminal::pnpm::run_pnpm_command(project_root, &["install"], false)
        .await
        .context("dev_setup::nextjs: Failed to install dependencies with pnpm")?;

    tracing::info!(target: "dev_setup::nextjs", path = %project_root.display(), "Next.js project scaffolded successfully with template and dependencies installed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_scaffold_nextjs_project() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_root = temp_dir.path().join("project");
        let template_url = "https://github.com/Svring/nextjs-project";

        // Run the scaffold function
        let result = scaffold_nextjs_project(&project_root, template_url).await;
        assert!(result.is_ok(), "scaffold_nextjs_project failed: {:?}", result.err());

        // Check for package.json
        let package_json = project_root.join("package.json");
        assert!(package_json.exists(), "package.json was not created");

        // Check for next.config.ts
        let next_config = project_root.join("next.config.ts");
        assert!(next_config.exists(), "next.config.ts was not created");

        // Check for node_modules (should exist if pnpm install succeeded)
        let node_modules = project_root.join("node_modules");
        assert!(node_modules.exists(), "node_modules was not created (pnpm install may have failed)");
    }
} 