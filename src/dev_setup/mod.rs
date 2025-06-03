pub mod codex;
pub mod config_files;
pub mod env;
pub mod nextjs;
pub mod mcp_converter;

use anyhow::{Context, Result};
use tracing;

pub async fn ensure_development_environment(
    template: Option<String>,
) -> Result<std::path::PathBuf> {
    tracing::info!(target: "dev_setup", "Attempting to ensure development environment...");

    // Ensure galatea_files folder and its essential contents exist or are created/updated.
    // This function is now designed to be idempotent and safe to call even if files exist.
    config_files::create_galatea_files_folder()
        .context("Failed to ensure galatea_files folder and its contents")?;

    // Get current working directory and determine project_dir_path
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path
        .parent()
        .context("Failed to get executable directory")?;
    let project_dir_path = exe_dir.join("project");

    // Use custom template if provided, otherwise use default
    let template_url = match template.as_deref() {
        Some("nextjs") => "https://github.com/Svring/nextjs-project",
        Some(url) => url,
        None => "https://github.com/Svring/nextjs-project", // Default template
    };

    // If the project directory doesn't exist, scaffold it.
    // Otherwise, assume it's correctly set up or managed externally.
    if !project_dir_path.exists() {
        tracing::info!(target: "dev_setup", 
            "Project directory {} does not exist. Scaffolding Next.js project from template: {}", 
            project_dir_path.display(), template_url
        );
        nextjs::scaffold_nextjs_project(&project_dir_path, template_url)
            .await
            .context("Failed to scaffold Next.js project")?;
        tracing::info!(target: "dev_setup", path = %project_dir_path.display(), "Next.js project scaffolded successfully.");
    } else {
        tracing::info!(target: "dev_setup", 
            "Project directory {} already exists. Skipping Next.js project scaffolding.", 
            project_dir_path.display()
        );
    }

    // Ensure openapi-mcp-generator is installed globally
    mcp_converter::ensure_openapi_mcp_generator_installed().await?;

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
        
        // The project directory path that ensure_development_environment will use
        let exe_path_for_test = std::env::current_exe().unwrap();
        let exe_dir_for_test = exe_path_for_test.parent().unwrap();
        let project_dir_to_be_created = exe_dir_for_test.join("project");

        // Ensure the project directory does NOT exist before the test
        if project_dir_to_be_created.exists() {
            fs::remove_dir_all(&project_dir_to_be_created).unwrap();
        }

        // Remove galatea_files if it exists from previous runs to ensure a clean state for this part of the test
        let galatea_files_dir = exe_dir_for_test.join("galatea_files");
        if galatea_files_dir.exists() {
            fs::remove_dir_all(&galatea_files_dir).unwrap();
        }

        let result = ensure_development_environment(Some("nextjs".to_string())).await;
        assert!(
            result.is_ok(),
            "ensure_development_environment failed: {:?}",
            result.err()
        );
        let final_project_path = result.unwrap();
        assert_eq!(final_project_path, project_dir_to_be_created, "Returned project path does not match expected.");

        // Check galatea_files and its files
        assert!(
            galatea_files_dir.exists(),
            "galatea_files directory was not created at {}", galatea_files_dir.display()
        );
        assert!(
            galatea_files_dir.join("config.toml").exists(),
            "config.toml was not created in {}", galatea_files_dir.display()
        );
        assert!(
            galatea_files_dir.join("project_structure.json").exists(),
            "project_structure.json was not created in {}", galatea_files_dir.display()
        );
        assert!(
            galatea_files_dir.join("developer_note.md").exists(),
            "developer_note.md was not created in {}", galatea_files_dir.display()
        );
        assert!(
            galatea_files_dir.join("openapi_specification").exists(),
            "openapi_specification directory was not created in {}", galatea_files_dir.display()
        );
        assert!(
            galatea_files_dir.join("openapi_specification").join("project_api.json").exists(),
            "project_api.json was not created in openapi_specification"
        );
        assert!(
            galatea_files_dir.join("openapi_specification").join("editor_api.json").exists(),
            "editor_api.json was not created in openapi_specification"
        );

        // Check Next.js project files (assuming the template creates these)
        // The project path is now `final_project_path` (which is `project_dir_to_be_created`)
        assert!(final_project_path.exists(), "Project directory {} was not created", final_project_path.display());
        let package_json = final_project_path.join("package.json");
        assert!(package_json.exists(), "package.json was not created in {}", final_project_path.display());
        let next_config = final_project_path.join("next.config.ts"); // Assuming .ts for the template
        assert!(next_config.exists(), "next.config.ts was not created in {}", final_project_path.display());
        // node_modules might take time or not be created by scaffold alone, depending on template. 
        // This assertion is kept but might be a source of flakes if the template doesn't guarantee it.
        let node_modules = final_project_path.join("node_modules");
        assert!(node_modules.exists(), "node_modules directory was not created in {}", final_project_path.display());
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
