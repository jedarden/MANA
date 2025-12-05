//! Session end handler
//!
//! Parses recent JSONL logs, updates accumulator state, and triggers
//! learning when trajectory count reaches threshold.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::learning;

const DEFAULT_THRESHOLD: u32 = 15;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AccumulatorState {
    pub trajectory_count: u32,
    pub pending_files: Vec<PathBuf>,
    pub last_learning_cycle: Option<DateTime<Utc>>,
    pub last_file_positions: std::collections::HashMap<PathBuf, u64>,
    pub retry_count: u32,
    pub version: u32,
}

impl AccumulatorState {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Process session end event
///
/// 1. Find JSONL log files
/// 2. Count new trajectories
/// 3. Update accumulator state
/// 4. Trigger learning if threshold met
pub async fn session_end() -> Result<()> {
    info!("Processing session end");

    // Get MANA data directory
    let mana_dir = get_mana_dir()?;
    std::fs::create_dir_all(&mana_dir)?;

    let state_path = mana_dir.join("learning-state.json");
    let mut state = AccumulatorState::load(&state_path)?;

    // Find Claude Code log directory
    let claude_logs = get_claude_logs_dir();
    if !claude_logs.exists() {
        debug!("Claude logs directory not found: {:?}", claude_logs);
        return Ok(());
    }

    // Count new trajectories from JSONL files
    let (new_trajectories, updated_positions) = count_new_trajectories(&claude_logs, &state)?;

    state.trajectory_count += new_trajectories;
    state.last_file_positions.extend(updated_positions);

    info!(
        "Accumulated {} trajectories (total: {})",
        new_trajectories, state.trajectory_count
    );

    // Check threshold
    if state.trajectory_count >= DEFAULT_THRESHOLD {
        info!("Threshold reached ({} >= {}), triggering learning",
              state.trajectory_count, DEFAULT_THRESHOLD);

        // Run foreground learning
        match learning::foreground_learn(&state.pending_files).await {
            Ok(result) => {
                info!("Learning complete: {} patterns created", result.patterns_created);

                // Reset state
                state.trajectory_count = 0;
                state.pending_files.clear();
                state.retry_count = 0;
                state.last_learning_cycle = Some(Utc::now());

                // Spawn background consolidation (fire-and-forget)
                learning::spawn_consolidation()?;
            }
            Err(e) => {
                warn!("Foreground learning failed: {}", e);
                state.retry_count += 1;

                if state.retry_count >= 3 {
                    warn!("Max retries reached, resetting accumulator state");
                    state.trajectory_count = 0;
                    state.pending_files.clear();
                    state.retry_count = 0;
                }
            }
        }
    }

    // Save state
    state.save(&state_path)?;

    Ok(())
}

fn get_mana_dir() -> Result<PathBuf> {
    // Check for .mana directory in current project first
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    // Fall back to home directory
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

fn get_claude_logs_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claude/projects"))
        .unwrap_or_else(|| PathBuf::from(".claude/projects"))
}

fn count_new_trajectories(
    logs_dir: &std::path::Path,
    state: &AccumulatorState,
) -> Result<(u32, std::collections::HashMap<PathBuf, u64>)> {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut total_new = 0u32;
    let mut updated_positions = std::collections::HashMap::new();

    // Collect all JSONL files from logs_dir and its subdirectories
    let mut jsonl_files = Vec::new();

    let entries = match std::fs::read_dir(logs_dir) {
        Ok(e) => e,
        Err(e) => {
            debug!("Could not read logs dir {:?}: {}", logs_dir, e);
            return Ok((0, updated_positions));
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Check subdirectory for JSONL files
            if let Ok(subentries) = std::fs::read_dir(&path) {
                for subentry in subentries.flatten() {
                    let subpath = subentry.path();
                    if subpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                        jsonl_files.push(subpath);
                    }
                }
            }
        } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            jsonl_files.push(path);
        }
    }

    debug!("Found {} JSONL files to process", jsonl_files.len());

    for path in jsonl_files {

        // Get last processed position for this file
        let start_offset = state.last_file_positions
            .get(&path)
            .copied()
            .unwrap_or(0);

        // Open file and seek to last position
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                debug!("Could not open {:?}: {}", path, e);
                continue;
            }
        };

        let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

        // Skip if we've already processed to the end
        if start_offset >= file_len {
            continue;
        }

        let mut reader = BufReader::new(file);
        if start_offset > 0 {
            if let Err(e) = reader.seek(SeekFrom::Start(start_offset)) {
                debug!("Could not seek in {:?}: {}", path, e);
                continue;
            }
        }

        let mut bytes_read = start_offset;
        let mut file_trajectories = 0u32;

        // Count trajectories: assistant messages with tool_use
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            bytes_read += line.len() as u64 + 1; // +1 for newline

            // Fast path: check for assistant type with tool_use before full parse
            // This is the pattern we're looking for based on the JSONL format
            if line.contains(r#""type":"assistant""#) ||
               (line.contains(r#""role":"assistant""#) && line.contains("tool_use")) {
                file_trajectories += 1;
            }
        }

        if file_trajectories > 0 {
            debug!(
                "Found {} new trajectories in {:?} (bytes {} to {})",
                file_trajectories, path, start_offset, bytes_read
            );
        }

        total_new += file_trajectories;
        updated_positions.insert(path, bytes_read);
    }

    Ok((total_new, updated_positions))
}
