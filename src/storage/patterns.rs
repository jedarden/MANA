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
    /// For Bash patterns, the primary command (cargo, npm, git, etc.)
    /// For Edit patterns, the file extension (rs, ts, py, etc.)
    pub command_category: Option<String>,
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
    /// Uses mmap for faster file access and prepared statement caching
    ///
    /// OPTIMIZATION: Uses minimal pragmas to reduce startup latency.
    /// Testing shows execute_batch adds ~1-2ms overhead. We skip optional
    /// pragmas since SQLite defaults are acceptable for read-only queries.
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        // Use URI mode for additional flags
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;

        // OPTIMIZATION: Skip execute_batch entirely - it adds parsing overhead.
        // SQLite's default cache (2000 pages = 8MB) is sufficient for read-only.
        // mmap is nice-to-have but adds ~0.5ms on cold start.
        // query_only is just a hint and has no performance benefit.

        // Keep prepared statements cached (this is in-memory, fast)
        conn.set_prepared_statement_cache_capacity(4);

        Ok(Self { conn })
    }

    /// Open pattern store with mmap enabled (for latency-sensitive hot paths)
    /// Use this when the connection will be reused many times
    #[allow(dead_code)]
    pub fn open_readonly_with_mmap(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // Enable mmap for repeated queries (amortizes setup cost)
        conn.pragma_update(None, "mmap_size", 2_097_152)?; // 2MB

        conn.set_prepared_statement_cache_capacity(8);

        Ok(Self { conn })
    }

    /// Open pattern store with write optimizations (for learning/consolidation)
    #[allow(dead_code)]
    pub fn open_write(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // WAL mode for better concurrent access during writes
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        Ok(Self { conn })
    }

    /// Fast insert without similarity checks - uses hash-based deduplication
    ///
    /// For bulk loading during learning. Uses INSERT OR IGNORE with pattern_hash
    /// as a uniqueness check. This is O(1) per insert vs O(n) for similarity-based.
    /// Similarity-based consolidation should run separately in background.
    pub fn insert_fast(&self, pattern: &Pattern) -> Result<i64> {
        // Use INSERT OR IGNORE - if pattern_hash already exists, skip silently
        // If it's a duplicate hash, increment the success count instead
        let changes = self.conn.execute(
            r#"
            INSERT INTO patterns
            (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(pattern_hash) DO UPDATE SET
                success_count = success_count + excluded.success_count,
                failure_count = failure_count + excluded.failure_count,
                last_used = CURRENT_TIMESTAMP
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.command_category,
                pattern.context_query,
                pattern.success_count,
                pattern.failure_count,
                pattern.embedding_id
            ],
        )?;

        if changes > 0 {
            Ok(self.conn.last_insert_rowid())
        } else {
            // Pattern was merged with existing
            Ok(0)
        }
    }

    /// Batch insert patterns in a single transaction
    ///
    /// Much faster than individual inserts for bulk loading.
    /// Uses a single transaction to batch all inserts, reducing disk I/O.
    pub fn insert_batch(&mut self, patterns: &[Pattern]) -> Result<usize> {
        // Start a transaction for the batch
        let tx = self.conn.transaction()?;

        let mut inserted = 0;
        {
            let mut stmt = tx.prepare_cached(
                r#"
                INSERT INTO patterns
                (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(pattern_hash) DO UPDATE SET
                    success_count = success_count + excluded.success_count,
                    failure_count = failure_count + excluded.failure_count,
                    last_used = CURRENT_TIMESTAMP
                "#,
            )?;

            for pattern in patterns {
                if stmt.execute(params![
                    pattern.pattern_hash,
                    pattern.tool_type,
                    pattern.command_category,
                    pattern.context_query,
                    pattern.success_count,
                    pattern.failure_count,
                    pattern.embedding_id
                ]).is_ok() {
                    inserted += 1;
                }
            }
        }

        tx.commit()?;
        Ok(inserted)
    }

    /// Insert a new pattern with similarity-based deduplication
    /// If a similar pattern exists (similarity > 0.85), we update it instead of creating a new one
    ///
    /// NOTE: This is slow for bulk operations. Use insert_fast() for learning.
    #[allow(dead_code)]
    pub fn insert(&self, pattern: &Pattern) -> Result<i64> {
        use crate::storage::calculate_similarity;

        // Check for existing similar patterns of the same tool type AND command category
        // This ensures Rust patterns don't get merged with Python patterns
        let existing = if pattern.command_category.is_some() {
            self.get_by_tool_and_category(&pattern.tool_type, pattern.command_category.as_deref(), 20)?
        } else {
            self.get_by_tool(&pattern.tool_type, 20)?
        };

        for existing_pattern in existing {
            let similarity = calculate_similarity(&pattern.context_query, &existing_pattern.context_query);

            // If very similar (>70%), update existing instead of creating new
            // Now that we filter by command_category, this only merges within the same tech stack
            if similarity > 0.70 {
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
            (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.command_category,
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
        // Use prepare_cached for faster repeated queries
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id
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
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get patterns by tool type and command category
    /// This is more efficient for Bash patterns where we want cargo vs npm vs git
    #[allow(dead_code)]
    pub fn get_by_tool_and_category(&self, tool_type: &str, category: Option<&str>, limit: usize) -> Result<Vec<Pattern>> {
        match category {
            Some(cat) => {
                let mut stmt = self.conn.prepare(
                    r#"
                    SELECT id, pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id
                    FROM patterns
                    WHERE tool_type = ?1 AND command_category = ?2
                    ORDER BY (success_count - failure_count) DESC, success_count DESC
                    LIMIT ?3
                    "#,
                )?;

                let patterns = stmt.query_map(params![tool_type, cat, limit as i64], |row| {
                    Ok(Pattern {
                        id: row.get(0)?,
                        pattern_hash: row.get(1)?,
                        tool_type: row.get(2)?,
                        command_category: row.get(3)?,
                        context_query: row.get(4)?,
                        success_count: row.get(5)?,
                        failure_count: row.get(6)?,
                        embedding_id: row.get(7)?,
                    })
                })?;

                patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
            }
            None => {
                // Fall back to get_by_tool when no category specified
                self.get_by_tool(tool_type, limit)
            }
        }
    }

    /// Update pattern success/failure counts
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn get_by_id(&self, id: i64) -> Result<Option<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id
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
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
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
    #[allow(dead_code)]
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

    /// Get patterns with score below threshold (for preview before pruning)
    pub fn get_patterns_below_score(&self, min_score: i64) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id
            FROM patterns
            WHERE (success_count - failure_count) < ?1
            ORDER BY (success_count - failure_count) ASC
            "#,
        )?;

        let patterns = stmt.query_map(params![min_score], |row| {
            Ok(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get top patterns across all tool types (for fallback)
    pub fn get_top_patterns(&self, limit: usize) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id
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
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
