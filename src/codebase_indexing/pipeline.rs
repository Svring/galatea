use crate::codebase_indexing::parser; // Using fully qualified path
use crate::codebase_indexing::postprocessor; // Import processing module
use crate::file_system::search;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Finds files by suffix, parses them, and saves the combined entities to a JSON file.
///
/// # Arguments
///
/// * `start_path` - The directory path to start searching from.
/// * `suffixes` - A slice of file suffixes to match (e.g., &["rs", "ts", "tsx"]).
/// * `output_file` - The path where the resulting JSON should be saved.
/// * `max_snippet_size` - Optional maximum size for snippets (triggers splitting).
/// * `exclude_dirs` - A slice of directory names to exclude.
/// * `granularity` - The granularity for post-processing.
///
/// # Returns
///
/// `Ok(())` on success, or an error.
pub fn index_directory(
    start_path: &Path,
    extensions: &[&str],
    output_file: &Path,
    max_snippet_size: Option<usize>,
    exclude_dirs: &[&str],
    granularity: postprocessor::Granularity, // Add granularity parameter
) -> Result<()> {
    println!(
        "Starting indexing in '{}' for extensions: {:?} (excluding: {:?}, granularity: {:?})",
        start_path.display(),
        extensions,
        exclude_dirs,
        granularity // Log granularity
    );

    // 1. Find files, passing exclude_dirs
    let files_to_parse = search::find_files_by_extensions(start_path, extensions, exclude_dirs)
        .with_context(|| format!("Failed scanning directory '{}'", start_path.display()))?;

    if files_to_parse.is_empty() {
        println!("No matching files found to index.");
        return Ok(());
    }
    println!("Found {} files to process.", files_to_parse.len());

    let mut all_entities: Vec<parser::entities::CodeEntity> = Vec::new();

    // 2. Parse each file based on its extension
    for file_path in files_to_parse {
        println!("  Parsing: {}", file_path.display());
        let extension = file_path.extension().and_then(|ext| ext.to_str());

        let parse_result = match extension {
            Some("rs") => {
                // Call the function re-exported from parser_mod
                parser::extract_rust_entities_from_file(&file_path, max_snippet_size)
            }
            Some("ts") => {
                // Call the function re-exported (and renamed) from parser_mod
                parser::extract_ts_entities(&file_path, false, max_snippet_size)
            }
            Some("tsx") => {
                // Call the function re-exported (and renamed) from parser_mod
                parser::extract_ts_entities(&file_path, true, max_snippet_size)
            }
            _ => {
                println!("  -> Skipping file with unsupported extension.");
                continue; // Skip this file
            }
        };

        match parse_result {
            Ok(entities) => {
                println!("    -> Extracted {} entities.", entities.len());
                all_entities.extend(entities);
            }
            Err(e) => {
                // Log error and continue with the next file
                eprintln!(
                    "    -> Error parsing {}: {}. Skipping file.",
                    file_path.display(),
                    e
                );
            }
        }
    }

    println!(
        "Total entities extracted before post-processing: {}",
        all_entities.len()
    );

    // 3. Post-process based on granularity (splitting is handled during parsing)
    let final_entities =
        postprocessor::post_process_entities(all_entities, granularity, max_snippet_size);

    println!(
        "Total entities after post-processing: {}",
        final_entities.len()
    );

    if final_entities.is_empty() {
        println!("No entities remain after post-processing. Output file will not be created.");
        return Ok(());
    }

    // 4. Serialize final results to JSON
    let json_output = serde_json::to_string_pretty(&final_entities)
        .context("Failed to serialize final entities to JSON")?;

    // 5. Save JSON to output file
    println!("Saving index to: {}", output_file.display());
    let mut file = fs::File::create(output_file)
        .with_context(|| format!("Failed to create output file: {}", output_file.display()))?;
    file.write_all(json_output.as_bytes())
        .with_context(|| format!("Failed to write to output file: {}", output_file.display()))?;

    println!("Indexing complete.");
    Ok(())
} 