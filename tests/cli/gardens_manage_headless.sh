#!/usr/bin/env bash
# Agents run Gardens end to end (0.46.0, migration 0181) against a REAL
# headless k2-daemon under a temp HOME (never the real ~/.k2, the
# production daemon or the keychain). Two roles: the owner token, and a
# credential that is not the owner's (an agent can't flip its own switch).
#
#   - `k2 sources access on|off --workspace W` writes
#     projects.agents_can_manage_gardens; any other credential is refused
#     (exit 3) and changes nothing.
#   - `k2 sources approve a b` (one click, many names) as the owner.
#   - Zen: `k2 zen garden put` writes a clean page and refuses one with
#     errors (exit 1, nothing written); a proposal an agent left in the
#     queue is shown and approved by the owner, which writes the widget file
#     and logs an Access Audit line; a stale proposal is refused (exit 1).
#
# Build first:  cargo build -p k2-daemon

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon)" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }
command -v sqlite3 >/dev/null || { echo "FAIL: sqlite3 is required" >&2; exit 1; }

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2gm-XXXXXX)"
DAEMON_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    sleep 0.3
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    if [ "$fail" -ne 0 ]; then
        echo "--- daemon log tail ---" >&2
        tail -30 "$SANDBOX/daemon.log" >&2 || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

DHOME="$SANDBOX/home"
mkdir -p "$DHOME/tmp" "$DHOME/run"
k2() { HOME="$DHOME" "$K2" "$@"; }
run() {
    set +e
    k2 "$@" >"$SANDBOX/out" 2>"$SANDBOX/err"
    rc=$?
    set -e
}

echo "== sandbox daemon =="
TMPDIR="$DHOME/tmp" XDG_RUNTIME_DIR="$DHOME/run" HOME="$DHOME" K2_AIRGAP=1 "$DAEMON" > "$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 150); do
    [ -s "$DHOME/.k2/heartbeat.port" ] && [ -s "$DHOME/.k2/heartbeat.token" ] && break
    sleep 0.2
done
[ -s "$DHOME/.k2/heartbeat.port" ] || { bad "daemon wrote its port"; exit 1; }
PORT="$(cat "$DHOME/.k2/heartbeat.port")"
OWNER_TOKEN="$(cat "$DHOME/.k2/heartbeat.token")"
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$PORT/boot-status" | grep -q '"ready"' && break
    sleep 0.2
done
ok "daemon up on $PORT"
DB=""
for f in "$DHOME/.k2/k2.db" "$DHOME/.k2/k2so.db"; do [ -s "$f" ] && DB="$f" && break; done
[ -n "$DB" ] || { bad "found the sandbox DB"; exit 1; }
sql() { sqlite3 -cmd ".timeout 5000" "$DB" "$1"; }
expect_eq "migration 0181 applied" "$(sql "SELECT COUNT(*) FROM _migrations WHERE name='0181_gardens_manage'")" "1"

WS="$SANDBOX/ws"
mkdir -p "$WS"
git -C "$WS" -c init.defaultBranch=main init -q
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$WS")" \
    "http://127.0.0.1:$PORT/cli/projects/add-from-path?token=$OWNER_TOKEN")"
PID="$(printf '%s' "$reg" | python3 -c 'import json,sys; v=json.load(sys.stdin); print((v.get("project") or v).get("id",""))')"
[ -n "$PID" ] || { bad "workspace registered ($reg)"; exit 1; }
ok "workspace $PID"

echo "== the switch: owner only =="
expect_eq "off by default" "$(sql "SELECT agents_can_manage_gardens FROM projects WHERE id='$PID'")" "0"
run sources access on --workspace "$WS"
expect_eq "access on exit 0" "$rc" "0"
expect_has "access on output" "$(cat "$SANDBOX/out")" "is ON for"
expect_eq "column on" "$(sql "SELECT agents_can_manage_gardens FROM projects WHERE id='$PID'")" "1"
run sources access off --workspace "$WS"
expect_eq "column off" "$(sql "SELECT agents_can_manage_gardens FROM projects WHERE id='$PID'")" "0"
# The second role: a credential that isn't the owner token.
code="$(curl -s -o "$SANDBOX/body" -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
    --data-raw "{\"workspace\":\"$WS\",\"toggle\":\"gardens_manage\",\"value\":true}" \
    "http://127.0.0.1:$PORT/cli/agent-access/set?token=not-the-owner")"
case "$code" in 401|403) ok "a non-owner credential is refused ($code)" ;; *) bad "a non-owner credential is refused (got $code: $(cat "$SANDBOX/body"))" ;; esac
expect_eq "a refused flip changes nothing" "$(sql "SELECT agents_can_manage_gardens FROM projects WHERE id='$PID'")" "0"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/sources/pause?token=$OWNER_TOKEN")"
expect_eq "a mutating sources route refuses GET (405)" "$code" "405"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/garden/put?token=$OWNER_TOKEN")"
expect_eq "garden/put refuses GET (405)" "$code" "405"

echo "== sources: one click, many names =="
HASH="$(printf '7%.0s' $(seq 1 64))"
add_source() {
    sql "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, agent_access, keep, status, made_by, made_by_project, rev, created_at, updated_at) VALUES ('id-$1', '$1', 'Label $1', '{\"fetch\":{\"url\":\"https://example.com/x\"}}', '$HASH', '[\"https://example.com:443\"]', 60, 'member', 'values', 'values', 'proposed', 'agent:$PID', '$PID', 1, 0, 0);"
}
add_source a-src
add_source b-src
run sources approve a-src b-src
expect_eq "batch approve exit 0" "$rc" "0"
expect_has "batch approve output" "$(cat "$SANDBOX/out")" "approved: a-src, b-src"
expect_eq "both active" "$(sql "SELECT group_concat(status) FROM (SELECT status FROM sources WHERE name IN ('a-src','b-src') ORDER BY name)")" "active,active"

echo "== zen: put and the proposal queue =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$OWNER_TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
expect_has "zen set up" "$resp" '"ok":true'
G1="$(k2 zen garden list --json | python3 -c 'import json,sys; v=json.load(sys.stdin); print([g["id"] for g in v["gardens"] if g["name"]=="Garden 1"][0])')"
[ -n "$G1" ] && ok "Garden 1 is $G1" || bad "Garden 1 id"
GFILE="$DHOME/.k2/zen/gardens/$G1.toml"
printf 'schema = 1\ntemplate = "k2.texting@1"\n\n[colors.light]\naccent = "#123456"\n' > "$SANDBOX/page.toml"
run zen garden put "$G1" --from "$SANDBOX/page.toml"
expect_eq "garden put exit 0" "$rc" "0"
expect_has "garden put output" "$(cat "$SANDBOX/out")" "Wrote gardens/$G1.toml"
expect_eq "the page landed" "$(cat "$GFILE")" "$(cat "$SANDBOX/page.toml")"
printf 'schema = 1\nnot_a_key = 3\n' > "$SANDBOX/bad.toml"
run zen garden put "$G1" --from "$SANDBOX/bad.toml"
expect_eq "a page with errors exits 1" "$rc" "1"
expect_has "nothing was written" "$(cat "$SANDBOX/err")" "nothing was written"
expect_eq "the good page stays" "$(cat "$GFILE")" "$(cat "$SANDBOX/page.toml")"
run zen proposals
expect_has "no proposals yet" "$(cat "$SANDBOX/out")" "No Garden proposals waiting."

# An agent without the switch proposed a new widget file (the row its put
# would write; empty base hash = the file didn't exist).
sql "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, note, status, made_by, made_by_project, made_by_session, created_at) VALUES ('zp-test1', 'widget', 'board', 'index.html', '<p>board</p>', '', 'first cut', 'pending', 'agent:$PID', '$PID', 'sess-x', 1);"
run zen proposals
expect_has "the proposal is listed" "$(cat "$SANDBOX/out")" "zp-test1"
expect_has "with its path" "$(cat "$SANDBOX/out")" "widgets/board/index.html"
run zen proposals show zp-test1
expect_has "show prints the content" "$(cat "$SANDBOX/out")" "<p>board</p>"
run zen proposals approve zp-test1
expect_eq "approve exit 0" "$rc" "0"
expect_has "approved" "$(cat "$SANDBOX/out")" "zp-test1: approved"
expect_eq "the widget file landed" "$(cat "$DHOME/.k2/zen/widgets/board/index.html")" "<p>board</p>"
expect_eq "the row is closed" "$(sql "SELECT status || ':' || decided_by FROM zen_proposals WHERE id='zp-test1'")" "approved:owner"
expect_has "Access Audit line" "$(cat "$DHOME/.k2/auth-audit.jsonl")" "zen-proposal-approve"
run zen proposals approve zp-test1
expect_eq "approving twice exits 1" "$rc" "1"
sql "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, status, made_by, made_by_project, created_at) VALUES ('zp-stale', 'garden', '$G1', NULL, 'schema = 1', 'deadbeef', 'pending', 'agent:$PID', '$PID', 2);"
run zen proposals approve zp-stale
expect_eq "a stale proposal exits 1" "$rc" "1"
expect_has "proposal_stale" "$(cat "$SANDBOX/err")" "proposal_stale"
expect_eq "the page is untouched" "$(cat "$GFILE")" "$(cat "$SANDBOX/page.toml")"
run zen proposals reject zp-stale
expect_has "reject" "$(cat "$SANDBOX/out")" "zp-stale: rejected"
run zen proposals show zp-nope
expect_eq "an unknown proposal exits 4" "$rc" "4"

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
