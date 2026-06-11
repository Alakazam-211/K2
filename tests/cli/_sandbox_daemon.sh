#!/usr/bin/env bash
# Helper for Phase 2.1b CLI tests that need a live daemon. Boots a
# foreground daemon under a temporary HOME, sets $K2SO_PORT/$K2SO_TOKEN,
# and arranges to kill the daemon + clean up the sandbox on EXIT.
#
# Source this from your test script:
#   source "$(dirname "$0")/_sandbox_daemon.sh"
#   sandbox_daemon_start
#   # ... use $SANDBOX_HOME / $K2SO_PORT / $K2SO_TOKEN ...
#
# Hard rules (see CLAUDE.md / memory/feedback_subagent_no_prod_reload.md):
#   - HOME=$SANDBOX_HOME for the daemon process. Never touch ~/.k2so/.
#   - Daemon binary is the worktree's target/release/k2so-daemon —
#     never `cargo install`, never bind to /Applications/K2SO.app.

set -euo pipefail

sandbox_daemon_start() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local project_root
    project_root="$(cd "$script_dir/../.." && pwd)"

    SANDBOX_HOME="$(mktemp -d -t k2so-2.1b-sandbox-XXXXXX)"
    export SANDBOX_HOME

    # Prefer release; fall back to debug if release isn't built yet.
    local daemon_bin="$project_root/target/release/k2so-daemon"
    if [ ! -x "$daemon_bin" ]; then
        daemon_bin="$project_root/target/debug/k2so-daemon"
    fi
    if [ ! -x "$daemon_bin" ]; then
        echo "FAIL: k2so-daemon binary not found in target/{release,debug}/" >&2
        echo "       Build first: cargo build -p k2so-daemon" >&2
        return 1
    fi
    SANDBOX_DAEMON_BIN="$daemon_bin"
    export SANDBOX_DAEMON_BIN

    # Launch the daemon in the background under the sandbox HOME.
    HOME="$SANDBOX_HOME" "$daemon_bin" >"$SANDBOX_HOME/daemon.log" 2>&1 &
    SANDBOX_DAEMON_PID=$!
    export SANDBOX_DAEMON_PID

    # Wait up to 10s for the daemon to write its port file.
    local i
    for i in $(seq 1 50); do
        if [ -f "$SANDBOX_HOME/.k2so/daemon.port" ]; then break; fi
        sleep 0.2
    done

    if [ ! -f "$SANDBOX_HOME/.k2so/daemon.port" ]; then
        echo "FAIL: daemon never wrote daemon.port (pid=$SANDBOX_DAEMON_PID)" >&2
        echo "log tail:" >&2
        tail -40 "$SANDBOX_HOME/daemon.log" >&2 || true
        kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
        return 1
    fi

    K2SO_PORT="$(cat "$SANDBOX_HOME/.k2so/daemon.port")"
    K2SO_TOKEN="$(cat "$SANDBOX_HOME/.k2so/daemon.token" 2>/dev/null || echo "")"
    export K2SO_PORT K2SO_TOKEN

    # /health is unauthenticated and a fast liveness probe.
    for i in $(seq 1 25); do
        if curl -sf --connect-timeout 1 "http://127.0.0.1:${K2SO_PORT}/health" >/dev/null 2>&1; then
            break
        fi
        sleep 0.2
    done

    if ! curl -sf --connect-timeout 1 "http://127.0.0.1:${K2SO_PORT}/health" >/dev/null 2>&1; then
        echo "FAIL: daemon /health never returned 200" >&2
        tail -40 "$SANDBOX_HOME/daemon.log" >&2 || true
        kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
        return 1
    fi

    # Best-effort cleanup on exit.
    trap _sandbox_daemon_cleanup EXIT
}

_sandbox_daemon_cleanup() {
    if [ -n "${SANDBOX_DAEMON_PID:-}" ]; then
        kill "$SANDBOX_DAEMON_PID" 2>/dev/null || true
        # Give the daemon a beat to exit cleanly before SIGKILL.
        sleep 0.3
        kill -9 "$SANDBOX_DAEMON_PID" 2>/dev/null || true
    fi
    if [ -n "${SANDBOX_HOME:-}" ] && [ -d "$SANDBOX_HOME" ]; then
        rm -rf "$SANDBOX_HOME"
    fi
}
