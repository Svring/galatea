use anyhow::{anyhow, ensure, Context, Result};
use lsp_types::Uri;
use std::path::{Path, PathBuf};
use std::str::FromStr;

// Changed from crate::file_system to super::search for relative import within the same module
use super::search;

// Transplanted from watcher.rs
pub fn get_project_root() -> Result<PathBuf> {
    let project_dir = std::env::current_exe()
        .context("Failed to get current executable path")?
        .parent()
        .ok_or_else(|| anyhow!("Executable has no parent directory"))?
        .join("project");

    ensure!(
        project_dir.is_dir(),
        "'project' subdirectory not found in {}.",
        project_dir.display()
    );

    Ok(project_dir)
}

/// Resolves an input path string to a canonicalized `PathBuf` within the project root.
///
/// The input can be absolute, relative, or incomplete. The process:
/// 1. Normalizes the input (removes "file://" prefix, handles Windows paths).
/// 2. Attempts direct resolution:
///    - Absolute paths within the project root are used directly.
///    - Absolute paths outside the root use the filename joined with the project root.
///    - Relative or incomplete paths are joined with the project root.
/// 3. Falls back to `search::find_file_by_suffix` to search for files matching the input suffix.
/// The resolved path is canonicalized and verified to exist within the project root.
pub fn resolve_path(input_path: &str) -> Result<PathBuf> {
    let proj_root = get_project_root()?;
    let path = PathBuf::from(input_path.trim());

    let candidate = match (path.is_absolute(), path.starts_with(&proj_root)) {
        // Absolute path already within the project root
        (true, true) => path,
        // Absolute path outside the project root – use the filename joined with the project root
        (true, false) => proj_root.join(path.file_name().unwrap_or_default()),
        // Relative (or otherwise non-absolute) path – strip optional "project" prefix and join
        (false, _) => {
            let stripped = path
                .strip_prefix(proj_root.file_name().unwrap_or_default())
                .unwrap_or(&path);
            proj_root.join(stripped)
        }
    };

    // Attempt to canonicalize the direct candidate and validate it in one `match`
    match dunce::canonicalize(&candidate) {
        Ok(canonical) if canonical.exists() && canonical.starts_with(&proj_root) => {
            return Ok(canonical);
        }
        _ => { /* fall-through to search fallback */ }
    }

    // Fallback to search using the new centralized function
    if let Some(found_path) = 
        search::find_file_by_suffix(&proj_root, input_path)?
    {
        return Ok(found_path);
    }
    
    Err(anyhow!(
        "Failed to resolve '{}' within project root '{}'",
        input_path,
        proj_root.display()
    ))
}

pub fn resolve_path_to_uri<P: AsRef<Path>>(input_path_like: P) -> Result<Uri> {
    let path_ref: &Path = input_path_like.as_ref();
    let path_str_for_resolver = path_ref.to_string_lossy();

    let resolved_canonical_path = resolve_path(&path_str_for_resolver)?;

    let uri_string = resolved_canonical_path.to_string_lossy().into_owned();
    let uri = Uri::from_str(&uri_string).context(format!(
        "Failed to convert path {} to URI",
        resolved_canonical_path.display()
    ))?;

    Ok(uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_resolve_path() -> Result<()> {
        // Setup a temporary project root for testing
        let temp_dir = std::env::current_exe()?
            .parent()
            .context("Executable has no parent directory")?
            .join("project");
        fs::create_dir_all(&temp_dir)?;

        // get_project_root() will now correctly point to temp_dir if tests are run correctly
        let project_root = get_project_root().unwrap();

        let test_file = project_root.join("src").join("app.tsx");
        fs::create_dir_all(test_file.parent().unwrap())?;
        fs::write(&test_file, "test content")?;

        let expected_result = dunce::canonicalize(&test_file)?; // Changed to dunce::canonicalize

        // Test 1: Absolute path within project root
        let input = format!("{}", test_file.to_string_lossy());
        let result = resolve_path(&input)?;
        println!(
            "Test 1: absolute input: {}\nResult: {:?}\nExpected: {:?}",
            input, result, expected_result
        );
        assert_eq!(result, expected_result);

        // Test 2: Relative path
        let input = "src/app.tsx";
        let result = resolve_path(input)?;
        println!(
            "Test 2: relative input: {}\nResult: {:?}\nExpected: {:?}",
            input, result, expected_result
        );
        assert_eq!(result, expected_result);

        // Test 3: Path with filename only, using search fallback
        let input = "app.tsx";
        let result = resolve_path(input)?;
        println!(
            "Test 3: filename input: {}\nResult: {:?}\nExpected: {:?}",
            input, result, expected_result
        );
        assert_eq!(result, expected_result);

        // Test 4: Non-existent path should fail
        let input = "nonexistent/file.ts";
        assert!(resolve_path(input).is_err());

        // Test 5: Relative path starting with project directory name
        let input = "project/src/app.tsx";
        let result = resolve_path(input)?;
        println!(
            "Test 5: project-prefixed relative input: {}\nResult: {:?}\nExpected: {:?}",
            input, result, expected_result
        );
        assert_eq!(result, expected_result);

        // Note: No cleanup here to avoid race conditions with parallel tests.
        Ok(())
    }

    #[test]
    fn test_get_project_root() -> Result<()> {
        // Ensure the expected project directory exists
        let temp_dir = std::env::current_exe()?
            .parent()
            .context("Executable has no parent directory")?
            .join("project");
        fs::create_dir_all(&temp_dir)?;

        // get_project_root should now return the same directory
        let project_root = get_project_root()?;
        let expected_root = dunce::canonicalize(&temp_dir)?;
        assert_eq!(project_root, expected_root);

        // Note: No cleanup here to avoid race conditions with parallel tests.
        Ok(())
    }

    #[test]
    fn test_resolve_path_to_uri() -> Result<()> {
        // Prepare a temporary project and file
        let temp_dir = std::env::current_exe()?
            .parent()
            .context("Executable has no parent directory")?
            .join("project");
        fs::create_dir_all(&temp_dir)?;

        let project_root = get_project_root()?;
        let test_file = project_root.join("src").join("app.tsx");
        fs::create_dir_all(test_file.parent().unwrap())?;
        fs::write(&test_file, "test content")?;

        // Expected URI (based on canonicalized file path)
        let canonical_path = dunce::canonicalize(&test_file)?;
        let expected_uri = Uri::from_str(&canonical_path.to_string_lossy())?;

        // Resolve using the function under test
        let result_uri = resolve_path_to_uri("src/app.tsx")?;

        assert_eq!(result_uri, expected_uri);

        // Note: No cleanup here to avoid race conditions with parallel tests.
        Ok(())
    }
} 