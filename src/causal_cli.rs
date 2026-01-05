//! CLI handlers for causal reasoning commands

use anyhow::Result;
use std::path::PathBuf;
use crate::storage::{CausalStore, get_mana_dir};

pub async fn handle_intervention(treatment_id: i64, outcome_id: i64) -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    let store = CausalStore::open(&db_path)?;
    let result = store.do_intervention(treatment_id, outcome_id)?;

    println!("Causal Intervention Analysis");
    println!("============================\n");
    println!("Treatment: Pattern #{}", result.treatment_pattern_id);
    println!("Outcome:   Pattern #{}", result.outcome_pattern_id);
    println!();
    println!("Causal Effect:       {:.3}", result.causal_effect);
    println!("95% CI:              ({:.3}, {:.3})",
        result.confidence_interval.0, result.confidence_interval.1);
    println!("P-value:             {:.4}", result.p_value);
    println!("Sample Size:         {}", result.sample_size);
    println!();

    if !result.confounders_detected.is_empty() {
        println!("Confounders Detected ({}):", result.confounders_detected.len());
        for confounder_id in &result.confounders_detected {
            println!("  - Pattern #{}", confounder_id);
        }
        println!();
    } else {
        println!("No significant confounders detected.\n");
    }

    if result.p_value < 0.05 {
        println!("✓ Statistically significant effect (p < 0.05)");
    } else {
        println!("✗ Effect not statistically significant (p >= 0.05)");
    }

    Ok(())
}

pub async fn handle_chains(from_id: i64, to_id: i64, max_hops: usize) -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    let store = CausalStore::open(&db_path)?;
    let chains = store.find_causal_chains(from_id, to_id, max_hops)?;

    println!("Causal Chains");
    println!("=============\n");
    println!("From: Pattern #{}", from_id);
    println!("To:   Pattern #{}", to_id);
    println!("Max Hops: {}\n", max_hops);

    if chains.is_empty() {
        println!("No causal chains found within {} hops.", max_hops);
        return Ok(());
    }

    println!("Found {} causal chain(s):\n", chains.len());

    for (i, chain) in chains.iter().enumerate() {
        println!("Chain #{} (strength: {:.3}):", i + 1, chain.path_strength);
        print!("  ");
        for (j, node_id) in chain.nodes.iter().enumerate() {
            print!("#{}", node_id);
            if j < chain.nodes.len() - 1 {
                print!(" -> ");
            }
        }
        println!();
        println!("  Total Effect: {:.3}", chain.total_effect);
        println!("  Edges: {}", chain.edges.len());

        for (j, edge) in chain.edges.iter().enumerate() {
            println!("    {}. #{} <-> #{} (lift: {:.2}, relation: {:?})",
                j + 1, edge.pattern_a_id, edge.pattern_b_id, edge.lift, edge.relation_type);
        }
        println!();
    }

    Ok(())
}

pub async fn handle_confounders(treatment_id: i64, outcome_id: i64, min_significance: f64) -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    let store = CausalStore::open(&db_path)?;
    let analysis = store.detect_confounders(treatment_id, outcome_id, min_significance)?;

    println!("Confounder Detection");
    println!("===================\n");
    println!("Treatment: Pattern #{}", treatment_id);
    println!("Outcome:   Pattern #{}", outcome_id);
    println!("Significance Threshold: {}\n", min_significance);

    println!("Unadjusted Effect: {:.3}", analysis.unadjusted_effect);
    println!("Adjusted Effect:   {:.3}", analysis.adjusted_effect);
    println!("Bias Estimate:     {:.3}\n", analysis.bias_estimate);

    if analysis.potential_confounders.is_empty() {
        println!("No significant confounders detected.");
    } else {
        println!("Potential Confounders ({}):\n", analysis.potential_confounders.len());

        for (i, confounder) in analysis.potential_confounders.iter().enumerate() {
            println!("{}. Pattern #{}", i + 1, confounder.pattern_id);
            println!("   Correlation with treatment: {:.3}", confounder.correlation_with_treatment);
            println!("   Correlation with outcome:   {:.3}", confounder.correlation_with_outcome);
            println!("   Backdoor path strength:     {:.3}", confounder.backdoor_path_strength);
            println!("   Significance (p-value):     {:.4}", confounder.significance);
            println!();
        }
    }

    Ok(())
}

pub async fn handle_stats() -> Result<()> {
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    let store = CausalStore::open(&db_path)?;
    let stats = store.causal_stats()?;

    println!("Causal Graph Statistics");
    println!("======================\n");

    println!("Graph Structure:");
    println!("  Nodes:  {} patterns", stats.total_nodes);
    println!("  Edges:  {} relationships", stats.total_edges);
    println!("  Avg connections per node: {:.1}", stats.avg_connections_per_node);
    println!("  Max chain length: {}\n", stats.max_chain_length);

    println!("Edge Types:");
    println!("  Synergies (lift > 1.5): {}", stats.synergy_edges);
    println!("  Conflicts (lift < 0.5): {}", stats.conflict_edges);
    println!("  Neutral: {}\n", stats.total_edges - stats.synergy_edges - stats.conflict_edges);

    if !stats.relation_type_counts.is_empty() {
        println!("Relation Types:");
        let mut sorted_types: Vec<_> = stats.relation_type_counts.iter().collect();
        sorted_types.sort_by(|a, b| b.1.cmp(a.1));

        for (rel_type, count) in sorted_types {
            println!("  {}: {}", rel_type, count);
        }
    }

    Ok(())
}
