//! Causal edge storage and discovery
//!
//! Tracks relationships between patterns to detect conflicts and synergies.
//! A causal edge records whether patterns tend to succeed or fail together.

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use tracing::debug;

/// A causal edge representing a relationship between two patterns
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CausalEdge {
    pub id: i64,
    pub pattern_a_id: i64,
    pub pattern_b_id: i64,
    /// Lift score: >1.5 = synergy, <0.5 = conflict
    pub lift: f64,
    pub co_occurrences: i64,
}

/// Causal edge store backed by SQLite
pub struct CausalStore {
    conn: Connection,
}

impl CausalStore {
    /// Open or create a causal store at the given database path
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        Ok(Self { conn })
    }

    /// Open causal store in read-only mode for fast queries
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Record a co-occurrence of two patterns with an outcome
    /// This updates the lift score based on whether they succeeded together
    pub fn record_cooccurrence(
        &self,
        pattern_a: i64,
        pattern_b: i64,
        both_succeeded: bool,
    ) -> Result<()> {
        // Skip self-referential edges - a pattern cannot conflict with itself
        if pattern_a == pattern_b {
            debug!("Skipping self-referential causal edge for pattern {}", pattern_a);
            return Ok(());
        }

        // Ensure consistent ordering (smaller ID first)
        let (id_a, id_b) = if pattern_a < pattern_b {
            (pattern_a, pattern_b)
        } else {
            (pattern_b, pattern_a)
        };

        // Check if edge exists
        let existing: Option<(i64, f64, i64)> = self.conn.query_row(
            "SELECT id, lift, co_occurrences FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).ok();

        match existing {
            Some((id, current_lift, co_count)) => {
                // Update existing edge with exponential moving average
                // Success pushes lift up toward 1.5, failure pushes it down toward 0.3
                // This ensures repeated failures can drive lift below 0.5 threshold
                let outcome_value = if both_succeeded { 1.5 } else { 0.3 };
                let alpha = 0.3; // Learning rate
                let new_lift = current_lift * (1.0 - alpha) + outcome_value * alpha;

                self.conn.execute(
                    "UPDATE causal_edges SET lift = ?, co_occurrences = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                    params![new_lift, co_count + 1, id],
                )?;
                debug!("Updated causal edge {} -> {}: lift {:.2} -> {:.2}", id_a, id_b, current_lift, new_lift);
            }
            None => {
                // Create new edge
                let initial_lift = if both_succeeded { 1.2 } else { 0.8 };

                self.conn.execute(
                    "INSERT INTO causal_edges (pattern_a_id, pattern_b_id, lift, co_occurrences) VALUES (?, ?, ?, 1)",
                    params![id_a, id_b, initial_lift],
                )?;
                debug!("Created causal edge {} -> {}: lift {:.2}", id_a, id_b, initial_lift);
            }
        }

        Ok(())
    }

    /// Get all conflicting patterns for a given pattern ID
    /// Returns pattern IDs that have lift < 0.5 (conflict threshold)
    pub fn get_conflicts(&self, pattern_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT pattern_b_id FROM causal_edges
            WHERE pattern_a_id = ? AND lift < 0.5 AND co_occurrences >= 3
            UNION
            SELECT pattern_a_id FROM causal_edges
            WHERE pattern_b_id = ? AND lift < 0.5 AND co_occurrences >= 3
            "#,
        )?;

        let conflicts = stmt.query_map(params![pattern_id, pattern_id], |row| row.get(0))?;
        conflicts.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all synergistic patterns for a given pattern ID
    /// Returns pattern IDs that have lift > 1.5 (synergy threshold)
    #[allow(dead_code)]
    pub fn get_synergies(&self, pattern_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT pattern_b_id FROM causal_edges
            WHERE pattern_a_id = ? AND lift > 1.5 AND co_occurrences >= 3
            UNION
            SELECT pattern_a_id FROM causal_edges
            WHERE pattern_b_id = ? AND lift > 1.5 AND co_occurrences >= 3
            "#,
        )?;

        let synergies = stmt.query_map(params![pattern_id, pattern_id], |row| row.get(0))?;
        synergies.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all edges for a pattern (for debugging/stats)
    #[allow(dead_code)]
    pub fn get_edges(&self, pattern_id: i64) -> Result<Vec<CausalEdge>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_a_id, pattern_b_id, lift, co_occurrences
            FROM causal_edges
            WHERE pattern_a_id = ? OR pattern_b_id = ?
            ORDER BY lift ASC
            "#,
        )?;

        let edges = stmt.query_map(params![pattern_id, pattern_id], |row| {
            Ok(CausalEdge {
                id: row.get(0)?,
                pattern_a_id: row.get(1)?,
                pattern_b_id: row.get(2)?,
                lift: row.get(3)?,
                co_occurrences: row.get(4)?,
            })
        })?;

        edges.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get count of causal edges
    #[allow(dead_code)]
    pub fn count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM causal_edges", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Clean up edges referencing deleted patterns
    pub fn cleanup_orphaned(&self) -> Result<usize> {
        let deleted = self.conn.execute(
            r#"
            DELETE FROM causal_edges
            WHERE pattern_a_id NOT IN (SELECT id FROM patterns)
               OR pattern_b_id NOT IN (SELECT id FROM patterns)
            "#,
            [],
        )?;
        Ok(deleted)
    }

    /// Clean up invalid self-referential edges (pattern conflicting with itself)
    pub fn cleanup_self_referential(&self) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM causal_edges WHERE pattern_a_id = pattern_b_id",
            [],
        )?;
        if deleted > 0 {
            debug!("Removed {} self-referential causal edges", deleted);
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (NamedTempFile, CausalStore) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();

        // Create minimal schema
        conn.execute_batch(
            r#"
            CREATE TABLE patterns (
                id INTEGER PRIMARY KEY,
                pattern_hash TEXT,
                tool_type TEXT,
                context_query TEXT,
                success_count INTEGER DEFAULT 0,
                failure_count INTEGER DEFAULT 0
            );
            CREATE TABLE causal_edges (
                id INTEGER PRIMARY KEY,
                pattern_a_id INTEGER,
                pattern_b_id INTEGER,
                lift REAL,
                co_occurrences INTEGER DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(pattern_a_id, pattern_b_id)
            );
            INSERT INTO patterns (id, pattern_hash, tool_type, context_query) VALUES
                (1, 'hash1', 'Bash', 'Pattern 1'),
                (2, 'hash2', 'Bash', 'Pattern 2'),
                (3, 'hash3', 'Edit', 'Pattern 3');
            "#,
        ).unwrap();
        drop(conn);

        let store = CausalStore::open(tmp.path()).unwrap();
        (tmp, store)
    }

    #[test]
    fn test_record_cooccurrence_creates_edge() {
        let (_tmp, store) = setup_test_db();

        store.record_cooccurrence(1, 2, true).unwrap();

        let count = store.count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_record_cooccurrence_updates_lift() {
        let (_tmp, store) = setup_test_db();

        // Record several failures - lift should decrease
        for _ in 0..5 {
            store.record_cooccurrence(1, 2, false).unwrap();
        }

        let edges = store.get_edges(1).unwrap();
        assert_eq!(edges.len(), 1);
        assert!(edges[0].lift < 0.6, "Lift should be low after failures: {}", edges[0].lift);
    }

    #[test]
    fn test_get_conflicts() {
        let (_tmp, store) = setup_test_db();

        // Record many failures to create a conflict
        // With EMA (alpha=0.3) starting at 0.8, need ~20 failures to get below 0.5
        for _ in 0..20 {
            store.record_cooccurrence(1, 2, false).unwrap();
        }

        // Verify lift dropped below threshold
        let edges = store.get_edges(1).unwrap();
        assert!(!edges.is_empty(), "Should have created an edge");
        assert!(edges[0].lift < 0.5, "Lift should be below conflict threshold: {}", edges[0].lift);
        assert!(edges[0].co_occurrences >= 3, "Should have enough co-occurrences: {}", edges[0].co_occurrences);

        let conflicts = store.get_conflicts(1).unwrap();
        assert!(conflicts.contains(&2), "Pattern 2 should be a conflict");
    }

    #[test]
    fn test_edge_ordering() {
        let (_tmp, store) = setup_test_db();

        // Regardless of order passed, should create same edge
        store.record_cooccurrence(2, 1, true).unwrap();
        store.record_cooccurrence(1, 2, true).unwrap();

        let count = store.count().unwrap();
        assert_eq!(count, 1, "Should only create one edge regardless of order");
    }
}
