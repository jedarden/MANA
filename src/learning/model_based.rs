//! Model-Based Reinforcement Learning
//!
//! Instead of learning from direct experience only, model-based RL:
//! 1. Learns a model of the environment (transition dynamics + reward)
//! 2. Plans using the learned model (can simulate without real interactions)
//! 3. Often more sample efficient than model-free methods
//!
//! This implementation includes:
//! - Linear dynamics model: s' = As + Ba + c
//! - Reward model: r = f(s, a)
//! - Model Predictive Control (MPC) for planning

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

/// Model-Based RL Configuration
#[derive(Debug, Clone)]
pub struct ModelBasedConfig {
    /// State dimension
    pub state_dim: usize,
    /// Number of actions
    pub num_actions: usize,
    /// Model learning rate
    pub model_lr: f64,
    /// Planning horizon for MPC
    pub planning_horizon: usize,
    /// Number of candidate sequences for MPC
    pub num_candidates: usize,
    /// Top-k candidates to keep (for CEM-style planning)
    pub elite_fraction: f64,
    /// Discount factor
    pub discount_factor: f64,
    /// Exploration noise for action selection
    pub exploration_noise: f64,
}

impl Default for ModelBasedConfig {
    fn default() -> Self {
        Self {
            state_dim: 64,
            num_actions: 10,
            model_lr: 0.01,
            planning_horizon: 10,
            num_candidates: 100,
            elite_fraction: 0.1,
            discount_factor: 0.99,
            exploration_noise: 0.1,
        }
    }
}

/// Experience tuple for model learning
#[derive(Debug, Clone)]
pub struct Transition {
    pub state: Vec<f64>,
    pub action: usize,
    pub reward: f64,
    pub next_state: Vec<f64>,
    pub done: bool,
}

/// Linear dynamics model: s' = A[a] * s + b[a]
/// Separate parameters for each action (action-conditional dynamics)
#[derive(Debug, Clone)]
struct DynamicsModel {
    /// Transition matrices for each action: [num_actions][state_dim][state_dim]
    transition_matrices: Vec<Vec<Vec<f64>>>,
    /// Bias vectors for each action
    biases: Vec<Vec<f64>>,
    state_dim: usize,
    num_actions: usize,
}

impl DynamicsModel {
    fn new(state_dim: usize, num_actions: usize) -> Self {
        // Initialize with identity-like matrices (predict same state)
        let transition_matrices: Vec<Vec<Vec<f64>>> = (0..num_actions)
            .map(|_| {
                (0..state_dim)
                    .map(|i| {
                        (0..state_dim)
                            .map(|j| if i == j { 0.95 } else { 0.0 })
                            .collect()
                    })
                    .collect()
            })
            .collect();

        let biases: Vec<Vec<f64>> = (0..num_actions)
            .map(|_| vec![0.0; state_dim])
            .collect();

        Self {
            transition_matrices,
            biases,
            state_dim,
            num_actions,
        }
    }

    /// Predict next state
    fn predict(&self, state: &[f64], action: usize) -> Vec<f64> {
        if action >= self.num_actions || state.len() != self.state_dim {
            return state.to_vec();
        }

        let matrix = &self.transition_matrices[action];
        let bias = &self.biases[action];

        (0..self.state_dim)
            .map(|i| {
                let dot: f64 = state.iter()
                    .zip(&matrix[i])
                    .map(|(s, m)| s * m)
                    .sum();
                dot + bias[i]
            })
            .collect()
    }

    /// Update model from transition
    fn update(&mut self, state: &[f64], action: usize, next_state: &[f64], lr: f64) {
        if action >= self.num_actions || state.len() != self.state_dim || next_state.len() != self.state_dim {
            return;
        }

        let predicted = self.predict(state, action);

        // Gradient descent on MSE
        for i in 0..self.state_dim {
            let error = next_state[i] - predicted[i];

            // Update matrix row
            for j in 0..self.state_dim {
                self.transition_matrices[action][i][j] += lr * error * state[j];
            }

            // Update bias
            self.biases[action][i] += lr * error;
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&(self.state_dim as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());

        for matrix in &self.transition_matrices {
            for row in matrix {
                for &val in row {
                    bytes.extend_from_slice(&val.to_le_bytes());
                }
            }
        }

        for bias in &self.biases {
            for &val in bias {
                bytes.extend_from_slice(&val.to_le_bytes());
            }
        }

        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            anyhow::bail!("Invalid dynamics model bytes");
        }

        let state_dim = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let num_actions = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

        let mut offset = 8;

        let mut transition_matrices = Vec::new();
        for _ in 0..num_actions {
            let mut matrix = Vec::new();
            for _ in 0..state_dim {
                let mut row = Vec::new();
                for _ in 0..state_dim {
                    let val = f64::from_le_bytes([
                        bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                        bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                    ]);
                    row.push(val);
                    offset += 8;
                }
                matrix.push(row);
            }
            transition_matrices.push(matrix);
        }

        let mut biases = Vec::new();
        for _ in 0..num_actions {
            let mut bias = Vec::new();
            for _ in 0..state_dim {
                let val = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                bias.push(val);
                offset += 8;
            }
            biases.push(bias);
        }

        Ok(Self {
            transition_matrices,
            biases,
            state_dim,
            num_actions,
        })
    }
}

/// Linear reward model: r = w[a] · s + b[a]
#[derive(Debug, Clone)]
struct RewardModel {
    /// Weights for each action
    weights: Vec<Vec<f64>>,
    /// Biases for each action
    biases: Vec<f64>,
    state_dim: usize,
    num_actions: usize,
}

impl RewardModel {
    fn new(state_dim: usize, num_actions: usize) -> Self {
        let weights: Vec<Vec<f64>> = (0..num_actions)
            .map(|_| vec![0.0; state_dim])
            .collect();
        let biases = vec![0.0; num_actions];

        Self {
            weights,
            biases,
            state_dim,
            num_actions,
        }
    }

    /// Predict reward
    fn predict(&self, state: &[f64], action: usize) -> f64 {
        if action >= self.num_actions || state.len() != self.state_dim {
            return 0.0;
        }

        let dot: f64 = state.iter()
            .zip(&self.weights[action])
            .map(|(s, w)| s * w)
            .sum();

        dot + self.biases[action]
    }

    /// Update from experience
    fn update(&mut self, state: &[f64], action: usize, reward: f64, lr: f64) {
        if action >= self.num_actions || state.len() != self.state_dim {
            return;
        }

        let predicted = self.predict(state, action);
        let error = reward - predicted;

        for (i, &s) in state.iter().enumerate() {
            self.weights[action][i] += lr * error * s;
        }
        self.biases[action] += lr * error;
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&(self.state_dim as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());

        for weights in &self.weights {
            for &w in weights {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }

        for &b in &self.biases {
            bytes.extend_from_slice(&b.to_le_bytes());
        }

        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            anyhow::bail!("Invalid reward model bytes");
        }

        let state_dim = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let num_actions = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

        let mut offset = 8;

        let mut weights = Vec::new();
        for _ in 0..num_actions {
            let mut w = Vec::new();
            for _ in 0..state_dim {
                let val = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                w.push(val);
                offset += 8;
            }
            weights.push(w);
        }

        let mut biases = Vec::new();
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
            state_dim,
            num_actions,
        })
    }
}

/// Model-Based RL Agent with MPC Planning
pub struct ModelBasedAgent {
    config: ModelBasedConfig,
    dynamics_model: DynamicsModel,
    reward_model: RewardModel,
    /// Experience buffer for model learning
    experience_buffer: Vec<Transition>,
    conn: Connection,
    episode_count: u64,
    step_count: u64,
}

impl ModelBasedAgent {
    pub fn new(db_path: &Path, config: ModelBasedConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let dynamics_model = DynamicsModel::new(config.state_dim, config.num_actions);
        let reward_model = RewardModel::new(config.state_dim, config.num_actions);

        let mut agent = Self {
            dynamics_model,
            reward_model,
            experience_buffer: Vec::new(),
            conn,
            episode_count: 0,
            step_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_models()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS mb_models (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                dynamics_bytes BLOB NOT NULL,
                reward_bytes BLOB NOT NULL,
                episode_count INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS mb_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                total_reward REAL NOT NULL,
                steps INTEGER NOT NULL,
                model_error REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_models(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, Vec<u8>, u64, u64)> = self.conn.query_row(
            "SELECT dynamics_bytes, reward_bytes, episode_count, step_count FROM mb_models WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        );

        if let Ok((dyn_bytes, rew_bytes, episodes, steps)) = result {
            self.dynamics_model = DynamicsModel::from_bytes(&dyn_bytes)?;
            self.reward_model = RewardModel::from_bytes(&rew_bytes)?;
            self.episode_count = episodes;
            self.step_count = steps;
        }

        Ok(())
    }

    fn save_models(&self) -> Result<()> {
        let dyn_bytes = self.dynamics_model.to_bytes();
        let rew_bytes = self.reward_model.to_bytes();

        self.conn.execute(
            r#"
            INSERT INTO mb_models (id, dynamics_bytes, reward_bytes, episode_count, step_count)
            VALUES (1, ?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                dynamics_bytes = ?1,
                reward_bytes = ?2,
                episode_count = ?3,
                step_count = ?4,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![dyn_bytes, rew_bytes, self.episode_count, self.step_count],
        )?;

        Ok(())
    }

    /// Store transition and update models
    pub fn store_transition(&mut self, transition: Transition) {
        // Update models immediately (online learning)
        self.dynamics_model.update(
            &transition.state,
            transition.action,
            &transition.next_state,
            self.config.model_lr,
        );

        self.reward_model.update(
            &transition.state,
            transition.action,
            transition.reward,
            self.config.model_lr,
        );

        self.experience_buffer.push(transition);
        self.step_count += 1;

        // Keep buffer bounded
        while self.experience_buffer.len() > 10000 {
            self.experience_buffer.remove(0);
        }
    }

    /// Model Predictive Control: plan using learned model
    pub fn select_action(&self, state: &[f64]) -> usize {
        // Generate random action sequences
        let mut candidates: Vec<(Vec<usize>, f64)> = (0..self.config.num_candidates)
            .map(|_| {
                let actions: Vec<usize> = (0..self.config.planning_horizon)
                    .map(|_| (rand::random::<f64>() * self.config.num_actions as f64) as usize)
                    .collect();
                (actions, 0.0)
            })
            .collect();

        // Evaluate each sequence
        for (actions, total_reward) in &mut candidates {
            let mut sim_state = state.to_vec();
            let mut discount = 1.0;

            for &action in actions.iter() {
                let reward = self.reward_model.predict(&sim_state, action);
                *total_reward += discount * reward;
                discount *= self.config.discount_factor;

                sim_state = self.dynamics_model.predict(&sim_state, action);
            }
        }

        // Select first action from best sequence
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let best_sequence = &candidates[0].0;
        let action = best_sequence[0];

        // Add exploration noise
        if rand::random::<f64>() < self.config.exploration_noise {
            (rand::random::<f64>() * self.config.num_actions as f64) as usize
        } else {
            action
        }
    }

    /// Cross-Entropy Method planning (more sophisticated than random shooting)
    pub fn select_action_cem(&self, state: &[f64], iterations: usize) -> usize {
        let horizon = self.config.planning_horizon;
        let num_actions = self.config.num_actions;

        // Initialize action distribution (uniform)
        let mut action_probs: Vec<Vec<f64>> = (0..horizon)
            .map(|_| vec![1.0 / num_actions as f64; num_actions])
            .collect();

        for _ in 0..iterations {
            // Sample action sequences from current distribution
            let mut candidates: Vec<(Vec<usize>, f64)> = (0..self.config.num_candidates)
                .map(|_| {
                    let actions: Vec<usize> = (0..horizon)
                        .map(|t| {
                            let r = rand::random::<f64>();
                            let mut cumsum = 0.0;
                            for (a, &p) in action_probs[t].iter().enumerate() {
                                cumsum += p;
                                if r < cumsum {
                                    return a;
                                }
                            }
                            num_actions - 1
                        })
                        .collect();
                    (actions, 0.0)
                })
                .collect();

            // Evaluate
            for (actions, total_reward) in &mut candidates {
                let mut sim_state = state.to_vec();
                let mut discount = 1.0;

                for &action in actions.iter() {
                    let reward = self.reward_model.predict(&sim_state, action);
                    *total_reward += discount * reward;
                    discount *= self.config.discount_factor;
                    sim_state = self.dynamics_model.predict(&sim_state, action);
                }
            }

            // Sort and select elite
            candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let elite_count = (self.config.elite_fraction * self.config.num_candidates as f64) as usize;
            let elite_count = elite_count.max(1);

            // Update distribution based on elite samples
            for t in 0..horizon {
                let mut counts = vec![0.0; num_actions];
                for (actions, _) in candidates.iter().take(elite_count) {
                    counts[actions[t]] += 1.0;
                }

                // Normalize with smoothing
                let total: f64 = counts.iter().sum();
                for (a, count) in counts.iter().enumerate() {
                    action_probs[t][a] = 0.8 * (count / total) + 0.2 * action_probs[t][a];
                }
            }
        }

        // Return most likely first action
        action_probs[0]
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Predict next state using learned model
    pub fn predict_next_state(&self, state: &[f64], action: usize) -> Vec<f64> {
        self.dynamics_model.predict(state, action)
    }

    /// Predict reward using learned model
    pub fn predict_reward(&self, state: &[f64], action: usize) -> f64 {
        self.reward_model.predict(state, action)
    }

    /// End episode
    pub fn end_episode(&mut self, total_reward: f64) -> Result<()> {
        // Compute model error on recent experiences
        let model_error = if self.experience_buffer.is_empty() {
            0.0
        } else {
            let recent: Vec<_> = self.experience_buffer.iter().rev().take(100).collect();
            let mut total_error = 0.0;

            for t in &recent {
                // Dynamics error
                let predicted_state = self.dynamics_model.predict(&t.state, t.action);
                let dyn_error: f64 = predicted_state.iter()
                    .zip(&t.next_state)
                    .map(|(p, a)| (p - a).powi(2))
                    .sum();

                // Reward error
                let predicted_reward = self.reward_model.predict(&t.state, t.action);
                let rew_error = (predicted_reward - t.reward).powi(2);

                total_error += dyn_error + rew_error;
            }

            total_error / recent.len() as f64
        };

        self.episode_count += 1;

        self.conn.execute(
            "INSERT INTO mb_episodes (total_reward, steps, model_error) VALUES (?1, ?2, ?3)",
            params![total_reward, self.experience_buffer.len() as i64, model_error],
        )?;

        self.save_models()?;

        Ok(())
    }

    /// Train on experience buffer (offline learning)
    pub fn train(&mut self, epochs: usize) {
        for _ in 0..epochs {
            for t in &self.experience_buffer.clone() {
                self.dynamics_model.update(
                    &t.state,
                    t.action,
                    &t.next_state,
                    self.config.model_lr,
                );
                self.reward_model.update(
                    &t.state,
                    t.action,
                    t.reward,
                    self.config.model_lr,
                );
            }
        }
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    pub fn buffer_size(&self) -> usize {
        self.experience_buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_dynamics_model() {
        let mut model = DynamicsModel::new(4, 3);

        let state = vec![1.0, 2.0, 3.0, 4.0];
        let next_state = vec![1.1, 2.1, 3.1, 4.1];

        // Train
        for _ in 0..100 {
            model.update(&state, 0, &next_state, 0.01);
        }

        // Predict should be close to next_state
        let predicted = model.predict(&state, 0);
        let error: f64 = predicted.iter()
            .zip(&next_state)
            .map(|(p, n)| (p - n).powi(2))
            .sum();

        assert!(error < 1.0);
    }

    #[test]
    fn test_reward_model() {
        let mut model = RewardModel::new(4, 3);

        let state = vec![1.0, 0.0, 1.0, 0.0];
        let reward = 5.0;

        // Train
        for _ in 0..100 {
            model.update(&state, 0, reward, 0.1);
        }

        let predicted = model.predict(&state, 0);
        assert!((predicted - reward).abs() < 1.0);
    }

    #[test]
    fn test_model_based_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mb.db");
        let config = ModelBasedConfig {
            state_dim: 4,
            num_actions: 3,
            planning_horizon: 3,
            num_candidates: 10,
            ..Default::default()
        };

        let mut agent = ModelBasedAgent::new(&db_path, config).unwrap();

        // Store some transitions
        for i in 0..10 {
            let state = vec![i as f64; 4];
            let action = i % 3;
            let next_state = vec![(i + 1) as f64; 4];

            agent.store_transition(Transition {
                state,
                action,
                reward: 1.0,
                next_state,
                done: false,
            });
        }

        // Select action
        let state = vec![5.0; 4];
        let action = agent.select_action(&state);
        assert!(action < 3);
    }

    #[test]
    fn test_cem_planning() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mb.db");
        let config = ModelBasedConfig {
            state_dim: 4,
            num_actions: 3,
            planning_horizon: 3,
            num_candidates: 20,
            elite_fraction: 0.2,
            ..Default::default()
        };

        let mut agent = ModelBasedAgent::new(&db_path, config).unwrap();

        // Store transitions where action 0 gives better reward
        for _ in 0..50 {
            let state: Vec<f64> = (0..4).map(|_| rand::random::<f64>()).collect();

            for action in 0..3 {
                let reward = if action == 0 { 10.0 } else { 1.0 };
                let next_state = state.clone();

                agent.store_transition(Transition {
                    state: state.clone(),
                    action,
                    reward,
                    next_state,
                    done: false,
                });
            }
        }

        // Train model
        agent.train(10);

        // CEM should prefer action 0
        let state = vec![0.5; 4];
        let action = agent.select_action_cem(&state, 3);

        // Action should be valid
        assert!(action < 3);
    }

    #[test]
    fn test_serialization() {
        let model = DynamicsModel::new(4, 3);
        let bytes = model.to_bytes();
        let restored = DynamicsModel::from_bytes(&bytes).unwrap();

        assert_eq!(model.state_dim, restored.state_dim);
        assert_eq!(model.num_actions, restored.num_actions);

        let rew_model = RewardModel::new(4, 3);
        let bytes = rew_model.to_bytes();
        let restored = RewardModel::from_bytes(&bytes).unwrap();

        assert_eq!(rew_model.state_dim, restored.state_dim);
    }
}
