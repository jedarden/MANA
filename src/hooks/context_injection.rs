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
fn query_patterns(tool: &str, _query: &str) -> Result<ContextInjection> {
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

    // Map tool argument to database tool_type
    let tool_type = match tool {
        "edit" => "Edit",
        "bash" => "Bash",
        "task" => "Task",
        _ => tool,
    };

    // Get relevant patterns sorted by success score
    let patterns = store.get_by_tool(tool_type, MAX_PATTERNS)?;

    if patterns.is_empty() {
        // Try getting failure patterns to avoid
        let failure_patterns = store.get_by_tool("failure", 2)?;
        if failure_patterns.is_empty() {
            return Ok(ContextInjection {
                context_block: String::new(),
                patterns_used: vec![],
            });
        }

        return format_failure_patterns(&failure_patterns);
    }

    format_success_patterns(&patterns)
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
    // Try to get the most relevant part
    let lines: Vec<&str> = context_query.lines().collect();

    if lines.len() >= 2 {
        // Return the approach line if it exists
        if let Some(approach) = lines.iter().find(|l| l.starts_with("Approach:")) {
            return approach.to_string();
        }
        // Or the response approach
        if let Some(approach) = lines.iter().find(|l| l.starts_with("Response approach:")) {
            return approach.to_string();
        }
        // Or the error if it's a failure
        if let Some(error) = lines.iter().find(|l| l.starts_with("Error:")) {
            return error.to_string();
        }
    }

    // Fall back to first 100 chars
    if context_query.len() > 100 {
        format!("{}...", &context_query[..100])
    } else {
        context_query.to_string()
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
        "edit" => format!(
            "Editing {}: {} file",
            input.file_path.as_deref().unwrap_or("unknown"),
            extract_extension(input.file_path.as_deref())
        ),
        "bash" => format!(
            "Command: {}",
            input.command.as_deref()
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("")
        ),
        "task" => format!(
            "Agent: {} - {}",
            input.subagent_type.as_deref().unwrap_or("unknown"),
            input.description.as_deref().unwrap_or("")
        ),
        _ => format!("Tool: {}", tool),
    }
}

fn extract_extension(path: Option<&str>) -> &str {
    path.and_then(|p| p.rsplit('.').next())
        .unwrap_or("unknown")
}
