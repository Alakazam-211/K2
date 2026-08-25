#!/usr/bin/env bash
# Loud unit around cli/k2 air-gap refuses. No network: the helper must
# exit before curl/GitHub. Fail the suite if the teaching error is missing
# or if a password prompt/network path is reached.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
[[ -x "$CLI" ]] || { echo "missing executable $CLI" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

run_refuse() {
    local label="$1"
    shift
    local out rc
    set +e
    out="$(K2_AIRGAP=1 "$CLI" "$@" 2>&1)"
    rc=$?
    set -e
    [[ "$rc" -ne 0 ]] || fail "$label: expected non-zero exit, got 0. output: $out"
    echo "$out" | grep -q 'K2_AIRGAP=1' || fail "$label: teaching error must name K2_AIRGAP=1. output: $out"
    echo "$out" | grep -qi 'air-gap' || fail "$label: teaching error must say air-gap. output: $out"
    echo "$out" | grep -qiE 'could not reach|supabase|github.com|Connection refused' && \
        fail "$label: must not network. output: $out"
    echo "ok $label"
}

run_refuse "connect login" connect login --email "nobody@example.invalid"
run_refuse "publish subdomain create" publish subdomain create staging --target localhost:1
run_refuse "daemon install" daemon install --dry-run
run_refuse "tunnel enable" tunnel enable

# Garbage env is fail-closed (block), opposite of K2_LISTEN.
out="$(K2_AIRGAP=garbage "$CLI" connect login --email "nobody@example.invalid" 2>&1)" || true
echo "$out" | grep -q 'K2_AIRGAP=1' || fail "garbage env: must refuse. output: $out"
echo "ok K2_AIRGAP=garbage refuses"

# Unset defaults off: connect login --help must still print (no refuse).
out="$(env -u K2_AIRGAP "$CLI" connect login --help 2>&1)" || true
echo "$out" | grep -qi 'usage\|email\|login' || fail "help should print when air-gap unset. output: $out"
echo "ok connect login --help with air-gap unset"

echo "airgap CLI helper tests passed"
