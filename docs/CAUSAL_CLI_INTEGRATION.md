# Causal System CLI Integration Guide

This document shows how to integrate the causal reasoning CLI commands into main.rs.

## Step 1: Add module import

Add after line 10 (after `mod bench;`):

```rust
mod causal_cli;
```

## Step 2: Add CausalAction enum

Add after the `AnalyzeAction` enum (around line 180):

```rust
#[derive(Subcommand)]
enum CausalAction {
    /// Query causal effect of an intervention (do-calculus)
    Intervention {
        /// Treatment pattern ID
        treatment_id: i64,
        /// Outcome pattern ID
        outcome_id: i64,
    },

    /// Find causal chains between two patterns
    Chains {
        /// Source pattern ID
        from_id: i64,
        /// Target pattern ID
        to_id: i64,
        /// Maximum number of hops
        #[arg(long, default_value = "3")]
        max_hops: usize,
    },

    /// Detect confounders between treatment and outcome
    Confounders {
        /// Treatment pattern ID
        treatment_id: i64,
        /// Outcome pattern ID
        outcome_id: i64,
        /// Minimum significance level (default 0.05)
        #[arg(long, default_value = "0.05")]
        min_significance: f64,
    },

    /// Show causal graph statistics
    Stats,
}
```

## Step 3: Add Causal command to Commands enum

Add after the `Analyze` command (around line 167):

```rust
    /// Causal reasoning and analysis
    Causal {
        #[command(subcommand)]
        action: CausalAction,
    },
```

## Step 4: Add handler in run_async_main

Add before the closing brace of the match statement (around line 1690):

```rust
        Commands::Causal { action } => {
            match action {
                CausalAction::Intervention { treatment_id, outcome_id } => {
                    causal_cli::handle_intervention(*treatment_id, *outcome_id).await?;
                }
                CausalAction::Chains { from_id, to_id, max_hops } => {
                    causal_cli::handle_chains(*from_id, *to_id, *max_hops).await?;
                }
                CausalAction::Confounders { treatment_id, outcome_id, min_significance } => {
                    causal_cli::handle_confounders(*treatment_id, *outcome_id, *min_significance).await?;
                }
                CausalAction::Stats => {
                    causal_cli::handle_stats().await?;
                }
            }
        }
```

## Usage Examples

Once integrated, you can use the following commands:

### Query causal intervention effect
```bash
mana causal intervention 12 45
```

### Find causal chains
```bash
mana causal chains 5 20 --max-hops 5
```

### Detect confounders
```bash
mana causal confounders 10 30 --min-significance 0.01
```

### Show causal graph statistics
```bash
mana causal stats
```

## Testing

Run the test suite:
```bash
cargo test --test causal_test
```

Expected output shows:
- Do-calculus intervention queries with confidence intervals
- Confounder detection with backdoor path analysis
- Causal chain discovery with multi-hop reasoning
- Graph statistics with synergy/conflict counts
