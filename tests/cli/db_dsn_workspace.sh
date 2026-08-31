#!/usr/bin/env bash
# R10: k2 db dsn / k2 store accept --workspace|--project (owner-token identity).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

export K2_PORT=1 K2SO_PORT=1 K2_HOOK_TOKEN=x

help_db="$("$K2" db --help)"
echo "$help_db" | grep -q -- '--workspace' || fail "k2 db --help must mention --workspace"
echo "$help_db" | grep -E 'dsn \| creds \[--json\] \[--workspace' >/dev/null \
    || fail "dsn usage must be: dsn | creds [--json] [--workspace <ws>]"

help_store="$("$K2" store --help)"
echo "$help_store" | grep -q -- '--workspace' || fail "k2 store --help must mention --workspace"

# Missing value is usage (exit 2) before any daemon call.
set +e
out="$("$K2" db dsn --workspace 2>&1)"
ec=$?
set -e
[ "$ec" -eq 2 ] || fail "k2 db dsn --workspace (no value) must exit 2, got $ec: $out"
echo "$out" | grep -qi 'workspace' || fail "missing --workspace value must mention workspace: $out"

set +e
out="$("$K2" store list --workspace 2>&1)"
ec=$?
set -e
[ "$ec" -eq 2 ] || fail "k2 store list --workspace (no value) must exit 2, got $ec: $out"

# Flag is stripped at cmd_db start so dsn's parser never sees --workspace.
set +e
out="$("$K2" db dsn --workspace /tmp --bogus 2>&1)"
ec=$?
set -e
[ "$ec" -eq 2 ] || fail "k2 db dsn --workspace /tmp --bogus must exit 2, got $ec: $out"
echo "$out" | grep -q -- "--workspace" && fail "stripped --workspace must not be 'unexpected': $out"
echo "$out" | grep -q -- "--bogus" || fail "dsn parser should reject leftover --bogus: $out"

set +e
out="$("$K2" store list --workspace /tmp --bogus 2>&1)"
ec=$?
set -e
[ "$ec" -eq 2 ] || fail "k2 store list --workspace /tmp --bogus must exit 2, got $ec: $out"
echo "$out" | grep -q -- "--workspace" && fail "store must strip --workspace: $out"
echo "$out" | grep -q -- "--bogus" || fail "store parser should reject leftover --bogus: $out"

echo "PASS: db dsn / store --workspace"
