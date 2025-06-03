use poem::Route;
use poem_openapi::{param::Path as OpenApiPath, payload::{Json as OpenApiJson, PlainText}, OpenApi, Object, ApiResponse, OpenApiService};
use crate::file_system;
use tokio::process::Command;
use crate::file_system::paths::{get_project_root, resolve_path};
use std::fs;
use walkdir::WalkDir;

// Define an API struct
pub struct ProjectApi;

#[derive(Object, serde::Serialize)]
pub struct ScriptResponse {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

// Use poem-openapi's Object derive for request/response structs
#[derive(Object, serde::Deserialize)] 
struct ProjectFindFilesRequest {
    #[oai(validator(min_length = 1))]
    dir: String,
    suffixes: Vec<String>,
    exclude_dirs: Option<Vec<String>>,
}

#[derive(Object, serde::Serialize)]
struct ProjectFindFilesResponse {
    files: Vec<String>,
}

#[derive(Object, serde::Deserialize)]
pub struct UpdateFileRequest {
    #[oai(validator(min_length = 0))]
    pub content: String,
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
    pub entries: Vec<String>,
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
    #[oai(path = "/health", method = "get")]
    async fn project_health(&self) -> HealthResponse {
        HealthResponse::Ok(PlainText("Project API route is healthy".to_string()))
    }

    #[oai(path = "/find-files", method = "post")]
    async fn find_files_handler(
        &self,
        req: OpenApiJson<ProjectFindFilesRequest>,
    ) -> FindFilesApiServerResponse {
        let dir = match resolve_path(&req.0.dir) {
            Ok(path) => path,
            Err(e) => {
                return FindFilesApiServerResponse::BadRequest(
                    PlainText(format!("Failed to resolve directory: {}", e)),
                );
            }
        };
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
            ]
        });
        let exclude_dirs_ref: Vec<&str> = exclude_dirs.iter().map(|s| s.as_str()).collect();

        match file_system::search::find_files_by_extensions(&dir, &suffixes_ref, &exclude_dirs_ref) {
            Ok(found_files) => {
                let file_paths = found_files
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect();
                FindFilesApiServerResponse::Ok(OpenApiJson(ProjectFindFilesResponse { files: file_paths }))
            }
            Err(e) => FindFilesApiServerResponse::InternalServerError(
                PlainText(format!("Error searching directory: {}", e)),
            ),
        }
    }

    #[oai(path = "/lint", method = "post")]
    async fn lint_handler(&self) -> ScriptApiServerResponse {
        let project_root = match get_project_root() {
            Ok(pr) => pr,
            Err(e) => return ScriptApiServerResponse::InternalServerError(PlainText(format!("Failed to get project root: {}", e))),
        };

        let mut cmd = Command::new("pnpm");
        cmd.current_dir(&project_root)
           .arg("run")
           .arg("lint");

        let output = match cmd.output().await {
            Ok(out) => out,
            Err(e) => return ScriptApiServerResponse::InternalServerError(PlainText(format!("Failed to execute pnpm lint: {}", e))),
        };

        ScriptApiServerResponse::Ok(OpenApiJson(ScriptResponse {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: output.status.code().unwrap_or(-1),
        }))
    }

    #[oai(path = "/format", method = "post")]
    async fn format_handler(&self) -> ScriptApiServerResponse {
        let project_root = match get_project_root() {
            Ok(pr) => pr,
            Err(e) => return ScriptApiServerResponse::InternalServerError(PlainText(format!("Failed to get project root: {}", e))),
        };

        let mut cmd = Command::new("pnpm");
        cmd.current_dir(&project_root)
           .arg("run")
           .arg("format");

        let output = match cmd.output().await {
            Ok(out) => out,
            Err(e) => return ScriptApiServerResponse::InternalServerError(PlainText(format!("Failed to execute pnpm format: {}", e))),
        };
        
        ScriptApiServerResponse::Ok(OpenApiJson(ScriptResponse {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: output.status.code().unwrap_or(-1),
        }))
    }
    
    #[oai(path = "/galatea-file/:filename", method = "put")]
    async fn update_galatea_file_handler(
        &self,
        filename: OpenApiPath<String>,
        req: OpenApiJson<UpdateFileRequest>
    ) -> GalateaFileUpdateResponse {
        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => return GalateaFileUpdateResponse::InternalServerError(PlainText(e.to_string())),
        };
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFileUpdateResponse::InternalServerError(PlainText("Failed to get executable directory".to_string())),
        };
        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileUpdateResponse::BadRequest(PlainText("Invalid file path".to_string()));
        }

        if let Err(e) = fs::write(&file_path, &req.0.content) {
            return GalateaFileUpdateResponse::InternalServerError(PlainText(format!("Failed to write file {}: {}", filename.0, e)));
        }
        
        let action = if file_path.exists() { "updated" } else { "created" };
        GalateaFileUpdateResponse::Ok(OpenApiJson(ScriptResponse {
            success: true,
            stdout: format!("File {} {} successfully", filename.0, action),
            stderr: String::new(),
            status: 0,
        }))
    }

    #[oai(path = "/galatea-file/:filename", method = "get")]
    async fn get_galatea_file_handler(&self, filename: OpenApiPath<String>) -> GalateaFileGetResponse {
        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => return GalateaFileGetResponse::InternalServerError(PlainText(e.to_string())),
        };
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFileGetResponse::InternalServerError(PlainText("Failed to get executable directory".to_string())),
        };
        let galatea_files_dir = exe_dir.join("galatea_files");
        let file_path = galatea_files_dir.join(&filename.0);

        if !file_path.starts_with(&galatea_files_dir) {
            return GalateaFileGetResponse::BadRequest(PlainText("Invalid file path".to_string()));
        }

        match fs::read_to_string(&file_path) {
            Ok(content) => GalateaFileGetResponse::Ok(PlainText(content)),
            Err(e) => GalateaFileGetResponse::NotFound(PlainText(format!("Failed to read file {}: {}", filename.0, e))),
        }
    }

    /// List all files and folders under the galatea_files folder (recursively)
    #[oai(path = "/list-galatea-files", method = "get")]
    async fn list_galatea_files_handler(&self) -> GalateaFilesListApiResponse {
        use std::path::Path;
        let exe_path = match std::env::current_exe() {
            Ok(ep) => ep,
            Err(e) => return GalateaFilesListApiResponse::InternalServerError(PlainText(e.to_string())),
        };
        let exe_dir = match exe_path.parent() {
            Some(ed) => ed,
            None => return GalateaFilesListApiResponse::InternalServerError(PlainText("Failed to get executable directory".to_string())),
        };
        let galatea_files_dir = exe_dir.join("galatea_files");
        if !galatea_files_dir.exists() {
            return GalateaFilesListApiResponse::InternalServerError(PlainText("galatea_files directory does not exist".to_string()));
        }
        let mut entries = Vec::new();
        let walker = WalkDir::new(&galatea_files_dir).into_iter();
        for entry in walker {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    if path == galatea_files_dir {
                        continue;
                    }
                    // Get the relative path from galatea_files_dir
                    if let Ok(rel_path) = path.strip_prefix(&galatea_files_dir) {
                        entries.push(rel_path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    return GalateaFilesListApiResponse::InternalServerError(PlainText(format!("Failed to read entry: {}", e)));
                }
            }
        }
        GalateaFilesListApiResponse::Ok(OpenApiJson(GalateaFilesListResponse { entries }))
    }
}

pub fn project_routes() -> Route {
    let api_service = OpenApiService::new(ProjectApi, "Project API", "1.0")
        .server("/api/project");
    Route::new().nest("/", api_service)
} 