use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Recursively finds files within a directory that match the given suffixes,
/// excluding specified directory names.
///
/// # Arguments
///
/// * `start_path` - The directory path to start searching from.
/// * `suffixes` - A slice of file suffixes to match (e.g., &["rs", "ts", "tsx"]).
/// * `exclude_dirs` - A slice of directory names to exclude (e.g., &["node_modules", "target"]).
///
/// # Returns
///
/// A `Result` containing a vector of `PathBuf`s for matching files, or an error.
pub fn find_files_by_suffix(
    start_path: &Path,
    suffixes: &[&str],
    exclude_dirs: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut matching_files = Vec::new();
    find_files_recursive(start_path, suffixes, exclude_dirs, &mut matching_files)
        .with_context(|| format!("Failed to scan directory: {}", start_path.display()))?;
    Ok(matching_files)
}

fn find_files_recursive(
    current_path: &Path,
    suffixes: &[&str],
    exclude_dirs: &[&str],
    matching_files: &mut Vec<PathBuf>,
) -> Result<()> {
    if current_path.is_dir() {
        if let Some(dir_name) = current_path.file_name().and_then(|n| n.to_str()) {
            if exclude_dirs.contains(&dir_name) {
                return Ok(());
            }
        }

        for entry_result in fs::read_dir(current_path)? {
            let entry = entry_result?;
            let path = entry.path();
            if path.is_dir() {
                find_files_recursive(&path, suffixes, exclude_dirs, matching_files)?;
            } else if path.is_file() {
                if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
                    if suffixes.contains(&extension) {
                        matching_files.push(path);
                    }
                }
            }
        }
    }
    Ok(())
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

        let suffixes = ["rs", "ts", "tsx"];
        let exclude = ["node_modules", "target"];
        let found_files = find_files_by_suffix(root, &suffixes, &exclude)?;

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
        let all_found = find_files_by_suffix(root, &suffixes, &no_exclude)?;
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

        let suffixes = ["rs", "tsx"];
        let exclude: [&str; 0] = [];
        let found_files = find_files_by_suffix(root, &suffixes, &exclude)?;

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

        let ts_files = find_files_by_suffix(root, &["ts"], &exclude)?;
        assert_eq!(ts_files.len(), 1);
        assert_eq!(ts_files[0], sub.join("file3.ts"));

        let no_files = find_files_by_suffix(root, &["java", "py"], &exclude)?;
        assert!(no_files.is_empty());

        Ok(())
    }
}
