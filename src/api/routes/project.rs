use poem::{Route, get, handler, post, web::Json, http::StatusCode, Error as PoemError};
use crate::api::models::{FindFilesRequest, FindFilesResponse};
use crate::file_system; // For find_files_by_extensions - will be file_system::search
use tokio::process::Command; // Re-add for direct command execution
use crate::file_system::paths::{get_project_root, resolve_path}; // Add resolve_path import
// crate::terminal::pnpm::run_pnpm_command could be used here instead of direct Command
// crate::file_system::paths::get_project_root is no longer needed here for these handlers

#[derive(serde::Serialize)]
pub struct ScriptResponse {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

#[poem::handler]
async fn project_health() -> &'static str {
    "Project API route is healthy"
}

#[handler]
async fn find_files_handler(
    Json(req): Json<FindFilesRequest>,
) -> Result<Json<FindFilesResponse>, PoemError> {
    // Resolve the directory path using resolve_path
    let dir = match resolve_path(&req.dir) {
        Ok(path) => path,
        Err(e) => {
            return Err(PoemError::from_string(
                format!("Failed to resolve directory: {}", e),
                StatusCode::BAD_REQUEST,
            ));
        }
    };
    let suffixes_ref: Vec<&str> = req.suffixes.iter().map(|s| s.as_str()).collect();
    let exclude_dirs = req.exclude_dirs.unwrap_or_else(|| {
        vec![
            "node_modules".to_string(),
            "target".to_string(),
            "dist".to_string(),
            "build".to_string(),
            ".git".to_string(),
            ".vscode".to_string(),
            ".idea".to_string(),
        ]
    });
    let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();
    
    // Corrected path to find_files_by_extensions
    match file_system::search::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(found_files) => {
            let file_paths = found_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            Ok(Json(FindFilesResponse { files: file_paths }))
        }
        Err(e) => Err(PoemError::from_string(
            format!("Error searching directory: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

#[handler]
async fn lint_handler() -> Result<Json<ScriptResponse>, PoemError> {
    let project_root = get_project_root().map_err(|e| 
        PoemError::from_string(format!("Failed to get project root: {}", e), StatusCode::INTERNAL_SERVER_ERROR)
    )?;

    let mut cmd = Command::new("pnpm");
    cmd.current_dir(&project_root) // Set current directory to project root
       .arg("run")
       .arg("lint");
    
    let output = cmd.output().await.map_err(|e| 
        PoemError::from_string(format!("Failed to execute pnpm lint: {}", e), StatusCode::INTERNAL_SERVER_ERROR)
    )?;

    Ok(Json(ScriptResponse {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status: output.status.code().unwrap_or(-1),
    }))
}

#[handler]
async fn format_handler() -> Result<Json<ScriptResponse>, PoemError> {
    let project_root = get_project_root().map_err(|e| 
        PoemError::from_string(format!("Failed to get project root: {}", e), StatusCode::INTERNAL_SERVER_ERROR)
    )?;

    let mut cmd = Command::new("pnpm");
    cmd.current_dir(&project_root) // Set current directory to project root
       .arg("run")
       .arg("format");

    let output = cmd.output().await.map_err(|e| 
        PoemError::from_string(format!("Failed to execute pnpm format: {}", e), StatusCode::INTERNAL_SERVER_ERROR)
    )?;

    Ok(Json(ScriptResponse {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status: output.status.code().unwrap_or(-1),
    }))
}

pub fn project_routes() -> Route {
    Route::new()
        .at("/health", get(project_health))
        .at("/find-files", post(find_files_handler))
        .at("/lint", post(lint_handler))
        .at("/format", post(format_handler))
    // Define other project-related routes here
} 