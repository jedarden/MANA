//! Storage module for MANA
//!
//! Handles pattern storage in SQLite (metadata) and provides
//! status/statistics reporting.

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tracing::info;

pub mod patterns;
pub mod similarity;
pub mod causal;
pub mod skills;

pub use patterns::{PatternStore, Pattern};
pub use similarity::calculate_similarity;
pub use causal::CausalStore;
#[allow(unused_imports)]
pub use causal::CausalEdge;
pub use skills::{SkillStore, Skill, consolidate_patterns_to_skills};

/// Initialize MANA storage and configuration
pub async fn init() -> Result<()> {
    let mana_dir = get_mana_dir()?;
    std::fs::create_dir_all(&mana_dir)?;

    // Initialize SQLite database
    let db_path = mana_dir.join("metadata.sqlite");
    let conn = Connection::open(&db_path)?;

    // Create tables
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS patterns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            pattern_hash TEXT UNIQUE NOT NULL,
            tool_type TEXT NOT NULL,
            command_category TEXT,
            context_query TEXT NOT NULL,
            success_count INTEGER DEFAULT 0,
            failure_count INTEGER DEFAULT 0,
            last_used DATETIME,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            embedding_id INTEGER
        );

        CREATE TABLE IF NOT EXISTS skills (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            description TEXT,
            pattern_ids TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS learning_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            event_type TEXT NOT NULL,
            details TEXT
        );

        -- Causal edges track pattern relationships discovered during learning
        -- positive lift (>1.5) = synergy (patterns work well together)
        -- negative lift (<0.5) = conflict (patterns interfere with each other)
        CREATE TABLE IF NOT EXISTS causal_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            pattern_a_id INTEGER NOT NULL,
            pattern_b_id INTEGER NOT NULL,
            lift REAL NOT NULL,
            co_occurrences INTEGER DEFAULT 1,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(pattern_a_id, pattern_b_id),
            FOREIGN KEY (pattern_a_id) REFERENCES patterns(id) ON DELETE CASCADE,
            FOREIGN KEY (pattern_b_id) REFERENCES patterns(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_patterns_tool ON patterns(tool_type);
        CREATE INDEX IF NOT EXISTS idx_patterns_hash ON patterns(pattern_hash);
        CREATE INDEX IF NOT EXISTS idx_causal_pattern_a ON causal_edges(pattern_a_id);
        CREATE INDEX IF NOT EXISTS idx_causal_pattern_b ON causal_edges(pattern_b_id);
        "#,
    )?;

    // Migration: add command_category column if it doesn't exist (for existing databases)
    let has_category: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('patterns') WHERE name = 'command_category'",
        [],
        |row| Ok(row.get::<_, i64>(0)? > 0),
    ).unwrap_or(false);

    if !has_category {
        conn.execute("ALTER TABLE patterns ADD COLUMN command_category TEXT", [])?;
        info!("Migrated patterns table to add command_category column");
    }

    // Always ensure the category index exists
    conn.execute("CREATE INDEX IF NOT EXISTS idx_patterns_category ON patterns(command_category)", [])?;

    info!("MANA initialized at {:?}", mana_dir);

    // Create default config if not exists
    let config_path = mana_dir.join("config.toml");
    if !config_path.exists() {
        let default_config = r#"# MANA Configuration

[learning]
# Trajectory threshold before triggering learning
threshold = 15
# Maximum patterns to inject per context
max_patterns_per_context = 5

[performance]
# Maximum time for context injection in milliseconds
injection_timeout_ms = 10
# Maximum time for pattern search in milliseconds
search_timeout_ms = 5

[storage]
# Maximum number of patterns to keep
max_patterns = 10000
# Decay factor for unused patterns (0-1)
decay_factor = 0.95
"#;
        std::fs::write(&config_path, default_config)?;
        info!("Created default configuration at {:?}", config_path);
    }

    Ok(())
}

/// Show current MANA status
pub async fn show_status() -> Result<()> {
    let mana_dir = get_mana_dir()?;

    println!("MANA Status");
    println!("============");
    println!();

    // Check if initialized
    if !mana_dir.exists() {
        println!("Status: NOT INITIALIZED");
        println!("Run 'mana init' to initialize MANA");
        return Ok(());
    }

    println!("Status: INITIALIZED");
    println!("Data directory: {:?}", mana_dir);

    // Check database
    let db_path = mana_dir.join("metadata.sqlite");
    if db_path.exists() {
        let conn = Connection::open(&db_path)?;
        let pattern_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        let causal_edge_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM causal_edges",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        println!("Patterns stored: {}", pattern_count);
        println!("Causal edges: {}", causal_edge_count);
    } else {
        println!("Database: NOT FOUND");
    }

    // Check learning state
    let state_path = mana_dir.join("learning-state.json");
    if state_path.exists() {
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&state_path)?
        )?;

        if let Some(count) = state.get("trajectory_count").and_then(|v| v.as_u64()) {
            println!("Pending trajectories: {}", count);
        }
    }

    Ok(())
}

/// Show detailed MANA statistics
pub async fn show_stats() -> Result<()> {
    let mana_dir = get_mana_dir()?;

    println!("MANA Statistics");
    println!("================");
    println!();

    if !mana_dir.exists() {
        println!("MANA not initialized. Run 'mana init' first.");
        return Ok(());
    }

    let db_path = mana_dir.join("metadata.sqlite");
    if !db_path.exists() {
        println!("No database found.");
        return Ok(());
    }

    let conn = Connection::open(&db_path)?;

    // Pattern statistics
    println!("Pattern Statistics:");
    println!("-------------------");

    let total_patterns: i64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    println!("  Total patterns: {}", total_patterns);

    // Patterns by tool type
    let mut stmt = conn.prepare(
        "SELECT tool_type, COUNT(*) FROM patterns GROUP BY tool_type ORDER BY COUNT(*) DESC"
    )?;
    let tool_counts = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    println!("  By tool type:");
    for (tool, count) in tool_counts.flatten() {
        println!("    {}: {}", tool, count);
    }

    // Success rate
    let (total_success, total_failure): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(success_count), 0), COALESCE(SUM(failure_count), 0) FROM patterns",
        [],
        |row| Ok((row.get(0)?, row.get(1)?))
    ).unwrap_or((0, 0));

    let total_uses = total_success + total_failure;
    if total_uses > 0 {
        let success_rate = (total_success as f64 / total_uses as f64) * 100.0;
        println!("  Success rate: {:.1}% ({}/{} uses)", success_rate, total_success, total_uses);
    } else {
        println!("  Success rate: N/A (no uses recorded)");
    }

    // Learning log
    println!();
    println!("Learning History:");
    println!("-----------------");

    let mut stmt = conn.prepare(
        "SELECT timestamp, event_type FROM learning_log ORDER BY timestamp DESC LIMIT 5"
    )?;
    let events = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut has_events = false;
    for result in events {
        has_events = true;
        if let Ok((timestamp, event_type)) = result {
            println!("  {} - {}", timestamp, event_type);
        }
    }

    if !has_events {
        println!("  No learning events recorded yet.");
    }

    // Causal Graph Statistics
    println!();
    println!("Causal Graph:");
    println!("-------------");

    let total_edges: i64 = conn.query_row(
        "SELECT COUNT(*) FROM causal_edges",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    println!("  Total edges: {}", total_edges);

    if total_edges > 0 {
        // Count synergies (lift > 1.5 with >= 3 co-occurrences)
        let synergies: i64 = conn.query_row(
            "SELECT COUNT(*) FROM causal_edges WHERE lift > 1.5 AND co_occurrences >= 3",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        // Count conflicts (lift < 0.5 with >= 3 co-occurrences)
        let conflicts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM causal_edges WHERE lift < 0.5 AND co_occurrences >= 3",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        // Count uncertain (0.5 <= lift <= 1.5 or < 3 co-occurrences)
        let uncertain = total_edges - synergies - conflicts;

        println!("  Synergies (lift > 1.5): {}", synergies);
        println!("  Conflicts (lift < 0.5): {}", conflicts);
        println!("  Uncertain/learning: {}", uncertain);

        // Average co-occurrences
        let avg_cooccur: f64 = conn.query_row(
            "SELECT AVG(co_occurrences) FROM causal_edges",
            [],
            |row| row.get(0)
        ).unwrap_or(0.0);
        println!("  Avg co-occurrences: {:.1}", avg_cooccur);
    }

    // Skill Statistics
    println!();
    println!("Skills:");
    println!("-------");

    // Check if skills table exists and has data
    let skill_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM skills",
        [],
        |row| row.get(0)
    ).unwrap_or(0);

    if skill_count > 0 {
        println!("  Total skills: {}", skill_count);

        // Skills by tool type
        let mut stmt = conn.prepare(
            "SELECT tool_type, COUNT(*), SUM(pattern_count) FROM skills GROUP BY tool_type ORDER BY COUNT(*) DESC"
        )?;
        let skill_stats = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })?;

        println!("  By tool type:");
        for result in skill_stats {
            if let Ok((tool, count, patterns)) = result {
                println!("    {}: {} skills ({} patterns)", tool, count, patterns);
            }
        }

        // Average success rate
        let avg_success_rate: f64 = conn.query_row(
            "SELECT AVG(CAST(total_success AS REAL) / NULLIF(total_success + total_failure, 0) * 100) FROM skills",
            [],
            |row| row.get(0)
        ).unwrap_or(0.0);
        println!("  Avg skill success rate: {:.1}%", avg_success_rate);
    } else {
        println!("  No skills created yet. Run 'mana consolidate' to create skills.");
    }

    Ok(())
}

fn get_mana_dir() -> Result<PathBuf> {
    // Check for .mana directory in current project first
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    // Fall back to home directory
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

/// Debug: show sample patterns for inspection
pub async fn debug_patterns(limit: usize) -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        println!("No database found.");
        return Ok(());
    }

    let conn = Connection::open(&db_path)?;

    println!("Sample Patterns (showing {} by type):", limit);
    println!("{}", "=".repeat(60));

    // Show patterns by type
    for tool_type in &["failure", "Bash", "Edit", "Write", "Task"] {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, tool_type, context_query, success_count, failure_count
            FROM patterns
            WHERE tool_type = ?1
            ORDER BY (success_count - failure_count) DESC
            LIMIT ?2
            "#,
        )?;

        let patterns = stmt.query_map(params![tool_type, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        let patterns: Vec<_> = patterns.filter_map(|r| r.ok()).collect();
        if patterns.is_empty() {
            continue;
        }

        println!("\n[{}] ({} patterns):", tool_type, patterns.len());
        println!("{}", "-".repeat(40));

        for (id, _tool, context, success, failure) in patterns {
            let score = success - failure;
            // Show first 2 lines of context
            let preview: String = context
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" | ");
            let preview = if preview.len() > 100 {
                format!("{}...", &preview[..100])
            } else {
                preview
            };
            println!("  #{} [score: {}] {}", id, score, preview);
        }
    }

    Ok(())
}

/// Prune low-quality patterns
pub async fn prune_patterns(min_score: i64, dry_run: bool) -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        println!("No database found.");
        return Ok(());
    }

    let store = PatternStore::open(&db_path)?;
    let before = store.count()?;

    if dry_run {
        // Preview what would be pruned
        let to_prune = store.get_patterns_below_score(min_score)?;
        println!("DRY RUN: Would prune {} patterns (score < {})", to_prune.len(), min_score);
        println!();

        if !to_prune.is_empty() {
            println!("Patterns that would be removed:");
            println!("{}", "-".repeat(60));
            for pattern in &to_prune {
                let score = pattern.success_count - pattern.failure_count;
                let preview: String = pattern.context_query
                    .lines()
                    .take(1)
                    .collect::<Vec<_>>()
                    .join(" ");
                let preview = if preview.len() > 60 {
                    format!("{}...", &preview[..60])
                } else {
                    preview
                };
                println!("  #{} [{}] score={}: {}", pattern.id, pattern.tool_type, score, preview);
            }
            println!();
            println!("Run without --dry-run to actually delete these patterns.");
        }
    } else {
        let pruned = store.prune_low_score(min_score)?;
        let after = store.count()?;

        println!("Pruned {} patterns (score < {})", pruned, min_score);
        println!("Patterns: {} -> {}", before, after);
    }

    Ok(())
}

/// Reset patterns and re-learn from logs
pub async fn relearn() -> Result<()> {
    use crate::learning::foreground_learn;

    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        println!("No database found. Run 'mana init' first.");
        return Ok(());
    }

    // Clear existing patterns
    let conn = Connection::open(&db_path)?;
    let deleted: i64 = conn.query_row("SELECT COUNT(*) FROM patterns", [], |r| r.get(0))?;
    conn.execute("DELETE FROM patterns", [])?;
    println!("Cleared {} existing patterns", deleted);

    // Reset learning state
    let state_path = mana_dir.join("learning-state.json");
    if state_path.exists() {
        std::fs::remove_file(&state_path)?;
    }

    // Re-learn from logs
    println!("Re-learning from Claude logs...");
    let result = foreground_learn(&[]).await?;
    println!(
        "Created {} patterns from {} trajectories in {}ms",
        result.patterns_created, result.trajectories_processed, result.duration_ms
    );

    Ok(())
}
