use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use jsonrpc_lite::{Id, JsonRpc, Params};
use lsp_types::notification::{DidOpenTextDocument, Exit, Notification as LspNotificationTrait};
use lsp_types::request::{GotoDefinition, Initialize, Request as LspRequestTrait, Shutdown};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, InitializeParams,
    PartialResultParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceFolder,
};

// --- Project and Dependency Management ---

pub fn get_project_root() -> Result<PathBuf, String> {
    let mut exe_path = std::env::current_exe()
        .map_err(|e| format!("Failed to get current executable path: {}", e))?;
    exe_path.pop(); // Remove the executable filename to get its directory

    let project_dir = exe_path.join("project");

    if !project_dir.is_dir() {
        return Err(format!(
            "'project' subdirectory not found in the executable's directory ({}). Please create it.",
            exe_path.display()
        ));
    }
    Ok(project_dir)
}

async fn run_npm_command(
    project_dir: &Path,
    args: &[&str],
    suppress_output: bool,
) -> Result<(), String> {
    let mut cmd = Command::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(args);

    if suppress_output {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    println!(
        "Running command: npm {} in {}",
        args.join(" "),
        project_dir.display()
    );

    let child = cmd.spawn().map_err(|e| {
        format!(
            "Failed to spawn npm command ({} {}): {}. Ensure npm is installed and in PATH.",
            "npm",
            args.join(" "),
            e
        )
    })?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for npm command: {}", e))?;

    if output.status.success() {
        if !suppress_output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.is_empty() {
                println!(
                    "npm stdout:
{}",
                    stdout
                );
            }
        }
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout); // stdout might also contain error info for npm
        Err(format!(
            "npm command failed with status: {}.
Command: npm {}
Stderr: {}
Stdout: {}",
            output.status,
            args.join(" "),
            stderr,
            stdout
        ))
    }
}

async fn is_tool_locally_installed(tool_name: &str, project_dir: &Path) -> bool {
    let bin_path = project_dir
        .join("node_modules")
        .join(".bin")
        .join(tool_name);
    let package_path = project_dir.join("node_modules").join(tool_name);
    bin_path.exists() || package_path.exists()
}

async fn ensure_npm_tool_available(
    tool_name: &str,
    project_dir: &Path,
    dev_dependency: bool,
    suppress_install_output: bool,
) -> Result<(), String> {
    if is_tool_locally_installed(tool_name, project_dir).await {
        println!(
            "Tool '{}' is already installed locally in the project.",
            tool_name
        );
        return Ok(());
    }

    println!(
        "Tool '{}' not found locally. Attempting to install...",
        tool_name
    );
    let mut args = vec!["install", "--loglevel", "error"]; // reduce default npm noise
    if dev_dependency {
        args.push("--save-dev");
    }
    args.push(tool_name);

    run_npm_command(project_dir, &args, suppress_install_output).await?;
    println!("Tool '{}' installed successfully.", tool_name);
    Ok(())
}

pub async fn ensure_eslint_available() -> Result<(), String> {
    let project_dir = get_project_root()?;
    ensure_npm_tool_available("eslint", &project_dir, true, true)
        .await
        .map_err(|e| format!("Failed to ensure eslint: {}", e))
}

pub async fn ensure_prettier_available() -> Result<(), String> {
    let project_dir = get_project_root()?;
    ensure_npm_tool_available("prettier", &project_dir, true, true)
        .await
        .map_err(|e| format!("Failed to ensure prettier: {}", e))
}

pub async fn ensure_typescript_language_server_available() -> Result<(), String> {
    let project_dir = get_project_root()?;
    // typescript-language-server and its peer dependency typescript
    ensure_npm_tool_available("typescript-language-server", &project_dir, true, true)
        .await
        .map_err(|e| format!("Failed to ensure typescript-language-server: {}", e))?;
    ensure_npm_tool_available("typescript", &project_dir, true, true)
        .await
        .map_err(|e| {
            format!(
                "Failed to ensure typescript (peer dependency for typescript-language-server): {}",
                e
            )
        })
}

// --- Linter (ESLint) Interaction ---

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EslintMessage {
    #[serde(rename = "ruleId")] // Corrected field name
    pub rule_id: Option<String>,
    pub severity: u8, // 1 for warning, 2 for error
    pub message: String,
    pub line: usize,
    pub column: usize,
    #[serde(rename = "endLine")]
    pub end_line: Option<usize>,
    #[serde(rename = "endColumn")]
    pub end_column: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EslintResult {
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub messages: Vec<EslintMessage>,
    #[serde(rename = "errorCount")]
    pub error_count: usize,
    #[serde(rename = "warningCount")]
    pub warning_count: usize,
    #[serde(rename = "fixableErrorCount")]
    pub fixable_error_count: usize,
    #[serde(rename = "fixableWarningCount")]
    pub fixable_warning_count: usize,
    pub source: Option<String>,
}

pub async fn run_eslint(target_paths: &[String]) -> Result<Vec<EslintResult>, String> {
    let project_dir = get_project_root()?;
    ensure_eslint_available().await?;

    let eslint_path = project_dir.join("node_modules").join(".bin").join("eslint");
    if !eslint_path.exists() {
        return Err("eslint executable not found after installation attempt.".to_string());
    }

    let mut cmd = Command::new(eslint_path);
    cmd.current_dir(&project_dir);
    cmd.arg("--format");
    cmd.arg("json");
    for path_str in target_paths {
        cmd.arg(path_str);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    println!(
        "Running eslint with paths: {:?} in {}",
        target_paths,
        project_dir.display()
    );

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn eslint: {}", e))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for eslint: {}", e))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);

    if stdout_str.trim().is_empty() {
        if output.status.success() {
            return Ok(Vec::new()); // No issues found, empty results
        } else {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "ESLint execution failed with status {} and no JSON output. Stderr: {}",
                output.status, stderr_str
            ));
        }
    }

    match serde_json::from_str::<Vec<EslintResult>>(&stdout_str) {
        Ok(results) => Ok(results),
        Err(e) => {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "Failed to parse ESLint JSON output: {}. Stdout: '{}'. Stderr: '{}'",
                e, stdout_str, stderr_str
            ))
        }
    }
}

// --- Formatter (Prettier) Interaction ---

pub async fn check_prettier(target_patterns: &[String]) -> Result<Vec<String>, String> {
    let project_dir = get_project_root()?;
    ensure_prettier_available().await?;

    let prettier_path = project_dir
        .join("node_modules")
        .join(".bin")
        .join("prettier");
    if !prettier_path.exists() {
        return Err("prettier executable not found after installation attempt.".to_string());
    }

    let mut cmd = Command::new(prettier_path);
    cmd.current_dir(&project_dir);
    cmd.arg("--check");
    for pattern in target_patterns {
        cmd.arg(pattern);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped()); // Capture stderr for error reporting

    println!(
        "Running prettier --check with patterns: {:?} in {}",
        target_patterns,
        project_dir.display()
    );

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn prettier --check: {}", e))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for prettier --check: {}", e))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    match output.status.code() {
        Some(0) => {
            // Success, all formatted
            println!("Prettier --check: All specified files are formatted correctly.");
            Ok(Vec::new())
        }
        Some(1) => {
            // Some files were not formatted
            let unformatted_files: Vec<String> = stdout_str
                .lines()
                .map(String::from)
                .filter(|s| !s.is_empty())
                .collect();
            Ok(unformatted_files)
        }
        _ => {
            // Other errors
            Err(format!(
                "Prettier --check failed with status: {}. Stderr: {}. Stdout: {}",
                output.status, stderr_str, stdout_str
            ))
        }
    }
}

pub async fn format_with_prettier(target_patterns: &[String]) -> Result<Vec<String>, String> {
    let project_dir = get_project_root()?;
    ensure_prettier_available().await?;

    let prettier_path = project_dir
        .join("node_modules")
        .join(".bin")
        .join("prettier");
    if !prettier_path.exists() {
        return Err("prettier executable not found after installation attempt.".to_string());
    }

    let mut cmd = Command::new(prettier_path);
    cmd.current_dir(&project_dir);
    cmd.arg("--write");
    cmd.arg("--list-different");
    for pattern in target_patterns {
        cmd.arg(pattern);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    println!(
        "Running prettier --write --list-different with patterns: {:?} in {}",
        target_patterns,
        project_dir.display()
    );

    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn prettier --write: {}", e))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for prettier --write: {}", e))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        let changed_files: Vec<String> = stdout_str
            .lines()
            .map(String::from)
            .filter(|s| !s.is_empty())
            .collect();
        if !changed_files.is_empty() {
            println!("Prettier --write: Formatted files: {:?}", changed_files);
        } else {
            println!("Prettier --write: No files needed formatting or no files matched patterns.");
        }
        Ok(changed_files)
    } else {
        Err(format!(
            "Prettier --write failed with status: {}. Stderr: {}. Stdout: {}",
            output.status, stderr_str, stdout_str
        ))
    }
}

// --- Language Server (typescript-language-server) Interaction ---

pub struct LspClient {
    writer: tokio::process::ChildStdin,
    response_rx: mpsc::Receiver<JsonRpc>,
    request_id_counter: AtomicI64,
    child_process: tokio::process::Child,
}

impl LspClient {
    pub async fn new() -> Result<Self, String> {
        let project_dir = get_project_root()?;
        ensure_typescript_language_server_available().await?;

        let tsserver_path = project_dir
            .join("node_modules")
            .join(".bin")
            .join("typescript-language-server");

        if !tsserver_path.exists() {
            return Err(format!("typescript-language-server executable not found at {} even after installation attempt.", tsserver_path.display()));
        }

        let mut child = Command::new(tsserver_path)
            .arg("--stdio")
            .current_dir(&project_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()) // Capture stderr for LSP too
            .spawn()
            .map_err(|e| format!("Failed to spawn typescript-language-server: {}", e))?;

        let stdin = child.stdin.take().ok_or("Failed to get LSP stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get LSP stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to get LSP stderr")?;

        let (response_tx, response_rx) = mpsc::channel(100);

        // Reader task for stdout
        let stdout_tx_clone = response_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut headers_map = std::collections::HashMap::new();
                let mut content_length: Option<usize> = None;

                loop {
                    // Header reading loop
                    let mut line_buffer = String::new();
                    match reader.read_line(&mut line_buffer).await {
                        Ok(0) => {
                            eprintln!("LSP stdout closed (EOF) while reading headers.");
                            return;
                        }
                        Ok(_) => {
                            let trimmed_line = line_buffer.trim();
                            if trimmed_line.is_empty() {
                                break;
                            } // End of headers

                            if let Some(len_str) = trimmed_line.strip_prefix("Content-Length: ") {
                                content_length = len_str.parse::<usize>().ok();
                                headers_map
                                    .insert("Content-Length".to_string(), len_str.to_string());
                            } else {
                                // Store other headers if necessary
                                if let Some((key, value)) = trimmed_line.split_once(": ") {
                                    headers_map.insert(key.to_string(), value.to_string());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading LSP stdout headers: {}", e);
                            return;
                        }
                    }
                }

                if let Some(len) = content_length {
                    let mut buffer = vec![0; len];
                    if reader.read_exact(&mut buffer).await.is_err() {
                        eprintln!("LSP stdout error reading message body or closed.");
                        return;
                    }
                    let msg_str = String::from_utf8_lossy(&buffer);
                    match serde_json::from_str::<JsonRpc>(&msg_str) {
                        Ok(rpc_msg) => {
                            if stdout_tx_clone.send(rpc_msg).await.is_err() {
                                eprintln!("Failed to send LSP message to internal channel (receiver dropped).");
                                return;
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to parse LSP message: {}. Raw: '{}'", e, msg_str);
                        }
                    }
                } else {
                    eprintln!("LSP message did not contain a valid Content-Length header. Headers received: {:?}", headers_map);
                }
            }
        });

        // Reader task for stderr
        tokio::spawn(async move {
            let mut stderr_reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                match stderr_reader.read_line(&mut line).await {
                    Ok(0) => {
                        eprintln!("LSP stderr closed (EOF).");
                        return;
                    } // EOF
                    Ok(_) => {
                        eprint!("LSP stderr: {}", line);
                        line.clear();
                    }
                    Err(e) => {
                        eprintln!("Error reading from LSP stderr: {}", e);
                        return;
                    }
                }
            }
        });

        Ok(Self {
            writer: stdin,
            response_rx,
            request_id_counter: AtomicI64::new(1),
            child_process: child,
        })
    }

    fn next_request_id(&self) -> Id {
        Id::Num(self.request_id_counter.fetch_add(1, Ordering::SeqCst))
    }

    async fn send_rpc(&mut self, rpc: JsonRpc) -> Result<(), String> {
        let msg_json = serde_json::to_string(&rpc)
            .map_err(|e| format!("Failed to serialize LSP RPC: {}", e))?;
        let msg_with_header = format!(
            "Content-Length: {}\r\n\r\n{}",
            msg_json.len(),
            msg_json
        );

        self.writer
            .write_all(msg_with_header.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to LSP stdin: {}", e))?;
        self.writer
            .flush()
            .await
            .map_err(|e| format!("Failed to flush LSP stdin: {}", e))?;
        Ok(())
    }

    async fn send_request(&mut self, method: &str, params: Params) -> Result<Id, String> {
        let id = self.next_request_id();
        let request_obj = JsonRpc::request_with_params(id.clone(), method, params);
        self.send_rpc(request_obj).await?;
        Ok(id)
    }

    async fn send_notification(&mut self, method: &str, params: Params) -> Result<(), String> {
        let notification_obj = JsonRpc::notification_with_params(method, params);
        self.send_rpc(notification_obj).await
    }

    async fn wait_for_response(
        &mut self,
        request_id: &Id,
        timeout_secs: u64,
    ) -> Result<JsonRpc, String> {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
        loop {
            match tokio::time::timeout_at(deadline, self.response_rx.recv()).await {
                Ok(Some(response_candidate)) => {
                    if response_candidate.get_id() == Some(request_id.clone()) {
                        return Ok(response_candidate);
                    } else {
                        // Log or handle other messages (notifications, other responses)
                        println!("LSP Client: Received non-matching or notification message while waiting for {:?}: {:?}", request_id, response_candidate);
                    }
                }
                Ok(None) => {
                    return Err(format!(
                        "LSP response channel closed while waiting for request ID {:?}",
                        request_id
                    ))
                }
                Err(_) => {
                    return Err(format!(
                        "Timeout waiting for LSP response for request ID {:?}",
                        request_id
                    ))
                } // Elapsed
            }
        }
    }

    pub async fn initialize(
        &mut self,
        root_uri: Uri,
        client_capabilities: ClientCapabilities,
    ) -> Result<lsp_types::InitializeResult, String> {
        let workspace_folder_path = root_uri
            .path()
            .to_string();
        
        let workspace_folder_name = Path::new(&workspace_folder_path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "project".to_string());

        let workspace_folder = WorkspaceFolder {
            uri: root_uri.clone(), // Use the provided root_uri
            name: workspace_folder_name,
        };

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri), // This is still technically needed for older LSP versions or specific servers
            root_path: None, // Deprecated in favor of root_uri and workspace_folders
            initialization_options: None,
            capabilities: client_capabilities,
            trace: None,
            workspace_folders: Some(vec![workspace_folder]), // Populate workspace_folders
            client_info: None,
            locale: None, // Kept as None, should be valid Option<String>
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let request_id = self
            .send_request(
                lsp_types::request::Initialize::METHOD,
                Params::from(
                    serde_json::to_value(params)
                        .map_err(|e| format!("Serialize InitializeParams error: {}", e))?,
                ),
            )
            .await?;

        let response_rpc = self.wait_for_response(&request_id, 10).await?;

        // Use get_result() on JsonRpc enum directly
        match response_rpc.get_result() {
            Some(result_value) => {
                serde_json::from_value::<lsp_types::InitializeResult>(result_value.clone())
                    .map_err(|e| format!("Failed to parse InitializeResult from LSP: {}", e))
            }
            None => {
                // This case implies it wasn't a Success response or get_result() failed internally based on response type
                // We need to check if it was an error explicitly if get_result() returns None for JsonRpc::Error
                if let JsonRpc::Error(e) = response_rpc {
                    Err(format!("LSP Initialize error: {:?}", e))
                } else {
                    Err("LSP Initialize: Did not receive a success or error response, or result was absent.".to_string())
                }
            }
        }
    }

    pub async fn notify_did_open(
        &mut self,
        uri: Uri,
        language_id: &str,
        version: i32,
        text: String,
    ) -> Result<(), String> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.to_string(),
                version,
                text,
            },
        };
        self.send_notification(
            lsp_types::notification::DidOpenTextDocument::METHOD,
            Params::from(
                serde_json::to_value(params)
                    .map_err(|e| format!("Serialize DidOpenParams error: {}", e))?,
            ),
        )
        .await
    }

    pub async fn goto_definition(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<Option<lsp_types::GotoDefinitionResponse>, String> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let request_id = self
            .send_request(
                lsp_types::request::GotoDefinition::METHOD,
                Params::from(
                    serde_json::to_value(params)
                        .map_err(|e| format!("Serialize GotoDefinitionParams error: {}", e))?,
                ),
            )
            .await?;

        let response_rpc = self.wait_for_response(&request_id, 5).await?;

        // Use get_result() on JsonRpc enum directly
        match response_rpc.get_result() {
            Some(result_value) => serde_json::from_value(result_value.clone()) // Option<GotoDefinitionResponse> handles null/array
                .map_err(|e| format!("Failed to parse GotoDefinitionResponse from LSP: {}", e)),
            None => {
                if let JsonRpc::Error(e) = response_rpc {
                    Err(format!("LSP GotoDefinition error: {:?}", e))
                } else {
                    Err("LSP GotoDefinition: Did not receive a success or error response, or result was absent.".to_string())
                }
            }
        }
    }

    pub async fn close(mut self) -> Result<(), String> {
        // Try to send shutdown request
        let shutdown_params = Params::None(());
        match self
            .send_request(lsp_types::request::Shutdown::METHOD, shutdown_params)
            .await
        {
            Ok(shutdown_id) => match self.wait_for_response(&shutdown_id, 5).await {
                Ok(_) => println!("LSP Shutdown successful."),
                Err(e) => eprintln!("LSP Shutdown request failed or timed out: {}", e),
            },
            Err(e) => eprintln!("Failed to send LSP Shutdown request: {}", e),
        }

        // Send exit notification (fire and forget)
        if let Err(e) = self
            .send_notification(lsp_types::notification::Exit::METHOD, Params::None(()))
            .await
        {
            eprintln!("Failed to send LSP Exit notification: {}", e);
        }

        // Wait for the child process to exit
        match self.child_process.wait().await {
            Ok(status) => println!("LSP child process exited with status: {}", status),
            Err(e) => eprintln!("Failed to wait for LSP child process exit: {}", e),
        }
        Ok(())
    }
}
