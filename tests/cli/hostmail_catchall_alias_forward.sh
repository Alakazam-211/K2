#!/usr/bin/env bash
# hostmail catchall/alias/forward: CLI siblings of quota; k2 mail * moved.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'M_ALIAS="${M_ALIAS:-}"' "$K2" || fail "_mail_py must export M_ALIAS"
grep -q 'M_MAILBOX="${M_MAILBOX:-}"' "$K2" || fail "_mail_py must export M_MAILBOX"
grep -q 'M_KEEP="${M_KEEP:-false}"' "$K2" || fail "_mail_py must export M_KEEP"

help="$("$K2" hostmail alias --help)"
printf '%s' "$help" | grep -q 'k2 hostmail alias add' || fail "alias --help must be leaf help: $help"
if printf '%s' "$help" | grep -q 'k2 hostmail <command>'; then
    fail "k2 hostmail alias --help must not be group help: $help"
fi
printf '%s' "$help" | grep -q 'create' || fail "alias help must teach create mints: $help"

group="$("$K2" hostmail --help)"
printf '%s' "$group" | grep -q 'catchall' || fail "hostmail --help must list catchall: $group"
printf '%s' "$group" | grep -q 'alias' || fail "hostmail --help must list alias: $group"
printf '%s' "$group" | grep -q 'forward' || fail "hostmail --help must list forward: $group"

fwd_help="$("$K2" hostmail forward set --help)"
printf '%s' "$fwd_help" | grep -q -- '--keep' || fail "forward set --help must mention --keep"

ca_help="$("$K2" hostmail catchall set --help)"
printf '%s' "$ca_help" | grep -q -- '--address' || fail "catchall set --help must mention --address"

set +e
mail_alias_out="$("$K2" mail alias add a@b.test sales@b.test 2>&1)"
mail_alias_rc=$?
set -e
if [ "$mail_alias_rc" -eq 0 ]; then
    fail "k2 mail alias must exit 2 moved-to-hostmail, got 0: $mail_alias_out"
fi
if ! printf '%s' "$mail_alias_out" | grep -q "moved to 'k2 hostmail alias'"; then
    fail "k2 mail alias must say moved to hostmail: $mail_alias_out"
fi

set +e
mail_ca_out="$("$K2" mail catchall show example.test 2>&1)"
mail_ca_rc=$?
set -e
if [ "$mail_ca_rc" -eq 0 ]; then
    fail "k2 mail catchall must exit 2, got 0: $mail_ca_out"
fi
if ! printf '%s' "$mail_ca_out" | grep -q "moved to 'k2 hostmail catchall'"; then
    fail "k2 mail catchall must say moved to hostmail: $mail_ca_out"
fi

set +e
mail_fw_out="$("$K2" mail forward set a@b.test --to b@b.test 2>&1)"
mail_fw_rc=$?
set -e
if [ "$mail_fw_rc" -eq 0 ]; then
    fail "k2 mail forward must exit 2, got 0: $mail_fw_out"
fi
if ! printf '%s' "$mail_fw_out" | grep -q "moved to 'k2 hostmail forward'"; then
    fail "k2 mail forward must say moved to hostmail: $mail_fw_out"
fi

# Parser smoke: missing dest / missing address must exit 2 without a daemon.
set +e
out="$(mktemp -t k2-hostmail-caf-XXXXXX)"
K2SO_PORT=1 K2_PORT=1 K2_HOOK_TOKEN=x "$K2" hostmail catchall set example.test >"$out" 2>&1
code=$?
set -e
if [ "$code" -eq 0 ]; then
    fail "catchall set without --address must exit 2: $(cat "$out")"
fi
grep -q 'address' "$out" || fail "catchall set usage must name --address: $(cat "$out")"

echo "OK: hostmail catchall/alias/forward CLI"
