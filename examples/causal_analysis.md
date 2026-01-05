# MANA Causal Analysis - Complete Example

This example demonstrates how to use MANA's extended causal reasoning system to analyze pattern relationships and discover causal structures.

## Scenario: Debugging a Deployment Pipeline

You have patterns representing steps in a deployment pipeline:
- Pattern #5: "Setup environment variables"
- Pattern #12: "Run database migrations"
- Pattern #20: "Deploy application"
- Pattern #15: "Restart load balancer"
- Pattern #30: "Verify health checks"

## Step 1: Initialize and Learn Patterns

```bash
# Initialize MANA
mana init

# Let MANA learn from your Claude Code sessions
# (This happens automatically as you work)

# Check status
mana status
```

## Step 2: Explore the Causal Graph

```bash
# See overall causal graph statistics
mana causal stats
```

Output:
```
Causal Graph Statistics
======================

Graph Structure:
  Nodes:  45 patterns
  Edges:  123 relationships
  Avg connections per node: 5.5
  Max chain length: 6

Edge Types:
  Synergies (lift > 1.5): 28
  Conflicts (lift < 0.5): 7
  Neutral: 88

Relation Types:
  Correlates: 98
  Causes: 15
  Enables: 8
  Prevents: 2
```

## Step 3: Investigate Deployment Failures

Deployments (Pattern #20) have been failing. Let's find what leads to successful deployments.

```bash
# Find causal chains from environment setup to deployment
mana causal chains 5 20 --max-hops 5
```

Output:
```
Causal Chains
=============

From: Pattern #5 (Setup environment)
To:   Pattern #20 (Deploy application)
Max Hops: 5

Found 3 causal chain(s):

Chain #1 (strength: 3.456):
  #5 -> #12 -> #20
  Total Effect: 1.728
  Edges: 2
    1. #5 <-> #12 (lift: 1.92, relation: Enables)
    2. #12 <-> #20 (lift: 1.80, relation: Causes)

Chain #2 (strength: 2.134):
  #5 -> #15 -> #20
  Total Effect: 1.461
  Edges: 2
    1. #5 <-> #15 (lift: 1.46, relation: Correlates)
    2. #15 <-> #20 (lift: 1.46, relation: Enables)

Chain #3 (strength: 1.234):
  #5 -> #20
  Total Effect: 1.234
  Edges: 1
    1. #5 <-> #20 (lift: 1.23, relation: Correlates)
```

**Interpretation:**
- Strongest path goes through database migrations (#12)
- Setup → Migrations → Deploy has highest causal strength
- This suggests migrations are critical for deployment success

## Step 4: Test Causal Intervention

What would happen if we *force* migrations to run (do-calculus)?

```bash
# Query intervention effect
mana causal intervention 12 20
```

Output:
```
Causal Intervention Analysis
============================

Treatment: Pattern #12 (Run migrations)
Outcome:   Pattern #20 (Deploy application)

Causal Effect:       1.743
95% CI:              (1.562, 1.924)
P-value:             0.0001
Sample Size:         127

Confounders Detected (1):
  - Pattern #15 (Restart load balancer)

✓ Statistically significant effect (p < 0.05)
```

**Interpretation:**
- Running migrations increases deployment success by 74.3%
- Effect is highly significant (p < 0.001)
- Load balancer restarts confound this relationship
- After adjusting for confounders, effect remains strong

## Step 5: Investigate the Confounder

Why is load balancer restart (#15) a confounder?

```bash
# Analyze confounder relationships
mana causal confounders 12 20 --min-significance 0.05
```

Output:
```
Confounder Detection
===================

Treatment: Pattern #12 (Run migrations)
Outcome:   Pattern #20 (Deploy application)
Significance Threshold: 0.05

Unadjusted Effect: 1.798
Adjusted Effect:   1.743
Bias Estimate:     0.031

Potential Confounders (1):

1. Pattern #15 (Restart load balancer)
   Correlation with treatment: 0.687
   Correlation with outcome:   0.723
   Backdoor path strength:     0.497
   Significance (p-value):     0.0045
```

**Interpretation:**
- Load balancer restarts correlate with both migrations and deployments
- Creates backdoor path: Migrations ← LB → Deployments
- Bias is 3.1% (small but significant)
- Adjusted effect accounts for this confounding

## Step 6: Pattern-Level Inspection

Look at specific patterns to understand context:

```bash
# View migration pattern details
mana patterns show 12

# View deployment pattern details
mana patterns show 20

# View load balancer pattern details
mana patterns show 15
```

Output:
```
Pattern #12
==========

Tool type: Bash
Score: 42 (87.5% success rate)
Uses: 56 success, 8 failure
Has embedding: ✓

Context:
python manage.py migrate --noinput
# Apply database migrations before deployment
# Critical for schema changes to take effect
```

## Step 7: Search for Similar Patterns

Find other patterns that might help:

```bash
mana patterns search "database" --limit 5
```

## Step 8: Make Informed Decisions

Based on causal analysis:

1. **Always run migrations before deployment** (strong causal effect)
2. **Monitor load balancer restarts** (significant confounder)
3. **Consider the migration → deployment chain** (highest strength)
4. **Test migrations in isolation** (to confirm causal effect)

## Advanced Usage

### A/B Testing Patterns

Compare two deployment strategies:

```bash
# Pattern 25: Blue-green deployment
# Pattern 30: Rolling deployment

# Find causal chains to success
mana causal chains 25 20
mana causal chains 30 20

# Compare effects
mana causal intervention 25 20
mana causal intervention 30 20
```

### Detecting Pattern Conflicts

Find patterns that interfere with each other:

```bash
# Show patterns that conflict with migrations
mana patterns show 12

# Look at causal edges section for conflicts (lift < 0.5)
```

### Time-Series Analysis

Use pattern timestamps to understand temporal causality:

```bash
# List patterns by recent usage
mana patterns list --sort recent --limit 20

# Find chains between recent patterns
mana causal chains <recent_id> <outcome_id>
```

## Best Practices

### When to Use Each Command

**`causal stats`**
- Quick overview of causal graph
- Monitor graph growth over time
- Identify imbalances (too many conflicts)

**`causal chains`**
- Understand multi-step processes
- Find indirect causal paths
- Debug complex workflows

**`causal intervention`**
- Estimate effect of changes
- Test hypotheses about causation
- Make data-driven decisions

**`causal confounders`**
- Validate causal claims
- Identify hidden variables
- Adjust effect estimates

### Interpretation Guidelines

**Lift Interpretation:**
- lift > 2.0: Very strong positive relationship
- lift > 1.5: Strong synergy
- 1.0 < lift < 1.5: Moderate positive effect
- lift ≈ 1.0: No relationship
- 0.5 < lift < 1.0: Moderate negative effect
- lift < 0.5: Strong conflict
- lift < 0.3: Very strong interference

**P-value Interpretation:**
- p < 0.001: Highly significant
- p < 0.01: Very significant
- p < 0.05: Significant
- p >= 0.05: Not significant

**Confidence Intervals:**
- Narrow CI: Precise estimate
- Wide CI: High uncertainty
- CI excludes 1.0: Significant effect
- CI includes 1.0: No clear effect

## Common Patterns

### Pattern Discovery
```bash
# Find high-value patterns
mana patterns list --min-score 10 --limit 10

# Get their IDs and test causal relationships
mana causal intervention <high_value_id> <outcome_id>
```

### Root Cause Analysis
```bash
# Find what leads to failures
mana patterns list --tool failure --limit 5

# Trace back to root causes
mana causal chains <failure_id> <success_id>
```

### Pipeline Optimization
```bash
# Identify bottlenecks
mana causal stats

# Find highest-impact interventions
for id in {1..50}; do
  mana causal intervention $id <critical_outcome_id>
done | grep "p < 0.05"
```

## Troubleshooting

### "No chains found"
- Increase --max-hops
- Check if patterns are connected (use `causal stats`)
- Verify patterns exist (use `patterns show`)

### "No confounders detected"
- Decrease --min-significance threshold
- Patterns may genuinely have no confounders
- Check if sample size is sufficient

### "Sample size: 0"
- Patterns haven't co-occurred yet
- Need more learning data
- Run workflows to generate more observations

## Next Steps

1. Integrate causal insights into workflow decisions
2. Monitor causal effects over time
3. Test interventions in practice
4. Share findings with team using `mana sync push`
5. Build automated alerts for causal pattern changes

---

For more information, see:
- `CAUSAL_SYSTEM_SUMMARY.md` - Technical details
- `CAUSAL_CLI_INTEGRATION.md` - Integration guide
- `tests/causal_test.rs` - Code examples
