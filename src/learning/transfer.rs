//! Transfer learning API
//!
//! Enables knowledge transfer:
//! - Cross-session: Transfer patterns between sessions
//! - Cross-project: Transfer relevant patterns to new projects
//! - Cross-domain: Adapt patterns for similar domains
//! - Policy transfer: Transfer RL policies between contexts

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use tracing::{info, warn};

use crate::storage::{Pattern, PatternStore};
use crate::sync::export::{export_patterns_to_vec, import_patterns_from_vec, ImportResult, MergeStrategy};
use crate::sync::{ExportablePattern, SecurityConfig};

/// Transfer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferConfig {
    pub min_score: i64,              // Minimum pattern score to transfer
    pub min_success_rate: f64,        // Minimum success rate (0.0-1.0)
    pub adapt_tier: bool,             // Adapt tier_path for destination
    pub preserve_provenance: bool,    // Keep provenance history
    pub merge_duplicates: bool,       // Merge similar patterns
    pub similarity_threshold: f64,    // Threshold for duplicate detection
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            min_score: 0,
            min_success_rate: 0.5,
            adapt_tier: true,
            preserve_provenance: true,
            merge_duplicates: true,
            similarity_threshold: 0.85,
        }
    }
}

/// Result of a transfer operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResult {
    pub patterns_transferred: usize,
    pub patterns_merged: usize,
    pub patterns_skipped: usize,
    pub skills_transferred: usize,
    pub causal_edges_transferred: usize,
    pub source: String,
    pub destination: String,
}

/// Transfer source specification
#[derive(Debug, Clone)]
pub enum TransferSource {
    Session(String),           // Session ID
    Project(String),           // Project path
    Database(PathBuf),         // Direct database path
    Export(PathBuf),           // Exported JSON file
}

/// Domain adaptation strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptationStrategy {
    Direct,          // Transfer as-is
    Contextualize,   // Adapt context to new domain
    Generalize,      // Remove domain-specific details
    Specialize,      // Add domain-specific context
}

pub struct TransferEngine {
    config: TransferConfig,
}

impl TransferEngine {
    pub fn new(config: TransferConfig) -> Self {
        Self { config }
    }

    /// Transfer patterns from source to destination
    pub fn transfer(
        &self,
        source: &TransferSource,
        dest_db: &Path,
    ) -> Result<TransferResult> {
        let source_db = self.resolve_source_db(source)?;

        info!("Transferring patterns from {:?} to {:?}", source_db, dest_db);

        // Get transferable patterns from source
        let transferable = self.get_patterns_from_source(&source_db)?;

        // Filter based on configuration
        let filtered: Vec<_> = transferable.into_iter()
            .filter(|p| {
                let score = p.success_count - p.failure_count;
                let total = p.success_count + p.failure_count;
                let success_rate = if total > 0 {
                    p.success_count as f64 / total as f64
                } else {
                    0.0
                };

                score >= self.config.min_score && success_rate >= self.config.min_success_rate
            })
            .collect();

        info!("Filtered to {} patterns meeting criteria", filtered.len());

        // Transfer patterns
        let result = self.import_patterns(dest_db, filtered)?;

        // Transfer causal edges if patterns were transferred
        let causal_edges_transferred = if result.imported > 0 || result.merged > 0 {
            self.transfer_causal_edges(&source_db, dest_db)?
        } else {
            0
        };

        // Transfer skills
        let skills_transferred = self.transfer_skills(&source_db, dest_db)?;

        Ok(TransferResult {
            patterns_transferred: result.imported,
            patterns_merged: result.merged,
            patterns_skipped: result.skipped,
            skills_transferred,
            causal_edges_transferred,
            source: source_db.to_string_lossy().to_string(),
            destination: dest_db.to_string_lossy().to_string(),
        })
    }

    /// Transfer patterns matching specific criteria
    pub fn transfer_filtered(
        &self,
        source: &TransferSource,
        dest_db: &Path,
        tool_types: Option<&[String]>,
        domains: Option<&[String]>,
    ) -> Result<TransferResult> {
        let source_db = self.resolve_source_db(source)?;

        info!("Transferring filtered patterns from {:?} to {:?}", source_db, dest_db);

        let mut transferable = self.get_patterns_from_source(&source_db)?;

        // Filter by tool type if specified
        if let Some(types) = tool_types {
            transferable.retain(|p| types.contains(&p.tool_type));
            info!("Filtered to {} patterns matching tool types", transferable.len());
        }

        // Filter by domain (context contains domain keywords)
        if let Some(keywords) = domains {
            transferable.retain(|p| {
                keywords.iter().any(|k| p.context_query.to_lowercase().contains(&k.to_lowercase()))
            });
            info!("Filtered to {} patterns matching domains", transferable.len());
        }

        // Apply standard filters
        let filtered: Vec<_> = transferable.into_iter()
            .filter(|p| {
                let score = p.success_count - p.failure_count;
                let total = p.success_count + p.failure_count;
                let success_rate = if total > 0 {
                    p.success_count as f64 / total as f64
                } else {
                    0.0
                };

                score >= self.config.min_score && success_rate >= self.config.min_success_rate
            })
            .collect();

        let result = self.import_patterns(dest_db, filtered)?;

        let causal_edges_transferred = if result.imported > 0 || result.merged > 0 {
            self.transfer_causal_edges(&source_db, dest_db)?
        } else {
            0
        };

        let skills_transferred = self.transfer_skills(&source_db, dest_db)?;

        Ok(TransferResult {
            patterns_transferred: result.imported,
            patterns_merged: result.merged,
            patterns_skipped: result.skipped,
            skills_transferred,
            causal_edges_transferred,
            source: source_db.to_string_lossy().to_string(),
            destination: dest_db.to_string_lossy().to_string(),
        })
    }

    /// Transfer only high-value patterns (top percentile)
    pub fn transfer_top_patterns(
        &self,
        source: &TransferSource,
        dest_db: &Path,
        percentile: f64,  // e.g., 0.9 for top 10%
    ) -> Result<TransferResult> {
        let source_db = self.resolve_source_db(source)?;

        info!("Transferring top {:.0}% patterns from {:?}", (1.0 - percentile) * 100.0, source_db);

        let mut transferable = self.get_patterns_from_source(&source_db)?;

        // Calculate transferability scores
        let mut scored: Vec<(ExportablePattern, f64)> = transferable
            .into_iter()
            .map(|p| {
                let score = calculate_transferability(
                    p.success_count - p.failure_count,
                    if p.success_count + p.failure_count > 0 {
                        p.success_count as f64 / (p.success_count + p.failure_count) as f64
                    } else {
                        0.0
                    },
                    p.success_count + p.failure_count,
                    0, // age_days - we don't track creation time in exportable patterns
                );
                (p, score)
            })
            .collect();

        // Sort by transferability score
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top percentile
        let cutoff = (scored.len() as f64 * (1.0 - percentile)).ceil() as usize;
        let top_patterns: Vec<_> = scored.into_iter().take(cutoff).map(|(p, _)| p).collect();

        info!("Selected {} patterns in top percentile", top_patterns.len());

        let result = self.import_patterns(dest_db, top_patterns)?;

        let causal_edges_transferred = if result.imported > 0 || result.merged > 0 {
            self.transfer_causal_edges(&source_db, dest_db)?
        } else {
            0
        };

        let skills_transferred = self.transfer_skills(&source_db, dest_db)?;

        Ok(TransferResult {
            patterns_transferred: result.imported,
            patterns_merged: result.merged,
            patterns_skipped: result.skipped,
            skills_transferred,
            causal_edges_transferred,
            source: source_db.to_string_lossy().to_string(),
            destination: dest_db.to_string_lossy().to_string(),
        })
    }

    /// Adapt patterns for a new domain
    pub fn transfer_with_adaptation(
        &self,
        source: &TransferSource,
        dest_db: &Path,
        strategy: AdaptationStrategy,
        target_domain: &str,
    ) -> Result<TransferResult> {
        let source_db = self.resolve_source_db(source)?;

        info!("Transferring patterns with {:?} adaptation to domain '{}'", strategy, target_domain);

        let transferable = self.get_patterns_from_source(&source_db)?;

        // Adapt patterns based on strategy
        let adapted: Vec<_> = transferable
            .into_iter()
            .filter_map(|mut p| {
                match strategy {
                    AdaptationStrategy::Direct => Some(p),
                    AdaptationStrategy::Contextualize => {
                        // Add domain context to the pattern
                        p.context_query = format!("[{}] {}", target_domain, p.context_query);
                        Some(p)
                    }
                    AdaptationStrategy::Generalize => {
                        // Remove domain-specific paths and names
                        p.context_query = generalize_context(&p.context_query);
                        Some(p)
                    }
                    AdaptationStrategy::Specialize => {
                        // Only transfer if relevant to target domain
                        if is_relevant_to_domain(&p.context_query, target_domain) {
                            Some(p)
                        } else {
                            None
                        }
                    }
                }
            })
            .filter(|p| {
                let score = p.success_count - p.failure_count;
                let total = p.success_count + p.failure_count;
                let success_rate = if total > 0 {
                    p.success_count as f64 / total as f64
                } else {
                    0.0
                };

                score >= self.config.min_score && success_rate >= self.config.min_success_rate
            })
            .collect();

        info!("Adapted {} patterns for transfer", adapted.len());

        let result = self.import_patterns(dest_db, adapted)?;

        let causal_edges_transferred = if result.imported > 0 || result.merged > 0 {
            self.transfer_causal_edges(&source_db, dest_db)?
        } else {
            0
        };

        let skills_transferred = self.transfer_skills(&source_db, dest_db)?;

        Ok(TransferResult {
            patterns_transferred: result.imported,
            patterns_merged: result.merged,
            patterns_skipped: result.skipped,
            skills_transferred,
            causal_edges_transferred,
            source: source_db.to_string_lossy().to_string(),
            destination: dest_db.to_string_lossy().to_string(),
        })
    }

    /// Transfer RL policy (Q-table) between contexts
    pub fn transfer_policy(
        &self,
        source: &TransferSource,
        dest_db: &Path,
    ) -> Result<PolicyTransferResult> {
        let source_db = self.resolve_source_db(source)?;

        info!("Transferring Q-learning policy from {:?} to {:?}", source_db, dest_db);

        let source_conn = Connection::open(&source_db)?;
        let dest_conn = Connection::open(dest_db)?;

        // Check if Q-table exists in source
        let table_exists: bool = source_conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='q_table'",
                [],
                |row| Ok(row.get::<_, i64>(0)? > 0)
            )
            .unwrap_or(false);

        if !table_exists {
            warn!("No Q-table found in source database");
            return Ok(PolicyTransferResult {
                states_transferred: 0,
                actions_transferred: 0,
                q_values_adapted: 0,
            });
        }

        // Ensure Q-table exists in destination
        dest_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS q_table (
                context_hash INTEGER NOT NULL,
                pattern_id INTEGER NOT NULL,
                q_value REAL NOT NULL,
                visit_count INTEGER DEFAULT 0,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (context_hash, pattern_id)
            )"
        )?;

        // Transfer Q-values
        let mut stmt = source_conn.prepare(
            "SELECT context_hash, pattern_id, q_value, visit_count FROM q_table"
        )?;

        let q_entries: Vec<(i64, i64, f64, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut states_transferred = 0;
        let mut actions_transferred = 0;
        let mut q_values_adapted = 0;

        for (context_hash, pattern_id, q_value, visit_count) in q_entries {
            // Check if entry exists in destination
            let exists: bool = dest_conn
                .query_row(
                    "SELECT 1 FROM q_table WHERE context_hash = ?1 AND pattern_id = ?2",
                    params![context_hash, pattern_id],
                    |_| Ok(true)
                )
                .unwrap_or(false);

            if exists {
                // Merge Q-values using weighted average based on visit counts
                dest_conn.execute(
                    "UPDATE q_table
                     SET q_value = (q_value * visit_count + ?1 * ?2) / (visit_count + ?2),
                         visit_count = visit_count + ?2,
                         last_updated = CURRENT_TIMESTAMP
                     WHERE context_hash = ?3 AND pattern_id = ?4",
                    params![q_value, visit_count, context_hash, pattern_id]
                )?;
                q_values_adapted += 1;
            } else {
                // Insert new entry
                dest_conn.execute(
                    "INSERT INTO q_table (context_hash, pattern_id, q_value, visit_count)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![context_hash, pattern_id, q_value, visit_count]
                )?;
                states_transferred += 1;
            }

            actions_transferred += 1;
        }

        info!("Transferred {} Q-table entries ({} new states, {} adapted)",
            actions_transferred, states_transferred, q_values_adapted);

        Ok(PolicyTransferResult {
            states_transferred,
            actions_transferred,
            q_values_adapted,
        })
    }

    /// Get transferable patterns from source
    pub fn get_transferable(
        &self,
        source: &TransferSource,
    ) -> Result<Vec<TransferablePattern>> {
        let source_db = self.resolve_source_db(source)?;
        let conn = Connection::open(&source_db)?;

        let mut stmt = conn.prepare(
            "SELECT id, tool_type, context_query, success_count, failure_count,
                    COALESCE(command_category, '') as tier,
                    julianday('now') - julianday(created_at) as age_days
             FROM patterns
             ORDER BY (success_count - failure_count) DESC"
        )?;

        let patterns = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let tool_type: String = row.get(1)?;
            let context_query: String = row.get(2)?;
            let success_count: i64 = row.get(3)?;
            let failure_count: i64 = row.get(4)?;
            let tier: String = row.get(5)?;
            let age_days: f64 = row.get(6).unwrap_or(0.0);

            let score = success_count - failure_count;
            let total = success_count + failure_count;
            let success_rate = if total > 0 {
                success_count as f64 / total as f64
            } else {
                0.0
            };

            let transferability_score = calculate_transferability(
                score,
                success_rate,
                total,
                age_days as i64,
            );

            // Create context preview (first 100 chars)
            let context_preview = if context_query.len() > 100 {
                format!("{}...", &context_query[..97])
            } else {
                context_query.clone()
            };

            Ok(TransferablePattern {
                pattern_id: id,
                tool_type,
                score,
                success_rate,
                context_preview,
                tier,
                transferability_score,
            })
        })?;

        let result: Vec<_> = patterns.filter_map(|r| r.ok()).collect();
        Ok(result)
    }

    /// Preview what would be transferred
    pub fn preview_transfer(
        &self,
        source: &TransferSource,
        dest_db: &Path,
    ) -> Result<TransferPreview> {
        let transferable = self.get_transferable(source)?;
        let total_patterns = transferable.len();

        let eligible_patterns = transferable.iter()
            .filter(|p| {
                p.score >= self.config.min_score &&
                p.success_rate >= self.config.min_success_rate
            })
            .count();

        // Check how many would merge
        let dest_conn = Connection::open(dest_db)?;
        let mut would_merge = 0;
        let mut conflicts = Vec::new();

        for pattern in &transferable {
            if pattern.score < self.config.min_score || pattern.success_rate < self.config.min_success_rate {
                continue;
            }

            // Check if pattern exists (by tool_type and context similarity)
            let exists: bool = dest_conn
                .query_row(
                    "SELECT 1 FROM patterns WHERE tool_type = ?1 LIMIT 1",
                    params![&pattern.tool_type],
                    |_| Ok(true)
                )
                .unwrap_or(false);

            if exists {
                would_merge += 1;
            }
        }

        let would_skip = total_patterns - eligible_patterns;

        // Estimate benefit (average transferability of eligible patterns)
        let estimated_benefit = if eligible_patterns > 0 {
            transferable.iter()
                .filter(|p| {
                    p.score >= self.config.min_score &&
                    p.success_rate >= self.config.min_success_rate
                })
                .map(|p| p.transferability_score)
                .sum::<f64>() / eligible_patterns as f64
        } else {
            0.0
        };

        if eligible_patterns == 0 {
            conflicts.push("No patterns meet the transfer criteria".to_string());
        }

        Ok(TransferPreview {
            total_patterns,
            eligible_patterns,
            would_merge,
            would_skip,
            estimated_benefit,
            conflicts,
        })
    }

    // Helper methods

    fn resolve_source_db(&self, source: &TransferSource) -> Result<PathBuf> {
        match source {
            TransferSource::Database(path) => Ok(path.clone()),
            TransferSource::Project(path) => {
                let project_path = PathBuf::from(path);
                let db_path = project_path.join(".mana").join("metadata.sqlite");
                if !db_path.exists() {
                    return Err(anyhow!("Project database not found at {:?}", db_path));
                }
                Ok(db_path)
            }
            TransferSource::Session(session_id) => {
                // Look for session in Claude logs
                let home = dirs::home_dir()
                    .ok_or_else(|| anyhow!("Could not find home directory"))?;
                let session_path = home.join(".claude").join("projects").join(session_id);
                let db_path = session_path.join(".mana").join("metadata.sqlite");
                if !db_path.exists() {
                    return Err(anyhow!("Session database not found at {:?}", db_path));
                }
                Ok(db_path)
            }
            TransferSource::Export(_path) => {
                Err(anyhow!("Export source not yet implemented - use Database source instead"))
            }
        }
    }

    fn get_patterns_from_source(&self, source_db: &Path) -> Result<Vec<ExportablePattern>> {
        let security = SecurityConfig {
            sanitize_paths: false,
            redact_secrets: false,
            encrypt: false,
            visibility: crate::sync::Visibility::Private,
        };

        export_patterns_to_vec(source_db, &security)
    }

    fn import_patterns(&self, dest_db: &Path, patterns: Vec<ExportablePattern>) -> Result<ImportResult> {
        let merge_strategy = if self.config.merge_duplicates {
            MergeStrategy::KeepBest
        } else {
            MergeStrategy::Add
        };

        import_patterns_from_vec(dest_db, patterns, merge_strategy)
    }

    fn transfer_causal_edges(&self, source_db: &Path, dest_db: &Path) -> Result<usize> {
        let source_conn = Connection::open(source_db)?;
        let dest_conn = Connection::open(dest_db)?;

        // Check if causal_edges table exists in source
        let table_exists: bool = source_conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='causal_edges'",
                [],
                |row| Ok(row.get::<_, i64>(0)? > 0)
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(0);
        }

        // Get causal edges with pattern hashes
        let mut stmt = source_conn.prepare(
            "SELECT
                (SELECT pattern_hash FROM patterns WHERE id = ce.pattern_a_id) as hash_a,
                (SELECT pattern_hash FROM patterns WHERE id = ce.pattern_b_id) as hash_b,
                ce.lift,
                ce.co_occurrences
             FROM causal_edges ce
             WHERE lift > 1.5 OR lift < 0.5"  // Only transfer significant edges
        )?;

        let edges: Vec<(String, String, f64, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut transferred = 0;

        for (hash_a, hash_b, lift, co_occurrences) in edges {
            // Find pattern IDs in destination
            let id_a: Option<i64> = dest_conn
                .query_row(
                    "SELECT id FROM patterns WHERE pattern_hash = ?1",
                    params![hash_a],
                    |row| row.get(0)
                )
                .ok();

            let id_b: Option<i64> = dest_conn
                .query_row(
                    "SELECT id FROM patterns WHERE pattern_hash = ?1",
                    params![hash_b],
                    |row| row.get(0)
                )
                .ok();

            if let (Some(a), Some(b)) = (id_a, id_b) {
                // Insert or update causal edge
                dest_conn.execute(
                    "INSERT INTO causal_edges (pattern_a_id, pattern_b_id, lift, co_occurrences)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(pattern_a_id, pattern_b_id) DO UPDATE SET
                         lift = (lift * co_occurrences + ?3 * ?4) / (co_occurrences + ?4),
                         co_occurrences = co_occurrences + ?4,
                         updated_at = CURRENT_TIMESTAMP",
                    params![a, b, lift, co_occurrences]
                )?;
                transferred += 1;
            }
        }

        info!("Transferred {} causal edges", transferred);
        Ok(transferred)
    }

    fn transfer_skills(&self, source_db: &Path, dest_db: &Path) -> Result<usize> {
        let source_conn = Connection::open(source_db)?;
        let dest_conn = Connection::open(dest_db)?;

        // Check if skills table exists
        let table_exists: bool = source_conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skills'",
                [],
                |row| Ok(row.get::<_, i64>(0)? > 0)
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(0);
        }

        // Get skills from source
        let mut stmt = source_conn.prepare(
            "SELECT name, description, pattern_ids FROM skills"
        )?;

        let skills: Vec<(String, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut transferred = 0;

        for (name, description, _pattern_ids) in skills {
            // Insert skill (pattern_ids will be remapped later if needed)
            let result = dest_conn.execute(
                "INSERT OR IGNORE INTO skills (name, description)
                 VALUES (?1, ?2)",
                params![name, description]
            );

            if result.is_ok() && result.unwrap() > 0 {
                transferred += 1;
            }
        }

        info!("Transferred {} skills", transferred);
        Ok(transferred)
    }
}

#[derive(Debug, Clone)]
pub struct TransferablePattern {
    pub pattern_id: i64,
    pub tool_type: String,
    pub score: i64,
    pub success_rate: f64,
    pub context_preview: String,
    pub tier: String,
    pub transferability_score: f64,
}

#[derive(Debug, Clone)]
pub struct TransferPreview {
    pub total_patterns: usize,
    pub eligible_patterns: usize,
    pub would_merge: usize,
    pub would_skip: usize,
    pub estimated_benefit: f64,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PolicyTransferResult {
    pub states_transferred: usize,
    pub actions_transferred: usize,
    pub q_values_adapted: usize,
}

/// Calculate transferability score for a pattern
pub fn calculate_transferability(
    score: i64,
    success_rate: f64,
    usage_count: i64,
    age_days: i64,
) -> f64 {
    let score_factor = (score as f64).max(0.0) / 10.0;
    let success_factor = success_rate;
    let usage_factor = (usage_count as f64).min(50.0) / 50.0;
    let freshness_factor = 1.0 / (1.0 + (age_days as f64 / 30.0));

    0.4 * success_factor + 0.3 * score_factor + 0.2 * usage_factor + 0.1 * freshness_factor
}

// Domain adaptation helpers

fn generalize_context(context: &str) -> String {
    // Remove absolute paths
    let mut result = context.to_string();

    // Replace common absolute path patterns with placeholders
    result = result.replace("/home/", "~/");
    result = result.replace("/Users/", "~/");
    result = result.replace("/workspaces/", "~/workspace/");

    // Remove specific project names (heuristic: paths with multiple segments)
    // This is a simple approach - could be enhanced with regex

    result
}

fn is_relevant_to_domain(context: &str, domain: &str) -> bool {
    let context_lower = context.to_lowercase();
    let domain_lower = domain.to_lowercase();

    // Check if domain keywords appear in context
    domain_lower.split_whitespace()
        .any(|keyword| context_lower.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_transferability() {
        // High score, high success rate, recent
        let score1 = calculate_transferability(10, 0.9, 50, 1);
        assert!(score1 > 0.7);

        // Low score, low success rate, old
        let score2 = calculate_transferability(-5, 0.3, 5, 90);
        assert!(score2 < 0.4);

        // Perfect pattern
        let score3 = calculate_transferability(20, 1.0, 100, 0);
        assert!(score3 > 0.9);
    }

    #[test]
    fn test_generalize_context() {
        let context = "/home/user/project/src/main.rs";
        let generalized = generalize_context(context);
        assert_eq!(generalized, "~/user/project/src/main.rs");

        let context2 = "/workspaces/my-project/test";
        let generalized2 = generalize_context(context2);
        assert_eq!(generalized2, "~/workspace/my-project/test");
    }

    #[test]
    fn test_is_relevant_to_domain() {
        assert!(is_relevant_to_domain("rust cargo build", "rust"));
        assert!(is_relevant_to_domain("npm install package", "npm"));
        assert!(!is_relevant_to_domain("python script.py", "rust"));
    }

    #[test]
    fn test_transfer_config_default() {
        let config = TransferConfig::default();
        assert_eq!(config.min_score, 0);
        assert_eq!(config.min_success_rate, 0.5);
        assert!(config.merge_duplicates);
    }
}
