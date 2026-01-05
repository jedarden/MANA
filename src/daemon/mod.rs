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
use crate::storage::{ReasoningStore, ReasoningChain, ReasoningStep};
use crate::storage::{init_global_pool, get_read_connection};
use crate::storage::{HealthMonitor, PruningConfig};

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

/// Configuration for ReasoningBank activation
#[derive(Debug, Clone)]
pub struct ReasoningBankConfig {
    /// Minimum log entries before triggering reasoning extraction
    pub min_log_entries: usize,
    /// Interval between reasoning extraction cycles (seconds)
    pub extraction_interval_secs: u64,
    /// Maximum reasoning chains to keep
    pub max_chains: usize,
}

impl Default for ReasoningBankConfig {
    fn default() -> Self {
        Self {
            min_log_entries: 50,
            extraction_interval_secs: 3600, // 1 hour
            max_chains: 1000,
        }
    }
}

/// Daemon state holding pre-loaded resources
pub struct DaemonState {
    pub conn: Connection,
    pub embedding_store: Option<EmbeddingStore>,
    pub reasoning_store: Option<ReasoningStore>,
    #[allow(dead_code)]
    pub mana_dir: PathBuf,
    pub reasoning_config: ReasoningBankConfig,
    /// Track accumulated log entries for ReasoningBank trigger
    pub log_entry_count: std::sync::atomic::AtomicUsize,
    /// Last reasoning extraction timestamp
    pub last_reasoning_extraction: std::sync::Mutex<std::time::Instant>,
    /// Health monitor for periodic pruning
    pub health_monitor: HealthMonitor,
    /// Last health check timestamp
    pub last_health_check: std::sync::Mutex<std::time::Instant>,
}

impl DaemonState {
    pub fn new(mana_dir: &Path) -> Result<Self> {
        info!("Loading pattern store with connection pooling...");
        let db_path = mana_dir.join("metadata.sqlite");

        // Initialize global connection pool for concurrent access
        init_global_pool(&db_path)?;
        info!("Connection pool initialized");

        // Keep a single read-only connection for this state object
        // (for backwards compatibility with existing code)
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

        // Initialize ReasoningBank store
        info!("Initializing ReasoningBank...");
        let reasoning_db_path = mana_dir.join("reasoning.sqlite");
        let reasoning_store = ReasoningStore::open(&reasoning_db_path).ok();

        if reasoning_store.is_some() {
            info!("ReasoningBank initialized successfully");
        } else {
            warn!("ReasoningBank not available");
        }

        Ok(Self {
            conn,
            embedding_store,
            reasoning_store,
            mana_dir: mana_dir.to_path_buf(),
            reasoning_config: ReasoningBankConfig::default(),
            log_entry_count: std::sync::atomic::AtomicUsize::new(0),
            last_reasoning_extraction: std::sync::Mutex::new(std::time::Instant::now()),
            health_monitor: HealthMonitor::new(PruningConfig::default()),
            last_health_check: std::sync::Mutex::new(std::time::Instant::now()),
        })
    }

    /// Check if health pruning should run (every 6 hours)
    pub fn should_run_health_check(&self) -> bool {
        let elapsed = self.last_health_check
            .lock()
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        // Run health check every 6 hours
        elapsed >= 6 * 3600
    }

    /// Run health check and auto-pruning if needed
    pub fn run_health_check(&self) -> Result<usize> {
        info!("Running periodic health check...");

        // Note: We need a writable connection for pruning
        // The daemon's conn is read-only, so we need to open a new one
        let db_path = self.mana_dir.join("metadata.sqlite");
        let write_conn = Connection::open(&db_path)?;

        let result = self.health_monitor.auto_prune(&write_conn)?;

        // Update last check time
        if let Ok(mut last) = self.last_health_check.lock() {
            *last = std::time::Instant::now();
        }

        info!("Health check complete: deleted={}, decayed={}, health={:.1}%",
              result.patterns_deleted,
              result.patterns_decayed,
              result.after_health.health_score * 100.0);

        Ok(result.patterns_deleted + result.patterns_decayed)
    }

    /// Check if ReasoningBank should be activated based on accumulated logs
    pub fn should_activate_reasoning(&self) -> bool {
        let count = self.log_entry_count.load(std::sync::atomic::Ordering::Relaxed);
        let elapsed = self.last_reasoning_extraction
            .lock()
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        // Activate if we have enough log entries AND enough time has passed
        count >= self.reasoning_config.min_log_entries
            && elapsed >= self.reasoning_config.extraction_interval_secs
    }

    /// Increment log entry count (called when processing session logs)
    pub fn increment_log_count(&self, count: usize) {
        self.log_entry_count.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
    }

    /// Run ReasoningBank extraction from accumulated logs
    pub fn run_reasoning_extraction(&self) -> Result<usize> {
        let reasoning_store = match &self.reasoning_store {
            Some(store) => store,
            None => return Ok(0),
        };

        info!("Running ReasoningBank extraction...");

        // Get Claude logs directory
        let claude_logs = dirs::home_dir()
            .map(|h| h.join(".claude").join("logs"))
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;

        if !claude_logs.exists() {
            debug!("No Claude logs directory found");
            return Ok(0);
        }

        let mut chains_extracted = 0;

        // Process recent log files
        let log_files: Vec<_> = std::fs::read_dir(&claude_logs)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "jsonl").unwrap_or(false))
            .collect();

        for entry in log_files.iter().take(10) {
            let path = entry.path();
            if let Ok(content) = std::fs::read_to_string(&path) {
                // Extract reasoning chains from the log content
                for line in content.lines().take(100) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(chains) = extract_reasoning_from_log(&json) {
                            for chain in chains {
                                if reasoning_store.store_chain(&chain).is_ok() {
                                    chains_extracted += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Reset counters after extraction
        self.log_entry_count.store(0, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut last) = self.last_reasoning_extraction.lock() {
            *last = std::time::Instant::now();
        }

        info!("ReasoningBank extraction complete: {} chains extracted", chains_extracted);
        Ok(chains_extracted)
    }

    /// Query ReasoningBank for relevant reasoning chains
    pub fn query_reasoning(&self, task: &str, tool_type: &str) -> Vec<ReasoningChain> {
        match &self.reasoning_store {
            Some(store) => store.find_similar(task, tool_type, 3).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Handle an inject request
    pub fn handle_inject(&self, tool: &str, input: &str) -> Result<String> {
        // For "prompt" tool type, skip daemon and use direct path
        // which has comprehensive multi-type querying and formatting
        if tool == "prompt" {
            return Err(anyhow::anyhow!("prompt tool type uses direct path"));
        }

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
        // Use pooled connection for concurrent access
        let conn = get_read_connection()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))?;

        let embed_status = if let Some(ref store) = self.embedding_store {
            let status = store.status()?;
            format!("{} vectors indexed", status.vector_count)
        } else {
            "not available".to_string()
        };

        // Get pool stats
        let pool_info = if let Some((read_stats, write_stats)) = crate::storage::get_pool_stats() {
            format!(" | Pool: R({}/{}) W({}/{})",
                read_stats.connections, read_stats.max_size,
                write_stats.connections, write_stats.max_size)
        } else {
            String::new()
        };

        Ok(format!(
            "Daemon running | {} patterns | Embeddings: {}{}",
            count, embed_status, pool_info
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
        "reasoning" => {
            // Trigger ReasoningBank extraction
            match state.run_reasoning_extraction() {
                Ok(count) => DaemonResponse::ok(Some(format!("Extracted {} reasoning chains", count))),
                Err(e) => DaemonResponse::err(format!("Reasoning extraction failed: {}", e)),
            }
        }
        "reasoning_status" => {
            // Get ReasoningBank status
            let log_count = state.log_entry_count.load(std::sync::atomic::Ordering::Relaxed);
            let should_activate = state.should_activate_reasoning();
            let chain_count = state.reasoning_store
                .as_ref()
                .and_then(|s| s.count().ok())
                .unwrap_or(0);

            DaemonResponse::ok(Some(format!(
                "ReasoningBank: {} chains | {} pending logs | activation: {}",
                chain_count, log_count, if should_activate { "ready" } else { "waiting" }
            )))
        }
        "health_check" => {
            // Trigger health check and pruning
            match state.run_health_check() {
                Ok(affected) => DaemonResponse::ok(Some(format!(
                    "Health check complete: {} patterns affected", affected
                ))),
                Err(e) => DaemonResponse::err(format!("Health check failed: {}", e)),
            }
        }
        "health_status" => {
            // Get health status without pruning
            let db_path = state.mana_dir.join("metadata.sqlite");
            match Connection::open(&db_path) {
                Ok(conn) => {
                    match state.health_monitor.check_health(&conn) {
                        Ok(status) => {
                            let should_run = state.should_run_health_check();
                            DaemonResponse::ok(Some(format!(
                                "Health: {:.1}% ({}) | Next check: {}",
                                status.health_score * 100.0,
                                if status.is_healthy { "healthy" } else { "unhealthy" },
                                if should_run { "now" } else { "later" }
                            )))
                        }
                        Err(e) => DaemonResponse::err(format!("Failed to check health: {}", e)),
                    }
                }
                Err(e) => DaemonResponse::err(format!("Failed to open database: {}", e)),
            }
        }
        "ping" => DaemonResponse::ok(Some("pong".to_string())),
        "shutdown" => {
            info!("Shutdown requested");
            DaemonResponse::ok(Some("shutting down".to_string()))
        }
        _ => DaemonResponse::err(format!("Unknown command: {}", req.command)),
    }
}

/// Extract reasoning chains from a log JSON entry
fn extract_reasoning_from_log(json: &serde_json::Value) -> Option<Vec<ReasoningChain>> {
    let mut chains = Vec::new();

    // Look for thinking blocks in the log
    if let Some(thinking) = json.get("thinking").and_then(|t| t.as_str()) {
        if thinking.len() > 50 {
            // Extract reasoning steps from thinking
            let steps = extract_steps_from_thinking(thinking);
            if steps.len() >= 2 {
                let task = json.get("task")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown task")
                    .to_string();

                let tool_type = json.get("tool")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let success = json.get("success")
                    .and_then(|s| s.as_bool())
                    .unwrap_or(true);

                chains.push(ReasoningChain {
                    id: 0,
                    pattern_id: None,
                    task,
                    tool_type,
                    outcome: if success { "success" } else { "failure" }.to_string(),
                    steps,
                    summary: thinking.lines().next().unwrap_or("").to_string(),
                    success_count: if success { 1 } else { 0 },
                    failure_count: if success { 0 } else { 1 },
                    created_at: String::new(),
                });
            }
        }
    }

    // Also look for explicit reasoning blocks
    if let Some(reasoning) = json.get("reasoning").and_then(|r| r.as_array()) {
        for step_json in reasoning {
            if let (Some(step_type), Some(content)) = (
                step_json.get("type").and_then(|t| t.as_str()),
                step_json.get("content").and_then(|c| c.as_str()),
            ) {
                // Individual reasoning steps can form chains
                let _ = (step_type, content); // Used in more complete implementation
            }
        }
    }

    if chains.is_empty() {
        None
    } else {
        Some(chains)
    }
}

/// Extract reasoning steps from thinking content
fn extract_steps_from_thinking(thinking: &str) -> Vec<ReasoningStep> {
    let mut steps = Vec::new();
    let mut step_num = 0;

    for line in thinking.lines() {
        let line = line.trim();
        if line.is_empty() || line.len() < 10 {
            continue;
        }

        // Detect thought patterns
        let (step_type, confidence) = if line.starts_with("I'll")
            || line.starts_with("Let me")
            || line.starts_with("First,")
            || line.starts_with("I need to") {
            ("thought", 0.8)
        } else if line.contains("found")
            || line.contains("noticed")
            || line.contains("shows")
            || line.contains("see that") {
            ("observation", 0.9)
        } else if line.contains("Running")
            || line.contains("Executing")
            || line.contains("Creating")
            || line.contains("will ") {
            ("action", 1.0)
        } else if line.contains("because")
            || line.contains("since")
            || line.contains("therefore") {
            ("reflection", 0.85)
        } else {
            continue;
        };

        steps.push(ReasoningStep {
            step_number: step_num,
            step_type: step_type.to_string(),
            content: line.to_string(),
            confidence: confidence as f32,
        });
        step_num += 1;

        // Limit steps per chain
        if steps.len() >= 10 {
            break;
        }
    }

    steps
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

    // Counter for periodic health checks
    let mut loop_count = 0;

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

                // Check health every ~60 seconds (600 iterations * 100ms)
                loop_count += 1;
                if loop_count >= 600 {
                    loop_count = 0;

                    // Run health check if it's time
                    if state.should_run_health_check() {
                        info!("Running scheduled health check...");
                        match state.run_health_check() {
                            Ok(affected) => {
                                if affected > 0 {
                                    info!("Health check affected {} patterns", affected);
                                }
                            }
                            Err(e) => {
                                warn!("Scheduled health check failed: {}", e);
                            }
                        }
                    }
                }
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

/// Path for daemon start lockfile
fn start_lock_path() -> PathBuf {
    let mana_dir = crate::get_mana_dir().unwrap_or_else(|_| PathBuf::from(".mana"));
    mana_dir.join("daemon.starting")
}

/// Ensure daemon is running, spawn if not
///
/// This function spawns the daemon in the background if it's not already running.
/// Uses the classic Unix double-fork daemonization to avoid zombie processes.
/// Uses a lockfile to prevent multiple concurrent daemon starts.
/// Returns Ok(true) if daemon was started, Ok(false) if already running.
pub fn ensure_daemon_running() -> Result<bool> {
    if is_running() {
        return Ok(false);
    }

    let lock_file = start_lock_path();

    // Check if another process is already starting the daemon
    // Use file creation as a simple lock mechanism
    if lock_file.exists() {
        // Check if the lock is stale (older than 10 seconds)
        if let Ok(metadata) = std::fs::metadata(&lock_file) {
            if let Ok(modified) = metadata.modified() {
                if modified.elapsed().map(|d| d.as_secs()).unwrap_or(0) < 10 {
                    // Another process is starting the daemon, wait for it
                    debug!("Another process is starting daemon, waiting...");
                    for _ in 0..10 {
                        std::thread::sleep(Duration::from_millis(100));
                        if is_running() {
                            return Ok(false);
                        }
                    }
                    // Still not running after waiting, fall through to try starting
                }
            }
        }
        // Stale lock file, remove it
        let _ = std::fs::remove_file(&lock_file);
    }

    // Try to create the lock file atomically
    // If it already exists, another process got there first
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_file)
    {
        Ok(_) => {
            debug!("Acquired daemon start lock");
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another process beat us to it, wait for daemon
            debug!("Lost race for daemon start lock");
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(100));
                if is_running() {
                    return Ok(false);
                }
            }
            return Ok(false);
        }
        Err(_) => {
            // Can't create lock file, proceed anyway
        }
    }

    // Clean up lock file when we're done (success or failure)
    struct LockGuard(PathBuf);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _lock_guard = LockGuard(lock_file);

    debug!("Daemon not running, attempting to start...");

    // Get path to current binary
    let current_exe = std::env::current_exe()
        .context("Failed to get current executable path")?;
    debug!("Current exe: {:?}", current_exe);

    // Get MANA directory (detects binary location first for reliability)
    let mana_dir = crate::get_mana_dir()?;
    debug!("MANA dir: {:?}", mana_dir);

    // Use the classic Unix double-fork daemonization pattern to avoid zombies
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // First fork: spawn a child that will fork again and exit
        // The parent waits for this child, so no zombie
        let mut child = unsafe {
            std::process::Command::new(&current_exe)
                .arg("daemon")
                .arg("start")
                .arg("--foreground")
                .arg("--daemonize") // Signal to do second fork internally
                .current_dir(&mana_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .pre_exec(|| {
                    // Create new session so child is not tied to parent's terminal
                    libc::setsid();
                    Ok(())
                })
                .spawn()
                .context("Failed to spawn daemon child")?
        };

        // Wait for the first child (it will exit immediately after second fork)
        // This prevents zombie processes
        match child.wait() {
            Ok(status) => {
                debug!("First fork child exited with: {:?}", status);
            }
            Err(e) => {
                warn!("Failed to wait for daemon child: {}", e);
            }
        }
    }

    #[cfg(not(unix))]
    {
        // On non-Unix, just spawn directly
        let _ = std::process::Command::new(&current_exe)
            .arg("daemon")
            .arg("start")
            .arg("--foreground")
            .current_dir(&mana_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    // Wait for daemon to initialize (up to 500ms)
    for i in 0..5 {
        std::thread::sleep(Duration::from_millis(100));
        if is_running() {
            info!("Daemon auto-started successfully after {}ms", (i + 1) * 100);
            return Ok(true);
        }
    }

    // Daemon may have failed to start, but don't error - fall back to direct path
    warn!("Daemon did not start in 500ms, using direct database access");
    Ok(false)
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
