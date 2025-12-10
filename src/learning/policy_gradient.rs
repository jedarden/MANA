//! Policy Gradient - Direct Policy Optimization
//!
//! Instead of learning value functions (like Q-learning), policy gradient methods
//! directly optimize the policy π(a|s) using gradient ascent on expected reward.
//!
//! REINFORCE algorithm: ∇J(θ) = E[∑_t ∇log(π(a_t|s_t)) * G_t]
//!
//! This implementation uses a linear softmax policy for simplicity.

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

/// Policy Gradient Configuration
#[derive(Debug, Clone)]
pub struct PolicyGradientConfig {
    /// Learning rate
    pub learning_rate: f64,
    /// Discount factor (gamma)
    pub discount_factor: f64,
    /// Feature dimension
    pub feature_dim: usize,
    /// Number of actions
    pub num_actions: usize,
    /// Baseline subtraction for variance reduction
    pub use_baseline: bool,
    /// Entropy bonus coefficient (encourages exploration)
    pub entropy_coef: f64,
}

impl Default for PolicyGradientConfig {
    fn default() -> Self {
        Self {
            learning_rate: 0.01,
            discount_factor: 0.99,
            feature_dim: 64,
            num_actions: 10,
            use_baseline: true,
            entropy_coef: 0.01,
        }
    }
}

/// Trajectory step for policy gradient
#[derive(Debug, Clone)]
pub struct TrajectoryStep {
    pub state: Vec<f64>,
    pub action: usize,
    pub reward: f64,
    pub log_prob: f64,
}

/// Linear softmax policy: π(a|s) = softmax(W·s + b)
#[derive(Debug, Clone)]
pub struct LinearPolicy {
    /// Weights: [num_actions][feature_dim]
    weights: Vec<Vec<f64>>,
    /// Biases
    biases: Vec<f64>,
    feature_dim: usize,
    num_actions: usize,
}

impl LinearPolicy {
    pub fn new(feature_dim: usize, num_actions: usize) -> Self {
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

    /// Compute action logits (before softmax)
    fn logits(&self, state: &[f64]) -> Vec<f64> {
        (0..self.num_actions)
            .map(|a| {
                let dot: f64 = state
                    .iter()
                    .zip(self.weights[a].iter())
                    .map(|(s, w)| s * w)
                    .sum();
                dot + self.biases[a]
            })
            .collect()
    }

    /// Compute action probabilities using softmax
    pub fn action_probs(&self, state: &[f64]) -> Vec<f64> {
        let logits = self.logits(state);

        // Stable softmax
        let max_logit = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let exp_logits: Vec<f64> = logits.iter().map(|&l| (l - max_logit).exp()).collect();
        let sum_exp: f64 = exp_logits.iter().sum();

        exp_logits.iter().map(|&e| e / sum_exp).collect()
    }

    /// Sample action from policy
    pub fn sample_action(&self, state: &[f64]) -> (usize, f64) {
        let probs = self.action_probs(state);

        // Sample from categorical distribution
        let r: f64 = rand::random();
        let mut cumsum = 0.0;
        for (action, &prob) in probs.iter().enumerate() {
            cumsum += prob;
            if r < cumsum {
                return (action, prob.ln());
            }
        }

        // Fallback to last action
        let last = self.num_actions - 1;
        (last, probs[last].ln())
    }

    /// Get log probability of an action
    pub fn log_prob(&self, state: &[f64], action: usize) -> f64 {
        let probs = self.action_probs(state);
        probs[action].max(1e-10).ln()
    }

    /// Compute entropy of policy (for exploration bonus)
    pub fn entropy(&self, state: &[f64]) -> f64 {
        let probs = self.action_probs(state);
        -probs.iter()
            .filter(|&&p| p > 1e-10)
            .map(|&p| p * p.ln())
            .sum::<f64>()
    }

    /// Update policy using gradient ascent
    /// ∇log(π(a|s)) = φ(s) * (1(a) - π(·|s))  for linear softmax
    pub fn update(&mut self, state: &[f64], action: usize, advantage: f64, learning_rate: f64) {
        let probs = self.action_probs(state);

        for a in 0..self.num_actions {
            let indicator = if a == action { 1.0 } else { 0.0 };
            let grad_log_prob = indicator - probs[a];

            // Update weights
            for (i, &s) in state.iter().enumerate() {
                self.weights[a][i] += learning_rate * advantage * grad_log_prob * s;
            }
            self.biases[a] += learning_rate * advantage * grad_log_prob;
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.num_actions as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.feature_dim as u32).to_le_bytes());

        for action_weights in &self.weights {
            for &w in action_weights {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        for &b in &self.biases {
            bytes.extend_from_slice(&b.to_le_bytes());
        }

        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            anyhow::bail!("Invalid policy bytes");
        }

        let num_actions = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let feature_dim = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;

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

/// Policy Gradient Agent (REINFORCE)
pub struct PolicyGradientAgent {
    config: PolicyGradientConfig,
    policy: LinearPolicy,
    /// Current episode trajectory
    trajectory: Vec<TrajectoryStep>,
    /// Running baseline (average return)
    baseline: f64,
    /// Database connection
    conn: Connection,
    episode_count: u64,
}

impl PolicyGradientAgent {
    pub fn new(db_path: &Path, config: PolicyGradientConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let policy = LinearPolicy::new(config.feature_dim, config.num_actions);

        let mut agent = Self {
            policy,
            trajectory: Vec::new(),
            baseline: 0.0,
            conn,
            episode_count: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_policy()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS pg_policy (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                policy_bytes BLOB NOT NULL,
                baseline REAL NOT NULL DEFAULT 0.0,
                episode_count INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS pg_episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                total_reward REAL NOT NULL,
                steps INTEGER NOT NULL,
                avg_entropy REAL NOT NULL,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_policy(&mut self) -> Result<()> {
        let result: rusqlite::Result<(Vec<u8>, f64, u64)> = self.conn.query_row(
            "SELECT policy_bytes, baseline, episode_count FROM pg_policy WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );

        if let Ok((bytes, baseline, episodes)) = result {
            self.policy = LinearPolicy::from_bytes(&bytes)?;
            self.baseline = baseline;
            self.episode_count = episodes;
        }

        Ok(())
    }

    fn save_policy(&self) -> Result<()> {
        let bytes = self.policy.to_bytes();

        self.conn.execute(
            r#"
            INSERT INTO pg_policy (id, policy_bytes, baseline, episode_count)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                policy_bytes = ?1,
                baseline = ?2,
                episode_count = ?3,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![bytes, self.baseline, self.episode_count],
        )?;

        Ok(())
    }

    /// Select action from policy
    pub fn select_action(&mut self, state: &[f64]) -> usize {
        let (action, log_prob) = self.policy.sample_action(state);

        // Store in trajectory (reward will be added later)
        self.trajectory.push(TrajectoryStep {
            state: state.to_vec(),
            action,
            reward: 0.0,
            log_prob,
        });

        action
    }

    /// Record reward for the last action
    pub fn record_reward(&mut self, reward: f64) {
        if let Some(step) = self.trajectory.last_mut() {
            step.reward = reward;
        }
    }

    /// Compute discounted returns
    fn compute_returns(&self) -> Vec<f64> {
        let n = self.trajectory.len();
        let mut returns = vec![0.0; n];

        if n == 0 {
            return returns;
        }

        returns[n - 1] = self.trajectory[n - 1].reward;

        for t in (0..n - 1).rev() {
            returns[t] = self.trajectory[t].reward + self.config.discount_factor * returns[t + 1];
        }

        returns
    }

    /// Update policy at end of episode (REINFORCE)
    pub fn end_episode(&mut self) -> Result<f64> {
        if self.trajectory.is_empty() {
            return Ok(0.0);
        }

        let returns = self.compute_returns();
        let total_reward: f64 = self.trajectory.iter().map(|s| s.reward).sum();
        let avg_return = returns.iter().sum::<f64>() / returns.len() as f64;

        // Update baseline (exponential moving average)
        if self.config.use_baseline {
            self.baseline = 0.9 * self.baseline + 0.1 * avg_return;
        }

        // Compute average entropy
        let avg_entropy: f64 = self.trajectory
            .iter()
            .map(|s| self.policy.entropy(&s.state))
            .sum::<f64>()
            / self.trajectory.len() as f64;

        // Policy gradient update
        for (step, &g_t) in self.trajectory.iter().zip(returns.iter()) {
            let advantage = g_t - if self.config.use_baseline { self.baseline } else { 0.0 };

            // Add entropy bonus to advantage
            let entropy = self.policy.entropy(&step.state);
            let adjusted_advantage = advantage + self.config.entropy_coef * entropy;

            self.policy.update(
                &step.state,
                step.action,
                adjusted_advantage,
                self.config.learning_rate,
            );
        }

        // Log episode
        self.episode_count += 1;
        self.conn.execute(
            "INSERT INTO pg_episodes (total_reward, steps, avg_entropy) VALUES (?1, ?2, ?3)",
            params![total_reward, self.trajectory.len() as i64, avg_entropy],
        )?;

        // Clear trajectory
        self.trajectory.clear();

        // Save policy
        self.save_policy()?;

        Ok(total_reward)
    }

    /// Get action probabilities for a state
    pub fn get_action_probs(&self, state: &[f64]) -> Vec<f64> {
        self.policy.action_probs(state)
    }

    pub fn episode_count(&self) -> u64 {
        self.episode_count
    }

    pub fn baseline(&self) -> f64 {
        self.baseline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_linear_policy() {
        let policy = LinearPolicy::new(4, 3);
        let state = vec![1.0, 0.0, 1.0, 0.5];

        let probs = policy.action_probs(&state);

        assert_eq!(probs.len(), 3);
        assert!((probs.iter().sum::<f64>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_policy_sampling() {
        let policy = LinearPolicy::new(4, 3);
        let state = vec![1.0, 0.0, 1.0, 0.5];

        // Sample multiple times
        for _ in 0..10 {
            let (action, log_prob) = policy.sample_action(&state);
            assert!(action < 3);
            assert!(log_prob <= 0.0); // Log prob is always <= 0
        }
    }

    #[test]
    fn test_policy_update() {
        let mut policy = LinearPolicy::new(4, 3);
        let state = vec![1.0, 1.0, 1.0, 1.0];

        let initial_probs = policy.action_probs(&state);

        // Update to favor action 0
        for _ in 0..10 {
            policy.update(&state, 0, 1.0, 0.1);
        }

        let new_probs = policy.action_probs(&state);
        assert!(new_probs[0] > initial_probs[0]);
    }

    #[test]
    fn test_policy_gradient_agent() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("pg.db");
        let config = PolicyGradientConfig {
            feature_dim: 4,
            num_actions: 3,
            ..Default::default()
        };

        let mut agent = PolicyGradientAgent::new(&db_path, config).unwrap();

        // Run episode
        for i in 0..5 {
            let state = vec![i as f64 / 5.0; 4];
            let _action = agent.select_action(&state);
            agent.record_reward(1.0);
        }

        let total_reward = agent.end_episode().unwrap();
        assert_eq!(total_reward, 5.0);
        assert_eq!(agent.episode_count(), 1);
    }

    #[test]
    fn test_entropy() {
        let policy = LinearPolicy::new(4, 3);
        let state = vec![0.0; 4];

        let entropy = policy.entropy(&state);
        // Uniform distribution has max entropy
        assert!(entropy > 0.0);
    }

    #[test]
    fn test_policy_serialization() {
        let policy = LinearPolicy::new(4, 3);
        let bytes = policy.to_bytes();
        let restored = LinearPolicy::from_bytes(&bytes).unwrap();

        assert_eq!(policy.num_actions, restored.num_actions);
        assert_eq!(policy.feature_dim, restored.feature_dim);
    }
}
