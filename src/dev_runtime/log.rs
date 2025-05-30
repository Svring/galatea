use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use anyhow::{Result, anyhow};

// Added imports for file logging
use std::path::Path;
use chrono::Local;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling; // For rolling::never

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LogSource {
    // Debugger sources (original names, consider namespacing if they become too generic)
    DebuggerNpmStdout,
    DebuggerNpmStderr,
    DebuggerPnpmStdout,
    DebuggerPnpmStderr,
    DebuggerGeneral,

    // Watcher general sources - These might be deprecated by ScriptRunner ones
    WatcherEslint,
    WatcherPrettier,

    // ScriptRunner sources
    ScriptRunnerEslint,
    ScriptRunnerPrettier,

    // Watcher LSP Client sources
    WatcherLspClientRequest,
    WatcherLspClientResponse,
    WatcherLspClientNotification, 
    WatcherLspClientError,        
    WatcherLspClientLifecycle,    

    // Watcher LSP Server I/O
    WatcherLspServerStdout,
    WatcherLspServerStderr,
    WatcherLspServerLifecycle, 
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
            LogLevel::Info 
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
    pub max_entries: Option<usize>, 
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

    filtered_logs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if let Some(max) = filters.max_entries {
        if filtered_logs.len() > max {
            filtered_logs.truncate(max);
        }
    }

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

// New function to initialize file-based tracing
pub fn init_file_logger(project_root: &Path) -> Result<(NonBlocking, WorkerGuard), anyhow::Error> {
    let log_dir = project_root.join("galatea_log");
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| anyhow!("Failed to create log directory {}: {}", log_dir.display(), e))?;

    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let log_file_name = format!("galatea_run_{}.log", timestamp);

    let file_appender = rolling::never(&log_dir, &log_file_name);
    let (non_blocking_appender, guard) = tracing_appender::non_blocking(file_appender);

    Ok((non_blocking_appender, guard))
} 