use poem::{Route, get, handler, post, web::{Json, Data}, http::StatusCode, Error as PoemError};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use lsp_types;

use crate::api::models::{GotoDefinitionApiRequest, GotoDefinitionApiResponse};
use crate::dev_runtime::lsp_client::LspClient;
use crate::file_system::{resolve_path, resolve_path_to_uri};

#[handler]
async fn lsp_api_health() -> &'static str {
    "LSP API route is healthy"
}

#[handler]
pub async fn lsp_goto_definition_api_handler(
    lsp_client_data: Data<&Arc<Mutex<LspClient>>>,
    Json(req): Json<GotoDefinitionApiRequest>,
) -> Result<Json<GotoDefinitionApiResponse>, PoemError> {
    let resolved_file_path = match resolve_path(&req.uri) {
        Ok(p) => p,
        Err(e) => {
            return Err(PoemError::from_string(
                format!(
                    "Failed to resolve input path/URI '{}' to a project file: {}",
                    req.uri,
                    e.to_string()
                ),
                StatusCode::BAD_REQUEST,
            ));
        }
    };

    let file_uri = match resolve_path_to_uri(&req.uri) {
        Ok(uri) => uri,
        Err(e) => {
            return Err(PoemError::from_string(
                format!("Failed to resolve input path/URI '{}' to a project file: {}", req.uri, e.to_string()),
                StatusCode::BAD_REQUEST,
            ));
        }
    };
    
    let file_content = match std::fs::read_to_string(&resolved_file_path) {
        Ok(content) => content,
        Err(e) => {
            return Err(PoemError::from_string(
                format!(
                    "Failed to read file for LSP didOpen '{}': {}",
                    resolved_file_path.display(),
                    e
                ),
                StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    };

    let position = lsp_types::Position {
        line: req.line,
        character: req.character,
    };

    let mut client_guard = lsp_client_data.0.lock().await;

    let language_id = resolved_file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or_else(
            || "plaintext".to_string(),
            |ext| match ext {
                "ts" => "typescript".to_string(),
                "tsx" => "typescriptreact".to_string(),
                "js" => "javascript".to_string(),
                "jsx" => "javascriptreact".to_string(),
                "json" => "json".to_string(),
                _ => "plaintext".to_string(),
            },
        );

    if let Err(e) = client_guard
        .notify_did_open(file_uri.clone(), &language_id, 0, file_content)
        .await
    {
        eprintln!(
            "LSP notify_did_open failed (continuing to goto_definition): {}",
            e
        );
    }

    match client_guard.goto_definition(file_uri, position).await {
        Ok(locations) => Ok(Json(GotoDefinitionApiResponse { locations })),
        Err(e) => Err(PoemError::from_string(
            format!("LSP goto_definition failed: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

pub fn lsp_routes() -> Route {
    Route::new()
        .at("/health", get(lsp_api_health))
        .at("/goto-definition", post(lsp_goto_definition_api_handler))
} 