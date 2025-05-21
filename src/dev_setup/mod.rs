pub mod nextjs;
pub mod codex;
pub mod env;

use anyhow::{Context, Result};
use std::path::Path;
use tracing;

async fn ensure_project_setup_internal(project_root: &Path, api_key: Option<String>) -> Result<()> {
    tracing::info!(target: "dev_setup", "Starting internal project setup checks...");

    // First, ensure the basic Next.js project structure and package.json exist.
    // If this fails (e.g., package.json missing), it will trigger reinitialization sooner.
    tracing::info!(target: "dev_setup", "Ensuring Next.js project dependencies and scripts...");
    nextjs::ensure_project_dependencies_and_scripts(project_root).await?;
    tracing::info!(target: "dev_setup", "Next.js project dependencies and scripts checked.");

    // Then, attempt to install @openai/codex CLI.
    tracing::info!(target: "dev_setup", "Ensuring @openai/codex CLI is installed...");
    codex::ensure_codex_cli_installed(project_root).await?;
    tracing::info!(target: "dev_setup", "@openai/codex CLI installation check completed.");

    tracing::info!(target: "dev_setup", "Ensuring Next.js project configuration (rewrites)...");
    nextjs::ensure_next_config(project_root).await?;
    tracing::info!(target: "dev_setup", "Next.js project configuration (rewrites) checked.");

    tracing::info!(target: "dev_setup", "Ensuring .codex directory structure and config.json...");
    codex::ensure_codex_config(project_root).await?;
    tracing::info!(target: "dev_setup", ".codex directory structure and config.json checked.");

    tracing::info!(target: "dev_setup", "Ensuring .env file if API key is provided...");
    env::ensure_env_file(project_root, api_key.as_deref()).await?;
    tracing::info!(target: "dev_setup", ".env file check completed.");

    tracing::info!(target: "dev_setup", "Internal project setup checks completed successfully.");
    Ok(())
}

pub async fn ensure_development_environment(project_root: &Path, api_key: Option<String>) -> Result<()> {
    tracing::info!(target: "dev_setup", path = %project_root.display(), "Attempting to ensure development environment...");

    match ensure_project_setup_internal(project_root, api_key.clone()).await {
        Ok(_) => {
            tracing::info!(target: "dev_setup", path = %project_root.display(), "Development environment successfully ensured on first attempt.");
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                target: "dev_setup",
                path = %project_root.display(),
                error = %e,
                "Initial attempt to ensure development environment failed. Attempting to reinitialize project."
            );

            nextjs::reinitialize_nextjs_project(project_root).await.context("Failed to reinitialize Next.js project after initial setup failure.")?;
            
            tracing::info!(target: "dev_setup", path = %project_root.display(), "Project reinitialized. Retrying environment setup...");
            ensure_project_setup_internal(project_root, api_key).await.context("Failed to ensure development environment even after reinitializing project.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // Imports items from the parent module (src/dev_setup/mod.rs)
    use std::fs;
    use tempfile::tempdir;

    // Helper function to initialize tracing for tests, if not already initialized.
    // This can be useful if tests are run in a way that doesn't init global tracing.
    fn ensure_tracing_initialized() {
        // Attempt to set a global default subscriber. If one is already set,
        // this will fail, which is fine – we just ensure one is set.
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    }


    #[tokio::test]
    async fn test_ensure_development_environment_with_api_key() {
        ensure_tracing_initialized();
        let base_temp_dir = tempdir().unwrap();

        let project_root_temp = base_temp_dir.path().join("project");
        fs::create_dir_all(&project_root_temp).unwrap();

        let api_key = "test_api_key_123".to_string();

        let result = ensure_development_environment(&project_root_temp, Some(api_key.clone())).await;
        assert!(result.is_ok(), "ensure_development_environment failed: {:?}", result.err());

        // Check for .codex/config.json in parent of project_root_temp
        let parent_dir = project_root_temp.parent().unwrap();
        let codex_config_path = parent_dir.join(".codex").join("config.json");
        assert!(codex_config_path.exists(), ".codex/config.json was not created");
        let codex_config_content = fs::read_to_string(codex_config_path).unwrap();
        // Check for a few key fields, assuming the default static content is used.
        assert!(codex_config_content.contains("\"model\": \"o3\""));
        assert!(codex_config_content.contains("\"provider\": \"sealos\""));

        // Check for .env file in project_root_temp
        let env_file_path = project_root_temp.join(".env");
        assert!(env_file_path.exists(), ".env file was not created");
        let env_content = fs::read_to_string(env_file_path).unwrap();
        assert_eq!(env_content, format!("export OPENAI_API_KEY=\"{}\"", api_key));

        // Check for package.json (Next.js dependency setup)
        let package_json_path = project_root_temp.join("package.json");
        assert!(package_json_path.exists(), "package.json was not created");
        // Further checks on package.json content can be added if necessary
        // For example, checking for specific scripts or dependencies.
        let package_json_content = fs::read_to_string(package_json_path).unwrap();
        assert!(package_json_content.contains("\"next\":")); 

        // Check for next.config.ts (Next.js config setup)
        // It could also be .js or .mjs, this test assumes .ts for simplicity of checking existence.
        let next_config_path_ts = project_root_temp.join("next.config.ts");
        // let next_config_path_js = project_root_temp.join("next.config.js");
        // let next_config_path_mjs = project_root_temp.join("next.config.mjs");
        assert!(
            next_config_path_ts.exists(),
            // || next_config_path_js.exists() 
            // || next_config_path_mjs.exists(),
            "next.config.[ts/js/mjs] was not created"
        );
        if next_config_path_ts.exists() {
            let next_config_content = fs::read_to_string(next_config_path_ts).unwrap();
            assert!(next_config_content.contains("source: \"/galatea/:path*\""));
        }
        // Similar content checks if .js or .mjs were found.
    }

    #[tokio::test]
    async fn test_ensure_development_environment_without_api_key() {
        ensure_tracing_initialized();
        let base_temp_dir = tempdir().unwrap();

        let project_root_temp = base_temp_dir.path().join("project");
        fs::create_dir_all(&project_root_temp).unwrap();

        let result = ensure_development_environment(&project_root_temp, None).await;
        assert!(result.is_ok(), "ensure_development_environment failed without API key: {:?}", result.err());

        // Check for .codex/config.json (should still be created)
        let parent_dir = project_root_temp.parent().unwrap();
        let codex_config_path = parent_dir.join(".codex").join("config.json");
        assert!(codex_config_path.exists(), ".codex/config.json was not created without API key");

        // .env file should NOT be created
        let env_file_path = project_root_temp.join(".env");
        assert!(!env_file_path.exists(), ".env file was created even without API key");
        
        // package.json and next.config should still be created
        let package_json_path = project_root_temp.join("package.json");
        assert!(package_json_path.exists(), "package.json was not created without API key");

        let next_config_path_ts = project_root_temp.join("next.config.ts");
        assert!(
            next_config_path_ts.exists(),
            "next.config.ts was not created without API key"
        );
    }
}