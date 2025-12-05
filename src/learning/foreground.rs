//! Foreground learning - quick pattern extraction
//!
//! Runs synchronously after session-end when threshold is reached.
//! Latency budget: <1 second.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

use super::trajectory::{parse_trajectories, Trajectory};
use super::LearningResult;
use crate::storage::{PatternStore, Pattern};

/// Maximum patterns to extract per trajectory (ReasoningBank constraint)
const MAX_PATTERNS_PER_TRAJECTORY: usize = 3;

/// Run foreground learning on accumulated trajectories
///
/// Extracts patterns from JSONL logs and stores them in the ReasoningBank.
/// This runs synchronously and should complete in <1 second.
pub async fn foreground_learn(pending_files: &[PathBuf]) -> Result<LearningResult> {
    let start = Instant::now();

    info!("Starting foreground learning with {} pending files", pending_files.len());

    let mut result = LearningResult::default();

    // Get MANA data directory
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");
    let store = PatternStore::open(&db_path)?;

    // Parse trajectories from all JSONL files in Claude logs
    let claude_logs = get_claude_logs_dir();
    if !claude_logs.exists() {
        info!("Claude logs directory not found, skipping learning");
        return Ok(result);
    }

    // Collect all JSONL files
    let jsonl_files = collect_jsonl_files(&claude_logs)?;
    info!("Found {} JSONL files to process", jsonl_files.len());

    // Parse trajectories
    let mut all_trajectories = Vec::new();
    for file in &jsonl_files {
        match parse_trajectories(file, 0) {
            Ok(trajectories) => {
                all_trajectories.extend(trajectories);
            }
            Err(e) => {
                debug!("Failed to parse {:?}: {}", file, e);
            }
        }
    }

    info!("Parsed {} trajectories total", all_trajectories.len());

    // Partition by verdict
    let (successful, failed): (Vec<_>, Vec<_>) = all_trajectories
        .into_iter()
        .partition(|t| t.verdict.map(|v| v.success).unwrap_or(false));

    info!("{} successful, {} failed trajectories", successful.len(), failed.len());

    // Extract patterns from successful trajectories
    for trajectory in successful.iter().take(50) {  // Limit to avoid timeout
        let patterns = extract_success_patterns(trajectory);
        for pattern in patterns {
            match store.insert(&pattern) {
                Ok(_) => result.patterns_created += 1,
                Err(e) => debug!("Failed to insert pattern: {}", e),
            }
        }
        result.trajectories_processed += 1;
    }

    // Extract failure lessons (what to avoid)
    for trajectory in failed.iter().take(20) {  // Limit to avoid timeout
        let patterns = extract_failure_patterns(trajectory);
        for pattern in patterns {
            match store.insert(&pattern) {
                Ok(_) => result.patterns_created += 1,
                Err(e) => debug!("Failed to insert pattern: {}", e),
            }
        }
        result.trajectories_processed += 1;
    }

    // Log learning event to database
    log_learning_event(&db_path, &result)?;

    result.duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "Foreground learning complete: {} patterns created from {} trajectories in {}ms",
        result.patterns_created, result.trajectories_processed, result.duration_ms
    );

    Ok(result)
}

/// Extract patterns from successful trajectories
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
            context_query,
            success_count: 1,
            failure_count: 0,
            embedding_id: None,
        });
    }

    // If no tool calls, skip - we want actionable patterns only
    patterns
}

/// Extract meaningful context from tool input
fn extract_tool_context(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Edit" | "Write" | "MultiEdit" => {
            let file_path = input.get("file_path")
                .and_then(|v| v.as_str())
                .map(|p| extract_filename(p))
                .unwrap_or("unknown file");
            let old_str_preview = input.get("old_string")
                .and_then(|v| v.as_str())
                .map(|s| truncate(s, 50))
                .unwrap_or("");
            if !old_str_preview.is_empty() {
                format!("editing {} (replacing '{}')", file_path, old_str_preview)
            } else {
                format!("writing to {}", file_path)
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
                .map(|p| extract_filename(p))
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

/// Extract patterns from failed trajectories (what to avoid)
fn extract_failure_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Find tool results with errors
    for result in &trajectory.tool_results {
        if result.is_error || result.content.to_lowercase().contains("error") {
            // Extract the key error message (first line or key phrase)
            let error_msg = extract_error_message(&result.content);

            // Only create pattern if we have a meaningful error message
            if error_msg.len() < 10 || error_msg == "AVOID:" {
                continue;
            }

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

/// Extract a short task category from the user query
fn extract_task_category(query: &str) -> String {
    // Get first meaningful phrase (up to 50 chars)
    let first_line = query.lines().next().unwrap_or(query);
    let words: Vec<&str> = first_line.split_whitespace().take(8).collect();
    let category = words.join(" ");
    if category.len() > 60 {
        format!("{}...", &category[..60])
    } else {
        category
    }
}

/// Extract key error message from tool result
fn extract_error_message(content: &str) -> String {
    // Look for common error patterns
    let lines: Vec<&str> = content.lines().collect();

    // Find line with "error" or "Error"
    for line in &lines {
        let lower = line.to_lowercase();
        if lower.contains("error:") || lower.contains("failed:") {
            return truncate(line.trim(), 150).to_string();
        }
    }

    // Look for exit code
    for line in &lines {
        if line.contains("exit code") || line.contains("Exit code") {
            return truncate(line.trim(), 150).to_string();
        }
    }

    // First non-empty line
    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.len() > 5 {
            return truncate(trimmed, 150).to_string();
        }
    }

    truncate(content, 150).to_string()
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
    }

    #[test]
    fn test_extract_failure_patterns() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Run the tests".into(),
            assistant_content: "Let me try again".into(),
            tool_calls: vec![],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "Error: test failed".into(),
                is_error: true,
            }],
            verdict: Some(Verdict { success: false, confidence: 0.8 }),
        };

        let patterns = extract_failure_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "failure");
        assert!(patterns[0].context_query.contains("Pitfall"));
        assert!(patterns[0].context_query.contains("test failed"));
    }
}
