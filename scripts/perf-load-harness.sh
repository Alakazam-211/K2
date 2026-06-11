#!/bin/bash
# K2SO Performance Load Harness
#
# Exercises K2SO under heavy filesystem + DB + terminal load to reveal
# whether any instrumented hot path breaches the 16ms UI-frame budget
# under contention. Output drives the P3/P4 decision (tokio polling /
# daemon split). If nothing stalls, both phases fall through their gates.
#
# How it works
#
# 1. External load:     filesystem storm (save-storm simulation), k2so CLI
#                       spam (DB write pressure), optional CPU hogger.
# 2. In-app load:       YOU follow the printed checklist inside K2SO —
#                       open N terminals, run noisy commands, fire a
#                       fuzzy search every so often.
# 3. Capture + analyze: K2SO's stderr (with K2SO_PERF=1) collects
#                       [perf] summary lines. `perf-analyze.sh`
#                       aggregates worst-case p50/p99/max per histogram.
#
# Prerequisites
#
#   - K2SO dev build running with stderr captured to a log file:
#
#       K2SO_PERF=1 bun run tauri dev 2>&1 | tee /tmp/k2so-perf.log
#
#     Or skip the tee and redirect:
#
#       K2SO_PERF=1 bun run tauri dev > /tmp/k2so-perf.log 2>&1
#
#   - A test project path to storm. Defaults to this repo.
#
# Usage
#
#   ./scripts/perf-load-harness.sh [--duration SEC] [--target DIR] [--rate HZ] [--log PATH]
#
# Defaults:  duration=300s (5 min), target=$(pwd), rate=20Hz, log=/tmp/k2so-perf.log

set -euo pipefail

# ---- args ----
DURATION=300
TARGET_DIR="$(pwd)"
RATE=20
LOG_PATH="/tmp/k2so-perf.log"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration) DURATION="$2"; shift 2 ;;
        --target)   TARGET_DIR="$2"; shift 2 ;;
        --rate)     RATE="$2"; shift 2 ;;
        --log)      LOG_PATH="$2"; shift 2 ;;
        -h|--help)
            grep -E '^#( |$)' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

# ---- colors ----
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
NC='\033[0m'
BOLD='\033[1m'

# ---- sanity ----
if [[ ! -d "$TARGET_DIR" ]]; then
    echo -e "${RED}target directory does not exist: $TARGET_DIR${NC}" >&2
    exit 1
fi

if [[ ! -f "$LOG_PATH" ]]; then
    echo -e "${YELLOW}warning:${NC} $LOG_PATH doesn't exist yet."
    echo "Start K2SO in another terminal first:"
    echo ""
    echo "    K2SO_PERF=1 bun run tauri dev 2>&1 | tee $LOG_PATH"
    echo ""
    read -p "Press enter once K2SO is running and the log is being written, or Ctrl-C to abort... "
fi

SCRATCH_DIR="$TARGET_DIR/.k2so-perf-scratch"
mkdir -p "$SCRATCH_DIR"

# ---- cleanup trap ----
PIDS=()
cleanup() {
    echo ""
    echo -e "${CYAN}Cleaning up load generators...${NC}"
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    rm -rf "$SCRATCH_DIR"
    echo -e "${GREEN}Done.${NC}"
}
trap cleanup EXIT INT TERM

# ---- banner ----
echo ""
echo -e "${BOLD}K2SO Performance Load Harness${NC}"
echo "────────────────────────────────────────────────────"
echo "  Duration:   ${DURATION}s"
echo "  Target dir: $TARGET_DIR"
echo "  Rate:       ${RATE}Hz filesystem events"
echo "  Log:        $LOG_PATH"
echo ""

# ---- manual checklist ----
cat <<EOF
${YELLOW}${BOLD}MANUAL LOAD — do these inside K2SO now:${NC}

  1) Open 4 terminals in the K2SO app.
  2) In terminal #1:  yes | awk 'BEGIN{while(1){print NR++,strftime()}}'
  3) In terminal #2:  find / -type f 2>/dev/null | head -200000
  4) In terminal #3:  cd $TARGET_DIR && cargo check --manifest-path src-tauri/Cargo.toml
  5) In terminal #4:  tail -f $LOG_PATH
  6) Open the Agents tab. Fire a heartbeat if the workspace has an agent.
  7) Trigger Fuzzy File Finder (Cmd+P) a few times during the run.
  8) Switch between projects in the sidebar once or twice.

External load generators will now run for ${DURATION}s against:
  $TARGET_DIR

Press enter when ready to start the external load...
EOF

read -r

START_TS="$(date +%s)"

# ─────────────────────────────────────────────────────────────
# Load generator 1: filesystem storm
#
# Touches + writes + deletes files inside $SCRATCH_DIR at $RATE Hz.
# Exercises watcher.rs debounce/batching. The coalesced emit count
# per window is what P1.4 reduced to 1 — if it's still 1 under this
# load, batching is holding. If we see emit_count > 1, something
# regressed.
# ─────────────────────────────────────────────────────────────
(
    sleep_sec="$(awk -v r="$RATE" 'BEGIN{ printf "%.4f", 1.0/r }')"
    n=0
    while :; do
        f="$SCRATCH_DIR/fs-storm-$((n % 50)).txt"
        printf 'line %d at %s\n' "$n" "$(date +%s%N)" > "$f"
        n=$((n + 1))
        if (( n % 200 == 0 )); then
            rm -f "$SCRATCH_DIR"/fs-storm-*.txt
        fi
        sleep "$sleep_sec"
    done
) &
PIDS+=($!)
echo -e "${GREEN}✓${NC} filesystem storm (PID $!) — ${RATE}Hz in $SCRATCH_DIR"

# ─────────────────────────────────────────────────────────────
# Load generator 2: DB write pressure via k2so CLI
#
# Fires `k2so work create` every 2s. Each call crosses the
# agent_sessions / activity_feed INSERT path that P1.3 cached.
# If prepare_cached held, DB timings stay flat.
# ─────────────────────────────────────────────────────────────
(
    i=0
    while :; do
        if command -v k2so >/dev/null 2>&1; then
            K2SO_PROJECT_PATH="$TARGET_DIR" k2so work create \
                --title "perf harness probe #$i" \
                --body "generated by perf-load-harness.sh" \
                --priority low --type task >/dev/null 2>&1 || true
        fi
        i=$((i + 1))
        sleep 2
    done
) &
PIDS+=($!)
echo -e "${GREEN}✓${NC} k2so CLI write storm (PID $!) — every 2s"

# ─────────────────────────────────────────────────────────────
# Load generator 3: deep-walk storm
#
# Triggers large-tree scans that race with in-app Fuzzy Finder.
# Exercises ignore::WalkBuilder (file_index.rs).
# ─────────────────────────────────────────────────────────────
(
    while :; do
        find "$TARGET_DIR" -type f -not -path '*/node_modules/*' -not -path '*/target/*' \
            -not -path '*/.git/*' >/dev/null 2>&1 || true
        sleep 3
    done
) &
PIDS+=($!)
echo -e "${GREEN}✓${NC} recursive walk storm (PID $!) — every 3s"

echo ""
echo -e "${CYAN}Running for ${DURATION}s. Histogram summaries flush every 100 samples per path.${NC}"
echo -e "${CYAN}Tail the log in another terminal:  tail -f $LOG_PATH | grep '\\[perf\\]'${NC}"
echo ""

# ---- progress dots ----
elapsed=0
while (( elapsed < DURATION )); do
    sleep 10
    elapsed=$((elapsed + 10))
    pct=$((elapsed * 100 / DURATION))
    printf '\r  [%-50s] %3d%% (%ds / %ds)' \
        "$(printf '#%.0s' $(seq 1 $((pct / 2))))" "$pct" "$elapsed" "$DURATION"
done
echo ""
echo ""

END_TS="$(date +%s)"

# ---- analyze ----
echo -e "${BOLD}Analyzing $LOG_PATH for [perf] lines between run start and end...${NC}"
echo ""

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
if [[ -x "$SCRIPT_DIR/perf-analyze.sh" ]]; then
    "$SCRIPT_DIR/perf-analyze.sh" "$LOG_PATH"
else
    echo -e "${YELLOW}perf-analyze.sh not found or not executable.${NC}"
    echo "Run it manually: scripts/perf-analyze.sh $LOG_PATH"
fi
