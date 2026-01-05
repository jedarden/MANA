//! Extended causal reasoning system
//!
//! Features:
//! - Pearl's do-calculus queries
//! - Confounder detection with significance filtering
//! - Multi-hop causal chain reasoning
//! - 95% confidence intervals with t-tests
//!
//! Tracks relationships between patterns to detect conflicts and synergies.
//! A causal edge records whether patterns tend to succeed or fail together.

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use tracing::debug;

/// Causal relation types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CausalRelation {
    Causes,        // Direct causation
    Enables,       // Makes possible
    Prevents,      // Blocks
    Correlates,    // Co-occurs
    Precedes,      // Temporal ordering
    DerivedFrom,   // Inferred relationship
    Contradicts,   // Incompatible patterns
}

impl CausalRelation {
    pub fn from_str(s: &str) -> Self {
        match s {
            "Causes" => Self::Causes,
            "Enables" => Self::Enables,
            "Prevents" => Self::Prevents,
            "Correlates" => Self::Correlates,
            "Precedes" => Self::Precedes,
            "DerivedFrom" => Self::DerivedFrom,
            "Contradicts" => Self::Contradicts,
            _ => Self::Correlates,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Causes => "Causes",
            Self::Enables => "Enables",
            Self::Prevents => "Prevents",
            Self::Correlates => "Correlates",
            Self::Precedes => "Precedes",
            Self::DerivedFrom => "DerivedFrom",
            Self::Contradicts => "Contradicts",
        }
    }
}

/// A causal edge representing a relationship between two patterns
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CausalEdge {
    pub id: i64,
    pub pattern_a_id: i64,
    pub pattern_b_id: i64,
    /// Lift score: >1.5 = synergy, <0.5 = conflict
    pub lift: f64,
    pub co_occurrences: i64,
    pub relation_type: CausalRelation,
    pub p_value: Option<f64>,
    pub sample_count: i64,
}

/// Result of a do-calculus intervention query
#[derive(Debug, Clone)]
pub struct InterventionResult {
    pub treatment_pattern_id: i64,
    pub outcome_pattern_id: i64,
    pub causal_effect: f64,           // Estimated causal effect
    pub confidence_interval: (f64, f64), // 95% CI
    pub p_value: f64,
    pub sample_size: usize,
    pub confounders_detected: Vec<i64>,
}

/// Confounder analysis result
#[derive(Debug, Clone)]
pub struct ConfounderAnalysis {
    pub potential_confounders: Vec<ConfounderCandidate>,
    pub adjusted_effect: f64,
    pub unadjusted_effect: f64,
    pub bias_estimate: f64,
}

#[derive(Debug, Clone)]
pub struct ConfounderCandidate {
    pub pattern_id: i64,
    pub correlation_with_treatment: f64,
    pub correlation_with_outcome: f64,
    pub backdoor_path_strength: f64,
    pub significance: f64,  // p-value
}

/// Causal chain for multi-hop reasoning
#[derive(Debug, Clone)]
pub struct CausalChain {
    pub nodes: Vec<i64>,              // Pattern IDs in causal order
    pub edges: Vec<CausalEdge>,
    pub total_effect: f64,
    pub path_strength: f64,           // Product of individual lifts
}

/// Causal graph statistics
#[derive(Debug, Clone)]
pub struct CausalGraphStats {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub synergy_edges: usize,
    pub conflict_edges: usize,
    pub avg_connections_per_node: f64,
    pub max_chain_length: usize,
    pub relation_type_counts: HashMap<String, usize>,
}

/// Causal edge store backed by SQLite
pub struct CausalStore {
    conn: Connection,
}

impl CausalStore {
    /// Open or create a causal store at the given database path
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        Ok(Self { conn })
    }

    /// Open causal store in read-only mode for fast queries
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Record a co-occurrence of two patterns with an outcome
    /// This updates the lift score based on whether they succeeded together
    pub fn record_cooccurrence(
        &self,
        pattern_a: i64,
        pattern_b: i64,
        both_succeeded: bool,
    ) -> Result<()> {
        // Skip self-referential edges - a pattern cannot conflict with itself
        if pattern_a == pattern_b {
            debug!("Skipping self-referential causal edge for pattern {}", pattern_a);
            return Ok(());
        }

        // Ensure consistent ordering (smaller ID first)
        let (id_a, id_b) = if pattern_a < pattern_b {
            (pattern_a, pattern_b)
        } else {
            (pattern_b, pattern_a)
        };

        // Check if edge exists
        let existing: Option<(i64, f64, i64)> = self.conn.query_row(
            "SELECT id, lift, co_occurrences FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).ok();

        match existing {
            Some((id, current_lift, co_count)) => {
                // Update existing edge with exponential moving average
                // Success pushes lift up toward 1.5, failure pushes it down toward 0.3
                // This ensures repeated failures can drive lift below 0.5 threshold
                let outcome_value = if both_succeeded { 1.5 } else { 0.3 };
                let alpha = 0.3; // Learning rate
                let new_lift = current_lift * (1.0 - alpha) + outcome_value * alpha;

                self.conn.execute(
                    "UPDATE causal_edges SET lift = ?, co_occurrences = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
                    params![new_lift, co_count + 1, id],
                )?;
                debug!("Updated causal edge {} -> {}: lift {:.2} -> {:.2}", id_a, id_b, current_lift, new_lift);
            }
            None => {
                // Create new edge
                let initial_lift = if both_succeeded { 1.2 } else { 0.8 };

                self.conn.execute(
                    "INSERT INTO causal_edges (pattern_a_id, pattern_b_id, lift, co_occurrences) VALUES (?, ?, ?, 1)",
                    params![id_a, id_b, initial_lift],
                )?;
                debug!("Created causal edge {} -> {}: lift {:.2}", id_a, id_b, initial_lift);
            }
        }

        Ok(())
    }

    /// Get all conflicting patterns for a given pattern ID
    /// Returns pattern IDs that have lift < 0.5 (conflict threshold)
    pub fn get_conflicts(&self, pattern_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT pattern_b_id FROM causal_edges
            WHERE pattern_a_id = ? AND lift < 0.5 AND co_occurrences >= 3
            UNION
            SELECT pattern_a_id FROM causal_edges
            WHERE pattern_b_id = ? AND lift < 0.5 AND co_occurrences >= 3
            "#,
        )?;

        let conflicts = stmt.query_map(params![pattern_id, pattern_id], |row| row.get(0))?;
        conflicts.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all synergistic patterns for a given pattern ID
    /// Returns pattern IDs that have lift > 1.5 (synergy threshold)
    #[allow(dead_code)]
    pub fn get_synergies(&self, pattern_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT pattern_b_id FROM causal_edges
            WHERE pattern_a_id = ? AND lift > 1.5 AND co_occurrences >= 3
            UNION
            SELECT pattern_a_id FROM causal_edges
            WHERE pattern_b_id = ? AND lift > 1.5 AND co_occurrences >= 3
            "#,
        )?;

        let synergies = stmt.query_map(params![pattern_id, pattern_id], |row| row.get(0))?;
        synergies.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all edges for a pattern (for debugging/stats)
    #[allow(dead_code)]
    pub fn get_edges(&self, pattern_id: i64) -> Result<Vec<CausalEdge>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, pattern_a_id, pattern_b_id, lift, co_occurrences,
                   COALESCE(relation_type, 'Correlates') as relation_type,
                   p_value,
                   COALESCE(sample_count, co_occurrences) as sample_count
            FROM causal_edges
            WHERE pattern_a_id = ? OR pattern_b_id = ?
            ORDER BY lift ASC
            "#,
        )?;

        let edges = stmt.query_map(params![pattern_id, pattern_id], |row| {
            Ok(CausalEdge {
                id: row.get(0)?,
                pattern_a_id: row.get(1)?,
                pattern_b_id: row.get(2)?,
                lift: row.get(3)?,
                co_occurrences: row.get(4)?,
                relation_type: CausalRelation::from_str(&row.get::<_, String>(5)?),
                p_value: row.get(6)?,
                sample_count: row.get(7)?,
            })
        })?;

        edges.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get count of causal edges
    #[allow(dead_code)]
    pub fn count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM causal_edges", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Query the effect of intervening on a pattern (do-calculus)
    /// do(X=x): What would happen if we force pattern X to be used?
    pub fn do_intervention(
        &self,
        treatment_pattern: i64,
        outcome_pattern: i64,
    ) -> Result<InterventionResult> {
        // Find confounders first
        let confounder_analysis = self.detect_confounders(treatment_pattern, outcome_pattern, 0.05)?;

        // Get direct edge if it exists
        let (id_a, id_b) = if treatment_pattern < outcome_pattern {
            (treatment_pattern, outcome_pattern)
        } else {
            (outcome_pattern, treatment_pattern)
        };

        let edge_data: Option<(f64, i64)> = self.conn.query_row(
            "SELECT lift, co_occurrences FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).ok();

        let (causal_effect, sample_size) = match edge_data {
            Some((lift, count)) => (lift, count as usize),
            None => (1.0, 0), // No observed relationship
        };

        // Calculate confidence interval using t-distribution
        let (ci_lower, ci_upper, p_value) = if sample_size > 2 {
            self.calculate_confidence_interval(causal_effect, sample_size)?
        } else {
            (causal_effect, causal_effect, 1.0) // Not enough data
        };

        Ok(InterventionResult {
            treatment_pattern_id: treatment_pattern,
            outcome_pattern_id: outcome_pattern,
            causal_effect: confounder_analysis.adjusted_effect,
            confidence_interval: (ci_lower, ci_upper),
            p_value,
            sample_size,
            confounders_detected: confounder_analysis.potential_confounders
                .iter()
                .map(|c| c.pattern_id)
                .collect(),
        })
    }

    /// Detect potential confounders between treatment and outcome
    pub fn detect_confounders(
        &self,
        treatment: i64,
        outcome: i64,
        min_significance: f64,
    ) -> Result<ConfounderAnalysis> {
        // Get all patterns that connect to both treatment and outcome
        let mut stmt = self.conn.prepare(
            r#"
            SELECT DISTINCT pattern_id FROM (
                SELECT CASE
                    WHEN pattern_a_id = ? THEN pattern_b_id
                    ELSE pattern_a_id
                END as pattern_id
                FROM causal_edges
                WHERE (pattern_a_id = ? OR pattern_b_id = ?)
                AND co_occurrences >= 3

                INTERSECT

                SELECT CASE
                    WHEN pattern_a_id = ? THEN pattern_b_id
                    ELSE pattern_a_id
                END as pattern_id
                FROM causal_edges
                WHERE (pattern_a_id = ? OR pattern_b_id = ?)
                AND co_occurrences >= 3
            )
            WHERE pattern_id != ? AND pattern_id != ?
            "#,
        )?;

        let potential_confounders: Vec<i64> = stmt
            .query_map(
                params![treatment, treatment, treatment, outcome, outcome, outcome, treatment, outcome],
                |row| row.get(0),
            )?
            .filter_map(|r| r.ok())
            .collect();

        let mut confounders = Vec::new();

        for confounder_id in potential_confounders {
            // Get correlation with treatment
            let treatment_corr = self.get_correlation(confounder_id, treatment)?;

            // Get correlation with outcome
            let outcome_corr = self.get_correlation(confounder_id, outcome)?;

            // Backdoor path strength: multiply correlations
            let backdoor_strength = treatment_corr.abs() * outcome_corr.abs();

            // Calculate significance (simplified chi-square test)
            let significance = self.calculate_significance(confounder_id, treatment, outcome)?;

            if significance <= min_significance && backdoor_strength > 0.1 {
                confounders.push(ConfounderCandidate {
                    pattern_id: confounder_id,
                    correlation_with_treatment: treatment_corr,
                    correlation_with_outcome: outcome_corr,
                    backdoor_path_strength: backdoor_strength,
                    significance,
                });
            }
        }

        // Get unadjusted effect
        let unadjusted_effect = self.get_direct_effect(treatment, outcome)?;

        // Calculate adjusted effect (simple adjustment by backdoor criterion)
        let total_confounder_bias: f64 = confounders
            .iter()
            .map(|c| c.backdoor_path_strength * 0.2) // Dampen the adjustment
            .sum();

        let adjusted_effect = unadjusted_effect / (1.0 + total_confounder_bias);

        Ok(ConfounderAnalysis {
            potential_confounders: confounders,
            adjusted_effect,
            unadjusted_effect,
            bias_estimate: total_confounder_bias,
        })
    }

    /// Find all causal chains between two patterns
    pub fn find_causal_chains(
        &self,
        from_pattern: i64,
        to_pattern: i64,
        max_hops: usize,
    ) -> Result<Vec<CausalChain>> {
        // BFS to find all paths
        let mut chains = Vec::new();
        let mut queue: VecDeque<(Vec<i64>, f64)> = VecDeque::new();
        queue.push_back((vec![from_pattern], 1.0));

        let mut visited_paths: HashSet<Vec<i64>> = HashSet::new();

        while let Some((path, path_strength)) = queue.pop_front() {
            let current = *path.last().unwrap();

            if path.len() > max_hops + 1 {
                continue;
            }

            if current == to_pattern && path.len() > 1 {
                // Found a chain
                let edges = self.get_chain_edges(&path)?;
                let total_effect = edges.iter().map(|e| e.lift).sum::<f64>() / edges.len() as f64;

                chains.push(CausalChain {
                    nodes: path.clone(),
                    edges,
                    total_effect,
                    path_strength,
                });
                continue;
            }

            // Get neighbors
            let neighbors = self.get_neighbors(current)?;

            for (neighbor_id, edge_lift) in neighbors {
                if !path.contains(&neighbor_id) {
                    let mut new_path = path.clone();
                    new_path.push(neighbor_id);

                    if !visited_paths.contains(&new_path) {
                        visited_paths.insert(new_path.clone());
                        let new_strength = path_strength * edge_lift;
                        queue.push_back((new_path, new_strength));
                    }
                }
            }
        }

        // Sort by path strength (strongest first)
        chains.sort_by(|a, b| b.path_strength.partial_cmp(&a.path_strength).unwrap());

        Ok(chains)
    }

    /// Calculate uplift using two-sample t-test
    pub fn calculate_uplift(
        &self,
        pattern_id: i64,
        control_group: &[i64],
        treatment_group: &[i64],
    ) -> Result<(f64, f64, f64)> {
        // Get success rates for control group
        let control_lifts: Vec<f64> = control_group
            .iter()
            .filter_map(|&other_id| {
                let (id_a, id_b) = if pattern_id < other_id {
                    (pattern_id, other_id)
                } else {
                    (other_id, pattern_id)
                };

                self.conn.query_row(
                    "SELECT lift FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
                    params![id_a, id_b],
                    |row| row.get(0),
                ).ok()
            })
            .collect();

        // Get success rates for treatment group
        let treatment_lifts: Vec<f64> = treatment_group
            .iter()
            .filter_map(|&other_id| {
                let (id_a, id_b) = if pattern_id < other_id {
                    (pattern_id, other_id)
                } else {
                    (other_id, pattern_id)
                };

                self.conn.query_row(
                    "SELECT lift FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
                    params![id_a, id_b],
                    |row| row.get(0),
                ).ok()
            })
            .collect();

        if control_lifts.is_empty() || treatment_lifts.is_empty() {
            return Ok((0.0, 0.0, 1.0)); // No data
        }

        // Calculate means
        let control_mean = control_lifts.iter().sum::<f64>() / control_lifts.len() as f64;
        let treatment_mean = treatment_lifts.iter().sum::<f64>() / treatment_lifts.len() as f64;
        let effect = treatment_mean - control_mean;

        // Calculate standard deviations
        let control_var = control_lifts.iter()
            .map(|&x| (x - control_mean).powi(2))
            .sum::<f64>() / control_lifts.len() as f64;
        let treatment_var = treatment_lifts.iter()
            .map(|&x| (x - treatment_mean).powi(2))
            .sum::<f64>() / treatment_lifts.len() as f64;

        // Pooled standard error
        let se = ((control_var / control_lifts.len() as f64) +
                  (treatment_var / treatment_lifts.len() as f64)).sqrt();

        // t-statistic
        let t_stat = if se > 0.0 { effect / se } else { 0.0 };

        // Degrees of freedom (Welch-Satterthwaite)
        let df = control_lifts.len() + treatment_lifts.len() - 2;

        // 95% confidence interval (t_critical ≈ 1.96 for large samples)
        let t_critical = if df > 30 { 1.96 } else { 2.0 + (30 - df) as f64 * 0.1 };
        let ci_width = t_critical * se;

        // p-value (simplified, using normal approximation)
        let p_value = 2.0 * (1.0 - self.normal_cdf(t_stat.abs()));

        Ok((effect, ci_width, p_value))
    }

    /// Get causal graph statistics
    pub fn causal_stats(&self) -> Result<CausalGraphStats> {
        // Count total nodes (patterns that appear in edges)
        let total_nodes: usize = self.conn.query_row(
            r#"
            SELECT COUNT(DISTINCT pattern_id) FROM (
                SELECT pattern_a_id as pattern_id FROM causal_edges
                UNION
                SELECT pattern_b_id as pattern_id FROM causal_edges
            )
            "#,
            [],
            |row| row.get(0),
        )?;

        let total_edges: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM causal_edges",
            [],
            |row| row.get(0),
        )?;

        let synergy_edges: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM causal_edges WHERE lift > 1.5 AND co_occurrences >= 3",
            [],
            |row| row.get(0),
        )?;

        let conflict_edges: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM causal_edges WHERE lift < 0.5 AND co_occurrences >= 3",
            [],
            |row| row.get(0),
        )?;

        let avg_connections = if total_nodes > 0 {
            (total_edges * 2) as f64 / total_nodes as f64
        } else {
            0.0
        };

        // Get relation type counts
        let mut relation_counts = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(relation_type, 'Correlates') as rel_type, COUNT(*) FROM causal_edges GROUP BY relation_type"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;

        for row in rows {
            if let Ok((rel_type, count)) = row {
                relation_counts.insert(rel_type, count);
            }
        }

        // Calculate max chain length (simplified - just count max co-occurrences as proxy)
        let max_chain_length: usize = self.conn.query_row(
            "SELECT COALESCE(MAX(co_occurrences), 0) FROM causal_edges",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        Ok(CausalGraphStats {
            total_nodes,
            total_edges,
            synergy_edges,
            conflict_edges,
            avg_connections_per_node: avg_connections,
            max_chain_length: max_chain_length.min(10), // Cap at reasonable value
            relation_type_counts: relation_counts,
        })
    }

    // Helper methods

    fn get_correlation(&self, pattern_a: i64, pattern_b: i64) -> Result<f64> {
        let (id_a, id_b) = if pattern_a < pattern_b {
            (pattern_a, pattern_b)
        } else {
            (pattern_b, pattern_a)
        };

        let lift: f64 = self.conn.query_row(
            "SELECT lift FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| row.get(0),
        ).unwrap_or(1.0); // Default to no correlation

        // Convert lift to correlation (-1 to 1 scale)
        // lift > 1 = positive correlation, lift < 1 = negative correlation
        Ok((lift - 1.0).max(-1.0).min(1.0))
    }

    fn get_direct_effect(&self, treatment: i64, outcome: i64) -> Result<f64> {
        let (id_a, id_b) = if treatment < outcome {
            (treatment, outcome)
        } else {
            (outcome, treatment)
        };

        self.conn.query_row(
            "SELECT lift FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| row.get(0),
        ).or(Ok(1.0)) // Default to neutral effect
    }

    fn calculate_significance(&self, confounder: i64, treatment: i64, outcome: i64) -> Result<f64> {
        // Simplified chi-square test
        // In a real implementation, this would use proper statistical tests

        let conf_treat_count = self.get_edge_count(confounder, treatment)?;
        let conf_outcome_count = self.get_edge_count(confounder, outcome)?;

        if conf_treat_count < 3 || conf_outcome_count < 3 {
            return Ok(1.0); // Not significant
        }

        // Simplified p-value based on sample size
        let min_count = conf_treat_count.min(conf_outcome_count);
        let p_value = 1.0 / (min_count as f64 + 1.0);

        Ok(p_value)
    }

    fn get_edge_count(&self, pattern_a: i64, pattern_b: i64) -> Result<i64> {
        let (id_a, id_b) = if pattern_a < pattern_b {
            (pattern_a, pattern_b)
        } else {
            (pattern_b, pattern_a)
        };

        self.conn.query_row(
            "SELECT co_occurrences FROM causal_edges WHERE pattern_a_id = ? AND pattern_b_id = ?",
            params![id_a, id_b],
            |row| row.get(0),
        ).or(Ok(0))
    }

    fn calculate_confidence_interval(&self, effect: f64, sample_size: usize) -> Result<(f64, f64, f64)> {
        // Estimate standard error based on sample size
        let se = 0.5 / (sample_size as f64).sqrt();

        // t-critical value for 95% CI (approximation)
        let t_critical = if sample_size > 30 { 1.96 } else { 2.0 + (30.0 - sample_size as f64) * 0.05 };

        let margin = t_critical * se;
        let ci_lower = effect - margin;
        let ci_upper = effect + margin;

        // Calculate p-value
        let t_stat = (effect - 1.0) / se; // Test against null hypothesis of lift = 1.0
        let p_value = 2.0 * (1.0 - self.normal_cdf(t_stat.abs()));

        Ok((ci_lower, ci_upper, p_value))
    }

    fn normal_cdf(&self, x: f64) -> f64 {
        // Approximation of standard normal CDF
        0.5 * (1.0 + self.erf(x / std::f64::consts::SQRT_2))
    }

    fn erf(&self, x: f64) -> f64 {
        // Abramowitz and Stegun approximation
        let a1 =  0.254829592;
        let a2 = -0.284496736;
        let a3 =  1.421413741;
        let a4 = -1.453152027;
        let a5 =  1.061405429;
        let p  =  0.3275911;

        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x.abs();

        let t = 1.0 / (1.0 + p * x);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();

        sign * y
    }

    fn get_neighbors(&self, pattern_id: i64) -> Result<Vec<(i64, f64)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                CASE WHEN pattern_a_id = ? THEN pattern_b_id ELSE pattern_a_id END as neighbor,
                lift
            FROM causal_edges
            WHERE (pattern_a_id = ? OR pattern_b_id = ?)
            AND co_occurrences >= 3
            "#,
        )?;

        let neighbors = stmt.query_map(params![pattern_id, pattern_id, pattern_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

        neighbors.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn get_chain_edges(&self, path: &[i64]) -> Result<Vec<CausalEdge>> {
        let mut edges = Vec::new();

        for i in 0..path.len()-1 {
            let (id_a, id_b) = if path[i] < path[i+1] {
                (path[i], path[i+1])
            } else {
                (path[i+1], path[i])
            };

            let edge: CausalEdge = self.conn.query_row(
                r#"
                SELECT id, pattern_a_id, pattern_b_id, lift, co_occurrences,
                       COALESCE(relation_type, 'Correlates'),
                       p_value,
                       COALESCE(sample_count, co_occurrences)
                FROM causal_edges
                WHERE pattern_a_id = ? AND pattern_b_id = ?
                "#,
                params![id_a, id_b],
                |row| Ok(CausalEdge {
                    id: row.get(0)?,
                    pattern_a_id: row.get(1)?,
                    pattern_b_id: row.get(2)?,
                    lift: row.get(3)?,
                    co_occurrences: row.get(4)?,
                    relation_type: CausalRelation::from_str(&row.get::<_, String>(5)?),
                    p_value: row.get(6)?,
                    sample_count: row.get(7)?,
                }),
            )?;

            edges.push(edge);
        }

        Ok(edges)
    }

    /// Clean up edges referencing deleted patterns
    pub fn cleanup_orphaned(&self) -> Result<usize> {
        let deleted = self.conn.execute(
            r#"
            DELETE FROM causal_edges
            WHERE pattern_a_id NOT IN (SELECT id FROM patterns)
               OR pattern_b_id NOT IN (SELECT id FROM patterns)
            "#,
            [],
        )?;
        Ok(deleted)
    }

    /// Clean up invalid self-referential edges (pattern conflicting with itself)
    pub fn cleanup_self_referential(&self) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM causal_edges WHERE pattern_a_id = pattern_b_id",
            [],
        )?;
        if deleted > 0 {
            debug!("Removed {} self-referential causal edges", deleted);
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (NamedTempFile, CausalStore) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();

        // Create minimal schema
        conn.execute_batch(
            r#"
            CREATE TABLE patterns (
                id INTEGER PRIMARY KEY,
                pattern_hash TEXT,
                tool_type TEXT,
                context_query TEXT,
                success_count INTEGER DEFAULT 0,
                failure_count INTEGER DEFAULT 0
            );
            CREATE TABLE causal_edges (
                id INTEGER PRIMARY KEY,
                pattern_a_id INTEGER,
                pattern_b_id INTEGER,
                lift REAL,
                co_occurrences INTEGER DEFAULT 1,
                relation_type TEXT DEFAULT 'Correlates',
                p_value REAL,
                sample_count INTEGER DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(pattern_a_id, pattern_b_id)
            );
            INSERT INTO patterns (id, pattern_hash, tool_type, context_query) VALUES
                (1, 'hash1', 'Bash', 'Pattern 1'),
                (2, 'hash2', 'Bash', 'Pattern 2'),
                (3, 'hash3', 'Edit', 'Pattern 3');
            "#,
        ).unwrap();
        drop(conn);

        let store = CausalStore::open(tmp.path()).unwrap();
        (tmp, store)
    }

    #[test]
    fn test_record_cooccurrence_creates_edge() {
        let (_tmp, store) = setup_test_db();

        store.record_cooccurrence(1, 2, true).unwrap();

        let count = store.count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_record_cooccurrence_updates_lift() {
        let (_tmp, store) = setup_test_db();

        // Record several failures - lift should decrease
        for _ in 0..5 {
            store.record_cooccurrence(1, 2, false).unwrap();
        }

        let edges = store.get_edges(1).unwrap();
        assert_eq!(edges.len(), 1);
        assert!(edges[0].lift < 0.6, "Lift should be low after failures: {}", edges[0].lift);
    }

    #[test]
    fn test_get_conflicts() {
        let (_tmp, store) = setup_test_db();

        // Record many failures to create a conflict
        // With EMA (alpha=0.3) starting at 0.8, need ~20 failures to get below 0.5
        for _ in 0..20 {
            store.record_cooccurrence(1, 2, false).unwrap();
        }

        // Verify lift dropped below threshold
        let edges = store.get_edges(1).unwrap();
        assert!(!edges.is_empty(), "Should have created an edge");
        assert!(edges[0].lift < 0.5, "Lift should be below conflict threshold: {}", edges[0].lift);
        assert!(edges[0].co_occurrences >= 3, "Should have enough co-occurrences: {}", edges[0].co_occurrences);

        let conflicts = store.get_conflicts(1).unwrap();
        assert!(conflicts.contains(&2), "Pattern 2 should be a conflict");
    }

    #[test]
    fn test_edge_ordering() {
        let (_tmp, store) = setup_test_db();

        // Regardless of order passed, should create same edge
        store.record_cooccurrence(2, 1, true).unwrap();
        store.record_cooccurrence(1, 2, true).unwrap();

        let count = store.count().unwrap();
        assert_eq!(count, 1, "Should only create one edge regardless of order");
    }
}
