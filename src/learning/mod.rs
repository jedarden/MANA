//! Learning module for MANA
//!
//! Handles foreground learning (quick pattern extraction) and
//! background consolidation (optimization of patterns).

use serde::{Deserialize, Serialize};

mod foreground;
mod consolidation;
pub mod trajectory;

pub use foreground::foreground_learn;
pub use consolidation::{consolidate, spawn_consolidation};
pub use trajectory::{Trajectory, ToolCall, ToolResult, Verdict, parse_trajectories};

/// Result of a foreground learning cycle
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LearningResult {
    pub patterns_created: u32,
    pub patterns_updated: u32,
    pub trajectories_processed: u32,
    pub duration_ms: u64,
}
