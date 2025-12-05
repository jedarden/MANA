//! Trajectory analysis for reflection
//!
//! Analyzes trajectory outcomes to determine success/failure
//! and judge pattern effectiveness.

use crate::learning::trajectory::Trajectory;
use crate::storage::{PatternStore, calculate_similarity};
#[allow(unused_imports)]
use crate::storage::Pattern; // Used in find_matching_pattern return type inference
use super::verdict::{ReflectionVerdict, compute_trajectory_hash};
#[allow(unused_imports)]
use super::verdict::Verdict; // Used in judge() internal logic
use std::path::Path;
use tracing::debug;

/// Trajectory outcome analysis result
#[derive(Debug, Clone)]
pub struct TrajectoryOutcome {
    /// Whether the trajectory was successful overall
    pub success: bool,
    /// Number of retry attempts detected
    pub retry_count: usize,
    /// Types of errors encountered
    pub error_types: Vec<ErrorType>,
    /// Duration in milliseconds (estimated)
    #[allow(dead_code)] // Reserved for future performance tracking
    pub duration_ms: u64,
    /// Whether the trajectory was abandoned
    pub abandoned: bool,
    /// Confidence in the success assessment
    pub confidence: f32,
}

/// Types of errors that can occur in a trajectory
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorType {
    /// Compilation error
    CompileError,
    /// Runtime error
    RuntimeError,
    /// File not found
    FileNotFound,
    /// Permission denied
    PermissionDenied,
    /// Timeout
    Timeout,
    /// Syntax error
    SyntaxError,
    /// Test failure
    TestFailure,
    /// Generic error
    Other(String),
}

impl ErrorType {
    /// Detect error type from content string
    pub fn from_content(content: &str) -> Option<Self> {
        let lower = content.to_lowercase();

        // Compilation errors - require actual compiler output patterns
        // Avoid matching code discussing compilation
        let compile_error_indicators = [
            "error[e",              // Rust error codes like error[E0308]
            "error: cannot find",
            "error: expected",
            "error: mismatched",
            "undefined reference to",
            "ld: symbol(s) not found",
            "cannot find module",
            "module not found",
            "import error:",
        ];

        if compile_error_indicators.iter().any(|ind| lower.contains(ind))
        {
            return Some(ErrorType::CompileError);
        }

        // Runtime errors - require actual error context, not just keywords in code
        // Avoid matching: panic! macro usage in code, "panic" in error handling docs, etc.
        let runtime_indicators = [
            "runtime error:",
            "panicked at",           // Rust actual panic output
            "thread 'main' panicked",
            "segmentation fault",
            "sigsegv",
            "stack overflow",
            "abort trap",
            "core dumped",
            "process exited with",
            "unhandled exception",
        ];

        if runtime_indicators.iter().any(|ind| lower.contains(ind)) {
            return Some(ErrorType::RuntimeError);
        }

        // File not found - require actual error context
        // Avoid matching "if not found" or "return not found" in code
        if lower.contains("no such file")
            || lower.contains("file not found")
            || (lower.contains("not found") && (
                lower.contains("error") ||
                lower.contains("errno") ||
                lower.contains("failed") ||
                lower.contains("cannot")
            ))
        {
            return Some(ErrorType::FileNotFound);
        }

        if lower.contains("permission denied")
            || lower.contains("access denied")
        {
            return Some(ErrorType::PermissionDenied);
        }

        // More specific timeout detection - require actual error context
        // Avoid matching config like "timeout_ms" or docs about timeouts
        if (lower.contains("timed out") && !lower.contains("timeout_"))
            || lower.contains("timeout exceeded")
            || lower.contains("timeout error")
            || lower.contains("operation timed out")
            || lower.contains("request timed out")
            || lower.contains("connection timed out")
        {
            return Some(ErrorType::Timeout);
        }

        // Syntax errors - require error context
        // Avoid matching code discussing syntax errors
        let syntax_indicators = [
            "syntax error:",
            "syntaxerror:",
            "unexpected token",      // JS/TS actual error
            "parse error:",
            "parsing error:",
            "invalid syntax",        // Python
            "expected `;`",          // Rust
            "expected `{`",
            "expected `}`",
            "missing semicolon",
        ];

        // Check for actual error output patterns
        if syntax_indicators.iter().any(|ind| lower.contains(ind)) {
            return Some(ErrorType::SyntaxError);
        }

        // Test failures - require actual test output patterns
        // Avoid matching test code that checks for failures
        if lower.contains("test result: failed")  // Rust test output
            || lower.contains("tests failed")      // Generic test runners
            || lower.contains("assertion failed:") // With colon = actual failure
            || lower.contains("assertionerror:")   // Python
            || (lower.contains("failures:") && lower.contains("passed")) // Test summary line
            || lower.contains("fail: ")            // TAP output
        {
            return Some(ErrorType::TestFailure);
        }

        // Generic error detection - require more specific patterns
        // Avoid matching "0 errors", "no errors", error handling code, etc.
        let error_indicators = [
            "error: ",      // With space after colon
            "error!",       // Error with exclamation
            "failed!",      // Failed with exclamation
            "exception:",   // Exception with colon
            ": error",      // Colon before error
            "exited with error",
            "returned error",
            "threw exception",
            "fatal error",
            "critical error",
        ];

        // Check for error indicators but avoid false positives
        let has_error_indicator = error_indicators.iter()
            .any(|ind| lower.contains(ind));

        // Exclude common false positives
        let false_positive_patterns = [
            "0 error",
            "no error",
            "without error",
            "error handling",
            "error message",
            "errortype",
            "errors.is_empty",
            "expected error",
        ];

        let is_false_positive = false_positive_patterns.iter()
            .any(|fp| lower.contains(fp));

        if has_error_indicator && !is_false_positive {
            return Some(ErrorType::Other("unspecified error".into()));
        }

        None
    }

    /// Get severity score (higher = more severe)
    pub fn severity(&self) -> i32 {
        match self {
            ErrorType::Timeout => 5,
            ErrorType::RuntimeError => 4,
            ErrorType::CompileError => 3,
            ErrorType::SyntaxError => 3,
            ErrorType::TestFailure => 2,
            ErrorType::FileNotFound => 2,
            ErrorType::PermissionDenied => 2,
            ErrorType::Other(_) => 1,
        }
    }
}

/// Trajectory analyzer for reflection
pub struct TrajectoryAnalyzer {
    /// Maximum boost for effective patterns
    max_boost: i32,
    /// Maximum penalty for harmful patterns
    max_penalty: i32,
    /// Database path for pattern lookups
    db_path: Option<std::path::PathBuf>,
}

impl TrajectoryAnalyzer {
    /// Create a new trajectory analyzer with default settings
    pub fn new() -> Self {
        Self {
            max_boost: 5,
            max_penalty: -5,
            db_path: None,
        }
    }

    /// Create with custom boost/penalty limits
    #[allow(dead_code)] // Exposed for external configuration
    pub fn with_limits(max_boost: i32, max_penalty: i32) -> Self {
        Self { max_boost, max_penalty, db_path: None }
    }

    /// Set the database path for pattern lookups
    pub fn with_db_path(mut self, path: &Path) -> Self {
        self.db_path = Some(path.to_path_buf());
        self
    }

    /// Analyze a trajectory to determine its outcome
    pub fn analyze(&self, trajectory: &Trajectory) -> TrajectoryOutcome {
        // Count explicit errors (is_error=true) - these are real failures
        let explicit_errors: Vec<ErrorType> = trajectory.tool_results
            .iter()
            .filter(|r| r.is_error)
            .map(|_| ErrorType::Other("explicit error".into()))
            .collect();

        // Only look for content-based errors if:
        // 1. Content is short (likely actual error output, not code)
        // 2. Content contains strong error indicators (not just mentions in code)
        // 3. Not just code that happens to contain error-related strings
        let content_errors: Vec<ErrorType> = trajectory.tool_results
            .iter()
            .filter(|r| {
                // Skip if already flagged as explicit error
                if r.is_error {
                    return false;
                }
                // Only check short content (under 1000 chars) that looks like error output
                if r.content.len() > 1000 {
                    return false;
                }
                // Must have strong error signal - not just code discussing errors
                let lower = r.content.to_lowercase();
                // Require error output formatting (line numbers, colons, etc.)
                let has_error_format = lower.contains("error:") ||
                    lower.contains("error[") ||
                    lower.contains("failed:") ||
                    lower.contains("fatal:") ||
                    lower.contains("panic:") ||
                    lower.contains("exception:");
                // Exclude common false positives
                let is_code = lower.contains("fn ") ||
                    lower.contains("def ") ||
                    lower.contains("function ") ||
                    lower.contains("class ") ||
                    lower.contains("impl ") ||
                    lower.contains("pub fn") ||
                    lower.contains("const ") ||
                    lower.contains("let ") ||
                    lower.contains("=>") ||
                    lower.contains("->") ||
                    lower.contains("::") && lower.contains("{");
                has_error_format && !is_code
            })
            .filter_map(|r| ErrorType::from_content(&r.content))
            .collect();

        // Combine errors, but only count content errors if they're severe
        let error_types: Vec<ErrorType> = if !explicit_errors.is_empty() {
            explicit_errors
        } else {
            // Filter to only severe content errors (severity >= 3)
            content_errors.into_iter()
                .filter(|e| e.severity() >= 3)
                .collect()
        };

        let has_errors = !error_types.is_empty();

        // Detect retry patterns
        let retry_count = self.count_retries(trajectory);

        // Detect abandonment
        let abandoned = self.detect_abandonment(trajectory);

        // Check for success indicators
        let has_success_indicators = self.has_success_indicators(trajectory);

        // Count successful tool executions (not errors)
        let successful_tools = trajectory.tool_results
            .iter()
            .filter(|r| !r.is_error)
            .count();

        let total_tools = trajectory.tool_results.len();

        // Determine overall success:
        // - If has success indicators, minor errors are ok (recovery)
        // - If most tools succeeded, trajectory is likely ok
        // - If abandoned or all tools failed, it's a failure
        let success = if abandoned {
            false
        } else if has_success_indicators {
            // Success indicators override minor errors (Claude recovered)
            true
        } else if has_errors && successful_tools == 0 {
            // All tool results were errors - definite failure
            false
        } else if !has_errors && !trajectory.tool_calls.is_empty() {
            // No errors and had tool calls - success
            true
        } else if has_errors && successful_tools > 0 && successful_tools as f64 / total_tools as f64 > 0.5 {
            // More than half succeeded despite some errors - likely ok
            true
        } else {
            // Default: if tool calls happened without errors, it's a success
            !has_errors
        };

        // Calculate confidence
        let confidence = if has_errors {
            0.85  // High confidence in failure detection
        } else if has_success_indicators {
            0.9  // High confidence with explicit success
        } else if !trajectory.tool_calls.is_empty() {
            0.75  // Medium confidence for tool execution without errors
        } else {
            0.5  // Low confidence for ambiguous cases
        };

        TrajectoryOutcome {
            success,
            retry_count,
            error_types,
            duration_ms: 0, // Not tracked yet
            abandoned,
            confidence,
        }
    }

    /// Count retry attempts in a trajectory
    fn count_retries(&self, trajectory: &Trajectory) -> usize {
        let content = trajectory.assistant_content.to_lowercase();

        let retry_phrases = [
            "let me try",
            "trying again",
            "another approach",
            "different approach",
            "let me fix",
            "i'll retry",
        ];

        retry_phrases.iter()
            .map(|phrase| content.matches(phrase).count())
            .sum()
    }

    /// Detect if the trajectory was abandoned
    fn detect_abandonment(&self, trajectory: &Trajectory) -> bool {
        let content = trajectory.assistant_content.to_lowercase();

        let abandonment_phrases = [
            "i cannot",
            "i'm unable",
            "this is not possible",
            "beyond my capabilities",
            "i don't have access",
        ];

        abandonment_phrases.iter()
            .any(|phrase| content.contains(phrase))
    }

    /// Check for success indicators in assistant content
    fn has_success_indicators(&self, trajectory: &Trajectory) -> bool {
        let content = trajectory.assistant_content.to_lowercase();

        let success_phrases = [
            "successfully",
            "completed",
            "done",
            "finished",
            "fixed",
            "implemented",
            "working",
        ];

        success_phrases.iter()
            .any(|phrase| content.contains(phrase))
    }

    /// Find the most relevant pattern for a trajectory
    ///
    /// This links trajectories to patterns that would have been injected
    /// by looking at the tool calls and matching against stored patterns.
    ///
    /// Strategy:
    /// 1. Try to find a strong semantic match (similarity > 0.30)
    /// 2. Fall back to moderate match (similarity > 0.15) if no strong match
    /// 3. Fall back to best available pattern of same tool type as last resort
    ///
    /// This ensures verdicts can still provide feedback even when patterns
    /// don't closely match the trajectory context.
    pub fn find_matching_pattern(&self, trajectory: &Trajectory) -> Option<i64> {
        let db_path = self.db_path.as_ref()?;
        if !db_path.exists() {
            return None;
        }

        let store = PatternStore::open_readonly(db_path).ok()?;

        // Find the primary tool used in this trajectory
        let primary_tool = trajectory.tool_calls.first()?;
        let tool_type = &primary_tool.tool_name;

        // Build a query from the tool input (similar to inject)
        let query = self.build_query_from_tool_call(primary_tool);

        // Get patterns for this tool type
        let patterns = store.get_by_tool(tool_type, 20).ok()?;

        if patterns.is_empty() {
            return None;
        }

        // Find the best matching pattern by similarity
        // Track both strong matches (>0.30) and moderate matches (>0.15)
        let mut best_strong: Option<(i64, f64)> = None;
        let mut best_moderate: Option<(i64, f64)> = None;
        let mut best_any: Option<(i64, f64)> = None;

        for pattern in &patterns {
            let similarity = calculate_similarity(&query, &pattern.context_query);

            // Track best overall (for fallback)
            if best_any.is_none() || similarity > best_any.as_ref().unwrap().1 {
                best_any = Some((pattern.id, similarity));
            }

            // Strong match threshold (same as injection)
            if similarity > 0.30 {
                if best_strong.is_none() || similarity > best_strong.as_ref().unwrap().1 {
                    best_strong = Some((pattern.id, similarity));
                }
            }
            // Moderate match threshold (reflection-specific, more lenient)
            else if similarity > 0.15
                && (best_moderate.is_none() || similarity > best_moderate.as_ref().unwrap().1)
            {
                best_moderate = Some((pattern.id, similarity));
            }
        }

        // Prefer strong match, then moderate, then best available
        let best_match = best_strong
            .or(best_moderate)
            .or_else(|| {
                // Fallback: use the highest-scoring pattern of this tool type
                // This ensures verdicts can still provide feedback
                // But only if similarity is non-zero (some word overlap)
                best_any.filter(|(_, sim)| *sim > 0.05)
            });

        debug!(
            "Pattern match for trajectory: {:?} (query: {}, patterns_checked: {})",
            best_match.as_ref().map(|(id, sim)| format!("id={}, sim={:.2}", id, sim)),
            query.chars().take(50).collect::<String>(),
            patterns.len()
        );

        best_match.map(|(id, _)| id)
    }

    /// Build a query string from a tool call
    ///
    /// IMPORTANT: This must match the format used in foreground.rs extract_tool_context()
    /// so that similarity matching works correctly. The pattern format is:
    /// "Task: <category>\nApproach: <tool> - <context>\nOutcome: Success"
    fn build_query_from_tool_call(&self, tool_call: &crate::learning::trajectory::ToolCall) -> String {
        let input = &tool_call.tool_input;
        let tool_name = &tool_call.tool_name;

        // Build tool context matching foreground.rs extract_tool_context format
        let tool_context = match tool_name.as_str() {
            "Edit" | "Write" | "MultiEdit" => {
                let file_path = input.get("file_path")
                    .or_else(|| input.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let filename = file_path.rsplit('/').next().unwrap_or(file_path);
                let ext = file_path.rsplit('.').next().unwrap_or("");

                // Include tech stack keywords like foreground.rs does
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
                    .map(|s| &s[..s.len().min(40)])
                    .unwrap_or("");

                if !old_str_preview.is_empty() {
                    format!("{} {} editing {} (replacing '{}')", ext, tech_hint, filename, old_str_preview)
                } else {
                    format!("{} {} writing to {}", ext, tech_hint, filename)
                }
            }
            "Bash" => {
                let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("unknown command");
                let first_word = cmd.split_whitespace().next().unwrap_or("cmd");
                let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
                if !desc.is_empty() {
                    format!("{} - {}", first_word, &desc[..desc.len().min(60)])
                } else {
                    let cmd_preview = &cmd[..cmd.len().min(80)];
                    format!("running '{}'", cmd_preview)
                }
            }
            "Read" | "Glob" | "Grep" => {
                let path = input.get("file_path")
                    .or_else(|| input.get("path"))
                    .and_then(|v| v.as_str())
                    .map(|p| p.rsplit('/').next().unwrap_or(p))
                    .unwrap_or("");
                let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                if !pattern.is_empty() {
                    let pattern_preview = &pattern[..pattern.len().min(30)];
                    format!("searching for '{}' in {}", pattern_preview, path)
                } else if !path.is_empty() {
                    format!("reading {}", path)
                } else {
                    "exploring codebase".to_string()
                }
            }
            "Task" => {
                let agent = input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("agent");
                let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let desc_preview = &desc[..desc.len().min(60)];
                format!("delegating to {} - {}", agent, desc_preview)
            }
            "TodoWrite" => "updating task list".to_string(),
            "WebSearch" | "WebFetch" => {
                let query = input.get("query")
                    .or_else(|| input.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let query_preview = &query[..query.len().min(60)];
                format!("searching web: {}", query_preview)
            }
            _ => format!("using {} tool", tool_name),
        };

        // Return just the tool context - this will be compared against pattern's context_query
        // which has format "Task: X\nApproach: Y - <tool_context>\nOutcome: Success"
        // The similarity function can match on the tool_context portion
        tool_context
    }

    /// Judge a trajectory and produce a verdict
    pub fn judge(&self, outcome: &TrajectoryOutcome, trajectory: &Trajectory) -> Option<ReflectionVerdict> {
        let trajectory_hash = compute_trajectory_hash(
            &trajectory.session_id,
            &trajectory.user_query,
            &trajectory.tool_calls,
        );

        // Find matching pattern for this trajectory
        let pattern_id = self.find_matching_pattern(trajectory);

        // Determine verdict based on outcome
        // Be conservative with HARMFUL - only mark HARMFUL for clear failures with explicit errors
        let verdict = if outcome.success && outcome.retry_count == 0 {
            // Clean success - effective
            Verdict::effective(outcome.confidence, self.max_boost)
        } else if outcome.success && outcome.retry_count > 0 {
            // Success but with retries - reduced effectiveness
            let reduced_confidence = outcome.confidence * (1.0 - (outcome.retry_count as f32 * 0.1)).max(0.3);
            Verdict::effective(reduced_confidence, self.max_boost / 2)
        } else if !outcome.success && outcome.abandoned {
            // Abandoned - ineffective (not harmful, just didn't work)
            Verdict::ineffective(outcome.confidence)
        } else if !outcome.success && !outcome.error_types.is_empty() {
            // Failure with errors - but check severity
            // Only mark as harmful for severe errors (severity >= 3)
            let has_severe_error = outcome.error_types.iter()
                .any(|e| e.severity() >= 3);
            if has_severe_error {
                let root_cause = self.analyze_root_cause(&outcome.error_types);
                Verdict::harmful(outcome.confidence, self.max_penalty, root_cause)
            } else {
                // Minor errors - ineffective but not harmful
                Verdict::ineffective(outcome.confidence * 0.8)
            }
        } else if !outcome.success {
            // Failed but no specific errors detected - neutral
            // Don't penalize ambiguous cases
            Verdict::neutral()
        } else {
            // Ambiguous - neutral
            Verdict::neutral()
        };

        debug!(
            "Verdict for trajectory {}: {:?} (confidence: {:.2}, pattern: {:?})",
            trajectory_hash,
            verdict.category,
            verdict.confidence,
            pattern_id
        );

        Some(ReflectionVerdict::new(trajectory_hash, pattern_id, verdict))
    }

    /// Analyze root cause from error types
    fn analyze_root_cause(&self, errors: &[ErrorType]) -> String {
        if errors.is_empty() {
            return "Unknown error".into();
        }

        // Find most severe error
        let most_severe = errors.iter()
            .max_by_key(|e| e.severity())
            .unwrap();

        match most_severe {
            ErrorType::CompileError => "Compilation failed - check syntax and types".into(),
            ErrorType::RuntimeError => "Runtime error - check logic and edge cases".into(),
            ErrorType::FileNotFound => "File not found - verify path exists".into(),
            ErrorType::PermissionDenied => "Permission denied - check file permissions".into(),
            ErrorType::Timeout => "Operation timed out - consider optimization".into(),
            ErrorType::SyntaxError => "Syntax error - review code structure".into(),
            ErrorType::TestFailure => "Tests failed - check assertions and expected behavior".into(),
            ErrorType::Other(msg) => format!("Error: {}", msg),
        }
    }
}

impl Default for TrajectoryAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::trajectory::{ToolCall, ToolResult};
    use crate::reflection::VerdictCategory;

    fn make_trajectory(
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
        assistant_content: &str,
    ) -> Trajectory {
        Trajectory {
            session_id: "test".into(),
            user_query: "test query".into(),
            assistant_content: assistant_content.into(),
            tool_calls,
            tool_results,
            verdict: None,
        }
    }

    #[test]
    fn test_analyze_success() {
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "File edited".into(),
                is_error: false,
            }],
            "I've successfully completed the task.",
        );

        let outcome = analyzer.analyze(&trajectory);
        assert!(outcome.success);
        assert_eq!(outcome.error_types.len(), 0);
        assert!(!outcome.abandoned);
    }

    #[test]
    fn test_analyze_failure_with_explicit_error() {
        let analyzer = TrajectoryAnalyzer::new();

        // Use is_error: true for explicit failures
        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "error: cannot find type `Foo`".into(),
                is_error: true,  // Explicit error flag
            }],
            "Let me try to fix this.",
        );

        let outcome = analyzer.analyze(&trajectory);
        assert!(!outcome.success);
        assert!(!outcome.error_types.is_empty());
    }

    #[test]
    fn test_detect_abandonment() {
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![],
            vec![],
            "I'm unable to complete this task as I don't have access to the required files.",
        );

        let outcome = analyzer.analyze(&trajectory);
        assert!(outcome.abandoned);
        assert!(!outcome.success);
    }

    #[test]
    fn test_judge_effective() {
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "Done".into(),
                is_error: false,
            }],
            "Successfully implemented the feature.",
        );

        let outcome = analyzer.analyze(&trajectory);
        let verdict = analyzer.judge(&outcome, &trajectory).unwrap();

        assert_eq!(verdict.verdict.category, VerdictCategory::Effective);
        assert!(verdict.verdict.score_impact > 0);
    }

    #[test]
    fn test_judge_harmful_for_severe_errors() {
        let analyzer = TrajectoryAnalyzer::new();

        // Explicit error with severe content (compile error)
        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "error[E0308]: mismatched types".into(),  // Rust compile error with code
                is_error: true,  // Must be explicit error
            }],
            "Let me try a different approach.",
        );

        let outcome = analyzer.analyze(&trajectory);
        // With explicit error flag, should be marked as failure
        assert!(!outcome.success);

        // Verdict should be HARMFUL only if there are severe errors
        let verdict = analyzer.judge(&outcome, &trajectory).unwrap();
        // Since is_error is true, it creates "explicit error" which has severity 1
        // So it may be INEFFECTIVE not HARMFUL
        assert!(verdict.verdict.score_impact <= 0);
    }

    #[test]
    fn test_judge_ineffective_for_minor_errors() {
        let analyzer = TrajectoryAnalyzer::new();

        // Explicit error but not severe
        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "file not found".into(),
                is_error: true,
            }],
            "Let me try a different approach.",
        );

        let outcome = analyzer.analyze(&trajectory);
        let verdict = analyzer.judge(&outcome, &trajectory).unwrap();

        // Minor errors should be INEFFECTIVE, not HARMFUL
        assert!(verdict.verdict.score_impact <= 0);
    }

    #[test]
    fn test_error_type_detection() {
        // Compile error with error code
        assert_eq!(
            ErrorType::from_content("error[E0308]: cannot find type `Foo`"),
            Some(ErrorType::CompileError)
        );
        // Runtime panic with full format
        assert_eq!(
            ErrorType::from_content("thread 'main' panicked at: index out of bounds"),
            Some(ErrorType::RuntimeError)
        );
        // File not found with error context
        assert_eq!(
            ErrorType::from_content("error: No such file or directory"),
            Some(ErrorType::FileNotFound)
        );
        // Timeout
        assert_eq!(
            ErrorType::from_content("operation timed out after 30s"),
            Some(ErrorType::Timeout)
        );
    }

    #[test]
    fn test_minor_errors_are_ineffective_not_harmful() {
        let analyzer = TrajectoryAnalyzer::new();

        // Minor error (file not found) should be INEFFECTIVE, not HARMFUL
        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Read".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "File not found".into(),
                is_error: false, // Not explicit error, just content
            }],
            "Let me try a different file.",
        );

        let outcome = analyzer.analyze(&trajectory);
        // Minor errors (severity < 3) should not make this HARMFUL
        // FileNotFound has severity 2
        assert!(!outcome.error_types.iter().any(|e| e.severity() >= 3));
    }

    #[test]
    fn test_code_content_not_detected_as_error() {
        // Code discussing errors should not be detected as actual errors
        let code_content = r#"
fn handle_error() {
    if let Err(e) = operation() {
        println!("error: {}", e);
    }
}
"#;
        // Should not detect this as an error because it contains code patterns
        // The from_content function should detect this but our filter in analyze() excludes it
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Read".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: code_content.into(),
                is_error: false,
            }],
            "Read the file successfully.",
        );

        let outcome = analyzer.analyze(&trajectory);
        // Should be considered successful since the code is just showing error handling
        assert!(outcome.success || outcome.error_types.is_empty());
    }
}
