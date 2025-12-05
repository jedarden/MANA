//! Learning module for MANA
//!
//! Handles foreground learning (quick pattern extraction) and
//! background consolidation (optimization of patterns).

use anyhow::Result;
use std::path::PathBuf;
use tracing::{debug, info};

mod foreground;
mod consolidation;

pub use foreground::foreground_learn;
pub use consolidation::{consolidate, spawn_consolidation};

/// Result of a foreground learning cycle
#[derive(Debug, Default)]
pub struct LearningResult {
    pub patterns_created: u32,
    pub patterns_updated: u32,
    pub trajectories_processed: u32,
    pub duration_ms: u64,
}
