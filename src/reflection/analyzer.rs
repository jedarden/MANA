//! Trajectory analysis for reflection
//!
//! Analyzes trajectory outcomes to determine success/failure
//! and judge pattern effectiveness.

use crate::learning::trajectory::Trajectory;
use super::verdict::{Verdict, ReflectionVerdict, compute_trajectory_hash};
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
}

impl TrajectoryAnalyzer {
    /// Create a new trajectory analyzer with default settings
    pub fn new() -> Self {
        Self {
            max_boost: 5,
            max_penalty: -5,
        }
    }

    /// Create with custom boost/penalty limits
    pub fn with_limits(max_boost: i32, max_penalty: i32) -> Self {
        Self { max_boost, max_penalty }
    }

    /// Analyze a trajectory to determine its outcome
    pub fn analyze(&self, trajectory: &Trajectory) -> TrajectoryOutcome {
        // Count explicit errors (is_error=true) - these are real failures
        let explicit_errors: Vec<ErrorType> = trajectory.tool_results
            .iter()
            .filter(|r| r.is_error)
            .map(|_| ErrorType::Other("explicit error".into()))
            .collect();

        // Only look for content-based errors if the content is short (likely error output)
        // Long content (>2000 chars) is likely code/docs and should be ignored
        let content_errors: Vec<ErrorType> = trajectory.tool_results
            .iter()
            .filter(|r| !r.is_error && r.content.len() < 2000)
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

    /// Judge a trajectory and produce a verdict
    pub fn judge(&self, outcome: &TrajectoryOutcome, trajectory: &Trajectory) -> Option<ReflectionVerdict> {
        let trajectory_hash = compute_trajectory_hash(
            &trajectory.session_id,
            &trajectory.user_query,
            &trajectory.tool_calls,
        );

        // Determine verdict based on outcome
        let verdict = if outcome.success && outcome.retry_count == 0 {
            // Clean success - effective
            Verdict::effective(outcome.confidence, self.max_boost)
        } else if outcome.success && outcome.retry_count > 0 {
            // Success but with retries - reduced effectiveness
            let reduced_confidence = outcome.confidence * (1.0 - (outcome.retry_count as f32 * 0.1)).max(0.3);
            Verdict::effective(reduced_confidence, self.max_boost / 2)
        } else if !outcome.success && !outcome.error_types.is_empty() {
            // Failure with errors - harmful
            let root_cause = self.analyze_root_cause(&outcome.error_types);
            Verdict::harmful(outcome.confidence, self.max_penalty, root_cause)
        } else if !outcome.success && outcome.abandoned {
            // Abandoned - ineffective
            Verdict::ineffective(outcome.confidence)
        } else {
            // Ambiguous - neutral
            Verdict::neutral()
        };

        debug!(
            "Verdict for trajectory {}: {:?} (confidence: {:.2})",
            trajectory_hash,
            verdict.category,
            verdict.confidence
        );

        Some(ReflectionVerdict::new(trajectory_hash, None, verdict))
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
    fn test_analyze_failure_with_error() {
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "error: cannot find type `Foo`".into(),
                is_error: false,
            }],
            "Let me try to fix this.",
        );

        let outcome = analyzer.analyze(&trajectory);
        assert!(!outcome.success);
        assert!(!outcome.error_types.is_empty());
        assert_eq!(outcome.error_types[0], ErrorType::CompileError);
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
    fn test_judge_harmful() {
        let analyzer = TrajectoryAnalyzer::new();

        let trajectory = make_trajectory(
            vec![ToolCall {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
            }],
            vec![ToolResult {
                tool_use_id: "1".into(),
                content: "error: compile error\nBuild failed".into(),
                is_error: true,
            }],
            "Let me try a different approach.",
        );

        let outcome = analyzer.analyze(&trajectory);
        let verdict = analyzer.judge(&outcome, &trajectory).unwrap();

        assert_eq!(verdict.verdict.category, VerdictCategory::Harmful);
        assert!(verdict.verdict.score_impact < 0);
        assert!(verdict.verdict.root_cause.is_some());
    }

    #[test]
    fn test_error_type_detection() {
        assert_eq!(
            ErrorType::from_content("error: cannot find type `Foo`"),
            Some(ErrorType::CompileError)
        );
        assert_eq!(
            ErrorType::from_content("panic: index out of bounds"),
            Some(ErrorType::RuntimeError)
        );
        assert_eq!(
            ErrorType::from_content("No such file or directory"),
            Some(ErrorType::FileNotFound)
        );
        assert_eq!(
            ErrorType::from_content("command timed out after 30s"),
            Some(ErrorType::Timeout)
        );
    }
}
