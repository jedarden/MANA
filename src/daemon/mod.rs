//! MANA Daemon Module
//!
//! Provides a long-running background process that keeps the pattern store
//! and embedding index in memory for faster context injection.
//!
//! Architecture:
//! - Unix socket server accepting JSON requests
//! - In-memory pattern cache with lazy loading
//! - Background learning and consolidation
//!
//! Protocol:
//! - Request: JSON object with "command" field
//! - Response: JSON object with "success" and "data" fields

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::embeddings::EmbeddingStore;
use crate::storage::calculate_similarity;

/// Socket path for daemon communication
pub fn socket_path() -> PathBuf {
    let mana_dir = crate::get_mana_dir().unwrap_or_else(|_| PathBuf::from(".mana"));
    mana_dir.join("daemon.sock")
}

/// PID file path for daemon process tracking
pub fn pid_path() -> PathBuf {
    let mana_dir = crate::get_mana_dir().unwrap_or_else(|_| PathBuf::from(".mana"));
    mana_dir.join("daemon.pid")
}

/// Request from client to daemon
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
}

/// Response from daemon to client
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResponse {
    pub fn ok(data: Option<String>) -> Self {
        Self {
            success: true,
            data,
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Daemon state holding pre-loaded resources
pub struct DaemonState {
    pub conn: Connection,
    pub embedding_store: Option<EmbeddingStore>,
    #[allow(dead_code)]
    pub mana_dir: PathBuf,
}

impl DaemonState {
    pub fn new(mana_dir: &Path) -> Result<Self> {
        info!("Loading pattern store...");
        let db_path = mana_dir.join("metadata.sqlite");

        // Open connection with mmap for fast repeated queries
        let conn = Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "mmap_size", 2_097_152)?; // 2MB mmap
        conn.set_prepared_statement_cache_capacity(8);

        info!("Loading embedding store...");
        let embedding_store = EmbeddingStore::open(mana_dir).ok();

        if embedding_store.is_some() {
            info!("Embedding store loaded successfully");
        } else {
            warn!("Embedding store not available");
        }

        Ok(Self {
            conn,
            embedding_store,
            mana_dir: mana_dir.to_path_buf(),
        })
    }

    /// Handle an inject request
    pub fn handle_inject(&self, tool: &str, input: &str) -> Result<String> {
        // Map tool argument to database tool_types
        let db_tool_type = match tool {
            "edit" => "Edit",
            "bash" => "Bash",
            "task" => "Task",
            "read" => "Read",
            _ => tool,
        };

        // Extract a query from the input for similarity matching
        let query = extract_query_from_input(input, tool);

        // Search for relevant patterns
        let mut patterns = Vec::new();

        // Try embedding search first
        if let Some(ref embed_store) = self.embedding_store {
            if let Ok(results) = embed_store.search_with_context(&query, 5) {
                for m in results {
                    let rate = m.success_rate() * 100.0;
                    patterns.push(format!(
                        "- **{}** (score: {}, {:.0}% success rate)\n  {}",
                        m.tool_type,
                        m.id,
                        rate,
                        truncate_context(&m.context_query, 100)
                    ));
                }
            }
        }

        // Fall back to similarity search
        if patterns.is_empty() {
            if let Ok(mut stmt) = self.conn.prepare(
                "SELECT tool_type, context_query, success_count, failure_count
                 FROM patterns
                 WHERE tool_type = ?1
                 ORDER BY (success_count - failure_count) DESC
                 LIMIT 10",
            ) {
                if let Ok(rows) = stmt.query_map([db_tool_type], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                }) {
                    for row in rows.flatten() {
                        let (tool_type, context_query, success, failure) = row;
                        let score = success - failure;
                        let rate = if success + failure > 0 {
                            (success as f64 / (success + failure) as f64) * 100.0
                        } else {
                            0.0
                        };

                        // Filter by similarity
                        let sim = calculate_similarity(&query, &context_query);
                        if sim > 0.35 {
                            patterns.push(format!(
                                "- **{}** (score: {}, {:.0}% success rate)\n  {}",
                                tool_type, score, rate,
                                truncate_context(&context_query, 100)
                            ));

                            if patterns.len() >= 3 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Build response
        if patterns.is_empty() {
            Ok(input.to_string())
        } else {
            let context_block = format!(
                "<mana-context>\n**Relevant patterns from previous successful operations:**\n\n{}\n</mana-context>\n\n{}",
                patterns.join("\n\n"),
                input
            );
            Ok(context_block)
        }
    }

    /// Handle a status request
    pub fn handle_status(&self) -> Result<String> {
        let count: i64 = self.conn.query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))?;

        let embed_status = if let Some(ref store) = self.embedding_store {
            let status = store.status()?;
            format!("{} vectors indexed", status.vector_count)
        } else {
            "not available".to_string()
        };

        Ok(format!(
            "Daemon running | {} patterns | Embeddings: {}",
            count, embed_status
        ))
    }
}

/// Extract a search query from the input JSON
fn extract_query_from_input(input: &str, tool: &str) -> String {
    // Try to parse as JSON and extract relevant fields
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(input) {
        // Check nested 'input' structure (Claude Code format)
        if let Some(inner) = json.get("input") {
            if let Some(cmd) = inner.get("command").and_then(|v| v.as_str()) {
                let first_word = cmd.split_whitespace().next().unwrap_or("");
                return format!("Bash {}", first_word);
            }
            if let Some(path) = inner.get("file_path").and_then(|v| v.as_str()) {
                let ext = path.rsplit('.').next().unwrap_or("unknown");
                let filename = path.rsplit('/').next().unwrap_or(path);
                return format!("Editing {} file {}", ext, filename);
            }
        }

        // Try flat structure
        if let Some(cmd) = json.get("command").and_then(|v| v.as_str()) {
            let first_word = cmd.split_whitespace().next().unwrap_or("");
            return format!("Bash {}", first_word);
        }
        if let Some(path) = json.get("file_path").and_then(|v| v.as_str()) {
            let ext = path.rsplit('.').next().unwrap_or("unknown");
            let filename = path.rsplit('/').next().unwrap_or(path);
            return format!("Editing {} file {}", ext, filename);
        }
    }

    // Fallback
    format!("Tool: {}", tool)
}

/// Truncate context for display
fn truncate_context(s: &str, max_len: usize) -> String {
    // Take first line only for cleaner display
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max_len.saturating_sub(3)])
    }
}

/// Handle a single client connection
fn handle_client(mut stream: UnixStream, state: &DaemonState) {
    let peer = stream.peer_addr().ok();
    debug!("Client connected: {:?}", peer);

    // Set read timeout to prevent hanging
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(30))) {
        warn!("Failed to set read timeout: {}", e);
    }

    let reader = BufReader::new(stream.try_clone().expect("Failed to clone stream"));

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                debug!("Client read error: {}", e);
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<DaemonRequest>(&line) {
            Ok(req) => handle_request(&req, state),
            Err(e) => DaemonResponse::err(format!("Invalid request: {}", e)),
        };

        let response_json = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => {
                error!("Failed to serialize response: {}", e);
                continue;
            }
        };

        if let Err(e) = writeln!(stream, "{}", response_json) {
            debug!("Failed to write response: {}", e);
            break;
        }

        if let Err(e) = stream.flush() {
            debug!("Failed to flush response: {}", e);
            break;
        }
    }

    debug!("Client disconnected: {:?}", peer);
}

/// Handle a single request
fn handle_request(req: &DaemonRequest, state: &DaemonState) -> DaemonResponse {
    match req.command.as_str() {
        "inject" => {
            let tool = req.tool.as_deref().unwrap_or("Bash");
            let input = req.input.as_deref().unwrap_or("");

            match state.handle_inject(tool, input) {
                Ok(result) => DaemonResponse::ok(Some(result)),
                Err(e) => DaemonResponse::err(format!("Inject failed: {}", e)),
            }
        }
        "status" => match state.handle_status() {
            Ok(status) => DaemonResponse::ok(Some(status)),
            Err(e) => DaemonResponse::err(format!("Status failed: {}", e)),
        },
        "ping" => DaemonResponse::ok(Some("pong".to_string())),
        "shutdown" => {
            info!("Shutdown requested");
            DaemonResponse::ok(Some("shutting down".to_string()))
        }
        _ => DaemonResponse::err(format!("Unknown command: {}", req.command)),
    }
}

/// Start the daemon server
pub fn start_daemon(mana_dir: &Path) -> Result<()> {
    let socket = socket_path();
    let pid_file = pid_path();

    // Clean up stale socket
    if socket.exists() {
        std::fs::remove_file(&socket).context("Failed to remove stale socket")?;
    }

    // Write PID file
    let pid = std::process::id();
    std::fs::write(&pid_file, pid.to_string()).context("Failed to write PID file")?;

    // Load state
    info!("Initializing daemon state...");
    let state = DaemonState::new(mana_dir)?;

    // Create socket
    info!("Starting daemon on {:?}", socket);
    let listener = UnixListener::bind(&socket).context("Failed to bind socket")?;

    // Set up signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("Received shutdown signal");
        r.store(false, Ordering::SeqCst);
    })
    .context("Failed to set signal handler")?;

    info!("Daemon ready, accepting connections");

    // Set non-blocking to allow checking running flag
    listener
        .set_nonblocking(true)
        .context("Failed to set non-blocking")?;

    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Set stream to blocking for actual communication
                stream
                    .set_nonblocking(false)
                    .expect("Failed to set blocking");
                handle_client(stream, &state);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No connection pending, sleep briefly
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                error!("Accept error: {}", e);
            }
        }
    }

    // Cleanup
    info!("Daemon shutting down");
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(&pid_file);

    Ok(())
}

/// Check if daemon is running
pub fn is_running() -> bool {
    let socket = socket_path();
    if !socket.exists() {
        return false;
    }

    // Try to connect
    match UnixStream::connect(&socket) {
        Ok(mut stream) => {
            // Send ping
            let req = serde_json::json!({"command": "ping"});
            if writeln!(stream, "{}", req).is_ok() && stream.flush().is_ok() {
                let mut reader = BufReader::new(stream);
                let mut response = String::new();
                if reader.read_line(&mut response).is_ok() {
                    return response.contains("pong");
                }
            }
            false
        }
        Err(_) => false,
    }
}

/// Send a request to the daemon
pub fn send_request(req: &DaemonRequest) -> Result<DaemonResponse> {
    let socket = socket_path();

    let mut stream = UnixStream::connect(&socket).context("Failed to connect to daemon")?;

    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .context("Failed to set timeout")?;

    let req_json = serde_json::to_string(req)?;
    writeln!(stream, "{}", req_json).context("Failed to send request")?;
    stream.flush().context("Failed to flush request")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .context("Failed to read response")?;

    serde_json::from_str(&response).context("Failed to parse response")
}

/// Stop the daemon
pub fn stop_daemon() -> Result<()> {
    let socket = socket_path();
    let pid_file = pid_path();

    if !is_running() {
        anyhow::bail!("Daemon is not running");
    }

    // Send shutdown command
    let req = DaemonRequest {
        command: "shutdown".to_string(),
        tool: None,
        context: None,
        input: None,
    };

    match send_request(&req) {
        Ok(_) => {
            info!("Shutdown command sent");
        }
        Err(e) => {
            warn!("Failed to send shutdown: {}", e);
        }
    }

    // Wait briefly for graceful shutdown
    std::thread::sleep(Duration::from_millis(500));

    // Force cleanup if needed
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    if pid_file.exists() {
        std::fs::remove_file(&pid_file)?;
    }

    Ok(())
}

/// Get daemon status
pub fn daemon_status() -> String {
    if is_running() {
        let req = DaemonRequest {
            command: "status".to_string(),
            tool: None,
            context: None,
            input: None,
        };

        match send_request(&req) {
            Ok(resp) => {
                if resp.success {
                    resp.data.unwrap_or_else(|| "Running".to_string())
                } else {
                    format!("Error: {}", resp.error.unwrap_or_default())
                }
            }
            Err(e) => format!("Connection error: {}", e),
        }
    } else {
        "Not running".to_string()
    }
}

/// Inject context via daemon (fast path)
pub fn inject_via_daemon(tool: &str, input: &str) -> Result<String> {
    let req = DaemonRequest {
        command: "inject".to_string(),
        tool: Some(tool.to_string()),
        context: None,
        input: Some(input.to_string()),
    };

    let resp = send_request(&req)?;

    if resp.success {
        Ok(resp.data.unwrap_or_else(|| input.to_string()))
    } else {
        anyhow::bail!(resp.error.unwrap_or_else(|| "Unknown error".to_string()))
    }
}
