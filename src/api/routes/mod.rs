use poem::Route;

pub mod code_intel;
pub mod editor_api;
pub mod logs_api;
pub mod lsp_api;
pub mod project;
pub mod codex_api;

pub fn all_routes() -> Route {
    Route::new()
        .nest("/project", project::project_routes())
        .nest("/code-intel", code_intel::code_intel_routes())
        .nest("/editor", editor_api::editor_routes())
        .nest("/logs", logs_api::logs_routes())
        .nest("/lsp", lsp_api::lsp_routes())
        .nest("/codex", codex_api::codex_routes())
} 