#!/usr/bin/env bash
# 0.46.4 field report: `k2 agent hire --dry-run` showed `--connect
# k2::<host>` as "would apply", then the real run failed not_found and
# stopped before --context.
#
# - the dry run asks the daemon to resolve every --connect target
#   (`/cli/connections?action=resolve`) and reports a miss up front
#   (FAILED, exit 4), and a local workspace as would-apply;
# - the real run applies every independent step even when a connect
#   fails, prints each step's result, and exits non-zero.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped (so no
# federation peer exists), no --launch, an empty agent shim dir. Loud:
# every check must pass; nothing is skipped.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
command -v jq >/dev/null || { echo "FAIL: jq is required" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1
SHIM_DIR="$(mktemp -d -t k2-hire-connect-shim-XXXXXX)"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
trap '_sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT

WORK="$SANDBOX_HOME/work"
mkdir -p "$WORK"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" "$K2_CLI" "$@"
}

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }
step() { jq -c --arg s "$1" --arg p "$2" '[.actions[] | select(.step == $s and ((.peer // .context // "") == $p))] | first'; }

echo "== a peer to connect to =="
peer="$WORK/peer-$RANDOM$RANDOM"
k2 agent hire "$peer" --json >/dev/null || fail "peer hire failed"
ok "peer hired"

echo "== the daemon resolves targets (read only, GET) =="
code="$(curl -s -o "$SANDBOX_HOME/r.json" -w '%{http_code}' \
    "http://127.0.0.1:${K2SO_PORT}/cli/connections?action=resolve&target=k2%3A%3Anowhere.k2.dev&token=${K2SO_TOKEN}")"
[ "$code" = "404" ] || fail "remote resolve want 404 got $code: $(cat "$SANDBOX_HOME/r.json")"
[ "$(jq -r .error.code "$SANDBOX_HOME/r.json")" = "not_found" ] || fail "want not_found: $(cat "$SANDBOX_HOME/r.json")"
code="$(curl -s -o "$SANDBOX_HOME/r.json" -w '%{http_code}' -G \
    --data-urlencode "action=resolve" --data-urlencode "target=$peer" --data-urlencode "token=${K2SO_TOKEN}" \
    "http://127.0.0.1:${K2SO_PORT}/cli/connections")"
[ "$code" = "200" ] || fail "local resolve want 200 got $code: $(cat "$SANDBOX_HOME/r.json")"
[ "$(jq -r .kind "$SANDBOX_HOME/r.json")" = "local" ] || fail "want kind local: $(cat "$SANDBOX_HOME/r.json")"
[ "$(jq -r .path "$SANDBOX_HOME/r.json")" = "$peer" ] || fail "want the peer path: $(cat "$SANDBOX_HOME/r.json")"
ok "resolve: remote miss 404 not_found, local hit 200"

echo "== dry run reports the miss up front =="
dir="$WORK/hire-$RANDOM$RANDOM"
mkdir -p "$dir"
printf '# Notes\n' >"$dir/notes.md"
set +e
out="$(k2 agent hire "$dir" --connect "k2::nowhere.k2.dev" --connect "$peer" --context notes.md --dry-run --json 2>"$SANDBOX_HOME/err")"
rc=$?
set -e
[ "$rc" = "4" ] || fail "dry run with an unknown --connect want exit 4 got $rc: $out $(cat "$SANDBOX_HOME/err")"
[ "$(jq -r .ok <<<"$out")" = "false" ] || fail "dry run ok must be false: $out"
s="$(step connect "k2::nowhere.k2.dev" <<<"$out")"
[ "$(jq -r .status <<<"$s")" = "failed" ] || fail "remote connect must be FAILED in the plan: $s"
[ "$(jq -r .error.code <<<"$s")" = "not_found" ] || fail "remote connect code: $s"
s="$(step connect "$peer" <<<"$out")"
[ "$(jq -r .status <<<"$s")" = "pending" ] || fail "local connect would apply: $s"
grep -q '"code": *"not_found"' "$SANDBOX_HOME/err" || fail "stderr names not_found: $(cat "$SANDBOX_HOME/err")"
k2 agent conf "$dir" --json >/dev/null 2>&1 && fail "a dry run registered the workspace"
ok "dry run: FAILED not_found for k2::nowhere.k2.dev, would-apply for the local peer, exit 4"

set +e
human="$(k2 agent hire "$dir" --connect "k2::nowhere.k2.dev" --dry-run 2>/dev/null)"
set -e
grep -q '\[FAILED\] *connect: k2::nowhere.k2.dev' <<<"$human" || fail "text plan shows FAILED: $human"
grep -q 'would fail' <<<"$human" || fail "text plan says it would fail: $human"
ok "text plan shows the FAILED step"

echo "== real run: the other steps still apply =="
set +e
out="$(k2 agent hire "$dir" --connect "k2::nowhere.k2.dev" --connect "$peer" --context notes.md --json 2>"$SANDBOX_HOME/err")"
rc=$?
set -e
[ "$rc" = "4" ] || fail "real run with a failed connect want exit 4 got $rc: $out $(cat "$SANDBOX_HOME/err")"
[ "$(jq -r '.ok' <<<"$out")" = "false" ] || fail "ok must be false: $out"
[ "$(jq -r '[.actions[] | select(.step == "register-workspace")][0].status' <<<"$out")" = "done" ] || fail "register: $out"
[ "$(jq -r .status <<<"$(step connect "k2::nowhere.k2.dev" <<<"$out")")" = "failed" ] || fail "remote connect failed: $out"
[ "$(jq -r .status <<<"$(step connect "$peer" <<<"$out")")" = "done" ] || fail "local connect done: $out"
[ "$(jq -r .status <<<"$(step seed-context notes.md <<<"$out")")" = "done" ] || fail "--context ran after the failed connect: $out"
conf="$(k2 agent conf "$dir" --json)"
[ "$(jq -r '[.connections[]?] | length' <<<"$conf")" = "1" ] || fail "one connection (the local peer): $conf"
ok "real run: connect + context applied, the failed connect reported, exit 4"

echo ""
echo "Results: $pass passed, 0 failed"
