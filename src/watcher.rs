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

// --- Project and Dependency Management ---

async fn run_npm_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let mut cmd = Command::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(args);

    // Configure stdout/stderr based on suppress_output flag
    match suppress_output {
        true => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
        false => {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }
    }

    println!(
        "Running command: npm {} in {}",
        args.join(" "),
        project_dir.display()
    );

    let child = cmd.spawn().with_context(|| {
        format!(
            "Failed to spawn npm command (npm {}). Ensure npm is installed and in PATH.",
            args.join(" ")
        )
    })?;

    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("Failed to wait for npm command: npm {}", args.join(" ")))?;

    match output.status.success() {
        true => {
            // Only print stdout if not suppressing output and stdout is not empty
            if !suppress_output {
                let stdout_data = String::from_utf8_lossy(&output.stdout);
                if !stdout_data.is_empty() {
                    println!("npm stdout:\n{}", stdout_data);
                }
            }
            Ok(())
        }
        false => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout); // stdout might also contain error info for npm
            Err(anyhow!(
                "npm command failed with status: {}.\nCommand: npm {}\nStderr: {}\nStdout: {}",
                output.status,
                args.join(" "),
                stderr,
                stdout
            ))
        }
    }
}

// Merged function to ensure core development dependencies are available
pub async fn ensure_dev_deps() -> Result<()> {
    let project_dir = resolver::get_project_root()?;
    let dev_dependencies_to_ensure = [
        "prettier",
        "typescript-language-server",
        "typescript", // Peer dependency for typescript-language-server
    ];

    for tool_name in dev_dependencies_to_ensure.iter() {
        let bin_path = project_dir
            .join("node_modules")
            .join(".bin")
            .join(tool_name);
        let package_path = project_dir.join("node_modules").join(tool_name);

        match bin_path.exists() || package_path.exists() {
            false => {
                println!(
                    "Development dependency '{}' not found locally. Attempting to install...",
                    tool_name
                );
                run_npm_command(
                    &project_dir,
                    &["install", "--loglevel", "error", "--save-dev", tool_name],
                    true, // suppress_output
                )
                .await
                .with_context(|| format!("Failed to install npm tool '{}'", tool_name))?;
                println!(
                    "Development dependency '{}' installed successfully.",
                    tool_name
                );
            }
            true => {
                println!(
                    "Development dependency '{}' is already installed locally in the project.",
                    tool_name
                );
            }
        }
    }
    Ok(())
}

// --- package.json Parsing ---

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PackageJsonData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub scripts: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub dependencies: std::collections::HashMap<String, String>,
    #[serde(
        default,
        rename = "devDependencies",
        skip_serializing_if = "std::collections::HashMap::is_empty"
    )]
    pub dev_dependencies: std::collections::HashMap<String, String>,
    // Add other common fields if desired, e.g.:
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>, // Can be String or a struct
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

pub async fn parse_package_json() -> Result<PackageJsonData> {
    let project_dir = resolver::get_project_root()?;
    let package_json_path = project_dir.join("package.json");

    if !package_json_path.exists() {
        return Err(anyhow!(
            "package.json not found in project directory: {}",
            project_dir.display()
        ));
    }

    let content = std::fs::read_to_string(&package_json_path).with_context(|| {
        format!(
            "Failed to read package.json from {}",
            package_json_path.display()
        )
    })?;

    serde_json::from_str::<PackageJsonData>(&content).with_context(|| {
        format!(
            "Failed to parse package.json content from {}\nContent: {}",
            package_json_path.display(),
            content
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

pub async fn run_eslint() -> Result<Vec<EslintResult>> {
    let project_dir = resolver::get_project_root()?;
    ensure_dev_deps()
        .await
        .context("Failed to ensure development dependencies for ESLint")?;

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
    ensure_dev_deps()
        .await
        .context("Failed to ensure development dependencies for Prettier format")?;

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
        ensure_dev_deps()
            .await
            .context("Failed to ensure development dependencies for LspClient new")?;

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
                let mut headers_map = std::collections::HashMap::new();
                let mut content_length: Option<usize> = None;

                loop {
                    // Header reading loop
                    let mut line_buffer = String::new();
                    match reader.read_line(&mut line_buffer).await {
                        Ok(0) => {
                            eprintln!(
                                "LSP stdout closed (EOF) while reading headers (from npm run lsp)."
                            );
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
                            eprintln!("Error reading LSP stdout headers (from npm run lsp): {}", e);
                            return;
                        }
                    }
                }

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
                    eprintln!("LSP message did not contain a valid Content-Length header (from npm run lsp). Headers received: {:?}", headers_map);
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
                    if response_candidate.get_id() == Some(request_id.clone()) {
                        return Ok(response_candidate);
                    } else {
                        // Log or handle other messages (notifications, other responses)
                        println!("LSP Client: Received non-matching or notification message while waiting for {:?}: {:?}", request_id, response_candidate);
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

    pub async fn initialize(
        &mut self,
        root_uri: Uri,
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
            root_uri: Some(root_uri), // This is still technically needed for older LSP versions or specific servers
            root_path: None,          // Deprecated in favor of root_uri and workspace_folders
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
