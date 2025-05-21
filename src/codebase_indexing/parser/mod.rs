// This file defines the public interface for the codebase_indexing::parser module.

// Declare the submodules
pub mod entities; // Renamed from structs
pub mod helpers;
pub mod rust_entity_parser;
pub mod ts_entity_parser;
pub mod tsx_display_parser; // Kept for now, consider if it's still needed

// Re-export the necessary public functions and structs
pub use entities::{CodeContext, CodeEntity};
pub use rust_entity_parser::extract_rust_entities_from_file;
pub use ts_entity_parser::extract_ts_entities_from_file as extract_ts_entities;
// tsx_display_parser is mostly for testing/debugging, might not need re-exporting here
// pub use tsx_display_parser::parse_and_print_tsx_file; 