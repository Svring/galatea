use super::helpers::*;
use super::structs::{CodeContext, CodeEntity};
use crate::processing::split_entity;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

// Main recursive function to collect entities
fn collect_entities_recursive<'a>(
    node: Node<'a>,
    source_code: &str,
    file_path: &Path,
    current_module_name: &Option<String>,
    current_struct_name: &Option<String>,
    entities: &mut Vec<CodeEntity>,
    max_snippet_size: Option<usize>,
) {
    let node_kind = node.kind();
    let mut entity_created_for_this_node = false;

    let mut potential_docstring: Option<String> = None;
    let mut doc_line_from = node.start_position().row + 1;
    let mut current_doc_comment_block = String::new();
    let mut s = node;
    while let Some(prev_s) = s.prev_named_sibling() {
        s = prev_s;
        if prev_s.kind() == "line_comment" || prev_s.kind() == "block_comment" {
            let comment_text = get_node_text(prev_s, source_code);
            if comment_text.starts_with("///")
                || comment_text.starts_with("//!")
                || comment_text.starts_with("/**")
                || comment_text.starts_with("/*!")
            {
                current_doc_comment_block =
                    format!("{}\n{}", comment_text.trim(), current_doc_comment_block);
                doc_line_from = prev_s.start_position().row + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if !current_doc_comment_block.trim().is_empty() {
        potential_docstring = Some(current_doc_comment_block.trim().to_string());
    }

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
            if let Some(name_n) = name_node {
                let name = get_node_text(name_n, source_code);
                if name_n.prev_sibling().map_or(true, |s| s.kind() != "<") {
                    let entity = CodeEntity {
                        name,
                        signature: get_node_text(node, source_code),
                        code_type: "Function".to_string(),
                        docstring: potential_docstring.clone(),
                        line: node.start_position().row + 1,
                        line_from: potential_docstring
                            .as_ref()
                            .map_or(node.start_position().row + 1, |_| doc_line_from),
                        line_to: node.end_position().row + 1,
                        context: CodeContext {
                            module: current_module_name.clone(),
                            file_path: file_path.to_string_lossy().to_string(),
                            file_name: file_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                            struct_name: current_struct_name.clone(),
                            snippet: get_node_text(node, source_code),
                        },
                        embedding: None,
                    };
                    create_and_add_entity(entity, entities);
                    entity_created_for_this_node = true;
                } else {
                    // println!("DEBUG Rust: function_item identifier '{}' skipped (might be generic?)", name);
                }
            } else {
                // println!("DEBUG Rust: function_item SKIPPED (no identifier node found)");
            }
        }
        "struct_item" => {
            let name_node = find_child_node_by_kind(node, "type_identifier")
                .or_else(|| find_child_node_by_kind(node, "identifier"));
            if let Some(name_n) = name_node {
                let struct_name_str = get_node_text(name_n, source_code);
                let entity = CodeEntity {
                    name: struct_name_str.clone(),
                    signature: get_node_text(node, source_code),
                    code_type: "Struct".to_string(),
                    docstring: potential_docstring.clone(),
                    line: node.start_position().row + 1,
                    line_from: potential_docstring
                        .as_ref()
                        .map_or(node.start_position().row + 1, |_| doc_line_from),
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
            } else {
                // println!("DEBUG Rust: struct_item SKIPPED (no name node found)");
            }
        }
        "impl_item" => {
            let type_node = find_child_node_by_kind(node, "type_identifier")
                .or_else(|| find_child_node_by_kind(node, "generic_type"))
                .or_else(|| find_child_node_by_kind(node, "scoped_type_identifier"))
                .or_else(|| find_child_node_by_kind(node, "identifier"));

            let impl_for_struct_name = type_node.map(|tn| get_node_text(tn, source_code));
            entity_created_for_this_node = true;

            let body_node_candidates = [
                find_child_node_by_field_name(node, "body"),
                find_child_node_by_kind(node, "declaration_list"),
                find_child_node_by_kind(node, "assoc_item_list"),
            ];
            if let Some(body_node) = body_node_candidates.into_iter().flatten().next() {
                let mut cursor = body_node.walk();
                for child_of_body in body_node.named_children(&mut cursor) {
                    if child_of_body.kind() == "function_item" {
                        collect_entities_recursive(
                            child_of_body,
                            source_code,
                            file_path,
                            current_module_name,
                            &impl_for_struct_name,
                            entities,
                            max_snippet_size,
                        );
                    } else if child_of_body.kind() == "associated_item" {
                        if let Some(func_node) =
                            find_child_node_by_kind(child_of_body, "function_item")
                        {
                            collect_entities_recursive(
                                func_node,
                                source_code,
                                file_path,
                                current_module_name,
                                &impl_for_struct_name,
                                entities,
                                max_snippet_size,
                            );
                        }
                    }
                }
            }
        }
        _ => {}
    }

    if !entity_created_for_this_node
        || node_kind == "source_file"
        || node_kind == "mod_item"
        || node_kind == "block"
    {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let csn = if node_kind == "impl_item" {
                &None
            } else {
                current_struct_name
            };
            collect_entities_recursive(
                child,
                source_code,
                file_path,
                current_module_name,
                csn,
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
    let language = tree_sitter_rust::language();
    parser
        .set_language(&language)
        .expect("Error loading Rust grammar");
    let tree = parser
        .parse(&source_code, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse Rust code"))?;

    let mut entities = Vec::new();
    let root_node = tree.root_node();
    let initial_module_name = file_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());

    collect_entities_recursive(
        root_node,
        &source_code,
        file_path,
        &initial_module_name,
        &None,
        &mut entities,
        max_snippet_size,
    );

    Ok(entities)
}

#[cfg(test)]
mod rust_entity_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_simple_function() -> Result<()> {
        let code = r#"
/// This is a doc comment for a function.
/// It has multiple lines.
fn my_function(a: i32) -> i32 {
    a + 1
}
"#;
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(code.as_bytes())?;
        let file_path = temp_file.path().to_path_buf();

        let entities = extract_rust_entities_from_file(&file_path, None)?;

        assert_eq!(entities.len(), 1);
        let func_entity = &entities[0];
        assert_eq!(func_entity.name, "my_function");
        assert_eq!(func_entity.code_type, "Function");
        assert!(func_entity
            .signature
            .contains("fn my_function(a: i32) -> i32"));
        assert_eq!(
            func_entity.docstring,
            Some(
                "/// This is a doc comment for a function.\n/// It has multiple lines.".to_string()
            )
        );
        assert_eq!(func_entity.line_from, 2);
        assert_eq!(func_entity.line, 4);
        assert_eq!(func_entity.line_to, 6);
        assert_eq!(
            func_entity.context.file_name,
            temp_file.path().file_name().unwrap().to_str().unwrap()
        );
        Ok(())
    }

    #[test]
    fn test_extract_struct_and_method() -> Result<()> {
        let code = r#"
/// Doc for MyStruct.
struct MyStruct {
    field: i32,
}

impl MyStruct {
    /// Doc for new method.
    fn new() -> Self {
        Self { field: 0 }
    }
}
"#;
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(code.as_bytes())?;
        let file_path = temp_file.path().to_path_buf();

        let entities = extract_rust_entities_from_file(&file_path, None)?;

        assert_eq!(entities.len(), 2, "Expected 2 entities (Struct, Method)");

        let struct_entity = entities
            .iter()
            .find(|e| e.name == "MyStruct" && e.code_type == "Struct")
            .expect("MyStruct not found");
        assert_eq!(
            struct_entity.docstring,
            Some("/// Doc for MyStruct.".to_string())
        );

        let method_entity = entities
            .iter()
            .find(|e| e.name == "new" && e.code_type == "Function")
            .expect("Method 'new' not found");
        assert_eq!(
            method_entity.docstring,
            Some("/// Doc for new method.".to_string())
        );
        assert_eq!(
            method_entity.context.struct_name,
            Some("MyStruct".to_string())
        );

        Ok(())
    }
}
