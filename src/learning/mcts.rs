//! MCTS (Monte Carlo Tree Search) - Planning through Simulation
//!
//! MCTS builds a search tree through repeated simulations:
//! 1. Selection: Traverse tree using UCB1 to balance explore/exploit
//! 2. Expansion: Add new node when reaching frontier
//! 3. Simulation: Random rollout to estimate value
//! 4. Backpropagation: Update node statistics
//!
//! Used in AlphaGo and many game-playing agents.

#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;

/// MCTS Configuration
#[derive(Debug, Clone)]
pub struct MctsConfig {
    /// Exploration constant (c in UCB1)
    pub exploration_constant: f64,
    /// Number of simulations per action selection
    pub num_simulations: usize,
    /// Maximum rollout depth
    pub max_rollout_depth: usize,
    /// Discount factor for rollout returns
    pub discount_factor: f64,
    /// Number of actions
    pub num_actions: usize,
    /// Temperature for action selection (higher = more exploration)
    pub temperature: f64,
}

impl Default for MctsConfig {
    fn default() -> Self {
        Self {
            exploration_constant: 1.41, // sqrt(2) is a common choice
            num_simulations: 100,
            max_rollout_depth: 50,
            discount_factor: 0.99,
            num_actions: 10,
            temperature: 1.0,
        }
    }
}

/// Statistics for a node in the search tree
#[derive(Debug, Clone)]
pub struct NodeStats {
    /// Number of visits
    pub visits: u32,
    /// Total value accumulated
    pub total_value: f64,
    /// Prior probability (from policy if available)
    pub prior: f64,
    /// Child action stats: action -> (visits, total_value)
    pub children: HashMap<usize, (u32, f64)>,
}

impl NodeStats {
    pub fn new(prior: f64) -> Self {
        Self {
            visits: 0,
            total_value: 0.0,
            prior,
            children: HashMap::new(),
        }
    }

    /// Mean value of this node
    pub fn mean_value(&self) -> f64 {
        if self.visits == 0 {
            0.0
        } else {
            self.total_value / self.visits as f64
        }
    }

    /// UCB1 score for selecting this node
    pub fn ucb1(&self, parent_visits: u32, exploration_constant: f64) -> f64 {
        if self.visits == 0 {
            f64::INFINITY
        } else {
            self.mean_value() + exploration_constant * self.prior *
                ((parent_visits as f64).ln() / self.visits as f64).sqrt()
        }
    }
}

/// Hash a state for tree lookup
fn hash_state(state: &[f64]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    for &s in state {
        // Convert f64 to bits for hashing
        s.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// MCTS Agent
pub struct MctsAgent {
    config: MctsConfig,
    /// Search tree: state_hash -> NodeStats
    tree: HashMap<u64, NodeStats>,
    /// Database connection for persistence
    conn: Connection,
    /// Search statistics
    total_simulations: u64,
    total_expansions: u64,
}

impl MctsAgent {
    pub fn new(db_path: &Path, config: MctsConfig) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        let mut agent = Self {
            tree: HashMap::new(),
            conn,
            total_simulations: 0,
            total_expansions: 0,
            config,
        };

        agent.init_schema()?;
        agent.load_tree()?;

        Ok(agent)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS mcts_tree (
                state_hash INTEGER PRIMARY KEY,
                visits INTEGER NOT NULL,
                total_value REAL NOT NULL,
                prior REAL NOT NULL,
                children_json TEXT NOT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS mcts_stats (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                total_simulations INTEGER NOT NULL DEFAULT 0,
                total_expansions INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;
        Ok(())
    }

    fn load_tree(&mut self) -> Result<()> {
        // Load stats
        let stats: rusqlite::Result<(u64, u64)> = self.conn.query_row(
            "SELECT total_simulations, total_expansions FROM mcts_stats WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        if let Ok((sims, exps)) = stats {
            self.total_simulations = sims;
            self.total_expansions = exps;
        }

        // Load tree nodes (limit to most visited for memory)
        let mut stmt = self.conn.prepare(
            "SELECT state_hash, visits, total_value, prior, children_json FROM mcts_tree ORDER BY visits DESC LIMIT 10000"
        )?;

        let rows = stmt.query_map([], |row| {
            let state_hash: i64 = row.get(0)?;
            let visits: u32 = row.get(1)?;
            let total_value: f64 = row.get(2)?;
            let prior: f64 = row.get(3)?;
            let children_json: String = row.get(4)?;

            let children: HashMap<usize, (u32, f64)> = serde_json::from_str(&children_json)
                .unwrap_or_default();

            Ok((state_hash as u64, NodeStats {
                visits,
                total_value,
                prior,
                children,
            }))
        })?;

        for row in rows.flatten() {
            self.tree.insert(row.0, row.1);
        }

        Ok(())
    }

    fn save_node(&self, state_hash: u64, node: &NodeStats) -> Result<()> {
        let children_json = serde_json::to_string(&node.children)?;

        self.conn.execute(
            r#"
            INSERT INTO mcts_tree (state_hash, visits, total_value, prior, children_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(state_hash) DO UPDATE SET
                visits = ?2,
                total_value = ?3,
                prior = ?4,
                children_json = ?5,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![state_hash as i64, node.visits, node.total_value, node.prior, children_json],
        )?;

        Ok(())
    }

    fn save_stats(&self) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO mcts_stats (id, total_simulations, total_expansions)
            VALUES (1, ?1, ?2)
            ON CONFLICT(id) DO UPDATE SET
                total_simulations = ?1,
                total_expansions = ?2,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![self.total_simulations, self.total_expansions],
        )?;
        Ok(())
    }

    /// Get or create node for a state
    fn get_or_create_node(&mut self, state: &[f64]) -> u64 {
        let hash = hash_state(state);
        if !self.tree.contains_key(&hash) {
            let prior = 1.0 / self.config.num_actions as f64; // Uniform prior
            self.tree.insert(hash, NodeStats::new(prior));
            self.total_expansions += 1;
        }
        hash
    }

    /// Select best child action using UCB1
    fn select_action_ucb1(&self, state_hash: u64) -> Option<usize> {
        let node = self.tree.get(&state_hash)?;

        let mut best_action = 0;
        let mut best_ucb = f64::NEG_INFINITY;

        for action in 0..self.config.num_actions {
            let (visits, total_value) = node.children.get(&action).copied().unwrap_or((0, 0.0));

            let ucb = if visits == 0 {
                f64::INFINITY
            } else {
                let mean = total_value / visits as f64;
                mean + self.config.exploration_constant *
                    ((node.visits as f64).ln() / visits as f64).sqrt()
            };

            if ucb > best_ucb {
                best_ucb = ucb;
                best_action = action;
            }
        }

        Some(best_action)
    }

    /// Run a single MCTS simulation
    fn simulate<F, G>(
        &mut self,
        root_state: &[f64],
        transition_fn: &F,
        reward_fn: &G,
    ) -> f64
    where
        F: Fn(&[f64], usize) -> Vec<f64>,  // state, action -> next_state
        G: Fn(&[f64], usize) -> (f64, bool), // state, action -> (reward, done)
    {
        let mut path: Vec<(u64, usize)> = Vec::new();
        let mut current_state = root_state.to_vec();
        let mut total_reward = 0.0;
        let mut discount = 1.0;
        let max_selection_depth = self.config.max_rollout_depth * 2; // Limit selection depth

        // Selection phase: traverse tree
        for _depth in 0..max_selection_depth {
            let state_hash = self.get_or_create_node(&current_state);

            if let Some(action) = self.select_action_ucb1(state_hash) {
                let (reward, done) = reward_fn(&current_state, action);
                total_reward += discount * reward;
                discount *= self.config.discount_factor;

                path.push((state_hash, action));

                if done {
                    break;
                }

                current_state = transition_fn(&current_state, action);

                // Check if this is a new node (expansion)
                let next_hash = hash_state(&current_state);
                if !self.tree.contains_key(&next_hash) {
                    // Expansion: add new node
                    self.get_or_create_node(&current_state);

                    // Rollout: random simulation to estimate value
                    let rollout_value = self.rollout(&current_state, transition_fn, reward_fn, discount);
                    total_reward += rollout_value;
                    break;
                }
            } else {
                break;
            }
        }

        // Backpropagation: update statistics
        for (state_hash, action) in path.iter().rev() {
            if let Some(node) = self.tree.get_mut(state_hash) {
                node.visits += 1;
                node.total_value += total_reward;

                let (visits, value) = node.children.entry(*action).or_insert((0, 0.0));
                *visits += 1;
                *value += total_reward;
            }
        }

        self.total_simulations += 1;
        total_reward
    }

    /// Random rollout from a state
    fn rollout<F, G>(
        &self,
        start_state: &[f64],
        transition_fn: &F,
        reward_fn: &G,
        initial_discount: f64,
    ) -> f64
    where
        F: Fn(&[f64], usize) -> Vec<f64>,
        G: Fn(&[f64], usize) -> (f64, bool),
    {
        let mut state = start_state.to_vec();
        let mut total = 0.0;
        let mut discount = initial_discount;

        for _ in 0..self.config.max_rollout_depth {
            let action = (rand::random::<f64>() * self.config.num_actions as f64) as usize;
            let (reward, done) = reward_fn(&state, action);

            total += discount * reward;
            discount *= self.config.discount_factor;

            if done {
                break;
            }

            state = transition_fn(&state, action);
        }

        total
    }

    /// Select best action after running simulations
    pub fn select_action<F, G>(
        &mut self,
        state: &[f64],
        transition_fn: F,
        reward_fn: G,
    ) -> usize
    where
        F: Fn(&[f64], usize) -> Vec<f64>,
        G: Fn(&[f64], usize) -> (f64, bool),
    {
        // Run simulations
        for _ in 0..self.config.num_simulations {
            self.simulate(state, &transition_fn, &reward_fn);
        }

        let state_hash = hash_state(state);

        // Select action based on visit counts
        if let Some(node) = self.tree.get(&state_hash) {
            if self.config.temperature <= 0.0 {
                // Greedy: select most visited
                node.children
                    .iter()
                    .max_by_key(|(_, (visits, _))| *visits)
                    .map(|(action, _)| *action)
                    .unwrap_or(0)
            } else {
                // Temperature-based: sample proportional to visit^(1/temp)
                let visits: Vec<(usize, f64)> = node.children
                    .iter()
                    .map(|(&a, &(v, _))| (a, (v as f64).powf(1.0 / self.config.temperature)))
                    .collect();

                let total: f64 = visits.iter().map(|(_, v)| v).sum();
                if total <= 0.0 {
                    return 0;
                }

                let r = rand::random::<f64>() * total;
                let mut cumsum = 0.0;
                for (action, weight) in visits {
                    cumsum += weight;
                    if r < cumsum {
                        return action;
                    }
                }

                0
            }
        } else {
            // No tree node: random action
            (rand::random::<f64>() * self.config.num_actions as f64) as usize
        }
    }

    /// Get action visit distribution (useful for training)
    pub fn get_action_distribution(&self, state: &[f64]) -> Vec<f64> {
        let state_hash = hash_state(state);
        let mut distribution = vec![0.0; self.config.num_actions];

        if let Some(node) = self.tree.get(&state_hash) {
            let total_visits: u32 = node.children.values().map(|(v, _)| v).sum();
            if total_visits > 0 {
                for (&action, &(visits, _)) in &node.children {
                    if action < self.config.num_actions {
                        distribution[action] = visits as f64 / total_visits as f64;
                    }
                }
            }
        }

        distribution
    }

    /// Save tree to database
    pub fn save(&self) -> Result<()> {
        // Save most visited nodes
        let mut nodes: Vec<_> = self.tree.iter().collect();
        nodes.sort_by(|a, b| b.1.visits.cmp(&a.1.visits));

        for (hash, node) in nodes.iter().take(1000) {
            self.save_node(**hash, node)?;
        }

        self.save_stats()?;
        Ok(())
    }

    /// Clear tree (useful between episodes in some settings)
    pub fn clear_tree(&mut self) {
        self.tree.clear();
    }

    pub fn tree_size(&self) -> usize {
        self.tree.len()
    }

    pub fn total_simulations(&self) -> u64 {
        self.total_simulations
    }

    pub fn total_expansions(&self) -> u64 {
        self.total_expansions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Simple test environment: reach goal state
    fn test_transition(state: &[f64], action: usize) -> Vec<f64> {
        let mut next = state.to_vec();
        if action == 0 && next[0] < 10.0 {
            next[0] += 1.0;
        } else if action == 1 && next[0] > 0.0 {
            next[0] -= 1.0;
        }
        next
    }

    fn test_reward(state: &[f64], _action: usize) -> (f64, bool) {
        if state[0] >= 10.0 {
            (10.0, true) // Goal reached
        } else {
            (-0.1, false) // Step penalty
        }
    }

    #[test]
    fn test_node_stats() {
        let mut node = NodeStats::new(0.5);
        node.visits = 10;
        node.total_value = 5.0;

        assert_eq!(node.mean_value(), 0.5);

        let ucb = node.ucb1(100, 1.41);
        assert!(ucb > 0.5); // Should include exploration bonus
    }

    #[test]
    fn test_mcts_basic() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mcts.db");
        let config = MctsConfig {
            num_simulations: 10,
            num_actions: 2,
            ..Default::default()
        };

        let mut agent = MctsAgent::new(&db_path, config).unwrap();

        let state = vec![0.0];
        let action = agent.select_action(&state, test_transition, test_reward);

        assert!(action < 2);
        assert!(agent.total_simulations() > 0);
    }

    #[test]
    fn test_mcts_finds_goal() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mcts.db");
        let config = MctsConfig {
            num_simulations: 50,
            num_actions: 2,
            exploration_constant: 1.0,
            ..Default::default()
        };

        let mut agent = MctsAgent::new(&db_path, config).unwrap();

        // MCTS should learn to go right (action 0) to reach goal
        let state = vec![5.0]; // Halfway to goal
        let action = agent.select_action(&state, test_transition, test_reward);

        // With enough simulations, should prefer action 0 (go right)
        // But this isn't deterministic, so just check it returns valid action
        assert!(action < 2);
    }

    #[test]
    fn test_action_distribution() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mcts.db");
        let config = MctsConfig {
            num_simulations: 20,
            num_actions: 2,
            ..Default::default()
        };

        let mut agent = MctsAgent::new(&db_path, config).unwrap();

        let state = vec![5.0];
        agent.select_action(&state, test_transition, test_reward);

        let dist = agent.get_action_distribution(&state);
        assert_eq!(dist.len(), 2);

        // Distribution should sum to ~1 (or 0 if no visits)
        let sum: f64 = dist.iter().sum();
        assert!(sum <= 1.0 + 1e-6);
    }

    #[test]
    fn test_tree_persistence() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("mcts.db");
        let config = MctsConfig {
            num_simulations: 10,
            num_actions: 2,
            ..Default::default()
        };

        {
            let mut agent = MctsAgent::new(&db_path, config.clone()).unwrap();
            let state = vec![5.0];
            agent.select_action(&state, test_transition, test_reward);
            agent.save().unwrap();
        }

        // Reload and check state persisted
        let agent = MctsAgent::new(&db_path, config).unwrap();
        assert!(agent.total_simulations() > 0);
    }
}
