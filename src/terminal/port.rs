use anyhow::{anyhow, Result};
use tokio::time::{sleep, Duration};
use tracing::{error, info, span, warn, Level};
use port_killer::kill as kill_processes_on_port; // Import port_killer

/// Ensures that a given TCP port is free. If occupied, it attempts to terminate the process.
pub async fn ensure_port_is_free(port: u16, service_name: &str) -> Result<()> {
    let span = span!(Level::INFO, "ensure_port_is_free", %port, service_name);
    let _enter = span.enter();

    info!(target: "galatea::terminal::port", "Attempting to ensure port is free using port_killer...");

    match kill_processes_on_port(port) {
        Ok(killed) => {
            if killed {
                info!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    "port_killer: Found and attempted to kill process(es) on port. Verifying port release..."
                );
                // Wait a moment for the OS to release the port
                sleep(Duration::from_millis(1000)).await;

                // Verify by trying to bind
                match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                    Ok(_listener) => {
                        info!(target: "galatea::terminal::port", port, service_name, "port_killer: Port successfully freed and verified by test bind.");
                        Ok(())
                    }
                    Err(bind_err) => {
                        error!(
                            target: "galatea::terminal::port",
                            port,
                            service_name,
                            error = ?bind_err,
                            "port_killer: Port was reportedly killed, but test bind failed. Port may still be in use or bind failed for other reasons."
                        );
                        Err(anyhow!(
                            "terminal::port::ensure_port_is_free: port_killer attempted to free port {} for {}, but it's still in use or test bind failed.",
                            port, service_name
                        ).context(bind_err))
                    }
                }
            } else {
                info!(target: "galatea::terminal::port", port, service_name, "port_killer: No process was found on port. Port is free.");
                Ok(())
            }
        }
        Err(e) => {
            let err_string = e.to_string().to_lowercase();
            warn!(
                target: "galatea::terminal::port",
                port,
                service_name,
                error_msg = %err_string,
                "port_killer::kill failed. Checking for missing utilities before fallback."
            );

            // Heuristic check for missing underlying tools (lsof, ss, etc.)
            // port_killer errors might include phrases like "No such file or directory", "command not found", 
            // or specific messages if it fails to find necessary system utilities.
            if err_string.contains("no such file or directory") || 
               err_string.contains("command not found") ||
               err_string.contains("lsof") || // If lsof is mentioned in an error, it might be missing/failing
               err_string.contains("netstat") || // port_killer might use netstat too
               err_string.contains("ss") // or ss
            {
                warn!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    "port_killer error suggests missing system utilities. Attempting fallback: direct bind test."
                );
                // Fallback: Try to bind to the port to check if it's free
                match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                    Ok(_listener) => {
                        info!(target: "galatea::terminal::port", port, service_name, "Fallback check: Port appears to be free (bind successful).");
                        Ok(())
                    }
                    Err(bind_err) => {
                        error!(
                            target: "galatea::terminal::port",
                            port,
                            service_name,
                            error = ?bind_err,
                            "Fallback check: Port appears to be in use or binding failed. Manual intervention may be required."
                        );
                        Err(anyhow!(
                            "terminal::port::ensure_port_is_free: Fallback check for port {} (service: {}) failed. Port seems occupied or binding failed (original port_killer error: {}).",
                            port, service_name, e
                        ).context(bind_err))
                    }
                }
            } else {
                // port_killer failed for other reasons (e.g., permissions, other internal error)
                error!(
                    target: "galatea::terminal::port",
                    port,
                    service_name,
                    error = ?e,
                    "port_killer::kill failed for a reason not suspected to be missing utilities."
                );
                Err(anyhow!(e).context(format!(
                    "terminal::port::ensure_port_is_free: port_killer::kill failed for port {} (service: {})",
                    port, service_name
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use tokio::net::TcpListener;
    use tracing_subscriber::fmt::format::FmtSpan;

    // Helper to initialize tracing for tests
    fn init_tracing() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_span_events(FmtSpan::CLOSE) // Log when spans close
            .try_init();
    }

    const TEST_PORT_FREE_ENSURE: u16 = 49141;
    const TEST_PORT_OCCUPIED_ENSURE: u16 = 49142;

    #[tokio::test]
    async fn ensure_port_is_free_succeeds_for_free_port() -> Result<()> {
        init_tracing();
        let port = TEST_PORT_FREE_ENSURE;
        let result = ensure_port_is_free(port, "test_free_port_service").await;
        assert!(result.is_ok(), "ensure_port_is_free failed for a free port: {:?}", result.err());
        Ok(())
    }

    #[tokio::test]
    async fn ensure_port_is_free_succeeds_and_frees_occupied_port() -> Result<()> {
        init_tracing();
        let port = TEST_PORT_OCCUPIED_ENSURE;
        let service_name = "test_occupied_port_service";

        // 1. Start a dummy TCP listener on the port
        let listener_handle = tokio::spawn(async move {
            let _listener = match TcpListener::bind(format!("127.0.0.1:{}", port)).await {
                Ok(listener) => listener,
                Err(e) => {
                    error!(target: "galatea::terminal::port::test", port, error = ?e, "Dummy listener failed to bind.");
                    panic!("Failed to bind dummy listener on port {}: {}", port, e);
                }
            };
            info!(target: "galatea::terminal::port::test", port, "Dummy listener started. Waiting for external termination...");
            
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Give the listener a moment to bind and for port_killer to be able to find it
        tokio::time::sleep(Duration::from_millis(500)).await;

        let initially_occupied = match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Ok(l) => {
                drop(l); 
                false 
            }
            Err(_) => true, 
        };

        if !initially_occupied {
            listener_handle.abort();
            return Err(anyhow!("Port {} was expected to be occupied by dummy listener, but test bind found it free. Listener might have failed to start correctly or port_killer has different detection.", port));
        }
        info!(target: "galatea::terminal::port::test", port, "Confirmed port is occupied by dummy listener before calling ensure_port_is_free.");
        
        info!(target: "galatea::terminal::port::test", port, "Calling ensure_port_is_free for occupied port.");
        let result = ensure_port_is_free(port, service_name).await;
        assert!(result.is_ok(), "ensure_port_is_free failed for occupied port {}: {:?}", port, result.err());
        info!(target: "galatea::terminal::port::test", port, "ensure_port_is_free completed successfully.");

        listener_handle.abort();

        Ok(())
    }
} 