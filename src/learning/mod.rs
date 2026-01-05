//! Learning module for MANA
//!
//! Handles foreground learning (quick pattern extraction) and
//! background consolidation (optimization of patterns).
//!
//! ## Reinforcement Learning Algorithms
//!
//! MANA supports 9 RL algorithms for adaptive pattern optimization:
//!
//! - **Q-Learning**: Off-policy TD control (classic, simple, effective)
//! - **SARSA**: On-policy TD control (safer exploration)
//! - **DQN**: Deep Q-Network with experience replay (function approximation)
//! - **Policy Gradient**: Direct policy optimization (REINFORCE)
//! - **Actor-Critic**: Combined value and policy learning (lower variance)
//! - **PPO**: Proximal Policy Optimization (stable, sample efficient)
//! - **Decision Transformer**: Sequence modeling for RL (return-conditioned)
//! - **MCTS**: Monte Carlo Tree Search (planning through simulation)
//! - **Model-Based RL**: Learn dynamics model + MPC planning (sample efficient)
//!
//! ## Reflexion Memory
//!
//! Self-critique and learning from failures through reflection.

use serde::{Deserialize, Serialize};

mod foreground;
mod consolidation;
pub mod trajectory;
pub mod failure_analysis;

// RL Algorithms
pub mod qlearning;
pub mod sarsa;
pub mod dqn;
pub mod policy_gradient;
pub mod actor_critic;
pub mod ppo;
pub mod decision_transformer;
pub mod mcts;
pub mod model_based;

// Reflexion Memory
pub mod reflexion;

// Transfer Learning
pub mod transfer;

pub use foreground::foreground_learn;
pub use consolidation::{consolidate, spawn_consolidation};

// Q-Learning exports
#[allow(unused_imports)]
pub use qlearning::{QLearningAgent, QLearningConfig, QLearningStats, hash_context};

// SARSA exports
#[allow(unused_imports)]
pub use sarsa::{SarsaAgent, SarsaConfig};

// DQN exports
#[allow(unused_imports)]
pub use dqn::{DqnAgent, DqnConfig, LinearQNetwork, Experience};

// Policy Gradient exports
#[allow(unused_imports)]
pub use policy_gradient::{PolicyGradientAgent, PolicyGradientConfig, TrajectoryStep};

// Actor-Critic exports
#[allow(unused_imports)]
pub use actor_critic::{ActorCriticAgent, ActorCriticConfig};

// PPO exports
#[allow(unused_imports)]
pub use ppo::{PpoAgent, PpoConfig};

// Decision Transformer exports
#[allow(unused_imports)]
pub use decision_transformer::{DecisionTransformerAgent, DecisionTransformerConfig, Trajectory, Timestep};

// MCTS exports
#[allow(unused_imports)]
pub use mcts::{MctsAgent, MctsConfig, NodeStats};

// Model-Based RL exports
#[allow(unused_imports)]
pub use model_based::{ModelBasedAgent, ModelBasedConfig, Transition};

// Reflexion Memory exports
#[allow(unused_imports)]
pub use reflexion::{ReflexionStore, Reflection, ReflectionInput, ReflectionOutcome, ReflexionStats};

// Failure Analysis exports
#[allow(unused_imports)]
pub use failure_analysis::{
    FailureAnalyzer, FailurePoint, FailureType, FailureSeverity,
    TrajectoryAnalysis, RootCause, FailureStats
};

// Transfer Learning exports
#[allow(unused_imports)]
pub use transfer::{
    TransferEngine, TransferConfig, TransferResult, TransferSource,
    AdaptationStrategy, TransferablePattern, TransferPreview, PolicyTransferResult,
    calculate_transferability
};

// Trajectory types are internal to foreground learning - only expose what's needed
#[allow(unused_imports)]
pub(crate) use trajectory::parse_trajectories;

/// Result of a foreground learning cycle
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LearningResult {
    pub patterns_created: u32,
    pub patterns_updated: u32,
    pub trajectories_processed: u32,
    pub duration_ms: u64,
}
