//! Decision Transformer - Sequence Modeling for RL
//!
//! Decision Transformer frames RL as sequence modeling:
//! Given a desired return, past states, actions, and rewards,
//! predict the next action that achieves that return.
//!
//! Input sequence: (R_1, s_1, a_1, R_2, s_2, a_2, ..., R_t, s_t)
//! Output: a_t
//!
//! This is a simplified implementation using linear layers instead of
//! a full transformer architecture, but captures the core idea of
//! return-conditioned action prediction.

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::VecDeque;
use std::path::Path;

/// Decision Transformer Configuration
#[derive(Debug, Clone)]
pub struct DecisionTransformerConfig {
    /// Context length (number of timesteps to consider)
    pub context_length: usize,
    /// State dimension
    pub state_dim: usize,
    /// Number of actions
    pub num_actions: usize,
    /// Hidden dimension
    pub hidden_dim: usize,
    /// Learning rate
    pub learning_rate: f64,
    /// Target return scale (normalize returns to this range)
    pub return_scale: f64,
}

impl Default for DecisionTransformerConfig {
    fn default() -> Self {
        Self {
            context_length: 20,
            state_dim: 64,
            num_actions: 10,
            hidden_dim: 128,
            learning_rate: 0.001,
            return_scale: 100.0,
        }
    }
}

/// A timestep in the trajectory
#[derive(Debug, Clone)]
pub struct Timestep {
    pub return_to_go: f64,
    pub state: Vec<f64>,
    pub action: Option<usize>,
    pub reward: f64,
}

/// Trajectory for training
#[derive(Debug, Clone)]
pub struct Trajectory {
    pub timesteps: Vec<Timestep>,
    pub total_return: f64,
}

impl Trajectory {
    pub fn new() -> Self {
        Self {
            timesteps: Vec::new(),
            total_return: 0.0,
        }
    }

    /// Add a timestep and compute return-to-go
    pub fn add(&mut self, state: Vec<f64>, action: usize, reward: f64) {
        self.total_return += reward;
        self.timesteps.push(Timestep {
            return_to_go: 0.0, // Will be computed at finalization
            state,
            action: Some(action),
            reward,
        });
    }

    /// Finalize trajectory by computing return-to-go values
    pub fn finalize(&mut self) {
        let n = self.timesteps.len();
        if n == 0 {
            return;
        }

        // Compute return-to-go backwards
        let mut rtg = 0.0;
        for t in (0..n).rev() {
            rtg += self.timesteps[t].reward;
            self.timesteps[t].return_to_go = rtg;
        }
    }
}

/// Linear sequence model (simplified transformer)
/// Projects (return, state) sequences to action predictions
#[derive(Debug, Clone)]
struct SequenceModel {
    /// Input projection: (return + state) -> hidden
    input_weights: Vec<Vec<f64>>,
    input_bias: Vec<f64>,
    /// Temporal aggregation weights (simplified attention)
    temporal_weights: Vec<f64>,
    /// Output projection: hidden -> actions
    output_weights: Vec<Vec<f64>>,
    output_bias: Vec<f64>,
    config: DecisionTransformerConfig,
}

impl SequenceModel {
    fn new(config: DecisionTransformerConfig) -> Self {
        let input_dim = 1 + config.state_dim; // return + state

        // Initialize weights with small random values
        let input_weights: Vec<Vec<f64>> = (0..config.hidden_dim)
            .map(|_| (0..input_dim).map(|_| (rand::random::<f64>() - 0.5) * 0.1).collect())
            .collect();
        let input_bias = vec![0.0; config.hidden_dim];

        let temporal_weights: Vec<f64> = (0..config.context_length)
            .map(|i| 1.0 / (config.context_length - i) as f64) // More recent = higher weight
            .collect();

        let output_weights: Vec<Vec<f64>> = (0..config.num_actions)
            .map(|_| (0..config.hidden_dim).map(|_| (rand::random::<f64>() - 0.5) * 0.1).collect())
            .collect();
        let output_bias = vec![0.0; config.num_actions];

        Self {
            input_weights,
            input_bias,
            temporal_weights,
            output_weights,
            output_bias,
            config,
        }
    }

    /// Forward pass: given context, predict action logits
    fn forward(&self, context: &[Timestep]) -> Vec<f64> {
        if context.is_empty() {
            return vec![0.0; self.config.num_actions];
        }

        // Process each timestep
        let mut aggregated_hidden = vec![0.0; self.config.hidden_dim];

        for (t, timestep) in context.iter().enumerate() {
            // Build input: [normalized_return, state...]
            let normalized_return = timestep.return_to_go / self.config.return_scale;
            let mut input = vec![normalized_return];
            input.extend(&timestep.state);

            // Pad or truncate state
            while input.len() < 1 + self.config.state_dim {
                input.push(0.0);
            }
            input.truncate(1 + self.config.state_dim);

            // Project to hidden
            let mut hidden = vec![0.0; self.config.hidden_dim];
            for (h, (weights, &bias)) in self.input_weights.iter().zip(&self.input_bias).enumerate() {
                let dot: f64 = input.iter().zip(weights).map(|(i, w)| i * w).sum();
                hidden[h] = (dot + bias).tanh(); // tanh activation
            }

            // Apply temporal weight
            let weight = if t < self.temporal_weights.len() {
                self.temporal_weights[t]
            } else {
                0.1
            };

            for (i, &h) in hidden.iter().enumerate() {
                aggregated_hidden[i] += weight * h;
            }
        }

        // Normalize by weight sum
        let weight_sum: f64 = self.temporal_weights.iter().take(context.len()).sum();
        if weight_sum > 0.0 {
            for h in &mut aggregated_hidden {
                *h /= weight_sum;
            }
        }

        // Project to action logits
        let mut logits = vec![0.0; self.config.num_actions];
        for (a, (weights, &bias)) in self.output_weights.iter().zip(&self.output_bias).enumerate() {
            let dot: f64 = aggregated_hidden.iter().zip(weights).map(|(h, w)| h * w).sum();
            logits[a] = dot + bias;
        }

        logits
    }

    /// Convert logits to action probabilities
    fn action_probs(&self, logits: &[f64]) -> Vec<f64> {
        let max_logit = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exp_logits: Vec<f64> = logits.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum_exp: f64 = exp_logits.iter().sum();
        exp_logits.iter().map(|&e| e / sum_exp).collect()
    }

    /// Sample action from distribution
    fn sample_action(&self, context: &[Timestep]) -> usize {
        let logits = self.forward(context);
        let probs = self.action_probs(&logits);

        let r: f64 = rand::random();
        let mut cumsum = 0.0;
        for (action, &prob) in probs.iter().enumerate() {
            cumsum += prob;
            if r < cumsum {
                return action;
            }
        }
        self.config.num_actions - 1
    }

    /// Greedy action selection
    fn best_action(&self, context: &[Timestep]) -> usize {
        let logits = self.forward(context);
        logits
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Update model on a trajectory segment
    fn update(&mut self, context: &[Timestep], target_action: usize) -> f64 {
        let logits = self.forward(context);
        let probs = self.action_probs(&logits);

        // Cross-entropy loss gradient
        let mut grad_logits = probs.clone();
        grad_logits[target_action] -= 1.0;

        let loss = -probs[target_action].max(1e-10).ln();

        // Backprop through output layer (simplified)
        // Build aggregated hidden (same as forward pass)
        let mut aggregated_hidden = vec![0.0; self.config.hidden_dim];
        for (t, timestep) in context.iter().enumerate() {
            let normalized_return = timestep.return_to_go / self.config.return_scale;
            let mut input = vec![normalized_return];
            input.extend(&timestep.state);
            while input.len() < 1 + self.config.state_dim {
                input.push(0.0);
            }
            input.truncate(1 + self.config.state_dim);

            let mut hidden = vec![0.0; self.config.hidden_dim];
            for (h, (weights, &bias)) in self.input_weights.iter().zip(&self.input_bias).enumerate() {
                let dot: f64 = input.iter().zip(weights).map(|(i, w)| i * w).sum();
                hidden[h] = (dot + bias).tanh();
            }

            let weight = if t < self.temporal_weights.len() {
                self.temporal_weights[t]
            } else {
                0.1
            };

            for (i, &h) in hidden.iter().enumerate() {
                aggregated_hidden[i] += weight * h;
            }
        }

        let weight_sum: f64 = self.temporal_weights.iter().take(context.len()).sum();
        if weight_sum > 0.0 {
            for h in &mut aggregated_hidden {
                *h /= weight_sum;
            }
        }

        // Update output weights
        for (a, grad) in grad_logits.iter().enumerate() {
            for (h, &hidden) in aggregated_hidden.iter().enumerate() {
                self.output_weights[a][h] -= self.config.learning_rate * grad * hidden;
            }
            self.output_bias[a] -= self.config.learning_rate * grad;
        }

        loss
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Config
        bytes.extend_from_slice(&(self.config.context_length as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.config.state_dim as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.config.num_actions as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.config.hidden_dim as u32).to_le_bytes());
        bytes.extend_from_slice(&self.config.return_scale.to_le_bytes());

        // Input weights
        for row in &self.input_weights {
            for &w in row {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        for &b in &self.input_bias {
            bytes.extend_from_slice(&b.to_le_bytes());
        }

        // Temporal weights
        for &w in &self.temporal_weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }

        // Output weights
        for row in &self.output_weights {
            for &w in row {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        for &b in &self.output_bias {
            bytes.extend_from_slice(&b.to_le_bytes());
        }

        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 24 {
            anyhow::bail!("Invalid DT model bytes");
        }

        let mut offset = 0;

        let context_length = u32::from_le_bytes([bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3]]) as usize;
        offset += 4;
        let state_dim = u32::from_le_bytes([bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3]]) as usize;
        offset += 4;
        let num_actions = u32::from_le_bytes([bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3]]) as usize;
        offset += 4;
        let hidden_dim = u32::from_le_bytes([bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3]]) as usize;
        offset += 4;
        let return_scale = f64::from_le_bytes([
            bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
            bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
        ]);
        offset += 8;

        let config = DecisionTransformerConfig {
            context_length,
            state_dim,
            num_actions,
            hidden_dim,
            return_scale,
            learning_rate: 0.001,
        };

        let input_dim = 1 + state_dim;

        // Read input weights
        let mut input_weights = Vec::new();
        for _ in 0..hidden_dim {
            let mut row = Vec::new();
            for _ in 0..input_dim {
                let w = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                row.push(w);
                offset += 8;
            }
            input_weights.push(row);
        }

        let mut input_bias = Vec::new();
        for _ in 0..hidden_dim {
            let b = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            input_bias.push(b);
            offset += 8;
        }

        let mut temporal_weights = Vec::new();
        for _ in 0..context_length {
            let w = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            temporal_weights.push(w);
            offset += 8;
        }

        let mut output_weights = Vec::new();
        for _ in 0..num_actions {
            let mut row = Vec::new();
            for _ in 0..hidden_dim {
                let w = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                row.push(w);
                offset += 8;
            }
            output_weights.push(row);
        }

        let mut output_bias = Vec::new();
        for _ in 0..num_actions {
            let b = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            output_bias.push(b);
            offset += 8;
        }

        Ok(Self {
            input_weights,
            input_bias,
            temporal_weights,
            output_weights,
            output_bias,
            config,
        })
    }
}

/// Decision Transformer Agent
pub struct DecisionTransformerAgent {
    config: DecisionTransformerConfig,
    model: SequenceModel,
    /// Current context window
    context: VecDeque<Timestep>,
    /// Target return for current episode
    target_return: f64,
    /// Current return-to-go
    current_rtg: f64,
    /// Trajectory buffer for training
    trajectories: Vec<Trajectory>,
    conn: Connection,
    episode_count: u64,
}

impl DecisionTransformerAgent {
    pub fn new(db_path: &Path, config: DecisionTransformerConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let model = SequenceModel::new(config.clone());

        let mut agent = Self {
            model,
            context: VecDeque::with_capacity(config.context_length),
            target_return: 100.0,
            current_rtg: 100.0,
            trajectories: Vec::new(),
            conn,
            episode_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_model()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS dt_model (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                model_bytes BLOB NOT NULL,
                episode_count INTEGER NOT NULL DEFAULT 0,
                avg_return REAL NOT NULL DEFAULT 0.0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS dt_trajectories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                total_return REAL NOT NULL,
                length INTEGER NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_model(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, u64)> = self.conn.query_row(
            "SELECT model_bytes, episode_count FROM dt_model WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        if let Ok((bytes, episodes)) = result {
            self.model = SequenceModel::from_bytes(&bytes)?;
            self.episode_count = episodes;
        }

        Ok(())
    }

    fn save_model(&self) -> Result<()> {
        let bytes = self.model.to_bytes();
        let avg_return: f64 = if self.trajectories.is_empty() {
            0.0
        } else {
            self.trajectories.iter().map(|t| t.total_return).sum::<f64>() / self.trajectories.len() as f64
        };

        self.conn.execute(
            r#"
            INSERT INTO dt_model (id, model_bytes, episode_count, avg_return)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                model_bytes = ?1,
                episode_count = ?2,
                avg_return = ?3,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![bytes, self.episode_count, avg_return],
        )?;

        Ok(())
    }

    /// Set target return for conditioning
    pub fn set_target_return(&mut self, target: f64) {
        self.target_return = target;
        self.current_rtg = target;
    }

    /// Start new episode
    pub fn start_episode(&mut self, target_return: f64) {
        self.context.clear();
        self.set_target_return(target_return);
    }

    /// Select action conditioned on target return
    pub fn select_action(&mut self, state: &[f64]) -> usize {
        // Add current timestep to context
        self.context.push_back(Timestep {
            return_to_go: self.current_rtg,
            state: state.to_vec(),
            action: None,
            reward: 0.0,
        });

        // Maintain context length
        while self.context.len() > self.config.context_length {
            self.context.pop_front();
        }

        // Get action from model
        let context_vec: Vec<Timestep> = self.context.iter().cloned().collect();
        self.model.best_action(&context_vec)
    }

    /// Record action and reward
    pub fn record_step(&mut self, action: usize, reward: f64) {
        if let Some(t) = self.context.back_mut() {
            t.action = Some(action);
            t.reward = reward;
        }

        // Update return-to-go
        self.current_rtg -= reward;
    }

    /// End episode and optionally train
    pub fn end_episode(&mut self, train: bool) -> Result<f64> {
        // Build trajectory from context
        let mut trajectory = Trajectory::new();
        for t in &self.context {
            if let Some(action) = t.action {
                trajectory.add(t.state.clone(), action, t.reward);
            }
        }
        trajectory.finalize();

        let total_return = trajectory.total_return;

        // Log trajectory
        self.conn.execute(
            "INSERT INTO dt_trajectories (total_return, length) VALUES (?1, ?2)",
            params![total_return, trajectory.timesteps.len() as i64],
        )?;

        if train && !trajectory.timesteps.is_empty() {
            self.trajectories.push(trajectory);

            // Train on recent trajectories
            self.train()?;
        }

        self.episode_count += 1;
        self.context.clear();
        self.save_model()?;

        Ok(total_return)
    }

    /// Train on collected trajectories
    fn train(&mut self) -> Result<()> {
        if self.trajectories.is_empty() {
            return Ok(());
        }

        // Sample and train on trajectory segments
        for traj in &self.trajectories {
            for start in 0..traj.timesteps.len() {
                let end = (start + self.config.context_length).min(traj.timesteps.len());
                let context: Vec<Timestep> = traj.timesteps[start..end].to_vec();

                if let Some(last) = context.last() {
                    if let Some(target_action) = last.action {
                        self.model.update(&context, target_action);
                    }
                }
            }
        }

        // Keep only recent trajectories
        while self.trajectories.len() > 100 {
            self.trajectories.remove(0);
        }

        Ok(())
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn target_return(&self) -> f64 {
        self.target_return
    }

    pub fn current_rtg(&self) -> f64 {
        self.current_rtg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_trajectory() {
        let mut traj = Trajectory::new();
        traj.add(vec![1.0, 0.0], 0, 1.0);
        traj.add(vec![0.0, 1.0], 1, 2.0);
        traj.add(vec![1.0, 1.0], 0, 3.0);
        traj.finalize();

        assert_eq!(traj.total_return, 6.0);
        assert_eq!(traj.timesteps[0].return_to_go, 6.0);
        assert_eq!(traj.timesteps[1].return_to_go, 5.0);
        assert_eq!(traj.timesteps[2].return_to_go, 3.0);
    }

    #[test]
    fn test_sequence_model() {
        let config = DecisionTransformerConfig {
            context_length: 5,
            state_dim: 4,
            num_actions: 3,
            hidden_dim: 8,
            ..Default::default()
        };

        let model = SequenceModel::new(config);

        let context = vec![
            Timestep { return_to_go: 10.0, state: vec![1.0, 0.0, 1.0, 0.0], action: Some(0), reward: 1.0 },
            Timestep { return_to_go: 9.0, state: vec![0.0, 1.0, 0.0, 1.0], action: Some(1), reward: 2.0 },
        ];

        let logits = model.forward(&context);
        assert_eq!(logits.len(), 3);

        let probs = model.action_probs(&logits);
        assert!((probs.iter().sum::<f64>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_decision_transformer_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("dt.db");
        let config = DecisionTransformerConfig {
            context_length: 5,
            state_dim: 4,
            num_actions: 3,
            hidden_dim: 8,
            ..Default::default()
        };

        let mut agent = DecisionTransformerAgent::new(&db_path, config).unwrap();

        // Run episode
        agent.start_episode(10.0);

        for i in 0..5 {
            let state = vec![i as f64 / 5.0; 4];
            let action = agent.select_action(&state);
            agent.record_step(action, 1.0);
        }

        let total = agent.end_episode(true).unwrap();
        assert!(total > 0.0);
    }

    #[test]
    fn test_model_serialization() {
        let config = DecisionTransformerConfig {
            context_length: 5,
            state_dim: 4,
            num_actions: 3,
            hidden_dim: 8,
            ..Default::default()
        };

        let model = SequenceModel::new(config.clone());
        let bytes = model.to_bytes();
        let restored = SequenceModel::from_bytes(&bytes).unwrap();

        assert_eq!(model.config.num_actions, restored.config.num_actions);
        assert_eq!(model.config.state_dim, restored.config.state_dim);
    }
}
