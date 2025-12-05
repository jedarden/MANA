//! Trajectory parsing from JSONL logs
//!
//! Parses Claude Code JSONL format to reconstruct trajectories
//! for pattern extraction.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use tracing::debug;

/// A reconstructed trajectory from JSONL logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trajectory {
    pub session_id: String,
    pub user_query: String,
    pub assistant_content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
    pub verdict: Option<Verdict>,
}

/// A tool call from the assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

/// Result from a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Verdict on trajectory success/failure
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Verdict {
    pub success: bool,
    pub confidence: f32,
}

/// JSONL message from Claude Code logs - using untagged to handle various formats
#[derive(Debug, Deserialize)]
struct JsonlMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<MessageContent>,
}

#[derive(Debug, Deserialize)]
struct MessageContent {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<serde_json::Value>,
}

/// Parse trajectories from a JSONL file
pub fn parse_trajectories(path: &Path, start_offset: u64) -> Result<Vec<Trajectory>> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();

    if start_offset >= file_len {
        return Ok(vec![]);
    }

    let mut reader = BufReader::new(file);
    if start_offset > 0 {
        reader.seek(SeekFrom::Start(start_offset))?;
    }

    // Group messages by session
    let mut sessions: HashMap<String, SessionData> = HashMap::new();
    let default_session = "default".to_string();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if line.is_empty() {
            continue;
        }

        let msg: JsonlMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let msg_type = match &msg.msg_type {
            Some(t) => t.as_str(),
            None => continue,
        };

        let session_id = msg.session_id.clone().unwrap_or_else(|| default_session.clone());
        let session = sessions.entry(session_id).or_default();

        match msg_type {
            "user" => {
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        // Check if this is a tool_result message (nested in user message)
                        if let Some(arr) = content.as_array() {
                            for item in arr {
                                if let Some(obj) = item.as_object() {
                                    if obj.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                        if let Some(tool_use_id) = obj.get("tool_use_id").and_then(|v| v.as_str()) {
                                            let content_str = obj.get("content")
                                                .map(|c| {
                                                    if let Some(s) = c.as_str() { s.to_string() }
                                                    else { c.to_string() }
                                                })
                                                .unwrap_or_default();
                                            let is_error = obj.get("is_error")
                                                .and_then(|v| v.as_bool())
                                                .unwrap_or(false);

                                            session.tool_results.push(ToolResult {
                                                tool_use_id: tool_use_id.to_string(),
                                                content: content_str,
                                                is_error,
                                            });
                                        }
                                    }
                                }
                            }
                        }

                        // Also capture plain user text for the query
                        if let Some(text) = extract_text_content(content) {
                            // Skip command messages
                            if !text.contains("<command-")
                               && !text.contains("<local-command")
                               && !text.contains("Caveat:")
                               && !text.is_empty()
                               && session.user_query.is_empty()
                               && text.len() > 5
                            {
                                session.user_query = text;
                            }
                        }
                    }
                }
            }
            "assistant" => {
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        // Parse content array for tool_use and text
                        if let Some(arr) = content.as_array() {
                            for item in arr {
                                if let Some(obj) = item.as_object() {
                                    match obj.get("type").and_then(|v| v.as_str()) {
                                        Some("tool_use") => {
                                            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                                                let input = obj.get("input").cloned().unwrap_or(serde_json::Value::Null);
                                                session.tool_calls.push(ToolCall {
                                                    tool_name: name.to_string(),
                                                    tool_input: input,
                                                });
                                            }
                                        }
                                        Some("text") => {
                                            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                                                if !session.assistant_content.is_empty() {
                                                    session.assistant_content.push('\n');
                                                }
                                                session.assistant_content.push_str(text);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Convert sessions to trajectories
    let mut trajectories = Vec::new();

    for (session_id, data) in sessions {
        // Only include sessions with actual tool calls
        if !data.tool_calls.is_empty() {
            let mut trajectory = Trajectory {
                session_id,
                user_query: data.user_query,
                assistant_content: data.assistant_content,
                tool_calls: data.tool_calls,
                tool_results: data.tool_results,
                verdict: None,
            };

            // Judge the trajectory
            trajectory.verdict = Some(judge_trajectory(&trajectory));
            trajectories.push(trajectory);
        }
    }

    debug!("Parsed {} trajectories from {:?}", trajectories.len(), path);
    Ok(trajectories)
}

#[derive(Debug, Default)]
struct SessionData {
    user_query: String,
    assistant_content: String,
    tool_calls: Vec<ToolCall>,
    tool_results: Vec<ToolResult>,
}

fn extract_text_content(content: &serde_json::Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }

    if let Some(arr) = content.as_array() {
        let texts: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                if let Some(obj) = item.as_object() {
                    if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                        return obj.get("text").and_then(|v| v.as_str()).map(String::from);
                    }
                }
                None
            })
            .collect();

        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }

    None
}

/// Heuristic-based verdict judgment
///
/// Based on ReasoningBank paper: use simple heuristics first, LLM judge optional
fn judge_trajectory(trajectory: &Trajectory) -> Verdict {
    // Check for errors in tool results
    let has_errors = trajectory.tool_results.iter().any(|r| {
        r.is_error ||
        r.content.to_lowercase().contains("error:") ||
        r.content.to_lowercase().contains("failed:") ||
        r.content.to_lowercase().contains("exception:")
    });

    // Check for positive feedback patterns in assistant response
    let has_completion = trajectory.assistant_content.to_lowercase().contains("complete") ||
        trajectory.assistant_content.to_lowercase().contains("done") ||
        trajectory.assistant_content.to_lowercase().contains("success") ||
        trajectory.assistant_content.to_lowercase().contains("finished");

    // Check if tools were executed
    let has_tool_execution = !trajectory.tool_calls.is_empty();

    // Scoring heuristics: If tools ran without explicit errors, consider it success
    let success = !has_errors && (has_tool_execution || has_completion);

    let confidence = if has_errors {
        0.8
    } else if has_tool_execution {
        0.85  // High confidence for tool execution without errors
    } else if has_completion {
        0.9
    } else {
        0.5
    };

    Verdict { success, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_judge_trajectory_success() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Fix the bug".into(),
            assistant_content: "I've completed the fix".into(),
            tool_calls: vec![ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({}),
            }],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "File edited successfully".into(),
                is_error: false,
            }],
            verdict: None,
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(verdict.success);
        assert!(verdict.confidence >= 0.8);
    }

    #[test]
    fn test_judge_trajectory_failure() {
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Fix the bug".into(),
            assistant_content: "Let me try again".into(),
            tool_calls: vec![],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "Error: File not found".into(),
                is_error: true,
            }],
            verdict: None,
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(!verdict.success);
    }
}
