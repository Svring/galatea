use poem::{Route, get, handler, post, web::{Json, Data}, http::StatusCode, Error as PoemError};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::api::models::{EditorCommandRequest, EditorCommandResponse};
use crate::dev_operation::editor::{self, Editor}; // Assuming editor moved under dev_operation or is directly accessible
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

    let resolved_req_path = match file_system::resolve_path(&req.path) {
        Ok(p) => p,
        Err(e) => return Err(PoemError::from_string(e.to_string(), StatusCode::BAD_REQUEST)),
    };

    if command_type != editor::CommandType::Create && 
       command_type != editor::CommandType::UndoEdit && 
       !resolved_req_path.exists() {
        return Err(PoemError::from_string(
            format!("File not found at resolved path: {}", resolved_req_path.display()),
            StatusCode::NOT_FOUND,
        ));
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    let editor_args = editor::EditorArgs {
        command: command_type,
        path: resolved_req_path.to_string_lossy().into_owned(),
        file_text: req.file_text.clone(),
        insert_line: req.insert_line,
        new_str: req.new_str.clone(),
        old_str: req.old_str.clone(),
        view_range: req.view_range.clone(),
    };

    let mut editor_guard = editor_data.0.lock().await;
    match editor::handle_command(&mut editor_guard, editor_args) {
        Ok(Some(content)) => {
            Ok(Json(EditorCommandResponse {
                success: true,
                message: Some(format!("Command '{}' executed successfully.", req.command)),
                content: Some(content.clone()),
                file_path: Some(resolved_req_path.to_string_lossy().into_owned()),
                operation: Some(req.command.clone()),
                line_count: Some(content.lines().count()),
                modified_at: Some(timestamp),
                modified_lines: None,
            }))
        },
        Ok(None) => {
            let mut response = EditorCommandResponse {
                success: true,
                message: Some(format!("Command '{}' executed successfully.", req.command)),
                content: None,
                file_path: Some(resolved_req_path.to_string_lossy().into_owned()),
                operation: Some(req.command.clone()),
                modified_at: Some(timestamp),
                line_count: Some(0), 
                modified_lines: None,
            };
            
            if req.command == "create" || req.command == "str_replace" || req.command == "insert" || req.command == "undo_edit" {
                let view_args = editor::EditorArgs {
                    command: editor::CommandType::View,
                    path: resolved_req_path.to_string_lossy().into_owned(),
                    file_text: None, insert_line: None, new_str: None, old_str: None, view_range: None,
                };
                if let Ok(Some(updated_content)) = editor::handle_command(&mut editor_guard, view_args) {
                    response.content = Some(updated_content.clone());
                    response.line_count = Some(updated_content.lines().count());
                    if req.command == "str_replace" && req.old_str.is_some() {
                        if let Some(old_str) = &req.old_str {
                            let line_count = old_str.lines().count();
                            if line_count > 0 && line_count < 100 { 
                                response.modified_lines = Some((1..=line_count).collect());
                            }
                        }
                    }
                    if req.command == "insert" && req.insert_line.is_some() {
                        response.modified_lines = Some(vec![req.insert_line.unwrap()]);
                    }
                }
            }
            Ok(Json(response))
        },
        Err(e) => Err(PoemError::from_string(e.to_string(),StatusCode::BAD_REQUEST)),
    }
}

pub fn editor_routes() -> Route {
    Route::new()
        .at("/health", get(editor_api_health))
        .at("/command", post(editor_command_api_handler))
} 