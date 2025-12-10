//! Q-Learning for adaptive pattern weight optimization
//!
//! This module implements a simple Q-learning algorithm to adaptively
//! adjust pattern weights based on outcomes. This enables MANA to:
//!
//! - Learn which patterns work best in different contexts
//! - Adapt to user preferences over time
//! - Balance exploration (trying new patterns) vs exploitation (using proven patterns)
//!
//! The Q-table maps (context_hash, pattern_id) -> Q-value
//! Q-values represent expected reward for using a pattern in a given context.

#![allow(dead_code)] // New API - will be integrated in future versions

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

/// Q-Learning configuration
#[derive(Debug, Clone)]
pub struct QLearningConfig {
    /// Learning rate (alpha): How much new information overrides old
    /// Range: 0.0 to 1.0, typical: 0.1
    pub learning_rate: f64,

    /// Discount factor (gamma): How much to value future rewards
    /// Range: 0.0 to 1.0, typical: 0.9
    pub discount_factor: f64,

    /// Exploration rate (epsilon): Probability of trying random pattern
    /// Range: 0.0 to 1.0, typical: 0.1
    pub exploration_rate: f64,

    /// Minimum exploration rate (epsilon won't decay below this)
    pub min_exploration_rate: f64,

    /// Exploration decay rate (per episode)
    pub exploration_decay: f64,

    /// Reward for successful pattern application
    pub success_reward: f64,

    /// Penalty for failed pattern application
    pub failure_penalty: f64,

    /// Bonus for patterns that lead to task completion
    pub completion_bonus: f64,
}

impl Default for QLearningConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.1,
            discount_factor: 0.9,
            exploration_rate: 0.15,
            min_exploration_rate: 0.05,
            exploration_decay: 0.995,
            success_reward: 1.0,
            failure_penalty: -0.5,
            completion_bonus: 2.0,
        }
    }
}

/// Q-Learning agent for pattern selection
pub struct QLearningAgent {
    config: QLearningConfig,
    /// In-memory Q-table cache for fast lookups
    q_cache: HashMap<(u64, i64), f64>,
    /// Database connection for persistence
    conn: Connection,
    /// Current exploration rate
    current_epsilon: f64,
    /// Episode counter
    episode_count: u64,
}

impl QLearningAgent {
    /// Create a new Q-learning agent
    pub fn new(db_path: &Path, config: QLearningConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let current_epsilon = config.exploration_rate;

        let mut agent = Self {
            config,
            q_cache: HashMap::new(),
            conn,
            current_epsilon,
            episode_count: 0,
        };

        agent.init_schema()?;
        agent.load_q_table()?;

        Ok(agent)
    }

    /// Initialize database schema
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS q_table (
                context_hash INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                q_value REAL NOT NULL DEFAULT 0.0,
                update_count INTEGER NOT NULL DEFAULT 0,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (context_hash, pattern_id)
            );

            CREATE TABLE IF NOT EXISTS q_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                context_hash INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                reward REAL NOT NULL,
                new_q_value REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS q_config (
                key TEXT PRIMARY KEY,
                value REAL NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_q_context ON q_table(context_hash);
            CREATE INDEX IF NOT EXISTS idx_q_pattern ON q_table(pattern_id);
            "#,
        )?;
        Ok(())
    }

    /// Load Q-table from database into memory
    fn load_q_table(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT context_hash, pattern_id, q_value FROM q_table"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;

        for row in rows.flatten() {
            let (context_hash, pattern_id, q_value) = row;
            self.q_cache.insert((context_hash, pattern_id), q_value);
        }

        // Load episode count
        self.episode_count = self.conn.query_row(
            "SELECT COUNT(*) FROM q_episodes",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        // Decay epsilon based on episodes
        for _ in 0..self.episode_count {
            self.current_epsilon *= self.config.exploration_decay;
            if self.current_epsilon < self.config.min_exploration_rate {
                self.current_epsilon = self.config.min_exploration_rate;
                break;
            }
        }

        Ok(())
    }

    /// Get Q-value for a (context, pattern) pair
    pub fn get_q_value(&self, context_hash: u64, pattern_id: i64) -> f64 {
        *self.q_cache.get(&(context_hash, pattern_id)).unwrap_or(&0.0)
    }

    /// Update Q-value based on reward
    pub fn update(&mut self, context_hash: u64, pattern_id: i64, reward: f64, next_max_q: f64) -> Result<f64> {
        let old_q = self.get_q_value(context_hash, pattern_id);

        // Q-learning update rule:
        // Q(s,a) = Q(s,a) + α * (r + γ * max(Q(s',a')) - Q(s,a))
        let new_q = old_q + self.config.learning_rate *
            (reward + self.config.discount_factor * next_max_q - old_q);

        // Update cache
        self.q_cache.insert((context_hash, pattern_id), new_q);

        // Persist to database
        self.conn.execute(
            r#"
            INSERT INTO q_table (context_hash, pattern_id, q_value, update_count)
            VALUES (?1, ?2, ?3, 1)
            ON CONFLICT(context_hash, pattern_id) DO UPDATE SET
                q_value = ?3,
                update_count = update_count + 1,
                last_updated = CURRENT_TIMESTAMP
            "#,
            params![context_hash as i64, pattern_id, new_q],
        )?;

        // Log episode
        self.conn.execute(
            "INSERT INTO q_episodes (context_hash, pattern_id, reward, new_q_value) VALUES (?1, ?2, ?3, ?4)",
            params![context_hash as i64, pattern_id, reward, new_q],
        )?;

        // Increment episode and decay epsilon
        self.episode_count += 1;
        self.current_epsilon *= self.config.exploration_decay;
        if self.current_epsilon < self.config.min_exploration_rate {
            self.current_epsilon = self.config.min_exploration_rate;
        }

        Ok(new_q)
    }

    /// Record a successful pattern application
    pub fn record_success(&mut self, context_hash: u64, pattern_id: i64, task_completed: bool) -> Result<f64> {
        let reward = if task_completed {
            self.config.success_reward + self.config.completion_bonus
        } else {
            self.config.success_reward
        };

        // For terminal states, next_max_q is 0
        let next_max_q = if task_completed { 0.0 } else {
            self.get_max_q_for_context(context_hash)
        };

        self.update(context_hash, pattern_id, reward, next_max_q)
    }

    /// Record a failed pattern application
    pub fn record_failure(&mut self, context_hash: u64, pattern_id: i64) -> Result<f64> {
        let next_max_q = self.get_max_q_for_context(context_hash);
        self.update(context_hash, pattern_id, self.config.failure_penalty, next_max_q)
    }

    /// Get the maximum Q-value for any pattern in a given context
    pub fn get_max_q_for_context(&self, context_hash: u64) -> f64 {
        self.q_cache.iter()
            .filter(|((ctx, _), _)| *ctx == context_hash)
            .map(|(_, q)| *q)
            .fold(0.0, f64::max)
    }

    /// Select the best pattern for a context (epsilon-greedy)
    pub fn select_pattern(&self, context_hash: u64, available_patterns: &[i64]) -> Option<i64> {
        if available_patterns.is_empty() {
            return None;
        }

        // Epsilon-greedy selection
        let random: f64 = rand::random();
        if random < self.current_epsilon {
            // Explore: random selection
            let idx = (rand::random::<f64>() * available_patterns.len() as f64) as usize;
            return Some(available_patterns[idx.min(available_patterns.len() - 1)]);
        }

        // Exploit: select best Q-value
        let mut best_pattern = available_patterns[0];
        let mut best_q = self.get_q_value(context_hash, best_pattern);

        for &pattern_id in &available_patterns[1..] {
            let q = self.get_q_value(context_hash, pattern_id);
            if q > best_q {
                best_q = q;
                best_pattern = pattern_id;
            }
        }

        Some(best_pattern)
    }

    /// Rank patterns by Q-value for a context
    pub fn rank_patterns(&self, context_hash: u64, pattern_ids: &[i64]) -> Vec<(i64, f64)> {
        let mut ranked: Vec<(i64, f64)> = pattern_ids.iter()
            .map(|&id| (id, self.get_q_value(context_hash, id)))
            .collect();

        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked
    }

    /// Get patterns with positive Q-values (proven to work)
    pub fn get_proven_patterns(&self, context_hash: u64, min_q: f64) -> Vec<(i64, f64)> {
        self.q_cache.iter()
            .filter(|((ctx, _), q)| *ctx == context_hash && **q >= min_q)
            .map(|((_, pid), q)| (*pid, *q))
            .collect()
    }

    /// Get current exploration rate
    pub fn exploration_rate(&self) -> f64 {
        self.current_epsilon
    }

    /// Get episode count
    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    /// Get Q-table size
    pub fn q_table_size(&self) -> usize {
        self.q_cache.len()
    }

    /// Get statistics
    pub fn stats(&self) -> Result<QLearningStats> {
        let total_entries = self.q_cache.len();
        let positive_q: usize = self.q_cache.values().filter(|&&q| q > 0.0).count();
        let negative_q: usize = self.q_cache.values().filter(|&&q| q < 0.0).count();

        let avg_q: f64 = if total_entries > 0 {
            self.q_cache.values().sum::<f64>() / total_entries as f64
        } else {
            0.0
        };

        let max_q = self.q_cache.values().fold(0.0, |a, &b| f64::max(a, b));
        let min_q = self.q_cache.values().fold(0.0, |a, &b| f64::min(a, b));

        // Count unique contexts
        let unique_contexts: usize = self.q_cache.keys()
            .map(|(ctx, _)| ctx)
            .collect::<std::collections::HashSet<_>>()
            .len();

        Ok(QLearningStats {
            total_entries,
            positive_q,
            negative_q,
            avg_q,
            max_q,
            min_q,
            unique_contexts,
            episode_count: self.episode_count,
            current_epsilon: self.current_epsilon,
        })
    }

    /// Prune Q-entries that haven't been updated recently
    pub fn prune_stale(&mut self, min_updates: i64) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM q_table WHERE update_count < ?1",
            params![min_updates],
        )?;

        // Reload cache
        self.q_cache.clear();
        self.load_q_table()?;

        Ok(deleted)
    }

    /// Decay all Q-values (useful for non-stationary environments)
    pub fn decay_all(&mut self, decay_factor: f64) -> Result<usize> {
        self.conn.execute(
            "UPDATE q_table SET q_value = q_value * ?1",
            params![decay_factor],
        )?;

        // Update cache
        for q in self.q_cache.values_mut() {
            *q *= decay_factor;
        }

        Ok(self.q_cache.len())
    }
}

/// Hash a context string to a u64
pub fn hash_context(context: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    context.hash(&mut hasher);
    hasher.finish()
}

/// Q-Learning statistics
#[derive(Debug, Clone)]
pub struct QLearningStats {
    pub total_entries: usize,
    pub positive_q: usize,
    pub negative_q: usize,
    pub avg_q: f64,
    pub max_q: f64,
    pub min_q: f64,
    pub unique_contexts: usize,
    pub episode_count: u64,
    pub current_epsilon: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_q_value_update() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig::default();
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        // Initial Q-value should be 0
        assert_eq!(agent.get_q_value(123, 1), 0.0);

        // Record success
        let new_q = agent.record_success(123, 1, false).unwrap();
        assert!(new_q > 0.0);

        // Q-value should persist
        assert_eq!(agent.get_q_value(123, 1), new_q);
    }

    #[test]
    fn test_q_learning_convergence() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig {
            learning_rate: 0.5, // High learning rate for fast test
            ..Default::default()
        };
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        // Simulate consistent success with pattern 1
        for _ in 0..10 {
            agent.record_success(100, 1, false).unwrap();
        }

        // Simulate consistent failure with pattern 2
        for _ in 0..10 {
            agent.record_failure(100, 2).unwrap();
        }

        // Pattern 1 should have higher Q-value
        let q1 = agent.get_q_value(100, 1);
        let q2 = agent.get_q_value(100, 2);
        assert!(q1 > q2, "q1={} should be > q2={}", q1, q2);
    }

    #[test]
    fn test_pattern_selection() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig {
            exploration_rate: 0.0, // No exploration for deterministic test
            ..Default::default()
        };
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        // Set up Q-values
        agent.record_success(100, 1, true).unwrap();
        agent.record_failure(100, 2).unwrap();

        // Should select pattern 1 (higher Q-value)
        let selected = agent.select_pattern(100, &[1, 2]);
        assert_eq!(selected, Some(1));
    }

    #[test]
    fn test_ranking() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig::default();
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        // Set up varying Q-values
        for _ in 0..5 {
            agent.record_success(100, 1, false).unwrap();
        }
        for _ in 0..3 {
            agent.record_success(100, 2, false).unwrap();
        }
        agent.record_success(100, 3, false).unwrap();

        let ranked = agent.rank_patterns(100, &[1, 2, 3]);
        assert_eq!(ranked[0].0, 1); // Pattern 1 should be first
        assert!(ranked[0].1 > ranked[1].1);
        assert!(ranked[1].1 > ranked[2].1);
    }

    #[test]
    fn test_epsilon_decay() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig {
            exploration_rate: 0.5,
            exploration_decay: 0.9,
            min_exploration_rate: 0.1,
            ..Default::default()
        };
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        let initial_epsilon = agent.exploration_rate();

        // Record some episodes
        for _ in 0..10 {
            agent.record_success(100, 1, false).unwrap();
        }

        let final_epsilon = agent.exploration_rate();
        assert!(final_epsilon < initial_epsilon);
        assert!(final_epsilon >= 0.1); // Should not go below minimum
    }

    #[test]
    fn test_stats() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let config = QLearningConfig::default();
        let mut agent = QLearningAgent::new(&db_path, config).unwrap();

        agent.record_success(100, 1, false).unwrap();
        agent.record_failure(100, 2).unwrap();
        agent.record_success(200, 1, false).unwrap();

        let stats = agent.stats().unwrap();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.unique_contexts, 2);
        assert_eq!(stats.episode_count, 3);
    }

    #[test]
    fn test_hash_context() {
        let hash1 = hash_context("fix bug in main.rs");
        let hash2 = hash_context("fix bug in main.rs");
        let hash3 = hash_context("different context");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
