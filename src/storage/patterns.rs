//! Pattern storage and retrieval
//!
//! Stores patterns in SQLite with metadata and provides
//! fast retrieval for context injection.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::debug;

use super::tiers::TierPath;

/// A stored pattern from the ReasoningBank
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: i64,
    pub pattern_hash: String,
    pub tool_type: String,
    /// For Bash patterns, the primary command (cargo, npm, git, etc.)
    /// For Edit patterns, the file extension (rs, ts, py, etc.)
    /// For instruction patterns, the category (testing, coding_style, version_control, etc.)
    pub command_category: Option<String>,
    pub context_query: String,
    pub success_count: i64,
    pub failure_count: i64,
    pub embedding_id: Option<i64>,
    /// Last time this pattern was used (ISO 8601 string)
    pub last_used: Option<String>,
    /// Number of times this pattern has been accessed
    pub access_count: i64,
    /// Hierarchical memory tier path (e.g., "global", "domain/infrastructure/k8s", "project/mana")
    pub tier_path: String,
    /// Number of unique sessions where this pattern appeared (for instruction patterns)
    #[serde(default = "default_session_count")]
    pub session_count: i64,
    /// Frequency weight multiplier - higher means more frequently repeated
    /// Calculated as: ln(1 + occurrences) * (1 + ln(session_count) * 0.5)
    #[serde(default = "default_frequency_weight")]
    pub frequency_weight: f64,
    /// JSON array of session IDs where this instruction appeared
    #[serde(default)]
    pub session_ids: Option<String>,
}

fn default_session_count() -> i64 { 1 }
fn default_frequency_weight() -> f64 { 1.0 }

impl Default for Pattern {
    fn default() -> Self {
        Self {
            id: 0,
            pattern_hash: String::new(),
            tool_type: String::new(),
            command_category: None,
            context_query: String::new(),
            success_count: 0,
            failure_count: 0,
            embedding_id: None,
            last_used: None,
            access_count: 0,
            tier_path: "global".to_string(),
            session_count: default_session_count(),
            frequency_weight: default_frequency_weight(),
            session_ids: None,
        }
    }
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
    /// Use this when the connection will be reused many times (e.g., daemon mode)
    ///
    /// Performance characteristics:
    /// - mmap_size=8MB: Maps DB to memory, avoids syscalls for reads
    /// - cache_size=4000 pages: ~16MB cache for hot data
    /// - temp_store=MEMORY: Keeps temp tables in RAM
    /// - prepared_statement_cache=16: Caches compiled SQL
    pub fn open_readonly_with_mmap(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // Enable mmap for repeated queries (amortizes setup cost)
        // 8MB is enough for most pattern databases (<10K patterns)
        conn.pragma_update(None, "mmap_size", 8_388_608)?; // 8MB

        // Increase page cache for hot data
        conn.pragma_update(None, "cache_size", 4000)?; // ~16MB

        // Keep temp tables in memory
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        // Larger prepared statement cache for hot paths
        conn.set_prepared_statement_cache_capacity(16);

        Ok(Self { conn })
    }

    /// Open pattern store optimized for maximum read performance
    /// Use this for benchmarking or daemon mode where startup cost is amortized
    #[allow(dead_code)]
    pub fn open_hot(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE,
        )?;

        // Maximum mmap for memory-mapped I/O
        conn.pragma_update(None, "mmap_size", 30_000_000)?; // 30MB

        // Large cache
        conn.pragma_update(None, "cache_size", 8000)?; // ~32MB

        // Memory temp store
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        // Disable locking for read-only
        conn.pragma_update(None, "locking_mode", "NORMAL")?;

        // Maximum prepared statement cache
        conn.set_prepared_statement_cache_capacity(32);

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
            (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id, access_count, tier_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(pattern_hash) DO UPDATE SET
                success_count = success_count + excluded.success_count,
                failure_count = failure_count + excluded.failure_count,
                last_used = CURRENT_TIMESTAMP,
                access_count = access_count + 1
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.command_category,
                pattern.context_query,
                pattern.success_count,
                pattern.failure_count,
                pattern.embedding_id,
                pattern.access_count,
                pattern.tier_path
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
                (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id, access_count, tier_path)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(pattern_hash) DO UPDATE SET
                    success_count = success_count + excluded.success_count,
                    failure_count = failure_count + excluded.failure_count,
                    last_used = CURRENT_TIMESTAMP,
                    access_count = access_count + 1
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
                    pattern.embedding_id,
                    pattern.access_count,
                    pattern.tier_path
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
            (pattern_hash, tool_type, command_category, context_query, success_count, failure_count, embedding_id, access_count, tier_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.command_category,
                pattern.context_query,
                pattern.success_count,
                pattern.failure_count,
                pattern.embedding_id,
                pattern.access_count,
                pattern.tier_path
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get patterns by tool type
    pub fn get_by_tool(&self, tool_type: &str, limit: usize) -> Result<Vec<Pattern>> {
        // Use prepare_cached for faster repeated queries
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
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
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
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
                    SELECT id, pattern_hash, tool_type, command_category, context_query,
                           success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                           session_count, frequency_weight, session_ids
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
                        last_used: row.get(8).ok(),
                        access_count: row.get(9).unwrap_or(0),
                        tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                        session_count: row.get(11).unwrap_or(1),
                        frequency_weight: row.get(12).unwrap_or(1.0),
                        session_ids: row.get(13).ok(),
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
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
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
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
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
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
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
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Mark patterns as used (updates last_used timestamp and increments access_count)
    /// Called after patterns are injected into context to prevent decay
    pub fn mark_patterns_used(&self, pattern_ids: &[i64]) -> Result<usize> {
        if pattern_ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<String> = pattern_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "UPDATE patterns SET last_used = CURRENT_TIMESTAMP, access_count = access_count + 1 WHERE id IN ({})",
            placeholders.join(",")
        );

        let params: Vec<&dyn rusqlite::ToSql> = pattern_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();

        let changes = self.conn.execute(&sql, params.as_slice())?;
        Ok(changes)
    }

    /// Get top patterns across all tool types (for fallback)
    pub fn get_top_patterns(&self, limit: usize) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
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
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get patterns by tier path
    /// Returns patterns matching the exact tier path
    pub fn get_by_tier(&self, tier: &TierPath, limit: usize) -> Result<Vec<Pattern>> {
        let tier_str = tier.to_path_string();

        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
            FROM patterns
            WHERE tier_path = ?1
            ORDER BY (success_count - failure_count) DESC, success_count DESC
            LIMIT ?2
            "#,
        )?;

        let patterns = stmt.query_map(params![tier_str, limit as i64], |row| {
            Ok(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Search patterns with automatic tier fallback
    ///
    /// Searches through the tier hierarchy, returning patterns from the most specific
    /// tier first, then falling back to broader tiers if needed.
    ///
    /// # Arguments
    ///
    /// * `tier` - The starting tier path to search from
    /// * `tool_type` - Optional tool type filter (None = all tool types)
    /// * `limit` - Maximum total patterns to return across all tiers
    ///
    /// # Returns
    ///
    /// Patterns ordered by:
    /// 1. Tier priority (Agent > Project > Domain > Global)
    /// 2. Score within each tier (success_count - failure_count)
    pub fn search_with_fallback(
        &self,
        tier: &TierPath,
        tool_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Pattern>> {
        let fallback_order = tier.search_fallback_order();
        let mut all_patterns = Vec::new();
        let mut remaining = limit;

        for tier_path in fallback_order {
            if remaining == 0 {
                break;
            }

            let tier_str = tier_path.to_path_string();

            let patterns = if let Some(tt) = tool_type {
                // Filter by both tier and tool type
                let mut stmt = self.conn.prepare(
                    r#"
                    SELECT id, pattern_hash, tool_type, command_category, context_query,
                           success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                           session_count, frequency_weight, session_ids
                    FROM patterns
                    WHERE tier_path = ?1 AND tool_type = ?2
                    ORDER BY (success_count - failure_count) DESC, success_count DESC
                    LIMIT ?3
                    "#,
                )?;

                let rows = stmt.query_map(params![tier_str, tt, remaining as i64], |row| {
                    Ok(Pattern {
                        id: row.get(0)?,
                        pattern_hash: row.get(1)?,
                        tool_type: row.get(2)?,
                        command_category: row.get(3)?,
                        context_query: row.get(4)?,
                        success_count: row.get(5)?,
                        failure_count: row.get(6)?,
                        embedding_id: row.get(7)?,
                        last_used: row.get(8).ok(),
                        access_count: row.get(9).unwrap_or(0),
                        tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                        session_count: row.get(11).unwrap_or(1),
                        frequency_weight: row.get(12).unwrap_or(1.0),
                        session_ids: row.get(13).ok(),
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            } else {
                // Filter by tier only
                let mut stmt = self.conn.prepare(
                    r#"
                    SELECT id, pattern_hash, tool_type, command_category, context_query,
                           success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                           session_count, frequency_weight, session_ids
                    FROM patterns
                    WHERE tier_path = ?1
                    ORDER BY (success_count - failure_count) DESC, success_count DESC
                    LIMIT ?2
                    "#,
                )?;

                let rows = stmt.query_map(params![tier_str, remaining as i64], |row| {
                    Ok(Pattern {
                        id: row.get(0)?,
                        pattern_hash: row.get(1)?,
                        tool_type: row.get(2)?,
                        command_category: row.get(3)?,
                        context_query: row.get(4)?,
                        success_count: row.get(5)?,
                        failure_count: row.get(6)?,
                        embedding_id: row.get(7)?,
                        last_used: row.get(8).ok(),
                        access_count: row.get(9).unwrap_or(0),
                        tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                        session_count: row.get(11).unwrap_or(1),
                        frequency_weight: row.get(12).unwrap_or(1.0),
                        session_ids: row.get(13).ok(),
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };

            let found = patterns.len();
            all_patterns.extend(patterns);
            remaining = remaining.saturating_sub(found);
        }

        Ok(all_patterns)
    }

    /// Get high-frequency instruction patterns for context injection
    ///
    /// Returns instruction patterns ordered by frequency weight (how often they're repeated).
    /// Patterns that appear across multiple sessions get higher weight.
    pub fn get_high_frequency_instructions(&self, limit: usize) -> Result<Vec<Pattern>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path,
                   session_count, frequency_weight, session_ids
            FROM patterns
            WHERE tool_type = 'instruction'
            ORDER BY frequency_weight DESC, session_count DESC
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
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                session_count: row.get(11).unwrap_or(1),
                frequency_weight: row.get(12).unwrap_or(1.0),
                session_ids: row.get(13).ok(),
            })
        })?;

        patterns.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Update instruction pattern with new session occurrence
    ///
    /// Tracks which sessions an instruction appears in, updates frequency weight,
    /// and increments usage counts. This is the key mechanism for detecting
    /// repeated user instructions across sessions.
    pub fn update_instruction_occurrence(&self, pattern_id: i64, session_id: &str) -> Result<()> {
        use std::collections::HashSet;

        // Get current session_ids
        let current_sessions: Option<String> = self.conn.query_row(
            "SELECT session_ids FROM patterns WHERE id = ?",
            params![pattern_id],
            |row| row.get(0),
        ).ok().flatten();

        let mut session_set: HashSet<String> = current_sessions
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let is_new_session = session_set.insert(session_id.to_string());

        if is_new_session {
            let new_session_count = session_set.len() as i64;
            let total_occurrences: i64 = self.conn.query_row(
                "SELECT success_count + failure_count FROM patterns WHERE id = ?",
                params![pattern_id],
                |row| row.get(0),
            ).unwrap_or(1) + 1;

            let new_weight = calculate_frequency_weight(new_session_count, total_occurrences);

            self.conn.execute(
                r#"
                UPDATE patterns
                SET session_count = ?,
                    frequency_weight = ?,
                    session_ids = ?,
                    success_count = success_count + 1,
                    last_used = CURRENT_TIMESTAMP
                WHERE id = ?
                "#,
                params![
                    new_session_count,
                    new_weight,
                    serde_json::to_string(&session_set).unwrap_or_default(),
                    pattern_id
                ],
            )?;
        } else {
            // Same session, just increment count
            self.conn.execute(
                "UPDATE patterns SET success_count = success_count + 1, last_used = CURRENT_TIMESTAMP WHERE id = ?",
                params![pattern_id],
            )?;
        }

        Ok(())
    }

    /// Insert instruction pattern with session tracking
    ///
    /// Inserts a new instruction pattern or updates an existing one if a similar
    /// instruction already exists (using a lower 70% similarity threshold).
    pub fn insert_instruction(&self, pattern: &Pattern, session_id: &str) -> Result<i64> {
        use crate::storage::calculate_similarity;
        use std::collections::HashSet;

        // Check for existing similar instruction patterns (lower threshold than regular patterns)
        let existing = self.get_by_tool("instruction", 50)?;

        for existing_pattern in existing {
            let similarity = calculate_similarity(&pattern.context_query, &existing_pattern.context_query);

            // Use 70% threshold for instructions (vs 90% for regular patterns)
            if similarity > 0.70 {
                debug!("Found similar instruction pattern {} (similarity: {:.2}), updating occurrence",
                       existing_pattern.id, similarity);

                // Update the existing pattern's session tracking
                self.update_instruction_occurrence(existing_pattern.id, session_id)?;
                return Ok(existing_pattern.id);
            }
        }

        // No similar pattern found, insert new one with initial session tracking
        let mut session_set = HashSet::new();
        session_set.insert(session_id.to_string());
        let session_ids_json = serde_json::to_string(&session_set).unwrap_or_default();

        self.conn.execute(
            r#"
            INSERT INTO patterns
            (pattern_hash, tool_type, command_category, context_query, success_count, failure_count,
             embedding_id, access_count, tier_path, session_count, frequency_weight, session_ids)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, 1.0, ?10)
            "#,
            params![
                pattern.pattern_hash,
                pattern.tool_type,
                pattern.command_category,
                pattern.context_query,
                pattern.success_count,
                pattern.failure_count,
                pattern.embedding_id,
                pattern.access_count,
                pattern.tier_path,
                session_ids_json
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }
}

/// Calculate frequency weight for an instruction pattern
///
/// Weight increases logarithmically with occurrences, and cross-session
/// appearances get a significant boost (50% for each order of magnitude
/// of sessions).
fn calculate_frequency_weight(session_count: i64, total_occurrences: i64) -> f64 {
    // Base weight increases logarithmically with occurrences
    let occurrence_factor = (1.0 + total_occurrences as f64).ln();

    // Cross-session bonus: instructions appearing in multiple sessions are more important
    let session_factor = if session_count > 1 {
        1.0 + (session_count as f64).ln() * 0.5
    } else {
        1.0
    };

    occurrence_factor * session_factor
}
