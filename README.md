# 🧠 MANA - Memory-Augmented Neural Assistant

<div align="center">

![Version](https://img.shields.io/badge/version-0.7.1-blue.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)
![License](https://img.shields.io/badge/license-MIT-green.svg)

**A high-performance learning system for Claude Code that improves context injection through pattern recognition, reinforcement learning, and causal reasoning.**

[Features](#-features) • [Installation](#-installation) • [Quick Start](#-quick-start) • [CLI Commands](#-cli-commands) • [Benchmarks](#-benchmarks)

</div>

---

## 📖 Overview

MANA transforms Claude Code into a continuously learning assistant by:

- 🔍 **Extracting patterns** from interactions via hooks
- 📈 **Learning from trajectories** to understand what works
- ⚡ **Injecting context** in under 10ms for real-time assistance
- 🔄 **Adapting patterns** across sessions and projects
- 🧬 **Reasoning causally** about why patterns succeed together
- 🌐 **Synchronizing knowledge** across workspaces with encryption

---

## ✨ Features

### 🎯 Core Capabilities

| Feature | Description |
|---------|-------------|
| **Pattern Learning** | Automatically extracts and stores successful patterns from Claude Code sessions |
| **Context Injection** | Pre-hook system injects relevant patterns before tool execution (<10ms) |
| **Trajectory Analysis** | Learns from entire conversation flows, not just individual commands |
| **Failure Analysis** | Root cause analysis of failed operations with Reflexion-style learning |

### 🤖 Reinforcement Learning Suite

MANA implements **9 RL algorithms** for adaptive pattern optimization:

| Algorithm | Type | Best For |
|-----------|------|----------|
| 📊 **Q-Learning** | Off-policy TD | Classic, effective baseline |
| 🎯 **SARSA** | On-policy TD | Safer exploration |
| 🧠 **DQN** | Deep Q-Network | Function approximation |
| 🎲 **Policy Gradient** | REINFORCE | Direct policy optimization |
| 🎭 **Actor-Critic** | Value + Policy | Lower variance training |
| 🚀 **PPO** | Proximal Policy | Stable, sample efficient |
| 🔮 **Decision Transformer** | Sequence Modeling | Return-conditioned RL |
| 🌳 **MCTS** | Monte Carlo Tree Search | Simulation-based planning |
| 🏗️ **Model-Based** | Dynamics Learning | Sample efficient MPC |

### 🔬 Causal Reasoning System

```
Pattern A ──[Causes]──► Pattern B
    │                       │
    └──[Enables]────────────┘
```

- **Do-Calculus** interventions with confidence intervals
- **Confounder detection** using Pearl's backdoor criterion
- **Multi-hop causal chains** via BFS pathfinding
- **Relation types**: Causes, Enables, Prevents, Correlates, Precedes, DerivedFrom, Contradicts
- **Lift interpretation**: >1.5 = synergy, <0.5 = conflict

### ⚡ SIMD Acceleration

Leverages `simsimd` for vectorized operations:

- **4-8x speedup** on x86 (AVX2/AVX-512)
- **3-5x speedup** on ARM (NEON)
- Sub-microsecond similarity for 384-dim vectors
- Supports: Cosine, Euclidean, Dot Product, Inner Product

### 🌐 Multi-Workspace Sync

| Backend | Use Case | Features |
|---------|----------|----------|
| 📁 **Git** | Simple, offline | Gitea/GitLab compatible |
| ☁️ **S3** | Scalable | MinIO, SeaweedFS support |
| 🗄️ **Supabase** | Teams | Real-time collaboration |
| 🔗 **P2P** | Decentralized | CRDT merge, mesh network |

**Security**: AES-256-GCM encryption, Argon2 key derivation, path sanitization, secret redaction

### 🔍 Provenance & Explainability

- Track **why** patterns were selected
- Full **reasoning chains** with justification
- **Verify** provenance integrity
- Human-readable **explanations**

---

## 📦 Installation

### Prerequisites

- Rust 1.70+ (2021 edition)
- SQLite 3.x (bundled)

### Build from Source

```bash
cd mana
cargo build --release
```

### Optional Features

```bash
# Enable S3 sync support
cargo build --release --features s3

# Enable Supabase team collaboration
cargo build --release --features supabase

# Enable all features
cargo build --release --all-features
```

---

## 🚀 Quick Start

### 1️⃣ Initialize MANA

```bash
mana init
```

### 2️⃣ Start the Daemon

```bash
mana daemon start
```

### 3️⃣ Check Status

```bash
mana status
```

### 4️⃣ View Patterns

```bash
mana patterns list
```

---

## 💻 CLI Commands

### 📋 Core Commands

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

### 🗂️ Pattern Management

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

### 🧬 Causal Reasoning

```bash
# View causal graph stats
mana causal stats

# Run do-calculus intervention
mana causal intervention <treatment_id> <outcome_id>

# Find causal chains
mana causal chains <from_id> <to_id>

# Detect confounders
mana causal confounders <treatment_id> <outcome_id>
```

### 🔄 Transfer Learning

```bash
# Transfer patterns from another project
mana transfer from /path/to/source

# List transferable patterns
mana transfer list /path/to/source

# Transfer RL policy
mana transfer policy /path/to/source
```

### 🌐 Synchronization

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

### 👥 Team Collaboration

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

### 🔧 Daemon Control

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

### 🏥 Health & Maintenance

```bash
# Check health status
mana health status

# Prune low-quality patterns
mana health prune
mana health prune --dry-run  # Preview only

# Relearn from scratch
mana relearn
```

### 🔍 Provenance

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

## 📊 Benchmarks

### 🎯 Performance Targets

| Metric | Target | Description |
|--------|--------|-------------|
| ⚡ **Context Injection** | <10ms | Time to inject relevant patterns |
| 🔍 **Pattern Search** | <0.5ms | HNSW approximate nearest neighbor |
| 💾 **Similarity Cache Hit** | <10μs | In-memory cache lookup |
| 📝 **Session-End Parse** | <20ms | Log parsing and analysis |
| 🚀 **Binary Startup** | <50ms | Cold start time |
| 🧮 **SIMD Vector Ops** | <1μs | 384-dimensional vectors |

### 📈 Running Benchmarks

```bash
# Run SIMD benchmark suite
mana bench simd

# Run full benchmark suite (requires criterion)
cargo bench
```

### 🔬 SIMD Benchmark Results

```
Benchmark: 384-dimensional vectors
┌────────────────────┬───────────┬──────────┐
│ Metric             │ Scalar    │ SIMD     │
├────────────────────┼───────────┼──────────┤
│ Cosine Similarity  │ 12.3 μs   │ 0.8 μs   │
│ Euclidean Distance │ 10.1 μs   │ 0.6 μs   │
│ Dot Product        │ 8.7 μs    │ 0.5 μs   │
│ Batch (1000 pairs) │ 11.2 ms   │ 1.4 ms   │
└────────────────────┴───────────┴──────────┘
Speedup: 8-15x on AVX-512
```

### 🏗️ Architecture Benchmarks

```
Context Injection Pipeline:
┌─────────────────────────────────────────────────────┐
│ 1. Socket connect .......... 0.1ms                  │
│ 2. Query parse ............. 0.2ms                  │
│ 3. Embedding lookup ........ 1.2ms                  │
│ 4. HNSW search ............. 0.4ms                  │
│ 5. Causal filtering ........ 0.8ms                  │
│ 6. Pattern ranking ......... 0.3ms                  │
│ 7. Response serialize ...... 0.2ms                  │
├─────────────────────────────────────────────────────┤
│ TOTAL ...................... 3.2ms (target: <10ms) │
└─────────────────────────────────────────────────────┘
```

---

## 🗄️ Architecture

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
│  │ • RL Suite   │◄──►│ • Do-calc    │◄──►│ • Git        │  │
│  │ • Transfer   │    │ • Chains     │    │ • S3         │  │
│  │ • Reflexion  │    │ • Lift       │    │ • Supabase   │  │
│  │ • Trajectory │    │ • Confound   │    │ • P2P/CRDT   │  │
│  └──────────────┘    └──────────────┘    └──────────────┘  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 📁 Directory Structure

```
mana/
├── src/
│   ├── main.rs              # CLI interface (2,576 lines)
│   ├── daemon/              # Background service
│   ├── embeddings/          # Vector embeddings & HNSW
│   ├── hooks/               # Claude Code integration
│   ├── learning/            # RL algorithms & transfer
│   ├── reflection/          # Pattern effectiveness
│   ├── storage/             # SQLite, causal, patterns
│   └── sync/                # Multi-workspace sync
├── tests/                   # Integration tests
├── benches/                 # Criterion benchmarks
└── docs/                    # Documentation
```

---

## ⚙️ Configuration

MANA uses TOML configuration at `.mana/config.toml`:

```toml
[general]
log_level = "info"

[daemon]
socket_path = ".mana/daemon.sock"
cache_size = 1000

[learning]
min_success_rate = 0.6
prune_threshold = 0.3

[sync]
backend = "git"
auto_sync = true
encrypt = true

[embeddings]
model = "minilm"
dimensions = 384
```

---

## 📚 Documentation

| Document | Description |
|----------|-------------|
| [CAUSAL_SYSTEM_SUMMARY.md](docs/CAUSAL_SYSTEM_SUMMARY.md) | Causal reasoning overview |
| [TRANSFER_QUICKSTART.md](docs/TRANSFER_QUICKSTART.md) | Transfer learning guide |
| [HEALTH_MONITORING.md](docs/HEALTH_MONITORING.md) | Health & pruning |
| [SIMD_INTEGRATION.md](docs/SIMD_INTEGRATION.md) | SIMD acceleration |
| [PROVENANCE_IMPLEMENTATION.md](docs/PROVENANCE_IMPLEMENTATION.md) | Explainability system |
| [CHANGELOG.md](CHANGELOG.md) | Version history |

---

## 🔧 Development

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

### Release Profile

```toml
[profile.release]
lto = true          # Link-time optimization
codegen-units = 1   # Better optimization
panic = "abort"     # Smaller binary
strip = true        # Strip symbols
```

---

## 📜 License

MIT License - see [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments

- **Claude Code** by Anthropic for the integration hooks
- **instant-distance** for HNSW implementation
- **simsimd** for SIMD-accelerated vector operations
- **rusqlite** for embedded SQLite
- **Pearl's causal inference** framework for reasoning foundations

---

<div align="center">

**Built with 🦀 Rust for maximum performance**

[Report Bug](https://github.com/jedarden/MANA/issues) • [Request Feature](https://github.com/jedarden/MANA/issues)

</div>
