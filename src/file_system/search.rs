use anyhow::{anyhow, Context, Result};
use dunce;
use std::fs;
use std::path::{Path, PathBuf};

/// Recursively finds files within a directory that match the given extensions,
/// excluding specified directory names.
///
/// # Arguments
///
/// * `start_path` - The directory path to start searching from.
/// * `extensions` - A slice of file extensions to match (e.g., &["rs", "ts", "tsx"]).
/// * `exclude_dirs` - A slice of directory names to exclude (e.g., &["node_modules", "target"]).
///
/// # Returns
///
/// A `Result` containing a vector of `PathBuf`s for matching files, or an error.
pub fn find_files_by_extensions(
    start_path: &Path,
    extensions: &[&str],
    exclude_dirs: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut matching_files = Vec::new();
    find_files_recursive(start_path, extensions, exclude_dirs, &mut matching_files).context(
        anyhow!("Failed to scan directory: {}", start_path.display()),
    )?;
    Ok(matching_files)
}

fn find_files_recursive(
    current_path: &Path,
    extensions: &[&str],
    exclude_dirs: &[&str],
    matching_files: &mut Vec<PathBuf>,
) -> Result<()> {
    // Combined guard: Skip if not a directory or if directory is in exclude list or is hidden (starts with '.')
    if !current_path.is_dir()
        || current_path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |dir_name| exclude_dirs.contains(&dir_name) || dir_name.starts_with('.'))
    {
        return Ok(());
    }

    // Iterate over entries in the current directory.
    for entry_result in fs::read_dir(current_path)? {
        let entry = entry_result?;
        let path = entry.path();

        // If the entry is a directory, recurse into it.
        // Then, `continue` to the next entry in the current directory.
        if path.is_dir() {
            find_files_recursive(&path, extensions, exclude_dirs, matching_files)?;
            continue;
        }

        // If the entry is a file (and not a directory, due to `continue` above),
        // check if its extension matches the desired extensions.
        if path.is_file() {
            let matches_suffix = path
                .extension()
                .and_then(|ext| ext.to_str()) // Get Option<&str> for the extension
                .map_or(false, |ext_str| extensions.contains(&ext_str)); // Check if non-empty extension is in extensions

            if matches_suffix {
                matching_files.push(path);
            }
        }
        // Other types of file system entries (e.g., symlinks not pointing to dirs/files) are ignored.
    }
    Ok(())
}

/// Searches for a file within a project directory that exactly matches a given extension string.
///
/// This function uses a predefined list of common source file extensions and
/// common directories to exclude (like node_modules, .git, target).
///
/// # Arguments
///
/// * `project_root` - The root directory of the project to search within.
/// * `input_path_suffix` - The exact suffix string the filename should end with (e.g., "app.tsx" or "utils/helpers.rs").
///
/// # Returns
///
/// A `Result` containing:
///   - `Ok(Some(PathBuf))` if a matching, existing, canonicalized file is found.
///   - `Ok(None)` if no such file is found.
///   - `Err(anyhow::Error)` if there's an issue during the search (e.g., reading directories).
pub fn find_file_by_suffix(
    project_root: &Path,
    input_path_suffix: &str,
) -> Result<Option<PathBuf>> {
    // Predefined extensions (formerly suffixes) and exclude_dirs
    let extensions_to_scan = [
        "ts", "tsx", "js", "jsx", "rs", "json", "py", "go", "java", "html", "css", "md", "txt",
        "yaml", "yml", "toml", "sh", "rb", "php", "c", "cpp", "h", "hpp", "cs", "fs", "dart", "kt",
        "swift", "scala", "pl", "pm", "lua",
    ];
    let exclude_dirs = [
        "node_modules",
        ".git",
        "target",
        "dist",
        "build",
        ".vscode",
        ".idea",
    ];

    let candidate_files =
        find_files_by_extensions(project_root, &extensions_to_scan, &exclude_dirs)?;

    let mut found_matches: Vec<PathBuf> = Vec::new();

    for scanned_file_path in candidate_files {
        // Skip files outside the project root
        if !scanned_file_path.starts_with(project_root) {
            continue;
        }

        let input_path_is_absolute = Path::new(input_path_suffix).is_absolute();

        // Determine if the file matches our criteria
        let matched_by_criteria = match input_path_is_absolute {
            true => {
                // For absolute paths, compare canonicalized paths
                dunce::canonicalize(&scanned_file_path)
                    .map(|canonical_scanned_file| {
                        canonical_scanned_file == PathBuf::from(input_path_suffix)
                    })
                    .unwrap_or(false)
            }
            false => {
                // For relative paths, check if the string representation ends with the suffix
                scanned_file_path
                    .to_string_lossy()
                    .ends_with(input_path_suffix)
            }
        };

        // If matched, try to canonicalize and add to results
        if matched_by_criteria {
            match dunce::canonicalize(&scanned_file_path) {
                Ok(canonical_path_to_store) if canonical_path_to_store.exists() => {
                    found_matches.push(canonical_path_to_store);
                }
                _ => { /* Path couldn't be canonicalized or doesn't exist after canonicalization */
                }
            }
        }
    }

    // Handle results based on the number of matches found
    match found_matches.len() {
        0 => Ok(None),                // No matches found
        1 => Ok(found_matches.pop()), // .pop() is safe as len is 1, returns Some(PathBuf)
        _ => {
            let matches_str = found_matches
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<String>>()
                .join(", \n  ");
            Err(anyhow!(
                "Input path suffix '{}' is ambiguous and matches multiple files:\n  {}",
                input_path_suffix,
                matches_str
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::tempdir;

    #[test]
    fn test_find_files_with_exclusion() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        let sub1 = root.join("subdir1");
        let sub2 = root.join("node_modules");
        let sub3 = root.join("target");
        let sub4 = sub1.join("nested");
        let sub5 = sub2.join("some_package");

        fs::create_dir_all(&sub1)?;
        fs::create_dir_all(&sub2)?;
        fs::create_dir_all(&sub3)?;
        fs::create_dir_all(&sub4)?;
        fs::create_dir_all(&sub5)?;

        File::create(root.join("root.rs"))?;
        File::create(sub1.join("sub1.ts"))?;
        File::create(sub4.join("nested.tsx"))?;
        File::create(sub2.join("ignored.ts"))?;
        File::create(sub3.join("ignored.rs"))?;
        File::create(sub5.join("deep_ignored.tsx"))?;

        let extensions = ["rs", "ts", "tsx"];
        let exclude = ["node_modules", "target"];
        let found_files = find_files_by_extensions(root, &extensions, &exclude)?;

        let mut found_paths: Vec<String> = found_files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        found_paths.sort();

        let mut expected_paths = vec![
            root.join("root.rs").to_string_lossy().into_owned(),
            sub1.join("sub1.ts").to_string_lossy().into_owned(),
            sub4.join("nested.tsx").to_string_lossy().into_owned(),
        ];
        expected_paths.sort();

        assert_eq!(
            found_paths, expected_paths,
            "Should exclude node_modules and target"
        );

        let no_exclude: [&str; 0] = [];
        let all_found = find_files_by_extensions(root, &extensions, &no_exclude)?;
        assert_eq!(
            all_found.len(),
            6,
            "Should find all 6 files when no exclusions"
        );

        Ok(())
    }

    #[test]
    fn test_find_files() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        let sub = root.join("subdir");
        fs::create_dir(&sub)?;

        File::create(root.join("file1.rs"))?;
        File::create(root.join("file2.txt"))?;
        File::create(sub.join("file3.ts"))?;
        File::create(sub.join("file4.tsx"))?;
        File::create(sub.join("file5.rs"))?;

        let extensions = ["rs", "tsx"];
        let exclude: [&str; 0] = [];
        let found_files = find_files_by_extensions(root, &extensions, &exclude)?;

        let mut found_paths: Vec<String> = found_files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        found_paths.sort();

        let mut expected_paths = vec![
            root.join("file1.rs").to_string_lossy().into_owned(),
            sub.join("file4.tsx").to_string_lossy().into_owned(),
            sub.join("file5.rs").to_string_lossy().into_owned(),
        ];
        expected_paths.sort();

        assert_eq!(found_paths, expected_paths);

        let ts_files = find_files_by_extensions(root, &["ts"], &exclude)?;
        assert_eq!(ts_files.len(), 1);
        assert_eq!(ts_files[0], sub.join("file3.ts"));

        let no_files = find_files_by_extensions(root, &["java", "py"], &exclude)?;
        assert!(no_files.is_empty());

        Ok(())
    }

    #[test]
    fn test_find_file_by_suffix() -> Result<()> {
        let dir = tempdir()?;
        let project_root = dir.path();

        // Setup a file structure
        let src_dir = project_root.join("src");
        let components_dir = src_dir.join("components");
        let module_dir = project_root.join("module");
        let assets_dir = project_root.join("assets");

        fs::create_dir_all(&components_dir)?;
        fs::create_dir_all(&module_dir)?;
        fs::create_dir_all(&assets_dir)?;

        File::create(project_root.join("main.rs"))?;
        File::create(src_dir.join("app.tsx"))?;
        File::create(components_dir.join("button.tsx"))?;
        File::create(components_dir.join("utils.rs"))?; // For ambiguity
        File::create(module_dir.join("utils.rs"))?; // For ambiguity
        File::create(assets_dir.join("image.png"))?; // Extension not in default scan list

        // Test 1: Unique match - full name in subdirectory
        let expected_path_app_tsx = dunce::canonicalize(src_dir.join("app.tsx"))?;
        let result = find_file_by_suffix(project_root, "app.tsx")?;
        assert_eq!(
            result,
            Some(expected_path_app_tsx),
            "Test 1 failed: app.tsx full name"
        );

        // Test 2: Unique match - relative path + name
        let expected_path_button_tsx = dunce::canonicalize(components_dir.join("button.tsx"))?;
        let result = find_file_by_suffix(project_root, "components/button.tsx")?;
        assert_eq!(
            result,
            Some(expected_path_button_tsx),
            "Test 2 failed: components/button.tsx relative path"
        );

        // Test 3: Unique match - root file
        let expected_path_main_rs = dunce::canonicalize(project_root.join("main.rs"))?;
        let result = find_file_by_suffix(project_root, "main.rs")?;
        assert_eq!(
            result,
            Some(expected_path_main_rs),
            "Test 3 failed: main.rs root file"
        );

        // Test 4: No match - non-existent suffix
        let result = find_file_by_suffix(project_root, "nonexistent.ts")?;
        assert_eq!(result, None, "Test 4 failed: nonexistent.ts no match");

        // Test 5: No match - file extension not in `extensions_to_scan`
        let result = find_file_by_suffix(project_root, "image.png")?;
        assert_eq!(
            result, None,
            "Test 5 failed: image.png extension not scanned"
        );

        // Test 6: Ambiguous match - short name "utils.rs"
        let result_ambiguous = find_file_by_suffix(project_root, "utils.rs");
        assert!(
            result_ambiguous.is_err(),
            "Test 6 failed: utils.rs should be ambiguous"
        );
        if let Err(e) = result_ambiguous {
            let error_message = e.to_string();
            assert!(
                error_message.contains("is ambiguous and matches multiple files"),
                "Test 6 failed: Error message should indicate ambiguity"
            );
            let path1_str = dunce::canonicalize(components_dir.join("utils.rs"))?
                .display()
                .to_string();
            let path2_str = dunce::canonicalize(module_dir.join("utils.rs"))?
                .display()
                .to_string();
            assert!(
                error_message.contains(&path1_str),
                "Test 6 failed: Error message should contain path to components/utils.rs"
            );
            assert!(
                error_message.contains(&path2_str),
                "Test 6 failed: Error message should contain path to module/utils.rs"
            );
        }

        // Test 7: Input suffix that is actually a directory name (should not match)
        let result = find_file_by_suffix(project_root, "src")?;
        assert_eq!(
            result, None,
            "Test 7 failed: 'src' directory name should not match"
        );

        let result = find_file_by_suffix(project_root, "components")?;
        assert_eq!(
            result, None,
            "Test 7 failed: 'components' directory name should not match"
        );

        // Test 8: Unique match - absolute input path suffix for a file in project
        let abs_path_main_rs_str = dunce::canonicalize(project_root.join("main.rs"))?
            .to_string_lossy()
            .into_owned();
        let result = find_file_by_suffix(project_root, &abs_path_main_rs_str)?;
        assert_eq!(
            result,
            Some(dunce::canonicalize(project_root.join("main.rs"))?),
            "Test 8 failed: absolute path for main.rs"
        );

        // Test 9: No match - absolute input path suffix for a file outside project
        // Create a file truly outside the project_root for this test.
        let external_tempdir = tempdir().context("Failed to create external tempdir for test 9")?;
        let truly_outside_file_path = external_tempdir.path().join("truly_outside.rs");
        File::create(&truly_outside_file_path)
            .context("Failed to create truly_outside.rs for test 9")?;
        let abs_path_truly_outside_str = dunce::canonicalize(&truly_outside_file_path)
            .context("Failed to canonicalize truly_outside.rs for test 9")?
            .to_string_lossy()
            .into_owned();

        // project_root is still dir.path() from the main test setup
        let result = find_file_by_suffix(project_root, &abs_path_truly_outside_str)?;
        assert_eq!(
            result, None,
            "Test 9 failed: absolute path outside project root should not match"
        );

        Ok(())
    }
} 