# Transfer Learning - Quick Start Guide

## 5-Minute Quick Start

### Install and Initialize
```bash
# Navigate to your project
cd /path/to/your/project

# Initialize MANA (if not already done)
mana init
```

### Transfer Patterns from Another Project
```bash
# Basic transfer (all eligible patterns)
mana transfer from /path/to/source/project

# Transfer only high-quality patterns
mana transfer from /path/to/source/project --min-score 5 --min-success-rate 0.8
```

### Preview Before Transferring
```bash
# See what would be transferred without doing it
mana transfer from /path/to/source --preview
```

### Filter by Domain
```bash
# Transfer only Rust-related patterns
mana transfer from /path/to/source --domains "rust,cargo" --tool-types "Bash,Edit"
```

### Transfer Top Patterns Only
```bash
# Transfer only the best 10% of patterns
mana transfer from /path/to/source --top 0.9
```

## Common Use Cases

### 1. Starting a New Project
```bash
# Transfer proven patterns from your best project
mana transfer from ~/projects/successful-project \
  --top 0.8 \
  --min-success-rate 0.75
```

### 2. Language-Specific Transfer
```bash
# Python development
mana transfer from ~/projects/python-work \
  --domains "python,pip,pytest" \
  --adapt specialize --target-domain "python"

# Rust development
mana transfer from ~/projects/rust-work \
  --domains "rust,cargo,tokio" \
  --adapt specialize --target-domain "rust"
```

### 3. Cleaning Up Patterns
```bash
# Generalize patterns for reuse
mana transfer from ~/projects/specific-app \
  --adapt generalize \
  --min-success-rate 0.7
```

### 4. Transfer Between Sessions
```bash
# Transfer from a Claude session ID
mana transfer from abc123def456 --preview
```

## Command Reference

### Transfer From
```bash
mana transfer from <source> [OPTIONS]

Options:
  --to <path>              Destination database
  --min-score <n>          Minimum score (default: 0)
  --min-success-rate <f>   Minimum success rate 0.0-1.0 (default: 0.5)
  --preview                Preview without transferring
  --tool-types <list>      Comma-separated tool types
  --domains <list>         Comma-separated domain keywords
  --top <percentile>       Transfer top N percentile (0.0-1.0)
  --adapt <strategy>       Adaptation: direct, contextualize, generalize, specialize
  --target-domain <name>   Target domain for adaptation
```

### List Transferable
```bash
mana transfer list <source> [OPTIONS]

Options:
  -l, --limit <n>          Max patterns to show (default: 20)
  --min-score <f>          Minimum transferability score 0.0-1.0 (default: 0.0)
```

### Transfer Policy
```bash
mana transfer policy <source> [OPTIONS]

Options:
  --to <path>              Destination database
```

## Understanding Sources

### Database Path
```bash
# Direct path to metadata.sqlite
mana transfer from /path/to/.mana/metadata.sqlite
```

### Project Directory
```bash
# Path to project root (must contain .mana/)
mana transfer from /path/to/project
```

### Session ID
```bash
# Claude session identifier
mana transfer from abc123session456
```

## Adaptation Strategies

### Direct (no modification)
```bash
mana transfer from <source> --adapt direct
```

### Contextualize (add domain tags)
```bash
mana transfer from <source> --adapt contextualize --target-domain "web-dev"
```

### Generalize (remove specifics)
```bash
mana transfer from <source> --adapt generalize
```

### Specialize (filter by relevance)
```bash
mana transfer from <source> --adapt specialize --target-domain "rust"
```

## Tips and Best Practices

### 1. Always Preview First
```bash
# Check what will be transferred
mana transfer from <source> --preview
```

### 2. Start with High Thresholds
```bash
# Begin conservative, then loosen if needed
mana transfer from <source> \
  --min-score 5 \
  --min-success-rate 0.8 \
  --top 0.9
```

### 3. Use Domain Filtering
```bash
# Only transfer relevant patterns
mana transfer from <source> \
  --domains "your,relevant,keywords" \
  --tool-types "Bash,Edit,Write"
```

### 4. Check Transferability Scores
```bash
# View patterns sorted by transferability
mana transfer list <source> --limit 50
```

### 5. Combine Filters
```bash
# Multiple filters for precision
mana transfer from <source> \
  --top 0.8 \
  --domains "rust,testing" \
  --tool-types "Bash,Edit" \
  --min-success-rate 0.75
```

## Troubleshooting

### No Patterns Transferred
- Lower `--min-score` and `--min-success-rate`
- Check source has patterns: `mana transfer list <source>`
- Use `--preview` to see eligibility

### Too Many Low-Quality Patterns
- Increase `--min-score` (e.g., 5 or higher)
- Increase `--min-success-rate` (e.g., 0.8)
- Use `--top 0.9` to get only best patterns

### Wrong Domain Patterns
- Add `--domains` filter
- Use `--adapt specialize --target-domain "your-domain"`
- Filter by `--tool-types`

## Advanced Examples

### Transfer Suite
```bash
# Complete transfer workflow
cd /new/project
mana init

# 1. Preview available patterns
mana transfer list /old/project --limit 100

# 2. Preview transfer
mana transfer from /old/project \
  --min-score 3 \
  --min-success-rate 0.7 \
  --preview

# 3. Execute transfer
mana transfer from /old/project \
  --min-score 3 \
  --min-success-rate 0.7

# 4. Transfer Q-learning policy
mana transfer policy /old/project

# 5. Verify
mana stats
```

### Multi-Source Transfer
```bash
# Transfer from multiple sources
for project in project-a project-b project-c; do
  mana transfer from ~/projects/$project \
    --min-score 5 \
    --top 0.9
done
```

### Targeted Transfer
```bash
# Transfer specific pattern types
mana transfer from <source> \
  --tool-types "Edit" \
  --domains "refactor,cleanup" \
  --adapt generalize
```

## Integration with MANA Workflow

### After Transfer
```bash
# Check what was transferred
mana patterns summary
mana stats

# View transferred patterns
mana patterns list --sort recent --limit 50

# Test with context injection
mana inject --tool bash
```

### Ongoing Maintenance
```bash
# Periodically clean up
mana health status
mana health auto-prune --dry-run

# Monitor effectiveness
mana stats
mana patterns list --min-score 5
```

## Next Steps

1. Read full documentation: `TRANSFER_LEARNING.md`
2. Explore API usage: `src/learning/transfer.rs`
3. Check implementation: `IMPLEMENTATION_SUMMARY.md`
4. Run tests: `cargo test transfer`

## Quick Reference Card

```
COMMAND                              DESCRIPTION
───────────────────────────────────  ──────────────────────────────
transfer from <source>               Transfer patterns
transfer from <src> --preview        Preview transfer
transfer from <src> --top 0.9        Transfer top 10%
transfer from <src> --domains "x,y"  Filter by domain
transfer list <source>               List transferable patterns
transfer policy <source>             Transfer Q-learning policy
───────────────────────────────────  ──────────────────────────────

FILTERS                              DESCRIPTION
───────────────────────────────────  ──────────────────────────────
--min-score N                        Minimum pattern score
--min-success-rate F                 Minimum success rate (0-1)
--tool-types "X,Y"                   Filter by tool types
--domains "X,Y"                      Filter by keywords
--top F                              Top N percentile (0-1)
───────────────────────────────────  ──────────────────────────────

ADAPTATION                           DESCRIPTION
───────────────────────────────────  ──────────────────────────────
--adapt direct                       No modification
--adapt generalize                   Remove specifics
--adapt contextualize                Add domain context
--adapt specialize                   Filter by relevance
───────────────────────────────────  ──────────────────────────────
```

## Support

For issues or questions:
1. Check documentation: `TRANSFER_LEARNING.md`
2. Run with verbose logging: `mana -v transfer ...`
3. Check MANA status: `mana status`
4. View help: `mana transfer --help`
