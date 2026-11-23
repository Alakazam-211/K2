#!/usr/bin/env bash
# Agent Isolation M2, row P5 (design rev 3.1, C2): name what the stage-1
# `k2agents` table logs. nftables logs addresses, ports and the uid, never
# a cgroup or a pid, so this watcher (root, stage 1 only, on our own boxes)
# follows the `k2agents:` kernel log lines and looks up each line's source
# port at once: `ss` gives the pid, `/proc/<pid>/cgroup` and `comm` the rest.
# A connection already closed can't be named; those are counted.
#
# usage: sudo scripts/agent-isolation-p5-watch.sh [out-file]
#   out-file (default /var/log/k2agents-p5.tsv): one line per logged
#   connection: time, proto, source port, destination port, pid, comm,
#   cgroup (or "-" when it couldn't be named).
# Stop with Ctrl-C; it prints the named/unnamed counts and the counters
# of the `inet k2agents` table.
set -uo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root (it reads every process's sockets)" >&2; exit 2; }
for t in journalctl ss nft; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 2; }; done
nft list table inet k2agents >/dev/null 2>&1 || { echo "no inet k2agents table loaded (stage 1 not on)" >&2; exit 2; }
out="${1:-/var/log/k2agents-p5.tsv}"
named=0
unnamed=0
summary() {
    echo "named: $named · not named (closed before the lookup): $unnamed · lines in $out"
    nft list counters table inet k2agents 2>/dev/null || nft list table inet k2agents | grep -E 'counter'
}
trap 'summary; exit 0' INT TERM

while IFS= read -r line; do
    case "$line" in *"k2agents: "*) ;; *) continue ;; esac
    proto="$(sed -n 's/.*PROTO=\([A-Z]*\).*/\1/p' <<<"$line")"
    spt="$(sed -n 's/.*SPT=\([0-9]*\).*/\1/p' <<<"$line")"
    dpt="$(sed -n 's/.*DPT=\([0-9]*\).*/\1/p' <<<"$line")"
    [ -n "$spt" ] || continue
    flag=-t
    [ "$proto" = UDP ] && flag=-u
    pid="$(ss -H -n -p "$flag" -a "sport = :$spt" 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)"
    if [ -n "$pid" ] && [ -r "/proc/$pid/cgroup" ]; then
        cg="$(cut -d: -f3 "/proc/$pid/cgroup")"
        comm="$(cat "/proc/$pid/comm" 2>/dev/null)"
        named=$((named + 1))
    else
        pid=- cg=- comm=-
        unnamed=$((unnamed + 1))
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "${proto:-?}" "$spt" "$dpt" "$pid" "$comm" "$cg" >> "$out"
done < <(journalctl -k -f -o short-unix --since now)
