//! Verdict types and judgment logic for reflection
//!
//! Verdicts represent the effectiveness assessment of patterns
//! based on trajectory outcomes.

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Verdict category for pattern effectiveness
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerdictCategory {
    /// Pattern directly contributed to success
    Effective,
    /// Pattern neither helped nor hurt
    Neutral,
    /// Pattern didn't help, minor negative signal
    Ineffective,
    /// Pattern caused errors or wasted effort
    Harmful,
}

impl VerdictCategory {
    /// Get the string representation for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            VerdictCategory::Effective => "EFFECTIVE",
            VerdictCategory::Neutral => "NEUTRAL",
            VerdictCategory::Ineffective => "INEFFECTIVE",
            VerdictCategory::Harmful => "HARMFUL",
        }
    }

    /// Parse from database string
    #[allow(dead_code)] // Reserved for future verdict deserialization
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "EFFECTIVE" => Some(VerdictCategory::Effective),
            "NEUTRAL" => Some(VerdictCategory::Neutral),
            "INEFFECTIVE" => Some(VerdictCategory::Ineffective),
            "HARMFUL" => Some(VerdictCategory::Harmful),
            _ => None,
        }
    }

    /// Get the score impact for this verdict category
    #[allow(dead_code)] // Reserved for future direct category-based scoring
    pub fn score_impact(&self, confidence: f32, max_boost: i32, max_penalty: i32) -> i32 {
        let base = match self {
            VerdictCategory::Effective => max_boost,
            VerdictCategory::Neutral => 0,
            VerdictCategory::Ineffective => -1,
            VerdictCategory::Harmful => max_penalty,
        };

        // Scale by confidence
        (base as f32 * confidence).round() as i32
    }
}

/// A verdict on pattern effectiveness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    /// The verdict category
    pub category: VerdictCategory,
    /// Confidence in this verdict (0.0 - 1.0)
    pub confidence: f32,
    /// Score impact to apply
    pub score_impact: i32,
    /// Root cause analysis (for failures)
    pub root_cause: Option<String>,
    /// Suggested improvement
    pub suggested_improvement: Option<String>,
}

impl Verdict {
    /// Create a new effective verdict
    pub fn effective(confidence: f32, boost: i32) -> Self {
        Self {
            category: VerdictCategory::Effective,
            confidence,
            score_impact: (boost as f32 * confidence).round() as i32,
            root_cause: None,
            suggested_improvement: None,
        }
    }

    /// Create a new neutral verdict
    pub fn neutral() -> Self {
        Self {
            category: VerdictCategory::Neutral,
            confidence: 1.0,
            score_impact: 0,
            root_cause: None,
            suggested_improvement: None,
        }
    }

    /// Create a new ineffective verdict
    pub fn ineffective(confidence: f32) -> Self {
        Self {
            category: VerdictCategory::Ineffective,
            confidence,
            score_impact: -1,
            root_cause: None,
            suggested_improvement: None,
        }
    }

    /// Create a new harmful verdict
    pub fn harmful(confidence: f32, penalty: i32, root_cause: String) -> Self {
        Self {
            category: VerdictCategory::Harmful,
            confidence,
            score_impact: (penalty as f32 * confidence).round() as i32,
            root_cause: Some(root_cause),
            suggested_improvement: None,
        }
    }

    /// Add a suggested improvement to the verdict
    #[allow(dead_code)] // Reserved for future improvement suggestions
    pub fn with_suggestion(mut self, suggestion: String) -> Self {
        self.suggested_improvement = Some(suggestion);
        self
    }
}

/// A complete reflection verdict linking trajectory to pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionVerdict {
    /// Hash of the trajectory for deduplication
    pub trajectory_hash: String,
    /// ID of the related pattern (if any)
    pub pattern_id: Option<i64>,
    /// The verdict
    pub verdict: Verdict,
    /// Confidence in the verdict (convenience accessor)
    pub confidence: f32,
    /// Was this a context mismatch?
    pub context_mismatch: bool,
}

impl ReflectionVerdict {
    /// Create a new reflection verdict
    pub fn new(trajectory_hash: String, pattern_id: Option<i64>, verdict: Verdict) -> Self {
        let confidence = verdict.confidence;
        Self {
            trajectory_hash,
            pattern_id,
            verdict,
            confidence,
            context_mismatch: false,
        }
    }

    /// Mark this verdict as a context mismatch
    #[allow(dead_code)] // Reserved for future context mismatch tracking
    pub fn with_context_mismatch(mut self) -> Self {
        self.context_mismatch = true;
        self
    }

    /// Get the verdict category string
    pub fn category_str(&self) -> &'static str {
        self.verdict.category.as_str()
    }

    /// Get the score impact
    pub fn score_impact(&self) -> i32 {
        self.verdict.score_impact
    }
}

/// Compute a hash for a trajectory for deduplication
pub fn compute_trajectory_hash(
    session_id: &str,
    user_query: &str,
    tool_calls: &[crate::learning::trajectory::ToolCall],
) -> String {
    let mut hasher = DefaultHasher::new();

    session_id.hash(&mut hasher);
    user_query.hash(&mut hasher);

    for call in tool_calls {
        call.tool_name.hash(&mut hasher);
        call.tool_input.to_string().hash(&mut hasher);
    }

    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verdict_category_roundtrip() {
        for category in [
            VerdictCategory::Effective,
            VerdictCategory::Neutral,
            VerdictCategory::Ineffective,
            VerdictCategory::Harmful,
        ] {
            let s = category.as_str();
            let parsed = VerdictCategory::from_str(s).unwrap();
            assert_eq!(category, parsed);
        }
    }

    #[test]
    fn test_score_impact() {
        let effective = VerdictCategory::Effective;
        assert_eq!(effective.score_impact(1.0, 5, -5), 5);
        assert_eq!(effective.score_impact(0.5, 5, -5), 3); // Rounded

        let harmful = VerdictCategory::Harmful;
        assert_eq!(harmful.score_impact(1.0, 5, -5), -5);
        assert_eq!(harmful.score_impact(0.8, 5, -5), -4);
    }

    #[test]
    fn test_verdict_creation() {
        let v = Verdict::effective(0.9, 5);
        assert_eq!(v.category, VerdictCategory::Effective);
        assert_eq!(v.score_impact, 5); // 5 * 0.9 = 4.5, rounded to 5

        let v = Verdict::harmful(0.8, -5, "Caused timeout".into());
        assert_eq!(v.category, VerdictCategory::Harmful);
        assert_eq!(v.score_impact, -4); // -5 * 0.8 = -4
        assert_eq!(v.root_cause, Some("Caused timeout".into()));
    }

    #[test]
    fn test_trajectory_hash() {
        use crate::learning::trajectory::ToolCall;

        let hash1 = compute_trajectory_hash(
            "session1",
            "fix the bug",
            &[ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({"file": "main.rs"}),
            }],
        );

        let hash2 = compute_trajectory_hash(
            "session1",
            "fix the bug",
            &[ToolCall {
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({"file": "main.rs"}),
            }],
        );

        // Same input should produce same hash
        assert_eq!(hash1, hash2);

        // Different input should produce different hash
        let hash3 = compute_trajectory_hash(
            "session2",
            "fix the bug",
            &[],
        );
        assert_ne!(hash1, hash3);
    }
}
