//! DQN (Deep Q-Network) - Neural Network-based Q-Learning
//!
//! A simplified DQN implementation using a linear function approximator
//! instead of a neural network (since we don't have deep learning deps).
//! This provides the key benefits of DQN:
//!
//! - Function approximation for generalization across similar states
//! - Experience replay for stable learning
//! - Target network for reduced oscillation
//!
//! For full neural network support, integrate with a DL framework.

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::VecDeque;
use std::path::Path;

/// DQN Configuration
#[derive(Debug, Clone)]
pub struct DqnConfig {
    /// Learning rate
    pub learning_rate: f64,
    /// Discount factor (gamma)
    pub discount_factor: f64,
    /// Exploration rate (epsilon)
    pub exploration_rate: f64,
    /// Minimum exploration rate
    pub min_exploration_rate: f64,
    /// Exploration decay per step
    pub exploration_decay: f64,
    /// Experience replay buffer size
    pub replay_buffer_size: usize,
    /// Mini-batch size for training
    pub batch_size: usize,
    /// Target network update frequency (steps)
    pub target_update_freq: usize,
    /// Feature dimension for state representation
    pub feature_dim: usize,
    /// Number of possible actions
    pub num_actions: usize,
}

impl Default for DqnConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.001,
            discount_factor: 0.99,
            exploration_rate: 1.0,
            min_exploration_rate: 0.01,
            exploration_decay: 0.9995,
            replay_buffer_size: 10000,
            batch_size: 32,
            target_update_freq: 100,
            feature_dim: 64,
            num_actions: 10,
        }
    }
}

/// Experience tuple for replay buffer
#[derive(Debug, Clone)]
pub struct Experience {
    pub state: Vec<f64>,
    pub action: usize,
    pub reward: f64,
    pub next_state: Vec<f64>,
    pub done: bool,
}

/// Linear Q-Network (simplified DQN without deep learning deps)
/// Uses linear function approximation: Q(s,a) = w_a · s + b_a
#[derive(Debug, Clone)]
pub struct LinearQNetwork {
    /// Weights for each action: [num_actions][feature_dim]
    weights: Vec<Vec<f64>>,
    /// Biases for each action
    biases: Vec<f64>,
    feature_dim: usize,
    num_actions: usize,
}

impl LinearQNetwork {
    pub fn new(feature_dim: usize, num_actions: usize) -> Self {
        // Initialize with small random weights
        let weights: Vec<Vec<f64>> = (0..num_actions)
            .map(|_| {
                (0..feature_dim)
                    .map(|_| (rand::random::<f64>() - 0.5) * 0.1)
                    .collect()
            })
            .collect();

        let biases = vec![0.0; num_actions];

        Self {
            weights,
            biases,
            feature_dim,
            num_actions,
        }
    }

    /// Forward pass: compute Q-values for all actions given state
    pub fn forward(&self, state: &[f64]) -> Vec<f64> {
        (0..self.num_actions)
            .map(|a| self.q_value(state, a))
            .collect()
    }

    /// Compute Q-value for a specific action
    pub fn q_value(&self, state: &[f64], action: usize) -> f64 {
        if action >= self.num_actions || state.len() != self.feature_dim {
            return 0.0;
        }

        let dot: f64 = state
            .iter()
            .zip(self.weights[action].iter())
            .map(|(s, w)| s * w)
            .sum();

        dot + self.biases[action]
    }

    /// Get best action for a state
    pub fn best_action(&self, state: &[f64]) -> usize {
        let q_values = self.forward(state);
        q_values
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Update weights using gradient descent
    pub fn update(&mut self, state: &[f64], action: usize, target: f64, learning_rate: f64) {
        if action >= self.num_actions || state.len() != self.feature_dim {
            return;
        }

        let prediction = self.q_value(state, action);
        let error = target - prediction;

        // Gradient descent update: w += lr * error * state
        for (i, &s) in state.iter().enumerate() {
            self.weights[action][i] += learning_rate * error * s;
        }
        self.biases[action] += learning_rate * error;
    }

    /// Copy weights from another network
    pub fn copy_from(&mut self, other: &LinearQNetwork) {
        self.weights = other.weights.clone();
        self.biases = other.biases.clone();
    }

    /// Serialize weights to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Write dimensions
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.feature_dim as u32).to_le_bytes());

        // Write weights
        for action_weights in &self.weights {
            for &w in action_weights {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }

        // Write biases
        for &b in &self.biases {
            bytes.extend_from_slice(&b.to_le_bytes());
        }

        bytes
    }

    /// Deserialize weights from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            anyhow::bail!("Invalid network bytes");
        }

        let num_actions = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let feature_dim = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

        let expected_len = 8 + (num_actions * feature_dim + num_actions) * 8;
        if bytes.len() < expected_len {
            anyhow::bail!("Invalid network bytes length");
        }

        let mut offset = 8;
        let mut weights = Vec::with_capacity(num_actions);

        for _ in 0..num_actions {
            let mut action_weights = Vec::with_capacity(feature_dim);
            for _ in 0..feature_dim {
                let w = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                action_weights.push(w);
                offset += 8;
            }
            weights.push(action_weights);
        }

        let mut biases = Vec::with_capacity(num_actions);
        for _ in 0..num_actions {
            let b = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            biases.push(b);
            offset += 8;
        }

        Ok(Self {
            weights,
            biases,
            feature_dim,
            num_actions,
        })
    }
}

/// DQN Agent
pub struct DqnAgent {
    config: DqnConfig,
    /// Online Q-network (updated every step)
    q_network: LinearQNetwork,
    /// Target Q-network (updated periodically)
    target_network: LinearQNetwork,
    /// Experience replay buffer
    replay_buffer: VecDeque<Experience>,
    /// Database connection for persistence
    conn: Connection,
    /// Current exploration rate
    current_epsilon: f64,
    /// Step counter
    step_count: u64,
    /// Episode counter
    episode_count: u64,
}

impl DqnAgent {
    pub fn new(db_path: &Path, config: DqnConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let current_epsilon = config.exploration_rate;

        let q_network = LinearQNetwork::new(config.feature_dim, config.num_actions);
        let target_network = LinearQNetwork::new(config.feature_dim, config.num_actions);

        let mut agent = Self {
            q_network,
            target_network,
            replay_buffer: VecDeque::with_capacity(config.replay_buffer_size),
            conn,
            current_epsilon,
            step_count: 0,
            episode_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_network()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS dqn_network (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                q_network BLOB NOT NULL,
                target_network BLOB NOT NULL,
                step_count INTEGER NOT NULL DEFAULT 0,
                episode_count INTEGER NOT NULL DEFAULT 0,
                epsilon REAL NOT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS dqn_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                total_reward REAL NOT NULL,
                steps INTEGER NOT NULL,
                final_epsilon REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_network(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, Vec<u8>, u64, u64, f64)> = self.conn.query_row(
            "SELECT q_network, target_network, step_count, episode_count, epsilon FROM dqn_network WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        );

        if let Ok((q_bytes, target_bytes, steps, episodes, epsilon)) = result {
            self.q_network = LinearQNetwork::from_bytes(&q_bytes)?;
            self.target_network = LinearQNetwork::from_bytes(&target_bytes)?;
            self.step_count = steps;
            self.episode_count = episodes;
            self.current_epsilon = epsilon;
        }

        Ok(())
    }

    fn save_network(&self) -> Result<()> {
        let q_bytes = self.q_network.to_bytes();
        let target_bytes = self.target_network.to_bytes();

        self.conn.execute(
            r#"
            INSERT INTO dqn_network (id, q_network, target_network, step_count, episode_count, epsilon)
            VALUES (1, ?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                q_network = ?1,
                target_network = ?2,
                step_count = ?3,
                episode_count = ?4,
                epsilon = ?5,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![q_bytes, target_bytes, self.step_count, self.episode_count, self.current_epsilon],
        )?;

        Ok(())
    }

    /// Select action using epsilon-greedy
    pub fn select_action(&self, state: &[f64]) -> usize {
        if rand::random::<f64>() < self.current_epsilon {
            // Random exploration
            (rand::random::<f64>() * self.config.num_actions as f64) as usize
        } else {
            // Greedy exploitation
            self.q_network.best_action(state)
        }
    }

    /// Store experience in replay buffer
    pub fn store_experience(&mut self, experience: Experience) {
        if self.replay_buffer.len() >= self.config.replay_buffer_size {
            self.replay_buffer.pop_front();
        }
        self.replay_buffer.push_back(experience);
    }

    /// Train on a mini-batch from replay buffer
    pub fn train_step(&mut self) -> Result<Option<f64>> {
        if self.replay_buffer.len() < self.config.batch_size {
            return Ok(None);
        }

        // Sample random mini-batch
        let batch: Vec<Experience> = (0..self.config.batch_size)
            .map(|_| {
                let idx = (rand::random::<f64>() * self.replay_buffer.len() as f64) as usize;
                self.replay_buffer[idx.min(self.replay_buffer.len() - 1)].clone()
            })
            .collect();

        let mut total_loss = 0.0;

        for exp in batch {
            // Compute target Q-value
            let target = if exp.done {
                exp.reward
            } else {
                // Double DQN: use online network to select action, target network to evaluate
                let best_action = self.q_network.best_action(&exp.next_state);
                exp.reward + self.config.discount_factor * self.target_network.q_value(&exp.next_state, best_action)
            };

            // Compute loss for logging
            let prediction = self.q_network.q_value(&exp.state, exp.action);
            total_loss += (target - prediction).powi(2);

            // Update online network
            self.q_network.update(&exp.state, exp.action, target, self.config.learning_rate);
        }

        self.step_count += 1;

        // Update target network periodically
        if self.step_count % self.config.target_update_freq as u64 == 0 {
            self.target_network.copy_from(&self.q_network);
        }

        // Decay epsilon
        self.current_epsilon = (self.current_epsilon * self.config.exploration_decay)
            .max(self.config.min_exploration_rate);

        Ok(Some(total_loss / self.config.batch_size as f64))
    }

    /// Run one complete step: select action, store experience, train
    pub fn step(
        &mut self,
        state: Vec<f64>,
        action: usize,
        reward: f64,
        next_state: Vec<f64>,
        done: bool,
    ) -> Result<Option<f64>> {
        self.store_experience(Experience {
            state,
            action,
            reward,
            next_state,
            done,
        });

        self.train_step()
    }

    /// End episode and save state
    pub fn end_episode(&mut self, total_reward: f64, steps: usize) -> Result<()> {
        self.episode_count += 1;

        self.conn.execute(
            "INSERT INTO dqn_episodes (total_reward, steps, final_epsilon) VALUES (?1, ?2, ?3)",
            params![total_reward, steps as i64, self.current_epsilon],
        )?;

        self.save_network()?;

        Ok(())
    }

    /// Get Q-values for a state
    pub fn get_q_values(&self, state: &[f64]) -> Vec<f64> {
        self.q_network.forward(state)
    }

    pub fn exploration_rate(&self) -> f64 {
        self.current_epsilon
    }

    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn replay_buffer_size(&self) -> usize {
        self.replay_buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_linear_network() {
        let net = LinearQNetwork::new(4, 3);

        let state = vec![1.0, 0.0, 1.0, 0.5];
        let q_values = net.forward(&state);

        assert_eq!(q_values.len(), 3);
    }

    #[test]
    fn test_network_update() {
        let mut net = LinearQNetwork::new(4, 3);

        let state = vec![1.0, 1.0, 1.0, 1.0];
        let initial_q = net.q_value(&state, 0);

        // Update towards higher target
        net.update(&state, 0, 10.0, 0.1);

        let new_q = net.q_value(&state, 0);
        assert!(new_q > initial_q);
    }

    #[test]
    fn test_network_serialization() {
        let net = LinearQNetwork::new(4, 3);
        let bytes = net.to_bytes();
        let restored = LinearQNetwork::from_bytes(&bytes).unwrap();

        assert_eq!(net.num_actions, restored.num_actions);
        assert_eq!(net.feature_dim, restored.feature_dim);
    }

    #[test]
    fn test_dqn_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("dqn.db");
        let config = DqnConfig {
            feature_dim: 4,
            num_actions: 3,
            batch_size: 4,
            ..Default::default()
        };

        let mut agent = DqnAgent::new(&db_path, config).unwrap();

        // Add experiences
        for i in 0..10 {
            let state = vec![i as f64 / 10.0; 4];
            let next_state = vec![(i + 1) as f64 / 10.0; 4];
            agent.step(state, i % 3, 1.0, next_state, i == 9).unwrap();
        }

        assert!(agent.replay_buffer_size() > 0);
    }

    #[test]
    fn test_experience_replay() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("dqn.db");
        let config = DqnConfig {
            feature_dim: 4,
            num_actions: 3,
            batch_size: 4,
            replay_buffer_size: 100,
            ..Default::default()
        };

        let mut agent = DqnAgent::new(&db_path, config).unwrap();

        // Fill buffer
        for i in 0..50 {
            agent.store_experience(Experience {
                state: vec![i as f64; 4],
                action: i % 3,
                reward: 1.0,
                next_state: vec![(i + 1) as f64; 4],
                done: false,
            });
        }

        // Should be able to train now
        let loss = agent.train_step().unwrap();
        assert!(loss.is_some());
    }
}
