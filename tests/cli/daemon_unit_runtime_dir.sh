#!/usr/bin/env bash
# Every systemd unit K2 writes or ships for the daemon carries its own
# runtime dir in the [Service] section:
#   RuntimeDirectory=k2
#   RuntimeDirectoryMode=0700
#   RuntimeDirectoryPreserve=yes
# The daemon keeps frpc.toml and agents' secret files there
# ($RUNTIME_DIRECTORY = /run/k2 for a system unit). Without it the daemon
# falls back to /run/user/<uid>/k2 or /dev/shm/k2-<uid>, which
# systemd-logind deletes when the user's last login session ends (no
# linger, RemoveIPC=yes): akzm, 2026-10-24. Preserve=yes because frpc and
# k2-ptyd agents outlive a daemon stop under KillMode=process.
#
# Static, like daemon_unit_oompolicy.sh: for each writer, the unit text
# whose ExecStart runs the daemon. The stage H drop-in
# (k2_root_proto::templates::handoff_dropin) is covered by its Rust tests.
# Run with: bash tests/cli/daemon_unit_runtime_dir.sh
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
        /^\[Service\]$/ { in_svc = 1; has_exec = 0; rd = 0; mode = 0; keep = 0; next }
        in_svc && $0 ~ exec_re { has_exec = 1 }
        in_svc && /^RuntimeDirectory=k2$/ { rd = 1 }
        in_svc && /^RuntimeDirectory=/ && !/^RuntimeDirectory=k2$/ { bad = 1 }
        in_svc && /^RuntimeDirectoryMode=0700$/ { mode = 1 }
        in_svc && /^RuntimeDirectoryPreserve=yes$/ { keep = 1 }
        in_svc && /^RuntimeDirectoryPreserve=/ && !/^RuntimeDirectoryPreserve=yes$/ { bad = 1 }
        in_svc && /^\[Install\]$/ {
            if (has_exec) { units++; if (rd && mode && keep) good++ }
            in_svc = 0
        }
        END { printf "%d %d %d\n", units + 0, good + 0, bad + 0 }
    ' "$file")"
    read -r units good bad <<<"$found"
    [ "$units" -ge 1 ] || fail "$1: no daemon unit found (ExecStart =~ $exec_re)"
    [ "$bad" -eq 0 ] || fail "$1: a daemon unit sets another RuntimeDirectory= or RuntimeDirectoryPreserve="
    [ "$good" -eq "$units" ] || fail "$1: $((units - good)) of $units daemon unit(s) lack RuntimeDirectory=k2 / Mode=0700 / Preserve=yes"
    ok "$1 ($units daemon unit(s))"
}

echo "== daemon units carry RuntimeDirectory=k2 (0700, preserved) =="
check scripts/provision-k2-server.sh '^ExecStart=.*/k2-daemon$'
check scripts/bootstrap-k2-dedicated.sh '^ExecStart=.*/k2-daemon$'
check scripts/install-daemon.sh '^ExecStart=[$]BIN_PATH$'
check cli/k2 '^ExecStart=[$]bin_path$'
check packaging/arch/k2-daemon.service '^ExecStart=/usr/bin/k2-daemon$'
check scripts/package-daemon-deb.sh '^ExecStart=/usr/bin/k2-daemon$'

echo "PASS ($pass checks)"
