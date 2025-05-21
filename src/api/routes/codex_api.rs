use poem::{
    error::{InternalServerError, NotFoundError},
    handler,
    http::StatusCode,
    post,
    get,
    web::{Data, Json, Path},
    IntoResponse,
    Response,
    Result,
    Route,
};
use serde::{Deserialize, Serialize};
// use serde_json::Value; // Removed: No longer needed for raw output
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use uuid::Uuid;
use dashmap::DashMap;

use crate::file_system;

// New struct for the request body
#[derive(Deserialize, Debug, Clone)]
struct CodexQueryRequest {
    query_text: String,
}

// Define the new response structure
#[derive(Serialize, Debug, Default, Clone)]
pub struct CodexApiResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    assistant_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_codex_output: Option<String>,
}

#[derive(Serialize, Debug)]
struct CodexSubmitResponse {
    task_id: String,
}

#[derive(Serialize, Debug, Clone)]
#[serde(tag = "status", content = "details")]
pub enum CodexTaskStatus {
    Pending { query_text: String, #[serde(skip)] last_updated: Instant },
    Processing { query_text: String, #[serde(skip)] last_updated: Instant },
    Completed { query_text: String, response: CodexApiResponse, #[serde(skip)] last_updated: Instant },
    Failed { query_text: String, error: String, #[serde(skip)] last_updated: Instant },
}

#[derive(Serialize, Debug)]
struct CodexStatusResponse {
    task_id: String,
    task_status: CodexTaskStatus,
}

#[derive(Debug)]
struct SimpleError(String);

impl std::fmt::Display for SimpleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SimpleError {}

// Removed try_pretty_print_json_string helper function as it's no longer needed.

async fn run_codex_command_logic(query_text: String) -> Result<CodexApiResponse, String> {
    let project_root_path = file_system::get_project_root().map_err(|e| {
        let err_msg = format!("Failed to determine project root for codex command: {}", e);
        eprintln!("{}", err_msg);
        err_msg
    })?;

    let mut cmd = Command::new("codex");
    cmd.arg("-q");
    cmd.arg(&query_text);
    cmd.current_dir(&project_root_path);

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut process = cmd.spawn().map_err(|e| {
        let err_msg = format!("Failed to start codex process: {}", e);
        eprintln!("{}", err_msg);
        err_msg
    })?;

    let mut stdout_str = String::new();
    if let Some(mut stdout) = process.stdout.take() {
        stdout.read_to_string(&mut stdout_str).await.map_err(|e| {
            let err_msg = format!("Failed to read codex stdout: {}", e);
            eprintln!("{}", err_msg);
            err_msg
        })?;
    } else {
        let err_msg = "Failed to capture codex stdout".to_string();
        eprintln!("{}", err_msg);
        return Err(err_msg);
    }

    let mut stderr_str = String::new();
    if let Some(mut stderr) = process.stderr.take() {
        if let Err(e) = stderr.read_to_string(&mut stderr_str).await {
            eprintln!("Failed to read codex stderr: {}", e);
        }
    }

    let status = process.wait().await.map_err(|e| {
        let err_msg = format!("Failed to wait for codex process: {}", e);
        eprintln!("{}", err_msg);
        err_msg
    })?;

    if !status.success() && !stderr_str.is_empty() {
        let err_msg = format!("Codex process error: {}", stderr_str);
        eprintln!("Codex process failed. Stderr: {}", stderr_str);
        return Err(err_msg);
    }

    if !stderr_str.is_empty() {
        println!("Codex stderr (non-fatal for task, but logged): {}", stderr_str);
    }

    if !stdout_str.is_empty() {
        Ok(CodexApiResponse {
            raw_codex_output: Some(stdout_str),
            ..Default::default()
        })
    } else {
        let err_msg = format!(
            "Codex command for query \"{}\" produced no stdout. Process success: {}. Stderr: '{}'",
            query_text, status.success(), stderr_str
        );
        eprintln!("{}", err_msg);
        Err(err_msg)
    }
}

#[handler]
async fn submit_codex_task_handler(
    query: Json<CodexQueryRequest>,
    tasks: Data<&Arc<DashMap<String, CodexTaskStatus>>>
) -> Result<impl IntoResponse> {
    let task_id = Uuid::new_v4().to_string();
    let query_text = query.0.query_text;

    tasks.insert(task_id.clone(), CodexTaskStatus::Pending { query_text: query_text.clone(), last_updated: Instant::now() });
    println!("Task {} submitted for query: \"{}\"", task_id, query_text);

    let tasks_clone: Arc<DashMap<String, CodexTaskStatus>> = Arc::clone(tasks.0);
    let task_id_clone = task_id.clone();
    let query_text_clone_for_task = query_text.clone();
    
    tokio::spawn(async move {
        let task_start_time = Instant::now();
        println!("Task {} (query: \"{}\") processing started...", task_id_clone, query_text_clone_for_task);
        tasks_clone.insert(task_id_clone.clone(), CodexTaskStatus::Processing { query_text: query_text_clone_for_task.clone(), last_updated: Instant::now() });

        match run_codex_command_logic(query_text_clone_for_task.clone()).await {
            Ok(response) => {
                tasks_clone.insert(task_id_clone.clone(), CodexTaskStatus::Completed { query_text: query_text_clone_for_task.clone(), response, last_updated: Instant::now() });
                let duration_ms = task_start_time.elapsed().as_secs_f64() * 1000.0;
                println!("Task {} (query: \"{}\") completed successfully in {:.2}ms.", task_id_clone, query_text_clone_for_task, duration_ms);
            }
            Err(error_message) => {
                tasks_clone.insert(task_id_clone.clone(), CodexTaskStatus::Failed { query_text: query_text_clone_for_task.clone(), error: error_message.clone(), last_updated: Instant::now() });
                let duration_ms = task_start_time.elapsed().as_secs_f64() * 1000.0;
                eprintln!("Task {} (query: \"{}\") failed after {:.2}ms: {}", task_id_clone, query_text_clone_for_task, duration_ms, error_message);
            }
        }
    });

    Ok((StatusCode::ACCEPTED, Json(CodexSubmitResponse { task_id })))
}

#[handler]
async fn get_codex_task_status_handler(
    task_id_param: Path<String>,
    tasks: Data<&Arc<DashMap<String, CodexTaskStatus>>>
) -> Result<impl IntoResponse> {
    let task_id = task_id_param.0;
    match tasks.get(&task_id) {
        Some(task_ref) => {
            let task_status_cloned = task_ref.value().clone();
            let response = Json(CodexStatusResponse {
                task_id: task_id.clone(),
                task_status: task_status_cloned.clone(),
            });

            match task_status_cloned {
                CodexTaskStatus::Completed { .. } | CodexTaskStatus::Failed { .. } => {
                    tasks.remove(&task_id);
                    println!("Task {} removed after query (status was Completed/Failed).", task_id);
                }
                _ => {}
            }
            Ok(response)
        }
        None => Err(NotFoundError.into()),
    }
}

pub fn codex_routes() -> Route {
    Route::new()
        .at("/submit", post(submit_codex_task_handler))
        .at("/status/:task_id", get(get_codex_task_status_handler))
}

// --- Memory Management Utilities ---

const TASK_MAX_LIFETIME_SECONDS: u64 = 3600; // 1 hour

// This function can be called by a background task in main.rs
pub fn cleanup_old_tasks(tasks: &Arc<DashMap<String, CodexTaskStatus>>) {
    let mut tasks_to_remove = Vec::new();
    let now = Instant::now();

    // Iterate to find tasks to remove. We collect IDs to avoid modifying the map while iterating.
    for entry in tasks.iter() {
        let task_id = entry.key();
        let status = entry.value();

        let task_last_updated = match status {
            CodexTaskStatus::Pending { last_updated, .. } => last_updated,
            CodexTaskStatus::Processing { last_updated, .. } => last_updated,
            CodexTaskStatus::Completed { last_updated, .. } => last_updated,
            CodexTaskStatus::Failed { last_updated, .. } => last_updated,
        };

        if now.duration_since(*task_last_updated).as_secs() > TASK_MAX_LIFETIME_SECONDS {
            tasks_to_remove.push(task_id.clone());
        }
    }

    // Remove the identified tasks
    for task_id in tasks_to_remove {
        if tasks.remove(&task_id).is_some() {
            println!("Task {} removed by TTL cleanup (older than 1 hour).", task_id);
        }
    }
}
