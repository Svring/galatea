use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LogSource {
    // Debugger sources
    DebuggerNpmStdout,
    DebuggerNpmStderr,
    DebuggerGeneral,

    // Watcher general sources
    WatcherEslint,
    WatcherPrettier,

    // Watcher LSP Client sources
    WatcherLspClientRequest,
    WatcherLspClientResponse,
    WatcherLspClientNotification, // Server-initiated notifications received by client
    WatcherLspClientError,        // Errors specific to client logic/communication
    WatcherLspClientLifecycle,    // e.g. initialize, shutdown

    // Watcher LSP Server I/O (raw output from the LSP process)
    WatcherLspServerStdout,
    WatcherLspServerStderr,
    WatcherLspServerLifecycle, // e.g. spawn, exit
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        if level == tracing::Level::ERROR {
            LogLevel::Error
        } else if level == tracing::Level::WARN {
            LogLevel::Warn
        } else if level == tracing::Level::INFO {
            LogLevel::Info
        } else if level == tracing::Level::DEBUG {
            LogLevel::Debug
        } else if level == tracing::Level::TRACE {
            LogLevel::Trace
        } else {
            LogLevel::Info // Default
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub source: LogSource,
    pub level: LogLevel,
    pub message: String,
}

pub static SHARED_LOG_STORE: Lazy<Arc<Mutex<Vec<LogEntry>>>> =
    Lazy::new(|| Arc::new(Mutex::new(Vec::new())));

pub fn add_log_entry(source: LogSource, level: LogLevel, message: String) {
    let entry = LogEntry {
        timestamp: SystemTime::now(),
        source: source.clone(),
        level,
        message: message.clone(),
    };
    if let Ok(mut store) = SHARED_LOG_STORE.lock() {
        store.push(entry);
    } else {
        // Fallback or error logging if lock fails - for now, print to stderr
        eprintln!(
            "CRITICAL: Failed to lock SHARED_LOG_STORE to add log entry: [Source: {:?}, Level: {:?}] {}",
            source, level, message
        );
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LogFilterOptions {
    pub sources: Option<Vec<LogSource>>,
    pub levels: Option<Vec<LogLevel>>,
    pub content_contains: Option<String>,
    pub since_timestamp: Option<SystemTime>,
    pub until_timestamp: Option<SystemTime>,
    pub max_entries: Option<usize>, // Limits the number of returned entries (most recent if not time-sorted otherwise)
}

pub fn get_shared_logs(filters: LogFilterOptions) -> Result<Vec<LogEntry>> {
    let store_guard = SHARED_LOG_STORE
        .lock()
        .map_err(|_| anyhow!("Failed to acquire shared log store lock"))?;

    let mut filtered_logs: Vec<LogEntry> = store_guard
        .iter()
        .filter(|entry| {
            let mut keep = true;

            if let Some(ref allowed_sources) = filters.sources {
                if !allowed_sources.contains(&entry.source) {
                    keep = false;
                }
            }

            if keep {
                if let Some(ref allowed_levels) = filters.levels {
                    if !allowed_levels.contains(&entry.level) {
                        keep = false;
                    }
                }
            }
            
            if keep {
                if let Some(ref content_filter) = filters.content_contains {
                    if !entry.message.to_lowercase().contains(&content_filter.to_lowercase()) {
                        keep = false;
                    }
                }
            }

            if keep {
                if let Some(since) = filters.since_timestamp {
                    if entry.timestamp < since {
                        keep = false;
                    }
                }
            }
            
            if keep {
                if let Some(until) = filters.until_timestamp {
                    if entry.timestamp > until {
                        keep = false;
                    }
                }
            }
            keep
        })
        .cloned()
        .collect();

    // Sort by timestamp descending to handle max_entries correctly (get latest)
    // This also ensures a consistent order before truncation if no other sort is applied.
    filtered_logs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if let Some(max) = filters.max_entries {
        if filtered_logs.len() > max {
            filtered_logs.truncate(max);
        }
    }

    // Restore original chronological order (oldest to newest) for the selected slice
    filtered_logs.reverse();

    Ok(filtered_logs)
}

pub fn clear_shared_logs() -> Result<()> {
    let mut store_guard = SHARED_LOG_STORE
        .lock()
        .map_err(|_| anyhow!("Failed to acquire shared log store lock for clearing"))?;
    store_guard.clear();
    Ok(())
} 