#!/usr/bin/env bash
# k2 compute's duration and size parsers (K2 Engine report 2026-11-04:
# `--timeout 1h30m` died with "value too great for base"). Compound
# durations add up; anything that isn't digits plus a unit is the usage
# error, and never reaches shell arithmetic.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
awk '/^_compute_secs\(\) \{/{f=1} /^_compute_bytes\(\) \{/{f=1} f{print} f&&/^\}/{f=0}' "$ROOT/cli/k2" > "$TMP/fns.sh"
. "$TMP/fns.sh"
fail=0
eq() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fail=1; fi; }
bad() { local out; out="$( ( "$2" "$3" ) 2>&1 )"; case "$out" in usage:*) echo "ok   $1" ;; *) echo "FAIL $1: $out"; fail=1 ;; esac; }
eq "90m" "$(_compute_secs 90m)" 5400
eq "1h30m" "$(_compute_secs 1h30m)" 5400
eq "1h30m15s" "$(_compute_secs 1h30m15s)" 5415
eq "2h" "$(_compute_secs 2h)" 7200
eq "300s" "$(_compute_secs 300s)" 300
eq "plain 300" "$(_compute_secs 300)" 300
eq "leading zero 08m" "$(_compute_secs 08m)" 480
bad "1.5h refused" _compute_secs 1.5h
bad "1h30 refused" _compute_secs 1h30
bad "empty refused" _compute_secs ""
bad "an expression never runs" _compute_secs "x[\$(touch $TMP/ran)]m"
[ -e "$TMP/ran" ] && { echo "FAIL the duration ran a command"; fail=1; }
eq "16G" "$(_compute_bytes 16G)" 17179869184
eq "512m" "$(_compute_bytes 512m)" 536870912
eq "08G" "$(_compute_bytes 08G)" 8589934592
bad "1.5G refused" _compute_bytes 1.5G
bad "a size expression never runs" _compute_bytes "x[\$(touch $TMP/ran2)]G"
[ -e "$TMP/ran2" ] && { echo "FAIL the size ran a command"; fail=1; }
[ "$fail" = 0 ] && echo "compute_durations: all passed"
exit "$fail"
