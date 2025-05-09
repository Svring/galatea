use tree_sitter::Node;

// Removed split_entity function - moved to processing.rs

pub fn get_node_text(node: Node, source_code: &str) -> String {
    node.utf8_text(source_code.as_bytes())
        .unwrap_or("")
        .to_string()
}

pub fn find_child_node_by_field_name<'a>(node: Node<'a>, field_name_str: &str) -> Option<Node<'a>> {
    for i in 0..node.named_child_count() {
        if let Some(child_node) = node.named_child(i) {
            if node.field_name_for_child(i as u32) == Some(field_name_str) {
                return Some(child_node);
            }
        }
    }
    None
}

pub fn find_child_node_by_kind<'a>(node: Node<'a>, kind_str: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child_node in node.named_children(&mut cursor) {
        if child_node.kind() == kind_str {
            return Some(child_node);
        }
    }
    None
}
