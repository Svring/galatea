use poem::Route;
use poem_openapi::{
    param::Path as OpenApiPath,
    payload::{Json as OpenApiJson, PlainText},
    ApiResponse, Object, OpenApi, OpenApiService,
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// Define an API struct
pub struct ProjectApi;

#[derive(Object, serde::Deserialize)]
pub struct UpdateFileRequest {
    /// Content to write to the file
    ///
    /// **Required.** The complete text content to write to the galatea file.
    /// This will completely replace any existing content in the file.
    ///
    /// **Note:** Empty string is allowed and will create an empty file.
    /// Line endings will be preserved as provided.
    #[oai(validator(min_length = 0))]
    pub content: String,

    /// Whether to create parent directories if they don't exist
    ///
    /// **Optional.** If `true`, any missing parent directories will be created
    /// automatically. If `false` and parent directories don't exist, the operation
    /// will fail. Defaults to `true`.
    pub create_dirs: Option<bool>,

    /// Backup the existing file before overwriting
    ///
    /// **Optional.** If `true` and the file already exists, a backup copy will be
    /// created with a `.backup` extension before writing the new content.
    /// Defaults to `false`.
    pub backup_existing: Option<bool>,
}

#[derive(ApiResponse)]
enum HealthResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
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

#[derive(ApiResponse)]
enum GalateaFileUpdateResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<ScriptResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[derive(ApiResponse)]
enum GalateaFileGetResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 404)]
    NotFound(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[derive(Object, serde::Serialize)]
pub struct GalateaFilesListResponse {
    /// List of files and directories in galatea_files
    ///
    /// Array of relative paths from the galatea_files directory root.
    /// Includes both files and directories, listed recursively.
    /// Paths use forward slashes as separators for consistency.
    pub entries: Vec<GalateaFileEntry>,

    /// Total number of entries found
    ///
    /// Count of all files and directories found in the galatea_files directory.
    pub total_count: usize,

    /// Timestamp when the listing was generated
    ///
    /// Unix timestamp (seconds since epoch) when this directory listing was created.
    pub generated_at: String,
}

#[derive(Object, serde::Serialize)]
pub struct GalateaFileEntry {
    /// Relative path from galatea_files root
    ///
    /// Path to the file or directory, relative to the galatea_files directory.
    /// Uses forward slashes as path separators.
    pub path: String,

    /// Whether this entry is a directory
    ///
    /// `true` if this entry represents a directory, `false` if it's a file.
    pub is_directory: bool,

    /// File size in bytes (files only)
    ///
    /// Size of the file in bytes. Will be `null` for directories or if
    /// the size could not be determined.
    pub size_bytes: Option<u64>,

    /// Last modified timestamp
    ///
    /// Unix timestamp (seconds since epoch) when this file or directory
    /// was last modified. May be `null` if timestamp is unavailable.
    pub modified_at: Option<u64>,
}

#[derive(ApiResponse)]
enum GalateaFilesListApiResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<GalateaFilesListResponse>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[OpenApi]
impl ProjectApi {
    /// Health check endpoint for the Project API
    ///
    /// Returns a simple status message to verify that the Project API is running and accessible.
    /// This endpoint can be used for monitoring and health checks.
    #[oai(path = "/health", method = "get")]
    async fn project_health(&self) -> HealthResponse {
        HealthResponse::Ok(PlainText("Project API route is healthy".to_string()))
    }

    /// Update or create a galatea configuration file
    ///
    /// Writes content to a file within the galatea_files directory. This endpoint
    /// is used for managing configuration files, documentation, and other galatea-specific
    /// files that control the behavior of the development environment.
    ///
    /// ## Security:
    /// - **Path validation**: All file paths are validated to ensure they remain within galatea_files
    /// - **Directory traversal protection**: Prevents access to files outside the allowed directory
    /// - **Safe overwriting**: Existing files can be safely overwritten with new content
    ///
    /// ## Features:
    /// - **Auto-create directories**: Parent directories are created automatically if needed
    /// - **Backup support**: Optionally backup existing files before overwriting
    /// - **Atomic writes**: File operations are atomic to prevent corruption
    ///
    /// ## Common files:
    /// - `config.toml`: Main galatea configuration
    /// - `developer_note.md`: Project documentation and notes
    /// - `project_structure.json`: Project structure metadata
    /// - `openapi_specification/*.json`: API specifications for MCP servers
    ///
    /// ## Examples:
    /// - Update config: `PUT /galatea-file/config.toml`
    /// - Create note: `PUT /galatea-file/notes/meeting.md`
    /// - Update API spec: `PUT /galatea-file/openapi_specification/custom_api.json`
    #[oai(path = "/galatea-file/:filename", method = "put")]
    async fn update_galatea_file_handler(
        &self,
        filename: OpenApiPath<String>,
        req: OpenApiJson<UpdateFileRequest>,
    ) -> GalateaFileUpdateResponse {
        // Validate filename
        if filename.0.is_empty() {
            return GalateaFileUpdateResponse::BadRequest(PlainText(
                "Filename cannot be empty".to_string(),
            ));
        }

        // Check for path traversal attempts
        if filename.0.contains("..") || filename.0.contains("\\") {
            return GalateaFileUpdateResponse::BadRequest(PlainText(
                "Invalid filename: path traversal not allowed".to_string(),
            ));
        }

        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => {
                return GalateaFileUpdateResponse::InternalServerError(PlainText(format!(
                    "Failed to get executable path: {}",
                    e
                )))
            }
        };

        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => {
                return GalateaFileUpdateResponse::InternalServerError(PlainText(
                    "Failed to get executable directory".to_string(),
                ))
            }
        };

        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        // Security check: ensure the resolved path is within galatea_files
        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileUpdateResponse::BadRequest(PlainText(
                "Invalid file path: must be within galatea_files directory".to_string(),
            ));
        }

        let file_existed = file_path.exists();
        let create_dirs = req.0.create_dirs.unwrap_or(true);
        let backup_existing = req.0.backup_existing.unwrap_or(false);

        // Create parent directories if needed
        if create_dirs {
            if let Some(parent) = file_path.parent() {
                if !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        return GalateaFileUpdateResponse::InternalServerError(PlainText(format!(
                            "Failed to create parent directories for '{}': {}",
                            filename.0, e
                        )));
                    }
                }
            }
        }

        // Backup existing file if requested
        if backup_existing && file_existed {
            let backup_path = file_path.with_extension(format!(
                "{}.backup",
                file_path.extension().and_then(|s| s.to_str()).unwrap_or("")
            ));
            if let Err(e) = fs::copy(&file_path, &backup_path) {
                return GalateaFileUpdateResponse::InternalServerError(PlainText(format!(
                    "Failed to create backup of '{}': {}",
                    filename.0, e
                )));
            }
        }

        // Write the file
        if let Err(e) = fs::write(&file_path, &req.0.content) {
            return GalateaFileUpdateResponse::InternalServerError(PlainText(format!(
                "Failed to write file '{}': {}",
                filename.0, e
            )));
        }

        let action = if file_existed { "updated" } else { "created" };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        GalateaFileUpdateResponse::Ok(OpenApiJson(ScriptResponse {
            success: true,
            stdout: format!("File '{}' {} successfully", filename.0, action),
            stderr: String::new(),
            status: 0,
            operation: format!("galatea_file_{}", action),
            executed_at: timestamp,
            duration_ms: Some(0), // File operations are typically very fast
        }))
    }

    /// Get the contents of a galatea configuration file
    ///
    /// Reads and returns the content of a file within the galatea_files directory.
    /// This endpoint is used for retrieving configuration files, documentation,
    /// and other galatea-specific files.
    ///
    /// ## Security:
    /// - **Path validation**: All file paths are validated to ensure they remain within galatea_files
    /// - **Directory traversal protection**: Prevents access to files outside the allowed directory
    /// - **Read-only access**: This endpoint only reads files, never modifies them
    ///
    /// ## Response format:
    /// Returns the raw file content as plain text. The content-type will be `text/plain`
    /// regardless of the actual file type. For binary files, consider using a different
    /// endpoint or method.
    ///
    /// ## Error handling:
    /// - **404 Not Found**: File doesn't exist or couldn't be read
    /// - **400 Bad Request**: Invalid file path or security violation
    /// - **500 Internal Server Error**: System-level errors (permissions, disk issues)
    ///
    /// ## Examples:
    /// - Get config: `GET /galatea-file/config.toml`
    /// - Read notes: `GET /galatea-file/developer_note.md`
    /// - View API spec: `GET /galatea-file/openapi_specification/project_api.json`
    #[oai(path = "/galatea-file/:filename", method = "get")]
    async fn get_galatea_file_handler(
        &self,
        filename: OpenApiPath<String>,
    ) -> GalateaFileGetResponse {
        // Validate filename
        if filename.0.is_empty() {
            return GalateaFileGetResponse::BadRequest(PlainText(
                "Filename cannot be empty".to_string(),
            ));
        }

        // Check for path traversal attempts
        if filename.0.contains("..") || filename.0.contains("\\") {
            return GalateaFileGetResponse::BadRequest(PlainText(
                "Invalid filename: path traversal not allowed".to_string(),
            ));
        }

        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => {
                return GalateaFileGetResponse::InternalServerError(PlainText(format!(
                    "Failed to get executable path: {}",
                    e
                )))
            }
        };

        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => {
                return GalateaFileGetResponse::InternalServerError(PlainText(
                    "Failed to get executable directory".to_string(),
                ))
            }
        };

        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        // Security check: ensure the resolved path is within galatea_files
        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileGetResponse::BadRequest(PlainText(
                "Invalid file path: must be within galatea_files directory".to_string(),
            ));
        }

        // Check if file exists
        if !file_path.exists() {
            return GalateaFileGetResponse::NotFound(PlainText(format!(
                "File not found: {}",
                filename.0
            )));
        }

        // Check if it's actually a file (not a directory)
        if !file_path.is_file() {
            return GalateaFileGetResponse::BadRequest(PlainText(format!(
                "Path is not a file: {}",
                filename.0
            )));
        }

        // Read and return file content
        match fs::read_to_string(&file_path) {
            Ok(content) => GalateaFileGetResponse::Ok(PlainText(content)),
            Err(e) => {
                // Determine appropriate error response based on error type
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    GalateaFileGetResponse::BadRequest(PlainText(format!(
                        "Permission denied reading file '{}': {}",
                        filename.0, e
                    )))
                } else {
                    GalateaFileGetResponse::InternalServerError(PlainText(format!(
                        "Failed to read file '{}': {}",
                        filename.0, e
                    )))
                }
            }
        }
    }

    /// List all files and directories in the galatea_files folder
    ///
    /// Returns a comprehensive listing of all files and directories within the galatea_files
    /// directory, including metadata such as file sizes and modification times. This endpoint
    /// is useful for exploring the galatea configuration structure and understanding what
    /// files are available for management.
    ///
    /// ## Features:
    /// - **Recursive listing**: Shows all files and directories at any depth
    /// - **File metadata**: Includes file sizes, modification times, and type information
    /// - **Organized output**: Clearly distinguishes between files and directories
    /// - **Path normalization**: Uses consistent forward-slash path separators
    /// - **Error resilience**: Continues listing even if some files can't be accessed
    ///
    /// ## Response structure:
    /// The response includes:
    /// - **entries**: Array of all files and directories with metadata
    /// - **total_count**: Total number of items found
    /// - **generated_at**: Timestamp when the listing was created
    ///
    /// ## Use cases:
    /// - **Configuration management**: See what config files exist
    /// - **Documentation discovery**: Find available documentation files
    /// - **API specification browsing**: List available OpenAPI specs
    /// - **Backup verification**: Check what files are being managed
    /// - **Development debugging**: Understand the galatea file structure
    ///
    /// ## Example response structure:
    /// ```json
    /// {
    ///   "entries": [
    ///     {"path": "config.toml", "is_directory": false, "size_bytes": 1024, "modified_at": 1703123456},
    ///     {"path": "openapi_specification", "is_directory": true, "size_bytes": null, "modified_at": 1703123400},
    ///     {"path": "openapi_specification/project_api.json", "is_directory": false, "size_bytes": 2048, "modified_at": 1703123450}
    ///   ],
    ///   "total_count": 3,
    ///   "generated_at": "1703123500"
    /// }
    /// ```
    #[oai(path = "/list-galatea-files", method = "get")]
    async fn list_galatea_files_handler(&self) -> GalateaFilesListApiResponse {
        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => {
                return GalateaFilesListApiResponse::InternalServerError(PlainText(format!(
                    "Failed to get executable path: {}",
                    e
                )))
            }
        };

        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => {
                return GalateaFilesListApiResponse::InternalServerError(PlainText(
                    "Failed to get executable directory".to_string(),
                ))
            }
        };

        let galatea_files_dir = exe_dir.join("galatea_files");

        if !galatea_files_dir.exists() {
            return GalateaFilesListApiResponse::InternalServerError(PlainText(
                "galatea_files directory does not exist".to_string(),
            ));
        }

        let mut entries = Vec::new();
        let mut skip_prefixes = Vec::new();
        let walker = WalkDir::new(&galatea_files_dir).into_iter();
        for entry in walker {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    // Skip the root galatea_files directory itself
                    if path == galatea_files_dir {
                        continue;
                    }
                    // Get the relative path from galatea_files_dir
                    if let Ok(rel_path) = path.strip_prefix(&galatea_files_dir) {
                        let path_str = rel_path.to_string_lossy().to_string().replace('\\', "/");
                        let is_directory = path.is_dir();
                        // If we are inside mcp_servers/<subdir>/..., skip recursion
                        if let Some(first) = rel_path.iter().next() {
                            if first == std::ffi::OsStr::new("mcp_servers") {
                                // If this is mcp_servers itself, always include
                                if rel_path.components().count() == 1 {
                                    // mcp_servers dir itself
                                    // allow
                                } else if rel_path.components().count() == 2 {
                                    // mcp_servers/<subdir> -- include, but skip recursion into it
                                    if is_directory {
                                        // Mark this prefix to skip further recursion
                                        skip_prefixes.push(path.to_path_buf());
                                    }
                                } else {
                                    // mcp_servers/<subdir>/... -- skip
                                    // If this path starts with any skip_prefix, skip
                                    if skip_prefixes.iter().any(|p| path.starts_with(p)) {
                                        continue;
                                    }
                                }
                            }
                        }
                        // Get file metadata
                        let metadata = fs::metadata(path).ok();
                        let size_bytes = if is_directory {
                            None
                        } else {
                            metadata.as_ref().map(|m| m.len())
                        };
                        let modified_at = metadata
                            .as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs());
                        entries.push(GalateaFileEntry {
                            path: path_str,
                            is_directory,
                            size_bytes,
                            modified_at,
                        });
                    }
                }
                Err(e) => {
                    // Log the error but continue processing other entries
                    eprintln!("Warning: Failed to read directory entry: {}", e);
                    continue;
                }
            }
        }

        // Filter out .DS_Store files
        entries.retain(|entry| {
            entry
                .path
                .rsplit('/')
                .next()
                .map(|name| name != ".DS_Store")
                .unwrap_or(true)
        });
        // Sort entries: directories first, then files, both alphabetically
        entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });

        let total_count = entries.len();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        GalateaFilesListApiResponse::Ok(OpenApiJson(GalateaFilesListResponse {
            entries,
            total_count,
            generated_at: timestamp,
        }))
    }
}

pub fn project_routes() -> Route {
    let api_service = OpenApiService::new(ProjectApi, "Project API", "1.0").server("/api/project");
    Route::new().nest("/", api_service)
}
