use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeContext {
    pub module: Option<String>,
    pub file_path: String,
    pub file_name: String,
    pub struct_name: Option<String>, // For Rust: Struct/Impl name. For TS: Class/Interface name
    pub snippet: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeEntity {
    pub name: String,
    pub signature: String,
    pub code_type: String, // e.g., "Function", "Struct", "Method", "Impl", "Trait", "Module", "Import", "Class", "Interface", "Variable"
    pub docstring: Option<String>,
    pub line: usize, // Starting line of the main definition (e.g., fn/class line)
    pub line_from: usize, // Starting line of the entire block (including doc comments)
    pub line_to: usize, // Ending line of the entire block
    pub context: CodeContext,
    #[serde(skip_serializing_if = "Option::is_none")] // Don't write embedding field if it's None
    pub embedding: Option<Vec<f32>>, // Added field for embedding vector
} 