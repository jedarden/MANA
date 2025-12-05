//! Foreground learning - quick pattern extraction
//!
//! Runs synchronously after session-end when threshold is reached.
//! Latency budget: <1 second.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info};

use super::trajectory::{parse_trajectories, Trajectory};
use super::LearningResult;
use crate::storage::{PatternStore, Pattern, CausalStore};
use crate::hooks::session_end_handler::AccumulatorState;

/// Maximum patterns to extract per trajectory (ReasoningBank constraint)
const MAX_PATTERNS_PER_TRAJECTORY: usize = 3;

/// Run foreground learning on accumulated trajectories
///
/// Extracts patterns from JSONL logs and stores them in the ReasoningBank.
/// This runs synchronously and should complete in <1 second.
///
/// OPTIMIZATION: Uses batch deduplication to reduce DB queries from O(n) to O(1)
/// where n is the number of patterns extracted. Previously each pattern required
/// a DB query + similarity calculations; now we deduplicate in-memory first.
///
/// IMPORTANT: Uses last_file_positions to only process NEW trajectories,
/// preventing score inflation from repeatedly processing the same data.
pub async fn foreground_learn(pending_files: &[PathBuf]) -> Result<LearningResult> {
    let start = Instant::now();

    info!("Starting foreground learning with {} pending files", pending_files.len());

    let mut result = LearningResult::default();

    // Get MANA data directory
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");
    let mut store = PatternStore::open(&db_path)?;

    // Load learning state to get file positions
    let state_path = mana_dir.join("learning-state.json");
    let state = AccumulatorState::load(&state_path)?;

    // Parse trajectories from all JSONL files in Claude logs
    let claude_logs = get_claude_logs_dir();
    if !claude_logs.exists() {
        info!("Claude logs directory not found, skipping learning");
        return Ok(result);
    }

    // Collect all JSONL files
    let jsonl_files = collect_jsonl_files(&claude_logs)?;
    info!("Found {} JSONL files to process", jsonl_files.len());

    // Track which files we actually processed (for updating positions)
    let mut new_positions: HashMap<PathBuf, u64> = HashMap::new();

    // Parse trajectories - USING STORED POSITIONS to only get new data
    let mut all_trajectories = Vec::new();
    for file in &jsonl_files {
        // Get the last processed position for this file (0 if never processed)
        let start_offset = state.last_file_positions
            .get(file)
            .copied()
            .unwrap_or(0);

        // Get current file size to track new position
        let file_len = std::fs::metadata(file)
            .map(|m| m.len())
            .unwrap_or(0);

        // Skip if we've already processed to the end
        if start_offset >= file_len {
            continue;
        }

        match parse_trajectories(file, start_offset) {
            Ok(trajectories) => {
                if !trajectories.is_empty() {
                    debug!("Parsed {} new trajectories from {:?} (offset {} -> {})",
                           trajectories.len(), file, start_offset, file_len);
                    all_trajectories.extend(trajectories);
                }
                // Record new position
                new_positions.insert(file.clone(), file_len);
            }
            Err(e) => {
                debug!("Failed to parse {:?}: {}", file, e);
            }
        }
    }

    info!("Parsed {} trajectories total", all_trajectories.len());

    // OPTIMIZATION: Collect all patterns first, then batch-deduplicate in memory
    // This reduces DB queries from O(n) to O(1) and avoids repeated similarity calculations
    let mut all_patterns: Vec<Pattern> = Vec::new();
    let mut edit_count = 0;
    let mut bash_count = 0;

    for trajectory in all_trajectories.iter().take(100) {
        // Extract patterns from individual successful tool calls
        let patterns = extract_per_tool_patterns(trajectory);
        for pattern in patterns {
            match pattern.tool_type.as_str() {
                "Edit" => edit_count += 1,
                "Bash" => bash_count += 1,
                _ => {}
            }
            all_patterns.push(pattern);
        }

        // Also extract failure patterns from error results
        let failure_patterns = extract_failure_patterns(trajectory);
        all_patterns.extend(failure_patterns);

        result.trajectories_processed += 1;
    }

    info!("Extracted {} Edit patterns, {} Bash patterns", edit_count, bash_count);

    // OPTIMIZATION: In-memory deduplication before DB insertion
    // Uses hash-based deduplication for O(1) lookup instead of O(n) similarity checks
    let dedupe_start = Instant::now();
    let deduplicated = deduplicate_patterns_fast(all_patterns);
    debug!("Deduplicated {} patterns to {} unique in {}ms",
           edit_count + bash_count, deduplicated.len(), dedupe_start.elapsed().as_millis());

    // OPTIMIZATION: Batch insert in single transaction for 10-100x speedup
    let insert_start = Instant::now();
    result.patterns_created = store.insert_batch(&deduplicated)? as u32;
    debug!("Batch inserted {} patterns in {}ms", result.patterns_created, insert_start.elapsed().as_millis());

    // Discover causal edges from pattern co-occurrences
    let causal_edges = discover_causal_edges(&db_path, &all_trajectories)?;
    if causal_edges > 0 {
        info!("Discovered {} causal edges from co-occurrences", causal_edges);
    }

    // Log learning event to database
    log_learning_event(&db_path, &result)?;

    // Update file positions in the learning state
    // This ensures we don't reprocess the same trajectories
    if !new_positions.is_empty() {
        let mut updated_state = state;
        updated_state.last_file_positions.extend(new_positions);
        if let Err(e) = updated_state.save(&state_path) {
            debug!("Failed to save updated file positions: {}", e);
        } else {
            debug!("Updated file positions for {} files", updated_state.last_file_positions.len());
        }
    }

    result.duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "Foreground learning complete: {} patterns created from {} trajectories in {}ms",
        result.patterns_created, result.trajectories_processed, result.duration_ms
    );

    Ok(result)
}

/// Fast in-memory pattern deduplication using hash-based grouping
///
/// Groups patterns by (tool_type, command_category) and keeps only unique ones.
/// Uses pattern_hash for exact duplicate detection, avoiding expensive similarity calculations.
/// This reduces the number of DB insertions significantly.
fn deduplicate_patterns_fast(patterns: Vec<Pattern>) -> Vec<Pattern> {
    use std::collections::HashSet;

    let mut seen_hashes: HashSet<String> = HashSet::with_capacity(patterns.len() / 10);
    let mut unique: Vec<Pattern> = Vec::with_capacity(patterns.len() / 10);

    for pattern in patterns {
        // Use pattern_hash for exact deduplication (O(1) lookup)
        if seen_hashes.insert(pattern.pattern_hash.clone()) {
            unique.push(pattern);
        }
    }

    unique
}

/// Extract patterns from individual tool calls regardless of overall trajectory success
/// This allows learning from successful Edit/Write calls in mixed sessions
fn extract_per_tool_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Build a map of tool_use_id -> error status from results
    // Currently not filtering by this, as we extract all successful tool patterns
    let _error_tool_ids: std::collections::HashSet<String> = trajectory.tool_results
        .iter()
        .filter(|r| r.is_error ||
                r.content.to_lowercase().contains("error:") ||
                r.content.to_lowercase().contains("failed:"))
        .map(|r| r.tool_use_id.clone())
        .collect();

    // Extract task category for context
    let task_category = extract_task_category(&trajectory.user_query);

    // Create patterns for each tool call (not limited - we deduplicate later)
    // Don't limit here because Edit calls often come after initial Bash commands
    for tool_call in trajectory.tool_calls.iter() {
        // For tools that produce patterns we care about
        match tool_call.tool_name.as_str() {
            "Edit" | "Write" | "MultiEdit" | "Bash" | "Task" | "Read" | "Grep" | "Glob" => {
                // Extract meaningful context from tool input
                let tool_context = extract_tool_context(&tool_call.tool_name, &tool_call.tool_input);

                // Only create pattern if context is meaningful
                if tool_context.len() < 10 {
                    continue;
                }

                // Extract command category for better filtering
                let command_category = extract_command_category(&tool_call.tool_name, &tool_call.tool_input);

                let context_query = format!(
                    "Task: {}\nApproach: {} - {}\nOutcome: Success",
                    task_category,
                    tool_call.tool_name,
                    tool_context
                );

                let pattern_hash = hash_string(&context_query);

                patterns.push(Pattern {
                    id: 0,  // Will be set by database
                    pattern_hash,
                    tool_type: tool_call.tool_name.clone(),
                    command_category,
                    context_query,
                    success_count: 1,
                    failure_count: 0,
                    embedding_id: None,
                });
            }
            _ => continue,
        }
    }

    patterns
}

/// Extract command category for grouping similar patterns
/// For Bash: returns the primary command (cargo, npm, git, etc.)
/// For Edit/Write: returns the file extension (rs, ts, py, etc.)
fn extract_command_category(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Bash" => {
            let cmd = input.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let first_word = cmd.split_whitespace().next().unwrap_or("");

            // Normalize common commands to categories
            let category = match first_word {
                // Rust ecosystem
                "cargo" | "rustc" | "rustup" | "rustfmt" => "cargo",
                // JavaScript ecosystem
                "npm" | "npx" | "yarn" | "pnpm" | "node" | "deno" | "bun" => "npm",
                // Python ecosystem
                "pip" | "pip3" | "python" | "python3" | "pytest" | "poetry" | "uv" | "conda" => "python",
                // Go ecosystem
                "go" => "go",
                // Git ecosystem
                "git" | "gh" => "git",
                // Docker ecosystem
                "docker" | "docker-compose" | "podman" => "docker",
                // Build tools
                "make" | "cmake" | "ninja" => "make",
                // Shell utilities (less useful to group, but still worth tracking)
                "ls" | "cd" | "cat" | "mkdir" | "rm" | "cp" | "mv" => "shell",
                "grep" | "find" | "sed" | "awk" => "shell",
                _ => first_word,
            };

            if !category.is_empty() {
                Some(category.to_string())
            } else {
                None
            }
        }
        "Edit" | "Write" | "MultiEdit" => {
            let file_path = input.get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ext = extract_extension(file_path);
            if !ext.is_empty() {
                Some(ext.to_string())
            } else {
                None
            }
        }
        "Read" | "Glob" => {
            let file_path = input.get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ext = extract_extension(file_path);
            if !ext.is_empty() {
                Some(ext.to_string())
            } else {
                None
            }
        }
        "Task" => {
            input.get("subagent_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

/// Extract patterns from successful trajectories
#[allow(dead_code)]
fn extract_success_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Extract task category for concise context
    let task_category = extract_task_category(&trajectory.user_query);

    // Create patterns for each tool call with rich context
    for tool_call in trajectory.tool_calls.iter().take(MAX_PATTERNS_PER_TRAJECTORY) {
        // Extract meaningful context from tool input
        let tool_context = extract_tool_context(&tool_call.tool_name, &tool_call.tool_input);

        // Only create pattern if context is meaningful
        if tool_context.len() < 10 {
            continue;
        }

        // Extract command category for better filtering
        let command_category = extract_command_category(&tool_call.tool_name, &tool_call.tool_input);

        let context_query = format!(
            "Task: {}\nApproach: {} - {}\nOutcome: Success",
            task_category,
            tool_call.tool_name,
            tool_context
        );

        let pattern_hash = hash_string(&context_query);

        patterns.push(Pattern {
            id: 0,  // Will be set by database
            pattern_hash,
            tool_type: tool_call.tool_name.clone(),
            command_category,
            context_query,
            success_count: 1,
            failure_count: 0,
            embedding_id: None,
        });
    }

    // If no tool calls, skip - we want actionable patterns only
    patterns
}

/// Extract meaningful context from tool input with tech stack hints
fn extract_tool_context(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Edit" | "Write" | "MultiEdit" => {
            let file_path = input.get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let filename = extract_filename(file_path);
            let ext = extract_extension(file_path);

            // Include tech stack keywords for better similarity matching
            let tech_hint = match ext {
                "rs" => "rust cargo",
                "ts" | "tsx" => "typescript npm node",
                "js" | "jsx" => "javascript npm node",
                "py" => "python pip",
                "go" => "golang",
                "rb" => "ruby",
                "java" => "java maven",
                "sh" | "bash" => "shell bash",
                "json" => "json config",
                "toml" => "toml rust cargo",
                "yaml" | "yml" => "yaml config",
                "md" => "markdown docs",
                _ => "",
            };

            let old_str_preview = input.get("old_string")
                .and_then(|v| v.as_str())
                .map(|s| truncate(s, 40))
                .unwrap_or("");

            if !old_str_preview.is_empty() {
                format!("{} {} editing {} (replacing '{}')", ext, tech_hint, filename, old_str_preview)
            } else {
                format!("{} {} writing to {}", ext, tech_hint, filename)
            }
        }
        "Bash" => {
            let cmd = input.get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown command");
            let first_word = cmd.split_whitespace().next().unwrap_or("cmd");
            let desc = input.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !desc.is_empty() {
                format!("{} - {}", first_word, truncate(desc, 60))
            } else {
                format!("running '{}'", truncate(cmd, 80))
            }
        }
        "Read" | "Glob" | "Grep" => {
            let path = input.get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
                .map(extract_filename)
                .unwrap_or("");
            let pattern = input.get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !pattern.is_empty() {
                format!("searching for '{}' in {}", truncate(pattern, 30), path)
            } else if !path.is_empty() {
                format!("reading {}", path)
            } else {
                "exploring codebase".to_string()
            }
        }
        "Task" => {
            let agent = input.get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("agent");
            let desc = input.get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("delegating to {} - {}", agent, truncate(desc, 60))
        }
        "TodoWrite" => {
            "updating task list".to_string()
        }
        "WebSearch" | "WebFetch" => {
            let query = input.get("query")
                .or_else(|| input.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("searching web: {}", truncate(query, 60))
        }
        _ => {
            format!("using {} tool", tool_name)
        }
    }
}

/// Extract filename from path
fn extract_filename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Extract file extension from path
fn extract_extension(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or("")
}

/// Discover causal edges from pattern co-occurrences in trajectories
///
/// This analyzes which patterns tend to appear together and whether
/// they lead to success or failure, building a causal graph.
fn discover_causal_edges(db_path: &Path, trajectories: &[Trajectory]) -> Result<usize> {
    let store = PatternStore::open(db_path)?;
    let causal_store = CausalStore::open(db_path)?;

    let mut edges_created = 0;

    for trajectory in trajectories.iter().take(50) {  // Process recent trajectories
        // Determine trajectory success
        let is_success = trajectory.verdict.map(|v| v.success).unwrap_or(false);

        // Get pattern IDs for tool calls in this trajectory
        let mut pattern_ids: Vec<i64> = Vec::new();

        for tool_call in trajectory.tool_calls.iter().take(MAX_PATTERNS_PER_TRAJECTORY) {
            // Try to find matching pattern by tool type
            let tool_type = &tool_call.tool_name;
            if let Ok(patterns) = store.get_by_tool(tool_type, 10) {
                // Find the best matching pattern for this tool call
                let tool_context = extract_tool_context(tool_type, &tool_call.tool_input);
                for pattern in patterns {
                    // Simple context match - if there's overlap, consider it related
                    if context_matches(&tool_context, &pattern.context_query) {
                        pattern_ids.push(pattern.id);
                        break;
                    }
                }
            }
        }

        // Record co-occurrences between all pairs of patterns
        for i in 0..pattern_ids.len() {
            for j in (i + 1)..pattern_ids.len() {
                if let Err(e) = causal_store.record_cooccurrence(
                    pattern_ids[i],
                    pattern_ids[j],
                    is_success,
                ) {
                    debug!("Failed to record causal edge: {}", e);
                } else {
                    edges_created += 1;
                }
            }
        }
    }

    Ok(edges_created)
}

/// Check if tool context matches a pattern's context
fn context_matches(tool_context: &str, pattern_context: &str) -> bool {
    // Extract key terms from both contexts
    let tool_words: std::collections::HashSet<&str> = tool_context
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .collect();

    let pattern_words: std::collections::HashSet<&str> = pattern_context
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .collect();

    // Check for significant overlap (at least 2 matching words)
    let overlap = tool_words.intersection(&pattern_words).count();
    overlap >= 2
}

/// Extract patterns from failed trajectories (what to avoid)
fn extract_failure_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Find tool results with errors
    for result in &trajectory.tool_results {
        if result.is_error || result.content.to_lowercase().contains("error") {
            // Extract the key error message (only if actionable)
            let error_msg = match extract_error_message(&result.content) {
                Some(msg) => msg,
                None => continue,  // Skip non-actionable errors
            };

            // Extract task category (first few words)
            let task_category = extract_task_category(&trajectory.user_query);

            let context_query = format!(
                "Task: {}\nPitfall: {}\nAdvice: Verify this approach won't hit the same error",
                task_category,
                error_msg
            );

            let pattern_hash = hash_string(&context_query);

            patterns.push(Pattern {
                id: 0,
                pattern_hash,
                tool_type: "failure".to_string(),
                command_category: None,
                context_query,
                success_count: 0,
                failure_count: 1,
                embedding_id: None,
            });

            if patterns.len() >= MAX_PATTERNS_PER_TRAJECTORY {
                break;
            }
        }
    }

    patterns
}

/// Extract a short, generalizable task category from the user query
///
/// This extracts the *type* of task rather than specific details,
/// making patterns more reusable across similar tasks.
fn extract_task_category(query: &str) -> String {
    let lower = query.to_lowercase();
    let first_line = query.lines().next().unwrap_or(query);

    // Detect task type by keywords and generalize
    // More specific matches first
    if lower.contains("fix") && lower.contains("type") && lower.contains("error") {
        return "Fix type error".to_string();
    }
    if lower.contains("fix") && (lower.contains("error") || lower.contains("bug")) {
        return "Fix error or bug".to_string();
    }
    if lower.contains("add") && lower.contains("feature") {
        return "Add new feature".to_string();
    }
    if lower.contains("implement") {
        return "Implement functionality".to_string();
    }
    if lower.contains("refactor") {
        return "Refactor code".to_string();
    }
    if lower.contains("test") && (lower.contains("write") || lower.contains("add") || lower.contains("create")) {
        return "Write tests".to_string();
    }
    if lower.contains("run") && lower.contains("test") {
        return "Run tests".to_string();
    }
    if lower.contains("debug") {
        return "Debug issue".to_string();
    }
    if lower.contains("build") || lower.contains("compile") {
        return "Build/compile project".to_string();
    }
    if lower.contains("install") || lower.contains("setup") {
        return "Install/setup dependencies".to_string();
    }
    if lower.contains("deploy") {
        return "Deploy application".to_string();
    }
    if lower.contains("create") && (lower.contains("api") || lower.contains("endpoint")) {
        return "Create API endpoint".to_string();
    }
    if lower.contains("create") && lower.contains("component") {
        return "Create UI component".to_string();
    }
    if lower.contains("update") || lower.contains("modify") {
        return "Update existing code".to_string();
    }
    if lower.contains("delete") || lower.contains("remove") {
        return "Remove code/feature".to_string();
    }
    if lower.contains("document") || lower.contains("docs") {
        return "Documentation".to_string();
    }
    if lower.contains("config") || lower.contains("configure") {
        return "Configure settings".to_string();
    }
    if lower.contains("migrate") {
        return "Migration task".to_string();
    }
    if lower.contains("search") || lower.contains("find") {
        return "Search codebase".to_string();
    }
    if lower.contains("read") || lower.contains("understand") || lower.contains("explain")
       || lower.contains("summarize") || lower.contains("analyze") || lower.contains("review") {
        return "Understand code".to_string();
    }

    // Fallback: extract action verb and object type
    let words: Vec<&str> = first_line.split_whitespace().collect();
    if words.len() >= 2 {
        let action = words[0].to_lowercase();
        // Common action verbs
        if matches!(action.as_str(), "add" | "create" | "fix" | "update" | "run" |
                    "write" | "build" | "delete" | "move" | "rename" | "check") {
            // Return generalized version
            return format!("{} {}", capitalize(&action), "code/files");
        }
    }

    // Last resort: take first few words
    let category: String = words.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
    if category.len() > 40 {
        format!("{}...", &category[..40])
    } else {
        category
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Extract key error message from tool result
///
/// Returns None if no actionable error message is found
fn extract_error_message(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();

    // Skip if content looks like noise (line numbers, code output, etc.)
    if is_noise_content(content) {
        return None;
    }

    // Look for specific actionable error patterns
    for line in &lines {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Skip line number prefixes (e.g., "123→", "1419→")
        if trimmed.chars().take_while(|c| c.is_ascii_digit()).count() > 0
           && trimmed.contains('→') {
            continue;
        }

        // Skip short lines
        if trimmed.len() < 15 {
            continue;
        }

        // Look for actionable error messages
        if lower.contains("error:") || lower.contains("failed:")
           || lower.contains("cannot find") || lower.contains("no such file")
           || lower.contains("permission denied") || lower.contains("command not found")
           || lower.contains("syntax error") || lower.contains("type error")
           || lower.contains("does not exist") || lower.contains("undefined")
           || lower.contains("not found") {
            // Remove noisy prefixes
            let clean = clean_error_line(trimmed);
            if clean.len() >= 20 && !is_noise_content(&clean) {
                return Some(truncate(&clean, 120).to_string());
            }
        }
    }

    None
}

/// Check if content is likely noise (code output, line numbers, etc.)
fn is_noise_content(content: &str) -> bool {
    let lower = content.to_lowercase();

    // Skip if it's mostly line numbers/code output
    if content.chars().filter(|c| c.is_ascii_digit() || *c == '→' || *c == '│').count()
       > content.len() / 4 {
        return true;
    }

    // Skip console.log/print statements
    if lower.contains("console.log") || lower.contains("console.err")
       || lower.contains("print(") {
        return true;
    }

    // Skip generic tool errors
    if lower.contains("<tool_use_error>") && !lower.contains("command") {
        return true;
    }

    // Skip markdown/formatting
    if content.starts_with('#') || content.starts_with('-') || content.starts_with('*') {
        return true;
    }

    false
}

/// Clean up error line by removing noise prefixes
fn clean_error_line(line: &str) -> String {
    let mut result = line.to_string();

    // Remove exit code prefix
    if let Some(idx) = result.find("Exit code") {
        result = result[idx..].to_string();
    }

    // Remove arrow prefixes
    if let Some(idx) = result.find('→') {
        result = result[idx + '→'.len_utf8()..].trim().to_string();
    }

    result
}

fn collect_jsonl_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            debug!("Could not read dir {:?}: {}", dir, e);
            return Ok(files);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Check subdirectory
            if let Ok(subentries) = std::fs::read_dir(&path) {
                for subentry in subentries.flatten() {
                    let subpath = subentry.path();
                    if subpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                        files.push(subpath);
                    }
                }
            }
        } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            files.push(path);
        }
    }

    Ok(files)
}

fn get_mana_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

fn get_claude_logs_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claude/projects"))
        .unwrap_or_else(|| PathBuf::from(".claude/projects"))
}

fn hash_string(s: &str) -> String {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find the last valid UTF-8 char boundary at or before max_len
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

fn log_learning_event(db_path: &Path, result: &LearningResult) -> Result<()> {
    let conn = Connection::open(db_path)?;

    conn.execute(
        r#"
        INSERT INTO learning_log (event_type, details)
        VALUES ('foreground_learning', ?1)
        "#,
        [serde_json::to_string(&result)?],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::trajectory::{ToolCall, ToolResult, Verdict};

    #[test]
    fn test_extract_success_patterns() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Fix the type error in main.rs".into(),
            assistant_content: "I've fixed the type error".into(),
            tool_calls: vec![ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({
                    "file_path": "/project/src/main.rs",
                    "old_string": "let x: String = 123;"
                }),
            }],
            tool_results: vec![],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
        };

        let patterns = extract_success_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "Edit");
        assert!(patterns[0].context_query.contains("Edit"));
        assert!(patterns[0].context_query.contains("main.rs"));
        // Should include tech stack hints
        assert!(patterns[0].context_query.contains("rust"), "Should include rust tech hint");
    }

    #[test]
    fn test_extract_failure_patterns() {
        // Use an actionable error message that passes the filter
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Run the tests".into(),
            assistant_content: "Let me try again".into(),
            tool_calls: vec![],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "Error: cannot find module 'missing-module' - check your dependencies".into(),
                is_error: true,
            }],
            verdict: Some(Verdict { success: false, confidence: 0.8 }),
        };

        let patterns = extract_failure_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "failure");
        assert!(patterns[0].context_query.contains("Pitfall"));
        assert!(patterns[0].context_query.contains("cannot find module"));
    }

    #[test]
    fn test_noise_content_filtered() {
        // Noise content should not create patterns
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Run tests".into(),
            assistant_content: "Failed".into(),
            tool_calls: vec![],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "123→    console.error('test')".into(),  // Noise
                is_error: true,
            }],
            verdict: Some(Verdict { success: false, confidence: 0.8 }),
        };

        let patterns = extract_failure_patterns(&trajectory);
        assert_eq!(patterns.len(), 0, "Noise content should be filtered");
    }

    #[test]
    fn test_extract_task_category_generalization() {
        // Should generalize specific queries to reusable categories
        assert_eq!(
            extract_task_category("Fix the type error in main.rs"),
            "Fix type error"
        );
        assert_eq!(
            extract_task_category("fix this bug in the authentication module"),
            "Fix error or bug"
        );
        assert_eq!(
            extract_task_category("Add a new feature for user authentication"),
            "Add new feature"
        );
        assert_eq!(
            extract_task_category("implement the login functionality"),
            "Implement functionality"
        );
        assert_eq!(
            extract_task_category("run the tests"),
            "Run tests"
        );
        assert_eq!(
            extract_task_category("write unit tests for the API"),
            "Write tests"
        );
        assert_eq!(
            extract_task_category("refactor the database module"),
            "Refactor code"
        );
        assert_eq!(
            extract_task_category("search for where errors are handled"),
            "Search codebase"
        );
    }

    #[test]
    fn test_extract_command_category_bash() {
        // Rust ecosystem
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "cargo build --release"})),
            Some("cargo".to_string())
        );
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "rustc --version"})),
            Some("cargo".to_string())
        );

        // JavaScript ecosystem
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "npm install express"})),
            Some("npm".to_string())
        );
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "npx create-react-app my-app"})),
            Some("npm".to_string())
        );

        // Python ecosystem
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "pip install requests"})),
            Some("python".to_string())
        );
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "pytest tests/"})),
            Some("python".to_string())
        );

        // Git ecosystem
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "git status"})),
            Some("git".to_string())
        );
        assert_eq!(
            extract_command_category("Bash", &serde_json::json!({"command": "gh pr list"})),
            Some("git".to_string())
        );
    }

    #[test]
    fn test_extract_command_category_edit() {
        // Rust files
        assert_eq!(
            extract_command_category("Edit", &serde_json::json!({"file_path": "/src/main.rs"})),
            Some("rs".to_string())
        );

        // TypeScript files
        assert_eq!(
            extract_command_category("Edit", &serde_json::json!({"file_path": "/src/App.tsx"})),
            Some("tsx".to_string())
        );

        // Python files
        assert_eq!(
            extract_command_category("Write", &serde_json::json!({"file_path": "/app/main.py"})),
            Some("py".to_string())
        );
    }

    #[test]
    fn test_extract_command_category_task() {
        assert_eq!(
            extract_command_category("Task", &serde_json::json!({"subagent_type": "researcher"})),
            Some("researcher".to_string())
        );
        assert_eq!(
            extract_command_category("Task", &serde_json::json!({"subagent_type": "coder"})),
            Some("coder".to_string())
        );
    }

    #[test]
    fn test_success_pattern_has_command_category() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Build the project".into(),
            assistant_content: "Building...".into(),
            tool_calls: vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({
                    "command": "cargo build --release",
                    "description": "Build release binary"
                }),
            }],
            tool_results: vec![],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
        };

        let patterns = extract_success_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "Bash");
        assert_eq!(patterns[0].command_category, Some("cargo".to_string()));
    }
}
