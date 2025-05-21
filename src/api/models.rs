use serde::{Deserialize, Serialize};
use crate::dev_runtime::logging as galatea_logging; // Alias to avoid conflict if we have a local logging
use crate::codebase_indexing::parser::CodeEntity; // Updated path
use lsp_types;

// Keep existing structs from main.rs for now, will move them here.
// Placeholder to make the file non-empty 

#[derive(Debug, Serialize, Deserialize)]
pub struct FindFilesRequest {
    pub dir: String,
    pub suffixes: Vec<String>,
    pub exclude_dirs: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FindFilesResponse {
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ParseFileRequest {
    pub file_path: String,
    pub max_snippet_size: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ParseDirectoryRequest {
    pub dir: String,
    pub suffixes: Vec<String>,
    pub exclude_dirs: Option<Vec<String>>,
    pub max_snippet_size: Option<usize>,
    pub granularity: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryRequest {
    pub collection_name: String,
    pub query_text: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub qdrant_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GenerateEmbeddingsRequest {
    pub input_file: String,
    pub output_file: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GenericApiResponse {
    pub success: bool,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpsertEmbeddingsRequest {
    pub input_file: String,
    pub collection_name: String,
    pub qdrant_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BuildIndexRequest {
    pub dir: String,
    pub suffixes: Vec<String>,
    pub exclude_dirs: Option<Vec<String>>,
    pub max_snippet_size: Option<usize>,
    pub granularity: Option<String>,
    pub embedding_model: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub collection_name: String,
    pub qdrant_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditorCommandRequest {
    pub command: String,
    pub path: String,
    pub file_text: Option<String>,
    pub insert_line: Option<usize>,
    pub new_str: Option<String>,
    pub old_str: Option<String>,
    pub view_range: Option<Vec<isize>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EditorCommandResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_lines: Option<Vec<usize>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetLogsRequest {
    #[serde(flatten)]
    pub filter_options: galatea_logging::LogFilterOptions,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetLogsResponse {
    pub success: bool,
    pub logs: Vec<galatea_logging::LogEntry>,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClearLogsResponse {
    pub success: bool,
    pub message: String,
}

// LSP related structs moved from dev_operation/models.rs
#[derive(Debug, Serialize, Deserialize)]
pub struct GotoDefinitionApiRequest {
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GotoDefinitionApiResponse {
    pub locations: Option<lsp_types::GotoDefinitionResponse>,
}

// Re-exporting GotoDefinitionApiRequest and GotoDefinitionApiResponse if they are made public in dev_operation::models
// pub use crate::dev_operation::models::{GotoDefinitionApiRequest, GotoDefinitionApiResponse};
// Alternatively, define them here if they are purely API models:
/*
#[derive(Debug, Serialize, Deserialize)]
pub struct GotoDefinitionApiRequest {
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GotoDefinitionApiResponse {
    pub locations: Option<lsp_types::GotoDefinitionResponse>,
}
*/ 