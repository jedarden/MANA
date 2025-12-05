//! Reflection module for analyzing pattern effectiveness
//!
//! Reflection enables MANA to learn *why* patterns succeed or fail,
//! not just *that* they did. It analyzes trajectories, produces verdicts,
//! and distills learnings into pattern updates.
//!
//! ## Triggers
//! - Data-driven: >= 50 new trajectories accumulated
//! - Time-driven: Every 4 hours (catch edge cases)
//! - Manual: `mana reflect` command
//!
//! ## Pipeline
//! 1. Trajectory Collection: Gather completed trajectories
//! 2. Verdict Judgment: Score patterns as EFFECTIVE/NEUTRAL/INEFFECTIVE/HARMFUL
//! 3. Root Cause Analysis: Analyze why failures occurred
//! 4. Memory Distillation: Update pattern scores based on verdicts

mod verdict;
mod analyzer;
mod distillation;

pub use verdict::ReflectionVerdict;
// VerdictCategory and Verdict are used internally; public for future extensions
#[allow(unused_imports)]
pub use verdict::{Verdict, VerdictCategory};
pub use analyzer::TrajectoryAnalyzer;
// TrajectoryOutcome is used internally; public for future extensions
#[allow(unused_imports)]
pub use analyzer::TrajectoryOutcome;
pub use distillation::MemoryDistiller;
// VerdictSummary and VerdictStats are used in main.rs analyze command
#[allow(unused_imports)]
pub use distillation::{VerdictSummary, VerdictStats};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

/// Reflection engine state
#[allow(dead_code)] // Reserved for daemon mode state tracking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReflectionState {
    /// Number of trajectories queued for reflection
    pub queued_trajectories: usize,
    /// Timestamp of last reflection cycle
    pub last_reflection: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of trajectories processed in last cycle
    pub last_cycle_trajectories: usize,
    /// Number of verdicts produced in last cycle
    pub last_cycle_verdicts: usize,
}

/// Reflection engine configuration
#[derive(Debug, Clone)]
pub struct ReflectionConfig {
    /// Minimum trajectories to trigger data-driven reflection
    #[allow(dead_code)] // Used by ReflectionEngine::should_reflect
    pub data_threshold: usize,
    /// Hours between time-driven reflections
    #[allow(dead_code)] // Used by ReflectionEngine::should_reflect
    pub time_interval_hours: u32,
    /// Minimum confidence to act on verdict
    pub min_confidence: f32,
    /// Maximum penalty for HARMFUL verdicts
    pub max_penalty: i32,
    /// Maximum boost for EFFECTIVE verdicts
    pub max_boost: i32,
    /// Enable failure root cause analysis
    #[allow(dead_code)] // Reserved for future root cause toggle
    pub analyze_failures: bool,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            data_threshold: 50,
            time_interval_hours: 4,
            min_confidence: 0.6,
            max_penalty: -5,
            max_boost: 5,
            analyze_failures: true,
        }
    }
}

/// Main reflection engine
pub struct ReflectionEngine {
    config: ReflectionConfig,
    analyzer: TrajectoryAnalyzer,
    distiller: MemoryDistiller,
    #[allow(dead_code)] // Reserved for future database-aware operations
    db_path: Option<std::path::PathBuf>,
}

impl ReflectionEngine {
    /// Create a new reflection engine
    #[allow(dead_code)] // Used by with_db_path, exposed for external use
    pub fn new(config: ReflectionConfig) -> Self {
        Self {
            config: config.clone(),
            analyzer: TrajectoryAnalyzer::new(),
            distiller: MemoryDistiller::new(config),
            db_path: None,
        }
    }

    /// Create a new reflection engine with database path for pattern linking
    pub fn with_db_path(config: ReflectionConfig, db_path: &Path) -> Self {
        Self {
            config: config.clone(),
            analyzer: TrajectoryAnalyzer::new().with_db_path(db_path),
            distiller: MemoryDistiller::new(config),
            db_path: Some(db_path.to_path_buf()),
        }
    }

    /// Check if reflection should be triggered based on current state
    #[allow(dead_code)] // Reserved for daemon mode automatic triggering
    pub fn should_reflect(&self, state: &ReflectionState) -> bool {
        // Data-driven trigger
        if state.queued_trajectories >= self.config.data_threshold {
            debug!("Data-driven reflection triggered: {} trajectories", state.queued_trajectories);
            return true;
        }

        // Time-driven trigger
        if let Some(last) = state.last_reflection {
            let hours_since = chrono::Utc::now()
                .signed_duration_since(last)
                .num_hours();
            if hours_since >= self.config.time_interval_hours as i64 {
                debug!("Time-driven reflection triggered: {} hours since last", hours_since);
                return true;
            }
        } else {
            // Never reflected before, check if we have any trajectories
            if state.queued_trajectories > 0 {
                return true;
            }
        }

        false
    }

    /// Run a reflection cycle on the given trajectories
    pub fn reflect(&self, trajectories: &[crate::learning::trajectory::Trajectory]) -> Result<Vec<ReflectionVerdict>> {
        info!("Starting reflection cycle on {} trajectories", trajectories.len());

        let mut verdicts = Vec::new();

        for trajectory in trajectories {
            // Analyze the trajectory outcome
            let outcome = self.analyzer.analyze(trajectory);

            // Generate verdict based on outcome
            if let Some(verdict) = self.analyzer.judge(&outcome, trajectory) {
                if verdict.confidence >= self.config.min_confidence {
                    verdicts.push(verdict);
                }
            }
        }

        info!("Reflection produced {} verdicts", verdicts.len());
        Ok(verdicts)
    }

    /// Apply verdicts to update pattern scores
    pub fn apply_verdicts(&self, conn: &Connection, verdicts: &[ReflectionVerdict]) -> Result<usize> {
        self.distiller.distill(conn, verdicts)
    }
}

/// Initialize reflection tables in the database
pub fn init_reflection_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Store reflection verdicts
        CREATE TABLE IF NOT EXISTS reflection_verdicts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trajectory_hash TEXT NOT NULL,
            pattern_id INTEGER,
            verdict TEXT NOT NULL,
            confidence REAL NOT NULL,
            root_cause TEXT,
            suggested_improvement TEXT,
            context_mismatch INTEGER DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (pattern_id) REFERENCES patterns(id) ON DELETE SET NULL
        );

        -- Track reflection cycles
        CREATE TABLE IF NOT EXISTS reflection_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_type TEXT NOT NULL,
            trajectories_analyzed INTEGER NOT NULL,
            verdicts_created INTEGER NOT NULL,
            patterns_updated INTEGER NOT NULL,
            patterns_created INTEGER NOT NULL,
            patterns_demoted INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- Indices for efficient queries
        CREATE INDEX IF NOT EXISTS idx_verdicts_pattern ON reflection_verdicts(pattern_id);
        CREATE INDEX IF NOT EXISTS idx_verdicts_verdict ON reflection_verdicts(verdict);
        CREATE INDEX IF NOT EXISTS idx_verdicts_created ON reflection_verdicts(created_at);
        "#,
    )?;

    debug!("Initialized reflection tables");
    Ok(())
}

/// Get reflection status from the database
pub fn get_reflection_status(db_path: &Path) -> Result<ReflectionStatus> {
    let conn = Connection::open(db_path)?;

    // Check if tables exist
    let tables_exist: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='reflection_verdicts'",
        [],
        |row| row.get(0),
    ).unwrap_or(false);

    if !tables_exist {
        return Ok(ReflectionStatus::default());
    }

    // Get verdict counts by category
    let mut stmt = conn.prepare(
        "SELECT verdict, COUNT(*) FROM reflection_verdicts GROUP BY verdict"
    )?;
    let verdict_counts: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    // Get last reflection info
    let last_reflection: Option<(String, i64, i64, i64)> = conn.query_row(
        "SELECT trigger_type, trajectories_analyzed, verdicts_created, duration_ms
         FROM reflection_log ORDER BY created_at DESC LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).ok();

    // Get total reflection cycles
    let total_cycles: i64 = conn.query_row(
        "SELECT COUNT(*) FROM reflection_log",
        [],
        |row| row.get(0),
    ).unwrap_or(0);

    Ok(ReflectionStatus {
        tables_exist,
        total_verdicts: verdict_counts.iter().map(|(_, c)| *c).sum(),
        effective_count: verdict_counts.iter().find(|(v, _)| v == "EFFECTIVE").map(|(_, c)| *c).unwrap_or(0),
        ineffective_count: verdict_counts.iter().find(|(v, _)| v == "INEFFECTIVE").map(|(_, c)| *c).unwrap_or(0),
        harmful_count: verdict_counts.iter().find(|(v, _)| v == "HARMFUL").map(|(_, c)| *c).unwrap_or(0),
        neutral_count: verdict_counts.iter().find(|(v, _)| v == "NEUTRAL").map(|(_, c)| *c).unwrap_or(0),
        total_cycles,
        last_trigger: last_reflection.as_ref().map(|(t, _, _, _)| t.clone()),
        last_trajectories: last_reflection.as_ref().map(|(_, t, _, _)| *t).unwrap_or(0),
        last_verdicts: last_reflection.as_ref().map(|(_, _, v, _)| *v).unwrap_or(0),
        last_duration_ms: last_reflection.map(|(_, _, _, d)| d).unwrap_or(0),
    })
}

/// Reflection status for display
#[derive(Debug, Default)]
pub struct ReflectionStatus {
    pub tables_exist: bool,
    pub total_verdicts: i64,
    pub effective_count: i64,
    pub ineffective_count: i64,
    pub harmful_count: i64,
    pub neutral_count: i64,
    pub total_cycles: i64,
    pub last_trigger: Option<String>,
    pub last_trajectories: i64,
    pub last_verdicts: i64,
    pub last_duration_ms: i64,
}

/// Log a reflection cycle to the database
#[allow(clippy::too_many_arguments)]
pub fn log_reflection_cycle(
    conn: &Connection,
    trigger_type: &str,
    trajectories_analyzed: usize,
    verdicts_created: usize,
    patterns_updated: usize,
    patterns_created: usize,
    patterns_demoted: usize,
    duration_ms: u64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO reflection_log
         (trigger_type, trajectories_analyzed, verdicts_created, patterns_updated, patterns_created, patterns_demoted, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            trigger_type,
            trajectories_analyzed as i64,
            verdicts_created as i64,
            patterns_updated as i64,
            patterns_created as i64,
            patterns_demoted as i64,
            duration_ms as i64,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_reflection_tables_init() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        init_reflection_tables(&conn).unwrap();

        // Verify tables exist
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'reflection%'",
            [],
            |row| row.get(0),
        ).unwrap();

        assert_eq!(count, 2); // reflection_verdicts and reflection_log
    }

    #[test]
    fn test_should_reflect_data_driven() {
        let config = ReflectionConfig::default();
        let engine = ReflectionEngine::new(config);

        let state = ReflectionState {
            queued_trajectories: 50,
            ..Default::default()
        };

        assert!(engine.should_reflect(&state));
    }

    #[test]
    fn test_should_reflect_time_driven() {
        let config = ReflectionConfig {
            time_interval_hours: 1,
            ..Default::default()
        };
        let engine = ReflectionEngine::new(config);

        let state = ReflectionState {
            queued_trajectories: 10,
            last_reflection: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
            ..Default::default()
        };

        assert!(engine.should_reflect(&state));
    }
}
