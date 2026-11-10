#!/usr/bin/env bash
# `k2 commit` asks the app to open an AI commit session. The daemon's
# /cli/commit and /cli/commit-merge only emit CliAiCommit (the app's sink
# opens the session) and answer with the action. Moved here from the old
# tests/cli-integration-test.sh section 12a, its one live check that no
# other test covered.
#
# Sandbox daemon under a temp HOME (worktree build), air-gapped. No app is
# running, so the event has no listener and no session opens.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }

for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

REPO="$SANDBOX_HOME/work/acme-repo"
mkdir -p "$REPO"
git -C "$REPO" init -q -b main
git -C "$REPO" -c user.name=Acme -c user.email=acme@example.com commit -q --allow-empty -m init
echo "change" >"$REPO/file.txt"

k2() {
    env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 K2_PORT="$K2SO_PORT" \
        K2_PROJECT_PATH="$REPO" "$K2_CLI" "$@"
}
pass=0
fail() { echo "FAIL: $*" >&2; echo "--- daemon log tail" >&2; tail -30 "$SANDBOX_HOME/daemon.log" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

# The CLI prints one progress line, then the daemon's JSON answer.
# $1 = the CLI output, $2 = the action the answer must name.
check_answer() {
    python3 - "$1" "$2" <<'PY'
import json, sys
out, want = sys.argv[1], sys.argv[2]
lines = out.splitlines()
assert len(lines) >= 2, f"want a progress line and a JSON answer, got: {out!r}"
d = json.loads("\n".join(lines[1:]))
assert d["success"] is True, d
assert d["action"] == want, d
PY
}

echo "== k2 commit -m =="
out="$(k2 commit -m "Acme commit message" 2>&1)" || fail "k2 commit exited non-zero: $out"
case "$out" in "Requesting AI Commit..."*) ;; *) fail "progress line: $out" ;; esac
check_answer "$out" commit || fail "k2 commit answer: $out"
ok "k2 commit -m: the daemon answers success with action commit"

echo "== k2 commit --merge -m =="
out="$(k2 commit --merge -m "Acme merge message" 2>&1)" || fail "k2 commit --merge exited non-zero: $out"
case "$out" in "Requesting AI Commit & Merge..."*) ;; *) fail "progress line: $out" ;; esac
check_answer "$out" commit-merge || fail "k2 commit --merge answer: $out"
ok "k2 commit --merge -m: the daemon answers success with action commit-merge"

echo "== k2 commit-merge stays retired =="
rc=0
out="$(k2 commit-merge -m "x" 2>&1)" || rc=$?
[ "$rc" -ne 0 ] || fail "k2 commit-merge must fail: $out"
case "$out" in *"no longer supported"*"k2 commit --merge"*) ;; *) fail "commit-merge pointer: $out" ;; esac
ok "k2 commit-merge fails and points at k2 commit --merge"

echo "ai_commit_emit: $pass passed"
