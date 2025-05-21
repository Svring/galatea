use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tracing;

pub async fn ensure_env_file(project_root: &Path, api_key_opt: Option<&str>) -> Result<()> {
    if let Some(api_key) = api_key_opt {
        let env_file_path = project_root.join(".env");
        let env_content = format!("OPENAI_API_KEY=\"{}\"", api_key);

        tracing::info!(
            target: "dev_setup::env",
            path = %env_file_path.display(),
            "Ensuring .env file with OPENAI_API_KEY."
        );

        fs::write(&env_file_path, &env_content)
            .map_err(|e| {
                tracing::error!(
                    target: "dev_setup::env",
                    path = %env_file_path.display(),
                    content = %env_content,
                    error = %e,
                    "Failed to write .env file."
                );
                e
            })
            .context(format!(
                "Failed to write .env file at {}",
                env_file_path.display()
            ))?;

        tracing::info!(
            target: "dev_setup::env",
            path = %env_file_path.display(),
            ".env file written successfully."
        );
    } else {
        tracing::debug!(
            target: "dev_setup::env",
            "No API key provided, skipping .env file creation."
        );
    }
    Ok(())
}
