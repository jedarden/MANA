//! Trajectory parsing from JSONL logs
//!
//! Parses Claude Code JSONL format to reconstruct trajectories
//! for pattern extraction. Now captures ALL log types including:
//! - System messages and prompts
//! - Thinking/reasoning blocks
//! - Images and document references
//! - Error messages
//! - Pure conversation (non-tool) sessions

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
    /// Extended fields for comprehensive ingestion
    pub thinking_content: Vec<ThinkingBlock>,
    pub system_messages: Vec<SystemMessage>,
    pub images: Vec<ImageReference>,
    pub documents: Vec<DocumentReference>,
    pub error_messages: Vec<ErrorMessage>,
    /// Whether this is a pure conversation (no tool calls)
    pub is_conversation_only: bool,
    /// All user messages in the session (not just the first query)
    pub user_messages: Vec<String>,
    /// Metadata about the trajectory
    pub metadata: TrajectoryMetadata,
}

impl Default for Trajectory {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            user_query: String::new(),
            assistant_content: String::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            verdict: None,
            thinking_content: Vec::new(),
            system_messages: Vec::new(),
            images: Vec::new(),
            documents: Vec::new(),
            error_messages: Vec::new(),
            is_conversation_only: true,
            user_messages: Vec::new(),
            metadata: TrajectoryMetadata::default(),
        }
    }
}

/// A tool call from the assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    /// Tool use ID for matching with results
    pub tool_use_id: Option<String>,
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

/// A thinking/reasoning block from extended thinking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub content: String,
    /// Position in the conversation (message index)
    pub position: usize,
}

/// System message or prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: String,
    pub msg_type: SystemMessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemMessageType {
    SystemPrompt,
    SystemReminder,
    Context,
    Other(String),
}

/// Reference to an image in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageReference {
    pub source_type: String, // "base64", "url", "file"
    pub media_type: Option<String>, // "image/png", "image/jpeg", etc.
    /// For file references, the path
    pub file_path: Option<String>,
    /// Position in conversation
    pub position: usize,
}

/// Reference to a document (PDF, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentReference {
    pub doc_type: String, // "pdf", "text", etc.
    pub source: String, // "base64", "url", "file"
    pub file_path: Option<String>,
    pub position: usize,
}

/// Error message from Claude Code or tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub error_type: ErrorType,
    pub message: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ErrorType {
    ToolError,
    ApiError,
    SystemError,
    ValidationError,
    Other(String),
}

/// Metadata about the trajectory
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrajectoryMetadata {
    /// Number of turns in the conversation
    pub turn_count: usize,
    /// Total number of tool calls
    pub tool_call_count: usize,
    /// Whether the session had errors
    pub had_errors: bool,
    /// Unique tools used
    pub tools_used: Vec<String>,
    /// Message types encountered
    pub message_types: Vec<String>,
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
///
/// Now captures ALL log types:
/// - user messages (all of them, not just the first)
/// - assistant messages (text, tool_use, thinking)
/// - system messages and prompts
/// - error messages
/// - images and document references
/// - Pure conversation sessions (no tool calls)
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

        // Track message types for metadata
        if !session.message_types.contains(&msg_type.to_string()) {
            session.message_types.push(msg_type.to_string());
        }
        session.message_count += 1;

        match msg_type {
            "user" => {
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        // Process content array for all content types
                        if let Some(arr) = content.as_array() {
                            for item in arr {
                                if let Some(obj) = item.as_object() {
                                    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

                                    match item_type {
                                        "tool_result" => {
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

                                                // Check for error content
                                                if is_error {
                                                    session.error_messages.push(ErrorMessage {
                                                        error_type: ErrorType::ToolError,
                                                        message: content_str.clone(),
                                                        context: Some(tool_use_id.to_string()),
                                                    });
                                                }

                                                session.tool_results.push(ToolResult {
                                                    tool_use_id: tool_use_id.to_string(),
                                                    content: content_str,
                                                    is_error,
                                                });
                                            }
                                        }
                                        "image" => {
                                            // Extract image reference
                                            let source = obj.get("source").and_then(|s| s.as_object());
                                            let source_type = source
                                                .and_then(|s| s.get("type"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();
                                            let media_type = source
                                                .and_then(|s| s.get("media_type"))
                                                .and_then(|v| v.as_str())
                                                .map(String::from);

                                            session.images.push(ImageReference {
                                                source_type,
                                                media_type,
                                                file_path: None,
                                                position: session.message_count,
                                            });
                                        }
                                        "document" => {
                                            // Extract document reference
                                            let source = obj.get("source").and_then(|s| s.as_object());
                                            let doc_type = source
                                                .and_then(|s| s.get("type"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();

                                            session.documents.push(DocumentReference {
                                                doc_type: doc_type.clone(),
                                                source: doc_type,
                                                file_path: None,
                                                position: session.message_count,
                                            });
                                        }
                                        "text" => {
                                            // Capture text content
                                            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                                                // Always add to user_messages list
                                                if !text.is_empty() && text.len() > 3 {
                                                    session.user_messages.push(text.to_string());
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                        // Also capture plain user text for the query (first meaningful one)
                        if let Some(text) = extract_text_content(content) {
                            // Always add non-empty text to user_messages
                            if !text.is_empty() && text.len() > 3
                               && !text.contains("<command-")
                               && !text.contains("<local-command") {
                                if !session.user_messages.contains(&text) {
                                    session.user_messages.push(text.clone());
                                }
                            }

                            // Set first meaningful query
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
                        // Parse content array for all content types
                        if let Some(arr) = content.as_array() {
                            for item in arr {
                                if let Some(obj) = item.as_object() {
                                    let item_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");

                                    match item_type {
                                        "tool_use" => {
                                            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                                                let input = obj.get("input").cloned().unwrap_or(serde_json::Value::Null);
                                                let tool_use_id = obj.get("id").and_then(|v| v.as_str()).map(String::from);

                                                // Track unique tools
                                                if !session.tools_used.contains(&name.to_string()) {
                                                    session.tools_used.push(name.to_string());
                                                }

                                                session.tool_calls.push(ToolCall {
                                                    tool_name: name.to_string(),
                                                    tool_input: input,
                                                    tool_use_id,
                                                });
                                            }
                                        }
                                        "text" => {
                                            if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                                                if !session.assistant_content.is_empty() {
                                                    session.assistant_content.push('\n');
                                                }
                                                session.assistant_content.push_str(text);
                                            }
                                        }
                                        "thinking" => {
                                            // Capture thinking/reasoning blocks
                                            if let Some(thinking) = obj.get("thinking").and_then(|v| v.as_str()) {
                                                session.thinking_blocks.push(ThinkingBlock {
                                                    content: thinking.to_string(),
                                                    position: session.message_count,
                                                });
                                            }
                                        }
                                        "image" => {
                                            // Assistant can also reference images
                                            let source_type = obj.get("source")
                                                .and_then(|s| s.as_object())
                                                .and_then(|s| s.get("type"))
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_string();

                                            session.images.push(ImageReference {
                                                source_type,
                                                media_type: None,
                                                file_path: None,
                                                position: session.message_count,
                                            });
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "system" => {
                // Capture system messages/prompts
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        if let Some(text) = extract_text_content(content) {
                            let msg_type = if text.contains("<system-reminder>") {
                                SystemMessageType::SystemReminder
                            } else if text.contains("context") || text.contains("Context") {
                                SystemMessageType::Context
                            } else {
                                SystemMessageType::SystemPrompt
                            };

                            session.system_messages.push(SystemMessage {
                                content: text,
                                msg_type,
                            });
                        }
                    }
                }
            }
            "error" => {
                // Capture error messages from Claude Code
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        if let Some(text) = extract_text_content(content) {
                            let error_type = if text.contains("API") || text.contains("api") {
                                ErrorType::ApiError
                            } else if text.contains("validation") || text.contains("Validation") {
                                ErrorType::ValidationError
                            } else {
                                ErrorType::SystemError
                            };

                            session.error_messages.push(ErrorMessage {
                                error_type,
                                message: text,
                                context: None,
                            });
                        }
                    }
                }
            }
            // Handle any other message types
            other => {
                // Store as system message with "Other" type
                if let Some(ref message) = msg.message {
                    if let Some(ref content) = message.content {
                        if let Some(text) = extract_text_content(content) {
                            if !text.is_empty() && text.len() > 10 {
                                session.system_messages.push(SystemMessage {
                                    content: text,
                                    msg_type: SystemMessageType::Other(other.to_string()),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Convert sessions to trajectories - NOW INCLUDES ALL SESSIONS
    let mut trajectories = Vec::new();

    for (session_id, data) in sessions {
        // Calculate metadata
        let had_errors = !data.error_messages.is_empty()
            || data.tool_results.iter().any(|r| r.is_error);
        let is_conversation_only = data.tool_calls.is_empty();

        // Include ALL sessions (not just those with tool calls)
        // Skip only completely empty sessions
        if data.user_query.is_empty()
           && data.assistant_content.is_empty()
           && data.tool_calls.is_empty()
           && data.thinking_blocks.is_empty() {
            continue;
        }

        let mut trajectory = Trajectory {
            session_id,
            user_query: data.user_query,
            assistant_content: data.assistant_content,
            tool_calls: data.tool_calls,
            tool_results: data.tool_results,
            verdict: None,
            thinking_content: data.thinking_blocks,
            system_messages: data.system_messages,
            images: data.images,
            documents: data.documents,
            error_messages: data.error_messages,
            is_conversation_only,
            user_messages: data.user_messages,
            metadata: TrajectoryMetadata {
                turn_count: data.message_count,
                tool_call_count: data.tools_used.len(),
                had_errors,
                tools_used: data.tools_used,
                message_types: data.message_types,
            },
        };

        // Judge the trajectory
        trajectory.verdict = Some(judge_trajectory(&trajectory));
        trajectories.push(trajectory);
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
    /// Extended fields for comprehensive ingestion
    thinking_blocks: Vec<ThinkingBlock>,
    system_messages: Vec<SystemMessage>,
    images: Vec<ImageReference>,
    documents: Vec<DocumentReference>,
    error_messages: Vec<ErrorMessage>,
    user_messages: Vec<String>,
    /// Track message position for ordering content
    message_count: usize,
    /// Track unique message types seen
    message_types: Vec<String>,
    /// Track unique tools used
    tools_used: Vec<String>,
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
/// Now also handles conversation-only trajectories and thinking-based sessions
fn judge_trajectory(trajectory: &Trajectory) -> Verdict {
    // Check for errors in tool results and error messages
    let has_errors = trajectory.tool_results.iter().any(|r| {
        r.is_error ||
        r.content.to_lowercase().contains("error:") ||
        r.content.to_lowercase().contains("failed:") ||
        r.content.to_lowercase().contains("exception:")
    }) || !trajectory.error_messages.is_empty();

    // Check for positive feedback patterns in assistant response
    let has_completion = trajectory.assistant_content.to_lowercase().contains("complete") ||
        trajectory.assistant_content.to_lowercase().contains("done") ||
        trajectory.assistant_content.to_lowercase().contains("success") ||
        trajectory.assistant_content.to_lowercase().contains("finished");

    // Check if tools were executed
    let has_tool_execution = !trajectory.tool_calls.is_empty();

    // Check if there was meaningful thinking/reasoning
    let has_thinking = !trajectory.thinking_content.is_empty();

    // Check if there was meaningful conversation
    let has_conversation = !trajectory.assistant_content.is_empty()
        && trajectory.assistant_content.len() > 50;

    // Scoring heuristics:
    // - Tools ran without errors = success
    // - Completion language = success
    // - Thinking blocks present (reasoning happened) = likely success
    // - Meaningful conversation response = success for conversation-only
    let success = !has_errors && (has_tool_execution || has_completion || has_thinking || has_conversation);

    let confidence = if has_errors {
        0.8
    } else if has_tool_execution {
        0.85  // High confidence for tool execution without errors
    } else if has_completion {
        0.9
    } else if has_thinking {
        0.75  // Thinking blocks indicate deliberate reasoning
    } else if has_conversation {
        0.7   // Meaningful conversation response
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
                tool_use_id: Some("123".into()),
            }],
            tool_results: vec![ToolResult {
                tool_use_id: "123".into(),
                content: "File edited successfully".into(),
                is_error: false,
            }],
            ..Default::default()
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
            ..Default::default()
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(!verdict.success);
    }

    #[test]
    fn test_judge_trajectory_conversation_only() {
        // Test that conversation-only trajectories are judged correctly
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "What is the purpose of this function?".into(),
            assistant_content: "This function handles user authentication by validating credentials against the database and returning a session token.".into(),
            is_conversation_only: true,
            ..Default::default()
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(verdict.success);
        assert!(verdict.confidence >= 0.7);
    }

    #[test]
    fn test_judge_trajectory_with_thinking() {
        // Test that trajectories with thinking blocks are judged correctly
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "How should I refactor this?".into(),
            assistant_content: "Here's my recommendation".into(),
            thinking_content: vec![ThinkingBlock {
                content: "Let me analyze the code structure and identify potential improvements...".into(),
                position: 1,
            }],
            ..Default::default()
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(verdict.success);
        assert!(verdict.confidence >= 0.75);
    }

    #[test]
    fn test_judge_trajectory_with_error_messages() {
        // Test that trajectories with error messages are marked as failures
        let trajectory = Trajectory {
            session_id: "test".into(),
            user_query: "Run the build".into(),
            assistant_content: "The build failed".into(),
            error_messages: vec![ErrorMessage {
                error_type: ErrorType::SystemError,
                message: "Build failed with exit code 1".into(),
                context: None,
            }],
            ..Default::default()
        };

        let verdict = judge_trajectory(&trajectory);
        assert!(!verdict.success);
    }

    #[test]
    fn test_trajectory_default() {
        // Test that Default implementation works correctly
        let trajectory = Trajectory::default();
        assert!(trajectory.session_id.is_empty());
        assert!(trajectory.tool_calls.is_empty());
        assert!(trajectory.is_conversation_only);
        assert!(trajectory.thinking_content.is_empty());
        assert!(trajectory.system_messages.is_empty());
    }
}
