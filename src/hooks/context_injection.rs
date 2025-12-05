//! Context injection for pre-hooks
//!
//! Reads tool input from stdin, queries ReasoningBank for relevant patterns,
//! and outputs context to stdout. Latency budget: <10ms.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::storage::{PatternStore, Pattern};

#[derive(Debug, Deserialize)]
struct ToolInput {
    tool_name: Option<String>,
    file_path: Option<String>,
    command: Option<String>,
    subagent_type: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct ContextInjection {
    context_block: String,
    patterns_used: Vec<i64>,
}

/// Maximum patterns to inject per context (to avoid overwhelming Claude)
const MAX_PATTERNS: usize = 3;

/// Maximum time budget for injection in milliseconds
const INJECTION_TIMEOUT_MS: u128 = 10;

/// Minimum relevance score to include a pattern
const MIN_RELEVANCE_SCORE: usize = 1;

/// Inject context from ReasoningBank based on tool input
///
/// Reads JSON from stdin, queries for relevant patterns, outputs context to stdout.
pub async fn inject_context(tool: &str) -> Result<()> {
    let start = Instant::now();
    debug!("Injecting context for tool: {}", tool);

    // Read input from stdin
    let stdin = io::stdin();
    let input: String = stdin.lock().lines()
        .filter_map(|line| line.ok())
        .collect::<Vec<_>>()
        .join("\n");

    if input.is_empty() {
        debug!("No input received, skipping context injection");
        return Ok(());
    }

    // Parse tool input
    let tool_input: ToolInput = match serde_json::from_str(&input) {
        Ok(ti) => ti,
        Err(e) => {
            debug!("Failed to parse tool input: {}, passing through", e);
            // Pass through original input
            print!("{}", input);
            io::stdout().flush()?;
            return Ok(());
        }
    };

    // Build query based on tool type
    let query = build_query(tool, &tool_input);
    debug!("Query: {}", query);

    // Query ReasoningBank for patterns
    let context = match query_patterns(tool, &query) {
        Ok(ctx) => ctx,
        Err(e) => {
            warn!("Failed to query patterns: {}, passing through", e);
            ContextInjection {
                context_block: String::new(),
                patterns_used: vec![],
            }
        }
    };

    // Check time budget
    let elapsed = start.elapsed().as_millis();
    if elapsed > INJECTION_TIMEOUT_MS {
        warn!("Context injection exceeded time budget: {}ms > {}ms", elapsed, INJECTION_TIMEOUT_MS);
    }

    // If we have context, inject it as a system-reminder style block
    if !context.context_block.is_empty() {
        info!("Injecting {} patterns in {}ms", context.patterns_used.len(), elapsed);
        println!("<mana-context>");
        println!("{}", context.context_block);
        println!("</mana-context>");
        println!();
    }

    // Pass through original input
    print!("{}", input);
    io::stdout().flush()?;

    debug!("Context injection complete in {}ms", start.elapsed().as_millis());
    Ok(())
}

/// Query patterns from the ReasoningBank
fn query_patterns(tool: &str, query: &str) -> Result<ContextInjection> {
    // Get MANA data directory
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        debug!("No database found, skipping pattern query");
        return Ok(ContextInjection {
            context_block: String::new(),
            patterns_used: vec![],
        });
    }

    // Open pattern store
    let store = PatternStore::open(&db_path)?;

    // Map tool argument to database tool_types (may need multiple)
    let tool_types: Vec<&str> = match tool {
        "edit" => vec!["Edit", "Write", "MultiEdit"],  // Edit hooks may have Write patterns
        "bash" => vec!["Bash"],
        "task" => vec!["Task"],
        _ => vec![tool],
    };

    // Get relevant patterns for these tool types
    let mut patterns: Vec<Pattern> = Vec::new();
    for tool_type in &tool_types {
        let mut type_patterns = store.get_by_tool(tool_type, MAX_PATTERNS * 2)?;
        patterns.append(&mut type_patterns);
    }
    // Sort by score and truncate
    patterns.sort_by(|a, b| {
        let a_score = a.success_count - a.failure_count;
        let b_score = b.success_count - b.failure_count;
        b_score.cmp(&a_score)
    });
    patterns.truncate(MAX_PATTERNS * 2);

    // Deduplicate patterns by extracting unique context insights
    let mut seen_insights: std::collections::HashSet<String> = std::collections::HashSet::new();
    patterns.retain(|p| {
        let insight = extract_insight(&p.context_query);
        // Normalize insight for comparison (first 50 chars, lowercased)
        let normalized = insight.to_lowercase().chars().take(50).collect::<String>();
        if seen_insights.contains(&normalized) {
            false
        } else {
            seen_insights.insert(normalized);
            true
        }
    });

    // Score patterns by query relevance if query is not empty
    if !query.is_empty() {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        // Score each pattern by word overlap and filter low-relevance
        let mut scored_patterns: Vec<(Pattern, usize)> = patterns
            .into_iter()
            .map(|p| {
                let score = score_pattern_relevance(&p.context_query, &query_words);
                (p, score)
            })
            .filter(|(_, score)| *score >= MIN_RELEVANCE_SCORE)
            .collect();

        // Sort by relevance score (descending)
        scored_patterns.sort_by(|a, b| b.1.cmp(&a.1));
        scored_patterns.truncate(MAX_PATTERNS);

        patterns = scored_patterns.into_iter().map(|(p, _)| p).collect();
    } else {
        patterns.truncate(MAX_PATTERNS);
    }

    if !patterns.is_empty() {
        return format_success_patterns(&patterns);
    }

    // If no tool-specific patterns, don't show failure patterns
    // (they're too generic to be useful)
    Ok(ContextInjection {
        context_block: String::new(),
        patterns_used: vec![],
    })
}

/// Score pattern relevance based on word overlap with query
fn score_pattern_relevance(context_query: &str, query_words: &[&str]) -> usize {
    let context_lower = context_query.to_lowercase();
    let mut score = 0;

    for word in query_words {
        if word.len() < 2 {
            continue;
        }

        // Exact word match (higher weight)
        if word.len() >= 3 && context_lower.contains(word) {
            score += 2;
        }

        // File extension match (highest weight for file operations)
        if word.len() <= 4 && is_file_extension(word) {
            // Extensions like .rs, .js, .py get high priority
            if context_lower.contains(&format!(".{}", word))
                || context_lower.contains(&format!(" {} ", word))
                || context_lower.contains(&format!(" {}\n", word)) {
                score += 5;
            }
        }

        // Language name match (high weight)
        let lang_keywords = ["rust", "typescript", "javascript", "python", "golang", "shell"];
        for lang in lang_keywords {
            if word.contains(lang) && context_lower.contains(lang) {
                score += 3;
            }
        }
    }

    score
}

/// Check if a word looks like a file extension
fn is_file_extension(word: &str) -> bool {
    matches!(word, "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "rb" |
             "java" | "cpp" | "c" | "h" | "md" | "json" | "yaml" | "yml" |
             "toml" | "sh" | "bash" | "html" | "css" | "sql")
}

/// Format success patterns into context block
fn format_success_patterns(patterns: &[Pattern]) -> Result<ContextInjection> {
    let mut context_lines = Vec::new();
    let mut pattern_ids = Vec::new();

    context_lines.push("**Relevant patterns from previous successful operations:**".to_string());
    context_lines.push(String::new());

    for pattern in patterns {
        let score = pattern.success_count - pattern.failure_count;
        let confidence = if pattern.success_count + pattern.failure_count > 0 {
            (pattern.success_count as f64 / (pattern.success_count + pattern.failure_count) as f64) * 100.0
        } else {
            50.0
        };

        context_lines.push(format!("- **{}** (score: {}, {:.0}% success rate)",
            pattern.tool_type, score, confidence));

        // Extract key insight from context_query
        let insight = extract_insight(&pattern.context_query);
        context_lines.push(format!("  {}", insight));
        context_lines.push(String::new());

        pattern_ids.push(pattern.id);
    }

    Ok(ContextInjection {
        context_block: context_lines.join("\n"),
        patterns_used: pattern_ids,
    })
}

/// Format failure patterns into warnings
fn format_failure_patterns(patterns: &[Pattern]) -> Result<ContextInjection> {
    let mut context_lines = Vec::new();
    let mut pattern_ids = Vec::new();

    context_lines.push("**Common pitfalls to avoid:**".to_string());
    context_lines.push(String::new());

    for pattern in patterns {
        // Extract the error insight
        let insight = extract_insight(&pattern.context_query);
        context_lines.push(format!("- ⚠️ {}", insight));
        pattern_ids.push(pattern.id);
    }

    Ok(ContextInjection {
        context_block: context_lines.join("\n"),
        patterns_used: pattern_ids,
    })
}

/// Extract a concise insight from the context query
fn extract_insight(context_query: &str) -> String {
    let lines: Vec<&str> = context_query.lines().collect();
    let mut insights = Vec::new();

    for line in &lines {
        let trimmed = line.trim();

        // Extract Pitfall content (key for failures)
        if trimmed.starts_with("Pitfall:") {
            let content = trimmed.trim_start_matches("Pitfall:").trim();
            if !content.is_empty() && content.len() > 5 {
                insights.push(format!("Watch out: {}", truncate_str(content, 80)));
            }
        }
        // Extract Approach content
        else if trimmed.starts_with("Approach:") {
            let content = trimmed.trim_start_matches("Approach:").trim();
            if !content.is_empty() && content.len() > 5 {
                insights.push(truncate_str(content, 100).to_string());
            }
        }
        // Extract tool context (e.g., "Edit - editing file.rs")
        else if trimmed.contains(" - ") && !trimmed.starts_with("Task:") {
            let parts: Vec<&str> = trimmed.splitn(2, " - ").collect();
            if parts.len() == 2 {
                insights.push(truncate_str(parts[1], 80).to_string());
            }
        }
    }

    if !insights.is_empty() {
        return insights.join(" | ");
    }

    // Fall back: get the second line if available (skip Task: line)
    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with("Task:")
            && !trimmed.starts_with("Outcome:")
            && !trimmed.starts_with("User asked:")
            && !trimmed.starts_with("Query:")
            && trimmed.len() > 5
        {
            return truncate_str(trimmed, 100).to_string();
        }
    }

    // Last resort
    truncate_str(context_query.lines().next().unwrap_or(context_query), 80).to_string()
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
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

fn build_query(tool: &str, input: &ToolInput) -> String {
    match tool {
        "edit" => {
            let path = input.file_path.as_deref().unwrap_or("unknown");
            let ext = extract_extension(Some(path));
            let filename = extract_filename(path);
            // Include extension, filename, and language hints for better matching
            let lang_hint = match ext {
                "rs" => "rust",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "py" => "python",
                "go" => "golang",
                "rb" => "ruby",
                "java" => "java",
                "cpp" | "cc" | "cxx" => "cpp",
                "c" | "h" => "c",
                "md" => "markdown",
                "json" => "json",
                "yaml" | "yml" => "yaml",
                "toml" => "toml",
                "sh" | "bash" => "shell script",
                _ => ext,
            };
            format!("Editing {} {} file {}", ext, lang_hint, filename)
        }
        "bash" => {
            let cmd = input.command.as_deref().unwrap_or("");
            let first_word = cmd.split_whitespace().next().unwrap_or("");
            let desc = input.description.as_deref().unwrap_or("");
            if !desc.is_empty() {
                format!("Command {} {}", first_word, desc)
            } else {
                format!("Command {}", first_word)
            }
        }
        "task" => format!(
            "Agent: {} - {}",
            input.subagent_type.as_deref().unwrap_or("unknown"),
            input.description.as_deref().unwrap_or("")
        ),
        _ => format!("Tool: {}", tool),
    }
}

fn extract_filename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn extract_extension(path: Option<&str>) -> &str {
    path.and_then(|p| p.rsplit('.').next())
        .unwrap_or("unknown")
}
