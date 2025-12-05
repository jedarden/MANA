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

use crate::storage::{PatternStore, Pattern, calculate_similarity};

/// Top-level hook input structure from Claude Code
#[derive(Debug, Deserialize)]
struct HookInput {
    tool_name: Option<String>,
    /// Claude Code uses "input" as the nested key
    input: Option<ToolInputFields>,
    /// Legacy: support "tool_input" for backwards compatibility
    tool_input: Option<ToolInputFields>,
    // Also support flat structure for backwards compatibility
    #[serde(flatten)]
    flat: ToolInputFields,
}

/// Fields that can appear in tool_input or at top level
#[derive(Debug, Default, Deserialize)]
struct ToolInputFields {
    file_path: Option<String>,
    command: Option<String>,
    subagent_type: Option<String>,
    description: Option<String>,
    content: Option<String>,
    prompt: Option<String>,
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

/// Minimum relevance score to include a pattern (0 = no filtering)
const MIN_RELEVANCE_SCORE: usize = 0;

/// Minimum similarity score for patterns after tech stack modifier applied
/// Set to 0.35 to effectively filter out mismatched tech stacks (0.3x penalty)
/// A pattern needs raw similarity of ~1.17 to pass with tech mismatch (impossible)
/// This ensures we don't show shell patterns for Rust queries, etc.
const MIN_TECH_STACK_SIMILARITY: f64 = 0.35;

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

    // Parse hook input
    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(hi) => hi,
        Err(e) => {
            debug!("Failed to parse hook input: {}, passing through", e);
            // Pass through original input
            print!("{}", input);
            io::stdout().flush()?;
            return Ok(());
        }
    };

    // Extract fields from nested input (Claude Code), tool_input (legacy), or flat structure
    let fields = if let Some(ref inp) = hook_input.input {
        inp
    } else if let Some(ref ti) = hook_input.tool_input {
        ti
    } else {
        &hook_input.flat
    };

    // Build query based on tool type
    let query = build_query(tool, fields);
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

    // Open pattern store in read-only mode for faster access
    let store = PatternStore::open_readonly(&db_path)?;

    // Map tool argument to database tool_types - prioritize exact matches
    let primary_types: Vec<&str> = match tool {
        "edit" => vec!["Edit", "Write", "MultiEdit"],
        "bash" => vec!["Bash"],
        "task" => vec!["Task"],
        "read" => vec!["Read", "Glob", "Grep"],
        "grep" => vec!["Grep", "Read", "Glob"],
        "web" => vec!["WebSearch", "WebFetch"],
        _ => vec![tool],
    };

    // Get relevant patterns for primary tool types only
    let mut patterns: Vec<Pattern> = Vec::new();
    for tool_type in &primary_types {
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

    // Score patterns by semantic similarity if query is not empty
    if !query.is_empty() {
        // Use TF-IDF style similarity scoring for better relevance
        let mut scored_patterns: Vec<(Pattern, f64)> = patterns
            .into_iter()
            .map(|p| {
                let similarity = calculate_similarity(query, &p.context_query);
                // Combine similarity with success score for final ranking
                let success_score = (p.success_count - p.failure_count) as f64;
                let combined_score = similarity * 0.6 + (success_score.max(0.0) / 10.0) * 0.4;
                (p, similarity, combined_score)
            })
            // Filter out patterns with very low similarity (likely tech stack mismatch)
            // This prevents showing shell patterns for Python queries, etc.
            .filter(|(_, sim, _)| *sim >= MIN_TECH_STACK_SIMILARITY)
            .map(|(p, _, score)| (p, score))
            .collect();

        // Sort by combined score (descending)
        scored_patterns.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored_patterns.truncate(MAX_PATTERNS);

        debug!("Ranked {} patterns by similarity (filtered by tech stack)", scored_patterns.len());
        patterns = scored_patterns.into_iter().map(|(p, _)| p).collect();
    } else {
        patterns.truncate(MAX_PATTERNS);
    }

    // If similarity filtering returned empty, don't fall back to potentially irrelevant patterns.
    // It's better to show no context than wrong context that could mislead the model.
    // The filtering likely removed patterns due to tech stack mismatch (e.g., shell patterns for Python query)
    if patterns.is_empty() && !query.is_empty() {
        debug!("Similarity filtering returned 0 patterns - no relevant context for this query");
        // Return empty - showing irrelevant patterns is worse than no patterns
    }

    if !patterns.is_empty() {
        return format_success_patterns(&patterns);
    }

    // No tool-specific patterns found - don't show unrelated patterns
    // (showing unrelated context is worse than no context)
    debug!("No patterns found for tool type: {}", tool);
    Ok(ContextInjection {
        context_block: String::new(),
        patterns_used: vec![],
    })
}


/// Format success patterns into context block
fn format_success_patterns(patterns: &[Pattern]) -> Result<ContextInjection> {
    let mut context_lines = Vec::new();
    let mut pattern_ids = Vec::new();
    let mut seen_insights: std::collections::HashSet<String> = std::collections::HashSet::new();

    context_lines.push("**Relevant patterns from previous successful operations:**".to_string());
    context_lines.push(String::new());

    for pattern in patterns {
        // Extract key insight from context_query
        let insight = extract_insight(&pattern.context_query);

        // Skip duplicates in output (compare full insight, lowercased)
        let normalized = insight.to_lowercase();
        if seen_insights.contains(&normalized) {
            continue;
        }
        seen_insights.insert(normalized);

        let score = pattern.success_count - pattern.failure_count;
        let confidence = if pattern.success_count + pattern.failure_count > 0 {
            (pattern.success_count as f64 / (pattern.success_count + pattern.failure_count) as f64) * 100.0
        } else {
            50.0
        };

        context_lines.push(format!("- **{}** (score: {}, {:.0}% success rate)",
            pattern.tool_type, score, confidence));
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

/// Extract a concise, actionable insight from the context query
fn extract_insight(context_query: &str) -> String {
    let lines: Vec<&str> = context_query.lines().collect();

    // Extract components
    let mut approach_detail = String::new();
    let mut pitfall_msg = String::new();

    for line in &lines {
        let trimmed = line.trim();

        // Extract Pitfall content (key for failures)
        if trimmed.starts_with("Pitfall:") {
            let content = trimmed.trim_start_matches("Pitfall:").trim();
            if !content.is_empty() && content.len() > 10 {
                pitfall_msg = format!("⚠️ {}", truncate_str(content, 100));
            }
        }
        // Extract Approach content - the specific action taken
        else if trimmed.starts_with("Approach:") {
            let content = trimmed.trim_start_matches("Approach:").trim();
            if !content.is_empty() && content.len() > 5 {
                // Format the approach as an actionable hint
                approach_detail = format_approach_hint(content);
            }
        }
    }

    // Prioritize: pitfalls first (warnings), then approaches (successful patterns)
    if !pitfall_msg.is_empty() {
        return pitfall_msg;
    }

    if !approach_detail.is_empty() {
        return approach_detail;
    }

    // Fallback: use first non-empty meaningful line
    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with("Task:")
            && !trimmed.starts_with("Outcome:")
            && !trimmed.starts_with("User asked:")
            && !trimmed.starts_with("Query:")
            && !trimmed.starts_with("Approach:")
            && trimmed.len() > 10
        {
            return truncate_str(trimmed, 100).to_string();
        }
    }

    // Last resort
    truncate_str(context_query.lines().next().unwrap_or(context_query), 80).to_string()
}

/// Format approach string into an actionable hint
/// Input: "Bash - npm - Initialize project with package.json"
/// Output: "Ran `npm`: Initialize project with package.json"
fn format_approach_hint(approach: &str) -> String {
    // Pattern: "ToolName - command - description"
    let parts: Vec<&str> = approach.splitn(3, " - ").collect();

    match parts.as_slice() {
        [tool, cmd, desc] => {
            // Format based on tool type
            match *tool {
                "Bash" => format!("Ran `{}`: {}", cmd, truncate_str(desc, 60)),
                "Edit" | "MultiEdit" => format!("{}", truncate_str(desc, 80)),
                "Write" => format!("{}", truncate_str(desc, 80)),
                "Read" | "Glob" | "Grep" => format!("{}", truncate_str(desc, 80)),
                "Task" => {
                    // Handle "delegating to agent - description" format
                    // cmd is like "delegating to researcher", extract agent name
                    let agent = cmd.trim_start_matches("delegating to ").trim();
                    format!("Delegated to {}: {}", agent, truncate_str(desc, 50))
                }
                _ => format!("{}: {}", tool, truncate_str(desc, 70)),
            }
        }
        [tool, desc] => {
            format!("{}", truncate_str(desc, 80))
        }
        _ => truncate_str(approach, 80).to_string(),
    }
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

fn build_query(tool: &str, input: &ToolInputFields) -> String {
    match tool {
        "edit" => {
            let path = input.file_path.as_deref().unwrap_or("unknown");
            let ext = extract_extension(Some(path));
            let filename = extract_filename(path);
            // Include multiple tech signals for robust stack detection
            let (lang_hint, extra_signals) = match ext {
                "rs" => ("rust", "cargo toml crate"),
                "ts" | "tsx" => ("typescript", "npm node package"),
                "js" | "jsx" => ("javascript", "npm node package"),
                "py" => ("python", "pip pytest"),
                "go" => ("golang", "mod"),
                "rb" => ("ruby", "gem"),
                "java" => ("java", "maven gradle"),
                "cpp" | "cc" | "cxx" => ("cpp", "cmake"),
                "c" | "h" => ("c", "cmake"),
                "md" => ("markdown", ""),
                "json" => ("json", "npm node package"),  // JSON often indicates Node.js
                "yaml" | "yml" => ("yaml", ""),
                "toml" => ("toml", "cargo rust"),  // TOML strongly indicates Rust
                "sh" | "bash" => ("shell", "bash"),
                _ => (ext, ""),
            };
            format!("Editing {} {} {} file {}", ext, lang_hint, extra_signals, filename)
        }
        "bash" => {
            let cmd = input.command.as_deref().unwrap_or("");
            let first_word = cmd.split_whitespace().next().unwrap_or("");
            let desc = input.description.as_deref().unwrap_or("");
            // Include "Bash" keyword for better pattern matching with stored patterns
            // Also include tech stack hints based on the command
            let tech_hint = match first_word {
                "npm" | "npx" | "yarn" | "pnpm" | "node" | "deno" | "bun" => "javascript npm node",
                "cargo" | "rustc" | "rustup" => "rust cargo",
                "pip" | "python" | "python3" | "pytest" | "poetry" | "uv" => "python pip",
                "go" => "golang go",
                "git" | "gh" => "git",
                "docker" | "docker-compose" => "docker container",
                "make" | "cmake" => "build make",
                _ => "",
            };
            if !desc.is_empty() {
                format!("Bash {} {} {}", first_word, tech_hint, desc)
            } else {
                format!("Bash {} {}", first_word, tech_hint)
            }
        }
        "task" => format!(
            "Agent: {} - {}",
            input.subagent_type.as_deref().unwrap_or("unknown"),
            input.description.as_deref().unwrap_or("")
        ),
        "read" => {
            let path = input.file_path.as_deref().unwrap_or("");
            let filename = extract_filename(path);
            format!("Reading {}", filename)
        },
        "web" => {
            // For web tools, build query from the input
            format!("Web search")
        },
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
