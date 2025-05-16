use anyhow::{anyhow, Context, Result};
use std::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{error, info, span, warn, Level};

// Implementation detail for get_pid_on_port, wrapped by a tracing span.
fn get_pid_on_port_impl(port: u16) -> Result<Option<u32>> {
    let cmd_output = Command::new("lsof")
        .args([
            "-i",
            &format!("tcp:{}", port),
            "-s",
            "TCP:LISTEN", // Only look for listening processes
            "-t",         // Output PIDs only
            "-P",         // Do not resolve port names to strings (e.g. "http" to 80)
        ])
        .output()
        .context(format!(
            "utils::get_pid_on_port: Failed to execute lsof command for port {}",
            port
        ))?;

    let stdout = String::from_utf8_lossy(&cmd_output.stdout);
    let stderr = String::from_utf8_lossy(&cmd_output.stderr);

    if cmd_output.status.success() {
        // lsof successfully found a listening process and printed its PID.
        if let Some(pid_str) = stdout.lines().next() {
            // Take the first PID if multiple are listed (though -t usually gives one per relevant process)
            let pid = pid_str.trim().parse::<u32>().with_context(|| {
                format!(
                    "utils::get_pid_on_port: Failed to parse PID '{}' from lsof output for port {}",
                    pid_str.trim(),
                    port
                )
            })?;
            info!(target: "galatea::utils", port, pid, "Port is occupied.");
            Ok(Some(pid))
        } else {
            // This case (success status but empty stdout with -t) should be rare.
            // It might imply no process found, or an lsof quirk.
            warn!(
                target: "galatea::utils",
                port,
                "lsof succeeded for port but returned no PID. Assuming port is free."
            );
            Ok(None)
        }
    } else {
        // lsof exits with status 1 if no files are found (i.e., port is free and no process is listening).
        // For other errors, stderr might contain more info.
        if cmd_output.status.code() == Some(1) && stdout.trim().is_empty() {
            info!(target: "galatea::utils", port, "Port is free (lsof exit code 1, empty stdout).");
            Ok(None) // Port is confirmed free.
        } else {
            // Any other non-zero exit code, or exit code 1 with unexpected output.
            error!(
                target: "galatea::utils",
                port,
                status = ?cmd_output.status,
                stdout = stdout.trim(),
                stderr = stderr.trim(),
                "lsof command failed or gave unexpected output for port."
            );
            Err(anyhow!(
                "utils::get_pid_on_port: lsof command failed or gave unexpected output for port {}. Status: {}. Stdout: '{}'. Stderr: '{}'",
                port,
                cmd_output.status,
                stdout.trim(),
                stderr.trim()
            ))
        }
    }
}

/// Checks if a given TCP port is occupied and returns the PID of the process if it is.
/// Uses `lsof` and is suitable for macOS/Linux.
fn get_pid_on_port(port: u16) -> Result<Option<u32>> {
    let span = span!(Level::DEBUG, "get_pid_on_port", %port);
    let _enter = span.enter();
    get_pid_on_port_impl(port)
}

/// Terminates a process by its PID using the `kill` command.
fn terminate_process(pid: u32, port: u16, service_name: &str) -> Result<()> {
    info!(target: "galatea::utils", pid, port, service_name, "Attempting to terminate process.");
    let output = Command::new("kill")
        .arg(pid.to_string()) // Default is SIGTERM
        .output()
        .with_context(|| {
            format!(
                "utils::terminate_process: Failed to execute kill command for PID {}",
                pid
            )
        })?;

    if output.status.success() {
        info!(target: "galatea::utils", pid, port, service_name, "Successfully sent termination signal to process.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(
            target: "galatea::utils",
            pid,
            port,
            service_name,
            status = ?output.status,
            stderr = stderr.trim(),
            "Failed to kill process."
        );
        Err(anyhow!(
            "utils::terminate_process: Failed to kill process PID {} (port {}, service {}). Status: {}. Stderr: {}",
            pid,
            port,
            service_name,
            output.status,
            stderr.trim()
        ))
    }
}

/// Ensures that a given TCP port is free. If occupied, it attempts to terminate the process.
pub async fn ensure_port_is_free(port: u16, service_name: &str) -> Result<()> {
    let span = span!(Level::INFO, "ensure_port_is_free", %port, service_name);
    let _enter = span.enter();

    info!(target: "galatea::utils", "Starting check...");
    match get_pid_on_port(port) {
        Ok(Some(pid)) => {
            warn!(
                target: "galatea::utils",
                pid,
                "Port is occupied. Attempting to terminate existing process."
            );
            terminate_process(pid, port, service_name)?;

            info!(target: "galatea::utils", "Waiting briefly for process to release port...");
            sleep(Duration::from_millis(1000)).await; // Wait for the port to be released

            match get_pid_on_port(port) {
                Ok(Some(new_pid)) => {
                    // If it's still occupied, especially by the same PID or a new one.
                    error!(
                        target: "galatea::utils",
                        initial_pid = pid,
                        current_pid = new_pid,
                        "Failed to free port. Process might not have terminated or another process took its place."
                    );
                    Err(anyhow!(
                        "utils::ensure_port_is_free: Failed to free port {} for {}. Initial PID: {}, current PID: {}. Process might not have terminated or another process took its place.",
                        port,
                        service_name,
                        pid,
                        new_pid
                    ))
                }
                Ok(None) => {
                    info!(target: "galatea::utils", "Port was successfully freed.");
                    Ok(())
                }
                Err(e) => {
                    // lsof failed on the re-check. This is ambiguous.
                    error!(
                        target: "galatea::utils",
                        error = ?e,
                        "Port status check after termination attempt failed. Port state uncertain."
                    );
                    Err(e.context(format!(
                        "utils::ensure_port_is_free: Port {} for {} might be free, but checking after termination failed",
                        port, service_name
                    )))
                }
            }
        }
        Ok(None) => {
            info!(target: "galatea::utils", "Port is already free.");
            Ok(())
        }
        Err(e) => {
            // Initial get_pid_on_port failed (e.g., lsof not found, or other lsof error).
            error!(
                target: "galatea::utils",
                error = ?e,
                "Initial check for port status failed."
            );
            Err(e.context(format!(
                "utils::ensure_port_is_free: Initial check for port {} status for {} failed",
                port, service_name
            )))
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

    const TEST_PORT_FREE_GET_PID: u16 = 49140;
    const TEST_PORT_FREE_ENSURE: u16 = 49141;
    const TEST_PORT_OCCUPIED_ENSURE: u16 = 49142;

    #[tokio::test]
    async fn get_pid_on_port_returns_none_for_free_port() -> Result<()> {
        init_tracing();
        let port = TEST_PORT_FREE_GET_PID;
        // Pre-condition: Ensure a stray listener isn't on this port from a previous failed test.
        // This is a bit meta, but for CI/robustness, good to try and clean up.
        // We can call ensure_port_is_free here, but that tests the tester. Or ignore for now.
        // For now, we assume the port is typically free.

        let result = get_pid_on_port(port);
        assert!(result.is_ok(), "get_pid_on_port failed: {:?}", result.err());
        assert_eq!(result.unwrap(), None, "Expected no PID for a free port {}", port);
        Ok(())
    }

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
                    error!(target: "galatea::utils::test", port, error = ?e, "Dummy listener failed to bind.");
                    panic!("Failed to bind dummy listener on port {}: {}", port, e);
                }
            };
            info!(target: "galatea::utils::test", port, "Dummy listener started. Waiting for external termination...");
            
            // This loop will keep the task alive. It will be terminated externally by `kill`
            // or by `listener_handle.abort()` at the end of the test.
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // info!(target: "galatea::utils::test", port, "Dummy listener task shutting down."); // Unreachable
        });

        // Give the listener a moment to bind and lsof to pick it up
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Check if the port is actually occupied by our listener (optional sanity check)
        match get_pid_on_port(port) {
            Ok(Some(_pid)) => {
                info!(target: "galatea::utils::test", port, "Confirmed port is occupied by dummy listener before calling ensure_port_is_free.");
            }
            Ok(None) => {
                listener_handle.abort(); // Clean up the task
                return Err(anyhow!("Port {} was expected to be occupied by dummy listener, but get_pid_on_port found it free. Listener might have failed to start correctly.", port));
            }
            Err(e) => {
                listener_handle.abort(); // Clean up the task
                return Err(e.context(format!("get_pid_on_port failed while checking dummy listener on port {}", port)));
            }
        }

        // 2. Call ensure_port_is_free for that port
        info!(target: "galatea::utils::test", port, "Calling ensure_port_is_free for occupied port.");
        let result = ensure_port_is_free(port, service_name).await;
        assert!(result.is_ok(), "ensure_port_is_free failed for occupied port {}: {:?}", port, result.err());
        info!(target: "galatea::utils::test", port, "ensure_port_is_free completed successfully.");

        // Explicitly abort the original listener task if it wasn't already (it should be)
        // to ensure cleanup if the test logic above had an early exit or issue.
        listener_handle.abort();

        Ok(())
    }
}
