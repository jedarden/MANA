//! Actor-Critic - Combining Policy Gradient and Value Function
//!
//! Actor-Critic methods use two components:
//! - Actor: Policy network π(a|s) that selects actions
//! - Critic: Value network V(s) that evaluates states
//!
//! The critic provides a lower-variance advantage estimate:
//! A(s,a) = r + γV(s') - V(s)  (TD error)
//!
//! This combines the benefits of policy gradient (direct policy optimization)
//! and value-based methods (lower variance through bootstrapping).

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

/// Actor-Critic Configuration
#[derive(Debug, Clone)]
pub struct ActorCriticConfig {
    /// Actor learning rate
    pub actor_lr: f64,
    /// Critic learning rate
    pub critic_lr: f64,
    /// Discount factor (gamma)
    pub discount_factor: f64,
    /// Feature dimension
    pub feature_dim: usize,
    /// Number of actions
    pub num_actions: usize,
    /// Entropy coefficient
    pub entropy_coef: f64,
    /// Value loss coefficient
    pub value_coef: f64,
    /// N-step returns (1 = TD(0), larger = more like Monte Carlo)
    pub n_steps: usize,
}

impl Default for ActorCriticConfig {
    fn default() -> Self {
        Self {
            actor_lr: 0.001,
            critic_lr: 0.005,
            discount_factor: 0.99,
            feature_dim: 64,
            num_actions: 10,
            entropy_coef: 0.01,
            value_coef: 0.5,
            n_steps: 5,
        }
    }
}

/// Transition for n-step returns
#[derive(Debug, Clone)]
struct Transition {
    state: Vec<f64>,
    action: usize,
    reward: f64,
    log_prob: f64,
}

/// Linear Actor (policy) network
#[derive(Debug, Clone)]
struct Actor {
    weights: Vec<Vec<f64>>,
    biases: Vec<f64>,
    feature_dim: usize,
    num_actions: usize,
}

impl Actor {
    fn new(feature_dim: usize, num_actions: usize) -> Self {
        let weights: Vec<Vec<f64>> = (0..num_actions)
            .map(|_| (0..feature_dim).map(|_| (rand::random::<f64>() - 0.5) * 0.1).collect())
            .collect();
        let biases = vec![0.0; num_actions];

        Self { weights, biases, feature_dim, num_actions }
    }

    fn action_probs(&self, state: &[f64]) -> Vec<f64> {
        let logits: Vec<f64> = (0..self.num_actions)
            .map(|a| {
                let dot: f64 = state.iter().zip(&self.weights[a]).map(|(s, w)| s * w).sum();
                dot + self.biases[a]
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

    fn entropy(&self, state: &[f64]) -> f64 {
        let probs = self.action_probs(state);
        -probs.iter().filter(|&&p| p > 1e-10).map(|&p| p * p.ln()).sum::<f64>()
    }

    fn update(&mut self, state: &[f64], action: usize, advantage: f64, lr: f64) {
        let probs = self.action_probs(state);
        for a in 0..self.num_actions {
            let indicator = if a == action { 1.0 } else { 0.0 };
            let grad = indicator - probs[a];
            for (i, &s) in state.iter().enumerate() {
                self.weights[a][i] += lr * advantage * grad * s;
            }
            self.biases[a] += lr * advantage * grad;
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.feature_dim as u32).to_le_bytes());
        for aw in &self.weights {
            for &w in aw {
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
            anyhow::bail!("Invalid actor bytes");
        }
        let num_actions = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let feature_dim = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

        let mut offset = 8;
        let mut weights = Vec::new();
        for _ in 0..num_actions {
            let mut aw = Vec::new();
            for _ in 0..feature_dim {
                let w = f64::from_le_bytes([
                    bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                    bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
                ]);
                aw.push(w);
                offset += 8;
            }
            weights.push(aw);
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

        Ok(Self { weights, biases, feature_dim, num_actions })
    }
}

/// Linear Critic (value) network
#[derive(Debug, Clone)]
struct Critic {
    weights: Vec<f64>,
    bias: f64,
    feature_dim: usize,
}

impl Critic {
    fn new(feature_dim: usize) -> Self {
        let weights: Vec<f64> = (0..feature_dim)
            .map(|_| (rand::random::<f64>() - 0.5) * 0.1)
            .collect();
        Self { weights, bias: 0.0, feature_dim }
    }

    fn value(&self, state: &[f64]) -> f64 {
        let dot: f64 = state.iter().zip(&self.weights).map(|(s, w)| s * w).sum();
        dot + self.bias
    }

    fn update(&mut self, state: &[f64], target: f64, lr: f64) {
        let prediction = self.value(state);
        let error = target - prediction;

        for (i, &s) in state.iter().enumerate() {
            self.weights[i] += lr * error * s;
        }
        self.bias += lr * error;
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.feature_dim as u32).to_le_bytes());
        for &w in &self.weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        bytes.extend_from_slice(&self.bias.to_le_bytes());
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 4 {
            anyhow::bail!("Invalid critic bytes");
        }
        let feature_dim = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;

        let mut offset = 4;
        let mut weights = Vec::new();
        for _ in 0..feature_dim {
            let w = f64::from_le_bytes([
                bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
                bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
            ]);
            weights.push(w);
            offset += 8;
        }

        let bias = f64::from_le_bytes([
            bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3],
            bytes[offset+4], bytes[offset+5], bytes[offset+6], bytes[offset+7],
        ]);

        Ok(Self { weights, bias, feature_dim })
    }
}

/// Actor-Critic Agent
pub struct ActorCriticAgent {
    config: ActorCriticConfig,
    actor: Actor,
    critic: Critic,
    /// N-step buffer
    buffer: Vec<Transition>,
    conn: Connection,
    episode_count: u64,
    step_count: u64,
}

impl ActorCriticAgent {
    pub fn new(db_path: &Path, config: ActorCriticConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let actor = Actor::new(config.feature_dim, config.num_actions);
        let critic = Critic::new(config.feature_dim);

        let mut agent = Self {
            actor,
            critic,
            buffer: Vec::new(),
            conn,
            episode_count: 0,
            step_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_networks()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS ac_networks (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                actor_bytes BLOB NOT NULL,
                critic_bytes BLOB NOT NULL,
                episode_count INTEGER NOT NULL DEFAULT 0,
                step_count INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS ac_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                total_reward REAL NOT NULL,
                steps INTEGER NOT NULL,
                avg_value REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_networks(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, Vec<u8>, u64, u64)> = self.conn.query_row(
            "SELECT actor_bytes, critic_bytes, episode_count, step_count FROM ac_networks WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        );

        if let Ok((actor_bytes, critic_bytes, episodes, steps)) = result {
            self.actor = Actor::from_bytes(&actor_bytes)?;
            self.critic = Critic::from_bytes(&critic_bytes)?;
            self.episode_count = episodes;
            self.step_count = steps;
        }

        Ok(())
    }

    fn save_networks(&self) -> Result<()> {
        let actor_bytes = self.actor.to_bytes();
        let critic_bytes = self.critic.to_bytes();

        self.conn.execute(
            r#"
            INSERT INTO ac_networks (id, actor_bytes, critic_bytes, episode_count, step_count)
            VALUES (1, ?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                actor_bytes = ?1,
                critic_bytes = ?2,
                episode_count = ?3,
                step_count = ?4,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![actor_bytes, critic_bytes, self.episode_count, self.step_count],
        )?;

        Ok(())
    }

    /// Select action using actor policy
    pub fn select_action(&mut self, state: &[f64]) -> usize {
        let (action, log_prob) = self.actor.sample_action(state);

        self.buffer.push(Transition {
            state: state.to_vec(),
            action,
            reward: 0.0,
            log_prob,
        });

        action
    }

    /// Record reward for last action
    pub fn record_reward(&mut self, reward: f64) {
        if let Some(t) = self.buffer.last_mut() {
            t.reward = reward;
        }
    }

    /// Perform n-step update
    pub fn update(&mut self, next_state: &[f64], done: bool) -> Result<()> {
        if self.buffer.len() < self.config.n_steps && !done {
            return Ok(());
        }

        // Compute n-step return
        let bootstrap_value = if done { 0.0 } else { self.critic.value(next_state) };
        let mut returns = bootstrap_value;

        // Work backwards through buffer
        for t in self.buffer.iter().rev() {
            returns = t.reward + self.config.discount_factor * returns;
        }

        // Update for first transition in buffer
        if let Some(t) = self.buffer.first() {
            let value = self.critic.value(&t.state);
            let advantage = returns - value;

            // Update actor
            let entropy = self.actor.entropy(&t.state);
            let actor_advantage = advantage + self.config.entropy_coef * entropy;
            self.actor.update(&t.state, t.action, actor_advantage, self.config.actor_lr);

            // Update critic
            self.critic.update(&t.state, returns, self.config.critic_lr * self.config.value_coef);

            self.step_count += 1;
        }

        // Remove first transition
        if !self.buffer.is_empty() {
            self.buffer.remove(0);
        }

        Ok(())
    }

    /// End episode
    pub fn end_episode(&mut self) -> Result<f64> {
        let total_reward: f64 = self.buffer.iter().map(|t| t.reward).sum();
        let steps = self.buffer.len();

        // Process remaining transitions
        while !self.buffer.is_empty() {
            self.update(&[], true)?;
        }

        let avg_value = 0.0; // Could compute average value seen

        self.episode_count += 1;
        self.conn.execute(
            "INSERT INTO ac_episodes (total_reward, steps, avg_value) VALUES (?1, ?2, ?3)",
            params![total_reward, steps as i64, avg_value],
        )?;

        self.save_networks()?;
        self.buffer.clear();

        Ok(total_reward)
    }

    /// Get action probabilities
    pub fn get_action_probs(&self, state: &[f64]) -> Vec<f64> {
        self.actor.action_probs(state)
    }

    /// Get state value
    pub fn get_value(&self, state: &[f64]) -> f64 {
        self.critic.value(state)
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn step_count(&self) -> u64 {
        self.step_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_actor() {
        let actor = Actor::new(4, 3);
        let state = vec![1.0, 0.0, 1.0, 0.5];

        let probs = actor.action_probs(&state);
        assert_eq!(probs.len(), 3);
        assert!((probs.iter().sum::<f64>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_critic() {
        let critic = Critic::new(4);
        let state = vec![1.0, 0.0, 1.0, 0.5];

        let _value = critic.value(&state);
        // Value can be any real number
    }

    #[test]
    fn test_actor_critic_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("ac.db");
        let config = ActorCriticConfig {
            feature_dim: 4,
            num_actions: 3,
            n_steps: 2,
            ..Default::default()
        };

        let mut agent = ActorCriticAgent::new(&db_path, config).unwrap();

        // Run episode
        for i in 0..5 {
            let state = vec![i as f64 / 5.0; 4];
            let _action = agent.select_action(&state);
            agent.record_reward(1.0);

            if i > 0 {
                agent.update(&state, false).unwrap();
            }
        }

        let total_reward = agent.end_episode().unwrap();
        assert!(total_reward > 0.0);
    }

    #[test]
    fn test_serialization() {
        let actor = Actor::new(4, 3);
        let bytes = actor.to_bytes();
        let restored = Actor::from_bytes(&bytes).unwrap();
        assert_eq!(actor.num_actions, restored.num_actions);

        let critic = Critic::new(4);
        let bytes = critic.to_bytes();
        let restored = Critic::from_bytes(&bytes).unwrap();
        assert_eq!(critic.feature_dim, restored.feature_dim);
    }
}
