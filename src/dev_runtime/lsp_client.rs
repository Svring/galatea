use anyhow::{anyhow, Context, Result};
use lsp_types::notification::Notification;
use lsp_types::request::Request;
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, GotoDefinitionParams, InitializeParams,
    PartialResultParams, Position, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams, WorkspaceFolder,
};
use serde_json::Value; // For params and results
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand; // Renamed to avoid conflict if std::process::Command is used
use tokio::sync::mpsc;
use tracing;
use jsonrpc_lite::{Id, JsonRpc, Params}; // Ensure this is the only JsonRpc import

use crate::file_system;
use crate::dev_runtime::logging::{self, LogLevel, LogSource};

// --- Language Server (typescript-language-server) Interaction ---

pub struct LspClient {
    writer: tokio::process::ChildStdin,
    response_rx: mpsc::Receiver<JsonRpc>,
    request_id_counter: AtomicI64,
    child_process: tokio::process::Child, // Keep the child process to manage its lifecycle
}

impl LspClient {
    // Note: The actual spawning of the LSP server process (npm run lsp)
    // should ideally be managed by a higher-level process supervisor (e.g., in dev_runtime)
    // This `new` function will assume the process is started elsewhere and pipes are provided,
    // or it could be adapted to take a pre-spawned Child process.
    // For now, to simplify the initial move, we'll keep the spawning logic here but acknowledge it should move.
    pub async fn new() -> Result<Self> {
        let project_dir = file_system::get_project_root()?;

        let msg_spawn = format!(
            "Spawning LSP server (npm run lsp) in {}",
            project_dir.display()
        );
        logging::add_log_entry(
            LogSource::WatcherLspServerLifecycle, // TODO: Change to a new LogSource like LspRuntimeLifecycle
            LogLevel::Info,
            msg_spawn.clone(),
        );
        tracing::info!(target: "galatea::dev_runtime::lsp_client", source_process = "lsp_server_spawner", "{}", msg_spawn);

        let mut cmd = TokioCommand::new("npm");
        cmd.current_dir(&project_dir)
            .args(&["run", "lsp"]) // The script "lsp": "typescript-language-server --stdio"
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "Failed to spawn 'npm run lsp' in project dir: {}",
                project_dir.display()
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to get LSP stdin after 'npm run lsp'"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to get LSP stdout after 'npm run lsp'"))?;
        let stderr_reader = BufReader::new(
            child
                .stderr
                .take()
                .ok_or_else(|| anyhow!("Failed to get LSP stderr after 'npm run lsp'"))?,
        );

        let (response_tx, response_rx) = mpsc::channel(128);

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buffer = String::new(); // Read lines into a string buffer
            loop {
                buffer.clear();
                let mut content_length: Option<usize> = None;

                // Read headers
                loop {
                    match reader.read_line(&mut buffer).await {
                        Ok(0) => { // EOF
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Warn, "LSP stdout EOF reached while reading headers.".to_string());
                            tracing::warn!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "LSP stdout EOF reached while reading headers.");
                            return;
                        }
                        Ok(_) => {
                            let line = buffer.trim_end(); // Keep buffer for next line
                            if line.is_empty() { // Empty line signifies end of headers
                                buffer.clear(); // Clear buffer for body reading
                                break;
                            }
                            if line.starts_with("Content-Length:") {
                                if let Some(val_str) = line.split(':').nth(1) {
                                    content_length = val_str.trim().parse::<usize>().ok();
                                }
                            }
                            // Clear buffer for the next header line
                            buffer.clear();
                        }
                        Err(e) => {
                            logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Error, format!("Error reading LSP stdout headers: {}", e));
                            tracing::error!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "Error reading LSP stdout headers: {}", e);
                            return;
                        }
                    }
                }

                if let Some(len) = content_length {
                    let mut body_buffer = vec![0; len];
                    if let Err(e) = reader.read_exact(&mut body_buffer).await {
                        logging::add_log_entry(LogSource::WatcherLspServerStdout, LogLevel::Error, format!("Error reading LSP content (length {}): {}", len, e));
                        tracing::error!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "Error reading LSP content (length {}): {}", len, e);
                        continue; // Try to recover by reading next message
                    }
                    
                    match std::str::from_utf8(&body_buffer) {
                        Ok(json_str) => {
                            match serde_json::from_str::<JsonRpc>(json_str) { // Use serde_json::from_str
                                Ok(rpc) => {
                                    if response_tx.send(rpc).await.is_err() {
                                        logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, "Failed to send parsed LSP RPC to internal channel (receiver dropped).".to_string());
                                        tracing::error!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "Failed to send parsed LSP RPC to internal channel (receiver dropped).");
                                        return; // Channel closed, stop task
                                    }
                                }
                                Err(e) => {
                                    logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, format!("Error parsing LSP JSON-RPC (Content-Length: {}): {}. Content: '{}'", len, e, json_str));
                                    tracing::error!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "Error parsing LSP JSON-RPC (Content-Length: {}): {}. Content: '{}'", len, e, json_str);
                                }
                            }
                        }
                        Err(e) => {
                            logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Error, format!("LSP message body (Content-Length: {}) was not valid UTF-8: {}", len, e));
                            tracing::error!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "LSP message body (Content-Length: {}) was not valid UTF-8: {}", len, e);
                        }
                    }
                } else {
                    logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Warn, "LSP message received without Content-Length header.".to_string());
                    tracing::warn!(target: "galatea::dev_runtime::lsp_client::stdout_reader", "LSP message without Content-Length header received.");
                    // This is likely an error in message framing from the server or our reader.
                    // We might lose sync here. Consider if we should attempt to resync or just error out.
                }
            }
        });

        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr_reader).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                logging::add_log_entry(LogSource::WatcherLspServerStderr, LogLevel::Warn, format!("LSP Server stderr: {}", line));
                tracing::warn!(target: "galatea::dev_runtime::lsp_client::stderr_reader", "LSP Server: {}", line);
            }
            logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, "LSP stderr task finished.".to_string());
            tracing::info!(target: "galatea::dev_runtime::lsp_client::stderr_reader", "LSP stderr task finished.");
        });

        Ok(Self {
            writer: stdin,
            response_rx,
            request_id_counter: AtomicI64::new(0),
            child_process: child,
        })
    }

    fn next_request_id(&self) -> Id {
        Id::Num(self.request_id_counter.fetch_add(1, Ordering::SeqCst) as i64) // Id::Num takes i64
    }

    async fn send_rpc(&mut self, rpc: JsonRpc) -> Result<()> {
        let rpc_string = serde_json::to_string(&rpc).context("Failed to serialize JsonRpc to string")?; // Use serde_json::to_string
        let message = format!("Content-Length: {}\\r\\n\\r\\n{}", rpc_string.len(), rpc_string);

        logging::add_log_entry(LogSource::WatcherLspClientRequest, LogLevel::Debug, format!("Sending LSP RPC: Method '{:?}', ID '{:?}'", rpc.get_method(), rpc.get_id()));
        tracing::trace!(target: "galatea::dev_runtime::lsp_client", "Sending LSP message: {}", message);
        
        self.writer
            .write_all(message.as_bytes())
            .await
            .context("Failed to write to LSP stdin")
    }

    async fn send_request(&mut self, method: &str, params_value: Value) -> Result<Id> {
        let id = self.next_request_id();
        let params = Params::from(params_value);
        let rpc = JsonRpc::request_with_params(id.clone(), method, params.clone());
        self.send_rpc(rpc).await.with_context(|| {
            format!(
                "Failed to send LSP request {} with params {:?}",
                method, params
            )
        })?;
        Ok(id)
    }

    async fn send_notification(&mut self, method: &str, params_value: Value) -> Result<()> {
        let params = Params::from(params_value);
        let rpc = JsonRpc::notification_with_params(method, params.clone());
        self.send_rpc(rpc).await.with_context(|| {
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
                  serde_json::to_value(params)
                      .context("Serialize InitializeParams error for LSP initialize")?,
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
          serde_json::to_value(params).context("Serialize DidOpenParams error")?,
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
                  serde_json::to_value(params)
                      .context("Serialize GotoDefinitionParams error for LSP")?,
              
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
        logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, "Closing LSP client and attempting to kill server process.".to_string());
        tracing::info!(target: "galatea::dev_runtime::lsp_client", "Closing LSP client and attempting to kill server process.");
        
        let exit_params_value = serde_json::Value::Null;
        let params = Params::from(exit_params_value);
        let rpc = JsonRpc::notification_with_params(lsp_types::notification::Exit::METHOD, params.clone());
        if let Err(e) = self.send_rpc(rpc).await {
            logging::add_log_entry(LogSource::WatcherLspClientError, LogLevel::Warn, format!("Failed to send exit notification to LSP server (proceeding with kill): {}",e));
            tracing::warn!(target: "galatea::dev_runtime::lsp_client", "Failed to send exit notification to LSP server: {}", e);
        }

        drop(self.writer);

        match self.child_process.try_wait() {
            Ok(Some(status)) => {
                logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, format!("LSP server process exited with status: {}", status));
                tracing::info!(target: "galatea::dev_runtime::lsp_client", "LSP server process already exited with status: {}", status);
            }
            Ok(None) => {
                tracing::info!(target: "galatea::dev_runtime::lsp_client", "LSP server process still running, attempting to kill.");
                if let Err(e) = self.child_process.kill().await {
                    logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Error, format!("Failed to kill LSP server process: {}", e));
                    return Err(anyhow!("Failed to kill LSP server process: {}", e));
                } else {
                    logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Info, "LSP server process killed successfully.".to_string());
                    tracing::info!(target: "galatea::dev_runtime::lsp_client", "LSP server process killed successfully.");
                }
            }
            Err(e) => {
                logging::add_log_entry(LogSource::WatcherLspServerLifecycle, LogLevel::Error, format!("Error checking LSP server process status: {}", e));
                return Err(anyhow!("Error checking LSP server process status: {}", e));
            }
        }
        Ok(())
    }
} 