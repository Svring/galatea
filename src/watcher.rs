use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use anyhow::{anyhow, Context, Result};

use jsonrpc_lite::{Id, JsonRpc, Params};
use lsp_types::notification::Notification as LspNotificationTrait;
use lsp_types::request::Request as LspRequestTrait;
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, InitializeParams,
    PartialResultParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri as LspUri, WorkDoneProgressParams, WorkspaceFolder,
};

use crate::resolver;

// --- Project and Dependency Management ---

async fn run_npm_command(
    project_dir: &Path,
    args: &[&str],
    suppress_output: bool,
) -> Result<()> {
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
        Err(anyhow!(
            "npm command failed with status: {}.\nCommand: npm {}\nStderr: {}\nStdout: {}",
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
) -> Result<()> {
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

    run_npm_command(project_dir, &args, suppress_install_output)
        .await
        .with_context(|| format!("Failed to install npm tool '{}'", tool_name))?;
    println!("Tool '{}' installed successfully.", tool_name);
    Ok(())
}

pub async fn ensure_eslint_available() -> Result<()> {
    let project_dir = resolver::get_project_root()?;
    ensure_npm_tool_available("eslint", &project_dir, true, true)
        .await
        .context("Failed to ensure eslint is available")
}

pub async fn ensure_prettier_available() -> Result<()> {
    let project_dir = resolver::get_project_root()?;
    ensure_npm_tool_available("prettier", &project_dir, true, true)
        .await
        .context("Failed to ensure prettier is available")
}

pub async fn ensure_typescript_language_server_available() -> Result<()> {
    let project_dir = resolver::get_project_root()?;
    // typescript-language-server and its peer dependency typescript
    ensure_npm_tool_available("typescript-language-server", &project_dir, true, true)
        .await
        .context("Failed to ensure typescript-language-server is available")?;
    ensure_npm_tool_available("typescript", &project_dir, true, true)
        .await
        .context("Failed to ensure typescript (peer dependency for typescript-language-server) is available")
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

    let content = std::fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read package.json from {}", package_json_path.display()))?;

    serde_json::from_str::<PackageJsonData>(&content)
        .with_context(|| format!(
            "Failed to parse package.json content from {}\nContent: {}",
            package_json_path.display(),
            content
        ))
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
    ensure_eslint_available().await.context("ESLint not available")?;

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir);
    let args = vec!["run", "lint"];

    cmd.args(&args);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    println!(
        "Running ESLint via npm script: npm {} in {}",
        args.join(" "),
        project_dir.display()
    );

    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn 'npm run lint' for project {}", project_dir.display()))?;
    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("Failed to wait for 'npm run lint' for project {}", project_dir.display()))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    // 1. Handle explicit "no errors" message from `next lint` (or similar tools)
    if stdout_str.contains("✔ No ESLint warnings or errors") 
        || stdout_str.contains("No problems found!") // Common ESLint direct output
        || stdout_str.contains("All matched files use Prettier code style!") // For Prettier via ESLint
    {
        if output.status.success() {
            println!("ESLint reported no warnings or errors via text output.");
            return Ok(Vec::new());
        } else {
            // Unusual: success message but error status.
            return Err(anyhow!(
                "'npm run lint' reported a text-based success message but exited with status {}. Stdout: '{}'. Stderr: '{}'",
                output.status, stdout_str, stderr_str
            ));
        }
    }

    // 2. Handle cases where stdout might be empty (or only whitespace)
    if stdout_str.trim().is_empty() {
        if output.status.success() {
            println!("ESLint command succeeded with empty stdout. Assuming no lint issues.");
            // This can happen if ESLint is configured to output '[]' for no errors to a file,
            // and script redirects stdout, or if the script itself is silent on success.
            return Ok(Vec::new());
        } else {
            // Failed and produced no stdout.
            return Err(anyhow!(
                "'npm run lint' execution failed with status {} and empty stdout. Stderr: '{}'",
                output.status, stderr_str
            ));
        }
    }

    // 3. Attempt to find and parse JSON if present in non-empty stdout.
    // ESLint's JSON output is an array. Find the part of the string that is `[{...}]` or `[]`.
    let mut json_data_to_parse: Option<String> = None;
    if let Some(start_brace) = stdout_str.find('[') {
        // Try to find the corresponding end brace for the array.
        // This is a simplified heuristic. A full parser would be needed for nested structures
        // if the output wasn't just a top-level array.
        if let Some(end_brace) = stdout_str.rfind(']') {
            if end_brace >= start_brace {
                 json_data_to_parse = Some(stdout_str[start_brace..=end_brace].to_string());
            }
        }
    }
    
    if let Some(json_str_slice) = json_data_to_parse {
        match serde_json::from_str::<Vec<EslintResult>>(&json_str_slice) {
            Ok(results) => {
                println!("Successfully parsed extracted ESLint JSON output ({} results).", results.len());
                return Ok(results);
            }
            Err(e_parse) => {
                // Extracted a potential JSON slice, but it failed to parse.
                // This is more specific than failing to parse the whole stdout.
                // Fall through to trying to parse the whole stdout_str, as the slice might have been wrong.
                 eprintln!(
                    "Failed to parse the extracted JSON slice from 'npm run lint' output: {}. Slice: '{}'. Will attempt to parse full stdout.",
                    e_parse, json_str_slice
                );
            }
        }
    }

    // 4. If not the "no errors" message, not empty, and no valid JSON slice found/parsed:
    // This means the output is unexpected. Try parsing the whole stdout_str.
    // This will give the original error context if JSON is present but malformed or our slicing failed.
    match serde_json::from_str::<Vec<EslintResult>>(&stdout_str) {
        Ok(results) => {
            println!("Successfully parsed full ESLint JSON output after slice attempt failed or was skipped ({} results).", results.len());
            return Ok(results);
        }
        Err(e_full_parse) => {
             return Err(anyhow!(
                "Failed to parse ESLint output from 'npm run lint'. Output was not a known success message, not empty, and no valid JSON was found or parsable. Serde error on full output: {}. Full Stdout: '{}'. Stderr: '{}'",
                e_full_parse, stdout_str, stderr_str
            ));
        }
    }
}

// --- Formatter (Prettier) Interaction ---

pub async fn check_prettier(target_patterns: &[String]) -> Result<Vec<String>> {
    let project_dir = resolver::get_project_root()?;
    ensure_prettier_available().await.context("Prettier not available")?;

    let prettier_path = project_dir
        .join("node_modules")
        .join(".bin")
        .join("prettier");
    if !prettier_path.exists() {
        return Err(anyhow!("prettier executable not found after installation attempt."));
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
        .with_context(|| format!("Failed to spawn prettier --check for project {}", project_dir.display()))?;
    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("Failed to wait for prettier --check for project {}", project_dir.display()))?;

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
            Err(anyhow!(
                "Prettier --check failed with status: {}. Stderr: {}. Stdout: {}",
                output.status, stderr_str, stdout_str
            ))
        }
    }
}

pub async fn format_with_prettier(target_patterns: &[String]) -> Result<Vec<String>> {
    let project_dir = resolver::get_project_root()?;
    ensure_prettier_available().await.context("Prettier not available for formatting")?; // Ensures 'prettier' is available for 'npx prettier' in the script

    // The 'npm run format' script is "npx prettier ./src --write".
    // This means target_patterns from the API will be ignored.
    // Also, we can't get --list-different output from this script directly.
    if !target_patterns.is_empty() {
        println!(
            "Warning: 'format_with_prettier' called with target_patterns: {:?}, \
            but these will be ignored due to using 'npm run format' which targets './src'.",
            target_patterns
        );
    }

    let mut cmd = Command::new("npm");
    cmd.current_dir(&project_dir);
    cmd.args(&["run", "format"]);

    cmd.stdout(Stdio::piped()); // Capture stdout, though we don't expect specific file list
    cmd.stderr(Stdio::piped());

    println!(
        "Running Prettier via npm script: npm run format in {}",
        project_dir.display()
    );

    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn 'npm run format' for project {}", project_dir.display()))?;
    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("Failed to wait for 'npm run format' for project {}", project_dir.display()))?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        if !stdout_str.is_empty() {
            println!("'npm run format' stdout: {}", stdout_str); // Log any output
        }
        if !stderr_str.is_empty() {
            println!("'npm run format' stderr: {}", stderr_str); // Log any stderr even on success
        }
        // Cannot reliably return changed files, so return empty Vec.
        Ok(Vec::new())
    } else {
        Err(anyhow!(
            "'npm run format' failed with status: {}. Stdout: '{}'. Stderr: '{}'",
            output.status, stdout_str, stderr_str
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
    pub async fn new() -> Result<Self> {
        let project_dir = resolver::get_project_root()?;
        ensure_typescript_language_server_available().await
            .context("Failed to ensure typescript-language-server is available for LspClient new")?;

        let mut cmd = Command::new("npm");
        cmd.current_dir(&project_dir);
        cmd.args(&["run", "lsp"]); // The script "lsp": "typescript-language-server --stdio"

        // Stdio setup remains the same
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

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
            .with_context(|| format!("Failed to flush LSP stdin for RPC: {:?}", rpc));
        Ok(())
    }

    async fn send_request(&mut self, method: &str, params: Params) -> Result<Id> {
        let id = self.next_request_id();
        let request_obj = JsonRpc::request_with_params(id.clone(), method, params.clone());
        self.send_rpc(request_obj).await
            .with_context(|| format!("Failed to send LSP request {} with params {:?}", method, params))?;
        Ok(id)
    }

    async fn send_notification(&mut self, method: &str, params: Params) -> Result<()> {
        let notification_obj = JsonRpc::notification_with_params(method, params.clone());
        self.send_rpc(notification_obj).await
            .with_context(|| format!("Failed to send LSP notification {} with params {:?}", method, params))?;
        Ok(())
    }

    async fn wait_for_response(
        &mut self,
        request_id: &Id,
        timeout_secs: u64,
    ) -> Result<JsonRpc> {
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
        root_uri: LspUri,
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

        let response_rpc = self.wait_for_response(&request_id, 10).await
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
        uri: LspUri,
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
            Params::from(
                serde_json::to_value(params)
                    .context("Serialize DidOpenParams error")?,
            ),
        )
        .await
    }

    pub async fn goto_definition(
        &mut self,
        uri: LspUri,
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

        let response_rpc = self.wait_for_response(&request_id, 5).await
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
