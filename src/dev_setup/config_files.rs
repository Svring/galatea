use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing;

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
    
    // Create project_structure.json
    create_empty_file(&galatea_files_dir, "project_structure.json")?;
    
    // Create developer_note.md
    create_empty_file(&galatea_files_dir, "developer_note.md")?;
    
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
        assert!(create_empty_file(&galatea_files_dir, "project_structure.json").is_ok());
        assert!(create_empty_file(&galatea_files_dir, "developer_note.md").is_ok());
        
        // Verify files were created
        assert!(galatea_files_dir.join("config.toml").exists());
        assert!(galatea_files_dir.join("project_structure.json").exists());
        assert!(galatea_files_dir.join("developer_note.md").exists());
    }
}
