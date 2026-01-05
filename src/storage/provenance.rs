//! Explainability and provenance tracking
//!
//! Features:
//! - Merkle tree proofs for audit trails
//! - Reasoning chain recording (Thought-Action-Observation)
//! - Action justification generation
//! - Causal chain with evidence tracking

use anyhow::{anyhow, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A provenance certificate for a pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceCertificate {
    pub pattern_id: i64,
    pub merkle_root: String,          // SHA256 hash
    pub creation_timestamp: i64,
    pub source_trajectories: Vec<String>, // Session IDs
    pub derivation_chain: Vec<DerivationStep>,
    pub confidence_factors: Vec<ConfidenceFactor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationStep {
    pub step_type: DerivationType,
    pub timestamp: i64,
    pub evidence: String,
    pub confidence_delta: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DerivationType {
    Extracted,        // Extracted from trajectory
    Merged,           // Merged with similar pattern
    Reflected,        // Updated by reflection system
    Reinforced,       // Success/failure feedback
    Decayed,          // Time-based decay
    UserFeedback,     // Explicit user input
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceFactor {
    pub factor_name: String,
    pub value: f64,
    pub weight: f64,
    pub evidence: Option<String>,
}

/// Reasoning chain for explainable decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningChainRecord {
    pub id: i64,
    pub pattern_id: i64,
    pub task_context: String,
    pub steps: Vec<ReasoningStep>,
    pub final_decision: String,
    pub confidence: f64,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    pub step_type: ReasoningStepType,
    pub content: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningStepType {
    Thought,      // Internal reasoning
    Action,       // Action taken
    Observation,  // Result observed
    Reflection,   // Meta-analysis
}

/// Action justification for explainable AI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionJustification {
    pub action_type: String,
    pub pattern_used: Option<i64>,
    pub reasoning: String,
    pub supporting_evidence: Vec<Evidence>,
    pub confidence: f64,
    pub alternatives_considered: Vec<Alternative>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub evidence_type: String,
    pub content: String,
    pub strength: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alternative {
    pub action: String,
    pub pattern_id: Option<i64>,
    pub score: f64,
    pub reason_rejected: String,
}

/// Provenance store for tracking pattern derivation and explainability
pub struct ProvenanceStore {
    conn: Connection,
}

impl ProvenanceStore {
    /// Create a new provenance store
    pub fn new(conn: Connection) -> Result<Self> {
        Ok(Self { conn })
    }

    /// Initialize provenance tables
    pub fn init_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS provenance (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern_id INTEGER NOT NULL,
                merkle_root TEXT NOT NULL,
                derivation_chain TEXT NOT NULL,  -- JSON
                confidence_factors TEXT NOT NULL, -- JSON
                source_trajectories TEXT NOT NULL, -- JSON array
                created_at INTEGER NOT NULL,
                FOREIGN KEY (pattern_id) REFERENCES patterns(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS reasoning_chains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern_id INTEGER,
                task_context TEXT NOT NULL,
                steps TEXT NOT NULL,  -- JSON
                final_decision TEXT NOT NULL,
                confidence REAL NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_provenance_pattern ON provenance(pattern_id);
            CREATE INDEX IF NOT EXISTS idx_reasoning_pattern ON reasoning_chains(pattern_id);
            CREATE INDEX IF NOT EXISTS idx_reasoning_timestamp ON reasoning_chains(created_at DESC);
            "#,
        )?;
        Ok(())
    }

    /// Generate Merkle proof for a pattern
    /// This creates a cryptographic audit trail by hashing the derivation chain
    pub fn generate_certificate(&self, pattern_id: i64) -> Result<ProvenanceCertificate> {
        // Get pattern information
        let pattern: (String, i64, i64, Option<String>) = self.conn.query_row(
            "SELECT pattern_hash, success_count, failure_count, last_used FROM patterns WHERE id = ?1",
            params![pattern_id],
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            )),
        )?;

        let (pattern_hash, success_count, failure_count, last_used) = pattern;

        // Get existing provenance or create new derivation chain
        let existing_prov: Option<(String, String, String, i64)> = self.conn
            .query_row(
                "SELECT derivation_chain, confidence_factors, source_trajectories, created_at FROM provenance WHERE pattern_id = ?1",
                params![pattern_id],
                |row| Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                )),
            )
            .optional()?;

        let (derivation_chain, confidence_factors, source_trajectories, creation_timestamp) = if let Some((dc, cf, st, ct)) = existing_prov {
            (
                serde_json::from_str::<Vec<DerivationStep>>(&dc)?,
                serde_json::from_str::<Vec<ConfidenceFactor>>(&cf)?,
                serde_json::from_str::<Vec<String>>(&st)?,
                ct,
            )
        } else {
            // Create initial derivation chain
            let now = current_timestamp();
            let initial_step = DerivationStep {
                step_type: DerivationType::Extracted,
                timestamp: now,
                evidence: format!("Pattern extracted with hash: {}", pattern_hash),
                confidence_delta: 0.5,
            };

            let initial_factors = vec![
                ConfidenceFactor {
                    factor_name: "success_rate".to_string(),
                    value: calculate_success_rate(success_count, failure_count),
                    weight: 0.4,
                    evidence: Some(format!("Success: {}, Failure: {}", success_count, failure_count)),
                },
                ConfidenceFactor {
                    factor_name: "usage_frequency".to_string(),
                    value: calculate_usage_score(success_count + failure_count),
                    weight: 0.3,
                    evidence: Some(format!("Total uses: {}", success_count + failure_count)),
                },
                ConfidenceFactor {
                    factor_name: "recency".to_string(),
                    value: calculate_recency_score(&last_used),
                    weight: 0.3,
                    evidence: last_used.clone(),
                },
            ];

            (vec![initial_step], initial_factors, Vec::new(), now)
        };

        // Calculate Merkle root from derivation chain
        let merkle_root = self.calculate_merkle_root(pattern_id, &derivation_chain)?;

        let certificate = ProvenanceCertificate {
            pattern_id,
            merkle_root,
            creation_timestamp,
            source_trajectories,
            derivation_chain,
            confidence_factors,
        };

        // Store or update the provenance record
        self.store_certificate(&certificate)?;

        Ok(certificate)
    }

    /// Verify a provenance certificate by recalculating its Merkle root
    pub fn verify_certificate(&self, cert: &ProvenanceCertificate) -> Result<bool> {
        let calculated_root = self.calculate_merkle_root(cert.pattern_id, &cert.derivation_chain)?;
        Ok(calculated_root == cert.merkle_root)
    }

    /// Calculate Merkle root from derivation chain using SHA256
    fn calculate_merkle_root(&self, pattern_id: i64, chain: &[DerivationStep]) -> Result<String> {
        // Build leaves from derivation chain
        let mut leaves: Vec<String> = chain.iter().map(|step| {
            // Hash each step
            let step_data = format!(
                "{}|{}|{}|{}",
                step.step_type as u8,
                step.timestamp,
                step.evidence,
                step.confidence_delta
            );
            sha256_hash(&step_data)
        }).collect();

        // Add pattern ID as first leaf for uniqueness
        leaves.insert(0, sha256_hash(&pattern_id.to_string()));

        // Build Merkle tree
        while leaves.len() > 1 {
            let mut next_level = Vec::new();
            for i in (0..leaves.len()).step_by(2) {
                if i + 1 < leaves.len() {
                    // Hash pair
                    let combined = format!("{}{}", leaves[i], leaves[i + 1]);
                    next_level.push(sha256_hash(&combined));
                } else {
                    // Odd one out - hash with itself
                    let combined = format!("{}{}", leaves[i], leaves[i]);
                    next_level.push(sha256_hash(&combined));
                }
            }
            leaves = next_level;
        }

        leaves.first()
            .cloned()
            .ok_or_else(|| anyhow!("Failed to calculate Merkle root"))
    }

    /// Store a provenance certificate
    fn store_certificate(&self, cert: &ProvenanceCertificate) -> Result<()> {
        let derivation_json = serde_json::to_string(&cert.derivation_chain)?;
        let confidence_json = serde_json::to_string(&cert.confidence_factors)?;
        let trajectories_json = serde_json::to_string(&cert.source_trajectories)?;

        self.conn.execute(
            r#"
            INSERT INTO provenance (pattern_id, merkle_root, derivation_chain, confidence_factors, source_trajectories, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(pattern_id) DO UPDATE SET
                merkle_root = excluded.merkle_root,
                derivation_chain = excluded.derivation_chain,
                confidence_factors = excluded.confidence_factors,
                source_trajectories = excluded.source_trajectories
            "#,
            params![
                cert.pattern_id,
                &cert.merkle_root,
                derivation_json,
                confidence_json,
                trajectories_json,
                cert.creation_timestamp,
            ],
        )?;

        Ok(())
    }

    /// Record a derivation step for a pattern
    pub fn record_derivation(&self, pattern_id: i64, step: DerivationStep) -> Result<()> {
        // Get existing certificate or create new one
        let mut cert = self.generate_certificate(pattern_id)?;

        // Add new step
        cert.derivation_chain.push(step);

        // Recalculate Merkle root
        cert.merkle_root = self.calculate_merkle_root(pattern_id, &cert.derivation_chain)?;

        // Update timestamp
        cert.creation_timestamp = current_timestamp();

        // Store updated certificate
        self.store_certificate(&cert)?;

        Ok(())
    }

    /// Get full provenance history for a pattern
    pub fn get_provenance(&self, pattern_id: i64) -> Result<ProvenanceCertificate> {
        self.generate_certificate(pattern_id)
    }

    /// Record a reasoning chain
    pub fn record_reasoning(&self, chain: &ReasoningChainRecord) -> Result<i64> {
        let steps_json = serde_json::to_string(&chain.steps)?;

        self.conn.execute(
            r#"
            INSERT INTO reasoning_chains (pattern_id, task_context, steps, final_decision, confidence, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                chain.pattern_id,
                &chain.task_context,
                steps_json,
                &chain.final_decision,
                chain.confidence,
                chain.timestamp,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Generate justification for an action
    pub fn justify_action(
        &self,
        action: &str,
        pattern_id: Option<i64>,
        alternatives: &[(i64, f64)],
    ) -> Result<ActionJustification> {
        let mut supporting_evidence = Vec::new();
        let mut alternatives_considered = Vec::new();

        // Get evidence from selected pattern
        if let Some(pid) = pattern_id {
            let cert = self.get_provenance(pid)?;

            // Add pattern provenance as evidence
            supporting_evidence.push(Evidence {
                evidence_type: "provenance".to_string(),
                content: format!("Pattern has {} derivation steps", cert.derivation_chain.len()),
                strength: 0.8,
                source: format!("Pattern #{}", pid),
            });

            // Add confidence factors as evidence
            for factor in &cert.confidence_factors {
                supporting_evidence.push(Evidence {
                    evidence_type: "confidence_factor".to_string(),
                    content: format!("{}: {:.2}", factor.factor_name, factor.value),
                    strength: factor.weight,
                    source: factor.evidence.clone().unwrap_or_default(),
                });
            }

            // Get pattern details
            let (context, success, failure): (String, i64, i64) = self.conn.query_row(
                "SELECT context_query, success_count, failure_count FROM patterns WHERE id = ?1",
                params![pid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

            supporting_evidence.push(Evidence {
                evidence_type: "historical_performance".to_string(),
                content: format!("Success rate: {:.1}%", calculate_success_rate(success, failure) * 100.0),
                strength: 0.9,
                source: format!("{} successes, {} failures", success, failure),
            });
        }

        // Build alternatives list
        for (alt_id, score) in alternatives {
            let (context, success, failure): (String, i64, i64) = self.conn.query_row(
                "SELECT context_query, success_count, failure_count FROM patterns WHERE id = ?1",
                params![alt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

            let preview = context.lines().take(1).collect::<Vec<_>>().join("");
            let preview = if preview.len() > 60 {
                format!("{}...", &preview[..60])
            } else {
                preview
            };

            alternatives_considered.push(Alternative {
                action: preview,
                pattern_id: Some(*alt_id),
                score: *score,
                reason_rejected: format!("Lower score ({:.2}) than selected pattern", score),
            });
        }

        // Calculate overall confidence
        let confidence = if let Some(pid) = pattern_id {
            let cert = self.get_provenance(pid)?;
            cert.confidence_factors.iter()
                .map(|f| f.value * f.weight)
                .sum::<f64>()
        } else {
            0.5
        };

        // Generate reasoning
        let reasoning = if let Some(pid) = pattern_id {
            format!(
                "Selected pattern #{} for action '{}' based on {} confidence factors and {} derivation steps. Confidence: {:.1}%",
                pid,
                action,
                supporting_evidence.len(),
                supporting_evidence.iter().find(|e| e.evidence_type == "provenance")
                    .and_then(|e| e.content.split_whitespace().nth(2))
                    .unwrap_or("0"),
                confidence * 100.0
            )
        } else {
            format!("No specific pattern selected for action '{}'. Using default behavior.", action)
        };

        Ok(ActionJustification {
            action_type: action.to_string(),
            pattern_used: pattern_id,
            reasoning,
            supporting_evidence,
            confidence,
            alternatives_considered,
        })
    }

    /// Get explanation for why a pattern was selected
    pub fn explain_selection(&self, pattern_id: i64, context: &str) -> Result<String> {
        let cert = self.get_provenance(pattern_id)?;

        let (pattern_hash, tool_type, context_query, success, failure): (String, String, String, i64, i64) =
            self.conn.query_row(
                "SELECT pattern_hash, tool_type, context_query, success_count, failure_count FROM patterns WHERE id = ?1",
                params![pattern_id],
                |row| Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                )),
            )?;

        let mut explanation = String::new();
        explanation.push_str(&format!("Pattern Selection Explanation for Pattern #{}\n", pattern_id));
        explanation.push_str(&format!("========================================\n\n"));

        explanation.push_str(&format!("Tool Type: {}\n", tool_type));
        explanation.push_str(&format!("Pattern Hash: {}\n", pattern_hash));
        explanation.push_str(&format!("Context: {}\n\n", context));

        explanation.push_str(&format!("Performance Metrics:\n"));
        explanation.push_str(&format!("  Success Count: {}\n", success));
        explanation.push_str(&format!("  Failure Count: {}\n", failure));
        explanation.push_str(&format!("  Success Rate: {:.1}%\n\n", calculate_success_rate(success, failure) * 100.0));

        explanation.push_str(&format!("Confidence Factors ({} total):\n", cert.confidence_factors.len()));
        for factor in &cert.confidence_factors {
            explanation.push_str(&format!("  {} = {:.2} (weight: {:.2})\n",
                factor.factor_name, factor.value, factor.weight));
            if let Some(evidence) = &factor.evidence {
                explanation.push_str(&format!("    Evidence: {}\n", evidence));
            }
        }
        explanation.push_str("\n");

        explanation.push_str(&format!("Derivation History ({} steps):\n", cert.derivation_chain.len()));
        for (i, step) in cert.derivation_chain.iter().enumerate() {
            explanation.push_str(&format!("  {}. {:?} (confidence delta: {:+.2})\n",
                i + 1, step.step_type, step.confidence_delta));
            explanation.push_str(&format!("     {}\n", step.evidence));
            explanation.push_str(&format!("     Timestamp: {}\n", format_timestamp(step.timestamp)));
        }
        explanation.push_str("\n");

        explanation.push_str(&format!("Merkle Root (audit trail): {}\n", cert.merkle_root));

        let verified = self.verify_certificate(&cert)?;
        explanation.push_str(&format!("Verification Status: {}\n", if verified { "VERIFIED" } else { "INVALID" }));

        Ok(explanation)
    }

    /// Get recent reasoning chains
    pub fn get_recent_reasoning(&self, limit: usize) -> Result<Vec<ReasoningChainRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pattern_id, task_context, steps, final_decision, confidence, created_at
             FROM reasoning_chains
             ORDER BY created_at DESC
             LIMIT ?1"
        )?;

        let chains = stmt.query_map(params![limit as i64], |row| {
            let steps_json: String = row.get(3)?;
            let steps: Vec<ReasoningStep> = serde_json::from_str(&steps_json)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

            Ok(ReasoningChainRecord {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                task_context: row.get(2)?,
                steps,
                final_decision: row.get(4)?,
                confidence: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })?;

        chains.collect::<Result<Vec<_>, _>>().map_err(|e| e.into())
    }
}

/// Calculate SHA256 hash of a string
fn sha256_hash(data: &str) -> String {
    use sha2::{Sha256, Digest};

    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Get current Unix timestamp
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Calculate success rate from counts
fn calculate_success_rate(success: i64, failure: i64) -> f64 {
    let total = success + failure;
    if total == 0 {
        0.5 // Neutral prior
    } else {
        success as f64 / total as f64
    }
}

/// Calculate usage score (logarithmic scale)
fn calculate_usage_score(uses: i64) -> f64 {
    if uses == 0 {
        0.0
    } else {
        (1.0 + uses as f64).ln() / 5.0 // Normalize to 0-1 range
    }
}

/// Calculate recency score based on last used timestamp
fn calculate_recency_score(last_used: &Option<String>) -> f64 {
    if last_used.is_none() {
        return 0.3; // Low score for never used
    }

    // For simplicity, return a moderate score
    // In production, would parse timestamp and calculate actual recency
    0.7
}

/// Format timestamp for human reading
fn format_timestamp(ts: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(ts as u64);
    format!("{:?}", datetime) // Simple format for now
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_merkle_root_calculation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE patterns (
                id INTEGER PRIMARY KEY,
                pattern_hash TEXT,
                success_count INTEGER,
                failure_count INTEGER,
                last_used TEXT
            )"
        ).unwrap();

        conn.execute(
            "INSERT INTO patterns (id, pattern_hash, success_count, failure_count) VALUES (1, 'test', 10, 2)",
            []
        ).unwrap();

        let store = ProvenanceStore::new(conn).unwrap();
        store.init_tables().unwrap();

        let steps = vec![
            DerivationStep {
                step_type: DerivationType::Extracted,
                timestamp: 1000,
                evidence: "Initial".to_string(),
                confidence_delta: 0.5,
            },
        ];

        let root1 = store.calculate_merkle_root(1, &steps).unwrap();
        let root2 = store.calculate_merkle_root(1, &steps).unwrap();

        assert_eq!(root1, root2, "Merkle root should be deterministic");
    }

    #[test]
    fn test_provenance_certificate() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE patterns (
                id INTEGER PRIMARY KEY,
                pattern_hash TEXT,
                tool_type TEXT,
                context_query TEXT,
                success_count INTEGER,
                failure_count INTEGER,
                last_used TEXT
            )"
        ).unwrap();

        conn.execute(
            "INSERT INTO patterns VALUES (1, 'test', 'Bash', 'test query', 10, 2, NULL)",
            []
        ).unwrap();

        let store = ProvenanceStore::new(conn).unwrap();
        store.init_tables().unwrap();

        let cert = store.generate_certificate(1).unwrap();
        assert_eq!(cert.pattern_id, 1);
        assert!(!cert.merkle_root.is_empty());

        let verified = store.verify_certificate(&cert).unwrap();
        assert!(verified, "Certificate should verify correctly");
    }
}
