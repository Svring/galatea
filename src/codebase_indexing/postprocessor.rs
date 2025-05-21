use crate::codebase_indexing::parser::entities::{CodeContext, CodeEntity};
use anyhow::Result;
use clap::ValueEnum;
use std::cmp::min;
use std::str::FromStr;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Granularity {
    /// Fine-grained: Extract most specific entities, merge only specific consecutive types (imports, consts).
    Fine,
    /// Medium: Merge consecutive entities towards ~half max_snippet_size.
    Medium,
    /// Coarse: Merge consecutive entities towards max_snippet_size.
    Coarse,
}

// Implement FromStr for parsing from string (used by clap)
impl FromStr for Granularity {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fine" => Ok(Granularity::Fine),
            "medium" => Ok(Granularity::Medium),
            "coarse" => Ok(Granularity::Coarse),
            _ => Err(anyhow::anyhow!(
                "Invalid granularity level: {}. Use fine, medium, or coarse.",
                s
            )),
        }
    }
}

// Default granularity
impl Default for Granularity {
    fn default() -> Self {
        Granularity::Fine
    }
}

// Split entity function (Moved from helpers)
pub fn split_entity(entity: CodeEntity, max_size: usize) -> Vec<CodeEntity> {
    if entity.context.snippet.len() <= max_size {
        return vec![entity];
    }
    let mut chunks = Vec::new();
    let lines: Vec<&str> = entity.context.snippet.lines().collect();
    let mut current_chunk_lines = Vec::new();
    let mut current_chunk_size = 0;
    let mut start_line_offset = 0;
    for (i, line) in lines.iter().enumerate() {
        let line_len = line.len() + 1;
        if current_chunk_size + line_len > max_size && !current_chunk_lines.is_empty() {
            let joined_chunk = current_chunk_lines
                .iter()
                .copied()
                .collect::<Vec<&str>>()
                .join("\n");
            chunks.push((start_line_offset, i - 1, joined_chunk));
            current_chunk_lines = vec![line];
            current_chunk_size = line_len;
            start_line_offset = i;
        } else {
            current_chunk_lines.push(line);
            current_chunk_size += line_len;
        }
    }
    if !current_chunk_lines.is_empty() {
        let joined_chunk = current_chunk_lines
            .iter()
            .copied()
            .collect::<Vec<&str>>()
            .join("\n");
        chunks.push((start_line_offset, lines.len() - 1, joined_chunk));
    }
    let total_chunks = chunks.len();
    let mut split_entities = Vec::new();
    for (i, (start_offset, end_offset, chunk_snippet)) in chunks.into_iter().enumerate() {
        let mut new_entity = entity.clone();
        new_entity.name = format!("{} [chunk {}/{}]", entity.name, i + 1, total_chunks);
        new_entity.context.snippet = chunk_snippet;
        new_entity.line_from = min(entity.line_to, entity.line_from + start_offset);
        new_entity.line_to = min(entity.line_to, entity.line_from + end_offset);
        new_entity.signature = format!(
            "Chunk {}/{} of original {}",
            i + 1,
            total_chunks,
            entity.code_type
        );
        split_entities.push(new_entity);
    }
    split_entities
}

// Placeholder for the main post-processing function
pub fn post_process_entities(
    entities: Vec<CodeEntity>,
    granularity: Granularity,
    max_snippet_size: Option<usize>,
) -> Vec<CodeEntity> {
    println!(
        "Post-processing with granularity: {:?}, max_size: {:?}",
        granularity, max_snippet_size
    );
    // TODO: Implement merging logic based on granularity
    match granularity {
        Granularity::Fine => {
            // Implement Fine merging (specific types only)
            // Example: merge_fine(entities, max_snippet_size)
            merge_fine_grained(entities, max_snippet_size)
        }
        Granularity::Medium => {
            // Implement Medium merging (any type, target size = max/2)
            let target_size = max_snippet_size.map(|s| s / 2);
            merge_aggressively(entities, target_size) // Target size for medium
        }
        Granularity::Coarse => {
            // Implement Coarse merging (any type, target size = max)
            merge_aggressively(entities, max_snippet_size) // Target size for coarse
        }
    }
}

// --- Merging Implementations ---

// Fine-grained merging (specific types like Import, Constant, Variable)
fn merge_fine_grained(
    mut entities: Vec<CodeEntity>,
    max_snippet_size: Option<usize>,
) -> Vec<CodeEntity> {
    if entities.len() < 2 {
        return entities;
    }
    entities.sort_by_key(|e| e.line_from);
    let mut merged_entities = Vec::new();
    let mut iter = entities.into_iter().peekable();

    while let Some(current_entity) = iter.next() {
        let current_type = current_entity.code_type.clone();
        // Only merge specific types
        let can_merge_type = matches!(current_type.as_str(), "Import" | "Constant" | "Variable");

        if can_merge_type {
            let mut merge_candidates = vec![current_entity];
            let mut combined_snippet_len = merge_candidates[0].context.snippet.len();

            while let Some(next_entity) = iter.peek() {
                if next_entity.code_type == current_type {
                    let next_snippet_len = next_entity.context.snippet.len();
                    let potential_new_len = combined_snippet_len + next_snippet_len + 1;
                    if max_snippet_size.map_or(true, |max_size| potential_new_len <= max_size) {
                        combined_snippet_len = potential_new_len;
                        merge_candidates.push(iter.next().unwrap());
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            if merge_candidates.len() > 1 {
                merged_entities.push(create_merged_entity(merge_candidates));
            } else {
                merged_entities.push(merge_candidates.remove(0));
            }
        } else {
            merged_entities.push(current_entity);
        }
    }
    merged_entities
}

// Aggressive merging (any consecutive entities up to target size)
fn merge_aggressively(
    mut entities: Vec<CodeEntity>,
    target_snippet_size: Option<usize>,
) -> Vec<CodeEntity> {
    if entities.len() < 2 {
        return entities;
    }
    entities.sort_by_key(|e| e.line_from);
    let mut merged_entities = Vec::new();
    let mut current_merged_candidates = Vec::new();

    for entity in entities {
        let current_merged_len = current_merged_candidates
            .iter()
            .map(|e: &CodeEntity| e.context.snippet.len() + 1)
            .sum::<usize>()
            .saturating_sub(1);
        let next_len = entity.context.snippet.len();
        let potential_new_len = current_merged_len
            + next_len
            + if current_merged_candidates.is_empty() {
                0
            } else {
                1
            };

        // If adding the next entity exceeds the target size (and we already have something), finalize the current merged block
        if !current_merged_candidates.is_empty()
            && target_snippet_size.map_or(false, |target| potential_new_len > target)
        {
            merged_entities.push(create_merged_entity(current_merged_candidates));
            current_merged_candidates = vec![entity]; // Start new block with current entity
        } else {
            // Otherwise, add the current entity to the candidates for merging
            current_merged_candidates.push(entity);
        }
    }

    // Add the last pending merged block
    if !current_merged_candidates.is_empty() {
        merged_entities.push(create_merged_entity(current_merged_candidates));
    }

    merged_entities
}

// Helper to create a single CodeEntity from a list of candidates
fn create_merged_entity(merge_candidates: Vec<CodeEntity>) -> CodeEntity {
    if merge_candidates.len() == 1 {
        return merge_candidates.into_iter().next().unwrap();
    }
    let first = merge_candidates.first().unwrap();
    let last = merge_candidates.last().unwrap();
    let types = merge_candidates
        .iter()
        .map(|e| e.code_type.as_str())
        .collect::<Vec<&str>>();

    // Determine merged type - if all same, keep it, otherwise generic
    let merged_code_type = if types.windows(2).all(|w| w[0] == w[1]) {
        first.code_type.clone()
    } else {
        "Merged Chunk".to_string() // Or be more specific like "Mixed Chunk"
    };

    let merged_name = format!(
        "Merged {} [lines {}-{}]",
        merged_code_type, first.line_from, last.line_to
    );
    let merged_snippet = merge_candidates
        .iter()
        .map(|e| e.context.snippet.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    let merged_signature = merge_candidates
        .iter()
        .map(|e| e.signature.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    let merged_docstring = merge_candidates.iter().find_map(|e| e.docstring.clone());

    CodeEntity {
        name: merged_name,
        signature: merged_signature,
        code_type: merged_code_type,
        docstring: merged_docstring,
        line: first.line,
        line_from: first.line_from,
        line_to: last.line_to,
        context: CodeContext {
            module: first.context.module.clone(),
            file_path: first.context.file_path.clone(),
            file_name: first.context.file_name.clone(),
            struct_name: None,
            snippet: merged_snippet,
        },
        embedding: None,
    }
} 