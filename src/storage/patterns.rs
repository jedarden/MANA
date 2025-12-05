//! Pattern storage and retrieval
//!
//! Stores patterns in SQLite with metadata and provides
//! fast retrieval for context injection.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::debug;

/// A stored pattern from the ReasoningBank
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: i64,
    pub pattern_hash: String,
    pub tool_type: String,
    pub context_query: String,
    pub success_count: i64,
    pub failure_count: i64,
    pub embedding_id: Option<i64>,
}

/// Pattern store backed by SQLite
pub struct PatternStore {
    conn: Connection,
}

impl PatternStore {
    /// Open or create a pattern store at the given path
    /// Uses default SQLite settings for maximum compatibility
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        Ok(Self { conn })
    }

    /// Open pattern store with read optimizations (for inject command)
    /// Skips write-related pragmas for faster startup
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Open pattern store with write optimizations (for learning/consolidation)
    pub fn open_write(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // WAL mode for better concurrent access during writes
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        Ok(Self { conn })
    }

    /// Insert a new pattern with similarity-based deduplication
    /// If a similar pattern exists (similarity > 0.85), we update it instead of creating a new one
    pub fn insert(&self, pattern: &Pattern) -> Result<i64> {
        use crate::storage::calculate_similarity;

        // Check for existing similar patterns of the same tool type
        let existing = self.get_by_tool(&pattern.tool_type, 20)?;

        for existing_pattern in existing {
            let similarity = calculate_similarity(&pattern.context_query, &existing_pattern.context_query);

            // If very similar (>85%), update existing instead of creating new
            if similarity > 0.85 {
                debug!("Merging similar pattern {} (similarity: {:.2})", existing_pattern.id, similarity);

                // Increment success/failure counts on existing pattern
                if pattern.success_count > 0 {
                    self.conn.execute(
                        "UPDATE patterns SET success_count = success_count + 1, last_used = CURRENT_TIMESTAMP WHERE id = ?",
                        params![existing_pattern.id],
                    )?;
                }
                if pattern.failure_count > 0 {
                    self.conn.execute(
                        "UPDATE patterns SET failure_count = failure_count + 1, last_used = CURRENT_TIMESTAMP WHERE id = ?",
                        params![existing_pattern.id],
                    )?;
                }

                return Ok(existing_pattern.id);
            }
        }

        // No similar pattern found, insert new one
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO patterns
            (pattern_hash, tool_type, context_query, success_count, failure_count, embedding_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.context_query,
                pattern.success_count,
                pattern.failure_count,
                pattern.embedding_id
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get patterns by tool type
    pub fn get_by_tool(&self, tool_type: &str, limit: usize) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_hash, tool_type, context_query, success_count, failure_count, embedding_id
            FROM patterns
            WHERE tool_type = ?1
            ORDER BY (success_count - failure_count) DESC, success_count DESC
            LIMIT ?2
            "#,
        )?;

        let patterns = stmt.query_map(params![tool_type, limit as i64], |row| {
            Ok(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                context_query: row.get(3)?,
                success_count: row.get(4)?,
                failure_count: row.get(5)?,
                embedding_id: row.get(6)?,
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Update pattern success/failure counts
    pub fn update_outcome(&self, pattern_id: i64, success: bool) -> Result<()> {
        let column = if success { "success_count" } else { "failure_count" };

        self.conn.execute(
            &format!(
                "UPDATE patterns SET {} = {} + 1, last_used = CURRENT_TIMESTAMP WHERE id = ?",
                column, column
            ),
            params![pattern_id],
        )?;

        Ok(())
    }

    /// Get pattern by ID
    pub fn get_by_id(&self, id: i64) -> Result<Option<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_hash, tool_type, context_query, success_count, failure_count, embedding_id
            FROM patterns
            WHERE id = ?1
            "#,
        )?;

        let mut rows = stmt.query(params![id])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                context_query: row.get(3)?,
                success_count: row.get(4)?,
                failure_count: row.get(5)?,
                embedding_id: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get total pattern count
    pub fn count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Decay unused patterns (reduce success_count)
    pub fn decay_unused(&self, decay_factor: f64, days_threshold: i64) -> Result<u64> {
        let changes = self.conn.execute(
            r#"
            UPDATE patterns
            SET success_count = CAST(success_count * ?1 AS INTEGER)
            WHERE last_used IS NULL OR last_used < datetime('now', ?2 || ' days')
            "#,
            params![decay_factor, -days_threshold],
        )?;

        Ok(changes as u64)
    }

    /// Delete patterns with low scores
    pub fn prune_low_score(&self, min_score: i64) -> Result<u64> {
        let changes = self.conn.execute(
            "DELETE FROM patterns WHERE (success_count - failure_count) < ?1",
            params![min_score],
        )?;

        Ok(changes as u64)
    }

    /// Get top patterns across all tool types (for fallback)
    pub fn get_top_patterns(&self, limit: usize) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_hash, tool_type, context_query, success_count, failure_count, embedding_id
            FROM patterns
            WHERE tool_type != 'failure'
            ORDER BY (success_count - failure_count) DESC, success_count DESC
            LIMIT ?1
            "#,
        )?;

        let patterns = stmt.query_map(params![limit as i64], |row| {
            Ok(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                context_query: row.get(3)?,
                success_count: row.get(4)?,
                failure_count: row.get(5)?,
                embedding_id: row.get(6)?,
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
