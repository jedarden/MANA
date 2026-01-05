# Multi-Factor Ranking System for Pattern Retrieval

## Overview

The multi-factor ranking system combines multiple signals to rank patterns by relevance:

- **Similarity** (50%): Semantic/text similarity to query
- **Recency** (20%): Recently accessed patterns ranked higher (exponential decay)
- **Popularity** (15%): Frequently used patterns score higher
- **Confidence** (15%): Success rate and absolute score

## Architecture

### Components

1. **`ranking.rs`**: Core ranking implementation
   - `PatternRanker`: Main ranking engine
   - `RankingConfig`: Configurable weights
   - `RankingFactors`: Individual factor scores
   - `RankedPattern`: Pattern with ranking metadata

2. **Pattern Fields**:
   - `last_used: Option<String>` - ISO 8601 timestamp of last use
   - `access_count: i64` - Number of times pattern has been accessed

3. **Database Migration**:
   - Auto-migrates existing databases to add `access_count` column
   - Backwards compatible with existing patterns

## Usage

### Basic Usage

```rust
use mana::storage::{PatternRanker, RankingConfig};

// Create ranker with default weights
let ranker = PatternRanker::new_default();

// Get top-k patterns with multi-factor ranking
let ranked = ranker.get_top_ranked(
    &conn,
    "Bash",           // tool type
    "cargo build",    // query
    5,                // top-k
)?;

for ranked_pattern in ranked {
    println!("Pattern: {:?}", ranked_pattern.pattern.context_query);
    println!("Final score: {:.3}", ranked_pattern.final_score);
    println!("  - Similarity: {:.3}", ranked_pattern.factors.similarity);
    println!("  - Recency: {:.3}", ranked_pattern.factors.recency_score);
    println!("  - Popularity: {:.3}", ranked_pattern.factors.popularity_score);
    println!("  - Confidence: {:.3}", ranked_pattern.factors.confidence_score);
}
```

### Custom Weights

```rust
use mana::storage::{PatternRanker, RankingConfig};

// Emphasize recency over other factors
let config = RankingConfig {
    similarity_weight: 0.40,
    recency_weight: 0.40,
    popularity_weight: 0.10,
    confidence_weight: 0.10,
    recency_decay_days: 7.0,
};

let ranker = PatternRanker::new(config);
```

### Manual Ranking

```rust
use mana::storage::{PatternRanker, calculate_similarity};

let ranker = PatternRanker::new_default();

// Get candidate patterns
let patterns = store.get_by_tool("Bash", 20)?;

// Calculate similarities
let patterns_with_similarity: Vec<(Pattern, f64)> = patterns
    .into_iter()
    .map(|p| {
        let similarity = calculate_similarity(query, &p.context_query);
        (p, similarity)
    })
    .collect();

// Rank with all factors
let ranked = ranker.rank(patterns_with_similarity, &conn)?;
```

## Ranking Factors

### 1. Similarity (50% weight)

- Calculated using TF-IDF or vector embeddings
- Range: 0.0 - 1.0
- Higher for semantically similar patterns

### 2. Recency (20% weight)

- Exponential decay based on `last_used` timestamp
- Formula: `score = 0.5^(days_old / half_life)`
- Default half-life: 7 days
- Never-used patterns get score of 0.0

Examples:
- Used today: ~1.0
- Used 7 days ago: ~0.5
- Used 14 days ago: ~0.25
- Never used: 0.0

### 3. Popularity (15% weight)

- Based on `access_count`
- Log-scaled to prevent domination by extremely popular patterns
- Formula: `log(1 + count) / log(1 + max_count)`
- Range: 0.0 - 1.0

### 4. Confidence (15% weight)

- Combines success rate and absolute performance
- Formula: `success_rate * 0.7 + log_scaled_score * 0.3`
- Range: 0.0 - 1.0
- Balances percentage and absolute metrics

## Integration Points

### Context Injection

The ranking system is used in `hooks/context_injection.rs`:

1. Patterns are retrieved from the database
2. Similarity scores are calculated
3. Multi-factor ranking is applied
4. Top-k patterns are selected for injection

### Pattern Updates

When patterns are accessed:

```rust
// Updates both last_used and access_count
store.mark_patterns_used(&pattern_ids)?;
```

This is automatically called after context injection.

## Performance

- **Startup overhead**: Minimal (migration checks cached)
- **Query time**: ~same as before (ranking happens on already-retrieved patterns)
- **Memory**: Negligible (only metadata)

## Backwards Compatibility

- Existing databases auto-migrate on first run
- Old patterns get `access_count = 0` and `last_used = NULL`
- Gracefully handles missing fields with defaults
- All existing code continues to work

## Testing

Run the built-in tests:

```bash
cd /workspaces/ardenone-cluster/mana
cargo test ranking
```

Test coverage includes:
- Recency score calculation with exponential decay
- Popularity normalization with log scaling
- Confidence calculation with success rate
- Weight sum validation
- End-to-end ranking

## Future Enhancements

Possible improvements:

1. **Configurable weights per tool type** (e.g., emphasize recency for Bash, confidence for Edit)
2. **Adaptive weights** based on query characteristics
3. **Per-user preferences** for ranking weights
4. **A/B testing** different ranking configurations
5. **Decay patterns** that haven't been used in X days
6. **Boost patterns** from the same tier_path as current context
