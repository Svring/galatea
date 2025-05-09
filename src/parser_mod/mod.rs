// This file defines the public interface for the parser_mod module.

// Declare the submodules
pub mod helpers;
pub mod rust_entity_parser;
pub mod structs;
pub mod ts_entity_parser;
pub mod tsx_display_parser;

// Re-export the necessary public functions
pub use rust_entity_parser::extract_rust_entities_from_file;
pub use structs::{CodeContext, CodeEntity}; // Make structs available if needed directly
pub use tsx_display_parser::parse_and_print_tsx_file;
// Rename the TS function to avoid name clash
pub use ts_entity_parser::extract_ts_entities_from_file as extract_ts_entities;
