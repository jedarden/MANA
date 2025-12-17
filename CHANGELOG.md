# Changelog

All notable changes to MANA will be documented in this file.

## [0.6.0] - 2025-12-17

### Added - ReasoningBank Integration in Daemon

#### Automatic Reasoning Extraction
- **Background ReasoningBank Activation**: Daemon now monitors accumulated log files and automatically activates ReasoningBank extraction when thresholds are met
- **Configurable Thresholds**:
  - `min_log_entries`: Minimum 50 log entries before activation (configurable)
  - `extraction_interval_secs`: 1 hour between extraction cycles
  - `max_chains`: Up to 1000 reasoning chains stored
- **Thinking Block Analysis**: Extracts reasoning steps from Claude's thinking blocks:
  - Thought patterns ("I'll...", "Let me...", "First,...")
  - Observations ("found...", "noticed...", "shows...")
  - Actions ("Running...", "Executing...", "Creating...")
  - Reflections ("because...", "since...", "therefore...")

#### New Daemon Commands
- `reasoning`: Manually trigger ReasoningBank extraction from logs
- `reasoning_status`: View ReasoningBank status (chain count, pending logs, activation status)

#### Querying Reasoning Chains
- **`query_reasoning(task, tool_type)`**: Query stored reasoning chains for relevant context
- Returns top 3 similar reasoning chains for injection

### Removed - External References
- Removed all references to external projects in comments and documentation
- Cleaned up benchmark output and changelog comparison tables
- Documentation now focuses on MANA's own capabilities

### Changed
- Bump version to 0.6.0
- `DaemonState` now includes `reasoning_store`, `reasoning_config`, and tracking fields
- Enhanced daemon with atomic log counting and extraction timing

## [0.5.3] - 2025-12-14

### Fixed - Daemon Reliability Improvements

This release makes the daemon auto-start mechanism completely reliable.

#### No More Zombie Processes
- **Double-fork daemonization**: Replaced `setsid --fork` with the classic Unix double-fork pattern
- **Proper process orphaning**: The daemon process is now properly orphaned and adopted by init
- **Child process reaping**: Parent waits for the intermediate child, preventing zombie accumulation

#### Race Condition Prevention
- **Lockfile mechanism**: Added `daemon.starting` lockfile to prevent multiple concurrent daemon starts
- **Atomic lock acquisition**: Uses `O_CREAT | O_EXCL` for atomic lock file creation
- **Stale lock detection**: Automatically removes lock files older than 10 seconds
- **Graceful fallback**: Processes that lose the race wait for the daemon to become available

#### Technical Details

| Issue | Impact | Fix |
|-------|--------|-----|
| Zombie processes | 50+ defunct mana processes | Double-fork daemonization |
| Multiple daemons | 3+ daemon instances running | Lockfile prevents concurrent starts |
| Race conditions | Unreliable daemon startup | Atomic lock with wait/retry |

## [0.5.2] - 2025-12-14

### Fixed - Daemon Auto-Start Reliability

This release fixes a critical issue where the daemon would fail to auto-start when invoked from certain working directories.

#### Root Cause
The `get_mana_dir()` function was using the current working directory to find the `.mana` folder. When the binary was in `/project/.mana/mana` but the working directory was `/project/mana/` (which also contained a `.mana/` folder), it would find the wrong directory.

#### Fix
- **Binary-location-first lookup**: `get_mana_dir()` now checks if the binary is inside a `.mana` directory first
- **Consistent paths**: Ensures daemon, inject, and all commands use the same `.mana` directory
- **Working directory independent**: The binary now works correctly regardless of where it's invoked from

## [0.5.1] - 2025-12-14

### Fixed - Critical Pattern Retention Bugs

This release fixes critical bugs that were causing learned patterns to be deleted prematurely.

#### Pattern Usage Tracking
- **Fixed `last_used` never being updated**: Patterns that were injected into context were not being marked as "used", causing them to be subject to decay and eventual deletion even when actively being recommended
- **Added `mark_patterns_used()`**: New function in `PatternStore` that updates the `last_used` timestamp when patterns are injected into context
- **Retention improvement**: Patterns that are actually being used for context injection will no longer decay

#### Daemon Auto-Start
- **Fixed daemon not auto-starting on inject**: The `inject` command now attempts to start the daemon automatically if it's not running
- **Added `ensure_daemon_running()`**: New function that spawns the daemon in background when needed
- **Graceful fallback**: If daemon fails to start, falls back to direct database access without error

#### Zombie Process Prevention
- **Fixed zombie processes**: Daemon spawning now uses proper daemonization with `setsid` to prevent zombie processes
- **Uses system `setsid` command**: Tries `setsid --fork` first (Linux)
- **Fallback to libc**: Falls back to `libc::setsid()` via `pre_exec` if setsid command unavailable
- **Session isolation**: Daemon runs in its own session, fully detached from parent

### Technical Details

| Bug | Impact | Fix |
|-----|--------|-----|
| `last_used` never set | 99.6% of patterns deleted over time | Now updated on every context injection |
| No daemon auto-start | Cold start on every inject (~10ms) | Daemon spawned automatically |
| Zombie processes | 19+ defunct processes accumulated | Proper setsid daemonization |

### Dependencies
- Added `libc = "0.2"` for Unix process control

## [0.5.0] - 2025-12-10

### Added - Comprehensive RL Algorithm Suite

#### Reflexion Memory (Self-Critique System)
- **Learning from Failures**: Store reflections on what went wrong and why
- **FTS Search**: Full-text search across reflection content
- **Validation Tracking**: Monitor which reflections improved outcomes
- **Confidence Scoring**: Track reliability of reflections over time

#### 8 New Reinforcement Learning Algorithms

1. **SARSA** (State-Action-Reward-State-Action)
   - On-policy TD learning using actual next action
   - Safer exploration compared to Q-learning
   - Eligibility traces support

2. **DQN** (Deep Q-Network)
   - Experience replay buffer for stable learning
   - Target network with periodic updates
   - Linear function approximation (no external ML dependencies)

3. **Policy Gradient** (REINFORCE)
   - Direct policy optimization
   - Baseline subtraction for variance reduction
   - Entropy bonus for exploration

4. **Actor-Critic**
   - Combined value (critic) and policy (actor) learning
   - N-step returns for bias/variance tradeoff
   - Lower variance than pure policy gradient

5. **PPO** (Proximal Policy Optimization)
   - Clipped surrogate objective for stability
   - Generalized Advantage Estimation (GAE)
   - Sample efficient and stable training

6. **Decision Transformer**
   - Sequence modeling approach to RL
   - Return-conditioned generation
   - Attention-based architecture simulation

7. **MCTS** (Monte Carlo Tree Search)
   - UCB1 selection for exploration/exploitation balance
   - Configurable rollout depth and simulations
   - Tree persistence across episodes
   - Fixed: Selection phase depth limiting for reliable performance

8. **Model-Based RL**
   - Learn dynamics and reward models from experience
   - Model Predictive Control (MPC) planning
   - Cross-Entropy Method (CEM) optimization
   - High sample efficiency

### RL Algorithm Comparison

| Algorithm | Policy | Sample Efficiency | Stability | Best For |
|-----------|--------|-------------------|-----------|----------|
| Q-Learning | Off-policy | Medium | High | Simple MDPs |
| SARSA | On-policy | Medium | High | Safe exploration |
| DQN | Off-policy | High | Medium | Large state spaces |
| Policy Gradient | On-policy | Low | Low | Continuous actions |
| Actor-Critic | On-policy | Medium | Medium | General purpose |
| PPO | On-policy | High | High | Most tasks |
| Decision Transformer | Offline | High | High | Offline RL |
| MCTS | Planning | N/A | High | Known dynamics |
| Model-Based | Off-policy | Very High | Medium | Limited samples |

### Feature Summary

| Feature | MANA v0.4.0 | MANA v0.5.0 |
|---------|-------------|-------------|
| RL Algorithms | 1 (Q-learning) | **9 algorithms** |
| Reflexion Memory | ❌ | ✅ |
| Self-Critique | ❌ | ✅ |

### Changed
- Bump version to 0.5.0
- Extended learning module with 8 new RL algorithms
- Added reflexion module for self-critique learning

## [0.4.0] - 2025-12-10

### Added - Advanced AI Features

#### HNSW Vector Index
- **Fast Approximate Nearest Neighbor Search**: O(log n) search complexity using instant-distance library
- **High Recall**: Configurable ef_construction and ef_search parameters for quality/speed tradeoff
- **Persistent Storage**: Save/load HNSW index to disk for fast startup
- **Batch Operations**: Efficient bulk add with automatic index rebuilding

#### ReasoningBank
- **Structured Reasoning Chains**: Store multi-step reasoning patterns (thought → observation → action)
- **Step-by-Step Tracking**: Track confidence scores per reasoning step
- **Reasoning Discovery**: Find similar reasoning chains for new tasks
- **Effectiveness Scoring**: Rank reasoning patterns by success rate
- **Conversation Extraction**: Automatically extract reasoning from Claude conversations

#### Q-Learning for Pattern Adaptation
- **Adaptive Pattern Weights**: Q-learning algorithm adjusts pattern priorities based on outcomes
- **Context-Aware Learning**: Different Q-values for same pattern in different contexts
- **Exploration vs Exploitation**: Epsilon-greedy selection with configurable decay
- **Persistent Q-Table**: SQLite-backed storage with episode logging
- **Statistics & Monitoring**: Track Q-value distribution, episode count, exploration rate

#### Vector Quantization
- **Scalar Quantization (SQ8)**: 4x memory reduction with ~1% recall loss
- **Scalar Quantization (SQ4)**: 8x memory reduction with ~3% recall loss
- **Binary Quantization**: 32x memory reduction for approximate search
- **Approximate Similarity**: Fast similarity computation on quantized vectors
- **Compression Statistics**: Track compression ratios and memory usage

### Feature Summary

| Feature | MANA v0.3.0 | MANA v0.4.0 |
|---------|-------------|-------------|
| HNSW Search | ❌ | ✅ |
| ReasoningBank | ❌ | ✅ |
| RL Algorithms | ❌ | ✅ (Q-learning) |
| Vector Quantization | ❌ | ✅ |

### Changed
- Bump version to 0.4.0
- Added instant-distance dependency for HNSW
- Extended storage module with reasoning chains
- Extended learning module with Q-learning

## [0.3.0] - 2025-12-09

### Added - Self-Healing & Performance

#### Self-Healing Capabilities
- **Pattern Validation**: Detects negative counts, duplicate hashes, orphaned embeddings
- **Auto-Repair**: Automatically fixes corrupted data (clamps negatives, merges duplicates)
- **Score Normalization**: Prevents unbounded score growth/decay with configurable bounds
- **Periodic Database Vacuum**: Automatic maintenance to prevent fragmentation

#### Performance Optimizations
- **LRU Similarity Cache**: Caches similarity calculations (1024 entries) for 10-50x speedup on repeated queries
- **Prepared Statement Pooling**: Reuses compiled SQL statements for faster queries
- **mmap-enabled SQLite**: Memory-mapped I/O for hot paths (8MB default, 30MB for daemon mode)
- **Enhanced Cache Settings**: 4000-8000 page cache for frequently accessed data

#### Enhanced Skill Library
- **Usage Tracking**: Tracks `times_recommended` and `times_used` per skill
- **Effectiveness Score**: Combines success rate (70%) + usage rate (30%)
- **Skill Composition Chains**: Tracks skill->skill dependencies via `skill_chains` table
- **Skill Discovery**:
  - `get_trending(hours, limit)` - Recently active skills
  - `get_most_effective(min_uses, limit)` - High success rate skills
  - `search(query)` - Semantic search across skill names/descriptions
  - `get_follow_up_skills()` / `get_prerequisite_skills()` - Skill sequencing

#### Comprehensive Benchmark Suite
- Percentile metrics (p50, p99) for latency measurements
- Performance summary table in output
- Similarity cache performance testing
- Batch insert throughput testing

### Performance Results

| Metric | MANA v0.3.0 |
|--------|-------------|
| Pattern Search p50 | 3μs |
| Batch Insert | 80,000+/s |
| Binary Startup | <1ms |
| Injection Latency | ~1.3ms |

### Changed
- Bump version to 0.3.0
- Enhanced ValidationReport with detailed issue tracking
- Improved skill consolidation with new schema

## [0.2.0] - Previous Release

### Added
- Initial release with core MANA functionality
- Pattern storage and retrieval
- Context injection for Claude Code
- Basic learning from Claude logs
- Reflection system with verdict analysis
- Causal graph for pattern relationships
