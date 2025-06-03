use crate::api::routes::editor_api::EditorApi;
use crate::api::routes::project::ProjectApi;
use anyhow::{Context, Result};
use poem_openapi::OpenApiService;
use std::fs;
use std::path::{Path, PathBuf};
use toml::{map::Map as TomlMap, Value as TomlValue};
use tracing;

/// Creates a 'galatea_files' folder in the same directory as the executable
/// containing config.toml, project_structure.json, and developer_note.md
pub fn create_galatea_files_folder() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;

    let exe_dir = exe_path
        .parent()
        .context("Failed to get executable directory")?;

    let galatea_files_dir = exe_dir.join("galatea_files");

    // Create the galatea_files directory if it doesn't exist
    if !galatea_files_dir.exists() {
        tracing::info!(target: "config_files",
            "Creating galatea_files directory at: {}",
            galatea_files_dir.display()
        );
        fs::create_dir_all(&galatea_files_dir)
            .context("Failed to create galatea_files directory")?;
    } else {
        tracing::info!(target: "config_files", "galatea_files directory already exists at: {}. Ensuring contents.", galatea_files_dir.display());
    }

    // Ensure config.toml exists
    create_empty_file(&galatea_files_dir, "config.toml")?;
    // Ensure project_structure.json exists
    create_empty_file(&galatea_files_dir, "project_structure.json")?;
    // Ensure developer_note.md exists
    create_empty_file(&galatea_files_dir, "developer_note.md")?;

    // Create openapi_specification directory if it doesn't exist
    let openapi_dir = galatea_files_dir.join("openapi_specification");
    if !openapi_dir.exists() {
        fs::create_dir_all(&openapi_dir)
            .context("Failed to create openapi_specification directory")?;
        tracing::info!(target: "config_files", "Created openapi_specification directory at: {}", openapi_dir.display());
    }
    // Always write/overwrite OpenAPI spec files to ensure they are up-to-date
    write_openapi_spec_files(&openapi_dir)?;

    tracing::info!(target: "config_files",
        "Successfully ensured galatea_files folder and its contents are up to date."
    );

    Ok(galatea_files_dir)
}

fn write_openapi_spec_files(openapi_dir: &Path) -> Result<()> {
    // Project API
    let project_api_service = OpenApiService::new(ProjectApi, "Project API", "1.0")
        .server("http://localhost:3051/api/project");
    let project_spec = project_api_service.spec();
    fs::write(openapi_dir.join("project_api.json"), project_spec)
        .context("Failed to write project_api.json")?;

    // Editor API
    let editor_api_service = OpenApiService::new(EditorApi, "Editor API", "1.0")
        .server("http://localhost:3051/api/editor");
    let editor_spec = editor_api_service.spec();
    fs::write(openapi_dir.join("editor_api.json"), editor_spec)
        .context("Failed to write editor_api.json")?;

    Ok(())
}

/// Helper to create an empty file with the given name in the specified directory
fn create_empty_file(dir: &Path, filename: &str) -> Result<()> {
    let file_path = dir.join(filename);
    if !file_path.exists() {
        fs::File::create(&file_path)
            .with_context(|| format!("Failed to create empty file {}", file_path.display()))?;
        tracing::debug!(target: "config_files", "Created empty file: {}", file_path.display());
    } else {
        tracing::debug!(target: "config_files", "File {} already exists. Skipping creation of empty file.", file_path.display());
    }
    Ok(())
}

/// Write or update a key-value pair in config.toml
pub fn set_config_value(key: &str, value: &str) -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path
        .parent()
        .context("Failed to get executable directory")?;
    let config_path = exe_dir.join("galatea_files").join("config.toml");

    // Read existing config if present
    let mut config: TomlMap<String, TomlValue> = if config_path.exists() {
        let content =
            std::fs::read_to_string(&config_path).context("Failed to read config.toml")?;
        content
            .parse::<TomlValue>()
            .unwrap_or(TomlValue::Table(TomlMap::new()))
            .as_table()
            .cloned()
            .unwrap_or(TomlMap::new())
    } else {
        TomlMap::new()
    };

    config.insert(key.to_string(), TomlValue::String(value.to_string()));
    let new_content = TomlValue::Table(config).to_string();
    std::fs::write(&config_path, new_content).context("Failed to write config.toml")?;
    Ok(())
}

/// Get a value by key from config.toml
pub fn get_config_value(key: &str) -> Option<String> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;
    let config_path = exe_dir.join("galatea_files").join("config.toml");
    if !config_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&config_path).ok()?;
    let value: toml::Value = content.parse().ok()?;
    value.get(key)?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_galatea_files_folder() {
        // Create a temporary directory to simulate the executable directory
        let temp_dir = tempdir().unwrap();
        let temp_exe_path = temp_dir.path().join("galatea");

        // Create a mock executable file
        fs::write(&temp_exe_path, "mock executable").unwrap();

        // Temporarily change the current exe path for testing
        // Note: This is a simplified test - in reality, we'd need to mock std::env::current_exe()
        // For now, we'll test the individual file creation functions

        let galatea_files_dir = temp_dir.path().join("galatea_files");
        fs::create_dir_all(&galatea_files_dir).unwrap();

        // Test individual file creation functions
        assert!(create_empty_file(&galatea_files_dir, "config.toml").is_ok());
        // Verify file was created
        assert!(galatea_files_dir.join("config.toml").exists());
    }
}
