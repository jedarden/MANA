//! Storage module for MANA
//!
//! Handles pattern storage in SQLite (metadata) and provides
//! status/statistics reporting.

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tracing::{debug, info};

mod patterns;

pub use patterns::PatternStore;

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

        CREATE INDEX IF NOT EXISTS idx_patterns_tool ON patterns(tool_type);
        CREATE INDEX IF NOT EXISTS idx_patterns_hash ON patterns(pattern_hash);
        "#,
    )?;

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

        println!("Patterns stored: {}", pattern_count);
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
    for result in tool_counts {
        if let Ok((tool, count)) = result {
            println!("    {}: {}", tool, count);
        }
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
