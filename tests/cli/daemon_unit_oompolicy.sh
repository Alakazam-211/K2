#!/usr/bin/env bash
# Every systemd unit K2 writes or ships for the daemon carries
# OOMPolicy=continue in its [Service] section. Agents run in the daemon's
# cgroup; with the default OOMPolicy=stop one OOM-killed agent makes systemd
# stop and restart the whole daemon (verified on the build box).
#
# Static: for each writer, take the unit text whose ExecStart runs the
# daemon and check the [Service] section (up to [Install]) holds the line.
# The stage H drop-in (k2_root_proto::templates::handoff_dropin) is
# covered by its Rust tests.
# Run with: bash tests/cli/daemon_unit_oompolicy.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

# $1 file, $2 ERE matching the daemon's ExecStart line.
check() {
    local file="$ROOT/$1" exec_re="$2" found
    [ -f "$file" ] || fail "missing $1"
    found="$(awk -v exec_re="$exec_re" '
        /^\[Service\]$/ { in_svc = 1; has_exec = 0; has_oom = 0; next }
        in_svc && $0 ~ exec_re { has_exec = 1 }
        in_svc && /^OOMPolicy=continue$/ { has_oom = 1 }
        in_svc && /^OOMPolicy=/ && !/^OOMPolicy=continue$/ { bad = 1 }
        in_svc && /^\[Install\]$/ {
            if (has_exec) { units++; if (has_oom) good++ }
            in_svc = 0
        }
        END { printf "%d %d %d\n", units + 0, good + 0, bad + 0 }
    ' "$file")"
    read -r units good bad <<<"$found"
    [ "$units" -ge 1 ] || fail "$1: no daemon unit found (ExecStart =~ $exec_re)"
    [ "$bad" -eq 0 ] || fail "$1: a daemon unit sets an OOMPolicy other than continue"
    [ "$good" -eq "$units" ] || fail "$1: $((units - good)) of $units daemon unit(s) lack OOMPolicy=continue"
    ok "$1 ($units daemon unit(s))"
}

echo "== daemon units carry OOMPolicy=continue =="
check scripts/provision-k2-server.sh '^ExecStart=.*/k2-daemon$'
check scripts/bootstrap-k2-dedicated.sh '^ExecStart=.*/k2-daemon$'
check scripts/install-daemon.sh '^ExecStart=[$]BIN_PATH$'
check cli/k2 '^ExecStart=[$]bin_path$'
check packaging/arch/k2-daemon.service '^ExecStart=/usr/bin/k2-daemon$'
check scripts/package-daemon-deb.sh '^ExecStart=/usr/bin/k2-daemon$'

echo "PASS ($pass checks)"
