use anyhow::{anyhow, Result};
use tokio::time::{sleep, Duration};
use tracing::{error, info, span, warn, Level};
use tokio::process::Command;
use std::process::Stdio;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing_subscriber::fmt::format::FmtSpan;

/// Ensures that a given TCP port is free. If occupied, it attempts to terminate the process using 'fuser'.
pub async fn ensure_port_is_free(port: u16, service_name: &str) -> Result<()> {
    let span = span!(Level::INFO, "ensure_port_is_free", %port, service_name);
    let _enter = span.enter();

    info!(target: "galatea::terminal::port", port, service_name, "Attempting to ensure port is free using fuser command...");

    let mut cmd = Command::new("fuser");
    cmd.arg("-k")
       .arg(format!("{}/tcp", port))
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());

    match cmd.output().await {
        Ok(output) => {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code();

            let port_is_considered_free = 
                // Case 1: fuser exits successfully (code 0)
                output.status.success() || 
                // Case 2: fuser exits with code 1, and no output (common for "port not in use")
                (exit_code == Some(1) && stdout_str.is_empty() && stderr_str.is_empty()) ||
                // Case 3: fuser exits non-zero, but stderr indicates no process was found/killed
                {
                    let stderr_lc = stderr_str.to_lowercase();
                    !output.status.success() && (
                        (stderr_lc.contains("specified filename") && stderr_lc.contains("does not exist")) ||
                        stderr_lc.contains("no process found") ||
                        stderr_lc.contains("no process killed")
                    )
                };

            if port_is_considered_free {
                if output.status.success() {
                    info!(
                        target: "galatea::terminal::port",
                        port,
                        service_name,
                        exit_code = ?exit_code,
                        %stdout_str,
                        %stderr_str,
                        "fuser command successful (exit 0 - likely killed process or port was already free). Verifying port release..."
                    );
                } else if exit_code == Some(1) && stdout_str.is_empty() && stderr_str.is_empty() {
                    info!(
                        target: "galatea::terminal::port",
                        port,
                        service_name,
                        exit_code = ?exit_code,
                        "fuser command exited 1 with no stdout/stderr. Port is considered free. Verifying..."
                    );
                } else {
                    info!(
                        target: "galatea::terminal::port",
                        port,
                        service_name,
                        exit_code = ?exit_code,
                        %stdout_str,
                        %stderr_str,
                        "fuser command (non-zero exit) stderr suggests no process on port or port not found. Assuming port is free. Verifying..."
                    );
                }
                // Proceed to verification block (which is outside this if/else)
            } else {
                // fuser failed for other reasons.
                error!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    exit_code = ?exit_code,
                    %stdout_str,
                    %stderr_str,
                    "fuser command failed. Manual intervention may be required."
                );
                return Err(anyhow!(
                    "terminal::port::ensure_port_is_free: fuser command failed for port {} (service: {}). Exit code: {:?}, stdout: '{}', stderr: '{}'",
                    port, service_name, exit_code, stdout_str, stderr_str
                ));
            }
        }
        Err(e) => { // This is an error in spawning/running the command itself, e.g., fuser not found
            if e.kind() == std::io::ErrorKind::NotFound {
                error!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    error = ?e,
                    "fuser command not found. Please ensure 'fuser' (often part of 'psmisc' or 'util-linux' package) is installed and in PATH."
                );
                return Err(anyhow!(
                    "terminal::port::ensure_port_is_free: fuser command not found for port {} (service: {}). Please ensure 'fuser' is installed.",
                    port, service_name
                ).context(e));
            } else {
                error!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    error = ?e,
                    "Failed to execute fuser command."
                );
                return Err(anyhow!(e).context(format!(
                    "terminal::port::ensure_port_is_free: Failed to execute fuser for port {} (service: {})",
                    port, service_name
                )));
            }
        }
    }

    // Verification block:
    // Wait a moment for the OS to release the port if fuser acted.
    // fuser -k is generally synchronous, but a small delay can help ensure the OS has processed the signal.
    sleep(Duration::from_millis(500)).await;

    match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
        Ok(_listener) => {
            info!(target: "galatea::terminal::port", port, service_name, "Port successfully verified as free by test bind after fuser attempt.");
            Ok(())
        }
        Err(bind_err) => {
            error!(
                target: "galatea::terminal::port",
                port,
                service_name,
                error = ?bind_err,
                "Test bind failed after fuser attempt. Port may still be in use or bind failed for other reasons."
            );
            Err(anyhow!(
                "terminal::port::ensure_port_is_free: fuser was run for port {} (service: {}), but test bind failed.",
                port, service_name
            ).context(bind_err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tracing_subscriber::fmt::format::FmtSpan;

    // Helper to initialize tracing for tests
    fn init_tracing() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_span_events(FmtSpan::CLOSE) // Log when spans close
            .try_init();
    }

    // RAII guard for aborting a JoinHandle on drop
    struct AbortOnDrop<T> {
        handle: Option<JoinHandle<T>>,
    }

    impl<T> AbortOnDrop<T> {
        fn new(handle: JoinHandle<T>) -> Self {
            AbortOnDrop { handle: Some(handle) }
        }
    }

    impl<T> Drop for AbortOnDrop<T> {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
        }
    }

    const TEST_PORT_FREE_ENSURE: u16 = 49141;
    const TEST_PORT_OCCUPIED_ENSURE: u16 = 49142;

    #[tokio::test]
    async fn ensure_port_is_free_succeeds_for_free_port() -> Result<()> {
        init_tracing();
        let port = TEST_PORT_FREE_ENSURE;
        let service_name = "test_free_port_service";

        // First, ensure nothing is on this port (e.g. from a previous failed test run)
        // This call itself should succeed if the port is already free.
        ensure_port_is_free(port, &format!("{}_cleanup", service_name)).await?;

        let result = ensure_port_is_free(port, service_name).await;
        assert!(result.is_ok(), "ensure_port_is_free failed for a free port: {:?}", result.err());

        // Explicitly try to bind to verify it's truly free
        match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Ok(_listener) => {
                info!(target: "galatea::terminal::port::test", port, "Successfully bound to port after ensure_port_is_free, confirming it's free.");
                // Listener drops here, releasing the port
            }
            Err(e) => {
                return Err(anyhow!("Failed to bind to port {} after ensure_port_is_free reported it as free: {}", port, e));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn ensure_port_is_free_succeeds_and_frees_occupied_port() -> Result<()> {
        init_tracing();
        let port = TEST_PORT_OCCUPIED_ENSURE;
        let service_name = "test_occupied_port_service";

        // Ensure the port is clear before we start, in case of previous test failures
        ensure_port_is_free(port, &format!("{}_initial_cleanup", service_name)).await
            .expect("Initial cleanup failed. Port could not be freed before test.");

        // 1. Start a dummy TCP listener on the port
        let listener_handle = tokio::spawn(async move {
            let listener = match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(listener) => listener,
                Err(e) => {
                    error!(target: "galatea::terminal::port::test", port, error = ?e, "Dummy listener failed to bind.");
                    panic!("Failed to bind dummy listener on port {}: {}", port, e);
                }
            };
            info!(target: "galatea::terminal::port::test", port, "Dummy listener started. Waiting for external termination...");
            
            // Keep the listener alive until the task is aborted
            if let Err(e) = listener.accept().await {
                 if !e.to_string().contains("cancelled") { // Tokio's way of signaling abort
                    warn!(target: "galatea::terminal::port::test", port, error = ?e, "Dummy listener accept() error (not cancellation).");
                 }
            }
            info!(target: "galatea::terminal::port::test", port, "Dummy listener task ended.");
        });
        let _listener_guard = AbortOnDrop::new(listener_handle);

        // Give the listener a moment to bind.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Confirm the port is occupied by trying to bind to it ourselves. This should fail.
        let initially_occupied = match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Ok(l) => {
                drop(l); 
                false 
            }
            Err(_) => true, 
        };

        if !initially_occupied {
            return Err(anyhow!("Port {} was expected to be occupied by dummy listener, but test bind found it free. Listener might have failed to start correctly or fuser has different detection.", port));
        }
        info!(target: "galatea::terminal::port::test", port, "Confirmed port is occupied by dummy listener before calling ensure_port_is_free.");
        
        info!(target: "galatea::terminal::port::test", port, "Calling ensure_port_is_free for occupied port.");
        let result = ensure_port_is_free(port, service_name).await;
        assert!(result.is_ok(), "ensure_port_is_free failed for occupied port {}: {:?}", port, result.err());
        info!(target: "galatea::terminal::port::test", port, "ensure_port_is_free completed successfully.");

        // Explicitly try to bind to verify it was actually freed
        match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Ok(_listener) => {
                info!(target: "galatea::terminal::port::test", port, "Successfully bound to port after ensure_port_is_free, confirming it was freed.");
                // Listener drops here, releasing the port
            }
            Err(e) => {
                return Err(anyhow!("Failed to bind to port {} after ensure_port_is_free was supposed to free it: {}", port, e));
            }
        }
        
        // No need to manually abort listener_handle, _listener_guard handles it.
        Ok(())
    }
} 