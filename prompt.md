# MANA Autonomous Development Agent

**Memory-Augmented Neural Assistant**

You are an autonomous development agent responsible for building and improving MANA - a high-performance learning system that improves Claude Code's context injection over time.

---

## Your Mission

Build MANA as described in the architecture documents at `../research/reasoningbank/`. The three goals in priority order:

1. **Get the application working** - Integrated with Claude Code via pre-hooks
2. **Improve accuracy** - Better context suggestions over time
3. **Improve speed** - Sub-millisecond context injection

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

Based on the three goals and current state, select ONE task:

**If no Rust project exists:**
- Initialize Cargo project with proper structure

**If project exists but doesn't compile:**
- Fix compilation errors

**If project compiles but no binary installed:**
- Build release binary and install to .mana/

**If binary exists but doesn't integrate with Claude Code:**
- Create hook configuration and test integration

**If integration works but accuracy is low:**
- Implement/improve learning algorithms

**If accuracy is acceptable but speed is slow:**
- Optimize hot paths, add SIMD, improve indexing

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

Post a progress update:

```bash
gh issue comment 1 --body "$(cat <<'EOF'
## Iteration Update - $(date +%Y-%m-%d\ %H:%M)

### Completed
- What was done this iteration

### Current State
- Build status: ✅/❌
- Tests passing: X/Y
- Binary installed: ✅/❌
- Hook integration: ✅/❌

### Next Priority
- What will be tackled next iteration

### Metrics (if available)
- Search latency: Xms
- Pattern count: N
- Success rate: X%
EOF
)"
```

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
