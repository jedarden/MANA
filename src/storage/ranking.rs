//! Multi-factor ranking system
//!
//! Combines multiple signals for pattern relevance:
//! - Similarity: Semantic/text similarity to query
//! - Recency: Recently accessed patterns ranked higher (exponential decay)
//! - Popularity: Frequently used patterns score higher
//! - Confidence: Success rate and absolute score

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::Pattern;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingConfig {
    pub similarity_weight: f64,    // Default: 0.50
    pub recency_weight: f64,       // Default: 0.20
    pub popularity_weight: f64,    // Default: 0.15
    pub confidence_weight: f64,    // Default: 0.15
    pub recency_decay_days: f64,   // Default: 7.0 (half-life)
}

impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            similarity_weight: 0.50,
            recency_weight: 0.20,
            popularity_weight: 0.15,
            confidence_weight: 0.15,
            recency_decay_days: 7.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankingFactors {
    pub similarity: f64,      // 0.0 - 1.0, from text/embedding similarity
    pub recency_score: f64,   // 0.0 - 1.0, exponential decay from last_used
    pub popularity_score: f64, // 0.0 - 1.0, normalized access count
    pub confidence_score: f64, // 0.0 - 1.0, success rate + score normalization
}

#[derive(Debug, Clone)]
pub struct RankedPattern {
    pub pattern: Pattern,
    pub factors: RankingFactors,
    pub final_score: f64,
}

pub struct PatternRanker {
    config: RankingConfig,
}

impl PatternRanker {
    pub fn new(config: RankingConfig) -> Self {
        Self { config }
    }

    pub fn new_default() -> Self {
        Self::new(RankingConfig::default())
    }

    /// Calculate recency score with exponential decay
    ///
    /// Uses half-life decay: score = 0.5^(days_since_use / half_life)
    /// - Patterns used today get score of 1.0
    /// - Patterns used `half_life` days ago get score of 0.5
    /// - Patterns never used get score of 0.0
    pub fn recency_score(&self, last_used: Option<DateTime<Utc>>) -> f64 {
        match last_used {
            None => 0.0, // Never used
            Some(timestamp) => {
                let now = Utc::now();
                let duration = now.signed_duration_since(timestamp);
                let days_old = duration.num_days() as f64;

                if days_old < 0.0 {
                    // Future timestamp (shouldn't happen, but handle gracefully)
                    1.0
                } else {
                    // Exponential decay with half-life
                    // score = 0.5^(days_old / half_life)
                    let exponent = days_old / self.config.recency_decay_days;
                    0.5_f64.powf(exponent)
                }
            }
        }
    }

    /// Calculate popularity from access patterns
    ///
    /// Normalizes access count to 0.0-1.0 range using log scaling
    /// This prevents patterns with extremely high access counts from dominating
    pub fn popularity_score(&self, access_count: i64, max_access: i64) -> f64 {
        if max_access <= 0 || access_count <= 0 {
            return 0.0;
        }

        // Use log scale to prevent very popular patterns from dominating
        // log(1 + x) ensures we get a smooth curve and handle 0 gracefully
        let normalized = (1.0 + access_count as f64).ln() / (1.0 + max_access as f64).ln();
        normalized.min(1.0).max(0.0)
    }

    /// Calculate confidence from success/failure ratio
    ///
    /// Combines:
    /// - Success rate (what % of uses were successful)
    /// - Absolute score (net success - failure)
    ///
    /// This ensures that patterns with many uses and high success rate are preferred
    pub fn confidence_score(&self, success: i64, failure: i64) -> f64 {
        let total = success + failure;
        if total == 0 {
            return 0.0;
        }

        // Success rate: 0.0 - 1.0
        let success_rate = success as f64 / total as f64;

        // Absolute score with log scaling to prevent dominance
        let net_score = (success - failure).max(0) as f64;
        let score_component = (1.0 + net_score).ln() / 10.0; // Scale down

        // Combine: 70% success rate, 30% absolute score
        let confidence = success_rate * 0.7 + score_component.min(1.0) * 0.3;
        confidence.min(1.0).max(0.0)
    }

    /// Rank a set of patterns with all factors
    ///
    /// Takes patterns with their similarity scores and combines with other factors
    /// Returns ranked patterns sorted by final score (descending)
    pub fn rank(
        &self,
        patterns: Vec<(Pattern, f64)>,
        conn: &Connection,
    ) -> Result<Vec<RankedPattern>> {
        // Get max access count for normalization
        let max_access: i64 = conn.query_row(
            "SELECT COALESCE(MAX(access_count), 0) FROM patterns",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        let mut ranked: Vec<RankedPattern> = patterns
            .into_iter()
            .map(|(pattern, similarity)| {
                // Parse last_used timestamp if it exists
                let last_used = pattern.last_used.as_ref().and_then(|s| {
                    DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                });

                let factors = RankingFactors {
                    similarity,
                    recency_score: self.recency_score(last_used),
                    popularity_score: self.popularity_score(pattern.access_count, max_access),
                    confidence_score: self.confidence_score(pattern.success_count, pattern.failure_count),
                };

                // Calculate weighted final score
                let final_score =
                    factors.similarity * self.config.similarity_weight +
                    factors.recency_score * self.config.recency_weight +
                    factors.popularity_score * self.config.popularity_weight +
                    factors.confidence_score * self.config.confidence_weight;

                RankedPattern {
                    pattern,
                    factors,
                    final_score,
                }
            })
            .collect();

        // Sort by final score (descending)
        ranked.sort_by(|a, b| {
            b.final_score.partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(ranked)
    }

    /// Get top-k patterns with multi-factor ranking
    ///
    /// This is a convenience method that:
    /// 1. Retrieves candidates from the database
    /// 2. Calculates similarity scores
    /// 3. Ranks using all factors
    /// 4. Returns top-k results
    pub fn get_top_ranked(
        &self,
        conn: &Connection,
        tool_type: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<RankedPattern>> {
        use super::calculate_similarity;

        // Retrieve candidate patterns (more than k to allow for better ranking)
        let candidates_count = (k * 3).max(20);

        let mut stmt = conn.prepare_cached(
            r#"
            SELECT id, pattern_hash, tool_type, command_category, context_query,
                   success_count, failure_count, embedding_id, last_used, access_count, tier_path
            FROM patterns
            WHERE tool_type = ?1
            ORDER BY (success_count - failure_count) DESC, success_count DESC
            LIMIT ?2
            "#,
        )?;

        let patterns = stmt.query_map(params![tool_type, candidates_count as i64], |row| {
            Ok(Pattern {
                id: row.get(0)?,
                pattern_hash: row.get(1)?,
                tool_type: row.get(2)?,
                command_category: row.get(3)?,
                context_query: row.get(4)?,
                success_count: row.get(5)?,
                failure_count: row.get(6)?,
                embedding_id: row.get(7)?,
                last_used: row.get(8).ok(),
                access_count: row.get(9).unwrap_or(0),
                tier_path: row.get(10).unwrap_or_else(|_| "global".to_string()),
                ..Default::default()
            })
        })?;

        // Calculate similarity and prepare for ranking
        let patterns_with_similarity: Vec<(Pattern, f64)> = patterns
            .filter_map(|r| r.ok())
            .map(|p| {
                let similarity = if query.is_empty() {
                    0.5 // Default similarity when no query
                } else {
                    calculate_similarity(query, &p.context_query)
                };
                (p, similarity)
            })
            .collect();

        // Rank and return top-k
        let mut ranked = self.rank(patterns_with_similarity, conn)?;
        ranked.truncate(k);
        Ok(ranked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recency_score() {
        let ranker = PatternRanker::new_default();

        // Never used
        assert_eq!(ranker.recency_score(None), 0.0);

        // Used right now
        let now = Utc::now();
        let score_now = ranker.recency_score(Some(now));
        assert!(score_now > 0.99 && score_now <= 1.0);

        // Used 7 days ago (half-life)
        let seven_days_ago = now - chrono::Duration::days(7);
        let score_half = ranker.recency_score(Some(seven_days_ago));
        assert!(score_half > 0.49 && score_half < 0.51);

        // Used 14 days ago (two half-lives)
        let fourteen_days_ago = now - chrono::Duration::days(14);
        let score_quarter = ranker.recency_score(Some(fourteen_days_ago));
        assert!(score_quarter > 0.24 && score_quarter < 0.26);
    }

    #[test]
    fn test_popularity_score() {
        let ranker = PatternRanker::new_default();

        // Zero access
        assert_eq!(ranker.popularity_score(0, 100), 0.0);

        // Max access
        let score_max = ranker.popularity_score(100, 100);
        assert_eq!(score_max, 1.0);

        // Mid-range access
        let score_mid = ranker.popularity_score(50, 100);
        assert!(score_mid > 0.0 && score_mid < 1.0);
    }

    #[test]
    fn test_confidence_score() {
        let ranker = PatternRanker::new_default();

        // No data
        assert_eq!(ranker.confidence_score(0, 0), 0.0);

        // Perfect success
        let score_perfect = ranker.confidence_score(10, 0);
        assert!(score_perfect > 0.9);

        // 50% success
        let score_half = ranker.confidence_score(5, 5);
        assert!(score_half > 0.3 && score_half < 0.6);

        // Mostly failures
        let score_low = ranker.confidence_score(1, 9);
        assert!(score_low < 0.3);
    }

    #[test]
    fn test_default_config_weights_sum_to_one() {
        let config = RankingConfig::default();
        let sum = config.similarity_weight + config.recency_weight +
                  config.popularity_weight + config.confidence_weight;
        assert!((sum - 1.0).abs() < 0.001, "Weights should sum to 1.0");
    }
}
