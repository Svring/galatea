use poem::Route;
use poem_openapi::{payload::{Json as OpenApiJson, PlainText}, OpenApi, Object, ApiResponse, OpenApiService, Enum};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::dev_operation::editor::{self, EditorOperationResult, SHARED_EDITOR};
use crate::file_system; // For resolve_path
use crate::file_system::paths::get_project_root;

// Define an API struct
pub struct EditorApi;

/// The editor command to execute
#[derive(Enum, serde::Deserialize, PartialEq, Clone)]
#[oai(rename_all = "snake_case")]
enum EditorCommand {
    /// View file contents - Read and display file content(s)
    /// 
    /// Can view a single file (use `path`) or multiple files (use `paths`).
    /// Supports optional line range specification with `view_range`.
    View,
    
    /// Create a new file - Write content to a new or existing file
    /// 
    /// Creates parent directories if needed. Will overwrite existing files.
    /// Requires `path` and `file_text` parameters.
    Create,
    
    /// Replace text in file - Find and replace text within a file
    /// 
    /// Performs case-sensitive text replacement. Replaces ALL occurrences.
    /// Requires `path` and `old_str`. Optional `new_str` (defaults to empty string for deletion).
    StrReplace,
    
    /// Insert text at line - Add new text after a specific line number
    /// 
    /// Line numbers are 1-indexed. Text is inserted AFTER the specified line.
    /// Requires `path`, `insert_line`, and `new_str` parameters.
    Insert,
    
    /// Undo last edit - Reverse the most recent edit operation
    /// 
    /// Can undo create, str_replace, or insert operations. Only one level of undo is supported.
    /// No additional parameters required.
    UndoEdit,
}

impl std::fmt::Display for EditorCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorCommand::View => write!(f, "view"),
            EditorCommand::Create => write!(f, "create"),
            EditorCommand::StrReplace => write!(f, "str_replace"),
            EditorCommand::Insert => write!(f, "insert"),
            EditorCommand::UndoEdit => write!(f, "undo_edit"),
        }
    }
}

impl From<EditorCommand> for editor::CommandType {
    fn from(cmd: EditorCommand) -> Self {
        match cmd {
            EditorCommand::View => editor::CommandType::View,
            EditorCommand::Create => editor::CommandType::Create,
            EditorCommand::StrReplace => editor::CommandType::StrReplace,
            EditorCommand::Insert => editor::CommandType::Insert,
            EditorCommand::UndoEdit => editor::CommandType::UndoEdit,
        }
    }
}

#[derive(Object, serde::Deserialize)]
struct EditorCommandRequest {
    /// The editor command to execute
    /// 
    /// Specifies which operation to perform. Each command has different parameter requirements.
    /// See the endpoint description for detailed command-specific documentation.
    command: EditorCommand,
    
    /// File path for single-file operations
    /// 
    /// **Required for:** create, str_replace, insert
    /// **Optional for:** view (when using single file), undo_edit
    /// **Not used for:** view with multiple files (use `paths` instead)
    /// 
    /// Can be absolute or relative to the project root. Examples:
    /// - `"src/main.rs"`
    /// - `"components/Button.tsx"`
    /// - `"/absolute/path/to/file.txt"`
    /// 
    /// For security, paths are resolved relative to the project root and cannot escape it.
    path: Option<String>,
    
    /// Multiple file paths for multi-file view operations
    /// 
    /// **Required for:** view command when viewing multiple files
    /// **Not used for:** any other commands
    /// 
    /// Cannot be used together with `path`. Must contain at least one path.
    /// Each path follows the same rules as the `path` parameter.
    /// 
    /// Example: `["src/main.rs", "src/lib.rs", "README.md"]`
    paths: Option<Vec<String>>,
    
    /// Content for new file creation
    /// 
    /// **Required for:** create command
    /// **Not used for:** view, str_replace, insert, undo_edit
    /// 
    /// The complete text content to write to the file. Supports any text format
    /// including code, markdown, JSON, etc. Line endings will be normalized.
    /// 
    /// Example: `"fn main() {\n    println!(\"Hello, world!\");\n}"`
    file_text: Option<String>,
    
    /// Line number (1-indexed) after which to insert text
    /// 
    /// **Required for:** insert command
    /// **Not used for:** view, create, str_replace, undo_edit
    /// 
    /// Must be a positive integer. The text will be inserted AFTER this line:
    /// - Line 1: Insert after the first line (new text becomes line 2)
    /// - Line 0: Invalid (must be ≥ 1)
    /// - Beyond file end: Error (cannot insert after non-existent line)
    /// 
    /// For empty files, use line 1 to insert the first line.
    #[oai(validator(minimum(value = "1")))]
    insert_line: Option<usize>,
    
    /// New text content for insert and replace operations
    /// 
    /// **Required for:** insert command
    /// **Optional for:** str_replace command (defaults to empty string for deletion)
    /// **Not used for:** view, create, undo_edit
    /// 
    /// For **insert**: The text to insert at the specified line.
    /// For **str_replace**: The replacement text (empty string deletes the matched text).
    /// 
    /// Examples:
    /// - Insert: `"    console.log('Debug info');"`
    /// - Replace: `"newFunctionName"` or `""` (for deletion)
    new_str: Option<String>,
    
    /// Text to find and replace
    /// 
    /// **Required for:** str_replace command
    /// **Not used for:** view, create, insert, undo_edit
    /// 
    /// The exact text to search for in the file. Matching is case-sensitive and literal
    /// (no regex). ALL occurrences will be replaced. Cannot be empty.
    /// 
    /// Examples:
    /// - `"oldFunctionName"`
    /// - `"TODO: implement this"`
    /// - `"const oldValue = 42;"`
    #[oai(validator(min_length = 1))]
    old_str: Option<String>,
    
    /// Line range for viewing files [start_line, end_line]
    /// 
    /// **Optional for:** view command
    /// **Not used for:** create, str_replace, insert, undo_edit
    /// 
    /// Specifies which lines to return when viewing files. Both numbers are 1-indexed.
    /// Must be exactly 2 elements: `[start_line, end_line]`
    /// 
    /// **Special values:**
    /// - `end_line = -1`: Read from start_line to end of file
    /// - `[1, -1]`: Read entire file (same as omitting view_range)
    /// - `[5, 10]`: Read lines 5 through 10 (inclusive)
    /// - `[1, 1]`: Read only the first line
    /// 
    /// **Validation:**
    /// - start_line must be ≥ 1
    /// - end_line must be ≥ start_line or -1
    /// - start_line cannot exceed file length
    /// - If end_line exceeds file length, it's clamped to file end
    view_range: Option<Vec<i32>>,
}

#[derive(Object, serde::Serialize, Clone)]
struct EditorFileViewResponse {
    /// File path that was requested
    /// 
    /// The original path from the request, useful for identifying which file
    /// this response corresponds to in multi-file operations.
    path: String,
    
    /// File content if successfully read
    /// 
    /// Contains the actual text content of the file. Will be `null` if there was
    /// an error reading the file (check the `error` field in that case).
    /// 
    /// For view operations with `view_range`, only the requested lines are included.
    /// Line endings are preserved as they exist in the file.
    content: Option<String>,
    
    /// Error message if file could not be read
    /// 
    /// Contains a human-readable error description if the file operation failed.
    /// Common errors include:
    /// - File not found
    /// - Permission denied
    /// - Invalid UTF-8 content
    /// - Path outside project root
    /// 
    /// Will be `null` if the operation succeeded.
    error: Option<String>,
    
    /// Number of lines in the file if successfully read
    /// 
    /// Total line count of the file content. For view operations with `view_range`,
    /// this still represents the total file length, not just the returned range.
    /// 
    /// Will be `null` if there was an error reading the file.
    line_count: Option<usize>,
}

#[derive(Object, serde::Serialize)]
struct EditorCommandResponse {
    /// Whether the command executed successfully
    /// 
    /// `true` if the operation completed without errors, `false` otherwise.
    /// Even if `success` is `true`, individual files in multi-file operations
    /// might have errors (check the `error` field in `multi_content` items).
    success: bool,
    
    /// Human-readable message about the operation result
    /// 
    /// Provides additional context about what happened during the operation.
    /// Examples:
    /// - `"Command 'view' executed successfully."`
    /// - `"Command 'create' executed successfully."`
    /// - `"File created and content updated."`
    message: Option<String>,
    
    /// File content for single-file operations
    /// 
    /// **Populated for:**
    /// - `view` command (single file)
    /// - `create`, `str_replace`, `insert` commands (shows updated content)
    /// 
    /// **Not populated for:**
    /// - `view` command with multiple files (see `multi_content`)
    /// - `undo_edit` command
    /// - Failed operations
    /// 
    /// Contains the complete file content after the operation.
    content: Option<String>,
    
    /// File path that was operated on for single-file operations
    /// 
    /// **Populated for:** All single-file operations
    /// **Not populated for:** Multi-file view operations
    /// 
    /// Shows the resolved absolute path that was actually used for the operation.
    file_path: Option<String>,
    
    /// Number of lines in the file for single-file operations
    /// 
    /// **Populated for:** Operations that return `content`
    /// **Not populated for:** Multi-file operations or failed operations
    /// 
    /// Represents the total line count after the operation completed.
    line_count: Option<usize>,
    
    /// Multiple file contents for multi-file view operations
    /// 
    /// **Populated for:** `view` command with `paths` parameter
    /// **Not populated for:** All other operations
    /// 
    /// Contains an array of file responses, one for each requested file.
    /// Each item includes the file path, content (or error), and line count.
    multi_content: Option<Vec<EditorFileViewResponse>>,
    
    /// The operation that was performed
    /// 
    /// **Always populated.** Contains the string representation of the command:
    /// - `"view"`, `"create"`, `"str_replace"`, `"insert"`, or `"undo_edit"`
    /// 
    /// Useful for logging and debugging to confirm which operation was executed.
    operation: Option<String>,
    
    /// Timestamp when the operation was completed
    /// 
    /// **Always populated.** Unix timestamp (seconds since epoch) as a string.
    /// Represents when the server finished processing the request.
    /// 
    /// Example: `"1703123456"`
    modified_at: Option<String>,
    
    /// Line numbers that were modified for edit operations
    /// 
    /// **Populated for:** Some edit operations where line tracking is available
    /// **Not populated for:** View operations or when line tracking is not feasible
    /// 
    /// Contains 1-indexed line numbers that were changed by the operation:
    /// - `str_replace`: Lines where replacements occurred (estimated)
    /// - `insert`: The line number where text was inserted
    /// - `create`: Not typically populated
    /// 
    /// This is a best-effort field and may not be available for all operations.
    modified_lines: Option<Vec<usize>>,
}

#[derive(ApiResponse)]
enum HealthResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
}

#[derive(ApiResponse)]
enum EditorCommandApiResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<EditorCommandResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 404)]
    NotFound(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[OpenApi]
impl EditorApi {
    /// Health check endpoint for the Editor API
    /// 
    /// Returns a simple status message to verify that the Editor API is running and accessible.
    /// This endpoint can be used for monitoring and health checks.
    #[oai(path = "/health", method = "get")]
    async fn editor_health(&self) -> HealthResponse {
        HealthResponse::Ok(PlainText("Editor API route is healthy".to_string()))
    }

    /// Execute an editor command
    /// 
    /// This is the main endpoint for performing file operations. It supports various commands:
    /// 
    /// - **view**: Read file contents (single file or multiple files)
    /// - **create**: Create a new file with specified content
    /// - **str_replace**: Find and replace text within a file
    /// - **insert**: Insert text at a specific line number
    /// - **undo_edit**: Undo the last edit operation
    /// 
    /// ## Command-specific requirements:
    /// 
    /// ### view
    /// - Requires either `path` (single file) OR `paths` (multiple files), but not both
    /// - Optional `view_range` to specify line range [start, end] (1-indexed, use -1 for end of file)
    /// 
    /// ### create
    /// - Requires `path` (target file path) and `file_text` (content to write)
    /// - Will create parent directories if they don't exist
    /// - Will overwrite existing files
    /// 
    /// ### str_replace
    /// - Requires `path`, `old_str` (text to find), and optionally `new_str` (replacement text, defaults to empty)
    /// - Replaces ALL occurrences of `old_str` with `new_str`
    /// - Case-sensitive matching
    /// 
    /// ### insert
    /// - Requires `path`, `insert_line` (1-indexed line number), and `new_str` (text to insert)
    /// - Inserts text AFTER the specified line number
    /// - Line 1 means insert after the first line (becomes line 2)
    /// 
    /// ### undo_edit
    /// - No additional parameters required
    /// - Undoes the last create, str_replace, or insert operation
    /// - Can only undo one level (no multiple undo history)
    /// 
    /// ## Response format:
    /// - Single-file operations return content in the `content` field
    /// - Multi-file view operations return an array in the `multi_content` field
    /// - Edit operations (create, str_replace, insert) will also return the updated file content
    #[oai(path = "/command", method = "post")]
    async fn editor_command_handler(
        &self,
        req: OpenApiJson<EditorCommandRequest>,
    ) -> EditorCommandApiResponse {
        let command_type = match req.0.command {
            EditorCommand::View => editor::CommandType::View,
            EditorCommand::Create => editor::CommandType::Create,
            EditorCommand::StrReplace => editor::CommandType::StrReplace,
            EditorCommand::Insert => editor::CommandType::Insert,
            EditorCommand::UndoEdit => editor::CommandType::UndoEdit,
        };

        // Path validation for non-view commands
        if command_type != editor::CommandType::View && req.0.path.is_none() {
            return EditorCommandApiResponse::BadRequest(
                PlainText(format!("'path' is required for command type '{}'", req.0.command)),
            );
        }
        
        // Path validation for view command
        if command_type == editor::CommandType::View && req.0.path.is_none() && req.0.paths.is_none() {
            return EditorCommandApiResponse::BadRequest(
                PlainText("For 'view' command, either 'path' or 'paths' must be provided.".to_string()),
            );
        }
        if command_type == editor::CommandType::View && req.0.path.is_some() && req.0.paths.is_some() {
            return EditorCommandApiResponse::BadRequest(
                PlainText("For 'view' command, provide either 'path' or 'paths', not both.".to_string()),
            );
        }
        if command_type == editor::CommandType::View && req.0.paths.as_ref().map_or(false, |p| p.is_empty()) {
            return EditorCommandApiResponse::BadRequest(
                PlainText("For 'view' command with 'paths', the list cannot be empty.".to_string()),
            );
        }

        // Resolve path(s) and check existence for non-create/undo commands
        let mut resolved_single_path: Option<PathBuf> = None;
        let mut resolved_multiple_paths: Option<Vec<PathBuf>> = None;

        if command_type != editor::CommandType::Create && command_type != editor::CommandType::UndoEdit {
            if let Some(p_str) = &req.0.path {
                let resolved_p = match file_system::resolve_path(p_str) {
                    Ok(path) => path,
                    Err(e) => {
                        return EditorCommandApiResponse::BadRequest(
                            PlainText(e.to_string()),
                        );
                    }
                };
                if !resolved_p.exists() {
                    return EditorCommandApiResponse::NotFound(
                        PlainText(format!("File not found at resolved path: {}", resolved_p.display())),
                    );
                }
                resolved_single_path = Some(resolved_p);
            } else if let Some(p_strs) = &req.0.paths {
                let mut temp_resolved_paths = Vec::new();
                for p_str in p_strs {
                    let resolved_p = match file_system::resolve_path(p_str) {
                        Ok(path) => path,
                        Err(e) => {
                            return EditorCommandApiResponse::BadRequest(
                                PlainText(e.to_string()),
                            );
                        }
                    };
                    if !resolved_p.exists() {
                        return EditorCommandApiResponse::NotFound(
                            PlainText(format!("File not found at resolved path: {}", resolved_p.display())),
                        );
                    }
                    temp_resolved_paths.push(resolved_p);
                }
                resolved_multiple_paths = Some(temp_resolved_paths);
            }
        } else if command_type == editor::CommandType::Create {
            // For create, path is needed but doesn't need to exist yet.
            if let Some(p_str) = &req.0.path {
                // Custom logic for new file creation: join to project root, canonicalize parent, check containment
                let proj_root = match get_project_root() {
                    Ok(root) => root,
                    Err(e) => {
                        return EditorCommandApiResponse::InternalServerError(
                            PlainText(e.to_string()),
                        );
                    }
                };
                let requested_path = std::path::Path::new(p_str);
                let candidate = if requested_path.is_absolute() {
                    if requested_path.starts_with(&proj_root) {
                        requested_path.to_path_buf()
                    } else {
                        proj_root.join(requested_path.file_name().unwrap_or_default())
                    }
                } else {
                    let stripped = requested_path.strip_prefix(proj_root.file_name().unwrap_or_default()).unwrap_or(requested_path);
                    proj_root.join(stripped)
                };
                // Canonicalize parent to check containment
                let parent = match candidate.parent() {
                    Some(p) => p,
                    None => {
                        return EditorCommandApiResponse::BadRequest(
                            PlainText("Invalid path: no parent directory".to_string()),
                        );
                    }
                };
                let canonical_parent = match dunce::canonicalize(parent) {
                    Ok(cp) => cp,
                    Err(e) => {
                        return EditorCommandApiResponse::BadRequest(
                            PlainText(format!("Failed to canonicalize parent directory: {}", e)),
                        );
                    }
                };
                if !canonical_parent.starts_with(&proj_root) {
                    return EditorCommandApiResponse::BadRequest(
                        PlainText("Target path is outside the project root".to_string()),
                    );
                }
                resolved_single_path = Some(candidate);
            } else {
                return EditorCommandApiResponse::BadRequest(
                    PlainText("'path' is required for create.".to_string()),
                );
            }
        } else if command_type == editor::CommandType::UndoEdit {
            // Undo might operate on a path stored in the editor, but API may still provide it for consistency or future use.
            if let Some(p_str) = &req.0.path {
                resolved_single_path = file_system::resolve_path(p_str).ok(); // Optional resolution for undo
            }
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let editor_args_path = resolved_single_path.as_ref().map(|p| p.to_string_lossy().into_owned());
        let editor_args_paths = resolved_multiple_paths.as_ref().map(|vec_p| vec_p.iter().map(|p| p.to_string_lossy().into_owned()).collect());

        // Convert view_range from i32 to isize
        let view_range_isize = req.0.view_range.as_ref().map(|vr| vr.iter().map(|&x| x as isize).collect());

        let editor_args = editor::EditorArgs {
            command: command_type.clone(),
            path: editor_args_path.clone(),
            paths: editor_args_paths,
            file_text: req.0.file_text.clone(),
            insert_line: req.0.insert_line,
            new_str: req.0.new_str.clone(),
            old_str: req.0.old_str.clone(),
            view_range: view_range_isize,
        };

        // Use the shared editor state
        let mut editor_guard = match SHARED_EDITOR.lock() {
            Ok(guard) => guard,
            Err(e) => {
                return EditorCommandApiResponse::InternalServerError(
                    PlainText(format!("Failed to acquire editor lock: {}", e)),
                );
            }
        };
        
        match editor::handle_command(&mut *editor_guard, editor_args) {
            Ok(editor_result) => {
                match editor_result {
                    EditorOperationResult::Single(Some(content)) => {
                        EditorCommandApiResponse::Ok(OpenApiJson(EditorCommandResponse {
                            success: true,
                            message: Some(format!("Command '{}' executed successfully.", req.0.command)),
                            content: Some(content.clone()),
                            file_path: editor_args_path,
                            operation: Some(req.0.command.to_string()),
                            line_count: Some(content.lines().count()),
                            modified_at: Some(timestamp),
                            multi_content: None,
                            modified_lines: None,
                        }))
                    }
                    EditorOperationResult::Single(None) => {
                        let mut response = EditorCommandResponse {
                            success: true,
                            message: Some(format!("Command '{}' executed successfully.", req.0.command)),
                            content: None,
                            file_path: editor_args_path.clone(),
                            operation: Some(req.0.command.to_string()),
                            modified_at: Some(timestamp),
                            line_count: None,
                            multi_content: None,
                            modified_lines: None,
                        };
                        
                        // If it was a mutating command, try to view the file to get its new content and line count
                        if req.0.command == EditorCommand::Create || req.0.command == EditorCommand::StrReplace || req.0.command == EditorCommand::Insert || req.0.command == EditorCommand::UndoEdit {
                            if let Some(ref p) = editor_args_path {
                                let view_args = editor::EditorArgs {
                                    command: editor::CommandType::View,
                                    path: Some(p.clone()),
                                    paths: None,
                                    file_text: None,
                                    insert_line: None,
                                    new_str: None,
                                    old_str: None,
                                    view_range: None,
                                };
                                if let Ok(EditorOperationResult::Single(Some(updated_content))) = editor::handle_command(&mut *editor_guard, view_args) {
                                    response.content = Some(updated_content.clone());
                                    response.line_count = Some(updated_content.lines().count());
                                    if req.0.command == EditorCommand::StrReplace && req.0.old_str.is_some() {
                                        if let Some(old_str_val) = &req.0.old_str {
                                            let line_c = old_str_val.lines().count();
                                            if line_c > 0 && line_c < 100 {
                                                response.modified_lines = Some((1..=line_c).collect());
                                            }
                                        }
                                    }
                                    if req.0.command == EditorCommand::Insert && req.0.insert_line.is_some() {
                                        response.modified_lines = Some(vec![req.0.insert_line.unwrap()]);
                                    }
                                }
                            }
                        }
                        EditorCommandApiResponse::Ok(OpenApiJson(response))
                    }
                    EditorOperationResult::Multi(multi_file_outputs) => {
                        let api_multi_content: Vec<EditorFileViewResponse> = multi_file_outputs
                            .into_iter()
                            .map(|output| EditorFileViewResponse {
                                path: output.path,
                                content: output.content,
                                error: output.error,
                                line_count: output.line_count,
                            })
                            .collect();
                        EditorCommandApiResponse::Ok(OpenApiJson(EditorCommandResponse {
                            success: true,
                            message: Some(format!("Command '{}' (multi-file) executed successfully.", req.0.command)),
                            multi_content: Some(api_multi_content),
                            operation: Some(req.0.command.to_string()),
                            modified_at: Some(timestamp),
                            content: None,
                            file_path: None,
                            line_count: None,
                            modified_lines: None,
                        }))
                    }
                }
            },
            Err(e) => EditorCommandApiResponse::BadRequest(PlainText(e.to_string())),
        }
    }
}

pub fn editor_routes() -> Route {
    let api_service = OpenApiService::new(EditorApi, "Editor API", "1.0")
        .server("/api/editor");
    Route::new().nest("/", api_service)
} 