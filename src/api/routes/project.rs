use poem::{Route, get, handler, post, web::Json, http::StatusCode};
use crate::api::models::{FindFilesRequest, FindFilesResponse};
use crate::file_system; // For find_files_by_extensions

#[poem::handler]
async fn project_health() -> &'static str {
    "Project API route is healthy"
}

#[handler]
async fn find_files_handler(
    Json(req): Json<FindFilesRequest>,
) -> Result<Json<FindFilesResponse>, poem::Error> {
    let dir = std::path::PathBuf::from(&req.dir);
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
    
    match file_system::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
        Ok(found_files) => {
            let file_paths = found_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect();
            Ok(Json(FindFilesResponse { files: file_paths }))
        }
        Err(e) => Err(poem::Error::from_string(
            format!("Error searching directory: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )),
    }
}

pub fn project_routes() -> Route {
    Route::new()
        .at("/health", get(project_health))
        .at("/find-files", post(find_files_handler))
    // Define other project-related routes here
} 