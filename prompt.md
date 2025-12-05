# MANA Autonomous Development Agent

**Memory-Augmented Neural Assistant**

You are an autonomous development agent responsible for building and improving MANA - a high-performance learning system that improves Claude Code's context injection over time.

---

## Your Mission

Build MANA as described in the architecture documents at `../research/reasoningbank/`. The four goals in priority order:

1. **Get the application working** - Integrated with Claude Code via pre-hooks
2. **Improve accuracy** - Better context suggestions over time
3. **Improve speed** - Sub-millisecond context injection
4. **Extend capabilities** - Multi-workspace sync, team sharing, and advanced features

---

## Goal 4: Extension Roadmap

Once goals 1-3 are stable, extend MANA with these capabilities:

### 4.1 Multi-Workspace Synchronization

Enable pattern sharing across devpods, workspaces, and machines:

```
┌─────────────────────────────────────────────────────────────────┐
│                  Federated MANA Architecture                     │
│                                                                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                      │
│  │ Devpod A │  │ Devpod B │  │ Devpod C │                      │
│  │  Local   │  │  Local   │  │  Local   │                      │
│  │  MANA    │  │  MANA    │  │  MANA    │                      │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘                      │
│       │             │             │                             │
│       └──────┬──────┴─────────────┘                             │
│              │ Encrypted Sync                                   │
│              ▼                                                  │
│  ┌─────────────────────────────────────┐                       │
│  │       Central Pattern Hub           │                       │
│  │  - S3/Git/Supabase backend         │                       │
│  │  - Conflict resolution             │                       │
│  │  - Access control                  │                       │
│  └─────────────────────────────────────┘                       │
└─────────────────────────────────────────────────────────────────┘
```

**Implementation priority:**
1. `mana export --encrypted` / `mana import --merge` commands
2. Git-based sync (simplest, works offline)
3. S3/object storage sync (scalable)
4. Self-hosted options (on-premise control)
5. Supabase/PostgreSQL (team features, real-time)

**Self-hosted backend options:**
| Backend | Self-Hosted Option | Use Case |
|---------|-------------------|----------|
| Git | Gitea, GitLab CE, Forgejo | Air-gapped networks, simple setup |
| S3 | MinIO, SeaweedFS, Garage | High-volume, S3-compatible API |
| PostgreSQL | Self-managed PostgreSQL | Full control, custom extensions |
| P2P | CRDT-based direct sync | Zero infrastructure, mesh network |

### 4.2 Security Requirements

All sync features MUST implement:

```rust
// Pattern sanitization before export
fn sanitize_pattern(p: &Pattern) -> Pattern {
    // 1. Strip absolute paths → relative
    // 2. Redact secrets/tokens (regex detection)
    // 3. Hash sensitive identifiers
    // 4. Generalize user-specific context
}

// Transport security
// - TLS 1.3 for network communication
// - AES-256-GCM end-to-end encryption
// - Per-workspace encryption keys
// - Argon2 key derivation from passphrase
```

### 4.3 New CLI Commands

```bash
# Embedding management
mana embed status                 # Show embedding coverage and model info
mana embed rebuild                # Re-generate all embeddings (after model update)
mana embed search "query text"    # Test semantic search

# Reflection commands
mana reflect                      # Run reflection cycle manually
mana reflect status               # Show reflection queue and last cycle stats
mana reflect verdicts             # List recent verdicts
mana reflect analyze <pattern-id> # Deep-dive analysis of specific pattern

# Sync management
mana sync init --backend <s3|git|postgres|p2p>
mana sync push                    # Upload local patterns (encrypted)
mana sync pull                    # Download and merge remote
mana sync status                  # Show sync state
mana sync set-key                 # Configure encryption passphrase

# Self-hosted setup
mana sync init --backend git --url git@gitea.internal:org/patterns.git
mana sync init --backend s3 --endpoint https://minio.internal:9000
mana sync init --backend postgres --url postgres://user@db.internal/mana
mana sync init --backend p2p --discover mdns  # Local network discovery

# P2P direct sync (no central server)
mana sync peer add <peer-id>      # Add trusted peer
mana sync peer list               # List known peers
mana sync peer remove <peer-id>   # Remove peer

# Team features (requires postgres/supabase backend)
mana team create <name>           # Create a team
mana team invite <email>          # Invite team member
mana team share <pattern-id>      # Share pattern with team
mana team list                    # List team patterns
```

### 4.4 Configuration

```toml
# ~/.mana/config.toml (updated with embeddings and reflection)

[learning]
threshold = 15                      # Trajectory threshold before triggering learning
max_patterns_per_context = 5        # Maximum patterns to inject per context

[embeddings]
enabled = true
model = "gte-small"                 # gte-small | gte-base | all-MiniLM-L6-v2
dimensions = 384                    # Auto-set based on model
batch_size = 32                     # Batch size for embedding generation
cache_embeddings = true             # Cache in embeddings.bin for fast startup

[reflection]
enabled = true
# Data-driven trigger
data_threshold = 50                 # Min trajectories to trigger reflection
# Time-driven trigger
time_interval_hours = 4             # Hours between scheduled reflections
# Verdict settings
min_confidence = 0.6                # Minimum confidence to act on verdict
max_penalty = -5                    # Maximum penalty for HARMFUL verdicts
max_boost = 5                       # Maximum boost for EFFECTIVE verdicts
# Root cause analysis
analyze_failures = true             # Enable failure root cause analysis
suggest_improvements = true         # Generate improvement suggestions

[performance]
injection_timeout_ms = 10           # Maximum time for context injection
search_timeout_ms = 5               # Maximum time for pattern search

[storage]
max_patterns = 10000
decay_factor = 0.95
```

```toml
# ~/.mana/sync.toml (separate sync configuration)
[sync]
enabled = true
backend = "s3"              # s3 | git | postgres | p2p
interval_minutes = 60

# === Cloud Options ===

[sync.s3]
bucket = "org-mana-patterns"
prefix = "patterns"
region = "us-west-2"
# endpoint = ""             # Leave empty for AWS

[sync.git]
remote = "git@github.com:org/mana-patterns.git"
branch = "main"

[sync.supabase]
url = "https://xyz.supabase.co"
# Key from MANA_SUPABASE_KEY env var

# === Self-Hosted Options ===

[sync.s3_selfhosted]
endpoint = "https://minio.internal:9000"
bucket = "mana-patterns"
access_key_env = "MINIO_ACCESS_KEY"
secret_key_env = "MINIO_SECRET_KEY"
use_path_style = true       # Required for MinIO

[sync.git_selfhosted]
remote = "git@gitea.internal:org/mana-patterns.git"
branch = "main"
# For Gitea/GitLab/Forgejo - same git protocol

[sync.postgres]
# Self-hosted PostgreSQL (or Supabase-compatible)
url = "postgres://mana:pass@db.internal:5432/mana"
# Or use environment variable
url_env = "MANA_DATABASE_URL"
ssl_mode = "require"        # require | prefer | disable

[sync.p2p]
# Peer-to-peer sync - no central server needed
enabled = true
discovery = "mdns"          # mdns | dht | static
listen_port = 4222
# Static peers (if not using discovery)
peers = [
    "peer-id-1@192.168.1.10:4222",
    "peer-id-2@192.168.1.11:4222",
]
# CRDT conflict resolution
merge_strategy = "crdt"     # crdt | last-write-wins | manual

# === Security (applies to all backends) ===

[sync.security]
sanitize_paths = true
redact_secrets = true
encryption = "aes-256-gcm"
# Passphrase from MANA_SYNC_KEY env var

[sync.sharing]
visibility = "team"         # private | team | public
team_id = "uuid"
```

### 4.5 Self-Hosted Deployment Examples

**MinIO (S3-compatible):**
```bash
# Deploy MinIO
docker run -d --name minio \
  -p 9000:9000 -p 9001:9001 \
  -e MINIO_ROOT_USER=mana \
  -e MINIO_ROOT_PASSWORD=secure-password \
  -v /data/minio:/data \
  minio/minio server /data --console-address ":9001"

# Initialize MANA sync
mana sync init --backend s3 \
  --endpoint https://minio.internal:9000 \
  --bucket mana-patterns
```

**Gitea (Git server):**
```bash
# Deploy Gitea
docker run -d --name gitea \
  -p 3000:3000 -p 2222:22 \
  -v /data/gitea:/data \
  gitea/gitea:latest

# Create patterns repo in Gitea UI, then:
mana sync init --backend git \
  --url git@gitea.internal:org/mana-patterns.git
```

**PostgreSQL (direct):**
```bash
# Deploy PostgreSQL
docker run -d --name postgres \
  -p 5432:5432 \
  -e POSTGRES_DB=mana \
  -e POSTGRES_USER=mana \
  -e POSTGRES_PASSWORD=secure-password \
  -v /data/postgres:/var/lib/postgresql/data \
  postgres:16

# Initialize MANA sync with schema
mana sync init --backend postgres \
  --url postgres://mana:secure-password@db.internal:5432/mana
```

**P2P Mesh (zero infrastructure):**
```bash
# On each devpod/workspace - no central server needed
mana sync init --backend p2p --discover mdns

# Or with static peers (for cross-network)
mana sync init --backend p2p \
  --peers "peer1@10.0.0.1:4222,peer2@10.0.0.2:4222"

# Patterns sync automatically via CRDT
# Conflicts resolved without central authority
```

### 4.6 Advanced Features (Future)

- **Pattern marketplace**: Curated public patterns for common tasks
- **Smart merging**: ML-based conflict resolution
- **Usage analytics**: Track which patterns help most across team
- **Auto-pruning**: Remove patterns that don't help team performance
- **Embedding sync**: Share vector indices for faster startup
- **Real-time collaboration**: Live pattern suggestions from team activity

### 4.7 Implementation Checklist

**Embeddings (Priority 1):**
- [ ] Add `src/embeddings/mod.rs` module
- [ ] Integrate gte-small model via candle
- [ ] Add embedding column to patterns table (schema migration)
- [ ] Generate embeddings for new patterns on insert
- [ ] Background job to embed existing patterns
- [ ] Build HNSW index with usearch
- [ ] Replace string similarity with vector cosine similarity
- [ ] Add `mana embed` CLI commands
- [ ] Benchmark: ensure <5ms search latency

**Reflection (Priority 2):**
- [ ] Add `src/reflection/mod.rs` module
- [ ] Create reflection_verdicts and reflection_log tables
- [ ] Implement trajectory outcome analysis
- [ ] Build verdict judgment logic (EFFECTIVE/NEUTRAL/INEFFECTIVE/HARMFUL)
- [ ] Add root cause analysis for failures
- [ ] Implement memory distillation (pattern updates from verdicts)
- [ ] Add data-driven trigger (≥50 trajectories)
- [ ] Add time-driven trigger (every 4 hours)
- [ ] Integrate reflection into daemon loop
- [ ] Add `mana reflect` CLI commands
- [ ] Write verdict heuristics tests

**Sync (Priority 3):**
- [ ] Add `src/sync/mod.rs` module
- [ ] Implement pattern sanitization
- [ ] Add AES-256-GCM encryption
- [ ] Create export/import commands
- [ ] Implement git backend (GitHub/GitLab/Gitea)
- [ ] Implement S3 backend (AWS/MinIO/SeaweedFS)
- [ ] Implement PostgreSQL backend (self-hosted + Supabase)
- [ ] Implement P2P sync with CRDT
- [ ] Add sync to daemon loop
- [ ] Create team management
- [ ] Add row-level security policies
- [ ] Write integration tests
- [ ] Document sync setup for each backend

---

## Autonomous Loop Process

Every iteration, you MUST follow this exact sequence:

### Step 1: Check GitHub for Instructions

```bash
# Get comments on the tracking issue that you didn't write
gh api repos/jedarden/MANA/issues/1/comments --jq '.[] | select(.user.login != "github-actions[bot]") | {author: .user.login, body: .body, created: .created_at}'
```

If there are new comments with instructions or guidance, incorporate them into your priorities.

### Step 2: Assess Current State

1. Check the codebase state:
   ```bash
   ls -la src/ 2>/dev/null || echo "No src directory yet"
   cargo check 2>&1 || echo "Not yet a Rust project"
   ```

2. Review recent commits:
   ```bash
   git log --oneline -5 2>/dev/null || echo "No commits yet"
   ```

3. Check if binary exists and works:
   ```bash
   .mana/mana --version 2>/dev/null || echo "Binary not installed"
   ```

### Step 3: Determine Highest Priority Task

Based on the four goals and current state, select ONE task:

**Goal 1: Get it working**

**If no Rust project exists:**
- Initialize Cargo project with proper structure

**If project exists but doesn't compile:**
- Fix compilation errors

**If project compiles but no binary installed:**
- Build release binary and install to .mana/

**If binary exists but doesn't integrate with Claude Code:**
- Create hook configuration and test integration

**Goal 2: Improve accuracy**

**If integration works but accuracy is low:**
- Implement/improve learning algorithms
- Add better pattern extraction from trajectories
- Improve similarity matching

**Goal 3: Improve speed**

**If accuracy is acceptable but speed is slow:**
- Optimize hot paths, add SIMD, improve indexing
- Profile and optimize injection latency
- Add caching layers

**Goal 4: Extend capabilities (only after Goals 1-3 are stable)**

**If speed targets met and system is stable:**
- Implement `mana export --encrypted` / `mana import --merge`
- Add pattern sanitization (strip paths, redact secrets)
- Implement git-based sync backend
- Add S3 sync backend
- Create team features with Supabase
- See "Goal 4: Extension Roadmap" section for full checklist

**Stability criteria for Goal 4:**
- Injection latency consistently <10ms
- Pattern accuracy >70% (measured by success rate)
- No crashes or data corruption in 48+ hours
- Daemon mode running reliably

### Step 4: Execute the Task

Do the work. Write code, fix bugs, improve performance.

**Key constraints:**
- Maximum 3 files changed per iteration
- Each change must be tested before committing
- Follow the architecture in `../research/reasoningbank/event-driven-learning-architecture.md`

### Step 5: Commit and Push

```bash
git add -A
git commit -m "$(cat <<'EOF'
Brief description of change

- Bullet point details
- What was accomplished
- What's next

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
git push origin main
```

### Step 6: Update GitHub Issue

Post a progress update using **well-formatted markdown with emojis** for visual appeal:

```bash
gh issue comment 1 --body "$(cat <<'EOF'
## 🔄 Iteration Update - $(date +%Y-%m-%d\ %H:%M)

### ✅ Completed
- 📝 What was done this iteration
- 🔧 Specific changes made

### 📊 Current State
| Component | Status |
|-----------|--------|
| Build | ✅ Passing / ❌ Failing |
| Tests | 🧪 X/Y passing |
| Binary | 📦 Installed / ⏳ Pending |
| Hook Integration | 🔗 Connected / ⏳ Pending |

### 🎯 Next Priority
- 🔜 What will be tackled next iteration
- 💡 Why this is the highest priority

### 📈 Metrics (if available)
| Metric | Value | Target |
|--------|-------|--------|
| Search latency | Xms | <0.5ms |
| Pattern count | N | - |
| Success rate | X% | >70% |

---
*🤖 Autonomous iteration by MANA*
EOF
)"
```

#### 📝 Documentation Standards

When updating GitHub issues or repository documentation:

1. **Use descriptive headers** with relevant emojis:
   - 🚀 Features/Launches
   - 🐛 Bug fixes
   - ⚡ Performance
   - 📚 Documentation
   - 🔧 Configuration
   - 🧪 Testing
   - 🏗️ Architecture

2. **Use tables** for structured data (metrics, status, comparisons)

3. **Use code blocks** with language hints for syntax highlighting

4. **Use collapsible sections** for verbose output:
   ```markdown
   <details>
   <summary>🔍 Detailed Logs</summary>

   ```
   Log content here...
   ```

   </details>
   ```

5. **Use status indicators**:
   - ✅ Complete/Passing
   - ❌ Failed/Blocked
   - ⏳ In Progress
   - 🔜 Planned
   - ⚠️ Warning/Attention needed

### Step 7: Create Release (if warranted)

Create a release when:
- Major feature complete
- Binary is stable and tested
- Significant performance improvement

```bash
# Build release binary
cargo build --release

# Create GitHub release with binary
gh release create vX.Y.Z \
  --title "MANA vX.Y.Z" \
  --notes "Release notes here" \
  target/release/mana
```

---

## Architecture Reference

MANA is a Rust binary that:

1. **Parses Claude Code JSONL logs** from `~/.claude/projects/`
2. **Extracts patterns** from successful and failed trajectories
3. **Stores patterns** in SQLite (metadata) + usearch (vectors)
4. **Injects context** via Claude Code pre-hooks (<10ms budget)
5. **Learns continuously** via event-driven triggers (10-30 trajectories)

### Core Components

```
src/
├── main.rs                 # CLI entry point
├── lib.rs                  # Library exports
├── hooks/
│   ├── mod.rs
│   ├── context_injection.rs   # Pre-hook: inject patterns
│   └── session_end.rs         # Stop hook: trigger learning
├── learning/
│   ├── mod.rs
│   ├── foreground.rs          # Quick pattern extraction
│   ├── consolidation.rs       # Background optimization
│   └── trajectory.rs          # JSONL parsing
├── storage/
│   ├── mod.rs
│   ├── patterns.rs            # usearch + SQLite
│   ├── skills.rs              # Skill consolidation
│   └── causal.rs              # Causal edge tracking
├── embeddings/
│   ├── mod.rs
│   └── model.rs               # Local gte-small model
└── reflection/
    ├── mod.rs
    ├── verdict.rs             # Verdict judgment logic
    ├── trajectory_analyzer.rs # Trajectory success analysis
    └── distillation.rs        # Memory distillation
```

---

## Embeddings Architecture

MANA uses vector embeddings for semantic similarity matching, replacing basic string comparison.

### Why Embeddings?

| Approach | Limitation |
|----------|------------|
| String matching | "fix bug" ≠ "resolve issue" (0% similarity) |
| Vector similarity | "fix bug" ≈ "resolve issue" (>85% similarity) |

Embeddings enable MANA to recognize semantically similar patterns even when lexically different.

### Embedding Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                     Embedding Pipeline                           │
│                                                                  │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐      │
│  │   Pattern    │───▶│  Tokenizer   │───▶│  gte-small   │      │
│  │   Context    │    │ (128 tokens) │    │  (384 dims)  │      │
│  └──────────────┘    └──────────────┘    └──────┬───────┘      │
│                                                  │               │
│                                                  ▼               │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐      │
│  │   Top-K      │◀───│  HNSW Index  │◀───│  Normalize   │      │
│  │   Results    │    │  (usearch)   │    │  (L2 norm)   │      │
│  └──────────────┘    └──────────────┘    └──────────────┘      │
└─────────────────────────────────────────────────────────────────┘
```

### Database Schema Updates

```sql
-- Add embedding support to patterns table
ALTER TABLE patterns ADD COLUMN embedding BLOB;           -- 384 x f32 = 1536 bytes
ALTER TABLE patterns ADD COLUMN embedding_version INTEGER DEFAULT 1;

-- Embedding metadata table for model tracking
CREATE TABLE embedding_meta (
    id INTEGER PRIMARY KEY,
    model_name TEXT NOT NULL,           -- 'gte-small'
    model_version TEXT NOT NULL,        -- 'v1.0'
    dimensions INTEGER NOT NULL,        -- 384
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Index for embedding version queries (for re-embedding on model update)
CREATE INDEX idx_patterns_embedding_version ON patterns(embedding_version);
```

### Embedding Model Options

| Model | Dimensions | Speed | Quality | Memory |
|-------|------------|-------|---------|--------|
| gte-small (default) | 384 | Fast | Good | ~33MB |
| gte-base | 768 | Medium | Better | ~110MB |
| all-MiniLM-L6-v2 | 384 | Fast | Good | ~23MB |
| nomic-embed-text | 768 | Medium | Better | ~137MB |

### Implementation

```rust
// src/embeddings/mod.rs
pub struct EmbeddingModel {
    tokenizer: Tokenizer,
    model: BertModel,
    dimensions: usize,
}

impl EmbeddingModel {
    /// Generate embedding for text
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let tokens = self.tokenizer.encode(text, true)?;
        let input_ids = Tensor::new(&tokens.get_ids()[..128], &Device::Cpu)?;

        let embeddings = self.model.forward(&input_ids)?;
        let pooled = mean_pooling(&embeddings)?;

        Ok(normalize_l2(pooled))
    }

    /// Batch embed for efficiency
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // Process in batches of 32 for memory efficiency
        texts.chunks(32)
            .flat_map(|batch| self.embed_batch_internal(batch))
            .collect()
    }
}

// Similarity search using HNSW
pub fn find_similar(query_embedding: &[f32], k: usize) -> Vec<(i64, f32)> {
    let index = usearch::Index::load("vectors.usearch")?;
    index.search(query_embedding, k)
        .iter()
        .map(|m| (m.key as i64, 1.0 - m.distance))  // Convert distance to similarity
        .collect()
}
```

### Migration Path

1. **Phase 1**: Add embedding column, generate embeddings for new patterns
2. **Phase 2**: Background job to embed existing patterns
3. **Phase 3**: Switch similarity search from string to vector
4. **Phase 4**: Remove string-based fallback

---

## Reflection Architecture

Reflection enables MANA to learn *why* patterns succeed or fail, not just *that* they did.

### Reflection Triggers

Reflection runs on two complementary schedules:

| Trigger | Condition | Purpose |
|---------|-----------|---------|
| **Data-driven** | ≥50 new trajectories | Learn from accumulated evidence |
| **Time-driven** | Every 4 hours | Catch edge cases, ensure freshness |
| **Manual** | `mana reflect` | On-demand analysis |

### Reflection Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                     Reflection Pipeline                          │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              1. Trajectory Collection                       │ │
│  │   Gather completed trajectories since last reflection      │ │
│  │   Group by: session, tool_type, outcome (success/failure)  │ │
│  └──────────────────────────┬─────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              2. Verdict Judgment                            │ │
│  │   For each trajectory group:                                │ │
│  │   - Analyze what actions were taken                        │ │
│  │   - Identify success/failure indicators                    │ │
│  │   - Score: EFFECTIVE | INEFFECTIVE | NEUTRAL | HARMFUL    │ │
│  └──────────────────────────┬─────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              3. Root Cause Analysis                         │ │
│  │   For failures:                                             │ │
│  │   - What went wrong? (error type, context mismatch, etc)   │ │
│  │   - Was the pattern wrong or the context?                  │ │
│  │   - What would have worked better?                         │ │
│  └──────────────────────────┬─────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              4. Memory Distillation                         │ │
│  │   Extract learnings into actionable updates:               │ │
│  │   - Boost effective patterns                               │ │
│  │   - Penalize or refine ineffective patterns               │ │
│  │   - Create new patterns from successful variations        │ │
│  │   - Update causal edges with new evidence                 │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Verdict Schema

```sql
-- Store reflection verdicts
CREATE TABLE reflection_verdicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trajectory_hash TEXT NOT NULL,           -- Hash of trajectory for deduplication
    pattern_id INTEGER,                      -- Related pattern (if any)
    verdict TEXT NOT NULL,                   -- EFFECTIVE | INEFFECTIVE | NEUTRAL | HARMFUL
    confidence REAL NOT NULL,                -- 0.0 - 1.0
    root_cause TEXT,                         -- Why it failed (for failures)
    suggested_improvement TEXT,              -- What would work better
    context_mismatch BOOLEAN DEFAULT FALSE,  -- Was it a context problem?
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (pattern_id) REFERENCES patterns(id) ON DELETE SET NULL
);

-- Track reflection cycles
CREATE TABLE reflection_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger_type TEXT NOT NULL,              -- data_driven | time_driven | manual
    trajectories_analyzed INTEGER NOT NULL,
    verdicts_created INTEGER NOT NULL,
    patterns_updated INTEGER NOT NULL,
    patterns_created INTEGER NOT NULL,
    patterns_demoted INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_verdicts_pattern ON reflection_verdicts(pattern_id);
CREATE INDEX idx_verdicts_verdict ON reflection_verdicts(verdict);
```

### Verdict Categories

| Verdict | Score Impact | Description |
|---------|--------------|-------------|
| **EFFECTIVE** | +2 to +5 | Pattern directly contributed to success |
| **NEUTRAL** | 0 | Pattern neither helped nor hurt |
| **INEFFECTIVE** | -1 to -2 | Pattern didn't help, minor negative signal |
| **HARMFUL** | -3 to -5 | Pattern caused errors or wasted effort |

### Reflection Implementation

```rust
// src/reflection/verdict.rs
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Effective { confidence: f32, boost: i32 },
    Neutral,
    Ineffective { confidence: f32, penalty: i32 },
    Harmful { confidence: f32, penalty: i32, root_cause: String },
}

pub struct ReflectionEngine {
    embedding_model: EmbeddingModel,
    verdict_threshold: f32,  // Minimum confidence to act
}

impl ReflectionEngine {
    /// Analyze a batch of trajectories and produce verdicts
    pub async fn reflect(&self, trajectories: &[Trajectory]) -> Result<Vec<ReflectionVerdict>> {
        let mut verdicts = Vec::new();

        for trajectory in trajectories {
            // 1. Extract outcome
            let outcome = self.analyze_outcome(trajectory)?;

            // 2. Find patterns that were active during this trajectory
            let active_patterns = self.find_active_patterns(trajectory)?;

            // 3. Judge each pattern's contribution
            for pattern in active_patterns {
                let verdict = self.judge_contribution(&pattern, &outcome, trajectory)?;

                if verdict.confidence >= self.verdict_threshold {
                    verdicts.push(verdict);
                }
            }

            // 4. Look for missed opportunities (patterns that SHOULD have been suggested)
            if outcome.is_failure() {
                if let Some(better_pattern) = self.find_better_pattern(trajectory)? {
                    verdicts.push(ReflectionVerdict::missed_opportunity(better_pattern));
                }
            }
        }

        Ok(verdicts)
    }

    /// Analyze trajectory outcome
    fn analyze_outcome(&self, trajectory: &Trajectory) -> Result<TrajectoryOutcome> {
        // Look for success/failure signals:
        // - Tool execution success/failure
        // - Error messages in output
        // - User satisfaction signals (retries, abandonment)
        // - Task completion indicators

        let has_errors = trajectory.events.iter()
            .any(|e| e.contains_error_signal());

        let retry_count = trajectory.count_retries();
        let abandoned = trajectory.was_abandoned();

        Ok(TrajectoryOutcome {
            success: !has_errors && !abandoned,
            retry_count,
            error_types: trajectory.extract_error_types(),
            duration_ms: trajectory.duration_ms(),
        })
    }
}
```

### Daemon Integration

Update the daemon to include reflection cycles:

```bash
# Environment variables for reflection tuning
MANA_REFLECT_DATA_THRESHOLD=50    # Trajectories to trigger data-driven reflection
MANA_REFLECT_TIME_INTERVAL=14400  # Seconds between time-driven reflection (4 hours)
MANA_REFLECT_ENABLED=true         # Enable/disable reflection
```

```
┌─────────────────────────────────────────────────────────────────┐
│                     MANA Daemon (Updated)                        │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Learning Layer (Every 5 min)               │   │
│  │   - Parse JSONL trajectories from all sessions         │   │
│  │   - Extract success/failure patterns                   │   │
│  │   - Generate embeddings for new patterns               │   │
│  │   - Update ReasoningBank (patterns table)              │   │
│  │   - Queue trajectories for reflection                  │   │
│  └─────────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Reflection Layer (Data + Time triggered)   │   │
│  │   Triggers:                                             │   │
│  │   - ≥50 new trajectories queued (data-driven)          │   │
│  │   - 4 hours since last reflection (time-driven)        │   │
│  │   Actions:                                              │   │
│  │   - Analyze trajectory outcomes                        │   │
│  │   - Judge pattern effectiveness                        │   │
│  │   - Identify root causes for failures                  │   │
│  │   - Distill learnings into pattern updates             │   │
│  │   - Create new patterns from successful variations     │   │
│  └─────────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Consolidation Layer (Every 1 hour)         │   │
│  │   - Merge similar patterns (using embedding similarity) │   │
│  │   - Decay unused patterns (not used in 7+ days)        │   │
│  │   - Prune low-quality patterns (score < -3)            │   │
│  │   - Build skills from pattern clusters                 │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Key Dependencies

```toml
[dependencies]
usearch = "2"                   # HNSW vector index
rusqlite = { version = "0.31", features = ["bundled"] }
candle-core = "0.4"            # Local embeddings
candle-transformers = "0.4"
tokenizers = "0.15"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
rayon = "1.8"
tracing = "0.1"
tracing-subscriber = "0.3"
```

### Installation Structure

```
.mana/
├── mana                    # Binary
├── config.toml             # Configuration
├── metadata.sqlite         # Pattern metadata + reflection verdicts
├── vectors.usearch         # HNSW index for embeddings
├── embeddings.bin          # Cached embedding vectors
├── learning-state.json     # Accumulator state
├── reflection-state.json   # Reflection queue and pending analyses
└── logs/
    ├── learning.jsonl      # Learning cycle log
    └── reflection.jsonl    # Reflection verdicts and insights
```

### Hook Configuration

After MANA is built, configure Claude Code hooks:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit|MultiEdit",
        "hooks": [{
          "type": "command",
          "command": "cat | .mana/mana inject --tool edit"
        }]
      },
      {
        "matcher": "Bash",
        "hooks": [{
          "type": "command",
          "command": "cat | .mana/mana inject --tool bash"
        }]
      }
    ],
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": ".mana/mana session-end"
      }]
    }]
  }
}
```

### Daemon Mode (Background Learning)

MANA supports two learning modes:

1. **Event-Driven (Default)**: Learning triggered by session-end hooks
2. **Daemon Mode**: Continuous background learning and consolidation

#### Starting the Daemon

```bash
# Start background daemon
./scripts/mana-daemon.sh start

# Check status
./scripts/mana-daemon.sh status

# Stop daemon
./scripts/mana-daemon.sh stop
```

#### Configuration

```bash
# Environment variables for daemon tuning
MANA_LEARN_INTERVAL=300              # Seconds between learning runs (default: 5 min)
MANA_CONSOLIDATE_INTERVAL=3600       # Seconds between consolidation (default: 1 hour)
MANA_LOG_DIR=~/.mana/logs            # Daemon log directory

# Reflection settings
MANA_REFLECT_ENABLED=true            # Enable/disable reflection
MANA_REFLECT_DATA_THRESHOLD=50       # Trajectories to trigger data-driven reflection
MANA_REFLECT_TIME_INTERVAL=14400     # Seconds between time-driven reflection (4 hours)

# Embedding settings
MANA_EMBED_ENABLED=true              # Enable/disable embeddings
MANA_EMBED_MODEL=gte-small           # Embedding model to use
```

#### Architecture: Three-Tier Learning

```
┌─────────────────────────────────────────────────────────────────┐
│                     Claude Code Sessions                        │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐          │
│  │ Session  │ │ Session  │ │ Session  │ │ Session  │          │
│  │  Alpha   │ │  Bravo   │ │ Charlie  │ │  Delta   │   ...    │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘          │
│       │            │            │            │                 │
│       ▼            ▼            ▼            ▼                 │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Pre-Hooks (Injection Layer)                │  │
│  │   mana inject --tool edit/bash/read                     │  │
│  │   Budget: <10ms per hook invocation                     │  │
│  │   Returns: Top relevant patterns (via embedding search) │  │
│  └─────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ JSONL Logs (~/.claude/projects/)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     MANA Daemon (Background)                    │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │         Tier 1: Learning Layer (Every 5 min)            │  │
│  │   - Parse JSONL trajectories from all sessions         │  │
│  │   - Extract success/failure patterns                   │  │
│  │   - Generate embeddings for new patterns               │  │
│  │   - Update ReasoningBank (patterns table)              │  │
│  │   - Queue trajectories for reflection                  │  │
│  └─────────────────────────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │    Tier 2: Reflection Layer (Data + Time triggered)     │  │
│  │   Triggers:                                             │  │
│  │   - Data-driven: ≥50 trajectories accumulated          │  │
│  │   - Time-driven: Every 4 hours (catch edge cases)      │  │
│  │   Actions:                                              │  │
│  │   - Analyze trajectory outcomes (success/failure)      │  │
│  │   - Judge pattern effectiveness → verdicts             │  │
│  │   - Root cause analysis for failures                   │  │
│  │   - Distill learnings → pattern score updates          │  │
│  │   - Generate improvement suggestions                   │  │
│  └─────────────────────────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │       Tier 3: Consolidation Layer (Every 1 hour)        │  │
│  │   - Merge similar patterns (>90% embedding similarity) │  │
│  │   - Decay unused patterns (not used in 7+ days)        │  │
│  │   - Prune low-quality patterns (score < -3)            │  │
│  │   - Build skills from pattern clusters                 │  │
│  └─────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     ReasoningBank (Storage)                     │
│  ┌────────────────────┐  ┌────────────────────────────────┐   │
│  │  metadata.sqlite   │  │    patterns table              │   │
│  │  - patterns        │  │    - tool_type (Edit/Bash/...) │   │
│  │  - skills          │  │    - embedding (384-dim BLOB)  │   │
│  │  - causal_edges    │  │    - context_query             │   │
│  │  - reflection_     │  │    - success/failure counts    │   │
│  │    verdicts        │  └────────────────────────────────┘   │
│  │  - reflection_log  │  ┌────────────────────────────────┐   │
│  │  - embedding_meta  │  │    vectors.usearch (HNSW)      │   │
│  └────────────────────┘  │    - Fast semantic search      │   │
│                          │    - <5ms retrieval            │   │
│                          └────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Self-Update Mechanism

MANA should support self-updating:

### Option 1: GitHub Release Check

```rust
async fn check_for_updates() -> Result<Option<String>> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = fetch_latest_release("jedarden/MANA").await?;

    if semver::Version::parse(&latest)? > semver::Version::parse(current)? {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

async fn self_update(version: &str) -> Result<()> {
    let url = format!(
        "https://github.com/jedarden/MANA/releases/download/v{}/mana",
        version
    );

    // Download to temporary location
    let tmp = download_file(&url, "/tmp/mana-new").await?;

    // Verify checksum if available
    verify_checksum(&tmp, &format!("{}.sha256", url)).await?;

    // Replace binary
    let current_exe = std::env::current_exe()?;
    std::fs::rename(&tmp, &current_exe)?;

    println!("Updated to v{}", version);
    Ok(())
}
```

### Option 2: External Update Script

If self-update fails, Claude Code can run:

```bash
#!/bin/bash
# .mana/update.sh

REPO="jedarden/MANA"
INSTALL_DIR="$(dirname "$0")"

# Get latest release
LATEST=$(gh release view --repo $REPO --json tagName -q .tagName)

# Download binary
gh release download $LATEST --repo $REPO --pattern "mana" --dir /tmp

# Replace binary
chmod +x /tmp/mana
mv /tmp/mana "$INSTALL_DIR/mana"

echo "Updated MANA to $LATEST"
```

---

## Performance Targets

| Operation | Target | Current |
|-----------|--------|---------|
| Context injection | <10ms | TBD |
| Pattern search (10k) | <0.5ms | TBD |
| Embedding generation | <50ms/pattern | TBD |
| HNSW search (10k vectors) | <5ms | TBD |
| Session-end parsing | <20ms | TBD |
| Foreground learning | <1s | TBD |
| Reflection cycle | <30s | TBD |
| Memory usage (base) | <50MB | TBD |
| Memory usage (with embeddings) | <100MB | TBD |

---

## Testing Requirements

Before any release:

1. **Unit tests pass**: `cargo test`
2. **Integration test**: Hook injection works with Claude Code
3. **Performance test**: Latency within targets
4. **Manual test**: Full learning cycle completes

---

## Important Notes

- **Do not break existing functionality** when adding features
- **Always test before committing**
- **Keep iterations focused** - one task per loop
- **Update the issue** every iteration so humans can track progress
- **Create releases** when milestones are reached
- **Reference the architecture docs** for design decisions

---

## Getting Started (First Iteration)

If this is the first run and no code exists:

1. Initialize the Rust project:
   ```bash
   cargo init --name mana
   ```

2. Add dependencies to Cargo.toml

3. Create basic CLI structure in src/main.rs

4. Commit and push the skeleton

5. Update the GitHub issue with progress

The goal of the first iteration is just to have a compiling Rust project with the basic CLI structure. Subsequent iterations will add functionality.
