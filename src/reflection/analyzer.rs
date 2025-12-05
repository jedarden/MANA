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

        if lower.contains("compile") && lower.contains("error")
            || lower.contains("cannot find") && lower.contains("type")
            || lower.contains("undefined reference")
        {
            return Some(ErrorType::CompileError);
        }

        if lower.contains("runtime error")
            || lower.contains("panic")
            || lower.contains("segmentation fault")
            || lower.contains("stack overflow")
        {
            return Some(ErrorType::RuntimeError);
        }

        if lower.contains("no such file")
            || lower.contains("file not found")
            || lower.contains("not found")
        {
            return Some(ErrorType::FileNotFound);
        }

        if lower.contains("permission denied")
            || lower.contains("access denied")
        {
            return Some(ErrorType::PermissionDenied);
        }

        if lower.contains("timeout")
            || lower.contains("timed out")
        {
            return Some(ErrorType::Timeout);
        }

        if lower.contains("syntax error")
            || lower.contains("unexpected token")
            || lower.contains("parse error")
        {
            return Some(ErrorType::SyntaxError);
        }

        if lower.contains("test failed")
            || lower.contains("assertion failed")
            || lower.contains("failures:")
        {
            return Some(ErrorType::TestFailure);
        }

        if lower.contains("error:")
            || lower.contains("failed:")
            || lower.contains("exception:")
        {
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
        // Collect error types from tool results
        let error_types: Vec<ErrorType> = trajectory.tool_results
            .iter()
            .filter_map(|r| {
                if r.is_error {
                    Some(ErrorType::Other("explicit error".into()))
                } else {
                    ErrorType::from_content(&r.content)
                }
            })
            .collect();

        let has_errors = !error_types.is_empty();

        // Detect retry patterns
        let retry_count = self.count_retries(trajectory);

        // Detect abandonment
        let abandoned = self.detect_abandonment(trajectory);

        // Check for success indicators
        let has_success_indicators = self.has_success_indicators(trajectory);

        // Determine overall success
        let success = !has_errors && !abandoned && (
            has_success_indicators || !trajectory.tool_calls.is_empty()
        );

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
