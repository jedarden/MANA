//! SARSA (State-Action-Reward-State-Action) - On-policy TD Learning
//!
//! Unlike Q-learning (off-policy), SARSA learns the value of the policy being followed,
//! making it safer for exploration in environments where mistakes are costly.
//!
//! Key difference from Q-learning:
//! - Q-learning: Q(s,a) ← Q(s,a) + α[r + γ·max_a'(Q(s',a')) - Q(s,a)]
//! - SARSA:      Q(s,a) ← Q(s,a) + α[r + γ·Q(s',a') - Q(s,a)]
//!
//! SARSA uses the actual next action a' (following the policy), not the max.

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

/// SARSA configuration
#[derive(Debug, Clone)]
pub struct SarsaConfig {
    /// Learning rate (alpha)
    pub learning_rate: f64,
    /// Discount factor (gamma)
    pub discount_factor: f64,
    /// Exploration rate (epsilon)
    pub exploration_rate: f64,
    /// Minimum exploration rate
    pub min_exploration_rate: f64,
    /// Exploration decay per episode
    pub exploration_decay: f64,
    /// Expected SARSA mode (uses expected value instead of sampled action)
    pub expected_sarsa: bool,
}

impl Default for SarsaConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.1,
            discount_factor: 0.9,
            exploration_rate: 0.15,
            min_exploration_rate: 0.05,
            exploration_decay: 0.995,
            expected_sarsa: false,
        }
    }
}

/// SARSA Agent for on-policy learning
pub struct SarsaAgent {
    config: SarsaConfig,
    q_table: HashMap<(u64, i64), f64>,
    conn: Connection,
    current_epsilon: f64,
    episode_count: u64,
    /// Pending state-action for SARSA update (s, a)
    pending_sa: Option<(u64, i64)>,
}

impl SarsaAgent {
    pub fn new(db_path: &Path, config: SarsaConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let current_epsilon = config.exploration_rate;

        let mut agent = Self {
            config,
            q_table: HashMap::new(),
            conn,
            current_epsilon,
            episode_count: 0,
            pending_sa: None,
        };

        agent.init_schema()?;
        agent.load_q_table()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sarsa_q_table (
                context_hash INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                q_value REAL NOT NULL DEFAULT 0.0,
                update_count INTEGER NOT NULL DEFAULT 0,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (context_hash, pattern_id)
            );

            CREATE TABLE IF NOT EXISTS sarsa_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                context_hash INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                reward REAL NOT NULL,
                next_context_hash INTEGER,
                next_pattern_id INTEGER,
                new_q_value REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_sarsa_context ON sarsa_q_table(context_hash);
            "#,
        )?;
        Ok(())
    }

    fn load_q_table(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT context_hash, pattern_id, q_value FROM sarsa_q_table"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;

        for row in rows.flatten() {
            self.q_table.insert((row.0, row.1), row.2);
        }

        self.episode_count = self.conn.query_row(
            "SELECT COUNT(*) FROM sarsa_episodes",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        // Decay epsilon based on episodes
        for _ in 0..self.episode_count.min(1000) {
            self.current_epsilon *= self.config.exploration_decay;
            if self.current_epsilon < self.config.min_exploration_rate {
                self.current_epsilon = self.config.min_exploration_rate;
                break;
            }
        }

        Ok(())
    }

    /// Get Q-value for (state, action)
    pub fn get_q_value(&self, state: u64, action: i64) -> f64 {
        *self.q_table.get(&(state, action)).unwrap_or(&0.0)
    }

    /// Select action using epsilon-greedy policy
    pub fn select_action(&mut self, state: u64, available_actions: &[i64]) -> Option<i64> {
        if available_actions.is_empty() {
            return None;
        }

        let action = if rand::random::<f64>() < self.current_epsilon {
            // Explore
            let idx = (rand::random::<f64>() * available_actions.len() as f64) as usize;
            available_actions[idx.min(available_actions.len() - 1)]
        } else {
            // Exploit
            *available_actions
                .iter()
                .max_by(|&&a, &&b| {
                    self.get_q_value(state, a)
                        .partial_cmp(&self.get_q_value(state, b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap()
        };

        // Store for SARSA update
        self.pending_sa = Some((state, action));

        Some(action)
    }

    /// SARSA update: called after taking action and observing reward + next state
    pub fn update(
        &mut self,
        state: u64,
        action: i64,
        reward: f64,
        next_state: u64,
        next_action: i64,
        terminal: bool,
    ) -> Result<f64> {
        let old_q = self.get_q_value(state, action);

        // SARSA update rule (uses actual next action, not max)
        let next_q = if terminal {
            0.0
        } else if self.config.expected_sarsa {
            // Expected SARSA: use expected value over all actions
            self.get_expected_q(next_state)
        } else {
            // Regular SARSA: use Q of actual next action
            self.get_q_value(next_state, next_action)
        };

        let new_q = old_q + self.config.learning_rate * (reward + self.config.discount_factor * next_q - old_q);

        // Update table
        self.q_table.insert((state, action), new_q);

        // Persist
        self.conn.execute(
            r#"
            INSERT INTO sarsa_q_table (context_hash, pattern_id, q_value, update_count)
            VALUES (?1, ?2, ?3, 1)
            ON CONFLICT(context_hash, pattern_id) DO UPDATE SET
                q_value = ?3,
                update_count = update_count + 1,
                last_updated = CURRENT_TIMESTAMP
            "#,
            params![state as i64, action, new_q],
        )?;

        // Log episode
        self.conn.execute(
            r#"
            INSERT INTO sarsa_episodes
            (context_hash, pattern_id, reward, next_context_hash, next_pattern_id, new_q_value)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                state as i64,
                action,
                reward,
                if terminal { None } else { Some(next_state as i64) },
                if terminal { None } else { Some(next_action) },
                new_q
            ],
        )?;

        // Decay epsilon
        self.episode_count += 1;
        self.current_epsilon = (self.current_epsilon * self.config.exploration_decay)
            .max(self.config.min_exploration_rate);

        Ok(new_q)
    }

    /// Expected value of Q under epsilon-greedy policy
    fn get_expected_q(&self, state: u64) -> f64 {
        let actions: Vec<_> = self.q_table
            .keys()
            .filter(|(s, _)| *s == state)
            .map(|(_, a)| *a)
            .collect();

        if actions.is_empty() {
            return 0.0;
        }

        let max_q = actions.iter()
            .map(|&a| self.get_q_value(state, a))
            .fold(f64::NEG_INFINITY, f64::max);

        let greedy_prob = 1.0 - self.current_epsilon + self.current_epsilon / actions.len() as f64;
        let explore_prob = self.current_epsilon / actions.len() as f64;

        let mut expected = 0.0;
        for &a in &actions {
            let q = self.get_q_value(state, a);
            let prob = if (q - max_q).abs() < 1e-9 { greedy_prob } else { explore_prob };
            expected += prob * q;
        }

        expected
    }

    /// Step through episode with automatic SARSA updates
    pub fn step(
        &mut self,
        reward: f64,
        next_state: u64,
        available_actions: &[i64],
        terminal: bool,
    ) -> Result<Option<i64>> {
        if let Some((prev_state, prev_action)) = self.pending_sa {
            // Select next action
            let next_action = if terminal || available_actions.is_empty() {
                0 // Dummy action for terminal state
            } else {
                self.select_action(next_state, available_actions).unwrap_or(0)
            };

            // SARSA update
            self.update(prev_state, prev_action, reward, next_state, next_action, terminal)?;

            if terminal {
                self.pending_sa = None;
                return Ok(None);
            }

            return Ok(Some(next_action));
        }

        // First step in episode
        if !available_actions.is_empty() {
            return Ok(self.select_action(next_state, available_actions));
        }

        Ok(None)
    }

    /// Reset episode state
    pub fn reset_episode(&mut self) {
        self.pending_sa = None;
    }

    pub fn exploration_rate(&self) -> f64 {
        self.current_epsilon
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn q_table_size(&self) -> usize {
        self.q_table.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_sarsa_update() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("sarsa.db");
        let config = SarsaConfig::default();
        let mut agent = SarsaAgent::new(&db_path, config).unwrap();

        // Initial Q should be 0
        assert_eq!(agent.get_q_value(100, 1), 0.0);

        // Update with reward
        let new_q = agent.update(100, 1, 1.0, 101, 2, false).unwrap();
        assert!(new_q > 0.0);
    }

    #[test]
    fn test_sarsa_on_policy() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("sarsa.db");
        let config = SarsaConfig {
            exploration_rate: 0.0, // No exploration for deterministic test
            ..Default::default()
        };
        let mut agent = SarsaAgent::new(&db_path, config).unwrap();

        // Set up Q-values
        agent.update(100, 1, 1.0, 100, 1, false).unwrap();
        agent.update(100, 2, -1.0, 100, 2, false).unwrap();

        // Should select action 1 (higher Q)
        let action = agent.select_action(100, &[1, 2]);
        assert_eq!(action, Some(1));
    }

    #[test]
    fn test_expected_sarsa() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("sarsa.db");
        let config = SarsaConfig {
            expected_sarsa: true,
            ..Default::default()
        };
        let mut agent = SarsaAgent::new(&db_path, config).unwrap();

        // Run some updates
        agent.update(100, 1, 1.0, 100, 1, false).unwrap();
        agent.update(100, 2, 0.5, 100, 2, false).unwrap();

        // Expected SARSA should work
        assert!(agent.q_table_size() > 0);
    }

    #[test]
    fn test_episode_step() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("sarsa.db");
        let config = SarsaConfig::default();
        let mut agent = SarsaAgent::new(&db_path, config).unwrap();

        // First step
        let action1 = agent.step(0.0, 100, &[1, 2, 3], false).unwrap();
        assert!(action1.is_some());

        // Second step (triggers update for first)
        let action2 = agent.step(1.0, 101, &[1, 2, 3], false).unwrap();
        assert!(action2.is_some());

        // Terminal step
        let action3 = agent.step(10.0, 102, &[], true).unwrap();
        assert!(action3.is_none());

        assert!(agent.episode_count() > 0);
    }
}
