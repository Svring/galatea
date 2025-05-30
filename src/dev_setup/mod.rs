pub mod codex;
pub mod config_files;
pub mod env;
pub mod nextjs;

use anyhow::{Context, Result};
use tracing;

pub async fn ensure_development_environment(
    template: Option<String>,
) -> Result<std::path::PathBuf> {
    tracing::info!(target: "dev_setup", "Attempting to ensure development environment...");

    // Get current working directory and determine project_dir_path
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path
        .parent()
        .context("Failed to get executable directory")?;
    let project_dir_path = exe_dir.join("project");

    // Ensure galatea_files folder exists (create if not)
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path
        .parent()
        .context("Failed to get executable directory")?;
    let galatea_files_dir = exe_dir.join("galatea_files");
    if !galatea_files_dir.exists() {
        config_files::create_galatea_files_folder()?;
    }

    // Use custom template if provided, otherwise use default
    let template_url = match template.as_deref() {
        Some("nextjs") => "https://github.com/Svring/nextjs-project",
        Some(url) => url,
        None => "https://github.com/Svring/nextjs-project",
    };

    nextjs::scaffold_nextjs_project(&project_dir_path, template_url)
        .await
        .context("Failed to scaffold Next.js project after initial setup failure.")?;

    tracing::info!(target: "dev_setup", path = %project_dir_path.display(), "Next.js project scaffolded successfully.");

    Ok(project_dir_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ensure_tracing_initialized() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    }

    #[tokio::test]
    async fn test_ensure_development_environment_creates_config_and_nextjs() {
        ensure_tracing_initialized();
        let base_temp_dir = tempdir().unwrap();
        let _guard = std::env::set_current_dir(&base_temp_dir);
        let project_root_temp = base_temp_dir.path().join("project");
        fs::create_dir_all(&project_root_temp).unwrap();

        // Remove galatea_files if it exists from previous runs
        let exe_path = std::env::current_exe().unwrap();
        let exe_dir = exe_path.parent().unwrap();
        let galatea_files_dir = exe_dir.join("galatea_files");
        if galatea_files_dir.exists() {
            fs::remove_dir_all(&galatea_files_dir).unwrap();
        }

        let result = ensure_development_environment(Some("nextjs".to_string())).await;
        assert!(
            result.is_ok(),
            "ensure_development_environment failed: {:?}",
            result.err()
        );

        // Check galatea_files and its files
        assert!(
            galatea_files_dir.exists(),
            "galatea_files directory was not created"
        );
        assert!(
            galatea_files_dir.join("config.toml").exists(),
            "config.toml was not created"
        );
        assert!(
            galatea_files_dir.join("project_structure.json").exists(),
            "project_structure.json was not created"
        );
        assert!(
            galatea_files_dir.join("developer_note.md").exists(),
            "developer_note.md was not created"
        );

        // Check Next.js project files
        let package_json = project_root_temp.join("package.json");
        assert!(package_json.exists(), "package.json was not created");
        let next_config = project_root_temp.join("next.config.ts");
        assert!(next_config.exists(), "next.config.ts was not created");
        let node_modules = project_root_temp.join("node_modules");
        assert!(node_modules.exists(), "node_modules was not created");
    }
}

// async fn ensure_project_setup_internal(project_root: &Path, api_key: Option<String>) -> Result<()> {
//     tracing::info!(target: "dev_setup", "Starting internal project setup checks...");

//     // The Next.js project structure, dependencies, and scripts are already provided by the template
//     // No need to ensure them separately since the template is pre-configured

//     // Install @openai/codex CLI.
//     tracing::info!(target: "dev_setup", "Ensuring @openai/codex CLI is installed...");
//     codex::ensure_codex_cli_installed(project_root).await?;
//     tracing::info!(target: "dev_setup", "@openai/codex CLI installation check completed.");

//     // The Next.js configuration (rewrites) should already be in the template
//     // No need to ensure it separately

//     tracing::info!(target: "dev_setup", "Ensuring .codex directory structure and config.json...");
//     codex::ensure_codex_config(project_root).await?;
//     tracing::info!(target: "dev_setup", ".codex directory structure and config.json checked.");

//     tracing::info!(target: "dev_setup", "Ensuring .env file if API key is provided...");
//     env::ensure_env_file(project_root, api_key.as_deref()).await?;
//     tracing::info!(target: "dev_setup", ".env file check completed.");

//     tracing::info!(target: "dev_setup", "Internal project setup checks completed successfully.");
//     Ok(())
// }
