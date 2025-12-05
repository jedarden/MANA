//! Context injection for pre-hooks
//!
//! Reads tool input from stdin, queries ReasoningBank for relevant patterns,
//! and outputs context to stdout. Latency budget: <10ms.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, Read as IoRead, Write};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, warn};

use crate::storage::{PatternStore, Pattern, calculate_similarity, CausalStore};

/// Top-level hook input structure from Claude Code
#[derive(Debug, Deserialize)]
struct HookInput {
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    content: Option<String>,
    #[allow(dead_code)]
    prompt: Option<String>,
}

#[derive(Debug, Serialize)]
struct ContextInjection {
    context_block: String,
    patterns_used: Vec<i64>,
}

/// Maximum patterns to inject per context (to avoid overwhelming Claude)
const MAX_PATTERNS: usize = 3;

/// Number of patterns to retrieve for similarity scoring (before filtering)
/// Balanced at 8 - enough for quality matches without excess overhead
const PATTERNS_TO_SCORE: usize = 8;

/// Maximum time budget for injection in milliseconds
const INJECTION_TIMEOUT_MS: u128 = 10;

/// Minimum relevance score to include a pattern (currently unused but reserved for future)
#[allow(dead_code)]
const MIN_RELEVANCE_SCORE: usize = 0;

/// Minimum similarity score for patterns after tech stack modifier applied
/// Set to 0.35 to effectively filter out mismatched tech stacks (0.3x penalty)
/// A pattern needs raw similarity of ~1.17 to pass with tech mismatch (impossible)
/// This ensures we don't show shell patterns for Rust queries, etc.
const MIN_TECH_STACK_SIMILARITY: f64 = 0.35;

/// Inject context from ReasoningBank based on tool input
///
/// Reads JSON from stdin, queries for relevant patterns, outputs context to stdout.
/// This is intentionally synchronous to avoid tokio runtime initialization overhead.
pub fn inject_context(tool: &str) -> Result<()> {
    let start = Instant::now();
    debug!("Injecting context for tool: {}", tool);

    // Read input from stdin - read all bytes at once for speed
    // Typical input is <1KB, so reading everything is fast
    let stdin = io::stdin();
    let mut input = String::with_capacity(1024);
    stdin.lock().read_to_string(&mut input)?;
    let stdin_time = start.elapsed().as_micros();

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
    let parse_time = start.elapsed().as_micros() - stdin_time;

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
    let query_start = Instant::now();
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
    let query_time = query_start.elapsed().as_micros();

    // Check time budget
    let elapsed = start.elapsed().as_millis();
    if elapsed > INJECTION_TIMEOUT_MS {
        warn!("Context injection exceeded time budget: {}ms > {}ms (stdin: {}µs, parse: {}µs, query: {}µs)",
              elapsed, INJECTION_TIMEOUT_MS, stdin_time, parse_time, query_time);
    }

    // If we have context, inject it as a system-reminder style block
    if !context.context_block.is_empty() {
        debug!("Injecting {} patterns in {}ms (stdin: {}µs, parse: {}µs, query: {}µs)",
               context.patterns_used.len(), elapsed, stdin_time, parse_time, query_time);
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
    let _query_start = Instant::now();

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
    let db_open_start = Instant::now();
    let store = PatternStore::open_readonly(&db_path)?;
    let db_open_time = db_open_start.elapsed().as_micros();
    debug!("DB open: {}µs", db_open_time);

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
    // Retrieve more patterns than we need so similarity scoring can find the best matches
    let mut patterns: Vec<Pattern> = Vec::new();
    for tool_type in &primary_types {
        let mut type_patterns = store.get_by_tool(tool_type, PATTERNS_TO_SCORE)?;
        patterns.append(&mut type_patterns);
    }

    // Patterns are already sorted by score from DB query
    // Skip heavy deduplication - similarity scoring handles relevance
    // Just do a quick truncate to limit work
    patterns.truncate(PATTERNS_TO_SCORE * 2);

    // Score patterns by semantic similarity if query is not empty
    if !query.is_empty() {
        debug!("Scoring {} patterns for query: {}", patterns.len(), query);

        // Use TF-IDF style similarity scoring for better relevance
        // Process patterns in batches for better cache locality
        let mut scored_patterns: Vec<(Pattern, f64)> = patterns
            .into_iter()
            .filter_map(|p| {
                let similarity = calculate_similarity(query, &p.context_query);

                // Early filter: skip patterns below threshold
                if similarity < MIN_TECH_STACK_SIMILARITY {
                    return None;
                }

                // Combine similarity with success score for final ranking
                let success_score = (p.success_count - p.failure_count) as f64;
                let combined_score = similarity * 0.6 + (success_score.max(0.0) / 10.0) * 0.4;
                debug!("  Pattern [{}]: sim={:.3}, combined={:.3}, context: {}",
                    p.tool_type, similarity, combined_score,
                    p.context_query.chars().take(60).collect::<String>());
                Some((p, combined_score))
            })
            .collect();

        // Sort by combined score (descending)
        scored_patterns.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Only filter causal conflicts if we have more candidates than needed
        // This avoids extra DB I/O in the common case
        if scored_patterns.len() > MAX_PATTERNS {
            scored_patterns = filter_causal_conflicts(&db_path, scored_patterns);
        }

        scored_patterns.truncate(MAX_PATTERNS);

        debug!("Ranked {} patterns by similarity (filtered by tech stack + causal)", scored_patterns.len());
        patterns = scored_patterns.into_iter().map(|(p, _)| p).collect();
    } else {
        patterns.truncate(MAX_PATTERNS);
    }

    // If similarity filtering returned empty due to tech stack mismatch,
    // try to provide generic helpful patterns with a caveat
    if patterns.is_empty() && !query.is_empty() {
        debug!("Similarity filtering returned 0 patterns - trying generic fallback");

        // Get top patterns without tech stack filtering for generic guidance
        // These are high-quality patterns that might still be helpful
        let fallback_patterns: Vec<Pattern> = store.get_top_patterns(PATTERNS_TO_SCORE)?
            .into_iter()
            .filter(|p| primary_types.iter().any(|t| p.tool_type.eq_ignore_ascii_case(t)))
            .take(MAX_PATTERNS)
            .collect();

        if !fallback_patterns.is_empty() {
            debug!("Using {} generic fallback patterns", fallback_patterns.len());
            return format_generic_patterns(&fallback_patterns);
        }
    }

    if !patterns.is_empty() {
        return format_success_patterns(&patterns);
    }

    // No patterns found at all
    debug!("No patterns found for tool type: {}", tool);
    Ok(ContextInjection {
        context_block: String::new(),
        patterns_used: vec![],
    })
}


/// Filter out patterns that conflict with top-ranked patterns
/// This uses causal edges to prevent recommending incompatible patterns together
/// OPTIMIZATION: Skip causal filtering for small result sets to reduce latency
fn filter_causal_conflicts(db_path: &std::path::Path, mut patterns: Vec<(Pattern, f64)>) -> Vec<(Pattern, f64)> {
    // Skip causal filtering entirely if we have few patterns
    // The overhead of opening another DB connection isn't worth it for small sets
    if patterns.len() <= MAX_PATTERNS + 1 {
        return patterns;
    }

    // Try to open causal store, skip filtering if it fails
    let causal_store = match CausalStore::open_readonly(db_path) {
        Ok(store) => store,
        Err(_) => return patterns, // No causal data available
    };

    // Get conflicts for the top pattern
    let top_pattern_id = patterns[0].0.id;
    let conflicts = match causal_store.get_conflicts(top_pattern_id) {
        Ok(c) => c,
        Err(_) => return patterns,
    };

    if conflicts.is_empty() {
        return patterns;
    }

    // Filter out conflicting patterns
    let original_len = patterns.len();
    patterns.retain(|(p, _)| !conflicts.contains(&p.id));

    let filtered = original_len - patterns.len();
    if filtered > 0 {
        debug!("Filtered {} conflicting patterns based on causal edges", filtered);
    }

    patterns
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

/// Format generic patterns as fallback (when no tech-specific match)
fn format_generic_patterns(patterns: &[Pattern]) -> Result<ContextInjection> {
    let mut context_lines = Vec::new();
    let mut pattern_ids = Vec::new();
    let mut seen_insights: std::collections::HashSet<String> = std::collections::HashSet::new();

    context_lines.push("**General patterns (no tech-specific matches found):**".to_string());
    context_lines.push(String::new());

    for pattern in patterns {
        let insight = extract_insight(&pattern.context_query);

        // Skip duplicates
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

/// Format failure patterns into warnings (reserved for future use)
#[allow(dead_code)]
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
                "Edit" | "MultiEdit" => truncate_str(desc, 80).to_string(),
                "Write" => truncate_str(desc, 80).to_string(),
                "Read" | "Glob" | "Grep" => truncate_str(desc, 80).to_string(),
                "Task" => {
                    // Handle "delegating to agent - description" format
                    // cmd is like "delegating to researcher", extract agent name
                    let agent = cmd.trim_start_matches("delegating to ").trim();
                    format!("Delegated to {}: {}", agent, truncate_str(desc, 50))
                }
                _ => format!("{}: {}", tool, truncate_str(desc, 70)),
            }
        }
        [_tool, desc] => {
            truncate_str(desc, 80).to_string()
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

/// Get MANA data directory with caching for performance
/// Uses a static cache to avoid repeated filesystem checks
fn get_mana_dir() -> Result<PathBuf> {
    use std::sync::OnceLock;
    static MANA_DIR: OnceLock<PathBuf> = OnceLock::new();

    Ok(MANA_DIR.get_or_init(|| {
        // Check project-local .mana first
        if let Ok(cwd) = std::env::current_dir() {
            let project_mana = cwd.join(".mana");
            if project_mana.exists() {
                return project_mana;
            }
        }

        // Fall back to home directory
        dirs::home_dir()
            .map(|h| h.join(".mana"))
            .unwrap_or_else(|| PathBuf::from(".mana"))
    }).clone())
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
            "Web search".to_string()
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
