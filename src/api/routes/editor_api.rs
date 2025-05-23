use poem::{Route, get, handler, post, web::{Json, Data}, http::StatusCode, Error as PoemError};
use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::api::models::{EditorCommandRequest, EditorCommandResponse, EditorFileViewResponse};
use crate::dev_operation::editor::{self, Editor, EditorOperationResult, MultiFileViewOutput};
use crate::file_system; // For resolve_path

#[handler]
async fn editor_api_health() -> &'static str {
    "Editor API route is healthy"
}

#[handler]
async fn editor_command_api_handler(
    editor_data: Data<&Arc<Mutex<Editor>>>,
    Json(req): Json<EditorCommandRequest>,
) -> Result<Json<EditorCommandResponse>, PoemError> {
    let command_type = match req.command.as_str() {
        "view" => editor::CommandType::View,
        "create" => editor::CommandType::Create,
        "str_replace" => editor::CommandType::StrReplace,
        "insert" => editor::CommandType::Insert,
        "undo_edit" => editor::CommandType::UndoEdit,
        _ => {
            return Err(PoemError::from_string(
                format!("Invalid command type: {}", req.command),
                StatusCode::BAD_REQUEST,
            ))
        }
    };

    // Path validation for non-view commands
    if command_type != editor::CommandType::View && req.path.is_none() {
        return Err(PoemError::from_string(
            format!("'path' is required for command type '{}'", req.command),
            StatusCode::BAD_REQUEST,
        ));
    }
    
    // Path validation for view command
    if command_type == editor::CommandType::View && req.path.is_none() && req.paths.is_none() {
        return Err(PoemError::from_string(
            "For 'view' command, either 'path' or 'paths' must be provided.".to_string(),
            StatusCode::BAD_REQUEST,
        ));
    }
    if command_type == editor::CommandType::View && req.path.is_some() && req.paths.is_some() {
        return Err(PoemError::from_string(
            "For 'view' command, provide either 'path' or 'paths', not both.".to_string(),
            StatusCode::BAD_REQUEST,
        ));
    }
    if command_type == editor::CommandType::View && req.paths.as_ref().map_or(false, |p| p.is_empty()) {
        return Err(PoemError::from_string(
            "For 'view' command with 'paths', the list cannot be empty.".to_string(),
            StatusCode::BAD_REQUEST,
        ));
    }

    // Resolve path(s) and check existence for non-create/undo commands
    let mut resolved_single_path: Option<PathBuf> = None;
    let mut resolved_multiple_paths: Option<Vec<PathBuf>> = None;

    if command_type != editor::CommandType::Create && command_type != editor::CommandType::UndoEdit {
        if let Some(p_str) = &req.path {
            let resolved_p = file_system::resolve_path(p_str)
                .map_err(|e| PoemError::from_string(e.to_string(), StatusCode::BAD_REQUEST))?;
            if !resolved_p.exists() {
                return Err(PoemError::from_string(
                    format!("File not found at resolved path: {}", resolved_p.display()),
                    StatusCode::NOT_FOUND,
                ));
            }
            resolved_single_path = Some(resolved_p);
        } else if let Some(p_strs) = &req.paths {
            let mut temp_resolved_paths = Vec::new();
            for p_str in p_strs {
                let resolved_p = file_system::resolve_path(p_str)
                    .map_err(|e| PoemError::from_string(e.to_string(), StatusCode::BAD_REQUEST))?;
                if !resolved_p.exists() {
                     return Err(PoemError::from_string(
                        format!("File not found at resolved path: {}", resolved_p.display()),
                        StatusCode::NOT_FOUND,
                    ));
                }
                temp_resolved_paths.push(resolved_p);
            }
            resolved_multiple_paths = Some(temp_resolved_paths);
        }
    } else if command_type == editor::CommandType::Create {
        // For create, path is needed but doesn't need to exist yet.
        // It could be single path or, if extended in future, multiple. Here, only single `path` for create.
        if let Some(p_str) = &req.path {
             resolved_single_path = Some(file_system::resolve_path(p_str)
                .map_err(|e| PoemError::from_string(e.to_string(), StatusCode::BAD_REQUEST))?);
        } else {
            // This case should be caught by earlier validation for create requiring `path`
            return Err(PoemError::from_string("'path' is required for create.", StatusCode::BAD_REQUEST));
        }
    } else if command_type == editor::CommandType::UndoEdit {
        // Undo might operate on a path stored in the editor, but API may still provide it for consistency or future use.
        // For now, if `req.path` is provided for undo, we can resolve it, but it's not strictly used by `undo_last_edit`.
        if let Some(p_str) = &req.path {
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

    let editor_args = editor::EditorArgs {
        command: command_type.clone(),
        path: editor_args_path.clone(), // Use resolved path if available
        paths: editor_args_paths, // Use resolved paths if available
        file_text: req.file_text.clone(),
        insert_line: req.insert_line,
        new_str: req.new_str.clone(),
        old_str: req.old_str.clone(),
        view_range: req.view_range.clone(),
    };

    let mut editor_guard = editor_data.0.lock().await;
    match editor::handle_command(&mut editor_guard, editor_args) {
        Ok(editor_result) => {
            match editor_result {
                EditorOperationResult::Single(Some(content)) => {
                    Ok(Json(EditorCommandResponse {
                        success: true,
                        message: Some(format!("Command '{}' executed successfully.", req.command)),
                        content: Some(content.clone()),
                        file_path: editor_args_path, // path from editor_args
                        operation: Some(req.command.clone()),
                        line_count: Some(content.lines().count()),
                        modified_at: Some(timestamp),
                        multi_content: None,
                        modified_lines: None, // TODO: Populate if applicable
                    }))
                }
                EditorOperationResult::Single(None) => { // For create, insert, replace, undo
                    let mut response = EditorCommandResponse {
                        success: true,
                        message: Some(format!("Command '{}' executed successfully.", req.command)),
                        content: None,
                        file_path: editor_args_path.clone(),
                        operation: Some(req.command.clone()),
                        modified_at: Some(timestamp),
                        line_count: None, 
                        multi_content: None,
                        modified_lines: None, 
                    };
                     // If it was a mutating command, try to view the file to get its new content and line count
                    if req.command == "create" || req.command == "str_replace" || req.command == "insert" || req.command == "undo_edit" {
                        if let Some(ref p) = editor_args_path { // Ensure path is available
                            let view_args = editor::EditorArgs {
                                command: editor::CommandType::View,
                                path: Some(p.clone()),
                                paths: None,
                                file_text: None, insert_line: None, new_str: None, old_str: None, view_range: None,
                            };
                            if let Ok(EditorOperationResult::Single(Some(updated_content))) = editor::handle_command(&mut editor_guard, view_args) {
                                response.content = Some(updated_content.clone());
                                response.line_count = Some(updated_content.lines().count());
                                if req.command == "str_replace" && req.old_str.is_some() {
                                    if let Some(old_str_val) = &req.old_str {
                                        let line_c = old_str_val.lines().count();
                                        if line_c > 0 && line_c < 100 { 
                                            response.modified_lines = Some((1..=line_c).collect());
                                        }
                                    }
                                }
                                if req.command == "insert" && req.insert_line.is_some() {
                                    response.modified_lines = Some(vec![req.insert_line.unwrap()]);
                                }
                            }
                        }
                    }
                    Ok(Json(response))
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
                    Ok(Json(EditorCommandResponse {
                        success: true,
                        message: Some(format!("Command '{}' (multi-file) executed successfully.", req.command)),
                        multi_content: Some(api_multi_content),
                        operation: Some(req.command.clone()),
                        modified_at: Some(timestamp),
                        content: None, file_path: None, line_count: None, modified_lines: None, // Not for multi
                    }))
                }
            }
        },
        Err(e) => Err(PoemError::from_string(e.to_string(),StatusCode::BAD_REQUEST)),
    }
}

pub fn editor_routes() -> Route {
    Route::new()
        .at("/health", get(editor_api_health))
        .at("/command", post(editor_command_api_handler))
} 