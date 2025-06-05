use poem::Route;
use poem_openapi::{param::Path as OpenApiPath, payload::{Json as OpenApiJson, PlainText}, OpenApi, Object, ApiResponse, OpenApiService, Enum};
use crate::file_system;
use tokio::process::Command;
use crate::file_system::paths::{get_project_root, resolve_path};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// Define an API struct
pub struct ProjectApi;

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

// Use poem-openapi's Object derive for request/response structs
#[derive(Object, serde::Deserialize)] 
struct ProjectFindFilesRequest {
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
struct ProjectFindFilesResponse {
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

#[derive(ApiResponse)]
enum HealthResponse {
    #[oai(status = 200)]
    Ok(PlainText<String>),
}

#[derive(ApiResponse)]
enum FindFilesApiServerResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<ProjectFindFilesResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[derive(ApiResponse)]
enum ScriptApiServerResponse {
    #[oai(status = 200)]
    Ok(OpenApiJson<ScriptResponse>),
    #[oai(status = 400)]
    BadRequest(PlainText<String>),
    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
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
        req: OpenApiJson<ProjectFindFilesRequest>,
    ) -> FindFilesApiServerResponse {
        // Validate and resolve directory path
        let dir = match resolve_path(&req.0.dir) {
            Ok(path) => path,
            Err(e) => {
                return FindFilesApiServerResponse::BadRequest(
                    PlainText(format!("Failed to resolve directory '{}': {}", req.0.dir, e)),
                );
            }
        };

        // Validate directory exists
        if !dir.exists() {
            return FindFilesApiServerResponse::BadRequest(
                PlainText(format!("Directory does not exist: {}", dir.display())),
            );
        }

        if !dir.is_dir() {
            return FindFilesApiServerResponse::BadRequest(
                PlainText(format!("Path is not a directory: {}", dir.display())),
            );
        }

        // Validate suffixes
        if req.0.suffixes.is_empty() {
            return FindFilesApiServerResponse::BadRequest(
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

                let response = ProjectFindFilesResponse {
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

                FindFilesApiServerResponse::Ok(OpenApiJson(response))
            }
            Err(e) => FindFilesApiServerResponse::InternalServerError(
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
    async fn script_handler(&self, req: OpenApiJson<ScriptExecutionRequest>) -> ScriptApiServerResponse {
        let start_time = std::time::Instant::now();
        
        // Determine working directory
        let working_dir = if let Some(ref wd) = req.0.working_dir {
            match resolve_path(wd) {
                Ok(path) => {
                    if !path.exists() || !path.is_dir() {
                        return ScriptApiServerResponse::BadRequest(
                            PlainText(format!("Working directory does not exist or is not a directory: {}", wd))
                        );
                    }
                    path
                }
                Err(e) => {
                    return ScriptApiServerResponse::BadRequest(
                        PlainText(format!("Failed to resolve working directory '{}': {}", wd, e))
                    );
                }
            }
        } else {
            match get_project_root() {
                Ok(pr) => pr,
                Err(e) => return ScriptApiServerResponse::InternalServerError(
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
            Err(e) => return ScriptApiServerResponse::InternalServerError(
                PlainText(format!("Failed to execute {} {}: {}", base_cmd, req.0.operation, e))
            ),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        ScriptApiServerResponse::Ok(OpenApiJson(ScriptResponse {
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
    async fn lint_handler(&self) -> ScriptApiServerResponse {
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
    async fn format_handler(&self) -> ScriptApiServerResponse {
        let req = ScriptExecutionRequest {
            operation: ScriptOperation::Format,
            args: None,
            working_dir: None,
            env_vars: None,
        };
        self.script_handler(OpenApiJson(req)).await
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
        req: OpenApiJson<UpdateFileRequest>
    ) -> GalateaFileUpdateResponse {
        // Validate filename
        if filename.0.is_empty() {
            return GalateaFileUpdateResponse::BadRequest(
                PlainText("Filename cannot be empty".to_string())
            );
        }

        // Check for path traversal attempts
        if filename.0.contains("..") || filename.0.contains("\\") {
            return GalateaFileUpdateResponse::BadRequest(
                PlainText("Invalid filename: path traversal not allowed".to_string())
            );
        }

        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => return GalateaFileUpdateResponse::InternalServerError(
                PlainText(format!("Failed to get executable path: {}", e))
            ),
        };
        
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFileUpdateResponse::InternalServerError(
                PlainText("Failed to get executable directory".to_string())
            ),
        };
        
        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        // Security check: ensure the resolved path is within galatea_files
        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileUpdateResponse::BadRequest(
                PlainText("Invalid file path: must be within galatea_files directory".to_string())
            );
        }

        let file_existed = file_path.exists();
        let create_dirs = req.0.create_dirs.unwrap_or(true);
        let backup_existing = req.0.backup_existing.unwrap_or(false);

        // Create parent directories if needed
        if create_dirs {
            if let Some(parent) = file_path.parent() {
                if !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        return GalateaFileUpdateResponse::InternalServerError(
                            PlainText(format!("Failed to create parent directories for '{}': {}", filename.0, e))
                        );
                    }
                }
            }
        }

        // Backup existing file if requested
        if backup_existing && file_existed {
            let backup_path = file_path.with_extension(
                format!("{}.backup", file_path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            );
            if let Err(e) = fs::copy(&file_path, &backup_path) {
                return GalateaFileUpdateResponse::InternalServerError(
                    PlainText(format!("Failed to create backup of '{}': {}", filename.0, e))
                );
            }
        }

        // Write the file
        if let Err(e) = fs::write(&file_path, &req.0.content) {
            return GalateaFileUpdateResponse::InternalServerError(
                PlainText(format!("Failed to write file '{}': {}", filename.0, e))
            );
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
    async fn get_galatea_file_handler(&self, filename: OpenApiPath<String>) -> GalateaFileGetResponse {
        // Validate filename
        if filename.0.is_empty() {
            return GalateaFileGetResponse::BadRequest(
                PlainText("Filename cannot be empty".to_string())
            );
        }

        // Check for path traversal attempts
        if filename.0.contains("..") || filename.0.contains("\\") {
            return GalateaFileGetResponse::BadRequest(
                PlainText("Invalid filename: path traversal not allowed".to_string())
            );
        }

        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => return GalateaFileGetResponse::InternalServerError(
                PlainText(format!("Failed to get executable path: {}", e))
            ),
        };
        
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFileGetResponse::InternalServerError(
                PlainText("Failed to get executable directory".to_string())
            ),
        };
        
        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        // Security check: ensure the resolved path is within galatea_files
        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileGetResponse::BadRequest(
                PlainText("Invalid file path: must be within galatea_files directory".to_string())
            );
        }

        // Check if file exists
        if !file_path.exists() {
            return GalateaFileGetResponse::NotFound(
                PlainText(format!("File not found: {}", filename.0))
            );
        }

        // Check if it's actually a file (not a directory)
        if !file_path.is_file() {
            return GalateaFileGetResponse::BadRequest(
                PlainText(format!("Path is not a file: {}", filename.0))
            );
        }

        // Read and return file content
        match fs::read_to_string(&file_path) {
            Ok(content) => GalateaFileGetResponse::Ok(PlainText(content)),
            Err(e) => {
                // Determine appropriate error response based on error type
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    GalateaFileGetResponse::BadRequest(
                        PlainText(format!("Permission denied reading file '{}': {}", filename.0, e))
                    )
                } else {
                    GalateaFileGetResponse::InternalServerError(
                        PlainText(format!("Failed to read file '{}': {}", filename.0, e))
                    )
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
            Err(e) => return GalateaFilesListApiResponse::InternalServerError(
                PlainText(format!("Failed to get executable path: {}", e))
            ),
        };
        
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFilesListApiResponse::InternalServerError(
                PlainText("Failed to get executable directory".to_string())
            ),
        };
        
        let galatea_files_dir = exe_dir.join("galatea_files");
        
        if !galatea_files_dir.exists() {
            return GalateaFilesListApiResponse::InternalServerError(
                PlainText("galatea_files directory does not exist".to_string())
            );
        }

        let mut entries = Vec::new();
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
                        
                        // Get file metadata
                        let metadata = fs::metadata(path).ok();
                        let size_bytes = if is_directory {
                            None
                        } else {
                            metadata.as_ref().map(|m| m.len())
                        };
                        
                        let modified_at = metadata.as_ref()
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

        // Sort entries: directories first, then files, both alphabetically
        entries.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.path.cmp(&b.path),
            }
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
    let api_service = OpenApiService::new(ProjectApi, "Project API", "1.0")
        .server("/api/project");
    Route::new().nest("/", api_service)
} 