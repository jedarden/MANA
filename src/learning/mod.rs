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
