pub mod models;
pub mod routes;

use poem::{Route, get};

// Health check endpoint for the API module itself
#[poem::handler]
async fn health() -> &'static str {
    "Galatea API module is running"
}

pub fn api_routes() -> Route {
    Route::new()
        .nest("/", routes::all_routes()) // Mount all other routes under /api (handled by main)
        .at("/health", get(health)) // Add a health check for the /api route itself
} 