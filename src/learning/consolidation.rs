//! Background consolidation - pattern optimization
//!
//! Runs asynchronously after foreground learning to:
//! - Merge similar patterns
//! - Decay unused patterns
//! - Build skill summaries

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use rusqlite::{Connection, params};
use tracing::{debug, info, warn};

use crate::storage::calculate_similarity;

/// Run consolidation tasks manually
pub async fn consolidate() -> Result<()> {
    info!("Starting consolidation");

    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        info!("No database found, skipping consolidation");
        return Ok(());
    }

    // Run consolidation tasks
    let merged = merge_similar_patterns(&db_path)?;
    let decayed = decay_unused_patterns(&db_path)?;
    let pruned = prune_low_quality_patterns(&db_path)?;

    // Consolidate patterns into skills
    let skills = consolidate_to_skills(&db_path)?;

    info!(
        "Consolidation complete: merged {} patterns, decayed {}, pruned {}, created {} skills",
        merged, decayed, pruned, skills
    );
    Ok(())
}

/// Consolidate patterns into skills
fn consolidate_to_skills(db_path: &PathBuf) -> Result<usize> {
    use crate::storage::consolidate_patterns_to_skills;

    consolidate_patterns_to_skills(db_path)
}

/// Merge patterns with very high similarity (>90%)
fn merge_similar_patterns(db_path: &PathBuf) -> Result<usize> {
    let conn = Connection::open(db_path)?;

    // Get all patterns grouped by tool type
    let mut stmt = conn.prepare(
        "SELECT id, tool_type, context_query, success_count, failure_count FROM patterns ORDER BY tool_type, (success_count - failure_count) DESC"
    )?;

    let patterns: Vec<(i64, String, String, i64, i64)> = stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?.filter_map(|r| r.ok()).collect();

    // Group by tool type
    let mut by_type: HashMap<String, Vec<(i64, String, i64, i64)>> = HashMap::new();
    for (id, tool_type, context, success, failure) in patterns {
        by_type.entry(tool_type).or_default().push((id, context, success, failure));
    }

    let mut merged_count = 0;
    let mut to_delete: Vec<i64> = Vec::new();

    for (_tool_type, type_patterns) in by_type {
        // Compare each pattern with others in same group
        let mut merged_into: HashMap<i64, i64> = HashMap::new();

        for i in 0..type_patterns.len() {
            let (id_i, ref ctx_i, _, _) = type_patterns[i];

            // Skip if already merged into another pattern
            if merged_into.contains_key(&id_i) {
                continue;
            }

            for (id_j, ctx_j, success_j, failure_j) in type_patterns.iter().skip(i + 1) {
                let (id_j, ctx_j, success_j, failure_j) = (*id_j, ctx_j, *success_j, *failure_j);

                // Skip if already merged
                if merged_into.contains_key(&id_j) {
                    continue;
                }

                let similarity = calculate_similarity(ctx_i, ctx_j);

                // Very high similarity = merge (90% threshold for consolidation)
                if similarity > 0.90 {
                    debug!("Merging pattern {} into {} (similarity: {:.2})", id_j, id_i, similarity);

                    // Merge counts into the first pattern
                    conn.execute(
                        "UPDATE patterns SET success_count = success_count + ?, failure_count = failure_count + ? WHERE id = ?",
                        params![success_j, failure_j, id_i],
                    )?;

                    // Mark for deletion
                    to_delete.push(id_j);
                    merged_into.insert(id_j, id_i);
                    merged_count += 1;
                }
            }
        }
    }

    // Delete merged patterns
    for id in &to_delete {
        conn.execute("DELETE FROM patterns WHERE id = ?", params![id])?;
    }

    Ok(merged_count)
}

/// Decay patterns that haven't been used recently
fn decay_unused_patterns(db_path: &PathBuf) -> Result<usize> {
    let conn = Connection::open(db_path)?;

    // Decay patterns not used in 7+ days
    let changes = conn.execute(
        r#"
        UPDATE patterns
        SET success_count = MAX(0, success_count - 1)
        WHERE last_used IS NULL
           OR last_used < datetime('now', '-7 days')
        "#,
        [],
    )?;

    Ok(changes)
}

/// Prune patterns with very low scores
fn prune_low_quality_patterns(db_path: &PathBuf) -> Result<usize> {
    let conn = Connection::open(db_path)?;

    // Delete patterns with very negative scores (failures > successes + 3)
    let changes = conn.execute(
        "DELETE FROM patterns WHERE (success_count - failure_count) < -3",
        [],
    )?;

    Ok(changes)
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

/// Spawn background consolidation process
///
/// Fire-and-forget: starts a detached process to run consolidation
/// without blocking the session-end hook.
pub fn spawn_consolidation() -> Result<()> {
    debug!("Spawning background consolidation");

    // Get path to current binary
    let current_exe = std::env::current_exe()?;

    // Spawn detached process
    // Note: This is a simple implementation; production would use proper daemonization
    match Command::new(&current_exe)
        .arg("consolidate")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {
            debug!("Background consolidation spawned");
            Ok(())
        }
        Err(e) => {
            warn!("Failed to spawn consolidation: {}", e);
            // Don't fail the session-end hook if consolidation can't spawn
            Ok(())
        }
    }
}
