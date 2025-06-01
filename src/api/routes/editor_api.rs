use poem::Route;
use poem_openapi::{payload::{Json as OpenApiJson, PlainText}, OpenApi, Object, ApiResponse, OpenApiService};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::dev_operation::editor::{self, EditorOperationResult, SHARED_EDITOR};
use crate::file_system; // For resolve_path
use crate::file_system::paths::get_project_root;

// Define an API struct
pub struct EditorApi;

#[derive(Object, serde::Deserialize)]
struct EditorCommandRequest {
    #[oai(validator(min_length = 1))]
    command: String,
    path: Option<String>,
    paths: Option<Vec<String>>,
    file_text: Option<String>,
    insert_line: Option<usize>,
    new_str: Option<String>,
    old_str: Option<String>,
    view_range: Option<Vec<i32>>,
}

#[derive(Object, serde::Serialize, Clone)]
struct EditorFileViewResponse {
    path: String,
    content: Option<String>,
    error: Option<String>,
    line_count: Option<usize>,
}

#[derive(Object, serde::Serialize)]
struct EditorCommandResponse {
    success: bool,
    message: Option<String>,
    content: Option<String>,
    file_path: Option<String>,
    line_count: Option<usize>,
    multi_content: Option<Vec<EditorFileViewResponse>>,
    operation: Option<String>,
    modified_at: Option<String>,
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
    #[oai(path = "/health", method = "get")]
    async fn editor_health(&self) -> HealthResponse {
        HealthResponse::Ok(PlainText("Editor API route is healthy".to_string()))
    }

    #[oai(path = "/command", method = "post")]
    async fn editor_command_handler(
        &self,
        req: OpenApiJson<EditorCommandRequest>,
    ) -> EditorCommandApiResponse {
        let command_type = match req.0.command.as_str() {
            "view" => editor::CommandType::View,
            "create" => editor::CommandType::Create,
            "str_replace" => editor::CommandType::StrReplace,
            "insert" => editor::CommandType::Insert,
            "undo_edit" => editor::CommandType::UndoEdit,
            _ => {
                return EditorCommandApiResponse::BadRequest(
                    PlainText(format!("Invalid command type: {}", req.0.command)),
                );
            }
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
                            operation: Some(req.0.command.clone()),
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
                            operation: Some(req.0.command.clone()),
                            modified_at: Some(timestamp),
                            line_count: None,
                            multi_content: None,
                            modified_lines: None,
                        };
                        
                        // If it was a mutating command, try to view the file to get its new content and line count
                        if req.0.command == "create" || req.0.command == "str_replace" || req.0.command == "insert" || req.0.command == "undo_edit" {
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
                                    if req.0.command == "str_replace" && req.0.old_str.is_some() {
                                        if let Some(old_str_val) = &req.0.old_str {
                                            let line_c = old_str_val.lines().count();
                                            if line_c > 0 && line_c < 100 {
                                                response.modified_lines = Some((1..=line_c).collect());
                                            }
                                        }
                                    }
                                    if req.0.command == "insert" && req.0.insert_line.is_some() {
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
                            operation: Some(req.0.command.clone()),
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