# Transfer Learning API for MANA

This document describes the Transfer Learning API implementation for MANA, enabling cross-session and cross-project knowledge transfer.

## Overview

The Transfer Learning API allows MANA to:
- **Transfer patterns between sessions**: Share learned patterns from one work session to another
- **Transfer patterns between projects**: Reuse successful patterns in new projects
- **Adapt patterns for different domains**: Intelligently modify patterns for new contexts
- **Transfer RL policies**: Move Q-learning policies between contexts

## Architecture

### Core Components

1. **TransferEngine** (`src/learning/transfer.rs`): Main orchestration engine
2. **TransferConfig**: Configuration for transfer operations
3. **TransferSource**: Specification of where patterns come from
4. **AdaptationStrategy**: How patterns should be adapted for new contexts

### File Structure

```
mana/src/learning/
├── transfer.rs          # Transfer learning implementation (NEW)
├── mod.rs              # Updated to export transfer module
└── ...

mana/src/main.rs        # Updated with CLI commands
```

## CLI Commands

### 1. Transfer Patterns from Source

Transfer patterns from a source to the current project:

```bash
# Basic transfer
mana transfer from <source>

# Transfer with filters
mana transfer from /path/to/source.db --min-score 5 --min-success-rate 0.7

# Preview what would be transferred
mana transfer from <session-id> --preview

# Transfer to specific destination
mana transfer from <source> --to /path/to/dest.db

# Transfer only top 10% patterns
mana transfer from <source> --top 0.9

# Filter by tool types
mana transfer from <source> --tool-types "Bash,Edit,Write"

# Filter by domain keywords
mana transfer from <source> --domains "rust,cargo,testing"

# Transfer with adaptation
mana transfer from <source> --adapt generalize
mana transfer from <source> --adapt specialize --target-domain "python"
```

### 2. List Transferable Patterns

View patterns available for transfer from a source:

```bash
# List top 20 transferable patterns
mana transfer list <source>

# List top 50 with minimum transferability score
mana transfer list <source> --limit 50 --min-score 0.7
```

### 3. Transfer RL Policy

Transfer Q-learning policy (Q-table) between contexts:

```bash
# Transfer policy from source to current project
mana transfer policy <source>

# Transfer to specific destination
mana transfer policy <source> --to /path/to/dest.db
```

## Transfer Sources

The `<source>` parameter can be:

1. **Database path**: Direct path to a `metadata.sqlite` file
   ```bash
   mana transfer from /path/to/metadata.sqlite
   ```

2. **Project directory**: Path to a project with `.mana` directory
   ```bash
   mana transfer from /path/to/project
   ```

3. **Session ID**: Claude session identifier
   ```bash
   mana transfer from abc123def456
   ```

## Configuration

### TransferConfig

```rust
pub struct TransferConfig {
    pub min_score: i64,              // Minimum pattern score to transfer
    pub min_success_rate: f64,        // Minimum success rate (0.0-1.0)
    pub adapt_tier: bool,             // Adapt tier_path for destination
    pub preserve_provenance: bool,    // Keep provenance history
    pub merge_duplicates: bool,       // Merge similar patterns
    pub similarity_threshold: f64,    // Threshold for duplicate detection
}
```

**Defaults:**
- `min_score`: 0
- `min_success_rate`: 0.5
- `adapt_tier`: true
- `preserve_provenance`: true
- `merge_duplicates`: true
- `similarity_threshold`: 0.85

### Adaptation Strategies

1. **Direct**: Transfer patterns as-is without modification
   ```bash
   mana transfer from <source> --adapt direct
   ```

2. **Contextualize**: Add domain context to patterns
   ```bash
   mana transfer from <source> --adapt contextualize --target-domain "web-dev"
   ```

3. **Generalize**: Remove domain-specific details (paths, names)
   ```bash
   mana transfer from <source> --adapt generalize
   ```

4. **Specialize**: Only transfer patterns relevant to target domain
   ```bash
   mana transfer from <source> --adapt specialize --target-domain "rust"
   ```

## Transferability Score

Each pattern receives a transferability score (0.0-1.0) based on:

- **Success rate** (40%): How often the pattern succeeds
- **Score** (30%): Net success - failure count
- **Usage** (20%): How frequently the pattern is used
- **Freshness** (10%): How recently the pattern was created

Formula:
```rust
transferability = 0.4 * success_rate
                + 0.3 * (score/10)
                + 0.2 * (min(usage, 50)/50)
                + 0.1 * (1 / (1 + age_days/30))
```

## Transfer Operations

### What Gets Transferred

1. **Patterns**: Tool usage patterns with success/failure counts
2. **Skills**: High-level skill abstractions
3. **Causal Edges**: Pattern relationships and synergies
4. **Q-Learning Policy**: Reinforcement learning state-action values

### Merge Strategies

When patterns already exist in the destination:

- **Add** (default): Merge counts for existing patterns
- **Replace**: Replace existing with imported
- **KeepBest**: Keep pattern with better success rate

## Examples

### Example 1: Transfer Successful Patterns to New Project

```bash
# Navigate to new project
cd /path/to/new-project

# Initialize MANA
mana init

# Transfer top patterns from previous project
mana transfer from /path/to/old-project --top 0.8 --min-success-rate 0.7

# Result:
# Transfer Complete
# =================
# Patterns transferred: 45
# Patterns merged: 12
# Skills transferred: 8
# Causal edges transferred: 23
```

### Example 2: Transfer Domain-Specific Knowledge

```bash
# Transfer only Rust-related patterns
mana transfer from <session-id> \
  --tool-types "Bash,Edit" \
  --domains "rust,cargo,tokio" \
  --min-score 3

# Preview before executing
mana transfer from <source> --preview
# Transfer Preview
# ================
# Total patterns in source: 230
# Eligible for transfer: 87
# Would merge with existing: 12
# Would skip: 143
# Estimated benefit score: 0.78
```

### Example 3: Transfer with Adaptation

```bash
# Generalize patterns for reuse across projects
mana transfer from /workspace/project-a \
  --adapt generalize \
  --min-success-rate 0.8

# Specialize for Python development
mana transfer from /workspace/polyglot-project \
  --adapt specialize \
  --target-domain "python" \
  --tool-types "Edit,Write,Bash"
```

### Example 4: Transfer RL Policy

```bash
# Transfer learned Q-values from experienced project
mana transfer policy /workspace/mature-project

# Result:
# Policy Transfer Complete
# ========================
# States transferred: 156
# Actions transferred: 892
# Q-values adapted: 234
```

## API Usage (Rust)

For programmatic use within MANA:

```rust
use mana::learning::{
    TransferEngine, TransferConfig, TransferSource, AdaptationStrategy
};

// Create transfer engine with custom config
let config = TransferConfig {
    min_score: 5,
    min_success_rate: 0.75,
    ..Default::default()
};
let engine = TransferEngine::new(config);

// Define source
let source = TransferSource::Database(
    PathBuf::from("/path/to/source.db")
);

// Transfer patterns
let result = engine.transfer(&source, dest_db)?;
println!("Transferred {} patterns", result.patterns_transferred);

// Preview transfer
let preview = engine.preview_transfer(&source, dest_db)?;
println!("Would transfer {} patterns", preview.eligible_patterns);

// Transfer with adaptation
let result = engine.transfer_with_adaptation(
    &source,
    dest_db,
    AdaptationStrategy::Generalize,
    "web-development"
)?;

// Get transferable patterns
let patterns = engine.get_transferable(&source)?;
for pattern in patterns.iter().take(10) {
    println!("{}: {:.2}", pattern.tool_type, pattern.transferability_score);
}
```

## Implementation Details

### Database Schema

The transfer system works with existing MANA tables:
- `patterns`: Pattern storage
- `skills`: Skill abstractions
- `causal_edges`: Pattern relationships
- `q_table`: Q-learning policy (optional)

### Pattern Matching

Patterns are matched by `pattern_hash` for deduplication. When transferring:
1. Hash is computed from tool_type + context
2. Existing patterns with same hash are merged
3. New patterns are inserted

### Security

- Paths are sanitized during transfer (configurable)
- Secrets can be redacted (via SecurityConfig)
- Provenance history tracks transfer source

## Performance

Transfer operations are optimized for:
- **Speed**: Batch operations using transactions
- **Memory**: Streaming for large pattern sets
- **Disk**: Efficient SQLite operations

Typical transfer times:
- 100 patterns: ~50ms
- 1000 patterns: ~300ms
- 10000 patterns: ~2s

## Future Enhancements

Planned improvements:
1. **Cross-user transfer**: Share patterns with team members
2. **Semantic filtering**: Use embeddings for context-aware transfer
3. **Incremental transfer**: Only transfer new/updated patterns
4. **Transfer analytics**: Track transfer effectiveness over time
5. **Automated transfer**: Suggest transfers based on current context

## Testing

Run tests:
```bash
cargo test transfer --lib
```

Integration tests cover:
- Basic transfer operations
- Adaptation strategies
- Policy transfer
- Preview functionality
- Error handling

## Troubleshooting

### Common Issues

**Q: Transfer shows 0 patterns transferred**
- Check source path is valid
- Verify patterns meet min_score and min_success_rate criteria
- Use `--preview` to see what would be transferred

**Q: Patterns not appearing after transfer**
- Ensure destination database is initialized (`mana init`)
- Check patterns aren't being filtered by tier/domain
- Verify merge strategy isn't skipping patterns

**Q: Transfer fails with "database locked"**
- Close other MANA processes
- Ensure no active learning operations
- Use read-only mode if only previewing

## Related Documentation

- [MANA Architecture](prompt.md)
- [Health Monitoring](HEALTH_MONITORING.md)
- [RL Algorithms](src/learning/README.md)
- [Sync API](src/sync/README.md)

## Contributing

When modifying the transfer API:
1. Update this documentation
2. Add tests for new features
3. Maintain backward compatibility
4. Update CLI help text

## License

Same as MANA project license.
