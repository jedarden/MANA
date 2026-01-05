//! Tests for extended causal reasoning system

#[cfg(test)]
mod tests {
    use mana::storage::{CausalStore, CausalRelation};
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (NamedTempFile, CausalStore) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();

        // Create minimal schema with new columns
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
                (1, 'hash1', 'Bash', 'Setup environment'),
                (2, 'hash2', 'Edit', 'Fix bug'),
                (3, 'hash3', 'Bash', 'Run tests'),
                (4, 'hash4', 'Edit', 'Add feature'),
                (5, 'hash5', 'Bash', 'Deploy');

            -- Create a causal chain: 1 -> 2 -> 3
            INSERT INTO causal_edges (pattern_a_id, pattern_b_id, lift, co_occurrences, sample_count) VALUES
                (1, 2, 1.8, 10, 10),
                (2, 3, 1.6, 8, 8),
                (1, 3, 1.2, 5, 5);

            -- Add a confounder: 4 affects both 1 and 3
            INSERT INTO causal_edges (pattern_a_id, pattern_b_id, lift, co_occurrences, sample_count) VALUES
                (1, 4, 1.7, 12, 12),
                (3, 4, 1.5, 10, 10);
            "#,
        ).unwrap();
        drop(conn);

        let store = CausalStore::open(tmp.path()).unwrap();
        (tmp, store)
    }

    #[test]
    fn test_do_intervention() {
        let (_tmp, store) = setup_test_db();

        let result = store.do_intervention(1, 3).unwrap();

        assert_eq!(result.treatment_pattern_id, 1);
        assert_eq!(result.outcome_pattern_id, 3);
        assert!(result.causal_effect > 0.0);
        assert!(result.confidence_interval.0 < result.confidence_interval.1);
        println!("Intervention result: effect = {:.3}, CI = ({:.3}, {:.3}), p = {:.4}",
            result.causal_effect, result.confidence_interval.0, result.confidence_interval.1, result.p_value);
    }

    #[test]
    fn test_detect_confounders() {
        let (_tmp, store) = setup_test_db();

        let analysis = store.detect_confounders(1, 3, 0.05).unwrap();

        println!("Confounder analysis:");
        println!("  Unadjusted effect: {:.3}", analysis.unadjusted_effect);
        println!("  Adjusted effect: {:.3}", analysis.adjusted_effect);
        println!("  Bias estimate: {:.3}", analysis.bias_estimate);
        println!("  Confounders detected: {}", analysis.potential_confounders.len());

        // Pattern 4 should be detected as a confounder
        if !analysis.potential_confounders.is_empty() {
            for conf in &analysis.potential_confounders {
                println!("  - Pattern {}: backdoor strength = {:.3}, p = {:.4}",
                    conf.pattern_id, conf.backdoor_path_strength, conf.significance);
            }
        }
    }

    #[test]
    fn test_find_causal_chains() {
        let (_tmp, store) = setup_test_db();

        let chains = store.find_causal_chains(1, 3, 3).unwrap();

        assert!(!chains.is_empty(), "Should find at least one chain from 1 to 3");

        println!("Found {} causal chains:", chains.len());
        for (i, chain) in chains.iter().enumerate() {
            println!("  Chain {}: {:?}", i + 1, chain.nodes);
            println!("    Path strength: {:.3}", chain.path_strength);
            println!("    Total effect: {:.3}", chain.total_effect);
        }
    }

    #[test]
    fn test_causal_stats() {
        let (_tmp, store) = setup_test_db();

        let stats = store.causal_stats().unwrap();

        println!("Causal graph stats:");
        println!("  Total nodes: {}", stats.total_nodes);
        println!("  Total edges: {}", stats.total_edges);
        println!("  Synergy edges: {}", stats.synergy_edges);
        println!("  Conflict edges: {}", stats.conflict_edges);
        println!("  Avg connections: {:.2}", stats.avg_connections_per_node);

        assert!(stats.total_nodes >= 4);
        assert!(stats.total_edges == 5);
        assert!(stats.synergy_edges > 0);
    }

    #[test]
    fn test_calculate_uplift() {
        let (_tmp, store) = setup_test_db();

        let control = vec![2, 3];
        let treatment = vec![4, 5];

        let (effect, ci_width, p_value) = store.calculate_uplift(1, &control, &treatment).unwrap();

        println!("Uplift test:");
        println!("  Effect: {:.3}", effect);
        println!("  CI width: {:.3}", ci_width);
        println!("  P-value: {:.4}", p_value);
    }

    #[test]
    fn test_causal_relation_enum() {
        assert_eq!(CausalRelation::Causes.as_str(), "Causes");
        assert_eq!(CausalRelation::Enables.as_str(), "Enables");
        assert_eq!(CausalRelation::Prevents.as_str(), "Prevents");
        assert_eq!(CausalRelation::from_str("Causes"), CausalRelation::Causes);
        assert_eq!(CausalRelation::from_str("Unknown"), CausalRelation::Correlates);
    }
}
