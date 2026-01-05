//! Background consolidation - pattern optimization and self-healing
//!
//! Runs asynchronously after foreground learning to:
//! - Merge similar patterns
//! - Decay unused patterns
//! - Build skill summaries
//! - Self-heal: validate, repair, and prevent degradation

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use rusqlite::{Connection, params};
use tracing::{debug, info, warn};

use crate::storage::calculate_similarity;

/// Self-healing validation report
#[derive(Debug, Default)]
pub struct ValidationReport {
    pub negative_counts: usize,
    pub orphaned_embeddings: usize,
    pub duplicate_hashes: usize,
    pub unbounded_scores: usize,
    pub repaired: usize,
}

impl ValidationReport {
    pub fn has_issues(&self) -> bool {
        self.negative_counts > 0 || self.orphaned_embeddings > 0
            || self.duplicate_hashes > 0 || self.unbounded_scores > 0
    }

    pub fn total_issues(&self) -> usize {
        self.negative_counts + self.orphaned_embeddings + self.duplicate_hashes + self.unbounded_scores
    }
}

/// Score bounds for self-healing (prevents unbounded decay/growth)
const MIN_SCORE: i64 = -10;
const MAX_SCORE: i64 = 1000;
const MAX_SUCCESS_COUNT: i64 = 500;
const MAX_FAILURE_COUNT: i64 = 100;

/// Run consolidation tasks manually
pub async fn consolidate() -> Result<()> {
    info!("Starting consolidation with self-healing");

    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        info!("No database found, skipping consolidation");
        return Ok(());
    }

    // SELF-HEALING PHASE 1: Validate and repair patterns
    let validation = validate_and_repair_patterns(&db_path)?;
    if validation.has_issues() {
        info!("Self-healing: found {} issues, repaired {}",
              validation.total_issues(), validation.repaired);
    }

    // SELF-HEALING PHASE 2: Normalize unbounded scores
    let normalized = normalize_pattern_scores(&db_path)?;
    if normalized > 0 {
        info!("Self-healing: normalized {} patterns with unbounded scores", normalized);
    }

    // Clean up invalid causal edges first (self-referential, orphaned)
    let cleaned = cleanup_causal_edges(&db_path)?;
    if cleaned > 0 {
        info!("Cleaned up {} invalid causal edges", cleaned);
    }

    // Run consolidation tasks
    let merged = merge_similar_patterns(&db_path)?;
    let decayed = decay_unused_patterns(&db_path)?;
    let pruned = prune_low_quality_patterns(&db_path)?;

    // Consolidate patterns into skills
    let skills = consolidate_to_skills(&db_path)?;

    // SELF-HEALING PHASE 3: Database maintenance (periodic)
    let vacuumed = periodic_vacuum(&db_path)?;
    if vacuumed {
        debug!("Performed periodic database vacuum");
    }

    info!(
        "Consolidation complete: merged {} patterns, decayed {}, pruned {}, created {} skills",
        merged, decayed, pruned, skills
    );
    Ok(())
}

/// Self-healing: Validate patterns and repair corruption
fn validate_and_repair_patterns(db_path: &Path) -> Result<ValidationReport> {
    let conn = Connection::open(db_path)?;
    let mut report = ValidationReport::default();

    // Check for negative counts (data corruption)
    let negative_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns WHERE success_count < 0 OR failure_count < 0",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    report.negative_counts = negative_count as usize;

    if negative_count > 0 {
        // Repair: clamp negative values to 0
        let repaired = conn.execute(
            "UPDATE patterns SET
                success_count = MAX(0, success_count),
                failure_count = MAX(0, failure_count)
             WHERE success_count < 0 OR failure_count < 0",
            []
        )?;
        report.repaired += repaired;
        warn!("Self-healing: repaired {} patterns with negative counts", repaired);
    }

    // Check for duplicate hashes (should be unique)
    let duplicates: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT pattern_hash, COUNT(*) as cnt FROM patterns
            GROUP BY pattern_hash HAVING cnt > 1
        )",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    report.duplicate_hashes = duplicates as usize;

    if duplicates > 0 {
        // Repair: merge duplicate hashes by keeping highest-scoring one
        let merged = merge_duplicate_hashes(&conn)?;
        report.repaired += merged;
        warn!("Self-healing: merged {} duplicate pattern hashes", merged);
    }

    // Check for orphaned embedding references
    let _orphaned: i64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns WHERE embedding_id IS NOT NULL AND embedding_id != 0",
        [],
        |row| row.get(0)
    ).unwrap_or(0);
    // Note: We can't fully verify embeddings without loading the index,
    // but we track the count for monitoring
    report.orphaned_embeddings = 0; // Would need index access to verify

    Ok(report)
}

/// Merge patterns with duplicate hashes (keeps the one with highest score)
fn merge_duplicate_hashes(conn: &Connection) -> Result<usize> {
    // Find all duplicate hashes
    let mut stmt = conn.prepare(
        "SELECT pattern_hash FROM patterns GROUP BY pattern_hash HAVING COUNT(*) > 1"
    )?;

    let hashes: Vec<String> = stmt.query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut merged = 0;
    for hash in hashes {
        // Get all patterns with this hash, ordered by score
        let mut stmt = conn.prepare(
            "SELECT id, success_count, failure_count FROM patterns
             WHERE pattern_hash = ?
             ORDER BY (success_count - failure_count) DESC"
        )?;

        let patterns: Vec<(i64, i64, i64)> = stmt.query_map([&hash], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?.filter_map(|r| r.ok()).collect();

        if patterns.len() > 1 {
            let (keep_id, _, _) = patterns[0];

            // Aggregate counts from duplicates into the keeper
            let total_success: i64 = patterns.iter().map(|(_, s, _)| s).sum();
            let total_failure: i64 = patterns.iter().map(|(_, _, f)| f).sum();

            // Update keeper with aggregated counts
            conn.execute(
                "UPDATE patterns SET success_count = ?, failure_count = ? WHERE id = ?",
                params![total_success, total_failure, keep_id]
            )?;

            // Delete duplicates
            for (id, _, _) in patterns.iter().skip(1) {
                conn.execute("DELETE FROM patterns WHERE id = ?", params![id])?;
                merged += 1;
            }
        }
    }

    Ok(merged)
}

/// Self-healing: Normalize scores to prevent unbounded growth/decay
fn normalize_pattern_scores(db_path: &Path) -> Result<usize> {
    let conn = Connection::open(db_path)?;

    // Find patterns with unbounded scores
    let unbounded: i64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns
         WHERE success_count > ? OR failure_count > ?
            OR (success_count - failure_count) < ?
            OR (success_count - failure_count) > ?",
        params![MAX_SUCCESS_COUNT, MAX_FAILURE_COUNT, MIN_SCORE, MAX_SCORE],
        |row| row.get(0)
    ).unwrap_or(0);

    if unbounded == 0 {
        return Ok(0);
    }

    // Normalize: apply exponential smoothing to bring counts into bounds
    // This preserves the ratio while reducing absolute values
    let normalized = conn.execute(
        r#"
        UPDATE patterns
        SET
            success_count = CASE
                WHEN success_count > ? THEN CAST(success_count * 0.8 AS INTEGER)
                ELSE success_count
            END,
            failure_count = CASE
                WHEN failure_count > ? THEN CAST(failure_count * 0.8 AS INTEGER)
                ELSE failure_count
            END
        WHERE success_count > ? OR failure_count > ?
        "#,
        params![MAX_SUCCESS_COUNT, MAX_FAILURE_COUNT, MAX_SUCCESS_COUNT, MAX_FAILURE_COUNT]
    )?;

    // Also clamp extreme negative scores (prevent indefinite decay)
    conn.execute(
        "UPDATE patterns SET failure_count = success_count - ?
         WHERE (success_count - failure_count) < ?",
        params![MIN_SCORE, MIN_SCORE]
    )?;

    Ok(normalized)
}

/// Periodic database vacuum to prevent fragmentation
fn periodic_vacuum(db_path: &Path) -> Result<bool> {
    use std::time::SystemTime;

    // Only vacuum once per day (check via file modification time)
    let metadata = std::fs::metadata(db_path)?;
    let modified = metadata.modified()?;
    let now = SystemTime::now();

    let age_hours = now.duration_since(modified)
        .map(|d| d.as_secs() / 3600)
        .unwrap_or(0);

    // Vacuum if database hasn't been modified in 24+ hours or is first run
    if age_hours >= 24 {
        let conn = Connection::open(db_path)?;
        conn.execute("VACUUM", [])?;
        // Rebuild indices for optimal performance
        conn.execute("REINDEX", [])?;
        return Ok(true);
    }

    Ok(false)
}

/// Clean up invalid causal edges (self-referential, orphaned)
fn cleanup_causal_edges(db_path: &Path) -> Result<usize> {
    use crate::storage::CausalStore;

    let store = CausalStore::open(db_path)?;
    let self_ref = store.cleanup_self_referential()?;
    let orphaned = store.cleanup_orphaned()?;
    Ok(self_ref + orphaned)
}

/// Consolidate patterns into skills
fn consolidate_to_skills(db_path: &Path) -> Result<usize> {
    use crate::storage::consolidate_patterns_to_skills;

    consolidate_patterns_to_skills(db_path)
}

/// Similarity threshold for regular patterns (tool calls, failures, etc.)
const REGULAR_SIMILARITY_THRESHOLD: f64 = 0.90;

/// Lower similarity threshold for instruction patterns
/// Instructions like "always use TypeScript" and "use TypeScript for all code"
/// should consolidate despite wording differences
const INSTRUCTION_SIMILARITY_THRESHOLD: f64 = 0.70;

/// Merge patterns with high similarity
/// Uses 90% threshold for regular patterns, 70% for instructions
fn merge_similar_patterns(db_path: &Path) -> Result<usize> {
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

    for (tool_type, type_patterns) in by_type {
        // Use lower threshold for instruction patterns to allow more variation in wording
        let similarity_threshold = if tool_type == "instruction" {
            INSTRUCTION_SIMILARITY_THRESHOLD
        } else {
            REGULAR_SIMILARITY_THRESHOLD
        };

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

                // Merge if above threshold
                if similarity > similarity_threshold {
                    debug!("Merging {} pattern {} into {} (similarity: {:.2})",
                           tool_type, id_j, id_i, similarity);

                    // Merge counts into the first pattern
                    conn.execute(
                        "UPDATE patterns SET success_count = success_count + ?, failure_count = failure_count + ? WHERE id = ?",
                        params![success_j, failure_j, id_i],
                    )?;

                    // For instruction patterns, also merge session tracking
                    if tool_type == "instruction" {
                        merge_instruction_sessions(&conn, id_i, id_j)?;
                    }

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

/// Merge session tracking data when consolidating instruction patterns
fn merge_instruction_sessions(conn: &Connection, keep_id: i64, merge_id: i64) -> Result<()> {
    use std::collections::HashSet;

    // Get session data from both patterns
    let keep_sessions: Option<String> = conn.query_row(
        "SELECT session_ids FROM patterns WHERE id = ?",
        params![keep_id],
        |row| row.get(0),
    ).ok().flatten();

    let merge_sessions: Option<String> = conn.query_row(
        "SELECT session_ids FROM patterns WHERE id = ?",
        params![merge_id],
        |row| row.get(0),
    ).ok().flatten();

    // Combine session sets
    let mut combined: HashSet<String> = keep_sessions
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    if let Some(merge_str) = merge_sessions {
        if let Ok(merge_set) = serde_json::from_str::<HashSet<String>>(&merge_str) {
            combined.extend(merge_set);
        }
    }

    // Update session count and frequency weight
    let new_session_count = combined.len() as i64;
    let total_occurrences: i64 = conn.query_row(
        "SELECT success_count + failure_count FROM patterns WHERE id = ?",
        params![keep_id],
        |row| row.get(0),
    ).unwrap_or(1);

    // Recalculate frequency weight
    let occurrence_factor = (1.0 + total_occurrences as f64).ln();
    let session_factor = if new_session_count > 1 {
        1.0 + (new_session_count as f64).ln() * 0.5
    } else {
        1.0
    };
    let new_weight = occurrence_factor * session_factor;

    conn.execute(
        r#"
        UPDATE patterns
        SET session_count = ?,
            frequency_weight = ?,
            session_ids = ?
        WHERE id = ?
        "#,
        params![
            new_session_count,
            new_weight,
            serde_json::to_string(&combined).unwrap_or_default(),
            keep_id
        ],
    )?;

    Ok(())
}

/// Decay patterns that haven't been used recently
fn decay_unused_patterns(db_path: &Path) -> Result<usize> {
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
fn prune_low_quality_patterns(db_path: &Path) -> Result<usize> {
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
