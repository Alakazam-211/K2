#!/usr/bin/env bash
# The must-not-break suite (tests/contract/must-not-break.ts) through the
# server SDK against a THROWAWAY daemon on this machine: the --cmd path
# Dannon's BFFs use today (the daemon's loopback port, no App pass).
#
#   bash tests/contract/run_sandbox.sh [path/to/k2-daemon]
#
# The daemon runs from this checkout's build (default
# $CARGO_TARGET_DIR/debug/k2-daemon, else target/debug/k2-daemon) under a
# temp HOME with its own TMPDIR and runtime dir, no agent CLI it can start,
# and is killed on exit. Never the real ~/.k2, never the keychain
# (a temp HOME keeps the vault key in a file), never the running daemon.
#
# k2 db needs the workspace Postgres (Linux): with none here the three db
# checks are reported as not run, and this script allows that only off
# Linux. On Linux, and from inside a wall, tests/linux/apps_e2e_sdk.sh runs
# the full suite.
set -Eeuo pipefail
# Name the line a `set -e` exit happens on.
trap 'rc=$?; echo "FAIL: line $LINENO: $BASH_COMMAND (exit $rc)" >&2' ERR

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-}"
if [ -z "$BIN" ]; then
	for c in "${CARGO_TARGET_DIR:-$ROOT/target}/debug/k2-daemon" "$ROOT/target/debug/k2-daemon"; do
		[ -x "$c" ] && { BIN="$c"; break; }
	done
fi
[ -x "$BIN" ] || { echo "FAIL: no k2-daemon build (cargo build -p k2-daemon --bin k2-daemon)" >&2; exit 1; }
node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit(a>22||(a===22&&b>=18)?0:1)' \
	|| { echo "FAIL: node 22.18+ runs the TypeScript suite as is (have $(node --version))" >&2; exit 1; }

T="$(mktemp -d /tmp/k2c.XXXXXX)"
DPID=""
cleanup() {
	if [ -n "$DPID" ]; then
		kill "$DPID" 2>/dev/null || true
		sleep 0.3
		kill -9 "$DPID" 2>/dev/null || true
	fi
	rm -rf "$T"
}
trap cleanup EXIT
mkdir -p "$T/home" "$T/shim" "$T/tmp" "$T/run" "$T/home/room"
# The shim dir stays EMPTY: no agent CLI can start, the way a server box
# with no model CLI behaves. A guest's post is stored and its wake fails
# quietly, so the pinned conversation the fixture seeds stays pinned (a
# shim that "starts" would mint a fresh conversation and re-pin).

env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$T/home" TMPDIR="$T/tmp" XDG_RUNTIME_DIR="$T/run" \
	K2_TEST_AGENT_SHIM_DIR="$T/shim" "$BIN" >"$T/daemon.log" 2>&1 &
DPID=$!
for _ in $(seq 1 150); do
	if [ -s "$T/home/.k2/heartbeat.port" ] && curl -sf "http://127.0.0.1:$(cat "$T/home/.k2/heartbeat.port")/boot-status" 2>/dev/null | grep -q '"ready"'; then
		break
	fi
	sleep 0.2
done
[ -s "$T/home/.k2/heartbeat.port" ] || { tail -n 40 "$T/daemon.log" >&2; echo "FAIL: the sandbox daemon never came up" >&2; exit 1; }
PORT="$(cat "$T/home/.k2/heartbeat.port")"
TOKEN="$(cat "$T/home/.k2/heartbeat.token")"
owner() { curl -sf -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:$PORT$1?token=$TOKEN"; }

ROOM="$(cd "$T/home/room" && pwd -P)"
owner /cli/projects/add-without-git "{\"path\":\"$ROOM\"}" >/dev/null || { echo "FAIL: register the room" >&2; exit 1; }
DB=""
for f in "$T/home/.k2/k2.db" "$T/home/.k2/k2so.db"; do [ -f "$f" ] && { DB="$f"; break; }; done
[ -n "$DB" ] || { ls -la "$T/home/.k2" >&2; echo "FAIL: no daemon database under $T/home/.k2" >&2; exit 1; }
# (The seed script goes through a file: macOS bash 3.2 misparses a heredoc
# inside a process substitution.)
cat >"$T/seed.py" <<'PY'
import sqlite3, sys, uuid
db = sqlite3.connect(sys.argv[1], timeout=30)
row = db.execute("SELECT id, handle FROM projects WHERE path = ?", (sys.argv[2],)).fetchone()
if not row or not row[1]:
    rows = db.execute("SELECT id, handle, path FROM projects").fetchall()
    sys.exit(f"no registered workspace with a handle at {sys.argv[2]}; projects: {rows}")
conv = str(uuid.uuid4())
db.execute(
    "INSERT INTO workspace_sessions (id, project_id, terminal_id, session_id, harness, owner, status, created_at) VALUES (?, ?, NULL, ?, 'claude', 'system', 'running', unixepoch()) "
        "ON CONFLICT(project_id) DO UPDATE SET session_id = excluded.session_id, harness = 'claude', status = 'running'",
    (f"ws-{conv}", row[0], conv),
)
db.commit()
# A transcript for the pinned conversation (as Claude names it), so a
# guest's post wakes the agent onto THIS conversation instead of minting a
# fresh one and re-pinning (a real workspace's pinned Chat has one).
import os
real = os.path.realpath(sys.argv[2])
slug = "".join(c if (c.isascii() and c.isalnum()) or c == "-" else "-" for c in real)
d = os.path.join(os.environ["K2C_HOME"], ".claude", "projects", slug)
os.makedirs(d, exist_ok=True)
with open(os.path.join(d, f"{conv}.jsonl"), "w") as f:
    f.write('{"type":"summary","summary":"contract fixture"}\n')
print(row[0], row[1])
PY
read -r ROOM_ID ROOM_HANDLE < <(K2C_HOME="$T/home" python3 "$T/seed.py" "$DB" "$ROOM")
[ -n "${ROOM_HANDLE:-}" ] || { echo "FAIL: the room's handle" >&2; exit 1; }

PW="contract-horse-$RANDOM-staple"
owner /cli/skin/roles '{"name":"contract"}' >/dev/null
owner /cli/skin/roles/room "{\"name\":\"contract\",\"handle\":\"$ROOM_HANDLE\",\"caps\":[\"thread:read\",\"thread:post\",\"files:read\",\"files:write\",\"store:read\",\"store:write\"]}" >/dev/null
for u in dana locky; do
	owner /cli/skin/users "{\"username\":\"$u\",\"email\":\"$u@contract.test\"}" >/dev/null || { echo "FAIL: guest $u" >&2; exit 1; }
	owner /cli/skin/users/password "{\"username\":\"$u\",\"password\":\"$PW\"}" >/dev/null
	owner /cli/skin/roles/assign "{\"username\":\"$u\",\"role\":\"contract\"}" >/dev/null
done
CARD="$(owner /cli/thread/ask "{\"addr\":\"$ROOM_HANDLE\",\"prompt\":\"Ship it?\",\"options\":\"Yes,No\"}" \
	| python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("id") or v["card"]["id"])')"

python3 - "$ROOM_HANDLE" "$ROOM_ID" "$PW" "$CARD" >"$T/fixture.json" <<'PY'
import json, sys
handle, rid, pw, card = sys.argv[1:5]
print(json.dumps({
    "room": {"handle": handle, "id": rid},
    "guest": {"username": "dana", "email": "dana@contract.test", "password": pw},
    "lockUser": {"username": "locky", "email": "locky@contract.test", "password": pw},
    "card": {"id": card, "answer": "Yes"},
    "db": None,
    "expectApp": "host",
    "checkEnv": False,
}))
PY

ALLOW=()
[ "$(uname -s)" = Linux ] || ALLOW=(--allow-not-run)
set +e
env -i PATH="$PATH" HOME="$T/home" K2_GATEWAY_URL="http://127.0.0.1:$PORT" \
	node "$ROOT/tests/contract/run.ts" "$T/fixture.json" "${ALLOW[@]}" >"$T/result.json"
rc=$?
set -e
python3 - "$T/result.json" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
for r in v["results"]:
    print(("  ok   " if r["ok"] else "  FAIL ") + r["name"] + ("" if r["ok"] else f"\n         {r['detail']}"))
for n in v["notRun"]:
    print(f"  --   {n['name']} (not run: {n['why']})")
PY
if [ "$rc" != 0 ]; then
	tail -n 30 "$T/daemon.log" >&2
	echo "FAIL: the must-not-break suite (exit $rc)" >&2
	exit "$rc"
fi
echo "must-not-break: ok on the --cmd path"
