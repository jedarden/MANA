//! ReasoningBank - Structured reasoning chain storage
//!
//! Inspired by AgentDB's ReasoningBank, this module stores structured
//! reasoning chains that capture the thought process behind successful
//! operations. This enables:
//!
//! - Learning from reasoning patterns, not just outcomes
//! - Identifying common problem-solving strategies
//! - Providing contextual reasoning hints during injection
//!
//! Architecture:
//! - ReasoningChain: A sequence of reasoning steps leading to an action
//! - ReasoningStep: Individual thought/observation/action tuples
//! - ReasoningStore: SQLite-backed storage with indexing

#![allow(dead_code)] // New API - will be integrated in future versions

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single step in a reasoning chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// Step number in the chain (0-indexed)
    pub step_number: i32,
    /// Type of step: "thought", "observation", "action", "reflection"
    pub step_type: String,
    /// The content of this step
    pub content: String,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
}

/// A complete reasoning chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningChain {
    /// Unique identifier
    pub id: i64,
    /// Pattern ID this reasoning is associated with (optional)
    pub pattern_id: Option<i64>,
    /// The task or query that initiated this reasoning
    pub task: String,
    /// Tool type used (Bash, Edit, etc.)
    pub tool_type: String,
    /// The final outcome (success/failure)
    pub outcome: String,
    /// Individual reasoning steps
    pub steps: Vec<ReasoningStep>,
    /// Summary of the reasoning approach
    pub summary: String,
    /// Number of times this reasoning pattern was successful
    pub success_count: i64,
    /// Number of times this reasoning pattern failed
    pub failure_count: i64,
    /// Creation timestamp
    pub created_at: String,
}

impl ReasoningChain {
    /// Calculate success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            0.5 // Unknown, default to neutral
        } else {
            self.success_count as f64 / total as f64
        }
    }

    /// Calculate effectiveness score
    pub fn effectiveness_score(&self) -> f64 {
        let rate = self.success_rate();
        let confidence = (self.success_count + self.failure_count) as f64 / 10.0;
        rate * confidence.min(1.0)
    }
}

/// ReasoningBank storage backed by SQLite
pub struct ReasoningStore {
    conn: Connection,
}

impl ReasoningStore {
    /// Open or create a reasoning store
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialize the database schema
    fn init_schema(&self) -> Result<()> {
        // Disable foreign keys during schema creation (pattern_id is optional)
        self.conn.execute("PRAGMA foreign_keys = OFF", [])?;

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS reasoning_chains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern_id INTEGER,
                task TEXT NOT NULL,
                tool_type TEXT NOT NULL,
                outcome TEXT NOT NULL,
                summary TEXT NOT NULL,
                success_count INTEGER DEFAULT 1,
                failure_count INTEGER DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS reasoning_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chain_id INTEGER NOT NULL,
                step_number INTEGER NOT NULL,
                step_type TEXT NOT NULL,
                content TEXT NOT NULL,
                confidence REAL DEFAULT 1.0,
                FOREIGN KEY (chain_id) REFERENCES reasoning_chains(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_reasoning_tool ON reasoning_chains(tool_type);
            CREATE INDEX IF NOT EXISTS idx_reasoning_pattern ON reasoning_chains(pattern_id);
            CREATE INDEX IF NOT EXISTS idx_reasoning_outcome ON reasoning_chains(outcome);
            CREATE INDEX IF NOT EXISTS idx_steps_chain ON reasoning_steps(chain_id);
            "#,
        )?;
        Ok(())
    }

    /// Store a new reasoning chain
    pub fn store_chain(&self, chain: &ReasoningChain) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO reasoning_chains (pattern_id, task, tool_type, outcome, summary, success_count, failure_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                chain.pattern_id,
                chain.task,
                chain.tool_type,
                chain.outcome,
                chain.summary,
                chain.success_count,
                chain.failure_count
            ],
        )?;

        let chain_id = self.conn.last_insert_rowid();

        // Store steps
        for step in &chain.steps {
            self.conn.execute(
                r#"
                INSERT INTO reasoning_steps (chain_id, step_number, step_type, content, confidence)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    chain_id,
                    step.step_number,
                    step.step_type,
                    step.content,
                    step.confidence
                ],
            )?;
        }

        Ok(chain_id)
    }

    /// Get reasoning chain by ID
    pub fn get_chain(&self, id: i64) -> Result<Option<ReasoningChain>> {
        let chain: Option<(i64, Option<i64>, String, String, String, String, i64, i64, String)> =
            self.conn.query_row(
                r#"
                SELECT id, pattern_id, task, tool_type, outcome, summary,
                       success_count, failure_count, created_at
                FROM reasoning_chains WHERE id = ?1
                "#,
                params![id],
                |row| Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?
                )),
            ).ok();

        let (id, pattern_id, task, tool_type, outcome, summary, success_count, failure_count, created_at) =
            match chain {
                Some(c) => c,
                None => return Ok(None),
            };

        // Get steps
        let steps = self.get_steps(id)?;

        Ok(Some(ReasoningChain {
            id,
            pattern_id,
            task,
            tool_type,
            outcome,
            steps,
            summary,
            success_count,
            failure_count,
            created_at,
        }))
    }

    /// Get steps for a chain
    fn get_steps(&self, chain_id: i64) -> Result<Vec<ReasoningStep>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT step_number, step_type, content, confidence
            FROM reasoning_steps
            WHERE chain_id = ?1
            ORDER BY step_number
            "#,
        )?;

        let steps = stmt.query_map(params![chain_id], |row| {
            Ok(ReasoningStep {
                step_number: row.get(0)?,
                step_type: row.get(1)?,
                content: row.get(2)?,
                confidence: row.get(3)?,
            })
        })?;

        steps.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find similar reasoning chains for a task
    pub fn find_similar(&self, task: &str, tool_type: &str, limit: usize) -> Result<Vec<ReasoningChain>> {
        // Simple keyword-based search for now
        // TODO: Add vector similarity search using embeddings
        let keywords: Vec<&str> = task
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| s.len() > 2)
            .take(5)
            .collect();

        let mut chains = Vec::new();

        for keyword in keywords {
            let pattern = format!("%{}%", keyword);
            let mut stmt = self.conn.prepare(
                r#"
                SELECT id, pattern_id, task, tool_type, outcome, summary,
                       success_count, failure_count, created_at
                FROM reasoning_chains
                WHERE tool_type = ?1 AND (task LIKE ?2 OR summary LIKE ?2)
                AND outcome = 'success'
                ORDER BY success_count DESC
                LIMIT ?3
                "#,
            )?;

            let rows = stmt.query_map(params![tool_type, pattern, limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?;

            for row in rows.flatten() {
                let (id, pattern_id, task, tool_type, outcome, summary, success_count, failure_count, created_at) = row;
                let steps = self.get_steps(id)?;

                chains.push(ReasoningChain {
                    id,
                    pattern_id,
                    task,
                    tool_type,
                    outcome,
                    steps,
                    summary,
                    success_count,
                    failure_count,
                    created_at,
                });
            }
        }

        // Deduplicate by ID
        chains.sort_by_key(|c| c.id);
        chains.dedup_by_key(|c| c.id);

        // Sort by effectiveness
        chains.sort_by(|a, b| {
            b.effectiveness_score()
                .partial_cmp(&a.effectiveness_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        chains.truncate(limit);
        Ok(chains)
    }

    /// Get top reasoning chains by effectiveness
    pub fn get_top_chains(&self, tool_type: Option<&str>, limit: usize) -> Result<Vec<ReasoningChain>> {
        let query = match tool_type {
            Some(_) => r#"
                SELECT id, pattern_id, task, tool_type, outcome, summary,
                       success_count, failure_count, created_at
                FROM reasoning_chains
                WHERE tool_type = ?1 AND outcome = 'success'
                ORDER BY (success_count * 1.0 / (success_count + failure_count + 1)) DESC,
                         success_count DESC
                LIMIT ?2
            "#,
            None => r#"
                SELECT id, pattern_id, task, tool_type, outcome, summary,
                       success_count, failure_count, created_at
                FROM reasoning_chains
                WHERE outcome = 'success'
                ORDER BY (success_count * 1.0 / (success_count + failure_count + 1)) DESC,
                         success_count DESC
                LIMIT ?2
            "#,
        };

        let mut stmt = self.conn.prepare(query)?;

        let rows: Vec<(i64, Option<i64>, String, String, String, String, i64, i64, String)> =
            match tool_type {
                Some(t) => stmt.query_map(params![t, limit as i64], |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                        row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?
                    ))
                })?.filter_map(|r| r.ok()).collect(),
                None => stmt.query_map(params![limit as i64], |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                        row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?
                    ))
                })?.filter_map(|r| r.ok()).collect(),
            };

        let mut chains = Vec::new();
        for (id, pattern_id, task, tool_type, outcome, summary, success_count, failure_count, created_at) in rows {
            let steps = self.get_steps(id)?;
            chains.push(ReasoningChain {
                id,
                pattern_id,
                task,
                tool_type,
                outcome,
                steps,
                summary,
                success_count,
                failure_count,
                created_at,
            });
        }

        Ok(chains)
    }

    /// Update outcome counts for a chain
    pub fn update_outcome(&self, chain_id: i64, success: bool) -> Result<()> {
        let column = if success { "success_count" } else { "failure_count" };
        self.conn.execute(
            &format!(
                "UPDATE reasoning_chains SET {} = {} + 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                column, column
            ),
            params![chain_id],
        )?;
        Ok(())
    }

    /// Link a reasoning chain to a pattern
    pub fn link_to_pattern(&self, chain_id: i64, pattern_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE reasoning_chains SET pattern_id = ?1 WHERE id = ?2",
            params![pattern_id, chain_id],
        )?;
        Ok(())
    }

    /// Get chains for a pattern
    pub fn get_chains_for_pattern(&self, pattern_id: i64) -> Result<Vec<ReasoningChain>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_id, task, tool_type, outcome, summary,
                   success_count, failure_count, created_at
            FROM reasoning_chains
            WHERE pattern_id = ?1
            ORDER BY success_count DESC
            "#,
        )?;

        let rows = stmt.query_map(params![pattern_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut chains = Vec::new();
        for row in rows.flatten() {
            let (id, pattern_id, task, tool_type, outcome, summary, success_count, failure_count, created_at) = row;
            let steps = self.get_steps(id)?;
            chains.push(ReasoningChain {
                id,
                pattern_id,
                task,
                tool_type,
                outcome,
                steps,
                summary,
                success_count,
                failure_count,
                created_at,
            });
        }

        Ok(chains)
    }

    /// Get total chain count
    pub fn count(&self) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM reasoning_chains",
            [],
            |row| row.get(0),
        ).map_err(Into::into)
    }

    /// Get statistics
    pub fn stats(&self) -> Result<ReasoningStats> {
        let total_chains: i64 = self.count()?;

        let successful_chains: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reasoning_chains WHERE outcome = 'success'",
            [],
            |row| row.get(0),
        )?;

        let total_steps: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reasoning_steps",
            [],
            |row| row.get(0),
        )?;

        let avg_steps: f64 = if total_chains > 0 {
            total_steps as f64 / total_chains as f64
        } else {
            0.0
        };

        Ok(ReasoningStats {
            total_chains,
            successful_chains,
            total_steps,
            avg_steps_per_chain: avg_steps,
        })
    }

    /// Extract reasoning chain from Claude conversation
    /// This parses a conversation to identify reasoning patterns
    pub fn extract_from_conversation(&self, conversation: &str, tool_type: &str, success: bool) -> Result<Option<i64>> {
        // Simple extraction: look for thinking patterns
        let mut steps = Vec::new();
        let mut step_num = 0;

        // Look for common reasoning patterns
        let lines: Vec<&str> = conversation.lines().collect();

        for line in &lines {
            let line = line.trim();

            // Detect thought patterns
            if line.starts_with("I'll") || line.starts_with("Let me") || line.starts_with("First,") {
                steps.push(ReasoningStep {
                    step_number: step_num,
                    step_type: "thought".to_string(),
                    content: line.to_string(),
                    confidence: 0.8,
                });
                step_num += 1;
            }

            // Detect observations
            if line.contains("found") || line.contains("noticed") || line.contains("shows") {
                steps.push(ReasoningStep {
                    step_number: step_num,
                    step_type: "observation".to_string(),
                    content: line.to_string(),
                    confidence: 0.9,
                });
                step_num += 1;
            }

            // Detect actions
            if line.contains("Running") || line.contains("Executing") || line.contains("Creating") {
                steps.push(ReasoningStep {
                    step_number: step_num,
                    step_type: "action".to_string(),
                    content: line.to_string(),
                    confidence: 1.0,
                });
                step_num += 1;
            }
        }

        // Only store if we found meaningful reasoning
        if steps.len() < 2 {
            return Ok(None);
        }

        // Create summary from first thought
        let summary = steps.iter()
            .find(|s| s.step_type == "thought")
            .map(|s| s.content.clone())
            .unwrap_or_else(|| "Automated reasoning chain".to_string());

        // Extract task from conversation start
        let task = lines.iter()
            .find(|l| !l.is_empty())
            .map(|l| l.to_string())
            .unwrap_or_else(|| "Unknown task".to_string());

        let chain = ReasoningChain {
            id: 0, // Will be set on insert
            pattern_id: None,
            task,
            tool_type: tool_type.to_string(),
            outcome: if success { "success" } else { "failure" }.to_string(),
            steps,
            summary,
            success_count: if success { 1 } else { 0 },
            failure_count: if success { 0 } else { 1 },
            created_at: String::new(), // Will be set on insert
        };

        let id = self.store_chain(&chain)?;
        Ok(Some(id))
    }
}

/// Statistics about reasoning chains
#[derive(Debug, Clone)]
pub struct ReasoningStats {
    pub total_chains: i64,
    pub successful_chains: i64,
    pub total_steps: i64,
    pub avg_steps_per_chain: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_chain() -> ReasoningChain {
        ReasoningChain {
            id: 0,
            pattern_id: None,
            task: "Fix compilation error in main.rs".to_string(),
            tool_type: "Edit".to_string(),
            outcome: "success".to_string(),
            steps: vec![
                ReasoningStep {
                    step_number: 0,
                    step_type: "thought".to_string(),
                    content: "I need to identify the type mismatch".to_string(),
                    confidence: 0.9,
                },
                ReasoningStep {
                    step_number: 1,
                    step_type: "observation".to_string(),
                    content: "The error shows String expected but &str provided".to_string(),
                    confidence: 1.0,
                },
                ReasoningStep {
                    step_number: 2,
                    step_type: "action".to_string(),
                    content: "Add .to_string() to convert &str to String".to_string(),
                    confidence: 1.0,
                },
            ],
            summary: "Fix type mismatch by converting &str to String".to_string(),
            success_count: 1,
            failure_count: 0,
            created_at: String::new(),
        }
    }

    #[test]
    fn test_store_and_retrieve() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let store = ReasoningStore::open(&db_path).unwrap();

        let chain = create_test_chain();
        let id = store.store_chain(&chain).unwrap();
        assert!(id > 0);

        let retrieved = store.get_chain(id).unwrap().unwrap();
        assert_eq!(retrieved.task, chain.task);
        assert_eq!(retrieved.steps.len(), 3);
    }

    #[test]
    fn test_find_similar() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let store = ReasoningStore::open(&db_path).unwrap();

        let chain = create_test_chain();
        store.store_chain(&chain).unwrap();

        let similar = store.find_similar("compilation error", "Edit", 5).unwrap();
        assert!(!similar.is_empty());
    }

    #[test]
    fn test_update_outcome() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let store = ReasoningStore::open(&db_path).unwrap();

        let chain = create_test_chain();
        let id = store.store_chain(&chain).unwrap();

        store.update_outcome(id, true).unwrap();
        store.update_outcome(id, false).unwrap();

        let updated = store.get_chain(id).unwrap().unwrap();
        assert_eq!(updated.success_count, 2);
        assert_eq!(updated.failure_count, 1);
    }

    #[test]
    fn test_stats() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let store = ReasoningStore::open(&db_path).unwrap();

        let chain = create_test_chain();
        store.store_chain(&chain).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.total_chains, 1);
        assert_eq!(stats.successful_chains, 1);
        assert_eq!(stats.total_steps, 3);
        assert!((stats.avg_steps_per_chain - 3.0).abs() < 0.01);
    }
}
