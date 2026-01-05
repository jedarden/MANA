# Provenance System Quick Start

## Installation

1. Build MANA with the new provenance features:
```bash
cd /workspaces/ardenone-cluster/mana
cargo build --release
```

2. Initialize the database (creates provenance tables):
```bash
mana init
```

## 5-Minute Tutorial

### Step 1: Learn Some Patterns

Let MANA learn from your Claude sessions:
```bash
# MANA automatically learns from ~/.claude/projects/*
# Or manually trigger learning:
mana consolidate
```

### Step 2: Find Interesting Patterns

```bash
# List top patterns
mana patterns list --limit 10

# Output:
# 1. [Bash] score:14 npm test
# 2. [Bash] score:12 cargo build
# 3. [Edit] score:11 fix TypeScript errors
# ...
```

### Step 3: Explain a Pattern

```bash
# Choose a pattern ID from the list above (e.g., pattern #1)
mana provenance explain 1
```

**You'll see**:
- Why this pattern was created
- Its success rate and usage statistics
- How its confidence has evolved
- Complete derivation history
- Cryptographic verification status

### Step 4: View Full Provenance

```bash
mana provenance show 1
```

**You'll see**:
- Merkle root (cryptographic hash)
- Source trajectories (when it was learned)
- Confidence factors breakdown
- Complete derivation chain
- Verification ✓ or ✗

### Step 5: Justify a Decision

```bash
# Explain why MANA would choose this pattern
mana provenance justify "run tests" --pattern-id 1
```

**You'll see**:
- Why this pattern was selected
- Supporting evidence with strength scores
- Alternative patterns that were considered
- Reasons why alternatives were rejected

## Common Commands

```bash
# Explain pattern selection
mana provenance explain <id> [--context "description"]

# Show complete provenance
mana provenance show <id>

# Justify an action
mana provenance justify "action" [--pattern-id <id>]

# View recent reasoning chains
mana provenance chains [--limit 10]

# Verify certificate integrity
mana provenance verify <id>
```

## Real-World Examples

### Example 1: Debug Unexpected Pattern

You notice MANA suggested an unexpected pattern. Find out why:

```bash
# Get the pattern ID from recent activity
mana patterns list --sort recent

# Explain why it was selected
mana provenance explain 42 --context "current task"

# See its complete history
mana provenance show 42
```

### Example 2: Audit a Decision

For compliance or transparency:

```bash
# Generate provenance report
mana provenance show 15 > pattern-15-audit.txt

# Verify integrity
mana provenance verify 15

# Check reasoning
mana provenance chains --limit 5 | grep "Pattern #15"
```

### Example 3: Compare Patterns

Why did MANA choose pattern A over pattern B?

```bash
# Justify the action that used pattern A
mana provenance justify "compile project" --pattern-id 10

# Look at alternatives section to see why pattern B was rejected
```

## Understanding the Output

### Derivation Types

- **Extracted**: Initial creation from trajectory
- **Merged**: Combined with similar pattern
- **Reflected**: Updated by reflection system
- **Reinforced**: Positive feedback (success)
- **Decayed**: Negative feedback (failure/time)
- **UserFeedback**: Manual adjustment

### Confidence Factors

- **success_rate** (40% weight): Reliability
- **usage_frequency** (30% weight): Popularity
- **recency** (30% weight): Freshness

### Reasoning Steps

- **Thought**: Internal analysis
- **Action**: What was done
- **Observation**: What happened
- **Reflection**: Learning from result

## Tips

1. **Start with patterns list**: `mana patterns list --limit 20`
2. **Use explain for quick overview**: `mana provenance explain <id>`
3. **Use show for full details**: `mana provenance show <id>`
4. **Always verify important patterns**: `mana provenance verify <id>`
5. **Check reasoning chains regularly**: `mana provenance chains --limit 10`

## Troubleshooting

### "No database found"
Run `mana init` first.

### "Pattern not found"
Check `mana patterns list` for valid IDs.

### "Certificate verification failed"
This indicates tampering. Should not happen in normal use.

### "No reasoning chains recorded"
Run learning first with `mana consolidate`.

## Integration with Other MANA Features

The provenance system works with:

- **Patterns**: Track evolution of learned patterns
- **Reflection**: See how verdicts affect confidence
- **Causal Graph**: Evidence for pattern relationships
- **Skills**: Understand skill composition
- **Transfer Learning**: Preserve provenance when transferring

## Advanced Usage

### Custom Context

```bash
mana provenance explain 5 --context "production deployment"
```

### Batch Verification

```bash
for id in {1..10}; do
    mana provenance verify $id
done
```

### Export for Audit

```bash
# Create audit report for top 10 patterns
for id in $(mana patterns list --limit 10 | grep -oP '^\d+'); do
    echo "=== Pattern $id ===" >> audit-report.txt
    mana provenance show $id >> audit-report.txt
    echo "" >> audit-report.txt
done
```

## Learn More

- **User Guide**: See `docs/provenance-usage.md` for detailed examples
- **Technical Docs**: See `src/storage/README_PROVENANCE.md` for API reference
- **Implementation**: See `PROVENANCE_IMPLEMENTATION.md` for architecture

## Get Help

```bash
# Show help for provenance commands
mana provenance --help

# Show help for specific subcommand
mana provenance explain --help
mana provenance show --help
mana provenance justify --help
```

## Next Steps

1. Explore your patterns: `mana patterns list`
2. Pick interesting ones and explain them
3. Check reasoning chains: `mana provenance chains`
4. Verify important patterns: `mana provenance verify <id>`
5. Use justification for decisions: `mana provenance justify <action>`

The provenance system provides complete transparency into MANA's learning and decision-making!
