use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

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

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir)
        .args(["run", "lint"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    println!(
        "Running ESLint via npm script: npm run lint in {}",
        project_dir.display()
    );

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
            if !output.status.success() {
                eprintln!(
                    "ESLint command 'npm run lint' failed with status {}, but valid JSON output was parsed. Using JSON results. Stderr: '{}'",
                    output.status, stderr_str
                );
            }
            Ok(results)
        }
        Err(_e_parse_json) => {
            // JSON parsing failed, treat output as plain text
            match output.status.success() {
                true => {
                    // Command succeeded. Check for known plain text success messages or empty output.
                    let trimmed_stdout = stdout_str.trim();
                    let is_success_message = trimmed_stdout.is_empty()
                        || [
                            "✔ No ESLint warnings or errors",
                            "No problems found!",
                            "All matched files use Prettier code style!",
                        ]
                        .iter()
                        .any(|msg| trimmed_stdout.contains(msg));

                    if is_success_message {
                        println!("ESLint command 'npm run lint' succeeded. Output was plain text indicating no issues (or empty).");
                    } else {
                        // Command succeeded, but output is unrecognized plain text.
                        println!(
                            "Warning: ESLint command 'npm run lint' succeeded, but its plain text output was not a known success message and not JSON. Assuming no parseable lint issues. Stdout: '{}'",
                            stdout_str
                        );
                    }
                    Ok(Vec::new())
                }
                false => {
                    // Command failed, and output was not JSON.
                    Err(anyhow!(
                        "'npm run lint' execution failed with status {}. Output was not parseable as JSON. Stdout: '{}'. Stderr: '{}'",
                        output.status, stdout_str, stderr_str
                    ))
                }
            }
        }
    }
}

// --- Formatter (Prettier) Interaction ---

pub async fn run_format() -> Result<Vec<String>> {
    let project_dir = resolver::get_project_root()?;

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir)
        .args(&["run", "format"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    println!(
        "Running Prettier via npm script: npm run format in {}",
        project_dir.display()
    );

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
            if !stdout_str.is_empty() {
                println!("'npm run format' stdout: {}", stdout_str);
            }

            // Log stderr as warning/info even on success
            if !stderr_str.is_empty() {
                println!("Warning/Info: 'npm run format' stderr: {}", stderr_str);
            }

            Ok(Vec::new())
        }
        false => Err(anyhow!(
            "'npm run format' execution failed with status {}. Stdout: '{}'. Stderr: '{}'",
            output.status,
            stdout_str,
            stderr_str
        )),
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

        let mut cmd = Command::new("npm");
        cmd.current_dir(&project_dir)
            .args(&["run", "lsp"]) // The script "lsp": "typescript-language-server --stdio"
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        println!(
            "Spawning LSP server via npm script: npm run lsp in {}",
            project_dir.display()
        );

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
                            eprintln!("LSP stdout: Scanned {} lines without finding a Content-Length header.", MAX_LINES_TO_SCAN_FOR_HEADERS);
                        }
                        break;
                    }

                    let mut line_buffer = String::new();
                    match reader.read_line(&mut line_buffer).await {
                        Ok(0) => {
                            // EOF
                            eprintln!("LSP stdout closed (EOF) while reading headers.");
                            return; // Exit task
                        }
                        Ok(_) => {
                            lines_scanned_for_this_message_headers += 1;
                            let trimmed_line = line_buffer.trim();

                            if trimmed_line.is_empty() {
                                if content_length.is_some() {
                                    break; // End of headers if Content-Length was found
                                } else {
                                    // Empty line before Content-Length, possibly initial noise.
                                    println!("LSP stdout (skipped initial empty line before Content-Length)");
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
                                println!(
                                    "LSP stdout (skipped initial non-header line): {}",
                                    trimmed_line
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading line for LSP headers: {}", e);
                            return; // Exit task
                        }
                    }
                } // End of header reading loop

                if let Some(len) = content_length {
                    let mut buffer = vec![0; len];
                    if reader.read_exact(&mut buffer).await.is_err() {
                        eprintln!(
                            "LSP stdout error reading message body or closed (from npm run lsp)."
                        );
                        return;
                    }
                    let msg_str = String::from_utf8_lossy(&buffer);
                    match serde_json::from_str::<JsonRpc>(&msg_str) {
                        Ok(rpc_msg) => {
                            if stdout_tx_clone.send(rpc_msg).await.is_err() {
                                eprintln!("Failed to send LSP message to internal channel (receiver dropped, from npm run lsp).");
                                return;
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Failed to parse LSP message (from npm run lsp): {}. Raw: '{}'",
                                e, msg_str
                            );
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
                        eprintln!(
                            "LSP message processing did not yield a Content-Length. Headers map: {:?}, Lines scanned: {}",
                            headers_map, lines_scanned_for_this_message_headers
                        );
                    }
                }
            } // End of main message reading loop
        });

        // Reader task for stderr
        tokio::spawn(async move {
            let mut stderr_reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                match stderr_reader.read_line(&mut line).await {
                    Ok(0) => {
                        eprintln!("LSP stderr closed (EOF) (from npm run lsp).");
                        return;
                    } // EOF
                    Ok(_) => {
                        eprint!("LSP stderr (from npm run lsp): {}", line); // Prefix to distinguish
                        line.clear();
                    }
                    Err(e) => {
                        eprintln!("Error reading from LSP stderr (from npm run lsp): {}", e);
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

    async fn send_rpc(&mut self, rpc: JsonRpc) -> Result<()> {
        let msg_json = serde_json::to_string(&rpc)
            .with_context(|| format!("Failed to serialize LSP RPC: {:?}", rpc))?;
        let msg_with_header = format!("Content-Length: {}\r\n\r\n{}", msg_json.len(), msg_json);

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
                            return Ok(response_candidate);
                        }
                        Some(other_id) => {
                            // It has an ID, but it's not the one we're waiting for
                            response_candidate.get_method()
                                .map_or_else(
                                    || {
                                        // Likely a response (Success or Error) to a different client-initiated request
                                        println!(
                                            "LSP Client: Received response for a different ID ({:?}) while waiting for ID {:?}. Full message: {:?}",
                                            other_id, request_id, response_candidate
                                        );
                                    },
                                    |method| {
                                        println!(
                                            "LSP Client: Received a SERVER REQUEST (Method: {}, ID: {:?}) while waiting for response to ID {:?}. This is unexpected while polling for a specific response.",
                                            method, other_id, request_id
                                        );
                                    }
                                );
                        }
                        None => {
                            // No ID typically means it's a Notification
                            let method_name = response_candidate
                                .get_method()
                                .unwrap_or("[unknown_method]");
                            
                            // Get detailed params representation
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

                            println!(
                                "LSP Client: Received notification (Method: {}) while waiting for response to ID {:?}.\nParams: {}",
                                method_name, request_id, params_detail
                            );
                        }
                    }
                }
                Ok(None) => {
                    return Err(anyhow!(
                        "LSP response channel closed while waiting for request ID {:?}",
                        request_id
                    ))
                }
                Err(_) => {
                    return Err(anyhow!(
                        "Timeout waiting for LSP response for request ID {:?}",
                        request_id
                    ))
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
            root_uri: None, // Explicitly not sending the deprecated rootUri field in the request
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
                        .context("Serialize InitializeParams error for LSP initialize")?,
                ),
            )
            .await
            .context("Sending Initialize request to LSP failed")?;

        let response_rpc = self
            .wait_for_response(&request_id, 10)
            .await
            .context("Waiting for Initialize response from LSP failed")?;

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
                uri,
                language_id: language_id.to_string(),
                version,
                text,
            },
        };
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
                        .context("Serialize GotoDefinitionParams error for LSP")?,
                ),
            )
            .await
            .context("Sending GotoDefinition request to LSP failed")?;

        let response_rpc = self
            .wait_for_response(&request_id, 5)
            .await
            .context("Waiting for GotoDefinition response from LSP failed")?;

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
