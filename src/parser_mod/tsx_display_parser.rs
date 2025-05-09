use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use tree_sitter::{Node, Parser, Tree};

// Original TSX parsing logic - Reverted to simpler version
fn parse_tsx_code_for_display(code: &str) -> Tree {
    let mut parser = Parser::new();
    let language = tree_sitter_typescript::LANGUAGE_TSX.into();
    parser
        .set_language(&language)
        .expect("Error loading TypeScript parser");
    let tree = parser.parse(code, None).unwrap();
    assert!(!tree.root_node().has_error());
    tree
}

// Simplified printer for TSX display tests
fn print_node_recursive_for_tsx(result: &mut String, node: Node, indent: usize) {
    let indent_str = " ".repeat(indent);
    result.push_str(&format!(
        "{}{} ({}-{})
",
        indent_str,
        node.kind(),
        node.start_position(),
        node.end_position()
    ));
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i) {
            print_node_recursive_for_tsx(result, child, indent + 2);
        }
    }
}

fn format_tsx_tree_for_display(tree: &Tree) -> String {
    let mut result = String::new();
    print_node_recursive_for_tsx(&mut result, tree.root_node(), 0);
    result
}

// Public function for TSX display (used by old tests)
pub fn parse_and_print_tsx_file(file_path: &PathBuf) -> Result<String> {
    let tsx_code = fs::read_to_string(file_path)?;
    let tree = parse_tsx_code_for_display(&tsx_code);
    Ok(format_tsx_tree_for_display(&tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_simple_tsx_parsing_and_printing() -> Result<()> {
        let code = r#"
const MyComponent = () => (
  <div>
    <h1>Hello, world!</h1>
  </div>
);
"#;
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(code.as_bytes())?;
        let file_path = temp_file.path().to_path_buf();

        let printed_tree = parse_and_print_tsx_file(&file_path)?;

        assert!(printed_tree.contains("program "));
        assert!(printed_tree.contains("lexical_declaration "));
        assert!(printed_tree.contains("variable_declarator "));
        assert!(printed_tree.contains("identifier ")); // Check for the kind itself
        assert!(printed_tree.contains("arrow_function "));
        assert!(printed_tree.contains("parenthesized_expression "));
        assert!(printed_tree.contains("jsx_element "));
        assert!(printed_tree.contains("jsx_opening_element "));
        assert!(printed_tree.contains("jsx_text ")); // Check for presence of jsx_text kind
        assert!(printed_tree.contains("jsx_closing_element "));

        Ok(())
    }

    #[test]
    #[should_panic] // Panics due to assert!(!tree.root_node().has_error()) in parse_tsx_code
    fn test_invalid_tsx_parsing() {
        let code = r#"
const MyComponent = () => (
  <div>
    <h1>Hello, world!
  </div>
);
"#; // Missing closing h1 tag
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(code.as_bytes()).unwrap();
        let file_path = temp_file.path().to_path_buf();

        let _ = parse_and_print_tsx_file(&file_path);
    }
}
