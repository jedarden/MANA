//! Foreground learning - quick pattern extraction
//!
//! Runs synchronously after session-end when threshold is reached.
//! Latency budget: <1 second.
//!
//! Supports automatic embedding generation for vector search retrieval.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

use super::trajectory::{parse_trajectories, Trajectory};
use super::LearningResult;
use crate::storage::{PatternStore, Pattern, CausalStore};
use crate::hooks::session_end_handler::AccumulatorState;
use crate::embeddings::{EmbeddingStore, EmbeddingConfig};

/// Maximum patterns to extract per trajectory (ReasoningBank constraint)
const MAX_PATTERNS_PER_TRAJECTORY: usize = 3;

/// Whether to automatically generate embeddings for new patterns
/// This enables vector search retrieval in the context injection pipeline
const AUTO_GENERATE_EMBEDDINGS: bool = true;

/// Maximum patterns to embed per learning cycle (to keep latency under control)
const MAX_PATTERNS_TO_EMBED: usize = 100;

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

    // Track pattern counts by type
    let mut reasoning_count = 0;
    let mut conversation_count = 0;
    let mut system_count = 0;
    let mut instruction_count = 0;

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

        // NEW: Extract reasoning patterns from thinking blocks
        let reasoning_patterns = extract_reasoning_patterns(trajectory);
        reasoning_count += reasoning_patterns.len();
        all_patterns.extend(reasoning_patterns);

        // NEW: Extract conversation patterns from pure dialogue
        let conv_patterns = extract_conversation_patterns(trajectory);
        conversation_count += conv_patterns.len();
        all_patterns.extend(conv_patterns);

        // NEW: Extract system context patterns
        let sys_patterns = extract_system_patterns(trajectory);
        system_count += sys_patterns.len();
        all_patterns.extend(sys_patterns);

        // NEW: Extract instruction patterns from user messages
        let instr_patterns = extract_instruction_patterns(trajectory);
        instruction_count += instr_patterns.len();
        all_patterns.extend(instr_patterns);

        result.trajectories_processed += 1;
    }

    info!("Extracted {} reasoning, {} conversation, {} system, {} instruction patterns",
          reasoning_count, conversation_count, system_count, instruction_count);

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

    // Auto-generate embeddings for vector search (if enabled)
    if AUTO_GENERATE_EMBEDDINGS && result.patterns_created > 0 {
        let embed_start = Instant::now();
        match generate_embeddings_for_new_patterns(&mana_dir) {
            Ok(embedded) => {
                if embedded > 0 {
                    info!("Generated {} embeddings for vector search in {}ms",
                          embedded, embed_start.elapsed().as_millis());
                }
            }
            Err(e) => {
                warn!("Failed to generate embeddings: {} (vector search will use TF-IDF fallback)", e);
            }
        }
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
                    last_used: None,
                    access_count: 0,
                    tier_path: "global".to_string(),
                    ..Default::default()
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
            last_used: None,
            access_count: 0,
            tier_path: "global".to_string(),
            embedding_id: None,
            ..Default::default()
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

    // Find tool results with errors - trust the is_error flag from Claude Code
    for result in &trajectory.tool_results {
        if result.is_error {
            // Use first meaningful line of error content, truncated
            let error_msg = extract_first_error_line(&result.content);

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
                last_used: None,
                access_count: 0,
                tier_path: "global".to_string(),
                ..Default::default()
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

/// Extract first meaningful error line from tool result content
///
/// Trusts is_error flag - no heuristic filtering of error types
fn extract_first_error_line(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();

        // Skip empty lines and line number prefixes
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().take_while(|c| c.is_ascii_digit()).count() > 0
           && trimmed.contains('→') {
            continue;
        }

        // Skip very short lines
        if trimmed.len() < 10 {
            continue;
        }

        // Return first meaningful line, cleaned and truncated
        let clean = clean_error_line(trimmed);
        return truncate(&clean, 150).to_string();
    }

    // Fallback: truncate entire content
    truncate(content, 150).to_string()
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

/// Generate embeddings for patterns that don't have them yet
///
/// This enables vector search retrieval in the context injection pipeline.
/// Runs as part of foreground learning to keep embeddings in sync with patterns.
fn generate_embeddings_for_new_patterns(mana_dir: &Path) -> Result<usize> {
    // Check if embedding index exists - if not, initialize it
    let index_path = mana_dir.join("vectors.usearch");

    // Open or create embedding store
    let config = EmbeddingConfig::default();
    let mut store = if index_path.exists() {
        EmbeddingStore::open(mana_dir)?
    } else {
        EmbeddingStore::new(mana_dir, &config)?
    };

    // Check current status
    let status = store.status()?;
    if status.unembedded_count == 0 {
        debug!("All patterns already have embeddings");
        return Ok(0);
    }

    debug!("Found {} patterns without embeddings", status.unembedded_count);

    // Generate embeddings for patterns that don't have them
    // Limit to MAX_PATTERNS_TO_EMBED to keep latency under control
    let mut embedded = 0;
    while embedded < MAX_PATTERNS_TO_EMBED {
        let batch_count = store.embed_missing()?;
        if batch_count == 0 {
            break;
        }
        embedded += batch_count;
        debug!("Embedded {} patterns (total: {})", batch_count, embedded);
    }

    if embedded > 0 {
        // Save the updated index
        store.save_index()?;
    }

    Ok(embedded)
}

/// Extract patterns from thinking/reasoning blocks
///
/// Creates patterns that capture how Claude reasons about problems,
/// useful for learning good reasoning strategies.
fn extract_reasoning_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Only extract from trajectories with thinking content
    if trajectory.thinking_content.is_empty() {
        return patterns;
    }

    // Extract task category for context
    let task_category = extract_task_category(&trajectory.user_query);

    for thinking in &trajectory.thinking_content {
        // Skip very short thinking blocks (likely not meaningful)
        if thinking.content.len() < 50 {
            continue;
        }

        // Extract key reasoning patterns from the thinking block
        let reasoning_summary = extract_reasoning_summary(&thinking.content);

        if reasoning_summary.is_empty() {
            continue;
        }

        let context_query = format!(
            "Task: {}\nReasoning: {}\nOutcome: {}",
            task_category,
            reasoning_summary,
            if trajectory.verdict.map(|v| v.success).unwrap_or(false) { "Success" } else { "Incomplete" }
        );

        let pattern_hash = hash_string(&context_query);

        patterns.push(Pattern {
            id: 0,
            pattern_hash,
            tool_type: "reasoning".to_string(),
            command_category: Some("thinking".to_string()),
            context_query,
            success_count: if trajectory.verdict.map(|v| v.success).unwrap_or(false) { 1 } else { 0 },
            failure_count: if trajectory.verdict.map(|v| v.success).unwrap_or(false) { 0 } else { 1 },
            embedding_id: None,
            last_used: None,
            access_count: 0,
            tier_path: "global".to_string(),
            ..Default::default()
        });

        if patterns.len() >= MAX_PATTERNS_PER_TRAJECTORY {
            break;
        }
    }

    patterns
}

/// Extract a summary of reasoning from thinking content
fn extract_reasoning_summary(thinking: &str) -> String {
    // Extract key reasoning patterns
    let mut summary_parts = Vec::new();

    let lower = thinking.to_lowercase();

    // Look for planning/strategy indicators
    if lower.contains("let me") || lower.contains("i need to") || lower.contains("first") {
        // Find the sentence containing planning
        for line in thinking.lines().take(5) {
            if line.to_lowercase().contains("let me")
               || line.to_lowercase().contains("i need to")
               || line.to_lowercase().contains("first") {
                let clean = line.trim();
                if clean.len() > 20 && clean.len() < 200 {
                    summary_parts.push(clean.to_string());
                    break;
                }
            }
        }
    }

    // Look for analysis patterns
    if lower.contains("analyzing") || lower.contains("looking at") || lower.contains("examining") {
        for line in thinking.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.contains("analyzing")
               || line_lower.contains("looking at")
               || line_lower.contains("examining") {
                let clean = line.trim();
                if clean.len() > 20 && clean.len() < 200 {
                    summary_parts.push(clean.to_string());
                    break;
                }
            }
        }
    }

    // Look for decision points
    if lower.contains("should") || lower.contains("could") || lower.contains("option") {
        for line in thinking.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.contains("should")
               || line_lower.contains("could")
               || (line_lower.contains("option") && !line_lower.contains("optional")) {
                let clean = line.trim();
                if clean.len() > 20 && clean.len() < 200 {
                    summary_parts.push(clean.to_string());
                    break;
                }
            }
        }
    }

    // If no specific patterns found, extract first meaningful line
    if summary_parts.is_empty() {
        for line in thinking.lines().take(10) {
            let clean = line.trim();
            if clean.len() > 30 && clean.len() < 200
               && !clean.starts_with("```")
               && !clean.starts_with("//")
               && !clean.starts_with("#") {
                summary_parts.push(clean.to_string());
                break;
            }
        }
    }

    // Combine and truncate
    let result = summary_parts.join(" | ");
    if result.len() > 300 {
        format!("{}...", &result[..297])
    } else {
        result
    }
}

/// Extract patterns from conversation-only trajectories
///
/// Creates patterns that capture good Q&A interactions, explanations,
/// and knowledge sharing sessions.
fn extract_conversation_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Only extract from conversation-only trajectories
    if !trajectory.is_conversation_only {
        return patterns;
    }

    // Need meaningful content
    if trajectory.assistant_content.len() < 100 {
        return patterns;
    }

    // Determine conversation type
    let conversation_type = classify_conversation(&trajectory.user_query);

    // Extract quality indicators
    let response_quality = assess_response_quality(&trajectory.assistant_content);

    // Only create patterns for high-quality conversations
    if response_quality < 0.6 {
        return patterns;
    }

    let context_query = format!(
        "Question type: {}\nQuery: {}\nResponse approach: {}\nQuality: {:.0}%",
        conversation_type,
        truncate(&trajectory.user_query, 100),
        extract_response_approach(&trajectory.assistant_content),
        response_quality * 100.0
    );

    let pattern_hash = hash_string(&context_query);

    patterns.push(Pattern {
        id: 0,
        pattern_hash,
        tool_type: "conversation".to_string(),
        command_category: Some(conversation_type),
        context_query,
        success_count: 1,
        failure_count: 0,
        embedding_id: None,
        last_used: None,
        access_count: 0,
        tier_path: "global".to_string(),
        ..Default::default()
    });

    patterns
}

/// Classify the type of conversation based on user query
fn classify_conversation(query: &str) -> String {
    let lower = query.to_lowercase();

    if lower.contains("what is") || lower.contains("what are") || lower.contains("what does") {
        "definition".to_string()
    } else if lower.contains("how do") || lower.contains("how to") || lower.contains("how can") {
        "how-to".to_string()
    } else if lower.contains("why") {
        "explanation".to_string()
    } else if lower.contains("explain") || lower.contains("describe") {
        "explanation".to_string()
    } else if lower.contains("difference") || lower.contains("compare") {
        "comparison".to_string()
    } else if lower.contains("example") || lower.contains("show me") {
        "example".to_string()
    } else if lower.contains("review") || lower.contains("feedback") {
        "review".to_string()
    } else if lower.starts_with("is ") || lower.starts_with("are ") || lower.starts_with("can ") {
        "yes-no".to_string()
    } else {
        "general".to_string()
    }
}

/// Assess the quality of a response
fn assess_response_quality(response: &str) -> f64 {
    let mut score: f64 = 0.5; // Base score

    // Length check (not too short, not too long)
    let len = response.len();
    if len > 200 && len < 5000 {
        score += 0.1;
    }

    // Structure indicators
    if response.contains("1.") || response.contains("- ") || response.contains("* ") {
        score += 0.1; // Has structure
    }

    // Code examples
    if response.contains("```") {
        score += 0.1; // Has code examples
    }

    // Explanation depth
    let lower = response.to_lowercase();
    if lower.contains("because") || lower.contains("this means") || lower.contains("in other words") {
        score += 0.1; // Has explanations
    }

    // Complete sentences
    if response.ends_with('.') || response.ends_with('!') || response.ends_with(':') {
        score += 0.05;
    }

    score.min(1.0)
}

/// Extract the approach used in a response
fn extract_response_approach(response: &str) -> String {
    let lower = response.to_lowercase();

    let mut approaches = Vec::new();

    if response.contains("```") {
        approaches.push("code example");
    }
    if lower.contains("1.") || lower.contains("step") {
        approaches.push("step-by-step");
    }
    if lower.contains("for example") || lower.contains("e.g.") {
        approaches.push("with examples");
    }
    if lower.contains("note:") || lower.contains("important:") || lower.contains("warning:") {
        approaches.push("with caveats");
    }

    if approaches.is_empty() {
        "direct explanation".to_string()
    } else {
        approaches.join(", ")
    }
}

/// Extract patterns from system messages (learning from context/prompts)
fn extract_system_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    use crate::learning::trajectory::SystemMessageType;

    let mut patterns = Vec::new();

    // Only process meaningful system messages
    for sys_msg in &trajectory.system_messages {
        // Skip reminders and very short messages
        if matches!(sys_msg.msg_type, SystemMessageType::SystemReminder) {
            continue;
        }

        if sys_msg.content.len() < 100 {
            continue;
        }

        // Extract context type
        let context_type = match &sys_msg.msg_type {
            SystemMessageType::SystemPrompt => "system_prompt",
            SystemMessageType::Context => "project_context",
            SystemMessageType::SystemReminder => continue, // Skip
            SystemMessageType::Other(t) => t.as_str(),
        };

        // Create a summary of the system context
        let context_summary = extract_system_summary(&sys_msg.content);

        if context_summary.len() < 20 {
            continue;
        }

        let context_query = format!(
            "Context type: {}\nSummary: {}\nUsed in: {}",
            context_type,
            context_summary,
            extract_task_category(&trajectory.user_query)
        );

        let pattern_hash = hash_string(&context_query);

        patterns.push(Pattern {
            id: 0,
            pattern_hash,
            tool_type: "system_context".to_string(),
            command_category: Some(context_type.to_string()),
            context_query,
            success_count: if trajectory.verdict.map(|v| v.success).unwrap_or(false) { 1 } else { 0 },
            failure_count: 0,
            embedding_id: None,
            last_used: None,
            access_count: 0,
            tier_path: "global".to_string(),
            ..Default::default()
        });

        if patterns.len() >= 2 {
            break; // Limit system patterns
        }
    }

    patterns
}

/// Extract a summary from system message content
fn extract_system_summary(content: &str) -> String {
    // Look for key configuration or instruction lines
    let mut summary_parts = Vec::new();

    for line in content.lines().take(20) {
        let trimmed = line.trim();

        // Skip empty or very short lines
        if trimmed.len() < 10 {
            continue;
        }

        // Skip markdown headers that are just formatting
        if trimmed.starts_with("##") && trimmed.len() < 30 {
            continue;
        }

        // Look for instruction-like content
        let lower = trimmed.to_lowercase();
        if lower.contains("must") || lower.contains("should") || lower.contains("always")
           || lower.contains("never") || lower.contains("important") {
            summary_parts.push(trimmed.to_string());
            if summary_parts.len() >= 3 {
                break;
            }
        }
    }

    // If no specific patterns, get first meaningful line
    if summary_parts.is_empty() {
        for line in content.lines().take(10) {
            let trimmed = line.trim();
            if trimmed.len() > 30 && trimmed.len() < 200
               && !trimmed.starts_with("```")
               && !trimmed.starts_with("//") {
                summary_parts.push(trimmed.to_string());
                break;
            }
        }
    }

    let result = summary_parts.join(" | ");
    if result.len() > 250 {
        format!("{}...", &result[..247])
    } else {
        result
    }
}

// ============================================================================
// Instruction Pattern Extraction
// ============================================================================

/// Directive keywords that signal user instructions
const DIRECTIVE_KEYWORDS: &[&str] = &[
    "always", "never", "must", "should", "prefer", "don't", "do not",
    "make sure", "ensure", "remember", "avoid", "whenever", "please"
];

/// Imperative verbs that often start instructions
const IMPERATIVE_VERBS: &[&str] = &[
    "use", "run", "check", "test", "add", "remove", "write", "create",
    "follow", "keep", "maintain", "apply", "format", "lint", "commit",
    "build", "deploy", "verify", "update", "include", "exclude"
];

/// Extract instruction patterns from user messages in a trajectory
///
/// Scans all user messages for directive-like content (instructions, preferences,
/// guidelines) and creates patterns for them. These patterns will be tracked
/// across sessions to identify frequently-repeated instructions.
pub fn extract_instruction_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Process all user messages in the trajectory
    for user_message in &trajectory.user_messages {
        let instructions = detect_instructions(user_message);

        for instruction in instructions {
            let instruction_type = classify_instruction_type(&instruction);

            let context_query = format!(
                "User instruction: {}\nType: {}\nContext: {}",
                instruction,
                instruction_type,
                extract_task_category_from_query(&trajectory.user_query),
            );

            let pattern_hash = hash_string(&context_query);

            patterns.push(Pattern {
                id: 0,
                pattern_hash,
                tool_type: "instruction".to_string(),
                command_category: Some(instruction_type),
                context_query,
                success_count: 1,
                failure_count: 0,
                embedding_id: None,
                last_used: None,
                access_count: 0,
                tier_path: "project".to_string(), // Instructions default to project-level
                session_count: 1,
                frequency_weight: 1.0,
                session_ids: None,
            });
        }
    }

    // Also check the main user query for instructions
    let query_instructions = detect_instructions(&trajectory.user_query);
    for instruction in query_instructions {
        // Avoid duplicates by checking if we already have this instruction
        let instruction_type = classify_instruction_type(&instruction);
        let context_query = format!(
            "User instruction: {}\nType: {}\nContext: {}",
            instruction,
            instruction_type,
            extract_task_category_from_query(&trajectory.user_query),
        );

        let pattern_hash = hash_string(&context_query);

        // Check for duplicates
        if patterns.iter().any(|p| p.pattern_hash == pattern_hash) {
            continue;
        }

        patterns.push(Pattern {
            id: 0,
            pattern_hash,
            tool_type: "instruction".to_string(),
            command_category: Some(instruction_type),
            context_query,
            success_count: 1,
            failure_count: 0,
            embedding_id: None,
            last_used: None,
            access_count: 0,
            tier_path: "project".to_string(),
            session_count: 1,
            frequency_weight: 1.0,
            session_ids: None,
        });
    }

    // Limit to prevent excessive patterns from a single trajectory
    patterns.truncate(5);
    patterns
}

/// Detect instruction-like content in a user message
///
/// Identifies sentences that contain directive keywords or start with
/// imperative verbs, filtering out noise and short fragments.
fn detect_instructions(message: &str) -> Vec<String> {
    let mut instructions = Vec::new();

    // Split into sentences (handle multiple delimiters)
    for sentence in message.split(|c| c == '.' || c == '!' || c == '\n') {
        let sentence = sentence.trim();

        // Filter: must be reasonable length
        if sentence.len() < 10 || sentence.len() > 300 {
            continue;
        }

        // Filter: skip code blocks
        if sentence.starts_with("```") || sentence.starts_with("   ") || sentence.starts_with("\t") {
            continue;
        }

        let sentence_lower = sentence.to_lowercase();

        // Check for directive keywords
        let has_directive = DIRECTIVE_KEYWORDS.iter()
            .any(|kw| sentence_lower.contains(kw));

        // Check for imperative verb at start (after common prefixes)
        let words: Vec<&str> = sentence_lower.split_whitespace().collect();
        let first_significant_word = words.iter()
            .find(|w| !["please", "can", "you", "could", "would", "i", "want", "need", "to"].contains(w))
            .unwrap_or(&"");

        let starts_with_imperative = IMPERATIVE_VERBS.iter()
            .any(|verb| first_significant_word == verb);

        if has_directive || starts_with_imperative {
            // Normalize the instruction
            let normalized = normalize_instruction(sentence);
            if !normalized.is_empty() && normalized.len() >= 10 {
                instructions.push(normalized);
            }
        }
    }

    instructions
}

/// Normalize an instruction for consistent storage
fn normalize_instruction(instruction: &str) -> String {
    let mut result = instruction.trim().to_string();

    // Remove leading "please" or "can you"
    let lower = result.to_lowercase();
    if lower.starts_with("please ") {
        result = result[7..].to_string();
    } else if lower.starts_with("can you ") {
        result = result[8..].to_string();
    } else if lower.starts_with("could you ") {
        result = result[10..].to_string();
    }

    // Capitalize first letter
    if let Some(c) = result.chars().next() {
        result = c.to_uppercase().to_string() + &result[c.len_utf8()..];
    }

    // Remove trailing punctuation for consistency
    result = result.trim_end_matches(|c| c == '.' || c == '!' || c == '?').to_string();

    result
}

/// Classify the type of instruction for categorization
fn classify_instruction_type(instruction: &str) -> String {
    let lower = instruction.to_lowercase();

    if lower.contains("test") || lower.contains("spec") || lower.contains("coverage") {
        "testing".to_string()
    } else if lower.contains("typescript") || lower.contains("eslint") || lower.contains("format")
           || lower.contains("lint") || lower.contains("style") || lower.contains("prettier") {
        "coding_style".to_string()
    } else if lower.contains("commit") || lower.contains("git") || lower.contains("branch")
           || lower.contains("push") || lower.contains("merge") {
        "version_control".to_string()
    } else if lower.contains("error") || lower.contains("handle") || lower.contains("catch")
           || lower.contains("exception") {
        "error_handling".to_string()
    } else if lower.contains("document") || lower.contains("comment") || lower.contains("readme") {
        "documentation".to_string()
    } else if lower.contains("build") || lower.contains("compile") || lower.contains("deploy") {
        "build_deploy".to_string()
    } else if lower.contains("import") || lower.contains("export") || lower.contains("module")
           || lower.contains("package") {
        "dependencies".to_string()
    } else if lower.contains("name") || lower.contains("variable") || lower.contains("function")
           || lower.contains("class") {
        "naming".to_string()
    } else {
        "general".to_string()
    }
}

/// Extract a task category from the user query for context
fn extract_task_category_from_query(query: &str) -> String {
    let lower = query.to_lowercase();

    if lower.contains("fix") || lower.contains("bug") || lower.contains("error") {
        "bug_fix".to_string()
    } else if lower.contains("add") || lower.contains("implement") || lower.contains("create") {
        "feature".to_string()
    } else if lower.contains("refactor") || lower.contains("improve") || lower.contains("clean") {
        "refactoring".to_string()
    } else if lower.contains("test") {
        "testing".to_string()
    } else if lower.contains("deploy") || lower.contains("release") {
        "deployment".to_string()
    } else {
        "general".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::trajectory::{ToolCall, ToolResult, Verdict, ThinkingBlock, SystemMessage, SystemMessageType};

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
                tool_use_id: Some("tool_123".into()),
            }],
            tool_results: vec![],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };

        let patterns = extract_failure_patterns(&trajectory);
        assert_eq!(patterns.len(), 0, "Noise content should be filtered");
    }

    #[test]
    fn test_extract_reasoning_patterns() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "How should I refactor this database module?".into(),
            assistant_content: "Here's my recommendation for refactoring".into(),
            thinking_content: vec![ThinkingBlock {
                content: "Let me analyze the current structure. I need to first understand the dependencies between modules. Looking at the code, I should consider extracting the database connection logic into a separate trait.".into(),
                position: 1,
            }],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
        };

        let patterns = extract_reasoning_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "reasoning");
        assert!(patterns[0].context_query.contains("Reasoning"));
    }

    #[test]
    fn test_extract_conversation_patterns() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "What is the difference between a trait and an interface?".into(),
            assistant_content: "A trait in Rust is similar to an interface in other languages, but with some key differences. First, traits can provide default implementations for methods. Second, traits support associated types which interfaces typically don't. Here's an example:\n\n```rust\ntrait Animal {\n    fn speak(&self);\n}\n```\n\nThis allows for more flexible and powerful abstractions.".into(),
            is_conversation_only: true,
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
        };

        let patterns = extract_conversation_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "conversation");
        assert!(patterns[0].context_query.contains("comparison"));
    }

    #[test]
    fn test_extract_system_patterns() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Fix the type error".into(),
            assistant_content: "Fixed".into(),
            system_messages: vec![SystemMessage {
                content: "You must always write tests for new code. You should follow the existing code style. Important: Never commit directly to main.".into(),
                msg_type: SystemMessageType::SystemPrompt,
            }],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
        };

        let patterns = extract_system_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "system_context");
    }

    #[test]
    fn test_classify_conversation() {
        assert_eq!(classify_conversation("What is a closure?"), "definition");
        assert_eq!(classify_conversation("How do I implement a trait?"), "how-to");
        assert_eq!(classify_conversation("Why does Rust use ownership?"), "explanation");
        assert_eq!(classify_conversation("What's the difference between Vec and array?"), "comparison");
        assert_eq!(classify_conversation("Show me an example of pattern matching"), "example");
        assert_eq!(classify_conversation("Is this code correct?"), "yes-no");
    }

    #[test]
    fn test_assess_response_quality() {
        // Short response = lower quality
        let short = "Yes.";
        assert!(assess_response_quality(short) < 0.6);

        // Structured response with code = higher quality
        let structured = "Here's how you can do it:\n\n1. First, create the struct\n2. Then implement the trait\n\n```rust\nstruct Foo;\n```\n\nThis works because the compiler can infer the types.";
        assert!(assess_response_quality(structured) >= 0.8);
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
                tool_use_id: Some("tool_456".into()),
            }],
            tool_results: vec![],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
        };

        let patterns = extract_success_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "Bash");
        assert_eq!(patterns[0].command_category, Some("cargo".to_string()));
    }

    #[test]
    fn test_extract_reasoning_summary() {
        let thinking = "Let me analyze this problem. First, I need to understand the requirements. Looking at the code structure, I should refactor the module.";
        let summary = extract_reasoning_summary(thinking);
        assert!(!summary.is_empty());
        assert!(summary.contains("Let me") || summary.contains("First") || summary.contains("Looking"));
    }

    // ========================================================================
    // Instruction Pattern Tests
    // ========================================================================

    #[test]
    fn test_detect_instructions_with_directive_keywords() {
        let message = "Always use TypeScript for new files. Never commit directly to main.";
        let instructions = detect_instructions(message);
        assert_eq!(instructions.len(), 2);
        assert!(instructions[0].contains("TypeScript"));
        assert!(instructions[1].contains("commit") || instructions[1].contains("main"));
    }

    #[test]
    fn test_detect_instructions_with_imperative_verbs() {
        let message = "Run the tests before pushing. Use conventional commits for all changes.";
        let instructions = detect_instructions(message);
        assert_eq!(instructions.len(), 2);
        assert!(instructions[0].contains("tests") || instructions[0].contains("Run"));
        assert!(instructions[1].contains("conventional") || instructions[1].contains("commits"));
    }

    #[test]
    fn test_detect_instructions_filters_short_content() {
        let message = "Fix it. Run tests before pushing code to the repository.";
        let instructions = detect_instructions(message);
        // "Fix it" is too short (< 10 chars), should be filtered
        assert_eq!(instructions.len(), 1);
        assert!(instructions[0].contains("tests") || instructions[0].contains("pushing"));
    }

    #[test]
    fn test_detect_instructions_filters_code_blocks() {
        let message = "Always use proper error handling.\n```rust\nfn main() {}\n```\nNever ignore errors.";
        let instructions = detect_instructions(message);
        // Should get 2 instructions, skipping the code block
        assert!(instructions.len() >= 1);
        assert!(instructions.iter().all(|i| !i.contains("fn main")));
    }

    #[test]
    fn test_normalize_instruction_removes_please() {
        assert_eq!(normalize_instruction("Please use TypeScript"), "Use TypeScript");
        assert_eq!(normalize_instruction("Can you always run tests"), "Always run tests");
        assert_eq!(normalize_instruction("Could you follow the style guide"), "Follow the style guide");
    }

    #[test]
    fn test_normalize_instruction_capitalizes() {
        assert_eq!(normalize_instruction("use typescript"), "Use typescript");
    }

    #[test]
    fn test_normalize_instruction_removes_trailing_punctuation() {
        assert_eq!(normalize_instruction("Always use TypeScript."), "Always use TypeScript");
        assert_eq!(normalize_instruction("Run tests!"), "Run tests");
    }

    #[test]
    fn test_classify_instruction_type() {
        assert_eq!(classify_instruction_type("Run tests before committing"), "testing");
        assert_eq!(classify_instruction_type("Use TypeScript for all code"), "coding_style");
        assert_eq!(classify_instruction_type("Use ESLint for formatting"), "coding_style");
        assert_eq!(classify_instruction_type("Always commit with descriptive messages"), "version_control");
        assert_eq!(classify_instruction_type("Push to feature branches first"), "version_control");
        assert_eq!(classify_instruction_type("Handle all errors properly"), "error_handling");
        assert_eq!(classify_instruction_type("Add documentation comments"), "documentation");
        assert_eq!(classify_instruction_type("Build before deploying"), "build_deploy");
        assert_eq!(classify_instruction_type("Some random instruction"), "general");
    }

    #[test]
    fn test_extract_instruction_patterns() {
        let trajectory = Trajectory {
            session_id: "test-session".into(),
            user_query: "Fix the bug and always run tests before committing".into(),
            user_messages: vec![
                "Please use TypeScript for new files".into(),
                "Make sure to follow the existing code style".into(),
            ],
            assistant_content: "I'll fix the bug now".into(),
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
            ..Default::default()
        };

        let patterns = extract_instruction_patterns(&trajectory);

        // Should extract instructions from both user_messages and user_query
        assert!(!patterns.is_empty());
        assert!(patterns.iter().all(|p| p.tool_type == "instruction"));

        // Check that patterns have correct structure
        for pattern in &patterns {
            assert!(pattern.context_query.contains("User instruction:"));
            assert!(pattern.context_query.contains("Type:"));
            assert!(!pattern.pattern_hash.is_empty());
        }
    }

    #[test]
    fn test_extract_task_category_from_query() {
        assert_eq!(extract_task_category_from_query("Fix the bug in main.rs"), "bug_fix");
        assert_eq!(extract_task_category_from_query("Add a new feature"), "feature");
        assert_eq!(extract_task_category_from_query("Implement login"), "feature");
        assert_eq!(extract_task_category_from_query("Refactor the database module"), "refactoring");
        assert_eq!(extract_task_category_from_query("Run the tests"), "testing");
        assert_eq!(extract_task_category_from_query("Deploy to production"), "deployment");
        assert_eq!(extract_task_category_from_query("Do something random"), "general");
    }
}
