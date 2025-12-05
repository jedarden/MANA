//! Context injection for pre-hooks
//!
//! Reads tool input from stdin, queries ReasoningBank for relevant patterns,
//! and outputs context to stdout. Latency budget: <10ms.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use tracing::{debug, info};

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
    patterns_used: Vec<u64>,
}

/// Inject context from ReasoningBank based on tool input
///
/// Reads JSON from stdin, queries for relevant patterns, outputs context to stdout.
pub async fn inject_context(tool: &str) -> Result<()> {
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

    // TODO: Query ReasoningBank for patterns
    // For now, just pass through the input
    let context = ContextInjection {
        context_block: String::new(), // No context yet
        patterns_used: vec![],
    };

    // If we have context, inject it
    if !context.context_block.is_empty() {
        info!("Injecting {} patterns", context.patterns_used.len());
        println!("<!-- MANA Context -->");
        println!("{}", context.context_block);
        println!("<!-- /MANA Context -->");
    }

    // Pass through original input
    print!("{}", input);
    io::stdout().flush()?;

    Ok(())
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
