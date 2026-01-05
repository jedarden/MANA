# MANA Causal Reasoning - Quick Reference

## Commands

```bash
# Show causal graph statistics
mana causal stats

# Query intervention effect (do-calculus)
mana causal intervention <treatment_id> <outcome_id>

# Find causal chains between patterns
mana causal chains <from_id> <to_id> [--max-hops N]

# Detect confounding variables
mana causal confounders <treatment_id> <outcome_id> [--min-significance F]
```

## Key Concepts

| Concept | Definition | Example |
|---------|------------|---------|
| **Lift** | Strength of relationship | lift=1.8 means 80% stronger together |
| **Synergy** | Patterns work well together | lift > 1.5 |
| **Conflict** | Patterns interfere | lift < 0.5 |
| **Confounder** | Variable affecting both treatment & outcome | Z→X, Z→Y creates backdoor path |
| **Causal Chain** | Multi-hop causal path | A→B→C |
| **Do-calculus** | Intervention query | do(X=x): force X to value x |
| **P-value** | Statistical significance | p<0.05 = significant |
| **Confidence Interval** | Uncertainty range | 95% CI: [lower, upper] |

## Lift Interpretation

| Lift | Meaning |
|------|---------|
| > 2.0 | Very strong synergy |
| 1.5 - 2.0 | Strong synergy |
| 1.1 - 1.5 | Moderate synergy |
| 0.9 - 1.1 | No relationship |
| 0.5 - 0.9 | Moderate conflict |
| < 0.5 | Strong conflict |

## Statistical Thresholds

| Metric | Threshold | Interpretation |
|--------|-----------|----------------|
| P-value | < 0.001 | Highly significant |
| P-value | < 0.01 | Very significant |
| P-value | < 0.05 | Significant |
| P-value | ≥ 0.05 | Not significant |
| Sample size | < 3 | Insufficient data |
| Sample size | 3-30 | Use t-distribution |
| Sample size | > 30 | Use normal approximation |

## Relation Types

| Type | Symbol | Meaning |
|------|--------|---------|
| Causes | A→B | A directly causes B |
| Enables | A⇢B | A makes B possible |
| Prevents | A⊣B | A blocks B |
| Correlates | A⟷B | A and B co-occur |
| Precedes | A⋯→B | A happens before B |
| DerivedFrom | A⟸B | A inferred from B |
| Contradicts | A⊥B | A and B incompatible |

## Common Workflows

### Find Root Causes
```bash
# 1. Identify outcome pattern
mana patterns list --sort score --limit 10

# 2. Find chains leading to it
mana causal chains <start_id> <outcome_id> --max-hops 5

# 3. Test strongest intervention
mana causal intervention <critical_id> <outcome_id>
```

### Validate Causal Claims
```bash
# 1. Query intervention effect
mana causal intervention <treatment_id> <outcome_id>

# 2. Check for confounders
mana causal confounders <treatment_id> <outcome_id>

# 3. Verify significance
# Look for: p < 0.05 and CI excluding 1.0
```

### Optimize Pipeline
```bash
# 1. Get graph overview
mana causal stats

# 2. Find bottlenecks (low lift)
mana patterns list --min-score -5

# 3. Find alternatives (high lift)
mana patterns search "similar task"

# 4. Test alternatives
mana causal intervention <alternative_id> <outcome_id>
```

## Output Interpretation

### Intervention Result
```
Causal Effect:       1.524   ← Main finding
95% CI:              (1.234, 1.814)   ← Uncertainty
P-value:             0.0023   ← Significance
Confounders:         2   ← Adjustment needed
```
**Read as:** "Treatment increases outcome by 52.4% (CI: 23-81%), p=0.002, accounting for 2 confounders."

### Confounder Analysis
```
Unadjusted Effect:   1.650   ← Naive estimate
Adjusted Effect:     1.524   ← True estimate
Bias Estimate:       0.076   ← Confounding bias
```
**Read as:** "Confounders inflate effect by 7.6%. Adjusted effect is 52.4%."

### Causal Chain
```
#5 -> #12 -> #20
Path strength: 3.456   ← Overall strength
Total effect: 1.728   ← Average lift
```
**Read as:** "Via this path, treatment has 72.8% positive effect with strength 3.5."

## Mathematical Formulas

**Lift:**
```
lift = P(success | A ∧ B) / P(success | ¬A ∧ ¬B)
```

**Correlation (from lift):**
```
corr = (lift - 1.0) ∈ [-1, 1]
```

**Backdoor Path Strength:**
```
strength = |corr(C,T)| × |corr(C,O)|
```

**Adjusted Effect:**
```
adjusted = unadjusted / (1 + 0.2 × Σ backdoor_strengths)
```

**Confidence Interval:**
```
CI = effect ± t_critical × SE
SE = 0.5 / √n
```

**P-value:**
```
t = (effect - 1.0) / SE
p = 2 × (1 - Φ(|t|))
```

## Files

- `src/storage/causal.rs` - Core implementation
- `src/causal_cli.rs` - CLI handlers
- `tests/causal_test.rs` - Tests
- `CAUSAL_SYSTEM_SUMMARY.md` - Detailed docs
- `CAUSAL_CLI_INTEGRATION.md` - Integration guide
- `examples/causal_analysis.md` - Complete example

## API Examples

```rust
use mana::storage::CausalStore;

// Open store
let store = CausalStore::open("metadata.sqlite")?;

// Query intervention
let result = store.do_intervention(12, 45)?;
println!("Effect: {:.2} ± {:.2}",
    result.causal_effect,
    result.confidence_interval.1 - result.causal_effect);

// Find chains
let chains = store.find_causal_chains(5, 20, 3)?;
for chain in chains {
    println!("Path: {:?}", chain.nodes);
}

// Detect confounders
let analysis = store.detect_confounders(10, 30, 0.05)?;
println!("Confounders: {}", analysis.potential_confounders.len());

// Get stats
let stats = store.causal_stats()?;
println!("Graph has {} nodes, {} edges", stats.total_nodes, stats.total_edges);
```

## Tips

✓ **Do:**
- Use intervention queries for causal claims
- Check for confounders before drawing conclusions
- Look at confidence intervals, not just point estimates
- Require p < 0.05 for significance
- Validate with domain knowledge

✗ **Don't:**
- Confuse correlation with causation (use intervention)
- Ignore confounders (use confounder detection)
- Over-interpret small samples (n < 10)
- Trust single chains (find multiple paths)
- Forget to check statistical significance

## Troubleshooting

| Problem | Solution |
|---------|----------|
| No data | Run `mana consolidate` to learn patterns |
| Sample size 0 | Patterns haven't co-occurred yet |
| Wide CIs | Need more data (low sample size) |
| High p-values | Effect not significant or need more data |
| No chains | Increase --max-hops or check connectivity |
| All confounders | Normal - use adjusted effect |

---

**Quick Start:** `mana causal stats` → `mana causal intervention X Y` → `mana causal confounders X Y`
