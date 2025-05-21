pub mod search;
pub mod paths; // Added paths module
// pub mod operations; // For future file read/write utilities

// Re-export common functions for convenience
pub use search::{find_file_by_suffix, find_files_by_extensions};
pub use paths::{get_project_root, resolve_path, resolve_path_to_uri}; // Re-export path functions 