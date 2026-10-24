#!/usr/bin/env bash
# U0 spike (b) — prd-compute-update-continuity-v1 §10 U0, C5, R1.
#
# Question: on THIS machine's systemd, does a walled job (a transient unit
# started with `systemd-run --pipe --wait`, the way k2-node-admin starts
# every job, kn/admin/jobs.rs) keep running when the root service that
# started it exits, is stopped, or crashes? With and without
# `KillMode=process` on that service. And: does a regular-file fd (a
# spool file, U2) passed as the job's stdout through `--pipe` keep taking
# the job's writes after the helper is gone?
#
# It builds a stand-in for k2-node-admin.service (`k2spike-admin@<mode>`),
# never the real units: nothing named k2-node*, k2job* or k2-jobs* is
# touched. Every unit it makes starts with `k2spike-` and is removed at
# the end (also on failure).
#
# Needs: root on a DISPOSABLE or test Linux box with systemd >= 245 (the
# job wall's floor; z13flow or a throwaway VM). Not a customer box, not a
# box with someone's work on it. ~3 minutes.
#
#   K2_SPIKE_OK=1 sudo -E bash linux-job-vs-root-service.sh [--report PATH]
#
# Exit 0 = every row ran (read the table: a row can say "dies"); 1 = the
# spike itself failed. Report: /var/tmp/k2-spike-linux-job.txt.
set -euo pipefail
cd /

REPORT="/var/tmp/k2-spike-linux-job.txt"
while [ $# -gt 0 ]; do
    case "$1" in
        --report) REPORT="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = "Linux" ] || { echo "Linux only" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run as root (sudo): it starts system units" >&2; exit 2; }
[ "${K2_SPIKE_OK:-}" = "1" ] || { echo "refusing: set K2_SPIKE_OK=1 (a disposable or test box only)" >&2; exit 2; }
command -v systemd-run >/dev/null || { echo "no systemd-run" >&2; exit 2; }
SDV="$(systemctl --version | awk 'NR==1{print $2}')"
[ "$SDV" -ge 245 ] || { echo "systemd $SDV < 245 (the job wall's floor)" >&2; exit 2; }

W="$(mktemp -d /var/tmp/k2spike.XXXXXX)"
chmod 0755 "$W"
UNITS_DIR=/run/systemd/system
ROWS=()

cleanup() {
    local rc=$?
    for u in $(systemctl list-units --all --plain --no-legend 'k2spike-*' | awk '{print $1}'); do
        systemctl stop "$u" >/dev/null 2>&1 || true
        systemctl reset-failed "$u" >/dev/null 2>&1 || true
    done
    rm -f "$UNITS_DIR"/k2spike-admin-*.service
    systemctl daemon-reload || true
    rm -rf "$W"
    exit "$rc"
}
trap cleanup EXIT

row() { ROWS+=("$1|$2|$3"); echo "  $1: $2 ($3)"; }
active() { [ "$(systemctl show -p ActiveState --value "$1" 2>/dev/null)" = active ]; }
main_pid() { systemctl show -p MainPID --value "$1"; }
wait_active() { for _ in $(seq 1 50); do active "$1" && return 0; sleep 0.2; done; return 1; }

# The stand-in executor: it starts ONE job the k2-node-admin way (stdout =
# a regular spool file opened by the "node", passed in as fd 3 → stdout of
# systemd-run --pipe), then idles like the executor does between requests.
cat > "$W/executor.sh" <<'EOF'
#!/bin/bash
# $1 = job unit name, $2 = spool path
exec 3>>"$2"
systemd-run --pipe --wait --quiet --unit "$1" --slice k2spike.slice \
    --property=RemainAfterExit=yes --property=KillMode=control-group --property=TimeoutStopSec=5 \
    /bin/sh -c 'i=0; while true; do i=$((i+1)); echo "tick $i $(date +%s)"; sleep 1; done' <&- >&3 2>&3 &
echo "helper $!" > "$2.helper"
while true; do sleep 3600; done
EOF
chmod 0755 "$W/executor.sh"

mk_executor() { # <mode: process|control-group> <n>
    local mode="$1" n="$2"
    cat > "$UNITS_DIR/k2spike-admin-$n.service" <<EOF
[Unit]
Description=K2 spike: stand-in for k2-node-admin.service (KillMode=$mode)
[Service]
Type=simple
ExecStart=$W/executor.sh k2spike-job-$n $W/spool-$n.out
KillMode=$mode
Restart=no
UMask=0077
EOF
    systemctl daemon-reload
}

case_run() { # <n> <mode> <how: stop|exit|kill9-exec|kill9-helper> <title>
    local n="$1" mode="$2" how="$3" title="$4"
    mk_executor "$mode" "$n"
    : > "$W/spool-$n.out"
    systemctl start "k2spike-admin-$n.service"
    wait_active "k2spike-job-$n.service" || { row "$title" "job never started" "spike failed"; return 1; }
    sleep 3
    local helper; helper="$(awk '{print $2}' "$W/spool-$n.out.helper")"
    local ex; ex="$(main_pid "k2spike-admin-$n.service")"
    local job_pid; job_pid="$(main_pid "k2spike-job-$n.service")"
    local before; before="$(wc -l < "$W/spool-$n.out")"
    case "$how" in
        stop) systemctl stop "k2spike-admin-$n.service" ;;
        exit) kill -TERM "$ex" ;;               # the executor exits by itself (re-exec after an update)
        kill9-exec) kill -KILL "$ex" ;;          # the executor crashes
        kill9-helper) kill -KILL "$helper" ;;    # the systemd-run helper dies on its own (OOM, R1)
    esac
    sleep 4
    local job_now="dies"
    if active "k2spike-job-$n.service" && [ "$(main_pid "k2spike-job-$n.service")" = "$job_pid" ]; then job_now="keeps running"; fi
    local helper_now="gone"; kill -0 "$helper" 2>/dev/null && helper_now="alive"
    sleep 3
    local after; after="$(wc -l < "$W/spool-$n.out")"
    local spool="stopped at $before→$after lines"
    [ "$after" -gt $((before + 4)) ] && spool="still written ($before→$after lines)"
    row "$title" "job $job_now" "helper $helper_now; spool $spool; job unit $(systemctl show -p ActiveState --value "k2spike-job-$n.service")/$(systemctl show -p SubState --value "k2spike-job-$n.service")"
    systemctl stop "k2spike-job-$n.service" >/dev/null 2>&1 || true
    systemctl reset-failed "k2spike-job-$n.service" >/dev/null 2>&1 || true
    systemctl stop "k2spike-admin-$n.service" >/dev/null 2>&1 || true
}

echo "== systemd $SDV, $(uname -r), $(. /etc/os-release && echo "$PRETTY_NAME")"
echo "== KillMode=control-group (today's k2-node-admin.service: no KillMode line)"
case_run 1 control-group stop        "control-group, systemctl stop executor"
case_run 2 control-group exit        "control-group, executor exits (SIGTERM to main pid)"
case_run 3 control-group kill9-exec  "control-group, executor kill -9"
echo "== KillMode=process (0.46.4's k2-node-admin.service)"
case_run 4 process stop              "process, systemctl stop executor"
case_run 5 process exit              "process, executor exits"
case_run 6 process kill9-exec        "process, executor kill -9"
case_run 7 process kill9-helper      "process, the systemd-run helper kill -9 (R1)"

{
    echo "U0 spike (b): walled job vs its root service — $(date -u +%FT%TZ)"
    echo "host $(hostname)  systemd $SDV  kernel $(uname -r)  $(. /etc/os-release && echo "$PRETTY_NAME")"
    echo
    printf '%-58s | %-14s | %s\n' "case" "job" "detail"
    for r in "${ROWS[@]}"; do IFS='|' read -r a b c <<<"$r"; printf '%-58s | %-14s | %s\n' "$a" "$b" "$c"; done
    echo
    echo "Read: rows 4-6 must say 'job keeps running' for C5 (KillMode=process) to hold."
    echo "Row 7 'job dies' = R1: a dead helper takes its unit; plan the D-Bus StartTransientUnit fallback (+2-4 d)."
    echo "'spool still written' after the helper is gone = a regular-file fd through --pipe works for U2."
} > "$REPORT"
echo "report: $REPORT"
cat "$REPORT"
