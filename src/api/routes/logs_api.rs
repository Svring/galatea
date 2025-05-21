use poem::{Route, get, handler, post, web::Json, http::StatusCode, Error as PoemError};
use crate::api::models::{GetLogsRequest, GetLogsResponse, ClearLogsResponse};
use crate::dev_runtime::log::{get_shared_logs, clear_shared_logs, LogFilterOptions};

#[poem::handler]
async fn logs_api_health() -> &'static str {
    "Logs API route is healthy"
}

#[handler]
async fn get_logs_api_handler(
    Json(req): Json<GetLogsRequest>,
) -> Result<Json<GetLogsResponse>, PoemError> {
    let filter_options = LogFilterOptions {
        sources: req.filter_options.sources,
        levels: req.filter_options.levels,
        content_contains: req.filter_options.content_contains,
        since_timestamp: req.filter_options.since_timestamp,
        until_timestamp: req.filter_options.until_timestamp,
        max_entries: req.filter_options.max_entries,
    };

    match get_shared_logs(filter_options) {
        Ok(logs) => {
            let count = logs.len();
            Ok(Json(GetLogsResponse {
                success: true,
                logs,
                count,
            }))
        }
        Err(e) => {
            eprintln!("Error getting shared logs: {:?}", e);
            Err(PoemError::from_string(
                format!("Failed to retrieve logs: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

#[handler]
async fn clear_logs_api_handler() -> Result<Json<ClearLogsResponse>, PoemError> {
    match clear_shared_logs() {
        Ok(_) => Ok(Json(ClearLogsResponse {
            success: true,
            message: "Logs cleared successfully.".to_string(),
        })),
        Err(e) => {
            eprintln!("Error clearing shared logs: {:?}", e);
            Err(PoemError::from_string(
                format!("Failed to clear logs: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    }
}

pub fn logs_routes() -> Route {
    Route::new()
        .at("/health", get(logs_api_health))
        .at("/get", post(get_logs_api_handler))
        .at("/clear", post(clear_logs_api_handler))
} 