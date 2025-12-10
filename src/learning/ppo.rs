//! PPO (Proximal Policy Optimization) - Stable Policy Updates
//!
//! PPO improves on vanilla policy gradient by:
//! 1. Using a clipped surrogate objective to prevent large policy updates
//! 2. Multiple epochs of training on the same batch of data
//! 3. Better sample efficiency through importance sampling
//!
//! Objective: L^CLIP(θ) = E[min(r_t(θ)A_t, clip(r_t(θ), 1-ε, 1+ε)A_t)]
//! where r_t(θ) = π_θ(a_t|s_t) / π_θ_old(a_t|s_t)

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

/// PPO Configuration
#[derive(Debug, Clone)]
pub struct PpoConfig {
    /// Learning rate
    pub learning_rate: f64,
    /// Discount factor (gamma)
    pub discount_factor: f64,
    /// GAE lambda for advantage estimation
    pub gae_lambda: f64,
    /// Clipping parameter (epsilon)
    pub clip_epsilon: f64,
    /// Number of epochs per update
    pub epochs: usize,
    /// Mini-batch size
    pub batch_size: usize,
    /// Feature dimension
    pub feature_dim: usize,
    /// Number of actions
    pub num_actions: usize,
    /// Entropy coefficient
    pub entropy_coef: f64,
    /// Value loss coefficient
    pub value_coef: f64,
    /// Max gradient norm for clipping
    pub max_grad_norm: f64,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.0003,
            discount_factor: 0.99,
            gae_lambda: 0.95,
            clip_epsilon: 0.2,
            epochs: 4,
            batch_size: 64,
            feature_dim: 64,
            num_actions: 10,
            entropy_coef: 0.01,
            value_coef: 0.5,
            max_grad_norm: 0.5,
        }
    }
}

/// Rollout buffer entry
#[derive(Debug, Clone)]
struct RolloutEntry {
    state: Vec<f64>,
    action: usize,
    reward: f64,
    value: f64,
    log_prob: f64,
    done: bool,
}

/// Linear policy and value network (combined for PPO)
#[derive(Debug, Clone)]
struct PpoNetwork {
    /// Policy weights: [num_actions][feature_dim]
    policy_weights: Vec<Vec<f64>>,
    policy_biases: Vec<f64>,
    /// Value weights
    value_weights: Vec<f64>,
    value_bias: f64,
    feature_dim: usize,
    num_actions: usize,
}

impl PpoNetwork {
    fn new(feature_dim: usize, num_actions: usize) -> Self {
        let policy_weights: Vec<Vec<f64>> = (0..num_actions)
            .map(|_| (0..feature_dim).map(|_| (rand::random::<f64>() - 0.5) * 0.1).collect())
            .collect();
        let policy_biases = vec![0.0; num_actions];

        let value_weights: Vec<f64> = (0..feature_dim)
            .map(|_| (rand::random::<f64>() - 0.5) * 0.1)
            .collect();

        Self {
            policy_weights,
            policy_biases,
            value_weights,
            value_bias: 0.0,
            feature_dim,
            num_actions,
        }
    }

    fn action_probs(&self, state: &[f64]) -> Vec<f64> {
        let logits: Vec<f64> = (0..self.num_actions)
            .map(|a| {
                let dot: f64 = state.iter().zip(&self.policy_weights[a]).map(|(s, w)| s * w).sum();
                dot + self.policy_biases[a]
            })
            .collect();

        let max_logit = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exp_logits: Vec<f64> = logits.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum_exp: f64 = exp_logits.iter().sum();

        exp_logits.iter().map(|&e| e / sum_exp).collect()
    }

    fn sample_action(&self, state: &[f64]) -> (usize, f64) {
        let probs = self.action_probs(state);
        let r: f64 = rand::random();
        let mut cumsum = 0.0;
        for (action, &prob) in probs.iter().enumerate() {
            cumsum += prob;
            if r < cumsum {
                return (action, prob.max(1e-10).ln());
            }
        }
        let last = self.num_actions - 1;
        (last, probs[last].max(1e-10).ln())
    }

    fn log_prob(&self, state: &[f64], action: usize) -> f64 {
        let probs = self.action_probs(state);
        probs[action].max(1e-10).ln()
    }

    fn value(&self, state: &[f64]) -> f64 {
        let dot: f64 = state.iter().zip(&self.value_weights).map(|(s, w)| s * w).sum();
        dot + self.value_bias
    }

    fn entropy(&self, state: &[f64]) -> f64 {
        let probs = self.action_probs(state);
        -probs.iter().filter(|&&p| p > 1e-10).map(|&p| p * p.ln()).sum::<f64>()
    }

    /// PPO update step
    fn update(
        &mut self,
        state: &[f64],
        action: usize,
        old_log_prob: f64,
        advantage: f64,
        returns: f64,
        config: &PpoConfig,
    ) -> (f64, f64, f64) {
        // Current policy log prob
        let new_log_prob = self.log_prob(state, action);

        // Importance ratio
        let ratio = (new_log_prob - old_log_prob).exp();

        // Clipped surrogate objective
        let surr1 = ratio * advantage;
        let surr2 = ratio.clamp(1.0 - config.clip_epsilon, 1.0 + config.clip_epsilon) * advantage;
        let policy_loss = -surr1.min(surr2);

        // Value loss
        let value_pred = self.value(state);
        let value_loss = (returns - value_pred).powi(2);

        // Entropy bonus
        let entropy = self.entropy(state);
        let entropy_loss = -entropy;

        // Policy gradient (simplified)
        let probs = self.action_probs(state);
        if ratio <= 1.0 + config.clip_epsilon && ratio >= 1.0 - config.clip_epsilon {
            for a in 0..self.num_actions {
                let indicator = if a == action { 1.0 } else { 0.0 };
                let grad = indicator - probs[a];
                let total_grad = advantage * grad - config.entropy_coef * probs[a].max(1e-10).ln() * grad;

                for (i, &s) in state.iter().enumerate() {
                    self.policy_weights[a][i] += config.learning_rate * total_grad * s;
                }
                self.policy_biases[a] += config.learning_rate * total_grad;
            }
        }

        // Value update
        let value_error = returns - value_pred;
        for (i, &s) in state.iter().enumerate() {
            self.value_weights[i] += config.learning_rate * config.value_coef * value_error * s;
        }
        self.value_bias += config.learning_rate * config.value_coef * value_error;

        (policy_loss, value_loss, entropy_loss)
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.feature_dim as u32).to_le_bytes());

        for pw in &self.policy_weights {
            for &w in pw {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        for &b in &self.policy_biases {
            bytes.extend_from_slice(&b.to_le_bytes());
        }
        for &w in &self.value_weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        bytes.extend_from_slice(&self.value_bias.to_le_bytes());

        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            anyhow::bail!("Invalid PPO network bytes");
        }

        let num_actions = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let feature_dim = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

        let mut offset = 8;
        let mut policy_weights = Vec::new();
        for _ in 0..num_actions {
            let mut pw = Vec::new();
            for _ in 0..feature_dim {
                let w = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                pw.push(w);
                offset += 8;
            }
            policy_weights.push(pw);
        }

        let mut policy_biases = Vec::new();
        for _ in 0..num_actions {
            let b = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            policy_biases.push(b);
            offset += 8;
        }

        let mut value_weights = Vec::new();
        for _ in 0..feature_dim {
            let w = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            value_weights.push(w);
            offset += 8;
        }

        let value_bias = f64::from_le_bytes([
            bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
            bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
        ]);

        Ok(Self {
            policy_weights,
            policy_biases,
            value_weights,
            value_bias,
            feature_dim,
            num_actions,
        })
    }
}

/// PPO Agent
pub struct PpoAgent {
    config: PpoConfig,
    network: PpoNetwork,
    /// Rollout buffer
    rollouts: Vec<RolloutEntry>,
    conn: Connection,
    episode_count: u64,
    update_count: u64,
}

impl PpoAgent {
    pub fn new(db_path: &Path, config: PpoConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let network = PpoNetwork::new(config.feature_dim, config.num_actions);

        let mut agent = Self {
            network,
            rollouts: Vec::new(),
            conn,
            episode_count: 0,
            update_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_network()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS ppo_network (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                network_bytes BLOB NOT NULL,
                episode_count INTEGER NOT NULL DEFAULT 0,
                update_count INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS ppo_updates (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                policy_loss REAL NOT NULL,
                value_loss REAL NOT NULL,
                entropy REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_network(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, u64, u64)> = self.conn.query_row(
            "SELECT network_bytes, episode_count, update_count FROM ppo_network WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );

        if let Ok((bytes, episodes, updates)) = result {
            self.network = PpoNetwork::from_bytes(&bytes)?;
            self.episode_count = episodes;
            self.update_count = updates;
        }

        Ok(())
    }

    fn save_network(&self) -> Result<()> {
        let bytes = self.network.to_bytes();

        self.conn.execute(
            r#"
            INSERT INTO ppo_network (id, network_bytes, episode_count, update_count)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                network_bytes = ?1,
                episode_count = ?2,
                update_count = ?3,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![bytes, self.episode_count, self.update_count],
        )?;

        Ok(())
    }

    /// Select action and store transition
    pub fn select_action(&mut self, state: &[f64]) -> usize {
        let (action, log_prob) = self.network.sample_action(state);
        let value = self.network.value(state);

        self.rollouts.push(RolloutEntry {
            state: state.to_vec(),
            action,
            reward: 0.0,
            value,
            log_prob,
            done: false,
        });

        action
    }

    /// Record reward
    pub fn record_reward(&mut self, reward: f64, done: bool) {
        if let Some(entry) = self.rollouts.last_mut() {
            entry.reward = reward;
            entry.done = done;
        }
    }

    /// Compute GAE (Generalized Advantage Estimation)
    fn compute_gae(&self, next_value: f64) -> (Vec<f64>, Vec<f64>) {
        let n = self.rollouts.len();
        let mut advantages = vec![0.0; n];
        let mut returns = vec![0.0; n];

        let mut last_gae = 0.0;
        let mut last_value = next_value;

        for t in (0..n).rev() {
            let entry = &self.rollouts[t];
            let next_val = if entry.done { 0.0 } else { last_value };

            let delta = entry.reward + self.config.discount_factor * next_val - entry.value;
            last_gae = delta + self.config.discount_factor * self.config.gae_lambda *
                       (if entry.done { 0.0 } else { last_gae });

            advantages[t] = last_gae;
            returns[t] = last_gae + entry.value;
            last_value = entry.value;
        }

        // Normalize advantages
        let mean = advantages.iter().sum::<f64>() / n as f64;
        let std = (advantages.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / n as f64).sqrt();

        if std > 1e-8 {
            for a in &mut advantages {
                *a = (*a - mean) / std;
            }
        }

        (advantages, returns)
    }

    /// PPO update
    pub fn update(&mut self, next_state: Option<&[f64]>) -> Result<()> {
        if self.rollouts.is_empty() {
            return Ok(());
        }

        let next_value = next_state.map(|s| self.network.value(s)).unwrap_or(0.0);
        let (advantages, returns) = self.compute_gae(next_value);

        // Store old log probs
        let old_log_probs: Vec<f64> = self.rollouts
            .iter()
            .map(|e| e.log_prob)
            .collect();

        let mut total_policy_loss = 0.0;
        let mut total_value_loss = 0.0;
        let mut total_entropy = 0.0;
        let mut update_count = 0;

        // Multiple epochs
        for _ in 0..self.config.epochs {
            // Mini-batch updates (simplified: use all data if less than batch_size)
            for (i, entry) in self.rollouts.iter().enumerate() {
                let (pl, vl, el) = self.network.update(
                    &entry.state,
                    entry.action,
                    old_log_probs[i],
                    advantages[i],
                    returns[i],
                    &self.config,
                );

                total_policy_loss += pl;
                total_value_loss += vl;
                total_entropy += -el;
                update_count += 1;
            }
        }

        // Log update
        if update_count > 0 {
            self.update_count += 1;
            self.conn.execute(
                "INSERT INTO ppo_updates (policy_loss, value_loss, entropy) VALUES (?1, ?2, ?3)",
                params![
                    total_policy_loss / update_count as f64,
                    total_value_loss / update_count as f64,
                    total_entropy / update_count as f64
                ],
            )?;
        }

        // Clear rollouts
        self.rollouts.clear();

        self.save_network()?;

        Ok(())
    }

    /// End episode
    pub fn end_episode(&mut self) -> Result<f64> {
        let total_reward: f64 = self.rollouts.iter().map(|e| e.reward).sum();

        self.update(None)?;

        self.episode_count += 1;
        self.save_network()?;

        Ok(total_reward)
    }

    pub fn get_action_probs(&self, state: &[f64]) -> Vec<f64> {
        self.network.action_probs(state)
    }

    pub fn get_value(&self, state: &[f64]) -> f64 {
        self.network.value(state)
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn update_count(&self) -> u64 {
        self.update_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ppo_network() {
        let net = PpoNetwork::new(4, 3);
        let state = vec![1.0, 0.0, 1.0, 0.5];

        let probs = net.action_probs(&state);
        assert_eq!(probs.len(), 3);
        assert!((probs.iter().sum::<f64>() - 1.0).abs() < 1e-6);

        let _value = net.value(&state);
    }

    #[test]
    fn test_ppo_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("ppo.db");
        let config = PpoConfig {
            feature_dim: 4,
            num_actions: 3,
            epochs: 2,
            ..Default::default()
        };

        let mut agent = PpoAgent::new(&db_path, config).unwrap();

        // Collect rollout
        for i in 0..10 {
            let state = vec![i as f64 / 10.0; 4];
            let _action = agent.select_action(&state);
            agent.record_reward(1.0, i == 9);
        }

        // Update
        let total_reward = agent.end_episode().unwrap();
        assert_eq!(total_reward, 10.0);
        assert_eq!(agent.episode_count(), 1);
    }

    #[test]
    fn test_gae() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("ppo.db");
        let config = PpoConfig {
            feature_dim: 4,
            num_actions: 3,
            ..Default::default()
        };

        let mut agent = PpoAgent::new(&db_path, config).unwrap();

        for i in 0..5 {
            let state = vec![i as f64 / 5.0; 4];
            agent.select_action(&state);
            agent.record_reward(1.0, i == 4);
        }

        let (advantages, returns) = agent.compute_gae(0.0);
        assert_eq!(advantages.len(), 5);
        assert_eq!(returns.len(), 5);
    }

    #[test]
    fn test_serialization() {
        let net = PpoNetwork::new(4, 3);
        let bytes = net.to_bytes();
        let restored = PpoNetwork::from_bytes(&bytes).unwrap();

        assert_eq!(net.num_actions, restored.num_actions);
        assert_eq!(net.feature_dim, restored.feature_dim);
    }
}
