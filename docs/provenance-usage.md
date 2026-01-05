# MANA Provenance & Explainability System

## Overview

The provenance tracking system provides complete explainability and audit trails for MANA's pattern learning and decision-making process. It uses SHA256-based Merkle trees to create cryptographically verifiable provenance certificates.

## Features

### 1. Provenance Certificates
- **Merkle Root**: SHA256-based cryptographic hash of the entire derivation chain
- **Source Trajectories**: List of session IDs where the pattern was learned
- **Derivation Chain**: Step-by-step history of how the pattern evolved
- **Confidence Factors**: Weighted factors contributing to pattern confidence

### 2. Reasoning Chains
- Record Thought-Action-Observation cycles
- Track decision-making processes
- Store evidence for each reasoning step
- Meta-analysis through reflection steps

### 3. Action Justification
- Explain why a particular action was chosen
- Show supporting evidence with strength scores
- List alternatives that were considered and why they were rejected
- Provide confidence levels for decisions

## CLI Commands

### Show Provenance for a Pattern

```bash
# Show full provenance certificate
mana provenance show <pattern_id>

# Example output:
# Provenance Certificate for Pattern #42
# ============================================================
#
# Merkle Root: a1b2c3d4e5f6...
# Created: 2025-12-23 10:30:45 UTC
#
# Source Trajectories (2):
#   1. session-2025-12-20-abc123
#   2. session-2025-12-22-def456
#
# Confidence Factors (3):
#   success_rate = 0.85 (weight: 0.40)
#     Evidence: Success: 17, Failure: 3
#   usage_frequency = 0.62 (weight: 0.30)
#     Evidence: Total uses: 20
#   recency = 0.70 (weight: 0.30)
#     Evidence: 2025-12-23 10:25:00
#
# Derivation Chain (4 steps):
#   1. Extracted (delta: +0.50)
#      Pattern extracted with hash: abc123...
#      Time: 2025-12-20 14:20:00 UTC
#   2. Reinforced (delta: +0.15)
#      Pattern succeeded in 5 consecutive uses
#      Time: 2025-12-21 09:15:30 UTC
#   3. Reflected (delta: +0.10)
#      Reflection analysis increased confidence
#      Time: 2025-12-22 16:45:20 UTC
#   4. Reinforced (delta: +0.05)
#      Pattern succeeded in challenging context
#      Time: 2025-12-23 10:25:00 UTC
#
# Certificate Status: VERIFIED ✓
```

### Explain Pattern Selection

```bash
# Explain why a pattern was selected
mana provenance explain <pattern_id>

# With custom context
mana provenance explain <pattern_id> --context "debugging error"

# Example output:
# Pattern Selection Explanation for Pattern #42
# ========================================
#
# Tool Type: Bash
# Pattern Hash: abc123def456...
# Context: debugging error
#
# Performance Metrics:
#   Success Count: 17
#   Failure Count: 3
#   Success Rate: 85.0%
#
# Confidence Factors (3 total):
#   success_rate = 0.85 (weight: 0.40)
#     Evidence: Success: 17, Failure: 3
#   usage_frequency = 0.62 (weight: 0.30)
#     Evidence: Total uses: 20
#   recency = 0.70 (weight: 0.30)
#     Evidence: 2025-12-23 10:25:00
#
# Derivation History (4 steps):
#   1. Extracted (confidence delta: +0.50)
#      Pattern extracted with hash: abc123...
#      Timestamp: 2025-12-20 14:20:00 UTC
#   [... additional steps ...]
#
# Merkle Root (audit trail): a1b2c3d4e5f6...
# Verification Status: VERIFIED
```

### Justify an Action

```bash
# Justify an action that used a specific pattern
mana provenance justify "run tests" --pattern-id 42

# Justify an action without a specific pattern
mana provenance justify "install dependencies"

# Example output:
# Action Justification
# ============================================================
#
# Action: run tests
# Pattern Used: #42
# Confidence: 74.5%
#
# Reasoning:
#   Selected pattern #42 for action 'run tests' based on
#   6 confidence factors and 4 derivation steps.
#   Confidence: 74.5%
#
# Supporting Evidence (6):
#   1. [provenance] Pattern has 4 derivation steps
#      Strength: 80.0% | Source: Pattern #42
#   2. [confidence_factor] success_rate: 0.85
#      Strength: 40.0% | Source: Success: 17, Failure: 3
#   3. [confidence_factor] usage_frequency: 0.62
#      Strength: 30.0% | Source: Total uses: 20
#   4. [confidence_factor] recency: 0.70
#      Strength: 30.0% | Source: 2025-12-23 10:25:00
#   5. [historical_performance] Success rate: 85.0%
#      Strength: 90.0% | Source: 17 successes, 3 failures
#
# Alternatives Considered (5):
#   1. [score: 12.00] npm test --coverage
#      Rejected: Lower score (12.00) than selected pattern
#   2. [score: 10.50] pytest -v
#      Rejected: Lower score (10.50) than selected pattern
#   [... more alternatives ...]
```

### View Reasoning Chains

```bash
# Show recent reasoning chains
mana provenance chains

# Show specific number of chains
mana provenance chains --limit 5

# Example output:
# Recent Reasoning Chains (showing 3)
# ============================================================
#
# 1. Chain #15 (Pattern #42)
#    Context: User requested to run tests with coverage
#    Confidence: 85.0%
#    Steps: 4
#      1. Thought: Need to run tests with coverage reporting
#      2. Action: Selected pattern for npm test --coverage
#         Evidence: Pattern has 85% success rate in similar contexts
#      3. Observation: Command executed successfully, coverage generated
#      4. Reflection: Pattern continues to perform well in test scenarios
#    Decision: Recommend pattern #42 for future test operations
#    Time: 2025-12-23 10:30:45 UTC
#
# 2. Chain #14 (Pattern #38)
#    [... similar structure ...]
```

### Verify Provenance Certificate

```bash
# Verify the integrity of a provenance certificate
mana provenance verify <pattern_id>

# Example output:
# Provenance Verification for Pattern #42
# ============================================================
#
# Merkle Root: a1b2c3d4e5f6789abcdef012345678...
# Derivation Steps: 4
#
# ✓ Certificate is VALID
#
# The provenance chain has not been tampered with.
# All derivation steps are cryptographically verified.
```

## Derivation Types

The system tracks six types of derivation steps:

1. **Extracted**: Pattern was extracted from a trajectory
2. **Merged**: Pattern was merged with a similar pattern
3. **Reflected**: Pattern was updated by the reflection system
4. **Reinforced**: Pattern received positive feedback (success)
5. **Decayed**: Pattern confidence decreased due to time or failures
6. **UserFeedback**: Pattern was modified based on explicit user input

## Confidence Factors

The system calculates confidence based on three weighted factors:

1. **Success Rate** (weight: 0.40)
   - Calculated from success_count / (success_count + failure_count)
   - Directly reflects pattern reliability

2. **Usage Frequency** (weight: 0.30)
   - Logarithmic scale of total uses
   - Rewards patterns that are used frequently

3. **Recency** (weight: 0.30)
   - How recently the pattern was used
   - Prevents stale patterns from dominating

## Reasoning Step Types

Reasoning chains support four step types:

1. **Thought**: Internal reasoning and analysis
2. **Action**: Concrete action taken
3. **Observation**: Results and outcomes observed
4. **Reflection**: Meta-analysis and learning

## Use Cases

### Debugging Pattern Selection
When MANA selects an unexpected pattern:
```bash
mana provenance explain <pattern_id> --context "current task"
```

### Auditing Decisions
For compliance or transparency requirements:
```bash
mana provenance show <pattern_id>
mana provenance verify <pattern_id>
```

### Understanding Learning
Track how patterns evolve over time:
```bash
mana provenance show <pattern_id>
# Examine the derivation chain to see how confidence changed
```

### Comparing Alternatives
See why one pattern was chosen over others:
```bash
mana provenance justify "action description" --pattern-id <id>
```

## Integration with Other MANA Features

### Reflection System
- Reflection verdicts automatically create derivation steps
- Confidence adjustments are recorded with evidence
- Verdicts appear in the provenance chain

### Causal Graph
- Causal relationships provide evidence for pattern selection
- Synergies and conflicts are included in justifications
- Pattern combinations are explained through causal reasoning

### Transfer Learning
- Provenance is preserved when transferring patterns
- Source domain and adaptation strategy are recorded
- Transfer confidence is tracked as a derivation step

## Technical Details

### Merkle Tree Construction

The system builds a Merkle tree from the derivation chain:

1. Each derivation step is hashed using SHA256
2. The pattern ID is included as the root leaf for uniqueness
3. Pairs of hashes are combined and hashed recursively
4. The final hash is the Merkle root

This provides:
- **Tamper Evidence**: Any change to the derivation chain changes the root
- **Efficient Verification**: Can verify without recalculating everything
- **Cryptographic Security**: SHA256 provides collision resistance

### Database Schema

```sql
CREATE TABLE provenance (
    id INTEGER PRIMARY KEY,
    pattern_id INTEGER NOT NULL,
    merkle_root TEXT NOT NULL,
    derivation_chain TEXT NOT NULL,  -- JSON
    confidence_factors TEXT NOT NULL, -- JSON
    source_trajectories TEXT NOT NULL, -- JSON
    created_at INTEGER NOT NULL,
    FOREIGN KEY (pattern_id) REFERENCES patterns(id)
);

CREATE TABLE reasoning_chains (
    id INTEGER PRIMARY KEY,
    pattern_id INTEGER,
    task_context TEXT NOT NULL,
    steps TEXT NOT NULL,  -- JSON
    final_decision TEXT NOT NULL,
    confidence REAL NOT NULL,
    created_at INTEGER NOT NULL
);
```

## Best Practices

1. **Regular Verification**: Periodically verify important patterns
   ```bash
   mana provenance verify <pattern_id>
   ```

2. **Document Decisions**: Use reasoning chains to record important decisions
   ```bash
   # This is done automatically by MANA during learning
   ```

3. **Review Derivations**: Check how patterns evolved
   ```bash
   mana provenance show <pattern_id>
   ```

4. **Understand Rejections**: When a pattern isn't selected, check alternatives
   ```bash
   mana provenance justify "action" --pattern-id <id>
   ```

## Future Enhancements

- Export provenance certificates as standalone JSON files
- Import/verify external certificates
- Merkle proof generation for specific derivation steps
- Integration with external audit systems
- Provenance-based pattern ranking
- Automated derivation step recording during learning

## Example Workflow

```bash
# 1. Initialize MANA
mana init

# 2. After learning, check top patterns
mana patterns list --limit 10

# 3. Explain why a pattern is highly ranked
mana provenance explain 5

# 4. View its complete provenance
mana provenance show 5

# 5. Verify the certificate
mana provenance verify 5

# 6. See recent reasoning chains
mana provenance chains --limit 5

# 7. Justify a recent action
mana provenance justify "build project" --pattern-id 5
```

## Troubleshooting

### "No database found"
Run `mana init` first to initialize the database and provenance tables.

### "Pattern not found"
Use `mana patterns list` to see available patterns and their IDs.

### "Certificate verification failed"
This indicates the provenance chain may have been modified. This should not happen under normal operation.

### "No reasoning chains recorded"
Reasoning chains are created during learning. Run some learning cycles first.
