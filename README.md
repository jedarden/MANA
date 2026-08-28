# MANA

<div align="center">

![Version](https://img.shields.io/badge/version-0.7.3-blue.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)
![License](https://img.shields.io/badge/license-Apache--2.0-green.svg)

**A Rust CLI and daemon that hooks into Claude Code, extracts reusable patterns from session transcripts, and injects the relevant ones back into context before a tool runs.**

[Features](#features) • [Installation](#installation) • [Quick Start](#quick-start) • [CLI Commands](#cli-commands) • [Benchmarks](#benchmarks)

</div>

---

## Overview

MANA ("Memory-Augmented Neural Assistant" — the name is historical; the current binary contains no neural model) is a pattern memory for Claude Code. It works through Claude Code's hook system:

- **Extracts patterns from sessions.** The session-end hook parses the transcript into trajectories (`src/learning/trajectory.rs`), records which tool invocations succeeded, and stores the resulting patterns in SQLite (`.mana/`).
- **Injects context before tools run.** A `PreToolUse` hook calls `mana inject --tool <tool>`, which asks the background daemon over a Unix socket for the patterns most similar to the current context and prints them for Claude. The latency budget for this path is 10 ms; `mana bench` measures it on your machine.
- **Keeps per-pattern statistics** (success rate, usage, freshness) and ranks patterns with them; `mana health prune` decays and removes low-value patterns.
- **Tracks which patterns are used together** in a co-occurrence graph with lift scores, queryable from the CLI (`mana causal ...`).
- **Analyzes failures** (`src/learning/failure_analysis.rs`) and stores reflections on what went wrong.
- **Syncs the pattern store between workspaces** over git, S3, Supabase, or a direct peer-to-peer connection, with AES-256-GCM encryption and secret redaction.

Embeddings are TF-IDF-style hashed vectors (`src/embeddings/model.rs`), not a transformer model; nearest-neighbour search uses an HNSW index from `instant-distance`, and distance calculations use `simsimd`.

---

## Features

### Core capabilities

| Feature | Description |
|---------|-------------|
| **Pattern learning** | Extracts patterns from Claude Code session transcripts at session end and stores them in SQLite with success statistics |
| **Context injection** | `PreToolUse` hook queries the daemon and prints relevant patterns before tool execution; 10 ms latency budget, checked by `mana bench` |
| **Trajectory analysis** | Parses whole conversation flows into trajectories rather than isolated commands (`src/learning/trajectory.rs`) |
| **Failure analysis** | Root-cause analysis of failed operations with stored reflections (`src/learning/failure_analysis.rs`, `src/learning/reflexion.rs`) |

### Learning modules

`src/learning/` contains implementations of nine reinforcement-learning algorithms: Q-learning, SARSA, DQN (linear function approximation), REINFORCE policy gradient, actor-critic, PPO, a decision transformer, MCTS, and a model-based agent. Each is a self-contained module with unit tests.

As of 0.7.3 only the Q-learning Q-table is used by a CLI command (`mana transfer policy` copies it between projects). Pattern ranking in the injection path uses the stored success statistics (`src/storage/ranking.rs`), not these agents. Treat the other eight modules as library code that is not yet wired into the runtime.

### Pattern co-occurrence ("causal") graph

```
Pattern A ──[Causes]──► Pattern B
    │                       │
    └──[Enables]────────────┘
```

`src/storage/causal.rs` maintains edges between patterns that appear together, with a lift score that moves up when they succeed together and down when they fail together (roughly >1.5 = synergy, <0.5 = conflict). On top of that:

- `do_intervention` reports the lift between a treatment and outcome pattern with a 95% confidence interval and any detected confounders. This is an observational estimate from the co-occurrence data, not a full do-calculus adjustment.
- `detect_confounders` looks for patterns connected to both ends of an edge and scores them by the product of their correlations (a backdoor-path heuristic).
- `find_causal_chains` finds multi-hop paths between two patterns by breadth-first search.
- Edge relation types: Causes, Enables, Prevents, Correlates, Precedes, DerivedFrom, Contradicts.

### SIMD distance calculations

`src/storage/simd_distance.rs` uses `simsimd`, which dispatches to AVX2/AVX-512 on x86 and NEON on ARM at runtime:

- Metrics: cosine, Euclidean, dot product, inner product
- Batch similarity and top-k helpers
- `mana bench simd` compares it against a naive implementation on your hardware; no speedup figures are committed in this repository

### Multi-workspace sync

| Backend | Notes |
|---------|-------|
| **Git** | Pushes an encrypted export to any git remote |
| **S3** | S3 or an S3-compatible endpoint (`MANA_S3_ENDPOINT`); requires `--features s3` |
| **Supabase** | Shared store with team commands; requires `--features supabase` |
| **P2P** | Direct TCP exchange between peers, merged as a last-writer-wins map (`src/sync/p2p_backend.rs`) |

**Security**: AES-256-GCM encryption with an Argon2id-derived key, path sanitization, and regex-based redaction of API keys and tokens before export (`src/sync/crypto.rs`, `src/sync/sanitize.rs`).

### Provenance

`src/storage/provenance.rs` records why each pattern was selected so that `mana provenance explain` can show the reasoning chain, `mana provenance justify` can explain recent actions, and `mana provenance verify` can check the record's integrity.

---

## Installation

### Prerequisites

- Rust (2021 edition)
- SQLite 3.x (bundled via `rusqlite`)

### Build from source

```bash
cd MANA
cargo build --release
```

### Optional features

```bash
# Enable S3 sync support
cargo build --release --features s3

# Enable Supabase team collaboration
cargo build --release --features supabase

# Enable all features
cargo build --release --all-features
```

---

## Quick Start

### 1. Initialize MANA

```bash
mana init
```

### 2. Start the daemon

```bash
mana daemon start
```

### 3. Check status

```bash
mana status
```

### 4. View patterns

```bash
mana patterns list
```

Hook wiring for Claude Code (`PreToolUse` → `mana inject`, `Stop` → `mana session-end`) is described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## CLI Commands

### Core commands

```bash
# Context injection (pre-hook)
mana inject --tool bash

# Process session end
mana session-end

# Manual consolidation
mana consolidate

# View statistics
mana stats
```

### Pattern management

```bash
# List all patterns
mana patterns list

# Search patterns
mana patterns search "docker build"

# Show pattern details
mana patterns show <pattern_id>

# Export/Import patterns
mana patterns export --output patterns.json
mana patterns import --input patterns.json
```

### Co-occurrence graph

```bash
# View causal graph stats
mana causal stats

# Estimate the effect of one pattern on another
mana causal intervention <treatment_id> <outcome_id>

# Find causal chains
mana causal chains <from_id> <to_id>

# Detect confounders
mana causal confounders <treatment_id> <outcome_id>
```

### Transfer learning

```bash
# Transfer patterns from another project
mana transfer from /path/to/source

# List transferable patterns
mana transfer list /path/to/source

# Copy the Q-learning table from another project
mana transfer policy /path/to/source
```

### Synchronization

```bash
# Initialize sync with a backend
mana sync init --backend git --remote git@github.com:user/patterns.git
mana sync init --backend s3 --bucket my-mana-bucket
mana sync init --backend supabase --url https://xxx.supabase.co

# Push/Pull patterns
mana sync push
mana sync pull

# Check sync status
mana sync status
```

### Team collaboration (Supabase backend)

```bash
# Create a team
mana team create "My Team"

# List teams
mana team list

# Invite members
mana team invite <team_id> user@email.com

# Share patterns with team
mana team share <team_id> <pattern_ids>
```

### Daemon control

```bash
# Start daemon (background)
mana daemon start

# Start in foreground (debug)
mana daemon start --foreground

# Stop daemon
mana daemon stop

# View daemon logs
mana daemon logs --tail
```

### Health and maintenance

```bash
# Check health status
mana health status

# Prune low-quality patterns
mana health prune
mana health prune --dry-run  # Preview only

# Relearn from scratch
mana relearn
```

### Provenance

```bash
# Explain why a pattern was selected
mana provenance explain <pattern_id>

# Show full provenance
mana provenance show <pattern_id>

# Justify recent actions
mana provenance justify <action>

# Verify integrity
mana provenance verify
```

---

## Benchmarks

### Performance targets

These are the budgets `mana bench` (`src/bench.rs`) checks against. They are targets, not measurements; no benchmark results are committed in this repository.

| Metric | Target | Description |
|--------|--------|-------------|
| **Context injection** | <10 ms | End-to-end `mana inject` including process start |
| **Pattern search** | <0.5 ms | Database query plus similarity ranking |
| **Similarity cache hit** | <10 μs | In-memory cache lookup |
| **Session-end parse** | <20 ms | Transcript parsing |
| **Binary startup** | <50 ms | Cold start |

### Running benchmarks

```bash
# Built-in latency checks against the targets above
mana bench

# SIMD vs naive distance calculations
mana bench simd

# Criterion micro-benchmarks (database, vector search, cache, serialization, end-to-end retrieval)
cargo bench
```

`benches/comprehensive.rs` covers database operations, HNSW search at several index sizes, the distance metrics, quantization, serialization, the similarity cache, and a full retrieval pipeline. Run it to get numbers for your hardware.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        MANA System                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐  │
│  │   Hooks      │    │   Daemon     │    │   Storage    │  │
│  │  ──────────  │    │  ──────────  │    │  ──────────  │  │
│  │ • Pre-hook   │───►│ • Socket     │───►│ • SQLite     │  │
│  │ • Post-hook  │    │ • Cache      │    │ • HNSW Index │  │
│  │ • Session    │    │ • Background │    │ • Embeddings │  │
│  └──────────────┘    └──────────────┘    └──────────────┘  │
│         │                   │                   │          │
│         ▼                   ▼                   ▼          │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐  │
│  │  Learning    │    │   Causal     │    │    Sync      │  │
│  │  ──────────  │    │  ──────────  │    │  ──────────  │  │
│  │ • Trajectory │◄──►│ • Lift       │◄──►│ • Git        │  │
│  │ • Failure    │    │ • Chains     │    │ • S3         │  │
│  │ • Reflexion  │    │ • Confound   │    │ • Supabase   │  │
│  │ • Transfer   │    │ • Intervene  │    │ • P2P        │  │
│  └──────────────┘    └──────────────┘    └──────────────┘  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Directory structure

```
MANA/
├── src/
│   ├── main.rs              # CLI interface
│   ├── daemon/              # Background service (Unix socket)
│   ├── embeddings/          # Hashed TF-IDF embeddings, HNSW index, quantization
│   ├── hooks/               # Claude Code integration
│   ├── learning/            # Trajectories, failure analysis, RL modules, transfer
│   ├── reflection/          # Pattern effectiveness
│   ├── storage/             # SQLite, co-occurrence graph, provenance, SIMD distance
│   └── sync/                # Multi-workspace sync
├── tests/                   # Integration tests
├── benches/                 # Criterion benchmarks
└── docs/                    # Documentation
```

---

## Configuration

`mana init` writes `.mana/config.toml` with these defaults:

```toml
[learning]
# Trajectory threshold before triggering learning
threshold = 15
# Maximum patterns to inject per context
max_patterns_per_context = 5

[performance]
# Maximum time for context injection in milliseconds
injection_timeout_ms = 10
# Maximum time for pattern search in milliseconds
search_timeout_ms = 5

[storage]
# Maximum number of patterns to keep
max_patterns = 10000
# Decay factor for unused patterns (0-1)
decay_factor = 0.95
```

As of 0.7.3 the binary writes this file but does not read these values back; the corresponding limits are compiled in. Sync backend settings are stored in their own files under `.mana/` by `mana sync init`.

---

## Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design and hook wiring |
| [CAUSAL_SYSTEM_SUMMARY.md](docs/CAUSAL_SYSTEM_SUMMARY.md) | Co-occurrence graph overview |
| [TRANSFER_QUICKSTART.md](docs/TRANSFER_QUICKSTART.md) | Transfer learning guide |
| [HEALTH_MONITORING.md](docs/HEALTH_MONITORING.md) | Health and pruning |
| [SIMD_INTEGRATION.md](docs/SIMD_INTEGRATION.md) | SIMD distance calculations |
| [PROVENANCE_IMPLEMENTATION.md](docs/PROVENANCE_IMPLEMENTATION.md) | Provenance system |
| [CHANGELOG.md](docs/CHANGELOG.md) | Version history |

---

## Development

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# With all features
cargo build --release --all-features
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test causal

# Run with output
cargo test -- --nocapture
```

### Benchmarking

```bash
# Run Criterion benchmarks
cargo bench

# Run built-in SIMD benchmark
cargo run --release -- bench simd
```

### Release profile

```toml
[profile.release]
lto = true          # Link-time optimization
codegen-units = 1   # Better optimization
panic = "abort"     # Smaller binary
strip = true        # Strip symbols
```

---

## License

Apache License 2.0 — see [LICENSE](LICENSE) for details.

---

## Acknowledgments

- **Claude Code** by Anthropic for the hook system
- **instant-distance** for the HNSW implementation
- **simsimd** for SIMD distance calculations
- **rusqlite** for embedded SQLite
- Pearl's causal inference work for the graph terminology

---

<div align="center">

[Report Bug](https://github.com/jedarden/MANA/issues) • [Request Feature](https://github.com/jedarden/MANA/issues)

</div>

---

Part of [jedarden.com](https://jedarden.com) · Read the write-up: [jedarden.com/projects/mana/](https://jedarden.com/projects/mana/)

*This GitHub repo is a read-only mirror of git.ardenone.com/jedarden/MANA — issues and PRs are welcome here either way.*
