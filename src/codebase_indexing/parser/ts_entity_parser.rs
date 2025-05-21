use super::helpers::*;
use super::entities::{CodeContext, CodeEntity};
use crate::codebase_indexing::postprocessor::split_entity;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

fn get_ts_docstring_and_start_line(node: Node, source_code: &str) -> (Option<String>, usize) {
    let mut potential_docstring: Option<String> = None;
    let mut doc_line_from = node.start_position().row + 1;
    let mut current_doc_comment_block = String::new();
    let mut s = node;

    // JSDoc comments are typically /** ... */ or line comments ///
    while let Some(prev_s) = s.prev_named_sibling() {
        s = prev_s;
        let kind = prev_s.kind();
        if kind == "comment" {
            // tree-sitter-typescript often uses a generic "comment" kind
            let comment_text = get_node_text(prev_s, source_code);
            // Check for JSDoc block or significant line comments
            if comment_text.starts_with("/**")
                || comment_text.starts_with("//!")
                || comment_text.starts_with("///")
            {
                current_doc_comment_block =
                    format!("{}\n{}", comment_text.trim(), current_doc_comment_block);
                doc_line_from = prev_s.start_position().row + 1;
            } else if comment_text.starts_with("//") && current_doc_comment_block.is_empty() {
                // Allow a block of single line comments if no JSDoc block found yet
                current_doc_comment_block =
                    format!("{}\n{}", comment_text.trim(), current_doc_comment_block);
                doc_line_from = prev_s.start_position().row + 1;
            } else if !comment_text.starts_with("//") && !comment_text.starts_with("/*") {
                break; // Not a comment that seems like a docstring leader
            } else if current_doc_comment_block.is_empty()
                && comment_text.starts_with("/*")
                && !comment_text.starts_with("/**")
            {
                break; // A regular block comment, not JSDoc, and no prior doc comments found.
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

// Helper to check for JSX presence
fn contains_jsx(node: Node) -> bool {
    if node.kind() == "jsx_element" || node.kind() == "jsx_self_closing_element" {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if contains_jsx(child) {
            return true;
        }
    }
    false
}

fn collect_ts_entities_recursive(
    node: Node,
    source_code: &str,
    file_path: &Path,
    current_module_name: &Option<String>,
    current_class_name: &Option<String>,
    entities: &mut Vec<CodeEntity>,
    inherited_docstring_info: Option<(Option<String>, usize)>,
    max_snippet_size: Option<usize>,
) {
    let node_kind = node.kind();
    let mut entity_created_for_this_node = false;

    let (potential_docstring, doc_line_from) = inherited_docstring_info
        .unwrap_or_else(|| get_ts_docstring_and_start_line(node, source_code));

    let create_and_add_entity = |entity: CodeEntity, entities: &mut Vec<CodeEntity>| {
        if let Some(max_size) = max_snippet_size {
            entities.extend(split_entity(entity, max_size));
        } else {
            entities.push(entity);
        }
    };

    if node_kind == "export_statement" {
        let declaration_node = node
            .child_by_field_name("declaration")
            .or_else(|| node.named_child(0))
            .or_else(|| node.child(0));
        let export_doc_info = get_ts_docstring_and_start_line(node, source_code);
        if let Some(decl_node) = declaration_node {
            collect_ts_entities_recursive(
                decl_node,
                source_code,
                file_path,
                current_module_name,
                current_class_name,
                entities,
                Some(export_doc_info),
                max_snippet_size,
            );
        }
        entity_created_for_this_node = true;
    }

    match node_kind {
        "import_statement" => {
            let mut _source_str = "?".to_string();
            let mut import_clause_str = "?".to_string();
            if let Some(source_node) = find_child_node_by_field_name(node, "source") {
                _source_str = get_node_text(source_node, source_code);
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "import_clause" | "namespace_import" | "named_imports" | "identifier" => {
                        import_clause_str = get_node_text(child, source_code);
                        break;
                    }
                    _ => {}
                }
            }
            let entity = CodeEntity {
                name: import_clause_str,
                signature: get_node_text(node, source_code),
                code_type: "Import".to_string(),
                docstring: potential_docstring.clone(),
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
        "function_declaration" | "method_definition" => {
            let name_node = if node_kind == "method_definition" {
                find_child_node_by_field_name(node, "name")
                    .or_else(|| find_child_node_by_kind(node, "property_identifier"))
                    .or_else(|| find_child_node_by_kind(node, "constructor"))
            } else {
                // function_declaration
                find_child_node_by_kind(node, "identifier")
                    .or_else(|| find_child_node_by_field_name(node, "name"))
            };

            if let Some(name_n) = name_node {
                let name = get_node_text(name_n, source_code);
                if name.contains(' ') || name.contains('(') || name.contains(':') || name.is_empty()
                {
                    // println!("DEBUG TS: Function/Method name '{}' seems incorrect, skipping.", name);
                } else {
                    let mut code_type = if node_kind == "method_definition"
                        || (node_kind == "function_declaration" && current_class_name.is_some())
                    {
                        "Method".to_string()
                    } else {
                        "Function".to_string()
                    };

                    // Check for JSX to determine if it's a Component
                    // More robust body finding: check field "body", then kind "statement_block"
                    let body_node = find_child_node_by_field_name(node, "body")
                        .or_else(|| find_child_node_by_kind(node, "statement_block"));

                    // println!("DEBUG TS Func: name={}, body_node_found={}, body_kind={:?}", name, body_node.is_some(), body_node.map(|n| n.kind()));

                    if code_type == "Function" {
                        if let Some(body) = body_node {
                            if contains_jsx(body) {
                                // println!("DEBUG TS Func: contains_jsx returned true for {}", name);
                                code_type = "Function Component".to_string();
                            } else {
                                // println!("DEBUG TS Func: contains_jsx returned false for {}", name);
                            }
                        } else {
                            // println!("DEBUG TS Func: No body node found for {}", name);
                        }
                    }

                    // println!("DEBUG TS: Adding {} entity: {}", code_type, name);
                    let entity = CodeEntity {
                        name,
                        signature: get_node_text(node, source_code),
                        code_type, // Use updated code_type
                        docstring: potential_docstring.clone(),
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
                            struct_name: current_class_name.clone(),
                            snippet: get_node_text(node, source_code),
                        },
                        embedding: None,
                    };
                    create_and_add_entity(entity, entities);
                    entity_created_for_this_node = true;
                }
            } else {
                // println!("DEBUG TS: Function/Method skipped - no name_node found ...");
            }
        }
        "lexical_declaration" => {
            // println!("DEBUG TS: Handling lexical_declaration: `{}`", get_node_text(node, source_code).lines().next().unwrap_or(""));
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "variable_declarator" {
                    let var_declarator = child;
                    // println!("DEBUG TS: Found variable_declarator: `{}`", get_node_text(var_declarator, source_code).lines().next().unwrap_or(""));
                    let name_node = find_child_node_by_field_name(var_declarator, "name")
                        .or_else(|| find_child_node_by_kind(var_declarator, "identifier"));
                    let value_node_opt = find_child_node_by_field_name(var_declarator, "value") // Keep using opt suffix
                                        .or_else(|| find_child_node_by_kind(var_declarator, "arrow_function"))
                                        .or_else(|| find_child_node_by_kind(var_declarator, "function_expression"));

                    if let Some(name_n) = name_node {
                        let name = get_node_text(name_n, source_code);
                        let mut processed = false;

                        if let Some(val_n) = value_node_opt {
                            // Check opt here
                            println!(
                                "DEBUG TS: VarDeclarator check: name=`{}`, value_kind=`{}`",
                                name,
                                val_n.kind()
                            ); // UNCOMMENTED
                            if val_n.kind() == "arrow_function"
                                || val_n.kind() == "function_expression"
                            {
                                let mut code_type = "Function".to_string();
                                if contains_jsx(val_n) {
                                    code_type = "Function Component".to_string();
                                }
                                println!(
                                    "DEBUG TS: >>> ADDING Function/Component entity: {}",
                                    name
                                ); // UNCOMMENTED
                                let entity = CodeEntity {
                                    name: name.clone(),
                                    signature: get_node_text(var_declarator, source_code),
                                    code_type,
                                    docstring: potential_docstring.clone(),
                                    line: name_n.start_position().row + 1,
                                    line_from: doc_line_from,
                                    line_to: var_declarator.end_position().row + 1,
                                    context: CodeContext {
                                        module: current_module_name.clone(),
                                        file_path: file_path.to_string_lossy().to_string(),
                                        file_name: file_path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string(),
                                        struct_name: None,
                                        snippet: get_node_text(var_declarator, source_code),
                                    },
                                    embedding: None,
                                };
                                create_and_add_entity(entity, entities);
                                processed = true;
                            }
                        }

                        if !processed {
                            // println!("DEBUG TS: >>> ADDING Var/Const entity: {}", name);
                            let entity = CodeEntity {
                                name: name.clone(),
                                signature: get_node_text(var_declarator, source_code),
                                code_type: if node.child(0).map_or(false, |n| n.kind() == "const") {
                                    "Constant".to_string()
                                } else {
                                    "Variable".to_string()
                                },
                                docstring: potential_docstring.clone(),
                                line: name_n.start_position().row + 1,
                                line_from: doc_line_from,
                                line_to: var_declarator.end_position().row + 1,
                                context: CodeContext {
                                    module: current_module_name.clone(),
                                    file_path: file_path.to_string_lossy().to_string(),
                                    file_name: file_path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string(),
                                    struct_name: None,
                                    snippet: get_node_text(var_declarator, source_code),
                                },
                                embedding: None,
                            };
                            create_and_add_entity(entity, entities);
                        }
                    } else {
                        println!("DEBUG TS: VarDeclarator skipped - no name_node found");
                        // UNCOMMENTED
                    }
                }
            }
        }
        "class_declaration" => {
            let name_node = find_child_node_by_kind(node, "type_identifier")
                .or_else(|| find_child_node_by_kind(node, "identifier"));

            if let Some(name_n) = name_node {
                let class_name_str = get_node_text(name_n, source_code);
                // println!("DEBUG TS: Adding Class entity: {}", class_name_str);
                let entity = CodeEntity {
                    name: class_name_str.clone(),
                    signature: get_node_text(node, source_code),
                    code_type: "Class".to_string(),
                    docstring: potential_docstring.clone(),
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
                if let Some(body_node) = find_child_node_by_field_name(node, "body")
                    .or_else(|| find_child_node_by_kind(node, "class_body"))
                {
                    // println!("DEBUG TS: Class '{}' body kind: {}", class_name_str, body_node.kind());
                    let mut cursor = body_node.walk();
                    for child_of_body in body_node.named_children(&mut cursor) {
                        // println!("DEBUG TS: Class '{}' body child kind: {}, text: `{}`", class_name_str, child_of_body.kind(), get_node_text(child_of_body, source_code).lines().next().unwrap_or(""));
                        collect_ts_entities_recursive(
                            child_of_body,
                            source_code,
                            file_path,
                            current_module_name,
                            &Some(class_name_str.clone()),
                            entities,
                            None,
                            max_snippet_size,
                        );
                    }
                }
            } else {
                // println!("DEBUG TS: Class declaration skipped - no name node found.");
            }
        }
        "interface_declaration" => {
            let name_node = find_child_node_by_field_name(node, "name")
                .or_else(|| find_child_node_by_kind(node, "type_identifier"));
            if let Some(name_n) = name_node {
                let interface_name_str = get_node_text(name_n, source_code);
                let entity = CodeEntity {
                    name: interface_name_str.clone(),
                    signature: get_node_text(node, source_code),
                    code_type: "Interface".to_string(),
                    docstring: potential_docstring.clone(),
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
        }
        _ => {}
    }

    if !entity_created_for_this_node {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_ts_entities_recursive(
                child,
                source_code,
                file_path,
                current_module_name,
                current_class_name,
                entities,
                None,
                max_snippet_size,
            );
        }
    }
}

pub fn extract_ts_entities_from_file(
    file_path: &PathBuf,
    is_tsx: bool,
    max_snippet_size: Option<usize>,
) -> Result<Vec<CodeEntity>> {
    let source_code = fs::read_to_string(file_path)?;
    let mut parser = Parser::new();
    let language = if is_tsx {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    parser
        .set_language(&language)
        .map_err(|e| anyhow::anyhow!("Error loading TS/TSX grammar: {}", e))?;
    let tree = parser
        .parse(&source_code, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse TS/TSX code"))?;

    let mut entities = Vec::new();
    let root_node = tree.root_node();
    let initial_module_name = file_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());

    collect_ts_entities_recursive(
        root_node,
        &source_code,
        file_path,
        &initial_module_name,
        &None,
        &mut entities,
        None,
        max_snippet_size,
    );

    Ok(entities)
}

#[cfg(test)]
mod ts_entity_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_ts_function_and_class() -> Result<()> {
        let code = r#"
/**
 * This is a JSDoc for a function.
 */
function greet(name: string): string {
    return `Hello, ${name}`;
}

// A simple class
export class User {
    name: string;

    /**
     * Constructor for User.
     * @param name The name of the user.
     */
    constructor(name: string) {
        this.name = name;
    }

    /** A method. */
    getName(): string {
        return this.name;
    }
}
"#;
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(code.as_bytes())?;
        let file_path = temp_file.path().to_path_buf();

        let entities = extract_ts_entities_from_file(&file_path, false, None)?;

        // Dump the final entities for debugging
        println!("DEBUG TS TEST: Final entities found: {:#?}", entities);

        // Expected: function greet, class User, method constructor, method getName
        assert_eq!(
            entities.len(),
            4,
            "Expected 4 entities. Found: {:#?}",
            entities
        );

        let func_greet = entities
            .iter()
            .find(|e| e.name == "greet")
            .expect("Function 'greet' not found");
        assert_eq!(func_greet.code_type, "Function");
        assert!(func_greet
            .docstring
            .as_ref()
            .expect("greet docstring")
            .contains("JSDoc for a function"));

        let class_user = entities
            .iter()
            .find(|e| e.name == "User")
            .expect("Class 'User' not found");
        assert_eq!(class_user.code_type, "Class");
        assert!(class_user
            .docstring
            .as_ref()
            .map_or(false, |s| s.contains("A simple class"))); // Doc comment is before export

        let constructor_method = entities
            .iter()
            .find(|e| e.name == "constructor" && e.context.struct_name == Some("User".to_string()))
            .expect("Constructor not found");
        assert_eq!(constructor_method.code_type, "Method");
        assert!(constructor_method
            .docstring
            .as_ref()
            .expect("constructor docstring")
            .contains("Constructor for User"));

        let getname_method = entities
            .iter()
            .find(|e| e.name == "getName" && e.context.struct_name == Some("User".to_string()))
            .expect("getName method not found");
        assert_eq!(getname_method.code_type, "Method");
        assert!(getname_method
            .docstring
            .as_ref()
            .expect("getName docstring")
            .contains("A method."));

        Ok(())
    }

    #[test]
    fn test_extract_tsx_arrow_function_component() -> Result<()> {
        let code = r#"
/**
 * A simple TSX component.
 */
export const MyComponent = (props: { message: string }) => {
    return <div>{props.message}</div>;
};
"#;
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(code.as_bytes())?;
        let file_path = temp_file.path().to_path_buf();

        let entities = extract_ts_entities_from_file(&file_path, true, None)?;

        // Dump the final entities for debugging
        println!("DEBUG TSX TEST: Final entities found: {:#?}", entities);

        assert_eq!(
            entities.len(),
            1,
            "Expected 1 entity (arrow function component). Found: {:#?}",
            entities
        );

        let component = &entities[0];
        assert_eq!(component.name, "MyComponent");
        assert_eq!(component.code_type, "Function Component");
        assert!(component
            .docstring
            .as_ref()
            .expect("component docstring")
            .contains("A simple TSX component"));
        Ok(())
    }
} 