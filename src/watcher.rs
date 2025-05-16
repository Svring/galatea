use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing;

use anyhow::{anyhow, Context, Result};

use jsonrpc_lite::{Id, JsonRpc, Params};
use lsp_types::notification::Notification;
use lsp_types::request::Request;
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, InitializeParams,
    PartialResultParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceFolder,
};

use crate::resolver;
use crate::logging::{self, LogLevel, LogSource};

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

pub async fn run_eslint() -> Result<Vec<EslintResult>> {
    let project_dir = resolver::get_project_root()?;
    let msg_start = format!("Running ESLint in {}", project_dir.display());
    logging::add_log_entry(LogSource::WatcherEslint, LogLevel::Info, msg_start.clone());
    tracing::info!(target: "galatea::watcher::eslint", source_process = "eslint_runner", "{}", msg_start);

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir)
        .args(["run", "lint"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .spawn()
        .with_context(|| {
            format!(
                "Failed to spawn 'npm run lint' for project {}",
                project_dir.display()
            )
        })?
        .wait_with_output()
        .await
        .with_context(|| {
            format!(
                "Failed to wait for 'npm run lint' for project {}",
                project_dir.display()
            )
        })?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    match serde_json::from_str::<Vec<EslintResult>>(&stdout_str) {
        Ok(results) => {
            // JSON parsing successful
            let log_msg = format!("ESLint completed. Found {} errors, {} warnings. Stderr (if any): {}", results.iter().map(|r| r.error_count).sum::<usize>(), results.iter().map(|r| r.warning_count).sum::<usize>(), stderr_str);
            logging::add_log_entry(LogSource::WatcherEslint, if output.status.success() { LogLevel::Info } else { LogLevel::Warn }, log_msg);
            if !output.status.success() {
                tracing::warn!(target: "galatea::watcher::eslint", source_process = "eslint_runner", 
                               "ESLint command failed with status {}, but valid JSON output was parsed. Using JSON results. Stderr: '{}'", 
                               output.status, stderr_str);
            }
            Ok(results)
        }
        Err(_e_parse_json) => {
            // JSON parsing failed, treat output as plain text
            match output.status.success() {
                true => {
                    let trimmed_stdout = stdout_str.trim();
                    let is_success_message = trimmed_stdout.is_empty()
                        || [
                            "✔ No ESLint warnings or errors",
                            "No problems found!",
                            "All matched files use Prettier code style!",
                        ]
                        .iter()
                        .any(|msg| trimmed_stdout.contains(msg));

                    let log_msg = if is_success_message {
                        "ESLint command succeeded. Output was plain text indicating no issues (or empty).".to_string()
                    } else {
                        format!(
                            "ESLint command succeeded, but its plain text output was not a known success message and not JSON. Assuming no parseable lint issues. Stdout: '{}'", 
                            stdout_str
                        )
                    };
                    logging::add_log_entry(LogSource::WatcherEslint, LogLevel::Info, log_msg.clone());

                    if is_success_message {
                        tracing::info!(target: "galatea::watcher::eslint", source_process = "eslint_runner", "{}", log_msg);
                    } else {
                        tracing::warn!(target: "galatea::watcher::eslint", source_process = "eslint_runner", "{}", log_msg);
                    }
                    Ok(Vec::new())
                }
                false => {
                    let err_msg = format!(
                        "'npm run lint' execution failed with status {}. Output was not parseable as JSON. Stdout: '{}'. Stderr: '{}'",
                        output.status, stdout_str, stderr_str
                    );
                    logging::add_log_entry(LogSource::WatcherEslint, LogLevel::Error, err_msg.clone());
                    Err(anyhow!(err_msg))
                }
            }
        }
    }
}

// --- Formatter (Prettier) Interaction ---

pub async fn run_format() -> Result<Vec<String>> {
    let project_dir = resolver::get_project_root()?;
    let msg_start = format!("Running Prettier in {}", project_dir.display());
    logging::add_log_entry(LogSource::WatcherPrettier, LogLevel::Info, msg_start.clone());
    tracing::info!(target: "galatea::watcher::prettier", source_process = "prettier_runner", "{}", msg_start);

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir)
        .args(&["run", "format"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .spawn()
        .with_context(|| {
            format!(
                "Failed to spawn 'npm run format' for project {}",
                project_dir.display()
            )
        })?
        .wait_with_output()
        .await
        .with_context(|| {
            format!(
                "Failed to wait for 'npm run format' for project {}",
                project_dir.display()
            )
        })?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    match output.status.success() {
        true => {
            // Log stdout if present
            let mut log_messages = Vec::new();
            if !stdout_str.is_empty() {
                let stdout_log = format!("'npm run format' stdout: {}", stdout_str);
                logging::add_log_entry(LogSource::WatcherPrettier, LogLevel::Debug, stdout_log.clone());
                tracing::info!(target: "galatea::watcher::prettier", source_process = "prettier_runner", "{}", stdout_log);
                log_messages.push(stdout_log);
            }

            // Log stderr as warning/info even on success
            if !stderr_str.is_empty() {
                let stderr_log = format!("'npm run format' stderr (info): {}", stderr_str);
                logging::add_log_entry(LogSource::WatcherPrettier, LogLevel::Info, stderr_log.clone());
                tracing::warn!(target: "galatea::watcher::prettier", source_process = "prettier_runner", "{}", stderr_log);
                log_messages.push(stderr_log);
            }
            let final_msg = if log_messages.is_empty() {
                "Prettier run completed with no specific output.".to_string()
            } else {
                format!("Prettier run completed. Details: {}", log_messages.join("; "))
            };
            logging::add_log_entry(LogSource::WatcherPrettier, LogLevel::Info, final_msg);
            Ok(Vec::new())
        }
        false => {
            let err_msg = format!(
                "'npm run format' execution failed with status {}. Stdout: '{}'. Stderr: '{}'",
                output.status,
                stdout_str,
                stderr_str
            );
            logging::add_log_entry(LogSource::WatcherPrettier, LogLevel::Error, err_msg.clone());
            Err(anyhow!(err_msg))
        }
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
    pub async fn new() -> Result<Self> {
        let project_dir = resolver::get_project_root()?;

        let msg_spawn = format!("Spawning LSP server (npm run lsp) in {}", project_dir.display());
        logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, msg_spawn.clone());
        tracing::info!(target: "galatea::watcher::lsp", source_process = "lsp_server_spawner", "{}", msg_spawn);

        let mut cmd = Command::new("npm");
        cmd.current_dir(&project_dir)
            .args(&["run", "lsp"]) // The script "lsp": "typescript-language-server --stdio"
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn 'npm run lsp'"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to get LSP stdin after 'npm run lsp'"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to get LSP stdout after 'npm run lsp'"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to get LSP stderr after 'npm run lsp'"))?;

        let (response_tx, response_rx) = mpsc::channel(100);

        // Reader task for stdout
        let stdout_tx_clone = response_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                // Main message reading loop
                let mut content_length: Option<usize> = None;
                let mut headers_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                let mut lines_scanned_for_this_message_headers = 0;
                const MAX_LINES_TO_SCAN_FOR_HEADERS: usize = 10;

                // Header reading loop
                loop {
                    if lines_scanned_for_this_message_headers >= MAX_LINES_TO_SCAN_FOR_HEADERS {
                        if content_length.is_none() {
                            let eof_msg = format!("Scanned {} lines without finding a Content-Length header.", MAX_LINES_TO_SCAN_FOR_HEADERS);
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Warn, eof_msg.clone());
                            tracing::warn!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", eof_msg);
                        }
                        break;
                    }

                    let mut line_buffer = String::new();
                    match reader.read_line(&mut line_buffer).await {
                        Ok(0) => {
                            // EOF
                            let eof_msg = "LSP stdout closed (EOF) while reading headers.";
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Warn, eof_msg.to_string());
                            tracing::warn!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", eof_msg);
                            return; // Exit task
                        }
                        Ok(_) => {
                            lines_scanned_for_this_message_headers += 1;
                            let trimmed_line = line_buffer.trim();
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Trace, format!("LSP Server Header Line: {}", trimmed_line));

                            if trimmed_line.is_empty() {
                                if content_length.is_some() {
                                    break; // End of headers if Content-Length was found
                                } else {
                                    // Empty line before Content-Length, possibly initial noise.
                                    logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Debug, "Skipped initial empty line before Content-Length".to_string());
                                    tracing::debug!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "Skipped initial empty line before Content-Length");
                                    continue; // Continue scanning up to MAX_LINES_TO_SCAN_FOR_HEADERS
                                }
                            }

                            if let Some(len_str) = trimmed_line.strip_prefix("Content-Length: ") {
                                content_length = len_str.parse::<usize>().ok();
                                headers_map
                                    .insert("Content-Length".to_string(), len_str.to_string());
                            } else if trimmed_line.contains(':') {
                                if let Some((key, value)) = trimmed_line.split_once(": ") {
                                    headers_map.insert(key.to_string(), value.to_string());
                                }
                            } else {
                                logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Debug, format!("Skipped initial non-header line: {}", trimmed_line));
                                tracing::debug!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "Skipped initial non-header line: {}", trimmed_line);
                            }
                        }
                        Err(e) => {
                            let err_msg = format!("Error reading line for LSP headers: {}", e);
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Error, err_msg.clone());
                            tracing::error!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", err_msg);
                            return; // Exit task
                        }
                    }
                } // End of header reading loop

                if let Some(len) = content_length {
                    let mut buffer = vec![0; len];
                    if reader.read_exact(&mut buffer).await.is_err() {
                        let err_msg = "Error reading message body or stream closed.";
                        logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Error, err_msg.to_string());
                        tracing::error!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", err_msg);
                        return;
                    }
                    let msg_str = String::from_utf8_lossy(&buffer);
                    logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Debug, format!("LSP Server Message Body: {}", msg_str));
                    match serde_json::from_str::<JsonRpc>(&msg_str) {
                        Ok(rpc_msg) => {
                            let rpc_log_msg = format!("Parsed LSP RPC message: {:?}", rpc_msg);
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Debug, rpc_log_msg);

                            if stdout_tx_clone.send(rpc_msg).await.is_err() {
                                let err_send_msg = "Failed to send LSP message to internal channel (receiver dropped).";
                                logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, err_send_msg.to_string());
                                tracing::error!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", err_send_msg);
                                return;
                            }
                        }
                        Err(e) => {
                            let parse_err_msg = format!("Failed to parse LSP message: {}. Raw: '{}'", e, msg_str);
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Error, parse_err_msg.clone());
                            tracing::error!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", "{}", parse_err_msg);
                        }
                    }
                } else {
                    // This path is taken if the header loop exited without content_length being set.
                    if lines_scanned_for_this_message_headers > 0
                        && (lines_scanned_for_this_message_headers < MAX_LINES_TO_SCAN_FOR_HEADERS
                            || !headers_map.is_empty())
                    {
                        // Avoid logging this if we simply hit MAX_LINES_TO_SCAN_FOR_HEADERS with no useful header info found,
                        // as that case is already logged above. Only log if we broke for other reasons (e.g. early empty line after some junk)
                        // or if we scanned fewer than max lines but still failed.
                        logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Warn, format!("LSP message processing did not yield a Content-Length. Headers map: {:?}, Lines scanned: {}", headers_map, lines_scanned_for_this_message_headers));
                        tracing::warn!(target: "galatea::watcher::lsp_stdout_parser", source_process = "lsp_server", 
                                       "LSP message processing did not yield a Content-Length. Headers map: {:?}, Lines scanned: {}", 
                                       headers_map, lines_scanned_for_this_message_headers);
                    }
                }
            } // End of main message reading loop
        });

        // Reader task for stderr
        // let stderr_tx_clone = response_tx; // Not strictly needed to clone for this, but good practice
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buffer = String::new();
            loop {
                match reader.read_line(&mut buffer).await {
                    Ok(0) => {
                        let eof_msg = "LSP Server (stderr): Stream closed.";
                        logging::add_log_entry(LogSource::WatcherLspServerStderr, LogLevel::Debug, eof_msg.to_string());
                        tracing::debug!(target: "galatea::watcher::lsp_server_stderr", source_process = "lsp_server", "{}", eof_msg);
                        break;
                    }
                    Ok(_) => {
                        let line_content = buffer.trim_end().to_string();
                        logging::add_log_entry(LogSource::WatcherLspServerStderr, LogLevel::Warn, line_content.clone());
                        tracing::warn!(target: "galatea::watcher::lsp_server_stderr", source_process = "lsp_server", "{}", line_content);
                        buffer.clear(); // Clear buffer for next line
                    }
                    Err(e) => {
                        let err_msg = format!("LSP Server (stderr): Error reading from stream: {}", e);
                        logging::add_log_entry(LogSource::WatcherLspServerStderr, LogLevel::Error, err_msg.clone());
                        tracing::error!(target: "galatea::watcher::lsp_server_stderr", source_process = "lsp_server", "{}", err_msg);
                        break;
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

    async fn send_rpc(&mut self, rpc: JsonRpc) -> Result<()> {
        let msg_json = serde_json::to_string(&rpc)
            .with_context(|| format!("Failed to serialize LSP RPC: {:?}", rpc))?;
        let msg_with_header = format!("Content-Length: {}\r\n\r\n{}", msg_json.len(), msg_json);

        logging::add_log_entry(
            LogSource::WatcherLspClientRequest,
            LogLevel::Debug, 
            format!("Sending LSP RPC: Method: {:?}, ID: {:?}, Params: {:?}", rpc.get_method(), rpc.get_id(), rpc.get_params())
        );

        self.writer
            .write_all(msg_with_header.as_bytes())
            .await
            .with_context(|| format!("Failed to write to LSP stdin for RPC: {:?}", rpc))?;
        self.writer
            .flush()
            .await
            .with_context(|| format!("Failed to flush LSP stdin for RPC: {:?}", rpc))?;
        Ok(())
    }

    async fn send_request(&mut self, method: &str, params: Params) -> Result<Id> {
        let id = self.next_request_id();
        let request_obj = JsonRpc::request_with_params(id.clone(), method, params.clone());
        self.send_rpc(request_obj).await.with_context(|| {
            format!(
                "Failed to send LSP request {} with params {:?}",
                method, params
            )
        })?;
        Ok(id)
    }

    async fn send_notification(&mut self, method: &str, params: Params) -> Result<()> {
        let notification_obj = JsonRpc::notification_with_params(method, params.clone());
        self.send_rpc(notification_obj).await.with_context(|| {
            format!(
                "Failed to send LSP notification {} with params {:?}",
                method, params
            )
        })?;
        Ok(())
    }

    async fn wait_for_response(&mut self, request_id: &Id, timeout_secs: u64) -> Result<JsonRpc> {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
        loop {
            match tokio::time::timeout_at(deadline, self.response_rx.recv()).await {
                Ok(Some(response_candidate)) => {
                    match response_candidate.get_id() {
                        Some(id) if id == request_id.clone() => {
                            logging::add_log_entry(
                                LogSource::WatcherLspClientResponse,
                                LogLevel::Debug, 
                                format!("Received matching LSP response for ID {:?}: {:?}", request_id, response_candidate)
                            );
                            return Ok(response_candidate);
                        }
                        Some(other_id) => {
                            let log_level = if response_candidate.get_method().is_some() { LogLevel::Warn } else { LogLevel::Debug };
                            logging::add_log_entry(
                                if response_candidate.get_method().is_some() { LogSource::WatcherLspClientNotification } else { LogSource::WatcherLspClientResponse }, 
                                log_level, 
                                format!(
                                    "Received LSP message for different ID ({:?}) while waiting for ID {:?}. Full message: {:?}", 
                                    other_id, request_id, response_candidate
                                )
                            );
                            response_candidate.get_method()
                                .map_or_else(
                                    || {
                                        tracing::debug!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", 
                                                      "Received response for a different ID ({:?}) while waiting for ID {:?}. Full message: {:?}", 
                                                      other_id, request_id, response_candidate);
                                    },
                                    |method| {
                                        tracing::warn!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", 
                                                     "Received a SERVER REQUEST (Method: {}, ID: {:?}) while waiting for response to ID {:?}. This is unexpected while polling for a specific response.", 
                                                     method, other_id, request_id);
                                    }
                                );
                        }
                        None => {
                            // No ID typically means it's a Notification
                            let method_name = response_candidate
                                .get_method()
                                .unwrap_or("[unknown_method]");
                            
                            // Define params_detail here before it's used
                            let params_detail = response_candidate.get_params().map_or(
                                "[no_params]".to_string(),
                                |params| match params {
                                    Params::Array(arr) => {
                                        let items = arr.iter()
                                            .map(|v| format!("{:?}", v))
                                            .collect::<Vec<_>>()
                                            .join(", ");
                                        format!("Array([{}])", items)
                                    },
                                    Params::Map(map) => {
                                        let items = map.iter()
                                            .map(|(k, v)| format!("{}: {:?}", k, v))
                                            .collect::<Vec<_>>()
                                            .join(", ");
                                        format!("Map({{ {} }})", items)
                                    },
                                    Params::None(_) => "None".to_string(),
                                },
                            );

                            logging::add_log_entry(
                                LogSource::WatcherLspClientNotification, // Server-initiated notification
                                LogLevel::Debug,
                                format!(
                                    "Received LSP notification (Method: {}) while waiting for response to ID {:?}. Params: {}", 
                                    method_name, request_id, params_detail // Now params_detail is in scope
                                )
                            );

                            tracing::debug!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", 
                                          "Received notification (Method: {}) while waiting for response to ID {:?}. Params: {}", 
                                          method_name, request_id, params_detail);
                        }
                    }
                }
                Ok(None) => {
                    let err_msg = format!(
                        "LSP response channel closed while waiting for request ID {:?}",
                        request_id
                    );
                    logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, err_msg.clone());
                    return Err(anyhow!(err_msg))
                }
                Err(_) => {
                    let timeout_msg = format!(
                        "Timeout waiting for LSP response for request ID {:?}",
                        request_id
                    );
                    logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, timeout_msg.clone());
                    return Err(anyhow!(timeout_msg))
                } // Elapsed
            }
        }
    }

    #[allow(deprecated)] // Suppress warnings for deprecated fields used in InitializeParams
    pub async fn initialize(
        &mut self,
        root_uri: Uri, // This uri is used to derive workspace_folder.uri
        client_capabilities: ClientCapabilities,
    ) -> Result<lsp_types::InitializeResult> {
        let workspace_folder_path = root_uri.path().to_string();

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
            root_uri: None, 
            root_path: None, 
            initialization_options: None,
            capabilities: client_capabilities,
            trace: None,
            workspace_folders: Some(vec![workspace_folder]), 
            client_info: None,
            locale: None, 
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        logging::add_log_entry(LogSource::WatcherLspClientLifecycle, LogLevel::Info, "Sending LSP Initialize request".to_string());
        let request_id = self
            .send_request(
                lsp_types::request::Initialize::METHOD,
                Params::from(
                    serde_json::to_value(params)
                        .context("Serialize InitializeParams error for LSP initialize")?,
                ),
            )
            .await
            .context("Sending Initialize request to LSP failed")?;

        let response_rpc = self
            .wait_for_response(&request_id, 10)
            .await
            .context("Waiting for Initialize response from LSP failed")?;

        logging::add_log_entry(LogSource::WatcherLspClientLifecycle, LogLevel::Info, format!("Received LSP Initialize response: {:?}", response_rpc.get_result().is_some()));
        // Use get_result() on JsonRpc enum directly
        match response_rpc.get_result() {
            Some(result_value) => {
                serde_json::from_value::<lsp_types::InitializeResult>(result_value.clone())
                    .context("Failed to parse InitializeResult from LSP response")
            }
            None => {
                // This case implies it wasn't a Success response or get_result() failed internally based on response type
                // We need to check if it was an error explicitly if get_result() returns None for JsonRpc::Error
                if let JsonRpc::Error(e) = response_rpc {
                    Err(anyhow!("LSP Initialize error: {:?}", e))
                } else {
                    Err(anyhow!("LSP Initialize: Did not receive a success or error response, or result was absent."))
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
    ) -> Result<()> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(), // Clone for logging
                language_id: language_id.to_string(),
                version,
                text,
            },
        };
        logging::add_log_entry(
            LogSource::WatcherLspClientNotification, // This is client sending a notification
            LogLevel::Info, 
            format!("Sending LSP DidOpenTextDocument notification for {:?}: lang={}, ver={}", uri, language_id, version)
        );
        self.send_notification(
            lsp_types::notification::DidOpenTextDocument::METHOD,
            Params::from(serde_json::to_value(params).context("Serialize DidOpenParams error")?),
        )
        .await
    }

    pub async fn goto_definition(
        &mut self,
        uri: Uri,
        position: Position,
    ) -> Result<Option<lsp_types::GotoDefinitionResponse>> {
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        logging::add_log_entry(
            LogSource::WatcherLspClientRequest,
            LogLevel::Info, 
            format!("Sending LSP GotoDefinition request for {:?}:({},{})", uri, position.line, position.character)
        );
        let request_id = self
            .send_request(
                lsp_types::request::GotoDefinition::METHOD,
                Params::from(
                    serde_json::to_value(params)
                        .context("Serialize GotoDefinitionParams error for LSP")?,
                ),
            )
            .await
            .context("Sending GotoDefinition request to LSP failed")?;

        let response_rpc = self
            .wait_for_response(&request_id, 5)
            .await
            .context("Waiting for GotoDefinition response from LSP failed")?;

        logging::add_log_entry(
            LogSource::WatcherLspClientResponse, 
            LogLevel::Info, 
            format!("Received LSP GotoDefinition response. Has result: {}", response_rpc.get_result().is_some())
        );
        // Use get_result() on JsonRpc enum directly
        match response_rpc.get_result() {
            Some(result_value) => serde_json::from_value(result_value.clone()) // Option<GotoDefinitionResponse> handles null/array
                .context("Failed to parse GotoDefinitionResponse from LSP response"),
            None => {
                if let JsonRpc::Error(e) = response_rpc {
                    Err(anyhow!("LSP GotoDefinition error: {:?}", e))
                } else {
                    Err(anyhow!("LSP GotoDefinition: Did not receive a success or error response, or result was absent."))
                }
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        // Try to send shutdown request
        let shutdown_params = Params::None(());
        logging::add_log_entry(LogSource::WatcherLspClientLifecycle, LogLevel::Info, "Attempting to send LSP Shutdown request".to_string());
        match self
            .send_request(lsp_types::request::Shutdown::METHOD, shutdown_params)
            .await
        {
            Ok(shutdown_id) => match self.wait_for_response(&shutdown_id, 5).await {
                Ok(_) => {
                    logging::add_log_entry(LogSource::WatcherLspClientLifecycle, LogLevel::Info, "LSP Shutdown successful.".to_string());
                    tracing::info!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", "LSP Shutdown successful.");
                }
                Err(e) => {
                    let err_msg = format!("LSP Shutdown request failed or timed out: {}", e);
                    logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, err_msg.clone());
                    tracing::error!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", "{}", err_msg);
                }
            },
            Err(e) => {
                let err_msg = format!("Failed to send LSP Shutdown request: {}", e);
                logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, err_msg.clone());
                tracing::error!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", "{}", err_msg);
            }
        }

        // Send exit notification (fire and forget)
        logging::add_log_entry(LogSource::WatcherLspClientLifecycle, LogLevel::Info, "Sending LSP Exit notification".to_string());
        if let Err(e) = self
            .send_notification(lsp_types::notification::Exit::METHOD, Params::None(()))
            .await
        {
            let err_msg = format!("Failed to send LSP Exit notification: {}", e);
            logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, err_msg.clone());
            tracing::error!(target: "galatea::watcher::lsp_client_logic", source_component = "lsp_client", "{}", err_msg);
        }

        // Wait for the child process to exit
        match self.child_process.wait().await {
            Ok(status) => {
                let exit_msg = format!("LSP child process exited with status: {}", status);
                logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, exit_msg.clone());
                tracing::info!(target: "galatea::watcher::lsp_client_logic", source_process = "lsp_server", "{}", exit_msg);
            }
            Err(e) => {
                let err_msg = format!("Failed to wait for LSP child process exit: {}", e);
                logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Error, err_msg.clone());
                tracing::error!(target: "galatea::watcher::lsp_client_logic", source_process = "lsp_server", "{}", err_msg);
            }
        }
        Ok(())
    }
}
