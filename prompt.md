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
4. Supabase/PostgreSQL (team features, real-time)

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
# Sync management
mana sync init --backend <s3|git|supabase>
mana sync push                    # Upload local patterns (encrypted)
mana sync pull                    # Download and merge remote
mana sync status                  # Show sync state
mana sync set-key                 # Configure encryption passphrase

# Team features (requires Supabase backend)
mana team create <name>           # Create a team
mana team invite <email>          # Invite team member
mana team share <pattern-id>      # Share pattern with team
mana team list                    # List team patterns
```

### 4.4 Configuration

```toml
# ~/.mana/sync.toml
[sync]
enabled = true
backend = "s3"              # s3 | git | supabase
interval_minutes = 60

[sync.s3]
bucket = "org-mana-patterns"
prefix = "patterns"
region = "us-west-2"

[sync.git]
remote = "git@github.com:org/mana-patterns.git"
branch = "main"

[sync.supabase]
url = "https://xyz.supabase.co"
# Key from MANA_SUPABASE_KEY env var

[sync.security]
sanitize_paths = true
redact_secrets = true
# Passphrase from MANA_SYNC_KEY env var

[sync.sharing]
visibility = "team"         # private | team | public
team_id = "uuid"
```

### 4.5 Advanced Features (Future)

- **Pattern marketplace**: Curated public patterns for common tasks
- **Smart merging**: ML-based conflict resolution
- **Usage analytics**: Track which patterns help most across team
- **Auto-pruning**: Remove patterns that don't help team performance
- **Embedding sync**: Share vector indices for faster startup
- **Real-time collaboration**: Live pattern suggestions from team activity

### 4.6 Implementation Checklist

- [ ] Add `src/sync/mod.rs` module
- [ ] Implement pattern sanitization
- [ ] Add AES-256-GCM encryption
- [ ] Create export/import commands
- [ ] Implement git backend
- [ ] Implement S3 backend
- [ ] Add sync to daemon loop
- [ ] Create team management (Supabase)
- [ ] Add row-level security policies
- [ ] Write integration tests
- [ ] Document sync setup

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
└── embeddings/
    ├── mod.rs
    └── model.rs               # Local gte-small model
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
├── metadata.sqlite         # Pattern metadata
├── vectors.usearch         # HNSW index
├── learning-state.json     # Accumulator state
└── logs/
    └── learning.jsonl      # Learning cycle log
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
MANA_LEARN_INTERVAL=300          # Seconds between learning runs (default: 5 min)
MANA_CONSOLIDATE_INTERVAL=3600   # Seconds between consolidation (default: 1 hour)
MANA_LOG_DIR=~/.mana/logs        # Daemon log directory
```

#### Architecture: Two-Tier Learning

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
│  │   Returns: Top relevant patterns from ReasoningBank     │  │
│  └─────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ JSONL Logs (~/.claude/projects/)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     MANA Daemon (Background)                    │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Learning Layer (Every 5 min)               │  │
│  │   - Parse JSONL trajectories from all sessions         │  │
│  │   - Extract success/failure patterns                   │  │
│  │   - Update ReasoningBank (patterns table)              │  │
│  │   - Discover causal edges between patterns             │  │
│  └─────────────────────────────────────────────────────────┘  │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │              Consolidation Layer (Every 1 hour)         │  │
│  │   - Merge similar patterns (>90% similarity)           │  │
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
│  │  - patterns table  │  │    - tool_type (Edit/Bash/...) │   │
│  │  - skills table    │  │    - command_category (rs/npm) │   │
│  │  - causal_edges    │  │    - context_query (task/appr) │   │
│  │  - learning_log    │  │    - success/failure counts    │   │
│  └────────────────────┘  └────────────────────────────────┘   │
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
| Session-end parsing | <20ms | TBD |
| Foreground learning | <1s | TBD |
| Memory usage | <50MB | TBD |

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
