//! Reflexion Memory - Self-critique and learning from failures
//!
//! Implements the Reflexion pattern for agents to:
//! - Reflect on past actions and their outcomes
//! - Generate self-critiques when tasks fail
//! - Store and retrieve lessons learned
//! - Improve future performance through accumulated wisdom
//!
//! Based on "Reflexion: Language Agents with Verbal Reinforcement Learning"

#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single reflection entry - a lesson learned from experience
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: i64,
    /// The task or goal that was attempted
    pub task: String,
    /// What action was taken
    pub action: String,
    /// The outcome (success/failure/partial)
    pub outcome: ReflectionOutcome,
    /// Self-critique: what went wrong or could be improved
    pub critique: String,
    /// Lesson learned: what to do differently next time
    pub lesson: String,
    /// Confidence in this lesson (0.0 - 1.0)
    pub confidence: f32,
    /// How many times this lesson has been validated
    pub validation_count: i32,
    /// How many times this lesson has been contradicted
    pub contradiction_count: i32,
    /// Associated pattern IDs
    pub pattern_ids: Vec<i64>,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// When this reflection was created
    pub created_at: DateTime<Utc>,
    /// When this reflection was last used
    pub last_used: DateTime<Utc>,
}

/// Outcome of an action that triggered reflection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReflectionOutcome {
    Success,
    Failure,
    Partial,
    Timeout,
    Error(String),
}

impl ReflectionOutcome {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Partial => "partial",
            Self::Timeout => "timeout",
            Self::Error(_) => "error",
        }
    }

    pub fn from_str(s: &str, error_msg: Option<&str>) -> Self {
        match s {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "partial" => Self::Partial,
            "timeout" => Self::Timeout,
            "error" => Self::Error(error_msg.unwrap_or("").to_string()),
            _ => Self::Failure,
        }
    }
}

/// Input for generating a reflection
#[derive(Debug, Clone)]
pub struct ReflectionInput {
    pub task: String,
    pub action: String,
    pub outcome: ReflectionOutcome,
    pub context: String,
    pub error_message: Option<String>,
    pub pattern_ids: Vec<i64>,
    pub tags: Vec<String>,
}

/// Reflexion memory store
pub struct ReflexionStore {
    conn: Connection,
}

impl ReflexionStore {
    /// Create a new reflexion store
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Create an in-memory store for testing
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS reflections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task TEXT NOT NULL,
                action TEXT NOT NULL,
                outcome TEXT NOT NULL,
                outcome_error TEXT,
                critique TEXT NOT NULL,
                lesson TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                validation_count INTEGER NOT NULL DEFAULT 0,
                contradiction_count INTEGER NOT NULL DEFAULT 0,
                tags TEXT NOT NULL DEFAULT '[]',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                last_used DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS reflection_patterns (
                reflection_id INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                PRIMARY KEY (reflection_id, pattern_id),
                FOREIGN KEY (reflection_id) REFERENCES reflections(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_reflections_outcome ON reflections(outcome);
            CREATE INDEX IF NOT EXISTS idx_reflections_confidence ON reflections(confidence DESC);
            CREATE INDEX IF NOT EXISTS idx_reflections_task ON reflections(task);

            -- Full-text search on lessons
            CREATE VIRTUAL TABLE IF NOT EXISTS reflections_fts USING fts5(
                task, action, critique, lesson, tags,
                content='reflections',
                content_rowid='id'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS reflections_ai AFTER INSERT ON reflections BEGIN
                INSERT INTO reflections_fts(rowid, task, action, critique, lesson, tags)
                VALUES (new.id, new.task, new.action, new.critique, new.lesson, new.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS reflections_ad AFTER DELETE ON reflections BEGIN
                INSERT INTO reflections_fts(reflections_fts, rowid, task, action, critique, lesson, tags)
                VALUES ('delete', old.id, old.task, old.action, old.critique, old.lesson, old.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS reflections_au AFTER UPDATE ON reflections BEGIN
                INSERT INTO reflections_fts(reflections_fts, rowid, task, action, critique, lesson, tags)
                VALUES ('delete', old.id, old.task, old.action, old.critique, old.lesson, old.tags);
                INSERT INTO reflections_fts(rowid, task, action, critique, lesson, tags)
                VALUES (new.id, new.task, new.action, new.critique, new.lesson, new.tags);
            END;
            "#,
        )?;
        Ok(())
    }

    /// Add a new reflection
    pub fn add_reflection(
        &self,
        input: &ReflectionInput,
        critique: &str,
        lesson: &str,
        confidence: f32,
    ) -> Result<i64> {
        let outcome_str = input.outcome.as_str();
        let outcome_error = match &input.outcome {
            ReflectionOutcome::Error(msg) => Some(msg.as_str()),
            _ => None,
        };
        let tags_json = serde_json::to_string(&input.tags)?;

        self.conn.execute(
            r#"
            INSERT INTO reflections (task, action, outcome, outcome_error, critique, lesson, confidence, tags)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                input.task,
                input.action,
                outcome_str,
                outcome_error,
                critique,
                lesson,
                confidence,
                tags_json,
            ],
        )?;

        let reflection_id = self.conn.last_insert_rowid();

        // Link patterns
        for pattern_id in &input.pattern_ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO reflection_patterns (reflection_id, pattern_id) VALUES (?1, ?2)",
                params![reflection_id, pattern_id],
            )?;
        }

        Ok(reflection_id)
    }

    /// Generate a self-critique from an action outcome
    pub fn generate_critique(&self, input: &ReflectionInput) -> String {
        match &input.outcome {
            ReflectionOutcome::Success => {
                format!(
                    "Task '{}' completed successfully with action '{}'. The approach was effective.",
                    input.task, input.action
                )
            }
            ReflectionOutcome::Failure => {
                format!(
                    "Task '{}' failed when attempting '{}'. The approach did not achieve the goal. \
                     Context: {}",
                    input.task, input.action, input.context
                )
            }
            ReflectionOutcome::Partial => {
                format!(
                    "Task '{}' was partially completed with '{}'. Some objectives were met but \
                     the full goal was not achieved. Context: {}",
                    input.task, input.action, input.context
                )
            }
            ReflectionOutcome::Timeout => {
                format!(
                    "Task '{}' timed out during '{}'. The operation took too long to complete. \
                     Consider breaking into smaller steps or optimizing the approach.",
                    input.task, input.action
                )
            }
            ReflectionOutcome::Error(msg) => {
                format!(
                    "Task '{}' encountered an error during '{}': {}. \
                     This error should be handled or prevented in future attempts.",
                    input.task, input.action, msg
                )
            }
        }
    }

    /// Generate a lesson learned from the reflection
    pub fn generate_lesson(&self, input: &ReflectionInput, similar_reflections: &[Reflection]) -> String {
        let mut lesson = match &input.outcome {
            ReflectionOutcome::Success => {
                format!(
                    "When facing '{}', the action '{}' is effective. Continue using this approach.",
                    input.task, input.action
                )
            }
            ReflectionOutcome::Failure => {
                format!(
                    "Avoid using '{}' for tasks like '{}'. Consider alternative approaches.",
                    input.action, input.task
                )
            }
            ReflectionOutcome::Partial => {
                format!(
                    "The action '{}' partially works for '{}'. Augment with additional steps.",
                    input.action, input.task
                )
            }
            ReflectionOutcome::Timeout => {
                format!(
                    "Action '{}' is too slow for '{}'. Optimize or use incremental approach.",
                    input.action, input.task
                )
            }
            ReflectionOutcome::Error(msg) => {
                format!(
                    "Add error handling for '{}' when performing '{}'. Error type: {}",
                    input.task, input.action, msg
                )
            }
        };

        // Augment with insights from similar reflections
        if !similar_reflections.is_empty() {
            let successful: Vec<_> = similar_reflections
                .iter()
                .filter(|r| r.outcome == ReflectionOutcome::Success)
                .collect();

            if !successful.is_empty() {
                lesson.push_str(&format!(
                    " Previous successful approaches: {}",
                    successful
                        .iter()
                        .take(2)
                        .map(|r| r.action.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        lesson
    }

    /// Reflect on an action and store the lesson
    pub fn reflect(&self, input: ReflectionInput) -> Result<Reflection> {
        // Find similar past reflections
        let similar = self.search_similar(&input.task, 5)?;

        // Generate critique and lesson
        let critique = self.generate_critique(&input);
        let lesson = self.generate_lesson(&input, &similar);

        // Calculate initial confidence based on outcome and similar experiences
        let base_confidence = match input.outcome {
            ReflectionOutcome::Success => 0.8,
            ReflectionOutcome::Failure => 0.6,
            ReflectionOutcome::Partial => 0.5,
            ReflectionOutcome::Timeout => 0.4,
            ReflectionOutcome::Error(_) => 0.3,
        };

        // Boost confidence if similar reflections exist
        let confidence: f32 = if similar.is_empty() {
            base_confidence
        } else {
            (base_confidence + 0.1_f32).min(1.0_f32)
        };

        // Store the reflection
        let id = self.add_reflection(&input, &critique, &lesson, confidence)?;

        Ok(Reflection {
            id,
            task: input.task,
            action: input.action,
            outcome: input.outcome,
            critique,
            lesson,
            confidence,
            validation_count: 0,
            contradiction_count: 0,
            pattern_ids: input.pattern_ids,
            tags: input.tags,
            created_at: Utc::now(),
            last_used: Utc::now(),
        })
    }

    /// Validate a lesson (it proved correct again)
    pub fn validate_lesson(&self, reflection_id: i64) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE reflections
            SET validation_count = validation_count + 1,
                confidence = MIN(1.0, confidence + 0.05),
                last_used = CURRENT_TIMESTAMP
            WHERE id = ?1
            "#,
            params![reflection_id],
        )?;
        Ok(())
    }

    /// Contradict a lesson (it proved wrong)
    pub fn contradict_lesson(&self, reflection_id: i64) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE reflections
            SET contradiction_count = contradiction_count + 1,
                confidence = MAX(0.0, confidence - 0.1),
                last_used = CURRENT_TIMESTAMP
            WHERE id = ?1
            "#,
            params![reflection_id],
        )?;
        Ok(())
    }

    /// Search for reflections similar to a task
    pub fn search_similar(&self, task: &str, limit: usize) -> Result<Vec<Reflection>> {
        // Use FTS for semantic search
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.id, r.task, r.action, r.outcome, r.outcome_error, r.critique,
                   r.lesson, r.confidence, r.validation_count, r.contradiction_count,
                   r.tags, r.created_at, r.last_used
            FROM reflections r
            JOIN reflections_fts fts ON r.id = fts.rowid
            WHERE reflections_fts MATCH ?1
            ORDER BY r.confidence DESC, r.validation_count DESC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![task, limit as i64], |row| {
            self.row_to_reflection(row)
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Get lessons relevant to a task (high confidence, validated)
    pub fn get_lessons_for_task(&self, task: &str, min_confidence: f32) -> Result<Vec<Reflection>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.id, r.task, r.action, r.outcome, r.outcome_error, r.critique,
                   r.lesson, r.confidence, r.validation_count, r.contradiction_count,
                   r.tags, r.created_at, r.last_used
            FROM reflections r
            JOIN reflections_fts fts ON r.id = fts.rowid
            WHERE reflections_fts MATCH ?1
              AND r.confidence >= ?2
              AND r.validation_count > r.contradiction_count
            ORDER BY r.confidence DESC, r.validation_count DESC
            LIMIT 10
            "#,
        )?;

        let rows = stmt.query_map(params![task, min_confidence], |row| {
            self.row_to_reflection(row)
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Get failures to avoid for a task
    pub fn get_failures_to_avoid(&self, task: &str) -> Result<Vec<Reflection>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.id, r.task, r.action, r.outcome, r.outcome_error, r.critique,
                   r.lesson, r.confidence, r.validation_count, r.contradiction_count,
                   r.tags, r.created_at, r.last_used
            FROM reflections r
            JOIN reflections_fts fts ON r.id = fts.rowid
            WHERE reflections_fts MATCH ?1
              AND r.outcome IN ('failure', 'error', 'timeout')
              AND r.confidence >= 0.5
            ORDER BY r.confidence DESC
            LIMIT 5
            "#,
        )?;

        let rows = stmt.query_map(params![task], |row| {
            self.row_to_reflection(row)
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Get all reflections for a pattern
    pub fn get_reflections_for_pattern(&self, pattern_id: i64) -> Result<Vec<Reflection>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT r.id, r.task, r.action, r.outcome, r.outcome_error, r.critique,
                   r.lesson, r.confidence, r.validation_count, r.contradiction_count,
                   r.tags, r.created_at, r.last_used
            FROM reflections r
            JOIN reflection_patterns rp ON r.id = rp.reflection_id
            WHERE rp.pattern_id = ?1
            ORDER BY r.created_at DESC
            "#,
        )?;

        let rows = stmt.query_map(params![pattern_id], |row| {
            self.row_to_reflection(row)
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Get high-value lessons (validated, high confidence)
    pub fn get_high_value_lessons(&self, limit: usize) -> Result<Vec<Reflection>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, task, action, outcome, outcome_error, critique, lesson,
                   confidence, validation_count, contradiction_count, tags,
                   created_at, last_used
            FROM reflections
            WHERE validation_count > contradiction_count
              AND confidence >= 0.7
            ORDER BY (validation_count - contradiction_count) DESC, confidence DESC
            LIMIT ?1
            "#,
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            self.row_to_reflection(row)
        })?;

        let mut results = Vec::new();
        for row in rows.flatten() {
            results.push(row);
        }
        Ok(results)
    }

    /// Get statistics
    pub fn stats(&self) -> Result<ReflexionStats> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reflections",
            [],
            |row| row.get(0),
        )?;

        let by_outcome: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT outcome, COUNT(*) FROM reflections GROUP BY outcome"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.flatten().collect()
        };

        let avg_confidence: f64 = self.conn.query_row(
            "SELECT COALESCE(AVG(confidence), 0.0) FROM reflections",
            [],
            |row| row.get(0),
        )?;

        let validated: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reflections WHERE validation_count > contradiction_count",
            [],
            |row| row.get(0),
        )?;

        let high_confidence: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reflections WHERE confidence >= 0.7",
            [],
            |row| row.get(0),
        )?;

        Ok(ReflexionStats {
            total_reflections: total,
            by_outcome,
            avg_confidence,
            validated_lessons: validated,
            high_confidence_lessons: high_confidence,
        })
    }

    /// Prune low-value reflections
    pub fn prune_low_value(&self, max_contradictions: i32, min_confidence: f32) -> Result<usize> {
        let deleted = self.conn.execute(
            r#"
            DELETE FROM reflections
            WHERE contradiction_count >= ?1
              AND confidence < ?2
              AND validation_count = 0
            "#,
            params![max_contradictions, min_confidence],
        )?;
        Ok(deleted)
    }

    fn row_to_reflection(&self, row: &rusqlite::Row) -> rusqlite::Result<Reflection> {
        let outcome_str: String = row.get(3)?;
        let outcome_error: Option<String> = row.get(4)?;
        let tags_json: String = row.get(10)?;
        let created_at: String = row.get(11)?;
        let last_used: String = row.get(12)?;

        let outcome = ReflectionOutcome::from_str(&outcome_str, outcome_error.as_deref());
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

        // Get pattern IDs for this reflection
        let reflection_id: i64 = row.get(0)?;
        let pattern_ids = Vec::new(); // Loaded separately if needed

        Ok(Reflection {
            id: reflection_id,
            task: row.get(1)?,
            action: row.get(2)?,
            outcome,
            critique: row.get(5)?,
            lesson: row.get(6)?,
            confidence: row.get(7)?,
            validation_count: row.get(8)?,
            contradiction_count: row.get(9)?,
            pattern_ids,
            tags,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            last_used: DateTime::parse_from_rfc3339(&last_used)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    }
}

/// Reflexion statistics
#[derive(Debug, Clone)]
pub struct ReflexionStats {
    pub total_reflections: i64,
    pub by_outcome: Vec<(String, i64)>,
    pub avg_confidence: f64,
    pub validated_lessons: i64,
    pub high_confidence_lessons: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_reflection() {
        let store = ReflexionStore::in_memory().unwrap();

        let input = ReflectionInput {
            task: "Fix compilation error".to_string(),
            action: "Added missing import".to_string(),
            outcome: ReflectionOutcome::Success,
            context: "Rust project".to_string(),
            error_message: None,
            pattern_ids: vec![1, 2],
            tags: vec!["rust".to_string(), "imports".to_string()],
        };

        let reflection = store.reflect(input).unwrap();

        assert!(reflection.id > 0);
        assert_eq!(reflection.outcome, ReflectionOutcome::Success);
        assert!(reflection.confidence > 0.5);
    }

    #[test]
    fn test_failure_reflection() {
        let store = ReflexionStore::in_memory().unwrap();

        let input = ReflectionInput {
            task: "Optimize database query".to_string(),
            action: "Added index on wrong column".to_string(),
            outcome: ReflectionOutcome::Failure,
            context: "PostgreSQL database".to_string(),
            error_message: None,
            pattern_ids: vec![],
            tags: vec!["database".to_string()],
        };

        let reflection = store.reflect(input).unwrap();

        assert!(reflection.critique.contains("failed"));
        assert!(reflection.lesson.contains("Avoid"));
    }

    #[test]
    fn test_validation() {
        let store = ReflexionStore::in_memory().unwrap();

        let input = ReflectionInput {
            task: "Test task".to_string(),
            action: "Test action".to_string(),
            outcome: ReflectionOutcome::Success,
            context: "".to_string(),
            error_message: None,
            pattern_ids: vec![],
            tags: vec![],
        };

        let reflection = store.reflect(input).unwrap();
        let initial_confidence = reflection.confidence;

        store.validate_lesson(reflection.id).unwrap();
        store.validate_lesson(reflection.id).unwrap();

        // Check that confidence increased
        let lessons = store.get_high_value_lessons(10).unwrap();
        let updated = lessons.iter().find(|r| r.id == reflection.id).unwrap();

        assert_eq!(updated.validation_count, 2);
        assert!(updated.confidence > initial_confidence);
    }

    #[test]
    fn test_contradiction() {
        let store = ReflexionStore::in_memory().unwrap();

        let input = ReflectionInput {
            task: "Test task".to_string(),
            action: "Test action".to_string(),
            outcome: ReflectionOutcome::Success,
            context: "".to_string(),
            error_message: None,
            pattern_ids: vec![],
            tags: vec![],
        };

        let reflection = store.reflect(input).unwrap();
        let initial_confidence = reflection.confidence;

        store.contradict_lesson(reflection.id).unwrap();

        // Verify contradiction was recorded (query the DB directly)
        let contradiction_count: i32 = store.conn.query_row(
            "SELECT contradiction_count FROM reflections WHERE id = ?1",
            params![reflection.id],
            |row| row.get(0),
        ).unwrap();

        assert_eq!(contradiction_count, 1);
    }

    #[test]
    fn test_error_reflection() {
        let store = ReflexionStore::in_memory().unwrap();

        let input = ReflectionInput {
            task: "Connect to API".to_string(),
            action: "Called endpoint".to_string(),
            outcome: ReflectionOutcome::Error("Connection timeout".to_string()),
            context: "Network issue".to_string(),
            error_message: Some("Connection timeout".to_string()),
            pattern_ids: vec![],
            tags: vec!["api".to_string(), "network".to_string()],
        };

        let reflection = store.reflect(input).unwrap();

        assert!(reflection.critique.contains("error"));
        assert!(reflection.lesson.contains("error handling"));
    }

    #[test]
    fn test_stats() {
        let store = ReflexionStore::in_memory().unwrap();

        // Add various reflections
        for outcome in [ReflectionOutcome::Success, ReflectionOutcome::Failure, ReflectionOutcome::Partial] {
            let input = ReflectionInput {
                task: format!("Task {:?}", outcome),
                action: "Action".to_string(),
                outcome,
                context: "".to_string(),
                error_message: None,
                pattern_ids: vec![],
                tags: vec![],
            };
            store.reflect(input).unwrap();
        }

        let stats = store.stats().unwrap();

        assert_eq!(stats.total_reflections, 3);
        assert!(!stats.by_outcome.is_empty());
    }
}
