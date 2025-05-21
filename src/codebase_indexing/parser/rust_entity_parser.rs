use super::helpers::*;
use super::entities::{CodeContext, CodeEntity};
use crate::codebase_indexing::postprocessor::split_entity;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

// Helper to extract doc comments and the line number where they start
fn get_rust_docstring_and_start_line(node: Node, source_code: &str) -> (Option<String>, usize) {
    let mut potential_docstring: Option<String> = None;
    let mut doc_line_from = node.start_position().row + 1; // Default to node's start
    let mut current_doc_comment_block = String::new();
    let mut s = node;

    while let Some(prev_s) = s.prev_named_sibling() {
        s = prev_s;
        let kind = prev_s.kind();
        if kind == "line_comment" || kind == "block_comment" {
            let comment_text = get_node_text(prev_s, source_code);
            if comment_text.starts_with("///") || comment_text.starts_with("//!") {
                // Outer or Inner doc comment line
                current_doc_comment_block =
                    format!("{}
{}", comment_text.trim(), current_doc_comment_block);
                doc_line_from = prev_s.start_position().row + 1;
            } else if comment_text.starts_with("/**") && !comment_text.starts_with("/***/")
                || comment_text.starts_with("/*!")
            {
                // Block doc comment (outer or inner)
                current_doc_comment_block =
                    format!("{}
{}", comment_text.trim(), current_doc_comment_block);
                doc_line_from = prev_s.start_position().row + 1;
            } else {
                // Not a doc comment, stop if we haven't found any doc comments yet.
                // If we have, this non-doc comment breaks the contiguous block.
                if current_doc_comment_block.is_empty() {
                    continue; // Could be other comments, keep looking up
                } else {
                    break; // Found doc comments, and now a non-doc comment
                }
            }
        } else {
            break; // Not a comment, stop.
        }
    }
    if !current_doc_comment_block.trim().is_empty() {
        potential_docstring = Some(current_doc_comment_block.trim().to_string());
    }
    (potential_docstring, doc_line_from)
}

fn collect_rust_entities_recursive(
    node: Node,
    source_code: &str,
    file_path: &Path,
    current_module_name: &Option<String>,
    current_struct_or_impl_name: &Option<String>,
    entities: &mut Vec<CodeEntity>,
    max_snippet_size: Option<usize>,
) {
    let node_kind = node.kind();
    let mut entity_created_for_this_node = false;

    let (potential_docstring, doc_line_from) = get_rust_docstring_and_start_line(node, source_code);

    let create_and_add_entity = |entity: CodeEntity, entities: &mut Vec<CodeEntity>| {
        if let Some(max_size) = max_snippet_size {
            entities.extend(split_entity(entity, max_size));
        } else {
            entities.push(entity);
        }
    };

    match node_kind {
        "function_item" => {
            let name_node = find_child_node_by_kind(node, "identifier");
            let body_node = find_child_node_by_kind(node, "block"); // For actual code block

            if let Some(name_n) = name_node {
                let name = get_node_text(name_n, source_code);
                let code_type = if current_struct_or_impl_name.is_some() {
                    "Method".to_string()
                } else {
                    "Function".to_string()
                };

                let mut signature = String::new();
                // Try to construct a more complete signature up to the body
                let mut sig_cursor = node.walk();
                for child in node.children(&mut sig_cursor) {
                    if body_node.is_some() && child.id() == body_node.unwrap().id() {
                        break; // Stop before the block
                    }
                    if child.kind() == "attribute_item" {
                        continue; // Skip attributes in signature line
                    }
                    signature.push_str(get_node_text(child, source_code).as_str());
                    signature.push(' ');
                }

                let entity = CodeEntity {
                    name,
                    signature: signature.trim().to_string(),
                    code_type,
                    docstring: potential_docstring,
                    line: name_n.start_position().row + 1, // Line of the identifier
                    line_from: doc_line_from,              // Start of doc comment or item
                    line_to: node.end_position().row + 1,
                    context: CodeContext {
                        module: current_module_name.clone(),
                        file_path: file_path.to_string_lossy().to_string(),
                        file_name: file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        struct_name: current_struct_or_impl_name.clone(),
                        snippet: get_node_text(node, source_code),
                    },
                    embedding: None,
                };
                create_and_add_entity(entity, entities);
                entity_created_for_this_node = true; // Mark as processed
            }
        }
        "struct_item" => {
            let name_node = find_child_node_by_kind(node, "type_identifier");
            if let Some(name_n) = name_node {
                let struct_name = get_node_text(name_n, source_code);
                let entity = CodeEntity {
                    name: struct_name.clone(),
                    signature: get_node_text(node, source_code), // Full struct definition
                    code_type: "Struct".to_string(),
                    docstring: potential_docstring,
                    line: name_n.start_position().row + 1,
                    line_from: doc_line_from,
                    line_to: node.end_position().row + 1,
                    context: CodeContext {
                        module: current_module_name.clone(),
                        file_path: file_path.to_string_lossy().to_string(),
                        file_name: file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        struct_name: None, // Struct itself doesn't have a parent struct_name
                        snippet: get_node_text(node, source_code),
                    },
                    embedding: None,
                };
                create_and_add_entity(entity, entities);
                entity_created_for_this_node = true;
                // Recursively process contents of the struct (e.g., field_declaration_list) if needed
                // For now, we treat the whole struct as one entity. Methods are handled via 'impl'.
            }
        }
        "impl_item" => {
            let type_node = find_child_node_by_kind(node, "type_identifier") // For `impl MyType`
                .or_else(|| find_child_node_by_kind(node, "generic_type")) // For `impl<T> MyType<T>`
                .or_else(|| find_child_node_by_kind(node, "trait_bound")); // For `impl Trait for MyType`

            let impl_name = type_node.map_or_else(
                || "anonymous_impl".to_string(),
                |n| get_node_text(n, source_code),
            );
            let new_impl_name = Some(impl_name.clone());

            // Create an entity for the impl block itself
            let entity = CodeEntity {
                name: format!("impl {}", impl_name),
                signature: get_node_text(node, source_code), // Full impl block
                code_type: "Impl".to_string(),
                docstring: potential_docstring,
                line: node.start_position().row + 1, // Line of the impl keyword
                line_from: doc_line_from,
                line_to: node.end_position().row + 1,
                context: CodeContext {
                    module: current_module_name.clone(),
                    file_path: file_path.to_string_lossy().to_string(),
                    file_name: file_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    struct_name: None, // Impl block itself, methods inside will reference it
                    snippet: get_node_text(node, source_code),
                },
                embedding: None,
            };
            create_and_add_entity(entity, entities);
            entity_created_for_this_node = true; // Mark impl block as processed

            // Process items within the impl block (e.g., function_item for methods)
            if let Some(body_node) = find_child_node_by_kind(node, "declaration_list")
                .or_else(|| find_child_node_by_kind(node, "field_declaration_list"))
            {
                let mut cursor = body_node.walk();
                for child_node in body_node.named_children(&mut cursor) {
                    collect_rust_entities_recursive(
                        child_node,
                        source_code,
                        file_path,
                        current_module_name,
                        &new_impl_name, // Pass the name of the struct/trait being implemented
                        entities,
                        max_snippet_size,
                    );
                }
            }
        }
        "trait_item" => {
            let name_node = find_child_node_by_kind(node, "type_identifier");
            if let Some(name_n) = name_node {
                let trait_name = get_node_text(name_n, source_code);
                let entity = CodeEntity {
                    name: trait_name.clone(),
                    signature: get_node_text(node, source_code), // Full trait definition
                    code_type: "Trait".to_string(),
                    docstring: potential_docstring,
                    line: name_n.start_position().row + 1,
                    line_from: doc_line_from,
                    line_to: node.end_position().row + 1,
                    context: CodeContext {
                        module: current_module_name.clone(),
                        file_path: file_path.to_string_lossy().to_string(),
                        file_name: file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        struct_name: None,
                        snippet: get_node_text(node, source_code),
                    },
                    embedding: None,
                };
                create_and_add_entity(entity, entities);
                entity_created_for_this_node = true;
                // Process items within the trait (e.g., associated types, methods)
            }
        }
        "mod_item" => {
            let name_node = find_child_node_by_kind(node, "identifier");
            if let Some(name_n) = name_node {
                let mod_name = get_node_text(name_n, source_code);
                let new_module_name = current_module_name
                    .as_ref()
                    .map_or_else(|| mod_name.clone(), |m| format!("{}::{}", m, mod_name));

                let entity = CodeEntity {
                    name: mod_name.clone(),
                    signature: format!("mod {};", mod_name), // Simplified signature
                    code_type: "Module".to_string(),
                    docstring: potential_docstring,
                    line: name_n.start_position().row + 1,
                    line_from: doc_line_from,
                    line_to: node.end_position().row + 1,
                    context: CodeContext {
                        module: current_module_name.clone(), // Parent module
                        file_path: file_path.to_string_lossy().to_string(),
                        file_name: file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        struct_name: None,
                        snippet: get_node_text(node, source_code),
                    },
                    embedding: None,
                };
                create_and_add_entity(entity, entities);
                entity_created_for_this_node = true;

                // Process contents of the module
                if let Some(body_node) = find_child_node_by_kind(node, "declaration_list") {
                    let mut cursor = body_node.walk();
                    for child_node in body_node.named_children(&mut cursor) {
                        collect_rust_entities_recursive(
                            child_node,
                            source_code,
                            file_path,
                            &Some(new_module_name.clone()), // Pass down the new module path
                            current_struct_or_impl_name,    // Inherit struct context if any
                            entities,
                            max_snippet_size,
                        );
                    }
                }
            }
        }
        "use_declaration" => {
            let entity = CodeEntity {
                name: get_node_text(node, source_code), // The full use statement
                signature: get_node_text(node, source_code),
                code_type: "Import".to_string(), // Treat 'use' as Import
                docstring: potential_docstring,
                line: node.start_position().row + 1,
                line_from: doc_line_from,
                line_to: node.end_position().row + 1,
                context: CodeContext {
                    module: current_module_name.clone(),
                    file_path: file_path.to_string_lossy().to_string(),
                    file_name: file_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    struct_name: None,
                    snippet: get_node_text(node, source_code),
                },
                embedding: None,
            };
            create_and_add_entity(entity, entities);
            entity_created_for_this_node = true;
        }
        // Add cases for const_item, static_item, enum_item, type_item etc.
        "const_item" | "static_item" => {
            let name_node = find_child_node_by_kind(node, "identifier");
            if let Some(name_n) = name_node {
                let name = get_node_text(name_n, source_code);
                let entity = CodeEntity {
                    name,
                    signature: get_node_text(node, source_code),
                    code_type: if node_kind == "const_item" {
                        "Constant".to_string()
                    } else {
                        "Static Variable".to_string()
                    },
                    docstring: potential_docstring,
                    line: name_n.start_position().row + 1,
                    line_from: doc_line_from,
                    line_to: node.end_position().row + 1,
                    context: CodeContext {
                        module: current_module_name.clone(),
                        file_path: file_path.to_string_lossy().to_string(),
                        file_name: file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        struct_name: current_struct_or_impl_name.clone(),
                        snippet: get_node_text(node, source_code),
                    },
                    embedding: None,
                };
                create_and_add_entity(entity, entities);
                entity_created_for_this_node = true;
            }
        }
        _ => {}
    }

    // If this node itself wasn't turned into an entity (e.g., it's a 'source_file' or 'block'),
    // recurse on its children.
    if !entity_created_for_this_node {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_rust_entities_recursive(
                child,
                source_code,
                file_path,
                current_module_name,
                current_struct_or_impl_name,
                entities,
                max_snippet_size,
            );
        }
    }
}

pub fn extract_rust_entities_from_file(
    file_path: &PathBuf,
    max_snippet_size: Option<usize>,
) -> Result<Vec<CodeEntity>> {
    let source_code = fs::read_to_string(file_path)?;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language().into())
        .map_err(|e| anyhow::anyhow!("Error loading Rust grammar: {}", e))?;
    let tree = parser
        .parse(&source_code, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse Rust code"))?;

    let mut entities = Vec::new();
    let root_node = tree.root_node();
    let initial_module_name = file_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());

    collect_rust_entities_recursive(
        root_node,
        &source_code,
        file_path,
        &initial_module_name, // Top-level items are in a module named after the file
        &None,                // No struct/impl context initially
        &mut entities,
        max_snippet_size,
    );
    Ok(entities)
} 