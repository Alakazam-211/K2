#!/bin/bash
# K2SO Performance Log Analyzer
#
# Reads a K2SO stderr log (with K2SO_PERF=1 enabled), extracts every
# `[perf] <name> — p50=... p99=...` summary line, and prints the
# worst-case p50/p99/max per histogram across the whole log.
#
# Exit code:
#   0 — no path breached the P3/P4 gate threshold (16ms p99)
#   1 — at least one path breached — P3 (or P4) may be worth it
#
# Usage:
#   ./scripts/perf-analyze.sh <log-path> [--gate-ms N]
#
# The gate threshold defaults to 16 (one 60fps frame). Anything under it
# means the UI event loop has headroom even under the exercised load.

set -euo pipefail

LOG="${1:-}"
GATE_MS=16

shift 2>/dev/null || true
while [[ $# -gt 0 ]]; do
    case "$1" in
        --gate-ms) GATE_MS="$2"; shift 2 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

if [[ -z "$LOG" || ! -f "$LOG" ]]; then
    echo "usage: $0 <log-path> [--gate-ms N]" >&2
    exit 2
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

GATE_US=$((GATE_MS * 1000))

# Extract every "[perf] NAME — p50=Nµs p99=Nµs mean=Nµs count=N min=Nµs max=Nµs" line,
# group by NAME, keep max of each metric. Print a table + an any-breach flag.

awk -v gate_us="$GATE_US" -v gate_ms="$GATE_MS" '
BEGIN {
    # Expected format examples:
    #   [perf] terminal_poll_tick — p50=84µs p99=412µs mean=104µs count=100 min=51µs max=918µs
    #   [perf] startup_db_init — 6800µs   (from perf_timer!)
    breach = 0
}

# Histogram summary lines (have p50/p99)
/\[perf\][^—]*— p50=/ {
    name = $0
    sub(/.*\[perf\] */, "", name)
    sub(/ —.*/, "", name)

    p50 = extract($0, "p50=")
    p99 = extract($0, "p99=")
    mean = extract($0, "mean=")
    maxv = extract($0, "max=")
    count = extract_int($0, "count=")

    if (!(name in seen)) {
        order[++n] = name
        seen[name] = 1
    }
    if (p99 > max_p99[name]) max_p99[name] = p99
    if (p50 > max_p50[name]) max_p50[name] = p50
    if (mean > max_mean[name]) max_mean[name] = mean
    if (maxv > max_max[name]) max_max[name] = maxv
    total_count[name] += count
    next
}

# Timer single-shot lines:  [perf] NAME — Nµs
/\[perf\][^—]*— [0-9]+µs *$/ {
    name = $0
    sub(/.*\[perf\] */, "", name)
    sub(/ —.*/, "", name)

    # last token before µs
    v = $0
    sub(/.*— /, "", v)
    sub(/µs.*/, "", v)
    v += 0

    if (!(name in seen)) {
        order[++n] = name
        seen[name] = 1
    }
    if (v > max_max[name]) max_max[name] = v
    if (v > max_p99[name]) max_p99[name] = v
    if (v > max_p50[name]) max_p50[name] = v
    if (v > max_mean[name]) max_mean[name] = v
    timer_count[name]++
    next
}

function extract(s, key,   start, rest, val) {
    start = index(s, key)
    if (start == 0) return 0
    rest = substr(s, start + length(key))
    # value is digits until "µs"
    val = rest
    sub(/µs.*/, "", val)
    return val + 0
}
function extract_int(s, key,   start, rest, val) {
    start = index(s, key)
    if (start == 0) return 0
    rest = substr(s, start + length(key))
    val = rest
    sub(/[^0-9].*/, "", val)
    return val + 0
}

function fmt(us,   ms) {
    if (us >= 1000) {
        ms = us / 1000.0
        return sprintf("%.2fms", ms)
    }
    return sprintf("%dµs", us)
}

END {
    if (n == 0) {
        print "No [perf] lines found in log."
        exit 2
    }

    # Header
    printf "%-38s %12s %12s %12s %12s %10s  %s\n", \
        "path", "p50 (max)", "p99 (max)", "mean (max)", "max", "samples", "gate"
    sep = ""
    for (i = 0; i < 110; i++) sep = sep "-"
    printf "%s\n", sep

    breach = 0
    for (i = 1; i <= n; i++) {
        nm = order[i]
        c = (total_count[nm] > 0) ? total_count[nm] : timer_count[nm]
        flag = (max_p99[nm] > gate_us) ? "OVER " gate_ms "ms" : "ok"
        if (flag !~ /ok/) breach = 1
        printf "%-38s %12s %12s %12s %12s %10d  %s\n", \
            nm, fmt(max_p50[nm]), fmt(max_p99[nm]), fmt(max_mean[nm]), \
            fmt(max_max[nm]), c, flag
    }

    printf "\nGate: p99 > %dms indicates UI-frame pressure (potential P3 candidate).\n", gate_ms
    exit breach
}
' "$LOG"

status=$?

echo ""
if [[ $status -eq 0 ]]; then
    echo -e "${GREEN}${BOLD}No path breached the ${GATE_MS}ms gate under load.${NC}"
    echo -e "${GREEN}→ P3 (tokio polling) does not meet its decision rule.${NC}"
    echo -e "${GREEN}→ P4 (daemon split) is gated on P3 — also unnecessary for speed.${NC}"
elif [[ $status -eq 1 ]]; then
    echo -e "${RED}${BOLD}One or more paths breached the ${GATE_MS}ms gate.${NC}"
    echo -e "${YELLOW}→ P3 (tokio-ize polling) is justified for the 'OVER' paths above.${NC}"
    echo -e "${YELLOW}→ If UI blocking persists post-P3, P4 (daemon split) becomes warranted.${NC}"
else
    echo -e "${YELLOW}No [perf] lines found. Did K2SO run with K2SO_PERF=1?${NC}"
fi

exit $status
