//! Pattern sanitization for secure sharing
//!
//! Sanitizes patterns before export to:
//! 1. Strip absolute paths to relative
//! 2. Redact secrets/tokens (API keys, passwords)
//! 3. Hash sensitive identifiers
//! 4. Generalize user-specific context

use crate::storage::Pattern;
use crate::sync::ExportablePattern;
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

/// Regex patterns for secret detection
static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // API keys (generic pattern: long alphanumeric strings)
        Regex::new(r#"(?i)(api[_-]?key|apikey|api_secret)['":\s=]+['"]?([a-zA-Z0-9_-]{20,})['"]?"#).unwrap(),
        // Bearer tokens
        Regex::new(r#"(?i)bearer\s+([a-zA-Z0-9._-]{20,})"#).unwrap(),
        // AWS keys
        Regex::new(r#"(?i)(aws_?access_?key_?id|aws_?secret_?access_?key)['":\s=]+['"]?([A-Z0-9/+=]{16,})['"]?"#).unwrap(),
        // GitHub tokens
        Regex::new(r#"(gh[pousr]_[a-zA-Z0-9]{36,})"#).unwrap(),
        // Private keys
        Regex::new(r#"-----BEGIN[A-Z ]*PRIVATE KEY-----"#).unwrap(),
        // Password patterns
        Regex::new(r#"(?i)(password|passwd|pwd)['":\s=]+['"]?([^\s'"]{8,})['"]?"#).unwrap(),
        // Database connection strings
        Regex::new(r#"(?i)(postgres|mysql|mongodb|redis)://[^\s]+"#).unwrap(),
        // JWT tokens
        Regex::new(r#"eyJ[a-zA-Z0-9_-]+\.eyJ[a-zA-Z0-9_-]+\.[a-zA-Z0-9_-]+"#).unwrap(),
        // Generic secrets in env vars
        Regex::new(r#"(?i)(secret|token|credential)['":\s=]+['"]?([a-zA-Z0-9_-]{16,})['"]?"#).unwrap(),
    ]
});

/// Common home directory patterns to strip
static HOME_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Unix home directories
        Regex::new(r#"/home/[^/\s]+/"#).unwrap(),
        Regex::new(r#"/Users/[^/\s]+/"#).unwrap(),
        Regex::new(r#"~/?"#).unwrap(),
        // Windows home directories
        Regex::new(r#"C:\\Users\\[^\\]+\\"#).unwrap(),
        Regex::new(r#"%USERPROFILE%"#).unwrap(),
        // Devpod/container paths
        Regex::new(r#"/workspaces/[^/\s]+/"#).unwrap(),
        Regex::new(r#"/workspace/"#).unwrap(),
    ]
});

/// Sanitize a pattern for export
///
/// Applies the following transformations:
/// 1. Strip absolute paths → relative
/// 2. Redact secrets/tokens
/// 3. Hash sensitive identifiers
/// 4. Generalize user-specific context
pub fn sanitize_pattern(pattern: &Pattern) -> ExportablePattern {
    let sanitized_context = sanitize_context(&pattern.context_query);

    // Recalculate hash from sanitized content
    let new_hash = calculate_hash(&sanitized_context);

    ExportablePattern {
        pattern_hash: new_hash,
        tool_type: pattern.tool_type.clone(),
        command_category: pattern.command_category.clone(),
        context_query: sanitized_context,
        success_count: pattern.success_count,
        failure_count: pattern.failure_count,
    }
}

/// Sanitize context query text
fn sanitize_context(context: &str) -> String {
    let mut result = context.to_string();

    // 1. Redact secrets first (before path manipulation might affect them)
    result = redact_secrets(&result);

    // 2. Strip absolute paths to relative
    result = strip_absolute_paths(&result);

    // 3. Generalize user-specific patterns
    result = generalize_user_context(&result);

    result
}

/// Redact secrets and sensitive tokens
fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();

    for pattern in SECRET_PATTERNS.iter() {
        // Replace the captured secret portion with [REDACTED]
        result = pattern.replace_all(&result, |caps: &regex::Captures| {
            if caps.len() > 2 {
                // Pattern captured key=value, replace just the value
                format!("{}[REDACTED]", &caps[1])
            } else if caps.len() > 1 {
                // Pattern captured just the secret
                "[REDACTED]".to_string()
            } else {
                "[REDACTED]".to_string()
            }
        }).to_string();
    }

    result
}

/// Strip absolute paths to relative
fn strip_absolute_paths(text: &str) -> String {
    let mut result = text.to_string();

    // Replace home directory patterns with empty string
    for pattern in HOME_PATTERNS.iter() {
        result = pattern.replace_all(&result, "").to_string();
    }

    // Replace remaining absolute Unix paths with just the filename
    let abs_unix = Regex::new(r#"/[a-zA-Z0-9_.-]+(/[a-zA-Z0-9_.-]+)+"#).unwrap();
    result = abs_unix.replace_all(&result, |caps: &regex::Captures| {
        let path = &caps[0];
        // Keep just the last component
        path.rsplit('/').next().unwrap_or(path).to_string()
    }).to_string();

    result
}

/// Generalize user-specific context
fn generalize_user_context(text: &str) -> String {
    let mut result = text.to_string();

    // Replace email addresses with placeholder
    let email_re = Regex::new(r#"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}"#).unwrap();
    result = email_re.replace_all(&result, "[email]").to_string();

    // Replace IP addresses with placeholder
    let ip_re = Regex::new(r#"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b"#).unwrap();
    result = ip_re.replace_all(&result, "[ip]").to_string();

    // Replace UUIDs with placeholder
    let uuid_re = Regex::new(r#"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"#).unwrap();
    result = uuid_re.replace_all(&result, "[uuid]").to_string();

    result
}

/// Calculate hash for deduplication
fn calculate_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Check if a pattern contains potentially sensitive information
/// Returns true if the pattern should be flagged for review
#[allow(dead_code)]
pub fn contains_sensitive_info(context: &str) -> bool {
    for pattern in SECRET_PATTERNS.iter() {
        if pattern.is_match(context) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_api_key() {
        let input = r#"api_key = "sk_live_abcdef1234567890abcdef""#;
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("sk_live"));
    }

    #[test]
    fn test_redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("eyJ"));
    }

    #[test]
    fn test_redact_github_token() {
        let input = "GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwxyz12";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("ghp_"));
    }

    #[test]
    fn test_strip_home_path() {
        let input = "/home/username/projects/myapp/src/main.rs";
        let result = strip_absolute_paths(input);
        assert!(!result.contains("username"));
        assert!(result.contains("main.rs"));
    }

    #[test]
    fn test_strip_mac_path() {
        let input = "/Users/jdoe/Developer/project/file.js";
        let result = strip_absolute_paths(input);
        assert!(!result.contains("jdoe"));
        assert!(result.contains("file.js"));
    }

    #[test]
    fn test_strip_devpod_path() {
        let input = "/workspaces/my-project/src/lib.rs";
        let result = strip_absolute_paths(input);
        assert!(!result.contains("my-project"));
        assert!(result.contains("lib.rs"));
    }

    #[test]
    fn test_generalize_email() {
        let input = "Contact: user@example.com for support";
        let result = generalize_user_context(input);
        assert!(result.contains("[email]"));
        assert!(!result.contains("user@example.com"));
    }

    #[test]
    fn test_generalize_uuid() {
        let input = "Session ID: 550e8400-e29b-41d4-a716-446655440000";
        let result = generalize_user_context(input);
        assert!(result.contains("[uuid]"));
        assert!(!result.contains("550e8400"));
    }

    #[test]
    fn test_sanitize_pattern_full() {
        let pattern = Pattern {
            id: 1,
            pattern_hash: "original_hash".to_string(),
            tool_type: "Bash".to_string(),
            command_category: Some("cargo".to_string()),
            context_query: r#"Running cargo build in /home/user/projects/app with API_KEY="secret123456789012345""#.to_string(),
            success_count: 5,
            failure_count: 1,
            embedding_id: None,
        };

        let sanitized = sanitize_pattern(&pattern);

        // Check secrets are redacted
        assert!(!sanitized.context_query.contains("secret123456789012345"));
        // Check paths are stripped
        assert!(!sanitized.context_query.contains("/home/user"));
        // Check success/failure counts preserved
        assert_eq!(sanitized.success_count, 5);
        assert_eq!(sanitized.failure_count, 1);
        // Check hash is different
        assert_ne!(sanitized.pattern_hash, "original_hash");
    }

    #[test]
    fn test_contains_sensitive_info() {
        assert!(contains_sensitive_info("api_key = 'my_secret_key_12345678'"));
        assert!(contains_sensitive_info("Bearer eyJhbGciOiJIUzI1NiJ9.test.signature"));
        assert!(!contains_sensitive_info("just a normal comment about code"));
    }
}
