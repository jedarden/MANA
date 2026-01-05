# Provenance Module Implementation

## Overview

This module implements explainability and provenance tracking for MANA's pattern learning system. It provides cryptographically verifiable audit trails using SHA256-based Merkle trees.

## Architecture

### Core Components

1. **ProvenanceStore** (`provenance.rs`)
   - Main storage and retrieval interface
   - Manages SQLite connections
   - Implements Merkle tree generation and verification

2. **ProvenanceCertificate**
   - Complete provenance record for a pattern
   - Includes Merkle root for verification
   - Stores derivation chain and confidence factors

3. **DerivationStep**
   - Individual steps in pattern evolution
   - Types: Extracted, Merged, Reflected, Reinforced, Decayed, UserFeedback
   - Timestamped with evidence

4. **ReasoningChainRecord**
   - Thought-Action-Observation sequences
   - Links to patterns
   - Records decision-making process

5. **ActionJustification**
   - Explains why actions were taken
   - Lists supporting evidence
   - Shows alternatives considered

## Key Features

### 1. Merkle Tree Proofs

```rust
// Generate a certificate with Merkle root
let cert = prov_store.generate_certificate(pattern_id)?;

// Verify certificate integrity
let is_valid = prov_store.verify_certificate(&cert)?;
```

**How it works:**
- Each derivation step is hashed using SHA256
- Pattern ID is included as root leaf for uniqueness
- Hashes are combined recursively in pairs
- Final hash is the Merkle root
- Any modification to the chain changes the root

### 2. Derivation Tracking

```rust
// Record a new derivation step
let step = DerivationStep {
    step_type: DerivationType::Reinforced,
    timestamp: current_timestamp(),
    evidence: "Pattern succeeded in 5 consecutive uses".to_string(),
    confidence_delta: 0.15,
};

prov_store.record_derivation(pattern_id, step)?;
```

**Derivation Types:**
- `Extracted`: Initial pattern extraction from trajectory
- `Merged`: Combined with similar pattern
- `Reflected`: Updated by reflection system
- `Reinforced`: Success feedback received
- `Decayed`: Confidence decreased
- `UserFeedback`: Manual user adjustment

### 3. Confidence Calculation

The system calculates pattern confidence using three weighted factors:

```rust
pub struct ConfidenceFactor {
    pub factor_name: String,
    pub value: f64,        // 0.0 to 1.0
    pub weight: f64,       // 0.0 to 1.0
    pub evidence: Option<String>,
}
```

**Default factors:**
- **success_rate** (weight: 0.40): `successes / (successes + failures)`
- **usage_frequency** (weight: 0.30): `ln(1 + uses) / 5.0`
- **recency** (weight: 0.30): Time-based decay score

**Overall confidence:**
```
confidence = Σ(factor.value × factor.weight)
```

### 4. Reasoning Chains

```rust
let chain = ReasoningChainRecord {
    id: 0,
    pattern_id: 42,
    task_context: "User requested to run tests".to_string(),
    steps: vec![
        ReasoningStep {
            step_type: ReasoningStepType::Thought,
            content: "Need to run tests with coverage".to_string(),
            evidence: None,
        },
        ReasoningStep {
            step_type: ReasoningStepType::Action,
            content: "Selected pattern for npm test --coverage".to_string(),
            evidence: Some("85% success rate".to_string()),
        },
        ReasoningStep {
            step_type: ReasoningStepType::Observation,
            content: "Command succeeded, coverage generated".to_string(),
            evidence: None,
        },
    ],
    final_decision: "Recommend pattern for future test operations".to_string(),
    confidence: 0.85,
    timestamp: current_timestamp(),
};

prov_store.record_reasoning(&chain)?;
```

### 5. Action Justification

```rust
// Get alternatives for comparison
let alternatives = vec![
    (pattern_id_2, 12.0),
    (pattern_id_3, 10.5),
];

let justification = prov_store.justify_action(
    "run tests",
    Some(pattern_id),
    &alternatives
)?;

// justification contains:
// - reasoning: Why this pattern was chosen
// - supporting_evidence: List of Evidence with strength scores
// - alternatives_considered: What was rejected and why
// - confidence: Overall confidence in the decision
```

## Database Schema

### provenance table

```sql
CREATE TABLE provenance (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pattern_id INTEGER NOT NULL,
    merkle_root TEXT NOT NULL,
    derivation_chain TEXT NOT NULL,      -- JSON serialized Vec<DerivationStep>
    confidence_factors TEXT NOT NULL,    -- JSON serialized Vec<ConfidenceFactor>
    source_trajectories TEXT NOT NULL,   -- JSON serialized Vec<String>
    created_at INTEGER NOT NULL,
    FOREIGN KEY (pattern_id) REFERENCES patterns(id) ON DELETE CASCADE
);

CREATE INDEX idx_provenance_pattern ON provenance(pattern_id);
```

### reasoning_chains table

```sql
CREATE TABLE reasoning_chains (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pattern_id INTEGER,
    task_context TEXT NOT NULL,
    steps TEXT NOT NULL,              -- JSON serialized Vec<ReasoningStep>
    final_decision TEXT NOT NULL,
    confidence REAL NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_reasoning_pattern ON reasoning_chains(pattern_id);
CREATE INDEX idx_reasoning_timestamp ON reasoning_chains(created_at DESC);
```

## Integration Points

### 1. Pattern Learning

When patterns are extracted:
```rust
// In learning/trajectory.rs
let step = DerivationStep {
    step_type: DerivationType::Extracted,
    timestamp: current_timestamp(),
    evidence: format!("Pattern extracted with hash: {}", pattern.pattern_hash),
    confidence_delta: 0.5,
};

prov_store.record_derivation(pattern.id, step)?;
```

### 2. Reflection System

When verdicts are generated:
```rust
// In reflection/analyzer.rs
if verdict.action == VerdictAction::Boost {
    let step = DerivationStep {
        step_type: DerivationType::Reflected,
        timestamp: current_timestamp(),
        evidence: verdict.reasoning.clone(),
        confidence_delta: 0.1,
    };
    prov_store.record_derivation(pattern_id, step)?;
}
```

### 3. Pattern Updates

When patterns succeed or fail:
```rust
// In storage/patterns.rs
if success {
    let step = DerivationStep {
        step_type: DerivationType::Reinforced,
        timestamp: current_timestamp(),
        evidence: "Pattern succeeded".to_string(),
        confidence_delta: 0.05,
    };
    prov_store.record_derivation(pattern_id, step)?;
}
```

## API Reference

### ProvenanceStore Methods

#### `new(conn: Connection) -> Result<Self>`
Create a new provenance store with the given database connection.

#### `init_tables() -> Result<()>`
Initialize the provenance and reasoning_chains tables. Call this once during setup.

#### `generate_certificate(pattern_id: i64) -> Result<ProvenanceCertificate>`
Generate a complete provenance certificate for a pattern. This includes:
- Calculating confidence factors
- Building derivation chain
- Computing Merkle root
- Storing the certificate

#### `verify_certificate(cert: &ProvenanceCertificate) -> Result<bool>`
Verify the cryptographic integrity of a certificate by recalculating the Merkle root.

#### `record_derivation(pattern_id: i64, step: DerivationStep) -> Result<()>`
Add a new derivation step to a pattern's provenance chain. Updates the Merkle root.

#### `get_provenance(pattern_id: i64) -> Result<ProvenanceCertificate>`
Retrieve the complete provenance certificate for a pattern.

#### `record_reasoning(chain: &ReasoningChainRecord) -> Result<i64>`
Store a reasoning chain record. Returns the chain ID.

#### `justify_action(action: &str, pattern_id: Option<i64>, alternatives: &[(i64, f64)]) -> Result<ActionJustification>`
Generate a justification for why an action was taken, including evidence and alternatives.

#### `explain_selection(pattern_id: i64, context: &str) -> Result<String>`
Generate a human-readable explanation of why a pattern was selected.

#### `get_recent_reasoning(limit: usize) -> Result<Vec<ReasoningChainRecord>>`
Retrieve recent reasoning chains, most recent first.

## Usage Examples

### Example 1: Generate and Verify Certificate

```rust
use storage::{ProvenanceStore, DerivationStep, DerivationType};
use rusqlite::Connection;

// Open connection
let conn = Connection::open("mana/metadata.sqlite")?;
let prov_store = ProvenanceStore::new(conn)?;
prov_store.init_tables()?;

// Generate certificate
let cert = prov_store.generate_certificate(42)?;

println!("Pattern #42 Provenance:");
println!("  Merkle Root: {}", cert.merkle_root);
println!("  Derivation Steps: {}", cert.derivation_chain.len());
println!("  Confidence Factors: {}", cert.confidence_factors.len());

// Verify
let is_valid = prov_store.verify_certificate(&cert)?;
assert!(is_valid, "Certificate should be valid");
```

### Example 2: Record Learning Event

```rust
// When a pattern succeeds
let step = DerivationStep {
    step_type: DerivationType::Reinforced,
    timestamp: current_timestamp(),
    evidence: format!("Pattern succeeded in {} uses", success_count),
    confidence_delta: 0.05,
};

prov_store.record_derivation(pattern_id, step)?;

// Certificate is automatically updated with new Merkle root
```

### Example 3: Explain Pattern Selection

```rust
let explanation = prov_store.explain_selection(
    42,
    "debugging compilation error"
)?;

println!("{}", explanation);
// Outputs formatted explanation with:
// - Pattern details
// - Performance metrics
// - Confidence factors
// - Derivation history
// - Verification status
```

### Example 4: Justify Decision

```rust
// Alternatives that were considered
let alternatives = vec![
    (15, 8.5),  // pattern_id, score
    (23, 7.2),
    (31, 6.8),
];

let justification = prov_store.justify_action(
    "compile project",
    Some(42),  // Selected pattern
    &alternatives
)?;

println!("Why pattern #42?");
println!("  Confidence: {:.1}%", justification.confidence * 100.0);
println!("  Reasoning: {}", justification.reasoning);
println!("  Evidence: {} pieces", justification.supporting_evidence.len());
println!("  Alternatives: {} considered", justification.alternatives_considered.len());
```

## Testing

Run the unit tests:
```bash
cargo test --lib storage::provenance
```

Key test cases:
- `test_merkle_root_calculation`: Verifies Merkle root is deterministic
- `test_provenance_certificate`: Tests certificate generation and verification
- `test_derivation_recording`: Tests adding steps to derivation chain
- `test_reasoning_chains`: Tests reasoning chain storage and retrieval

## Performance Considerations

1. **Merkle Root Calculation**: O(n log n) where n = number of derivation steps
   - Typical chains have < 20 steps, so this is very fast
   - Cached in database, only recalculated when chain changes

2. **Certificate Generation**:
   - 3 database queries (pattern data, existing provenance, causal edges)
   - SHA256 hashing is extremely fast (< 1μs per hash)
   - Total time: < 5ms for typical patterns

3. **Verification**:
   - Just recalculates Merkle root and compares
   - < 1ms for typical chains

4. **Storage**:
   - JSON serialization for flexible schema
   - Indexes on pattern_id for fast lookups
   - Reasonable storage: ~1KB per certificate

## Security Considerations

1. **Cryptographic Integrity**
   - SHA256 provides 256-bit collision resistance
   - Merkle root changes with any modification to chain
   - Cannot forge certificates without breaking SHA256

2. **Timestamp Integrity**
   - Unix timestamps are immutable once recorded
   - Sequence of timestamps provides ordering guarantee

3. **Database Integrity**
   - Foreign key constraints ensure pattern references are valid
   - ON DELETE CASCADE prevents orphaned provenance records

## Future Enhancements

1. **Merkle Proofs**: Generate inclusion proofs for specific steps
2. **Certificate Export**: JSON export for external verification
3. **Distributed Verification**: P2P certificate sharing and verification
4. **Provenance Merging**: Combine certificates when merging patterns
5. **Selective Disclosure**: Prove properties without revealing full chain
6. **Audit Logs**: Separate table for all provenance queries

## References

- [Merkle Trees](https://en.wikipedia.org/wiki/Merkle_tree)
- [SHA-256](https://en.wikipedia.org/wiki/SHA-2)
- [Provenance in AI Systems](https://arxiv.org/abs/2106.01516)
- [Explainable AI](https://www.darpa.mil/program/explainable-artificial-intelligence)
