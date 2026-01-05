//! Trajectory failure point analysis
//!
//! Analyzes trajectories to identify:
//! - Exact failure points in execution
//! - Error classification and categorization
//! - Context-aware suggestions for each failure type
//! - Root cause identification

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::trajectory::{Trajectory, ToolResult};

/// Classification of failure types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureType {
    // Tool failures
    FileNotFound,
    PermissionDenied,
    SyntaxError,
    CompilationError,
    RuntimeError,
    TestFailure,

    // Logic failures
    WrongApproach,
    IncompleteImplementation,
    MissingDependency,
    ConfigurationError,

    // External failures
    NetworkError,
    ApiError,
    TimeoutError,
    ResourceExhausted,

    // Context failures
    MisunderstandingRequirement,
    IncorrectAssumption,
    MissingContext,

    Unknown(String),
}

impl FailureType {
    /// Get a human-readable description of this failure type
    pub fn description(&self) -> &'static str {
        match self {
            FailureType::FileNotFound => "File or directory does not exist",
            FailureType::PermissionDenied => "Insufficient permissions to access resource",
            FailureType::SyntaxError => "Code contains syntax errors",
            FailureType::CompilationError => "Code fails to compile",
            FailureType::RuntimeError => "Code crashes or panics during execution",
            FailureType::TestFailure => "Tests fail to pass",
            FailureType::WrongApproach => "Solution approach is fundamentally incorrect",
            FailureType::IncompleteImplementation => "Solution is partial or incomplete",
            FailureType::MissingDependency => "Required dependency or module is missing",
            FailureType::ConfigurationError => "Configuration is invalid or missing",
            FailureType::NetworkError => "Network connection or request failed",
            FailureType::ApiError => "API call failed or returned error",
            FailureType::TimeoutError => "Operation exceeded time limit",
            FailureType::ResourceExhausted => "Out of memory, disk space, or other resource",
            FailureType::MisunderstandingRequirement => "Misinterpreted user requirements",
            FailureType::IncorrectAssumption => "Made incorrect assumptions about code or context",
            FailureType::MissingContext => "Lacked necessary context to solve problem",
            FailureType::Unknown(_) => "Unclassified error",
        }
    }

    /// Check if this failure type is recoverable
    pub fn is_recoverable(&self) -> bool {
        match self {
            FailureType::FileNotFound => true,
            FailureType::PermissionDenied => false,
            FailureType::SyntaxError => true,
            FailureType::CompilationError => true,
            FailureType::RuntimeError => true,
            FailureType::TestFailure => true,
            FailureType::WrongApproach => false,
            FailureType::IncompleteImplementation => true,
            FailureType::MissingDependency => true,
            FailureType::ConfigurationError => true,
            FailureType::NetworkError => true,
            FailureType::ApiError => true,
            FailureType::TimeoutError => true,
            FailureType::ResourceExhausted => false,
            FailureType::MisunderstandingRequirement => false,
            FailureType::IncorrectAssumption => false,
            FailureType::MissingContext => false,
            FailureType::Unknown(_) => true,
        }
    }
}

/// A specific failure point in a trajectory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailurePoint {
    pub step_index: usize,
    pub tool_name: String,
    pub failure_type: FailureType,
    pub error_message: String,
    pub context_before: String,        // What was being attempted
    pub suggested_fix: Option<String>, // Context-aware suggestion
    pub severity: FailureSeverity,
    pub is_recoverable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureSeverity {
    Minor,      // Recoverable with retry
    Moderate,   // Needs different approach
    Major,      // Blocks progress
    Critical,   // Cascading failures
}

impl FailureSeverity {
    /// Get the numeric score for this severity (for sorting/comparison)
    pub fn score(&self) -> u8 {
        match self {
            FailureSeverity::Minor => 1,
            FailureSeverity::Moderate => 2,
            FailureSeverity::Major => 3,
            FailureSeverity::Critical => 4,
        }
    }
}

/// Complete analysis of a trajectory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryAnalysis {
    pub session_id: String,
    pub total_steps: usize,
    pub failure_points: Vec<FailurePoint>,
    pub success_rate: f64,
    pub primary_failure: Option<FailurePoint>,
    pub root_cause: Option<RootCause>,
    pub recovery_suggestions: Vec<String>,
    pub similar_past_failures: Vec<String>, // Pattern IDs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCause {
    pub description: String,
    pub category: FailureType,
    pub confidence: f64,
    pub contributing_factors: Vec<String>,
}

/// Analyzer for identifying failure points and root causes
pub struct FailureAnalyzer {
    // Could add configuration fields here
}

impl FailureAnalyzer {
    pub fn new() -> Self {
        Self {}
    }

    /// Analyze a trajectory for failure points
    pub fn analyze(&self, trajectory: &Trajectory) -> TrajectoryAnalysis {
        let mut failure_points = Vec::new();

        // Analyze tool results for errors
        for (idx, result) in trajectory.tool_results.iter().enumerate() {
            if result.is_error || self.looks_like_error(&result.content) {
                let tool_name = trajectory.tool_calls
                    .iter()
                    .find(|tc| tc.tool_use_id.as_ref() == Some(&result.tool_use_id))
                    .map(|tc| tc.tool_name.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                let failure_type = self.classify_error(&result.content, &tool_name);
                let severity = self.determine_severity(&failure_type, idx, trajectory.tool_results.len());
                let context = self.extract_context(trajectory, idx);
                let suggested_fix = self.suggest_fix(&failure_type, &result.content, &tool_name, trajectory);

                failure_points.push(FailurePoint {
                    step_index: idx,
                    tool_name,
                    failure_type: failure_type.clone(),
                    error_message: result.content.clone(),
                    context_before: context,
                    suggested_fix,
                    severity,
                    is_recoverable: failure_type.is_recoverable(),
                });
            }
        }

        // Analyze error messages
        for (idx, error_msg) in trajectory.error_messages.iter().enumerate() {
            let failure_type = self.classify_error(&error_msg.message, "System");
            let severity = FailureSeverity::Major; // System errors are typically serious

            failure_points.push(FailurePoint {
                step_index: trajectory.tool_results.len() + idx,
                tool_name: "System".to_string(),
                failure_type: failure_type.clone(),
                error_message: error_msg.message.clone(),
                context_before: error_msg.context.clone().unwrap_or_default(),
                suggested_fix: self.suggest_fix(&failure_type, &error_msg.message, "System", trajectory),
                severity,
                is_recoverable: failure_type.is_recoverable(),
            });
        }

        // Calculate success rate
        let total_steps = trajectory.tool_calls.len();
        let failed_steps = failure_points.len();
        let success_rate = if total_steps > 0 {
            1.0 - (failed_steps as f64 / total_steps as f64)
        } else {
            1.0
        };

        // Find primary failure (highest severity, earliest)
        let primary_failure = failure_points
            .iter()
            .max_by_key(|fp| (fp.severity.score(), -(fp.step_index as i32)))
            .cloned();

        // Identify root cause
        let root_cause = self.find_root_cause(&failure_points);

        // Generate recovery suggestions
        let recovery_suggestions = self.generate_recovery_suggestions(&failure_points, trajectory);

        TrajectoryAnalysis {
            session_id: trajectory.session_id.clone(),
            total_steps,
            failure_points,
            success_rate,
            primary_failure,
            root_cause,
            recovery_suggestions,
            similar_past_failures: Vec::new(), // TODO: implement similarity search
        }
    }

    /// Classify an error message into a failure type
    pub fn classify_error(&self, error: &str, tool: &str) -> FailureType {
        let error_lower = error.to_lowercase();

        // File-related errors
        if error_lower.contains("no such file")
            || error_lower.contains("not found") && (error_lower.contains("file") || error_lower.contains("directory"))
            || error_lower.contains("enoent")
            || error_lower.contains("cannot find") {
            return FailureType::FileNotFound;
        }

        // Permission errors
        if error_lower.contains("permission denied")
            || error_lower.contains("access denied")
            || error_lower.contains("eacces")
            || error_lower.contains("forbidden") {
            return FailureType::PermissionDenied;
        }

        // Syntax errors
        if error_lower.contains("syntax error")
            || error_lower.contains("syntaxerror")
            || error_lower.contains("unexpected token")
            || error_lower.contains("parse error")
            || error_lower.contains("invalid syntax") {
            return FailureType::SyntaxError;
        }

        // Compilation errors
        if error_lower.contains("compilation failed")
            || error_lower.contains("compiler error")
            || error_lower.contains("failed to compile")
            || error_lower.contains("build failed")
            || (tool == "Bash" && (error_lower.contains("cargo") || error_lower.contains("rustc")) && error_lower.contains("error")) {
            return FailureType::CompilationError;
        }

        // Runtime errors
        if error_lower.contains("runtime error")
            || error_lower.contains("panic")
            || error_lower.contains("segmentation fault")
            || error_lower.contains("sigsegv")
            || error_lower.contains("null pointer")
            || error_lower.contains("stack overflow") {
            return FailureType::RuntimeError;
        }

        // Test failures
        if error_lower.contains("test failed")
            || error_lower.contains("assertion failed")
            || error_lower.contains("test result: fail")
            || error_lower.contains("failures:") && error_lower.contains("test") {
            return FailureType::TestFailure;
        }

        // Dependency errors
        if error_lower.contains("cannot find package")
            || error_lower.contains("module not found")
            || error_lower.contains("no such module")
            || error_lower.contains("dependency")
            || error_lower.contains("import error")
            || error_lower.contains("modulenotfounderror") {
            return FailureType::MissingDependency;
        }

        // Configuration errors
        if error_lower.contains("configuration")
            || error_lower.contains("config")
            || error_lower.contains("invalid option")
            || error_lower.contains("missing required") {
            return FailureType::ConfigurationError;
        }

        // Network errors
        if error_lower.contains("network")
            || error_lower.contains("connection refused")
            || error_lower.contains("connection timeout")
            || error_lower.contains("could not resolve host")
            || error_lower.contains("dns") {
            return FailureType::NetworkError;
        }

        // API errors
        if error_lower.contains("api error")
            || error_lower.contains("http")
            || error_lower.contains("status code")
            || error_lower.contains("request failed") {
            return FailureType::ApiError;
        }

        // Timeout errors
        if error_lower.contains("timeout")
            || error_lower.contains("timed out")
            || error_lower.contains("deadline exceeded") {
            return FailureType::TimeoutError;
        }

        // Resource exhaustion
        if error_lower.contains("out of memory")
            || error_lower.contains("oom")
            || error_lower.contains("no space left")
            || error_lower.contains("disk full")
            || error_lower.contains("resource temporarily unavailable") {
            return FailureType::ResourceExhausted;
        }

        // Default to unknown with the error snippet
        FailureType::Unknown(error.chars().take(50).collect())
    }

    /// Generate context-aware suggestions for a failure
    pub fn suggest_fix(&self, failure_type: &FailureType, error: &str, tool: &str, trajectory: &Trajectory) -> Option<String> {
        match failure_type {
            FailureType::FileNotFound => {
                Some("Check if the file path is correct. Use `ls` or `find` to locate the file, or create it if it should exist.".to_string())
            }
            FailureType::PermissionDenied => {
                Some("Check file permissions with `ls -la`. You may need to use `chmod` to modify permissions or run with appropriate privileges.".to_string())
            }
            FailureType::SyntaxError => {
                Some("Review the code for syntax errors. Check for missing brackets, semicolons, or incorrect indentation.".to_string())
            }
            FailureType::CompilationError => {
                Some("Fix the compilation errors reported. Check type mismatches, missing imports, or API changes.".to_string())
            }
            FailureType::RuntimeError => {
                Some("Add error handling and validate inputs. Check for null/None values and add appropriate guards.".to_string())
            }
            FailureType::TestFailure => {
                Some("Review the failing test assertions. Update test expectations or fix the implementation to match requirements.".to_string())
            }
            FailureType::WrongApproach => {
                Some("Reconsider the solution approach. Review the requirements and consider alternative strategies.".to_string())
            }
            FailureType::IncompleteImplementation => {
                Some("Complete the implementation. Review the user query to ensure all requirements are addressed.".to_string())
            }
            FailureType::MissingDependency => {
                // Extract dependency name from error if possible
                let suggestion = if error.contains("cargo") || tool.contains("Rust") {
                    "Add the missing dependency to Cargo.toml and run `cargo build`.".to_string()
                } else if error.contains("npm") || error.contains("yarn") {
                    "Install the missing package with `npm install <package>` or `yarn add <package>`.".to_string()
                } else if error.contains("pip") || error.contains("python") {
                    "Install the missing module with `pip install <module>`.".to_string()
                } else {
                    "Install the missing dependency using the appropriate package manager.".to_string()
                };
                Some(suggestion)
            }
            FailureType::ConfigurationError => {
                Some("Review the configuration file for errors. Check syntax and ensure required fields are present.".to_string())
            }
            FailureType::NetworkError => {
                Some("Check network connectivity. Verify the URL/hostname and ensure the service is accessible.".to_string())
            }
            FailureType::ApiError => {
                Some("Check API documentation for correct usage. Verify authentication credentials and request parameters.".to_string())
            }
            FailureType::TimeoutError => {
                Some("Increase timeout duration or optimize the operation. Check for infinite loops or blocking operations.".to_string())
            }
            FailureType::ResourceExhausted => {
                Some("Free up system resources. Check for memory leaks, clean up temporary files, or reduce resource usage.".to_string())
            }
            FailureType::MisunderstandingRequirement => {
                Some("Re-read the user's request carefully. Ask for clarification if requirements are ambiguous.".to_string())
            }
            FailureType::IncorrectAssumption => {
                Some("Validate assumptions about the codebase. Use tools to inspect actual state before making changes.".to_string())
            }
            FailureType::MissingContext => {
                Some("Gather more context. Read related files, check documentation, or inspect the codebase structure.".to_string())
            }
            FailureType::Unknown(_) => {
                Some("Review the error message for clues. Search for similar errors or consult documentation.".to_string())
            }
        }
    }

    /// Identify the root cause from multiple failure points
    pub fn find_root_cause(&self, failures: &[FailurePoint]) -> Option<RootCause> {
        if failures.is_empty() {
            return None;
        }

        // Group failures by type
        let mut type_counts: HashMap<FailureType, usize> = HashMap::new();
        for failure in failures {
            *type_counts.entry(failure.failure_type.clone()).or_insert(0) += 1;
        }

        // Find the most common failure type
        let (primary_type, count) = type_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(t, c)| (t.clone(), *c))?;

        // Calculate confidence based on how dominant this failure type is
        let confidence = *count as f64 / failures.len() as f64;

        // Generate description and contributing factors
        let description = if failures.len() == 1 {
            format!("{}: {}", primary_type.description(), failures[0].error_message.chars().take(100).collect::<String>())
        } else {
            format!("Multiple {} errors ({} occurrences)", primary_type.description(), count)
        };

        let contributing_factors: Vec<String> = failures
            .iter()
            .filter(|f| f.failure_type == primary_type)
            .take(3)
            .map(|f| format!("Step {}: {}", f.step_index, f.tool_name))
            .collect();

        Some(RootCause {
            description,
            category: primary_type,
            confidence,
            contributing_factors,
        })
    }

    /// Get statistics on failure types across multiple analyses
    pub fn failure_statistics(&self, analyses: &[TrajectoryAnalysis]) -> FailureStats {
        let mut failure_type_counts: HashMap<FailureType, usize> = HashMap::new();
        let mut total_trajectories = 0;
        let mut failed_trajectories = 0;
        let mut total_failure_points = 0;
        let mut recoverable_failures = 0;

        for analysis in analyses {
            total_trajectories += 1;

            if !analysis.failure_points.is_empty() {
                failed_trajectories += 1;
                total_failure_points += analysis.failure_points.len();

                for failure in &analysis.failure_points {
                    *failure_type_counts.entry(failure.failure_type.clone()).or_insert(0) += 1;
                    if failure.is_recoverable {
                        recoverable_failures += 1;
                    }
                }
            }
        }

        let mut most_common_failures: Vec<(FailureType, usize)> = failure_type_counts
            .into_iter()
            .collect();
        most_common_failures.sort_by(|a, b| b.1.cmp(&a.1));

        let avg_failure_points_per_trajectory = if failed_trajectories > 0 {
            total_failure_points as f64 / failed_trajectories as f64
        } else {
            0.0
        };

        let recovery_rate = if total_failure_points > 0 {
            recoverable_failures as f64 / total_failure_points as f64
        } else {
            0.0
        };

        FailureStats {
            total_trajectories,
            failed_trajectories,
            failure_type_counts: most_common_failures.iter().cloned().collect(),
            most_common_failures,
            avg_failure_points_per_trajectory,
            recovery_rate,
        }
    }

    // Helper methods

    fn looks_like_error(&self, content: &str) -> bool {
        let lower = content.to_lowercase();
        lower.contains("error:")
            || lower.contains("error ")
            || lower.contains("failed:")
            || lower.contains("failed ")
            || lower.contains("exception:")
            || lower.contains("fatal:")
            || lower.contains("panic")
    }

    fn extract_context(&self, trajectory: &Trajectory, step_index: usize) -> String {
        // Try to get the tool call that led to this result
        if let Some(tool_call) = trajectory.tool_calls.get(step_index) {
            format!("Executing {} with input: {}",
                tool_call.tool_name,
                tool_call.tool_input.to_string().chars().take(100).collect::<String>())
        } else {
            "Unknown context".to_string()
        }
    }

    fn determine_severity(&self, failure_type: &FailureType, step_index: usize, total_steps: usize) -> FailureSeverity {
        // Early failures are more severe
        let position_factor = (step_index as f64 / total_steps.max(1) as f64);

        // Determine base severity from failure type
        let base_severity = match failure_type {
            FailureType::ResourceExhausted
            | FailureType::PermissionDenied
            | FailureType::WrongApproach => FailureSeverity::Critical,

            FailureType::CompilationError
            | FailureType::RuntimeError
            | FailureType::TestFailure
            | FailureType::MisunderstandingRequirement => FailureSeverity::Major,

            FailureType::SyntaxError
            | FailureType::MissingDependency
            | FailureType::ConfigurationError
            | FailureType::IncorrectAssumption => FailureSeverity::Moderate,

            _ => FailureSeverity::Minor,
        };

        // Early failures get upgraded in severity
        if position_factor < 0.3 && base_severity == FailureSeverity::Moderate {
            FailureSeverity::Major
        } else if position_factor < 0.3 && base_severity == FailureSeverity::Minor {
            FailureSeverity::Moderate
        } else {
            base_severity
        }
    }

    fn generate_recovery_suggestions(&self, failures: &[FailurePoint], trajectory: &Trajectory) -> Vec<String> {
        let mut suggestions = Vec::new();

        if failures.is_empty() {
            return suggestions;
        }

        // Add suggestions from each failure point
        for failure in failures.iter().take(3) {
            if let Some(suggestion) = &failure.suggested_fix {
                suggestions.push(suggestion.clone());
            }
        }

        // Add overall trajectory-level suggestions
        if failures.len() > 3 {
            suggestions.push(format!("Multiple failures detected ({}). Consider breaking the task into smaller steps.", failures.len()));
        }

        // Check for cascading failures
        let severity_critical_count = failures.iter().filter(|f| f.severity == FailureSeverity::Critical).count();
        if severity_critical_count > 0 {
            suggestions.push("Critical failures detected. Address fundamental issues before proceeding.".to_string());
        }

        suggestions
    }
}

impl Default for FailureAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct FailureStats {
    pub total_trajectories: usize,
    pub failed_trajectories: usize,
    pub failure_type_counts: HashMap<FailureType, usize>,
    pub most_common_failures: Vec<(FailureType, usize)>,
    pub avg_failure_points_per_trajectory: f64,
    pub recovery_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::trajectory::{Trajectory, ToolCall, ToolResult};

    #[test]
    fn test_classify_file_not_found() {
        let analyzer = FailureAnalyzer::new();
        let result = analyzer.classify_error("Error: No such file or directory: /path/to/file", "Read");
        assert_eq!(result, FailureType::FileNotFound);
    }

    #[test]
    fn test_classify_permission_denied() {
        let analyzer = FailureAnalyzer::new();
        let result = analyzer.classify_error("Permission denied: cannot access file", "Write");
        assert_eq!(result, FailureType::PermissionDenied);
    }

    #[test]
    fn test_classify_syntax_error() {
        let analyzer = FailureAnalyzer::new();
        let result = analyzer.classify_error("SyntaxError: unexpected token", "Edit");
        assert_eq!(result, FailureType::SyntaxError);
    }

    #[test]
    fn test_classify_compilation_error() {
        let analyzer = FailureAnalyzer::new();
        let result = analyzer.classify_error("error: could not compile `project`", "Bash");
        assert_eq!(result, FailureType::CompilationError);
    }

    #[test]
    fn test_analyze_trajectory_with_failures() {
        let analyzer = FailureAnalyzer::new();

        let trajectory = Trajectory {
            session_id: "test-session".to_string(),
            user_query: "Fix the bug".to_string(),
            tool_calls: vec![
                ToolCall {
                    tool_name: "Edit".to_string(),
                    tool_input: serde_json::json!({"file": "main.rs"}),
                    tool_use_id: Some("call1".to_string()),
                },
            ],
            tool_results: vec![
                ToolResult {
                    tool_use_id: "call1".to_string(),
                    content: "Error: No such file or directory: main.rs".to_string(),
                    is_error: true,
                },
            ],
            ..Default::default()
        };

        let analysis = analyzer.analyze(&trajectory);

        assert_eq!(analysis.failure_points.len(), 1);
        assert_eq!(analysis.failure_points[0].failure_type, FailureType::FileNotFound);
        assert!(analysis.failure_points[0].suggested_fix.is_some());
    }

    #[test]
    fn test_root_cause_identification() {
        let analyzer = FailureAnalyzer::new();

        let failures = vec![
            FailurePoint {
                step_index: 0,
                tool_name: "Edit".to_string(),
                failure_type: FailureType::SyntaxError,
                error_message: "syntax error".to_string(),
                context_before: "editing file".to_string(),
                suggested_fix: None,
                severity: FailureSeverity::Moderate,
                is_recoverable: true,
            },
            FailurePoint {
                step_index: 1,
                tool_name: "Bash".to_string(),
                failure_type: FailureType::SyntaxError,
                error_message: "syntax error".to_string(),
                context_before: "compiling".to_string(),
                suggested_fix: None,
                severity: FailureSeverity::Moderate,
                is_recoverable: true,
            },
        ];

        let root_cause = analyzer.find_root_cause(&failures);

        assert!(root_cause.is_some());
        let root_cause = root_cause.unwrap();
        assert_eq!(root_cause.category, FailureType::SyntaxError);
        assert_eq!(root_cause.confidence, 1.0);
    }

    #[test]
    fn test_failure_statistics() {
        let analyzer = FailureAnalyzer::new();

        let analyses = vec![
            TrajectoryAnalysis {
                session_id: "1".to_string(),
                total_steps: 2,
                failure_points: vec![
                    FailurePoint {
                        step_index: 0,
                        tool_name: "Edit".to_string(),
                        failure_type: FailureType::SyntaxError,
                        error_message: "error".to_string(),
                        context_before: "context".to_string(),
                        suggested_fix: None,
                        severity: FailureSeverity::Minor,
                        is_recoverable: true,
                    },
                ],
                success_rate: 0.5,
                primary_failure: None,
                root_cause: None,
                recovery_suggestions: vec![],
                similar_past_failures: vec![],
            },
        ];

        let stats = analyzer.failure_statistics(&analyses);

        assert_eq!(stats.total_trajectories, 1);
        assert_eq!(stats.failed_trajectories, 1);
        assert_eq!(stats.recovery_rate, 1.0);
    }

    #[test]
    fn test_failure_type_recoverability() {
        assert!(FailureType::FileNotFound.is_recoverable());
        assert!(!FailureType::PermissionDenied.is_recoverable());
        assert!(FailureType::SyntaxError.is_recoverable());
        assert!(!FailureType::WrongApproach.is_recoverable());
    }

    #[test]
    fn test_severity_scoring() {
        assert_eq!(FailureSeverity::Minor.score(), 1);
        assert_eq!(FailureSeverity::Moderate.score(), 2);
        assert_eq!(FailureSeverity::Major.score(), 3);
        assert_eq!(FailureSeverity::Critical.score(), 4);
    }
}
