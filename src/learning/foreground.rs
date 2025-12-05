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

    // Create patterns for each tool call
    for tool_call in trajectory.tool_calls.iter().take(MAX_PATTERNS_PER_TRAJECTORY) {
        let context_query = format!(
            "User asked: {}\nApproach: Used {} tool",
            truncate(&trajectory.user_query, 200),
            tool_call.tool_name
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

    // If no tool calls, create a pattern from the assistant response
    if patterns.is_empty() && !trajectory.assistant_content.is_empty() {
        let context_query = format!(
            "Query: {}\nResponse approach: {}",
            truncate(&trajectory.user_query, 200),
            truncate(&trajectory.assistant_content, 300)
        );

        let pattern_hash = hash_string(&context_query);

        patterns.push(Pattern {
            id: 0,
            pattern_hash,
            tool_type: "response".to_string(),
            context_query,
            success_count: 1,
            failure_count: 0,
            embedding_id: None,
        });
    }

    patterns
}

/// Extract patterns from failed trajectories (what to avoid)
fn extract_failure_patterns(trajectory: &Trajectory) -> Vec<Pattern> {
    let mut patterns = Vec::new();

    // Find tool results with errors
    for result in &trajectory.tool_results {
        if result.is_error || result.content.to_lowercase().contains("error") {
            let context_query = format!(
                "AVOID: {}\nError: {}",
                truncate(&trajectory.user_query, 150),
                truncate(&result.content, 200)
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
                tool_input: serde_json::json!({"file_path": "main.rs"}),
            }],
            tool_results: vec![],
            verdict: Some(Verdict { success: true, confidence: 0.9 }),
        };

        let patterns = extract_success_patterns(&trajectory);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].tool_type, "Edit");
        assert!(patterns[0].context_query.contains("type error"));
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
        assert!(patterns[0].context_query.contains("AVOID"));
    }
}
