//! Memory health monitoring and event-driven pruning
//!
//! Proactive memory maintenance based on health conditions:
//! - Delete floor-confidence memories (score < -5)
//! - Decay stale memories (30+ days unused)
//! - Penalize never-accessed patterns
//! - Storage pressure triggers

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tracing::{info, debug};

/// Health status of the pattern database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub total_patterns: i64,
    pub low_confidence_count: i64,    // score < -2
    pub floor_confidence_count: i64,   // score < -5
    pub stale_count: i64,             // 30+ days unused
    pub never_accessed_count: i64,    // access_count = 0
    pub avg_score: f64,
    pub storage_size_bytes: u64,
    pub is_healthy: bool,
    pub health_score: f64,            // 0.0 - 1.0
}

impl HealthStatus {
    /// Determine if the database is healthy based on thresholds
    pub fn compute_health(&mut self, config: &PruningConfig) {
        // Health score factors:
        // - Floor patterns are very bad (each -5 points)
        // - Low confidence patterns are bad (each -2 points)
        // - Stale patterns are slightly bad (each -1 point)
        // - Never accessed patterns indicate poor quality (each -1 point)

        let mut health = 100.0;

        if self.total_patterns > 0 {
            // Penalize floor confidence heavily
            let floor_ratio = self.floor_confidence_count as f64 / self.total_patterns as f64;
            health -= floor_ratio * 50.0;

            // Penalize low confidence moderately
            let low_ratio = self.low_confidence_count as f64 / self.total_patterns as f64;
            health -= low_ratio * 20.0;

            // Penalize stale patterns
            let stale_ratio = self.stale_count as f64 / self.total_patterns as f64;
            health -= stale_ratio * 15.0;

            // Penalize never accessed patterns
            let unused_ratio = self.never_accessed_count as f64 / self.total_patterns as f64;
            health -= unused_ratio * 15.0;

            // Bonus for good average score
            if self.avg_score > 3.0 {
                health += 10.0;
            }
        }

        // Storage pressure
        if self.storage_size_bytes > config.max_storage_bytes {
            let pressure = (self.storage_size_bytes as f64 / config.max_storage_bytes as f64) - 1.0;
            health -= pressure * 20.0;
        }

        // Clamp to 0-100
        health = health.max(0.0).min(100.0);

        self.health_score = health / 100.0;
        self.is_healthy = self.health_score >= config.target_health_score;
    }
}

/// Pruning actions that can be taken
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PruningAction {
    DeleteFloor,        // Remove score < -5 patterns
    DecayStale,         // Reduce score for 30+ day unused
    PenalizeUnused,     // Decay never-accessed patterns
    AggressiveDecay,    // Signal/noise ratio degradation
    StoragePressure,    // Prune when DB size exceeds threshold
}

impl std::fmt::Display for PruningAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PruningAction::DeleteFloor => write!(f, "DeleteFloor"),
            PruningAction::DecayStale => write!(f, "DecayStale"),
            PruningAction::PenalizeUnused => write!(f, "PenalizeUnused"),
            PruningAction::AggressiveDecay => write!(f, "AggressiveDecay"),
            PruningAction::StoragePressure => write!(f, "StoragePressure"),
        }
    }
}

/// Configuration for pruning behavior
#[derive(Debug, Clone)]
pub struct PruningConfig {
    pub floor_threshold: i64,         // Default: -5
    pub low_threshold: i64,           // Default: -2
    pub stale_days: i64,              // Default: 30
    pub decay_factor: f64,            // Default: 0.85
    pub max_storage_bytes: u64,       // Default: 100MB
    pub target_health_score: f64,     // Default: 0.7
}

impl Default for PruningConfig {
    fn default() -> Self {
        Self {
            floor_threshold: -5,
            low_threshold: -2,
            stale_days: 30,
            decay_factor: 0.85,
            max_storage_bytes: 100 * 1024 * 1024, // 100MB
            target_health_score: 0.7,
        }
    }
}

/// Result of a pruning operation
#[derive(Debug, Serialize, Deserialize)]
pub struct PruningResult {
    pub actions_taken: Vec<PruningAction>,
    pub patterns_deleted: usize,
    pub patterns_decayed: usize,
    pub before_health: HealthStatus,
    pub after_health: HealthStatus,
}

/// Health monitor for pattern database
pub struct HealthMonitor {
    config: PruningConfig,
}

impl HealthMonitor {
    /// Create a new health monitor with the given configuration
    pub fn new(config: PruningConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default() -> Self {
        Self::new(PruningConfig::default())
    }

    /// Compute current health status of the database
    pub fn check_health(&self, conn: &Connection) -> Result<HealthStatus> {
        // Get total patterns
        let total_patterns: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        // Get low confidence count (score < -2)
        let low_confidence_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns WHERE (success_count - failure_count) < ?1",
            [self.config.low_threshold],
            |row| row.get(0)
        ).unwrap_or(0);

        // Get floor confidence count (score < -5)
        let floor_confidence_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns WHERE (success_count - failure_count) < ?1",
            [self.config.floor_threshold],
            |row| row.get(0)
        ).unwrap_or(0);

        // Get stale patterns (30+ days unused)
        let stale_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns
             WHERE last_used IS NULL OR last_used < datetime('now', '-' || ?1 || ' days')",
            [self.config.stale_days],
            |row| row.get(0)
        ).unwrap_or(0);

        // Get never accessed count (access_count = 0 or NULL)
        // Note: We need to handle case where access_count column doesn't exist yet
        let never_accessed_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns WHERE
             (SELECT COUNT(*) FROM pragma_table_info('patterns') WHERE name = 'access_count') > 0
             AND (access_count IS NULL OR access_count = 0)",
            [],
            |row| row.get(0)
        ).unwrap_or(0);

        // Get average score
        let avg_score: f64 = conn.query_row(
            "SELECT AVG(success_count - failure_count) FROM patterns WHERE total_patterns > 0",
            [],
            |row| row.get(0)
        ).unwrap_or(0.0);

        // Get database file size
        let db_path = conn.path().unwrap_or("");
        let storage_size_bytes = std::fs::metadata(db_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let mut status = HealthStatus {
            total_patterns,
            low_confidence_count,
            floor_confidence_count,
            stale_count,
            never_accessed_count,
            avg_score,
            storage_size_bytes,
            is_healthy: true,
            health_score: 1.0,
        };

        // Compute health metrics
        status.compute_health(&self.config);

        Ok(status)
    }

    /// Determine which pruning actions should be taken based on health status
    pub fn recommend_actions(&self, status: &HealthStatus) -> Vec<PruningAction> {
        let mut actions = Vec::new();

        // Always delete floor-confidence patterns
        if status.floor_confidence_count > 0 {
            actions.push(PruningAction::DeleteFloor);
        }

        // Decay stale patterns if there are many
        let stale_ratio = if status.total_patterns > 0 {
            status.stale_count as f64 / status.total_patterns as f64
        } else {
            0.0
        };

        if stale_ratio > 0.2 {
            actions.push(PruningAction::DecayStale);
        }

        // Penalize never-accessed patterns if ratio is high
        let unused_ratio = if status.total_patterns > 0 {
            status.never_accessed_count as f64 / status.total_patterns as f64
        } else {
            0.0
        };

        if unused_ratio > 0.3 {
            actions.push(PruningAction::PenalizeUnused);
        }

        // Aggressive decay if signal/noise ratio is poor
        if status.avg_score < 0.0 && status.low_confidence_count > status.total_patterns / 2 {
            actions.push(PruningAction::AggressiveDecay);
        }

        // Storage pressure pruning
        if status.storage_size_bytes > self.config.max_storage_bytes {
            actions.push(PruningAction::StoragePressure);
        }

        actions
    }

    /// Execute a single pruning action
    pub fn execute_action(&self, conn: &Connection, action: PruningAction) -> Result<usize> {
        match action {
            PruningAction::DeleteFloor => {
                // Delete patterns with score < -5
                let deleted = conn.execute(
                    "DELETE FROM patterns WHERE (success_count - failure_count) < ?1",
                    [self.config.floor_threshold],
                )?;

                info!("Deleted {} floor-confidence patterns (score < {})",
                      deleted, self.config.floor_threshold);
                Ok(deleted)
            }

            PruningAction::DecayStale => {
                // Reduce success_count for stale patterns
                let decayed = conn.execute(
                    "UPDATE patterns
                     SET success_count = CAST(success_count * ?1 AS INTEGER)
                     WHERE last_used IS NULL OR last_used < datetime('now', '-' || ?2 || ' days')",
                    params![self.config.decay_factor, self.config.stale_days],
                )?;

                info!("Decayed {} stale patterns ({}+ days unused)",
                      decayed, self.config.stale_days);
                Ok(decayed)
            }

            PruningAction::PenalizeUnused => {
                // Check if access_count column exists
                let has_access_count: bool = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('patterns') WHERE name = 'access_count'",
                    [],
                    |row| Ok(row.get::<_, i64>(0)? > 0),
                ).unwrap_or(false);

                if !has_access_count {
                    debug!("access_count column not found, skipping PenalizeUnused");
                    return Ok(0);
                }

                // Reduce score for never-accessed patterns
                let penalized = conn.execute(
                    "UPDATE patterns
                     SET success_count = CAST(success_count * ?1 AS INTEGER)
                     WHERE access_count IS NULL OR access_count = 0",
                    [self.config.decay_factor],
                )?;

                info!("Penalized {} never-accessed patterns", penalized);
                Ok(penalized)
            }

            PruningAction::AggressiveDecay => {
                // Apply aggressive decay to all low-confidence patterns
                let decayed = conn.execute(
                    "UPDATE patterns
                     SET success_count = CAST(success_count * ?1 AS INTEGER)
                     WHERE (success_count - failure_count) < ?2",
                    params![self.config.decay_factor * 0.8, self.config.low_threshold],
                )?;

                info!("Applied aggressive decay to {} low-confidence patterns", decayed);
                Ok(decayed)
            }

            PruningAction::StoragePressure => {
                // Delete lowest-scoring patterns until we're under the limit
                // Target: remove bottom 10% of patterns
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM patterns",
                    [],
                    |row| row.get(0)
                ).unwrap_or(0);

                let to_remove = (total as f64 * 0.1) as i64;

                let deleted = conn.execute(
                    "DELETE FROM patterns
                     WHERE id IN (
                         SELECT id FROM patterns
                         ORDER BY (success_count - failure_count) ASC
                         LIMIT ?1
                     )",
                    [to_remove],
                )?;

                info!("Deleted {} patterns due to storage pressure", deleted);
                Ok(deleted)
            }
        }
    }

    /// Run full health check and automatic pruning
    pub fn auto_prune(&self, conn: &Connection) -> Result<PruningResult> {
        // Get initial health status
        let before_health = self.check_health(conn)?;

        info!("Health check before pruning: score={:.2}, healthy={}",
              before_health.health_score, before_health.is_healthy);

        // Determine actions to take
        let actions = self.recommend_actions(&before_health);

        if actions.is_empty() {
            info!("No pruning actions needed");
            return Ok(PruningResult {
                actions_taken: actions,
                patterns_deleted: 0,
                patterns_decayed: 0,
                before_health: before_health.clone(),
                after_health: before_health,
            });
        }

        info!("Recommended actions: {:?}", actions);

        // Execute actions
        let mut total_deleted = 0;
        let mut total_decayed = 0;

        for action in &actions {
            match self.execute_action(conn, *action) {
                Ok(count) => {
                    match action {
                        PruningAction::DeleteFloor | PruningAction::StoragePressure => {
                            total_deleted += count;
                        }
                        _ => {
                            total_decayed += count;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to execute action {:?}: {}", action, e);
                }
            }
        }

        // Get final health status
        let after_health = self.check_health(conn)?;

        info!("Health check after pruning: score={:.2}, healthy={}",
              after_health.health_score, after_health.is_healthy);

        Ok(PruningResult {
            actions_taken: actions,
            patterns_deleted: total_deleted,
            patterns_decayed: total_decayed,
            before_health,
            after_health,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_computation() {
        let config = PruningConfig::default();
        let mut status = HealthStatus {
            total_patterns: 100,
            low_confidence_count: 10,
            floor_confidence_count: 5,
            stale_count: 20,
            never_accessed_count: 15,
            avg_score: 2.0,
            storage_size_bytes: 50 * 1024 * 1024,
            is_healthy: true,
            health_score: 1.0,
        };

        status.compute_health(&config);

        // Should be unhealthy with these metrics
        assert!(status.health_score < 0.7);
        assert!(!status.is_healthy);
    }

    #[test]
    fn test_action_recommendations() {
        let monitor = HealthMonitor::default();

        let status = HealthStatus {
            total_patterns: 100,
            low_confidence_count: 10,
            floor_confidence_count: 5,
            stale_count: 25,
            never_accessed_count: 35,
            avg_score: 2.0,
            storage_size_bytes: 50 * 1024 * 1024,
            is_healthy: false,
            health_score: 0.5,
        };

        let actions = monitor.recommend_actions(&status);

        // Should recommend deleting floor patterns
        assert!(actions.contains(&PruningAction::DeleteFloor));

        // Should recommend decaying stale patterns (25% is > 20% threshold)
        assert!(actions.contains(&PruningAction::DecayStale));

        // Should recommend penalizing unused (35% is > 30% threshold)
        assert!(actions.contains(&PruningAction::PenalizeUnused));
    }
}
