use std::fs;
use std::path::{Path, PathBuf};

// Enum to represent the type of the last operation for undo functionality
#[derive(Debug)]
enum LastOperation {
    None,
    Create {
        path: PathBuf,
    }, // File was created, undo is deletion
    Overwrite {
        path: PathBuf,
        original_content: Vec<u8>,
    }, // File existed and was overwritten or modified
}

// Editor structure to hold state, like the last operation for undo
pub struct Editor {
    last_op: LastOperation,
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            last_op: LastOperation::None,
        }
    }

    // Private helper to record an operation that modified a file
    fn record_write_op(&mut self, path: &Path, original_content: Option<Vec<u8>>) {
        if let Some(content) = original_content {
            self.last_op = LastOperation::Overwrite {
                path: path.to_path_buf(),
                original_content: content,
            };
        } else {
            // File was newly created (or didn't exist before this op for create command)
            self.last_op = LastOperation::Create {
                path: path.to_path_buf(),
            };
        }
    }
}

// Define the command types based on the schema
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandType {
    View,
    Create,
    StrReplace,
    Insert,
    UndoEdit,
}

// Arguments for the editor commands, derived from the schema
#[derive(Debug, Clone)]
pub struct EditorArgs {
    pub command: CommandType,
    pub path: String,
    pub file_text: Option<String>,      // For Create
    pub insert_line: Option<usize>,     // For Insert (1-indexed)
    pub new_str: Option<String>,        // For StrReplace (optional), Insert (required)
    pub old_str: Option<String>,        // For StrReplace (required)
    pub view_range: Option<Vec<isize>>, // For View (e.g., [1, 10] or [5, -1])
}

pub fn handle_command(editor: &mut Editor, args: EditorArgs) -> Result<Option<String>, String> {
    let path = PathBuf::from(&args.path);

    match args.command {
        CommandType::View => view_file(&path, args.view_range),
        CommandType::Create => {
            let content = args.file_text.ok_or_else(|| {
                "Error: 'file_text' is required for 'create' command.".to_string()
            })?;
            create_file(editor, &path, &content)
        }
        CommandType::StrReplace => {
            let old_s = args.old_str.ok_or_else(|| {
                "Error: 'old_str' is required for 'str_replace' command.".to_string()
            })?;
            let new_s = args.new_str.unwrap_or_default();
            str_replace_in_file(editor, &path, &old_s, &new_s)
        }
        CommandType::Insert => {
            let line_num_1_indexed = args.insert_line.ok_or_else(|| {
                "Error: 'insert_line' is required for 'insert' command.".to_string()
            })?;
            if line_num_1_indexed == 0 {
                return Err("Error: 'insert_line' must be 1-indexed and positive.".to_string());
            }
            let new_s = args
                .new_str
                .ok_or_else(|| "Error: 'new_str' is required for 'insert' command.".to_string())?;
            insert_into_file(editor, &path, line_num_1_indexed - 1, &new_s)
        }
        CommandType::UndoEdit => undo_last_edit(editor),
    }
}

fn view_file(path: &Path, view_range: Option<Vec<isize>>) -> Result<Option<String>, String> {
    if !path.exists() {
        return Err(format!("Error: File not found at '{}'", path.display()));
    }
    if !path.is_file() {
        return Err(format!("Error: Path '{}' is not a file.", path.display()));
    }

    let file_content = fs::read_to_string(path)
        .map_err(|e| format!("Error reading file '{}': {}", path.display(), e))?;

    match view_range {
        Some(range) => {
            if range.len() != 2 {
                return Err("Error: 'view_range' must contain exactly two elements: [start_line, end_line].".to_string());
            }
            let start_line = range[0]; // 1-indexed
            let mut end_line = range[1]; // 1-indexed or -1

            let lines: Vec<&str> = file_content.lines().collect();
            let total_lines = lines.len() as isize;

            if start_line <= 0 {
                return Err("Error: Start line in 'view_range' must be positive.".to_string());
            }

            if total_lines == 0 {
                // Empty file
                if start_line == 1 && (end_line == -1 || end_line >= 1) {
                    return Ok(Some("".to_string())); // e.g. [1,1] or [1,-1] on empty file
                } else if start_line == 1 && end_line < 1 && end_line != -1 {
                    // e.g. [1,0]
                    return Err(format!(
                        "Error: End line {} is invalid for start line {} on an empty file.",
                        end_line, start_line
                    ));
                }
                // start_line > 1 on empty file, or invalid end_line for start_line 1
                return Err(format!(
                    "Error: Start line {} is beyond the end of an empty file or range is invalid.",
                    start_line
                ));
            }

            // File has content
            if start_line > total_lines {
                return Err(format!(
                    "Error: Start line {} is beyond the end of file ({} lines).",
                    start_line, total_lines
                ));
            }

            if end_line == -1 {
                end_line = total_lines;
            } else if end_line == 0 {
                return Err("Error: End line in 'view_range' cannot be 0.".to_string());
            } else if end_line < start_line {
                return Err(format!(
                    "Error: End line {} cannot be less than start line {}.",
                    end_line, start_line
                ));
            }

            // Cap end_line if it exceeds total_lines (and wasn't originally -1)
            if range[1] != -1 && end_line > total_lines {
                end_line = total_lines;
            }

            let start_0_idx = (start_line - 1) as usize;
            // end_line is 1-indexed inclusive. For count: end_line - start_line + 1
            let count = (end_line - start_line + 1).max(0) as usize;

            let selected_lines: Vec<&str> = lines
                .iter()
                .skip(start_0_idx)
                .take(count)
                .copied()
                .collect();

            Ok(Some(selected_lines.join("\n")))
        }
        None => Ok(Some(file_content)),
    }
}

fn create_file(editor: &mut Editor, path: &Path, content: &str) -> Result<Option<String>, String> {
    let original_content = if path.exists() {
        if path.is_dir() {
            return Err(format!(
                "Error: Path '{}' is a directory, cannot create file.",
                path.display()
            ));
        }
        Some(fs::read(path).map_err(|e| {
            format!(
                "Error reading existing file '{}' for undo: {}",
                path.display(),
                e
            )
        })?)
    } else {
        None
    };

    fs::write(path, content)
        .map_err(|e| format!("Error writing file '{}': {}", path.display(), e))?;

    editor.record_write_op(path, original_content);
    Ok(None)
}

fn str_replace_in_file(
    editor: &mut Editor,
    path: &Path,
    old_str: &str,
    new_str: &str,
) -> Result<Option<String>, String> {
    if !path.exists() {
        return Err(format!("Error: File not found at '{}'", path.display()));
    }
    if !path.is_file() {
        return Err(format!("Error: Path '{}' is not a file.", path.display()));
    }
    if old_str.is_empty() {
        return Err("Error: 'old_str' for replacement cannot be empty.".to_string());
    }

    let original_content_bytes =
        fs::read(path).map_err(|e| format!("Error reading file '{}': {}", path.display(), e))?;

    let original_content_str = String::from_utf8(original_content_bytes.clone())
        .map_err(|e| format!("Error: File '{}' is not valid UTF-8: {}", path.display(), e))?;

    let modified_content = original_content_str.replace(old_str, new_str);

    if modified_content != original_content_str {
        fs::write(path, &modified_content)
            .map_err(|e| format!("Error writing to file '{}': {}", path.display(), e))?;
        editor.record_write_op(path, Some(original_content_bytes));
    }

    Ok(None)
}

fn insert_into_file(
    editor: &mut Editor,
    path: &Path,
    insert_line_0_indexed: usize,
    text_to_insert: &str,
) -> Result<Option<String>, String> {
    if !path.exists() {
        return Err(format!(
            "Error: File not found at '{}' for insert operation.",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!("Error: Path '{}' is not a file.", path.display()));
    }

    let original_content_bytes =
        fs::read(path).map_err(|e| format!("Error reading file '{}': {}", path.display(), e))?;
    let original_content_str = String::from_utf8(original_content_bytes.clone())
        .map_err(|e| format!("Error: File '{}' is not valid UTF-8: {}", path.display(), e))?;

    let mut lines: Vec<String> = original_content_str.lines().map(String::from).collect();

    if insert_line_0_indexed > lines.len() {
        // e.g. 3 lines (len=3, idx 0,1,2). insert_0_idx=3 (after line 3 / at line 4). This is an append.
        // If insert_0_idx=4, then 4 > 3 is true -> Error.
        return Err(format!(
            "Error: 'insert_line' {} (0-indexed: {}) is out of bounds for file with {} lines. Cannot insert after a non-existent line.",
            insert_line_0_indexed + 1, insert_line_0_indexed, lines.len()
        ));
    }

    if lines.is_empty() && insert_line_0_indexed == 0 {
        // Inserting into an empty file at (1-indexed) line 1
        lines.push(text_to_insert.to_string());
    } else if insert_line_0_indexed == lines.len() {
        // Append: insert_line (1-idx) is 1 greater than total lines
        lines.push(text_to_insert.to_string());
    } else {
        // Insert in the middle: insert_line_0_indexed < lines.len()
        lines.insert(insert_line_0_indexed + 1, text_to_insert.to_string());
    }

    let mut modified_content = lines.join("\n");
    // Handle trailing newline consistency: if original had one (and wasn't empty), and new one doesn't, add it.
    if !original_content_str.is_empty()
        && original_content_str.ends_with('\n')
        && !lines.is_empty()
        && !modified_content.ends_with('\n')
    {
        modified_content.push('\n');
    }
    // If original was empty, or didn't end with newline, and new content (from single line insert) doesn't have one, it's fine.

    if modified_content != original_content_str {
        // Compare with string representation, not bytes, due to potential newline char differences
        fs::write(path, &modified_content)
            .map_err(|e| format!("Error writing to file '{}': {}", path.display(), e))?;
        editor.record_write_op(path, Some(original_content_bytes));
    }

    Ok(None)
}

fn undo_last_edit(editor: &mut Editor) -> Result<Option<String>, String> {
    match std::mem::replace(&mut editor.last_op, LastOperation::None) {
        LastOperation::None => Err("Error: No operation to undo.".to_string()),
        LastOperation::Create { path } => {
            if path.exists() && path.is_file() {
                fs::remove_file(&path).map_err(|e| {
                    format!(
                        "Error undoing creation (deleting file '{}'): {}",
                        path.display(),
                        e
                    )
                })?;
            }
            // If not exists or not a file, consider undo successful as the state (no file) is achieved.
            Ok(None)
        }
        LastOperation::Overwrite {
            path,
            original_content,
        } => {
            if path.is_dir() {
                // editor.last_op is already None, must restore it if op fails early
                editor.last_op = LastOperation::Overwrite {
                    path: path.clone(),
                    original_content,
                };
                return Err(format!(
                    "Error undoing overwrite: Path '{}' is a directory.",
                    path.display()
                ));
            }
            fs::write(&path, original_content).map_err(|e| {
                // Attempt to restore last_op if write fails. This is tricky.
                // For simplicity here, we assume if write fails, the state is uncertain for another undo.
                format!(
                    "Error undoing overwrite (writing original content to '{}'): {}",
                    path.display(),
                    e
                )
            })?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir; // Add tempfile = "3" to [dev-dependencies] in Cargo.toml

    fn make_args_struct(command: CommandType, path_str: &str) -> EditorArgs {
        EditorArgs {
            command,
            path: path_str.to_string(),
            file_text: None,
            insert_line: None,
            new_str: None,
            old_str: None,
            view_range: None,
        }
    }

    #[test]
    fn test_create_view_and_undo_create() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_cvu.txt");
        let mut editor = Editor::new();
        let file_path_str = file_path.to_str().unwrap();

        // Create
        let create_args = EditorArgs {
            file_text: Some("Hello\nWorld".to_string()),
            ..make_args_struct(CommandType::Create, file_path_str)
        };
        handle_command(&mut editor, create_args).unwrap();
        assert!(file_path.exists());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "Hello\nWorld");

        // View
        let view_args = make_args_struct(CommandType::View, file_path_str);
        let content = handle_command(&mut editor, view_args).unwrap().unwrap();
        assert_eq!(content, "Hello\nWorld");

        // Undo Create
        let undo_args = make_args_struct(CommandType::UndoEdit, file_path_str); // Path in args not used by undo
        handle_command(&mut editor, undo_args).unwrap();
        assert!(!file_path.exists());

        // Undo again (should fail)
        let undo_again_args = make_args_struct(CommandType::UndoEdit, file_path_str);
        assert!(handle_command(&mut editor, undo_again_args).is_err());
    }

    #[test]
    fn test_overwrite_and_undo() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_ow.txt");
        let mut editor = Editor::new();
        let file_path_str = file_path.to_str().unwrap();

        fs::write(&file_path, "Original").unwrap();

        // Overwrite (using Create command)
        let overwrite_args = EditorArgs {
            file_text: Some("New Content".to_string()),
            ..make_args_struct(CommandType::Create, file_path_str)
        };
        handle_command(&mut editor, overwrite_args).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "New Content");

        // Undo Overwrite
        let undo_args = make_args_struct(CommandType::UndoEdit, file_path_str);
        handle_command(&mut editor, undo_args).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "Original");
    }

    #[test]
    fn test_str_replace_and_undo() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_sr.txt");
        let mut editor = Editor::new();
        let file_path_str = file_path.to_str().unwrap();

        fs::write(&file_path, "hello world, hello moon").unwrap();

        // Replace
        let replace_args = EditorArgs {
            old_str: Some("hello".to_string()),
            new_str: Some("bye".to_string()),
            ..make_args_struct(CommandType::StrReplace, file_path_str)
        };
        handle_command(&mut editor, replace_args).unwrap();
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "bye world, bye moon"
        );

        // Undo Replace
        let undo_args = make_args_struct(CommandType::UndoEdit, file_path_str);
        handle_command(&mut editor, undo_args).unwrap();
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "hello world, hello moon"
        );
    }

    #[test]
    fn test_insert_and_undo() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_ins.txt");
        let mut editor = Editor::new();
        let file_path_str = file_path.to_str().unwrap();

        fs::write(&file_path, "Line 1\nLine 3").unwrap();

        // Insert
        let insert_args = EditorArgs {
            insert_line: Some(1), // after 1st line (0-indexed 0)
            new_str: Some("Line 2".to_string()),
            ..make_args_struct(CommandType::Insert, file_path_str)
        };
        handle_command(&mut editor, insert_args).unwrap();
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "Line 1\nLine 2\nLine 3"
        );

        // Undo Insert
        let undo_args = make_args_struct(CommandType::UndoEdit, file_path_str);
        handle_command(&mut editor, undo_args).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "Line 1\nLine 3");
    }

    #[test]
    fn test_view_ranges_detailed() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("view_range.txt");
        fs::write(&file_path, "L1\nL2\nL3\nL4\nL5").unwrap();
        let mut editor = Editor::new();
        let path_str = file_path.to_str().unwrap();

        // Test cases
        let test_cases = vec![
            // range, expected_output (Ok value or Err part of message)
            (Some(vec![2, 4]), Ok("L2\nL3\nL4")),  // Lines 2,3,4
            (Some(vec![3, -1]), Ok("L3\nL4\nL5")), // Lines 3 to end
            (Some(vec![1, 1]), Ok("L1")),          // Line 1 only
            (Some(vec![5, 5]), Ok("L5")),          // Last line only
            (Some(vec![1, 5]), Ok("L1\nL2\nL3\nL4\nL5")), // All lines
            (Some(vec![4, 10]), Ok("L4\nL5")),     // Range exceeding end, capped
            (
                Some(vec![6, 7]),
                Err("Start line 6 is beyond the end of file (5 lines)"),
            ), // Start out of bounds
            (
                Some(vec![0, 2]),
                Err("Start line in 'view_range' must be positive"),
            ),
            (
                Some(vec![2, 0]),
                Err("End line in 'view_range' cannot be 0"),
            ),
            (
                Some(vec![3, 2]),
                Err("End line 2 cannot be less than start line 3"),
            ),
            (
                Some(vec![1, 2, 3]),
                Err("'view_range' must contain exactly two elements"),
            ),
        ];

        for (range, expected) in test_cases {
            let mut args = make_args_struct(CommandType::View, path_str);
            args.view_range = range.clone();
            let result = handle_command(&mut editor, args);
            match expected {
                Ok(exp_str) => assert_eq!(
                    result.unwrap().unwrap(),
                    exp_str,
                    "Mismatch for range {:?}",
                    range
                ),
                Err(err_msg_part) => {
                    let err = result.unwrap_err();
                    assert!(
                        err.contains(err_msg_part),
                        "Expected error containing '{}' for range {:?}, got '{}'",
                        err_msg_part,
                        range,
                        err
                    );
                }
            }
        }

        // Test on empty file
        let empty_file_path = dir.path().join("empty_view.txt");
        fs::write(&empty_file_path, "").unwrap();
        let empty_path_str = empty_file_path.to_str().unwrap();

        let mut args_empty = make_args_struct(CommandType::View, empty_path_str);
        args_empty.view_range = Some(vec![1, 1]);
        assert_eq!(
            handle_command(&mut editor, args_empty.clone())
                .unwrap()
                .unwrap(),
            ""
        );

        args_empty.view_range = Some(vec![1, -1]);
        assert_eq!(
            handle_command(&mut editor, args_empty.clone())
                .unwrap()
                .unwrap(),
            ""
        );

        args_empty.view_range = Some(vec![2, 2]);
        assert!(handle_command(&mut editor, args_empty.clone())
            .unwrap_err()
            .contains("Start line 2 is beyond the end of an empty file"));
    }

    #[test]
    fn test_insert_into_empty_file_and_append() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("insert_empty_append.txt");
        let mut editor = Editor::new();
        let path_str = file_path.to_str().unwrap();

        // Create empty file
        fs::File::create(&file_path).unwrap();

        // Insert into empty file (at line 1)
        let mut args = EditorArgs {
            insert_line: Some(1),
            new_str: Some("First Line".to_string()),
            ..make_args_struct(CommandType::Insert, path_str)
        };
        handle_command(&mut editor, args.clone()).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "First Line");

        // Insert after current line 1 (becomes line 2)
        args.insert_line = Some(1); // After "First Line"
        args.new_str = Some("Second Line".to_string());
        handle_command(&mut editor, args.clone()).unwrap();
        // "First Line", then "Second Line" inserted after it.
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "First Line\nSecond Line"
        );

        // Append (insert after line 2, which is current last line)
        args.insert_line = Some(2); // After "Second Line"
        args.new_str = Some("Third Line".to_string());
        handle_command(&mut editor, args.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            "First Line\nSecond Line\nThird Line"
        );
    }

    #[test]
    fn test_insert_error_conditions() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("insert_errors.txt");
        let mut editor = Editor::new();
        let path_str = file_path.to_str().unwrap();

        fs::write(&file_path, "Line A\nLine B").unwrap(); // 2 lines

        // insert_line 0
        let mut args = EditorArgs {
            insert_line: Some(0),
            new_str: Some("fail".to_string()),
            ..make_args_struct(CommandType::Insert, path_str)
        };
        assert!(handle_command(&mut editor, args.clone())
            .unwrap_err()
            .contains("'insert_line' must be 1-indexed"));

        // insert_line out of bounds (too high)
        // File has 2 lines (0, 1). insert_line: Some(4) -> 0-indexed 3. lines.len() = 2. 3 > 2 -> Error.
        args.insert_line = Some(4);
        let err_msg = handle_command(&mut editor, args.clone()).unwrap_err();
        assert!(err_msg.contains("is out of bounds for file with 2 lines"));
        assert!(err_msg.contains("'insert_line' 4 (0-indexed: 3)"));

        // insert into non-existent file
        let non_existent_path = dir.path().join("ghost.txt").to_str().unwrap().to_string();
        args.path = non_existent_path;
        args.insert_line = Some(1);
        assert!(handle_command(&mut editor, args.clone())
            .unwrap_err()
            .contains("File not found"));
    }

    #[test]
    fn test_str_replace_no_change() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("replace_no_change.txt");
        let initial_content = "no match here";
        fs::write(&file_path, initial_content).unwrap();
        let mut editor = Editor::new();

        // Record a dummy op to see if it gets overwritten
        editor.last_op = LastOperation::Create {
            path: PathBuf::from("dummy"),
        };

        let replace_args = EditorArgs {
            old_str: Some("nonexistent".to_string()),
            new_str: Some("replacement".to_string()),
            ..make_args_struct(CommandType::StrReplace, file_path.to_str().unwrap())
        };
        handle_command(&mut editor, replace_args).unwrap();

        assert_eq!(fs::read_to_string(&file_path).unwrap(), initial_content); // Content unchanged
                                                                              // Ensure last_op was NOT updated because no change was made
        match editor.last_op {
            LastOperation::Create { ref path } if path.to_str() == Some("dummy") => {}
            _ => panic!("last_op should not have been updated by a no-op replace"),
        }
    }
} 