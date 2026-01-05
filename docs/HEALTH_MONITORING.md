# MANA Health Monitoring System

## Overview

Event-driven pruning system with proactive memory maintenance for MANA. Automatically monitors and maintains database health based on configurable conditions.

## Implementation

### Files Created/Modified

1. **`/workspaces/ardenone-cluster/mana/src/storage/health.rs`** (NEW)
   - Complete health monitoring implementation
   - Health status computation
   - Pruning actions and strategies
   - Auto-pruning engine

2. **`/workspaces/ardenone-cluster/mana/src/storage/mod.rs`** (MODIFIED)
   - Added `pub mod health;`
   - Exported health types
   - Added `access_count` column migration for tracking pattern usage

3. **`/workspaces/ardenone-cluster/mana/src/main.rs`** (MODIFIED)
   - Added `Health` command with subcommands
   - CLI interface for health status and auto-pruning
   - Dry-run support

4. **`/workspaces/ardenone-cluster/mana/src/daemon/mod.rs`** (MODIFIED)
   - Integrated health monitor into daemon state
   - Periodic health checks (every 6 hours)
   - Background auto-pruning
   - New daemon commands: `health_check`, `health_status`

## Features

### Health Metrics

The system tracks:
- **Total patterns**: Count of all patterns in database
- **Low confidence**: Patterns with score < -2
- **Floor confidence**: Patterns with score < -5 (marked for deletion)
- **Stale patterns**: Patterns unused for 30+ days
- **Never accessed**: Patterns with `access_count` = 0
- **Average score**: Overall quality metric
- **Storage size**: Database file size in bytes

### Health Score Calculation

The health score (0.0 - 1.0) is computed based on:
- Floor patterns: -50 points (very bad)
- Low confidence: -20 points (bad)
- Stale patterns: -15 points (slightly bad)
- Never accessed: -15 points (poor quality indicator)
- Storage pressure: -20 points (if over limit)
- Good average score (>3.0): +10 points (bonus)

Database is considered "healthy" if health score >= 0.7 (70%)

### Pruning Actions

Five types of pruning actions:

1. **DeleteFloor**: Remove patterns with score < -5
2. **DecayStale**: Reduce success_count for 30+ day unused patterns
3. **PenalizeUnused**: Decay never-accessed patterns
4. **AggressiveDecay**: Apply heavy decay when signal/noise ratio is poor
5. **StoragePressure**: Delete bottom 10% of patterns when storage exceeds limit

### Configuration

Default `PruningConfig`:
```rust
PruningConfig {
    floor_threshold: -5,           // Score threshold for deletion
    low_threshold: -2,             // Score threshold for low confidence
    stale_days: 30,                // Days before pattern is considered stale
    decay_factor: 0.85,            // Multiplier for decay operations
    max_storage_bytes: 100MB,      // Storage limit before pressure pruning
    target_health_score: 0.7,      // Target health (70%)
}
```

## CLI Usage

### Check Health Status

```bash
mana health
```

Shows:
- Overall health score and status
- Pattern statistics breakdown
- Storage usage
- Recommended pruning actions

### Run Auto-Pruning (Dry Run)

```bash
mana health auto-prune --dry-run
```

Shows what would be pruned without actually executing.

### Execute Auto-Pruning

```bash
mana health auto-prune
```

Executes all recommended pruning actions and shows before/after health scores.

## Daemon Integration

When the daemon is running, health checks occur automatically:

- **Frequency**: Every 6 hours
- **Trigger**: Checked every ~60 seconds in daemon loop
- **Action**: Automatic pruning if health score is below target
- **Logging**: Results logged via tracing

### Manual Daemon Commands

Query health status via daemon:
```bash
# Send health_status command to daemon socket
echo '{"command":"health_status"}' | nc -U ~/.mana/daemon.sock

# Trigger manual health check
echo '{"command":"health_check"}' | nc -U ~/.mana/daemon.sock
```

## Database Schema Changes

New column added to `patterns` table:
```sql
ALTER TABLE patterns ADD COLUMN access_count INTEGER DEFAULT 0;
```

This column tracks how many times a pattern has been accessed/injected, enabling the "never accessed" metric. The migration runs automatically on `mana init`.

## Example Output

### Health Status
```
MANA Health Status
==================

Overall Health: 45.2% NEEDS ATTENTION

Pattern Statistics:
  Total patterns: 1000
  Low confidence (score < -2): 150 (15.0%)
  Floor confidence (score < -5): 50 (5.0%)
  Stale (30+ days unused): 300 (30.0%)
  Never accessed: 200 (20.0%)
  Average score: 1.23

Storage:
  Database size: 12.34 MB

Recommended Actions:
  - DeleteFloor
  - DecayStale
  - PenalizeUnused

Run 'mana health auto-prune' to execute these actions.
```

### Auto-Pruning
```
Running health auto-pruning...

Pruning Complete
================

Actions taken: 3
  - DeleteFloor
  - DecayStale
  - PenalizeUnused

Patterns deleted: 50
Patterns decayed: 450

Health Score:
  Before: 45.2% (unhealthy)
  After:  72.8% (healthy)

Database health improved!
```

## Architecture

### Health Monitor Flow

```
┌─────────────────┐
│ HealthMonitor   │
│ (config)        │
└────────┬────────┘
         │
         ├──> check_health(conn)
         │    └──> Returns HealthStatus
         │
         ├──> recommend_actions(status)
         │    └──> Returns Vec<PruningAction>
         │
         └──> auto_prune(conn)
              ├──> check_health (before)
              ├──> recommend_actions
              ├──> execute_action for each
              ├──> check_health (after)
              └──> Returns PruningResult
```

### Daemon Integration

```
┌──────────────┐
│ DaemonState  │
│              │
│ - conn       │
│ - health_    │
│   monitor    │
│ - last_      │
│   health_    │
│   check      │
└──────┬───────┘
       │
       ├──> should_run_health_check()
       │    └──> Checks if 6 hours elapsed
       │
       └──> run_health_check()
            └──> Opens writable connection
                 └──> Calls auto_prune()
```

### Periodic Execution

The daemon checks health every ~60 seconds in the main loop. If 6 hours have elapsed since the last check, it automatically runs `run_health_check()`.

## Testing

The module includes unit tests:

```rust
#[test]
fn test_health_computation() { ... }

#[test]
fn test_action_recommendations() { ... }
```

Run with:
```bash
cargo test --lib storage::health
```

## Performance Considerations

- **Health checks**: Fast (< 10ms) - simple SQL aggregations
- **Pruning operations**: Moderate (10-100ms) - depends on database size
- **Daemon overhead**: Negligible - checks every 60s, executes every 6 hours
- **Storage**: No additional storage overhead (uses existing columns + 1 new column)

## Future Enhancements

Potential improvements:
- [ ] Configurable health check intervals via config file
- [ ] Pattern quality scoring based on recency + frequency
- [ ] Machine learning for optimal pruning thresholds
- [ ] Health history tracking (store health snapshots)
- [ ] Alerting when health drops below threshold
- [ ] Pattern resurrection (restore accidentally pruned patterns)
- [ ] Advanced decay strategies (exponential, linear, custom)

## Conclusion

The health monitoring system provides automated, event-driven memory maintenance for MANA. It ensures the pattern database stays healthy by:
1. Continuously monitoring quality metrics
2. Automatically pruning low-quality patterns
3. Decaying stale/unused patterns
4. Managing storage pressure
5. Running transparently in the background (daemon mode)

This keeps MANA's memory sharp and focused on high-quality, useful patterns.
