use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing;
use toml::{Value as TomlValue, map::Map as TomlMap};
use std::collections::BTreeMap;

/// Creates a 'galatea_files' folder in the same directory as the executable
/// containing config.toml, project_structure.json, and developer_note.md
pub fn create_galatea_files_folder() -> Result<PathBuf> {
    let exe_path = std::env::current_exe()
        .context("Failed to get current executable path")?;
    
    let exe_dir = exe_path.parent()
        .context("Failed to get executable directory")?;
    
    let galatea_files_dir = exe_dir.join("galatea_files");
    
    if galatea_files_dir.exists() {
        tracing::info!(target: "config_files", "galatea_files directory already exists at: {}. Skipping creation.", galatea_files_dir.display());
        return Ok(galatea_files_dir);
    }
    
    tracing::info!(target: "config_files", 
        "Creating galatea_files directory at: {}", 
        galatea_files_dir.display()
    );
    
    // Create the galatea_files directory
    fs::create_dir_all(&galatea_files_dir)
        .context("Failed to create galatea_files directory")?;
    
    // Create config.toml
    create_empty_file(&galatea_files_dir, "config.toml")?;
    
    tracing::info!(target: "config_files", 
        "Successfully created galatea_files folder with all configuration files"
    );
    
    Ok(galatea_files_dir)
}

/// Helper to create an empty file with the given name in the specified directory
fn create_empty_file(dir: &Path, filename: &str) -> Result<()> {
    let file_path = dir.join(filename);
    fs::File::create(&file_path)
        .with_context(|| format!("Failed to create empty file {}", file_path.display()))?;
    tracing::debug!(target: "config_files", "Created empty file: {}", file_path.display());
    Ok(())
}

/// Write or update a key-value pair in config.toml
pub fn set_config_value(key: &str, value: &str) -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = exe_path.parent().context("Failed to get executable directory")?;
    let config_path = exe_dir.join("galatea_files").join("config.toml");

    // Read existing config if present
    let mut config: TomlMap<String, TomlValue> = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .context("Failed to read config.toml")?;
        content.parse::<TomlValue>()
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
