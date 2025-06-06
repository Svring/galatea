use poem::Route;
use poem_openapi::{payload::{Json as OpenApiJson, PlainText}, OpenApi, Object, ApiResponse, OpenApiService, Enum};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::dev_operation::editor::{self, EditorOperationResult, SHARED_EDITOR};
use crate::file_system; // For resolve_path
use crate::file_system::paths::{get_project_root, resolve_path};
use tokio::process::Command;
use std::fs;

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

#[derive(ApiResponse)]
enum FindFilesApiResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<FindFilesResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[derive(ApiResponse)]
enum ScriptApiResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<ScriptResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

/// The type of script operation to execute
#[derive(Enum, serde::Deserialize, PartialEq, Clone)]
#[oai(rename_all = "snake_case")]
enum ScriptOperation {
    /// Run linting checks on the project
    /// 
    /// Executes `pnpm run lint` to check code quality and style issues.
    /// Returns detailed output including any linting errors or warnings.
    Lint,
    
    /// Format code in the project
    /// 
    /// Executes `pnpm run format` to automatically format code according to
    /// project style guidelines. May modify files in place.
    Format,
    
    /// Build the project
    /// 
    /// Executes `pnpm run build` to compile and build the project.
    /// Returns build output and any compilation errors.
    Build,
    
    /// Run tests
    /// 
    /// Executes `pnpm run test` to run the project's test suite.
    /// Returns test results and coverage information if available.
    Test,
    
    /// Install dependencies
    /// 
    /// Executes `pnpm install` to install or update project dependencies.
    /// Useful for ensuring all packages are up to date.
    Install,
}

impl std::fmt::Display for ScriptOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptOperation::Lint => write!(f, "lint"),
            ScriptOperation::Format => write!(f, "format"),
            ScriptOperation::Build => write!(f, "build"),
            ScriptOperation::Test => write!(f, "test"),
            ScriptOperation::Install => write!(f, "install"),
        }
    }
}

#[derive(Object, serde::Serialize)]
pub struct ScriptResponse {
    /// Whether the script executed successfully
    /// 
    /// `true` if the script completed with exit code 0, `false` otherwise.
    /// Note that some operations (like linting) may return non-zero exit codes
    /// even when they complete successfully but find issues.
    pub success: bool,
    
    /// Standard output from the script execution
    /// 
    /// Contains the normal output from the executed command. For linting operations,
    /// this typically includes a summary of issues found. For build operations,
    /// this includes compilation progress and success messages.
    pub stdout: String,
    
    /// Standard error output from the script execution
    /// 
    /// Contains error messages and warnings from the executed command.
    /// May include detailed error descriptions, stack traces, or diagnostic information.
    pub stderr: String,
    
    /// Exit status code of the executed script
    /// 
    /// The numeric exit code returned by the process:
    /// - `0`: Success
    /// - `1`: General errors
    /// - `2`: Misuse of shell builtins
    /// - Other codes: Command-specific error conditions
    pub status: i32,
    
    /// The operation that was performed
    /// 
    /// String representation of the script operation that was executed.
    /// Useful for logging and identifying which operation produced this response.
    pub operation: String,
    
    /// Timestamp when the operation was completed
    /// 
    /// Unix timestamp (seconds since epoch) as a string representing when
    /// the script execution finished.
    pub executed_at: String,
    
    /// Duration of the script execution in milliseconds
    /// 
    /// How long the script took to execute, useful for performance monitoring
    /// and identifying slow operations.
    pub duration_ms: Option<u64>,
}

#[derive(Object, serde::Deserialize)] 
struct FindFilesRequest {
    /// Directory path to search within
    /// 
    /// **Required.** The directory to search for files. Can be absolute or relative
    /// to the project root. The path will be resolved and validated to ensure it's
    /// within the project boundaries for security.
    /// 
    /// Examples:
    /// - `"src"` - Search in the src directory
    /// - `"components"` - Search in components directory
    /// - `"."` - Search in project root
    /// - `"/absolute/path/to/search"` - Absolute path (must be within project)
    #[oai(validator(min_length = 1))]
    dir: String,
    
    /// File extensions to search for
    /// 
    /// **Required.** List of file extensions to include in the search.
    /// Extensions should be provided without the leading dot.
    /// 
    /// Examples:
    /// - `["ts", "tsx"]` - TypeScript files
    /// - `["js", "jsx", "ts", "tsx"]` - All JavaScript/TypeScript files
    /// - `["md"]` - Markdown files
    /// - `["json", "yaml", "yml"]` - Configuration files
    /// 
    /// **Note:** Empty list will return no results.
    #[oai(validator(min_items = 1))]
    suffixes: Vec<String>,
    
    /// Directories to exclude from search
    /// 
    /// **Optional.** List of directory names to skip during the search.
    /// If not provided, a sensible default list of common build/cache directories
    /// will be used: `node_modules`, `target`, `dist`, `build`, `.git`, `.vscode`, `.idea`.
    /// 
    /// Examples:
    /// - `["node_modules", "dist"]` - Skip these specific directories
    /// - `[]` - Don't exclude any directories (search everything)
    /// - `null` - Use default exclusion list
    exclude_dirs: Option<Vec<String>>,
    
    /// Maximum number of files to return
    /// 
    /// **Optional.** Limits the number of files returned to prevent overwhelming
    /// responses for large projects. Defaults to 1000 if not specified.
    /// 
    /// **Range:** 1 to 10000
    #[oai(validator(minimum(value = "1"), maximum(value = "10000")))]
    max_results: Option<usize>,
    
    /// Whether to include file size information
    /// 
    /// **Optional.** If `true`, the response will include file size information
    /// for each found file. Defaults to `false` for faster responses.
    include_file_info: Option<bool>,
}

#[derive(Object, serde::Serialize)]
struct FileInfo {
    /// File path relative to the search directory
    /// 
    /// The path to the file, relative to the directory that was searched.
    /// Always uses forward slashes as path separators for consistency.
    path: String,
    
    /// File size in bytes
    /// 
    /// Size of the file in bytes. Only included if `include_file_info` was `true`
    /// in the request. Will be `null` if file size could not be determined.
    size_bytes: Option<u64>,
    
    /// Last modified timestamp
    /// 
    /// Unix timestamp (seconds since epoch) when the file was last modified.
    /// Only included if `include_file_info` was `true` in the request.
    modified_at: Option<u64>,
}

#[derive(Object, serde::Serialize)]
struct FindFilesResponse {
    /// List of found files
    /// 
    /// Array of files that match the search criteria. If `include_file_info` was
    /// `false` in the request, each item will only have the `path` field populated.
    files: Vec<FileInfo>,
    
    /// Total number of files found
    /// 
    /// The total count of files that matched the search criteria, even if the
    /// results were limited by `max_results`. Useful for pagination or showing
    /// "X of Y results" information.
    total_found: usize,
    
    /// Whether results were truncated
    /// 
    /// `true` if the search found more files than `max_results` allowed.
    /// When `true`, consider refining the search criteria or increasing `max_results`.
    truncated: bool,
    
    /// Search parameters that were used
    /// 
    /// Echo of the search parameters for reference, useful for debugging
    /// or confirming what was actually searched.
    search_params: SearchParams,
}

#[derive(Object, serde::Serialize)]
struct SearchParams {
    /// Directory that was searched
    directory: String,
    
    /// File extensions that were searched for
    extensions: Vec<String>,
    
    /// Directories that were excluded
    excluded_directories: Vec<String>,
    
    /// Maximum results limit that was applied
    max_results: usize,
}

#[derive(Object, serde::Deserialize)]
struct ScriptExecutionRequest {
    /// The script operation to execute
    /// 
    /// **Required.** Specifies which script operation to run. Each operation
    /// corresponds to a specific npm/pnpm script in the project.
    operation: ScriptOperation,
    
    /// Additional arguments to pass to the script
    /// 
    /// **Optional.** Extra command-line arguments to pass to the script.
    /// These will be appended to the base command.
    /// 
    /// Examples:
    /// - For lint: `["--fix"]` to automatically fix issues
    /// - For test: `["--coverage"]` to generate coverage reports
    /// - For build: `["--production"]` for production builds
    args: Option<Vec<String>>,
    
    /// Working directory for script execution
    /// 
    /// **Optional.** Directory to run the script from. If not provided,
    /// defaults to the project root. Must be within the project boundaries.
    working_dir: Option<String>,
    
    /// Environment variables to set
    /// 
    /// **Optional.** Additional environment variables to set when running the script.
    /// These will be merged with the existing environment.
    /// 
    /// Example: `{"NODE_ENV": "development", "DEBUG": "true"}`
    env_vars: Option<std::collections::HashMap<String, String>>,
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

    /// Find files in the project by extension
    /// 
    /// Searches for files within a specified directory that match given file extensions.
    /// This is useful for discovering source files, configuration files, or any other
    /// files of specific types within the project structure.
    /// 
    /// ## Features:
    /// - **Recursive search**: Searches through all subdirectories
    /// - **Extension filtering**: Only returns files with specified extensions
    /// - **Directory exclusion**: Skips common build/cache directories by default
    /// - **Result limiting**: Prevents overwhelming responses for large projects
    /// - **File metadata**: Optionally includes file size and modification time
    /// - **Security**: All paths are validated to ensure they're within project boundaries
    /// 
    /// ## Default excluded directories:
    /// `node_modules`, `target`, `dist`, `build`, `.git`, `.vscode`, `.idea`
    /// 
    /// ## Examples:
    /// - Find all TypeScript files: `{"dir": "src", "suffixes": ["ts", "tsx"]}`
    /// - Find configuration files: `{"dir": ".", "suffixes": ["json", "yaml", "toml"]}`
    /// - Search everything: `{"dir": ".", "suffixes": ["*"], "exclude_dirs": []}`
    #[oai(path = "/find-files", method = "post")]
    async fn find_files_handler(
        &self,
        req: OpenApiJson<FindFilesRequest>,
    ) -> FindFilesApiResponse {
        // Validate and resolve directory path
        let dir = match resolve_path(&req.0.dir) {
            Ok(path) => path,
            Err(e) => {
                return FindFilesApiResponse::BadRequest(
                    PlainText(format!("Failed to resolve directory '{}': {}", req.0.dir, e)),
                );
            }
        };

        // Validate directory exists
        if !dir.exists() {
            return FindFilesApiResponse::BadRequest(
                PlainText(format!("Directory does not exist: {}", dir.display())),
            );
        }

        if !dir.is_dir() {
            return FindFilesApiResponse::BadRequest(
                PlainText(format!("Path is not a directory: {}", dir.display())),
            );
        }

        // Validate suffixes
        if req.0.suffixes.is_empty() {
            return FindFilesApiResponse::BadRequest(
                PlainText("At least one file extension must be specified".to_string()),
            );
        }

        // Set up search parameters
        let suffixes_ref: Vec<&str> = req.0.suffixes.iter().map(|s| s.as_str()).collect();
        let exclude_dirs = req.0.exclude_dirs.clone().unwrap_or_else(|| {
            vec![
                "node_modules".to_string(),
                "target".to_string(),
                "dist".to_string(),
                "build".to_string(),
                ".git".to_string(),
                ".vscode".to_string(),
                ".idea".to_string(),
                ".next".to_string(),
                "coverage".to_string(),
                ".nyc_output".to_string(),
            ]
        });
        let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
        let max_results = req.0.max_results.unwrap_or(1000);
        let include_file_info = req.0.include_file_info.unwrap_or(false);

        // Perform the search
        match file_system::search::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
            Ok(found_files) => {
                let total_found = found_files.len();
                let truncated = total_found > max_results;
                let files_to_process = if truncated {
                    &found_files[..max_results]
                } else {
                    &found_files[..]
                };

                let mut file_infos = Vec::new();
                for file_path in files_to_process {
                    let relative_path = match file_path.strip_prefix(&dir) {
                        Ok(rel) => rel.to_string_lossy().to_string(),
                        Err(_) => file_path.to_string_lossy().to_string(),
                    };

                    let (size_bytes, modified_at) = if include_file_info {
                        let metadata = fs::metadata(file_path).ok();
                        let size = metadata.as_ref().and_then(|m| Some(m.len()));
                        let modified = metadata.as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs());
                        (size, modified)
                    } else {
                        (None, None)
                    };

                    file_infos.push(FileInfo {
                        path: relative_path.replace('\\', "/"), // Normalize path separators
                        size_bytes,
                        modified_at,
                    });
                }

                let response = FindFilesResponse {
                    files: file_infos,
                    total_found,
                    truncated,
                    search_params: SearchParams {
                        directory: req.0.dir.clone(),
                        extensions: req.0.suffixes.clone(),
                        excluded_directories: exclude_dirs,
                        max_results,
                    },
                };

                FindFilesApiResponse::Ok(OpenApiJson(response))
            }
            Err(e) => FindFilesApiResponse::InternalServerError(
                PlainText(format!("Error searching directory '{}': {}", req.0.dir, e)),
            ),
        }
    }

    /// Execute a project script
    /// 
    /// Runs various project maintenance and development scripts such as linting,
    /// formatting, building, testing, or installing dependencies. This endpoint
    /// provides a unified interface for executing common development tasks.
    /// 
    /// ## Supported operations:
    /// - **lint**: Check code quality and style (`pnpm run lint`)
    /// - **format**: Auto-format code (`pnpm run format`)
    /// - **build**: Compile and build the project (`pnpm run build`)
    /// - **test**: Run the test suite (`pnpm run test`)
    /// - **install**: Install/update dependencies (`pnpm install`)
    /// 
    /// ## Features:
    /// - **Custom arguments**: Pass additional flags to the underlying commands
    /// - **Working directory**: Run scripts from specific directories
    /// - **Environment variables**: Set custom environment for script execution
    /// - **Detailed output**: Returns stdout, stderr, exit codes, and timing information
    /// - **Error handling**: Graceful handling of script failures with detailed diagnostics
    /// 
    /// ## Examples:
    /// - Basic lint: `{"operation": "lint"}`
    /// - Lint with auto-fix: `{"operation": "lint", "args": ["--fix"]}`
    /// - Test with coverage: `{"operation": "test", "args": ["--coverage"]}`
    /// - Production build: `{"operation": "build", "env_vars": {"NODE_ENV": "production"}}`
    #[oai(path = "/script", method = "post")]
    async fn script_handler(&self, req: OpenApiJson<ScriptExecutionRequest>) -> ScriptApiResponse {
        let start_time = std::time::Instant::now();
        
        // Determine working directory
        let working_dir = if let Some(ref wd) = req.0.working_dir {
            match resolve_path(wd) {
                Ok(path) => {
                    if !path.exists() || !path.is_dir() {
                        return ScriptApiResponse::BadRequest(
                            PlainText(format!("Working directory does not exist or is not a directory: {}", wd))
                        );
                    }
                    path
                }
                Err(e) => {
                    return ScriptApiResponse::BadRequest(
                        PlainText(format!("Failed to resolve working directory '{}': {}", wd, e))
                    );
                }
            }
        } else {
            match get_project_root() {
                Ok(pr) => pr,
                Err(e) => return ScriptApiResponse::InternalServerError(
                    PlainText(format!("Failed to get project root: {}", e))
                ),
            }
        };

        // Build command based on operation
        let (base_cmd, base_args) = match req.0.operation {
            ScriptOperation::Lint => ("pnpm", vec!["run", "lint"]),
            ScriptOperation::Format => ("pnpm", vec!["run", "format"]),
            ScriptOperation::Build => ("pnpm", vec!["run", "build"]),
            ScriptOperation::Test => ("pnpm", vec!["run", "test"]),
            ScriptOperation::Install => ("pnpm", vec!["install"]),
        };

        let mut cmd = Command::new(base_cmd);
        cmd.current_dir(&working_dir);
        
        // Add base arguments
        for arg in base_args {
            cmd.arg(arg);
        }
        
        // Add custom arguments if provided
        if let Some(ref args) = req.0.args {
            for arg in args {
                cmd.arg(arg);
            }
        }
        
        // Set environment variables if provided
        if let Some(ref env_vars) = req.0.env_vars {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        // Execute the command
        let output = match cmd.output().await {
            Ok(out) => out,
            Err(e) => return ScriptApiResponse::InternalServerError(
                PlainText(format!("Failed to execute {} {}: {}", base_cmd, req.0.operation, e))
            ),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        ScriptApiResponse::Ok(OpenApiJson(ScriptResponse {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: output.status.code().unwrap_or(-1),
            operation: req.0.operation.to_string(),
            executed_at: timestamp,
            duration_ms: Some(duration_ms),
        }))
    }

    /// Legacy lint endpoint (deprecated)
    /// 
    /// **Deprecated**: Use `/script` endpoint with `{"operation": "lint"}` instead.
    /// This endpoint is maintained for backward compatibility but may be removed in future versions.
    #[oai(path = "/lint", method = "post", deprecated = true)]
    async fn lint_handler(&self) -> ScriptApiResponse {
        let req = ScriptExecutionRequest {
            operation: ScriptOperation::Lint,
            args: None,
            working_dir: None,
            env_vars: None,
        };
        self.script_handler(OpenApiJson(req)).await
    }

    /// Legacy format endpoint (deprecated)
    /// 
    /// **Deprecated**: Use `/script` endpoint with `{"operation": "format"}` instead.
    /// This endpoint is maintained for backward compatibility but may be removed in future versions.
    #[oai(path = "/format", method = "post", deprecated = true)]
    async fn format_handler(&self) -> ScriptApiResponse {
        let req = ScriptExecutionRequest {
            operation: ScriptOperation::Format,
            args: None,
            working_dir: None,
            env_vars: None,
        };
        self.script_handler(OpenApiJson(req)).await
    }
}

pub fn editor_routes() -> Route {
    let api_service = OpenApiService::new(EditorApi, "Editor API", "1.0")
        .server("/api/editor");
    Route::new().nest("/", api_service)
} 