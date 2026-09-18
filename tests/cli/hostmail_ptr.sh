#!/usr/bin/env bash
# hostmail ptr CLI wiring — no daemon / no OVH.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

# Subcommand + routes.
grep -q 'cmd_hostmail_ptr' "$K2" || fail "cmd_hostmail_ptr missing"
grep -A30 'verb == "ptr_show"' "$K2" | grep -q '/cli/mail/ptr' \
  || fail "ptr_show must GET /cli/mail/ptr"
grep -A20 'verb == "ptr_set"' "$K2" | grep -q '/cli/mail/ptr/set' \
  || fail "ptr_set must POST /cli/mail/ptr/set"

# Help + schema mention ptr.
help="$("$K2" hostmail ptr --help)"
printf '%s' "$help" | grep -q 'ptr show' || fail "ptr --help must mention show"
printf '%s' "$help" | grep -q 'ptr set' || fail "ptr --help must mention set"

show_help="$("$K2" hostmail ptr show --help)"
printf '%s' "$show_help" | grep -q '/cli/mail/ptr' \
  || fail "ptr show --help must name /cli/mail/ptr"

set_help="$("$K2" hostmail ptr set --help)"
printf '%s' "$set_help" | grep -q '/cli/mail/ptr/set' \
  || fail "ptr set --help must name /cli/mail/ptr/set"

# Schema JSON.
"$K2" --schema 2>/dev/null | grep -q '"name": "hostmail ptr show"' \
  || fail "schema missing hostmail ptr show"
"$K2" --schema 2>/dev/null | grep -q '"name": "hostmail ptr set"' \
  || fail "schema missing hostmail ptr set"

# Usage without hostname exits 2.
set +e
out="$("$K2" hostmail ptr set 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "ptr set without hostname must exit 2, got $rc: $out"
printf '%s' "$out" | grep -qi 'hostname' || fail "ptr set usage must name hostname: $out"

# k2 mail ptr moved to hostmail (exit 2).
set +e
out="$("$K2" mail ptr 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || fail "k2 mail ptr must exit 2, got $rc ($out)"
printf '%s' "$out" | grep -q "hostmail" || fail "k2 mail ptr must point at k2 hostmail ($out)"

# CLI keeps TCP for mail (quota pattern) — _uds_eligible omits /cli/mail/.
if grep -A40 '_uds_eligible()' "$K2" | grep -q '/cli/mail'; then
  fail "_uds_eligible must not glob /cli/mail (ptr stays TCP like quota)"
fi

echo "OK: hostmail ptr CLI wiring"
