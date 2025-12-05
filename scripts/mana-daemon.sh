#!/bin/bash
# MANA Background Learning Daemon
#
# Continuously monitors Claude Code logs and triggers learning/consolidation
# periodically rather than waiting for session-end hooks.
# Also handles automatic pattern synchronization across workspaces.
#
# Usage:
#   ./mana-daemon.sh start     - Start daemon in background
#   ./mana-daemon.sh stop      - Stop running daemon
#   ./mana-daemon.sh status    - Check daemon status
#   ./mana-daemon.sh run       - Run in foreground (for debugging)
#
# Configuration (via environment variables):
#   MANA_LEARN_INTERVAL   - Seconds between learning runs (default: 300 = 5 min)
#   MANA_CONSOLIDATE_INTERVAL - Seconds between consolidation runs (default: 3600 = 1 hour)
#   MANA_SYNC_INTERVAL    - Seconds between sync operations (default: 3600 = 1 hour)
#   MANA_SYNC_ENABLED     - Enable automatic sync (default: false)
#   MANA_LOG_DIR          - Directory for daemon logs (default: ~/.mana/logs)
#   MANA_BINARY           - Path to mana binary (auto-detected)

set -e

# Configuration defaults
MANA_LEARN_INTERVAL="${MANA_LEARN_INTERVAL:-300}"
MANA_CONSOLIDATE_INTERVAL="${MANA_CONSOLIDATE_INTERVAL:-3600}"
MANA_SYNC_INTERVAL="${MANA_SYNC_INTERVAL:-3600}"
MANA_SYNC_ENABLED="${MANA_SYNC_ENABLED:-false}"
MANA_LOG_DIR="${MANA_LOG_DIR:-$HOME/.mana/logs}"
PID_FILE="${MANA_PID_FILE:-$HOME/.mana/daemon.pid}"

# Find MANA binary
find_mana_binary() {
    # Check project-local first
    if [[ -x ".mana/mana" ]]; then
        echo ".mana/mana"
        return
    fi

    # Check common locations
    for loc in \
        "/workspaces/ord-options-testing/.mana/mana" \
        "$HOME/.mana/mana" \
        "$(which mana 2>/dev/null)"; do
        if [[ -x "$loc" ]]; then
            echo "$loc"
            return
        fi
    done

    echo ""
}

MANA_BINARY="${MANA_BINARY:-$(find_mana_binary)}"

# Logging
log() {
    local level="$1"
    shift
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] [$level] $*"
}

log_info() { log "INFO" "$@"; }
log_error() { log "ERROR" "$@"; }
log_debug() { log "DEBUG" "$@"; }

# Check if daemon is running
is_running() {
    if [[ -f "$PID_FILE" ]]; then
        local pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
    fi
    return 1
}

# Check if sync is configured
is_sync_configured() {
    if [[ -f ".mana/sync.toml" ]] || [[ -f "$HOME/.mana/sync.toml" ]]; then
        return 0
    fi
    return 1
}

# Main daemon loop
daemon_loop() {
    log_info "MANA daemon starting"
    log_info "Binary: $MANA_BINARY"
    log_info "Learn interval: ${MANA_LEARN_INTERVAL}s"
    log_info "Consolidate interval: ${MANA_CONSOLIDATE_INTERVAL}s"
    log_info "Sync enabled: $MANA_SYNC_ENABLED"
    if [[ "$MANA_SYNC_ENABLED" == "true" ]]; then
        log_info "Sync interval: ${MANA_SYNC_INTERVAL}s"
    fi

    if [[ -z "$MANA_BINARY" ]] || [[ ! -x "$MANA_BINARY" ]]; then
        log_error "MANA binary not found or not executable"
        exit 1
    fi

    local last_learn=0
    local last_consolidate=0
    local last_sync=0

    # Trap signals for clean shutdown
    trap 'log_info "Shutting down..."; exit 0' SIGTERM SIGINT

    while true; do
        local now=$(date +%s)

        # Run learning if interval elapsed
        if (( now - last_learn >= MANA_LEARN_INTERVAL )); then
            log_info "Running learning cycle"
            if "$MANA_BINARY" session-end 2>&1 | head -20; then
                log_info "Learning cycle complete"
            else
                log_error "Learning cycle failed"
            fi
            last_learn=$now
        fi

        # Run consolidation if interval elapsed
        if (( now - last_consolidate >= MANA_CONSOLIDATE_INTERVAL )); then
            log_info "Running consolidation"
            if "$MANA_BINARY" consolidate 2>&1 | head -20; then
                log_info "Consolidation complete"
            else
                log_error "Consolidation failed"
            fi
            last_consolidate=$now
        fi

        # Run sync if enabled, configured, and interval elapsed
        if [[ "$MANA_SYNC_ENABLED" == "true" ]] && is_sync_configured; then
            if (( now - last_sync >= MANA_SYNC_INTERVAL )); then
                log_info "Running sync cycle"

                # First pull to get remote changes
                log_info "Pulling remote patterns..."
                if "$MANA_BINARY" sync pull 2>&1 | head -10; then
                    log_info "Pull complete"
                else
                    log_error "Pull failed (continuing...)"
                fi

                # Then push local changes
                log_info "Pushing local patterns..."
                if "$MANA_BINARY" sync push -m "Daemon auto-sync at $(date '+%Y-%m-%d %H:%M:%S')" 2>&1 | head -10; then
                    log_info "Push complete"
                else
                    log_error "Push failed (continuing...)"
                fi

                last_sync=$now
                log_info "Sync cycle complete"
            fi
        fi

        # Sleep for a short interval to check for signals
        sleep 30
    done
}

# Start daemon in background
start_daemon() {
    if is_running; then
        log_error "Daemon already running (PID: $(cat "$PID_FILE"))"
        exit 1
    fi

    # Create log directory
    mkdir -p "$MANA_LOG_DIR"

    # Start daemon in background
    nohup "$0" run >> "$MANA_LOG_DIR/daemon.log" 2>&1 &
    local pid=$!

    echo "$pid" > "$PID_FILE"
    log_info "Daemon started with PID $pid"
    log_info "Logs: $MANA_LOG_DIR/daemon.log"
}

# Stop daemon
stop_daemon() {
    if ! is_running; then
        log_error "Daemon not running"
        exit 1
    fi

    local pid=$(cat "$PID_FILE")
    log_info "Stopping daemon (PID: $pid)"

    kill "$pid" 2>/dev/null || true
    rm -f "$PID_FILE"

    log_info "Daemon stopped"
}

# Show daemon status
show_status() {
    if is_running; then
        local pid=$(cat "$PID_FILE")
        echo "MANA daemon is running (PID: $pid)"
        echo "Binary: $MANA_BINARY"
        echo "Log dir: $MANA_LOG_DIR"
        echo ""
        echo "Intervals:"
        echo "  Learn: ${MANA_LEARN_INTERVAL}s"
        echo "  Consolidate: ${MANA_CONSOLIDATE_INTERVAL}s"
        echo "  Sync: ${MANA_SYNC_INTERVAL}s (enabled: $MANA_SYNC_ENABLED)"

        # Show sync configuration status
        echo ""
        if is_sync_configured; then
            echo "Sync: ✅ Configured"
            if [[ "$MANA_SYNC_ENABLED" == "true" ]]; then
                echo "  Auto-sync: ✅ Enabled"
            else
                echo "  Auto-sync: ❌ Disabled (set MANA_SYNC_ENABLED=true)"
            fi
        else
            echo "Sync: ❌ Not configured (run 'mana sync init')"
        fi

        # Show last few log lines
        if [[ -f "$MANA_LOG_DIR/daemon.log" ]]; then
            echo ""
            echo "Recent logs:"
            tail -10 "$MANA_LOG_DIR/daemon.log"
        fi
    else
        echo "MANA daemon is not running"

        # Check for stale PID file
        if [[ -f "$PID_FILE" ]]; then
            echo "(stale PID file exists, removing)"
            rm -f "$PID_FILE"
        fi
    fi
}

# Main command handler
case "${1:-}" in
    start)
        start_daemon
        ;;
    stop)
        stop_daemon
        ;;
    status)
        show_status
        ;;
    run)
        daemon_loop
        ;;
    *)
        echo "MANA Background Learning Daemon"
        echo ""
        echo "Usage: $0 {start|stop|status|run}"
        echo ""
        echo "Commands:"
        echo "  start   - Start daemon in background"
        echo "  stop    - Stop running daemon"
        echo "  status  - Check daemon status and show recent logs"
        echo "  run     - Run in foreground (for debugging)"
        echo ""
        echo "Environment variables:"
        echo "  MANA_LEARN_INTERVAL=$MANA_LEARN_INTERVAL (seconds between learning)"
        echo "  MANA_CONSOLIDATE_INTERVAL=$MANA_CONSOLIDATE_INTERVAL (seconds between consolidation)"
        echo "  MANA_SYNC_INTERVAL=$MANA_SYNC_INTERVAL (seconds between sync)"
        echo "  MANA_SYNC_ENABLED=$MANA_SYNC_ENABLED (true to enable auto-sync)"
        echo "  MANA_LOG_DIR=$MANA_LOG_DIR"
        echo "  MANA_BINARY=$MANA_BINARY"
        echo ""
        echo "To enable automatic sync:"
        echo "  1. Configure sync: mana sync init --remote <url>"
        echo "  2. Set environment: export MANA_SYNC_ENABLED=true"
        echo "  3. Optionally set key: export MANA_SYNC_KEY=<passphrase>"
        echo "  4. Start daemon: $0 start"
        exit 1
        ;;
esac
