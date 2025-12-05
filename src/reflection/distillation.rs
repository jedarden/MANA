//! Memory distillation for applying reflection verdicts
//!
//! Distillation takes reflection verdicts and applies them
//! to update pattern scores in the ReasoningBank.

use anyhow::Result;
use rusqlite::{Connection, params};
use tracing::{debug, info};

use super::{ReflectionConfig, ReflectionVerdict};

/// Memory distiller for applying verdicts to patterns
pub struct MemoryDistiller {
    config: ReflectionConfig,
}

impl MemoryDistiller {
    /// Create a new memory distiller
    pub fn new(config: ReflectionConfig) -> Self {
        Self { config }
    }

    /// Apply verdicts to update pattern scores
    ///
    /// Returns the number of patterns updated
    pub fn distill(&self, conn: &Connection, verdicts: &[ReflectionVerdict]) -> Result<usize> {
        let mut updated = 0;

        for verdict in verdicts {
            if let Some(pattern_id) = verdict.pattern_id {
                // Apply score change to pattern
                let change = self.calculate_score_change(verdict);

                if change != 0 {
                    let rows = if change > 0 {
                        conn.execute(
                            "UPDATE patterns SET success_count = success_count + ?1 WHERE id = ?2",
                            params![change, pattern_id],
                        )?
                    } else {
                        conn.execute(
                            "UPDATE patterns SET failure_count = failure_count + ?1 WHERE id = ?2",
                            params![-change, pattern_id],
                        )?
                    };

                    if rows > 0 {
                        updated += 1;
                        debug!(
                            "Updated pattern {} with score change {} (verdict: {:?})",
                            pattern_id,
                            change,
                            verdict.verdict.category
                        );
                    }
                }
            }

            // Store the verdict in reflection_verdicts table
            self.store_verdict(conn, verdict)?;
        }

        info!("Distilled {} verdicts, updated {} patterns", verdicts.len(), updated);
        Ok(updated)
    }

    /// Calculate the score change for a verdict
    fn calculate_score_change(&self, verdict: &ReflectionVerdict) -> i32 {
        let base = verdict.score_impact();

        // Apply confidence scaling
        let scaled = (base as f32 * verdict.confidence).round() as i32;

        // Clamp to configured limits
        scaled.clamp(self.config.max_penalty, self.config.max_boost)
    }

    /// Store a verdict in the database
    fn store_verdict(&self, conn: &Connection, verdict: &ReflectionVerdict) -> Result<()> {
        conn.execute(
            "INSERT INTO reflection_verdicts
             (trajectory_hash, pattern_id, verdict, confidence, root_cause, suggested_improvement, context_mismatch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                verdict.trajectory_hash,
                verdict.pattern_id,
                verdict.category_str(),
                verdict.confidence,
                verdict.verdict.root_cause,
                verdict.verdict.suggested_improvement,
                verdict.context_mismatch as i32,
            ],
        )?;

        Ok(())
    }

    /// Get recent verdicts for a pattern
    pub fn get_pattern_verdicts(conn: &Connection, pattern_id: i64, limit: usize) -> Result<Vec<VerdictSummary>> {
        let mut stmt = conn.prepare(
            "SELECT verdict, confidence, root_cause, created_at
             FROM reflection_verdicts
             WHERE pattern_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
        )?;

        let verdicts = stmt.query_map(params![pattern_id, limit as i64], |row| {
            Ok(VerdictSummary {
                category: row.get::<_, String>(0)?,
                confidence: row.get(1)?,
                root_cause: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

        Ok(verdicts)
    }

    /// Aggregate verdict statistics for a pattern
    pub fn get_pattern_stats(conn: &Connection, pattern_id: i64) -> Result<VerdictStats> {
        let stats: VerdictStats = conn.query_row(
            "SELECT
                COUNT(*) as total,
                COALESCE(SUM(CASE WHEN verdict = 'EFFECTIVE' THEN 1 ELSE 0 END), 0) as effective,
                COALESCE(SUM(CASE WHEN verdict = 'INEFFECTIVE' THEN 1 ELSE 0 END), 0) as ineffective,
                COALESCE(SUM(CASE WHEN verdict = 'HARMFUL' THEN 1 ELSE 0 END), 0) as harmful,
                COALESCE(SUM(CASE WHEN verdict = 'NEUTRAL' THEN 1 ELSE 0 END), 0) as neutral,
                COALESCE(AVG(confidence), 0.0) as avg_confidence
             FROM reflection_verdicts
             WHERE pattern_id = ?1",
            params![pattern_id],
            |row| {
                Ok(VerdictStats {
                    total: row.get(0)?,
                    effective: row.get(1)?,
                    ineffective: row.get(2)?,
                    harmful: row.get(3)?,
                    neutral: row.get(4)?,
                    avg_confidence: row.get(5)?,
                })
            },
        )?;

        Ok(stats)
    }

    /// Identify patterns that should be demoted or removed
    #[allow(dead_code)] // Reserved for future automated pattern pruning
    pub fn identify_demotions(conn: &Connection, threshold: f32) -> Result<Vec<i64>> {
        // Find patterns where harmful verdicts significantly outweigh effective ones
        let mut stmt = conn.prepare(
            "SELECT pattern_id,
                    SUM(CASE WHEN verdict = 'EFFECTIVE' THEN confidence ELSE 0 END) as eff_score,
                    SUM(CASE WHEN verdict = 'HARMFUL' THEN confidence ELSE 0 END) as harm_score
             FROM reflection_verdicts
             WHERE pattern_id IS NOT NULL
             GROUP BY pattern_id
             HAVING harm_score > eff_score * ?1"
        )?;

        let patterns: Vec<i64> = stmt
            .query_map(params![threshold], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(patterns)
    }
}

/// Summary of a single verdict
#[derive(Debug, Clone)]
pub struct VerdictSummary {
    pub category: String,
    pub confidence: f32,
    pub root_cause: Option<String>,
    #[allow(dead_code)] // Reserved for future use in detailed verdict display
    pub created_at: String,
}

/// Aggregated verdict statistics
#[derive(Debug, Clone, Default)]
pub struct VerdictStats {
    pub total: i64,
    pub effective: i64,
    #[allow(dead_code)] // Reserved for future use in expanded statistics display
    pub ineffective: i64,
    pub harmful: i64,
    #[allow(dead_code)] // Reserved for future use in expanded statistics display
    pub neutral: i64,
    pub avg_confidence: f64,
}

impl VerdictStats {
    /// Calculate effectiveness ratio
    pub fn effectiveness_ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.effective as f64 / self.total as f64
    }

    /// Calculate harm ratio
    pub fn harm_ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.harmful as f64 / self.total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection::verdict::Verdict;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();

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

            CREATE TABLE reflection_verdicts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trajectory_hash TEXT NOT NULL,
                pattern_id INTEGER,
                verdict TEXT NOT NULL,
                confidence REAL NOT NULL,
                root_cause TEXT,
                suggested_improvement TEXT,
                context_mismatch INTEGER DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            INSERT INTO patterns (id, pattern_hash, tool_type, context_query, success_count, failure_count)
            VALUES (1, 'hash1', 'Edit', 'test context', 5, 2);
            "#,
        ).unwrap();

        conn
    }

    #[test]
    fn test_distill_effective_verdict() {
        let conn = setup_test_db();
        let config = ReflectionConfig::default();
        let distiller = MemoryDistiller::new(config);

        let verdict = ReflectionVerdict::new(
            "traj_hash".into(),
            Some(1),
            Verdict::effective(0.9, 5),
        );

        let updated = distiller.distill(&conn, &[verdict]).unwrap();
        assert_eq!(updated, 1);

        // Check pattern was updated
        let success: i64 = conn.query_row(
            "SELECT success_count FROM patterns WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();

        // Should have added ~4-5 to success count (5 * 0.9 rounded)
        assert!(success > 5);
    }

    #[test]
    fn test_distill_harmful_verdict() {
        let conn = setup_test_db();
        let config = ReflectionConfig::default();
        let distiller = MemoryDistiller::new(config);

        let verdict = ReflectionVerdict::new(
            "traj_hash".into(),
            Some(1),
            Verdict::harmful(0.8, -5, "Test failure".into()),
        );

        let updated = distiller.distill(&conn, &[verdict]).unwrap();
        assert_eq!(updated, 1);

        // Check pattern failure count was updated
        let failure: i64 = conn.query_row(
            "SELECT failure_count FROM patterns WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();

        // Should have added ~4 to failure count (5 * 0.8 = 4)
        assert!(failure > 2);
    }

    #[test]
    fn test_verdict_stored() {
        let conn = setup_test_db();
        let config = ReflectionConfig::default();
        let distiller = MemoryDistiller::new(config);

        let verdict = ReflectionVerdict::new(
            "traj_hash".into(),
            Some(1),
            Verdict::effective(0.9, 5),
        );

        distiller.distill(&conn, &[verdict]).unwrap();

        // Check verdict was stored
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM reflection_verdicts WHERE pattern_id = 1",
            [],
            |row| row.get(0),
        ).unwrap();

        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_pattern_stats() {
        let conn = setup_test_db();

        // Insert some test verdicts
        conn.execute_batch(
            r#"
            INSERT INTO reflection_verdicts (trajectory_hash, pattern_id, verdict, confidence)
            VALUES
                ('h1', 1, 'EFFECTIVE', 0.9),
                ('h2', 1, 'EFFECTIVE', 0.8),
                ('h3', 1, 'HARMFUL', 0.7);
            "#,
        ).unwrap();

        let stats = MemoryDistiller::get_pattern_stats(&conn, 1).unwrap();

        assert_eq!(stats.total, 3);
        assert_eq!(stats.effective, 2);
        assert_eq!(stats.harmful, 1);
        assert!(stats.effectiveness_ratio() > 0.5);
    }
}
