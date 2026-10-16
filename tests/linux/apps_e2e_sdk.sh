#!/usr/bin/env bash
# Lane 4 phase (runtime S23, §14.2 + AR40): the must-not-break contract
# through the server SDK, against the real daemon, from BOTH sides of the
# wall. Linux only: it runs AS ROOT INSIDE the throwaway VM that
# tests/linux/apps_guarded_e2e.sh runs in, after phase A has the daemon,
# box prep and the guarded engine up. Sourced by lane 1's runner, which
# then calls `apps_e2e_sdk`.
#
# What the runner provides (checked first; a missing one fails loudly):
#   IT, K2HOME, WS, DPORT, OWNER_TOKEN   (as in apps_guarded_e2e.sh)
#   K2 fail ok info                      (its helper functions)
#   $IT/src/sdk/server/                  sdk/server from this checkout
#   $IT/src/tests/contract/              tests/contract from this checkout
#   SDK_DB_TABLE (optional)              a k2 db dump table in $WS with an
#                                        RLS policy on k2.skin_uid(); when
#                                        unset this phase creates one
#
# Steps:
#   1. the room ($WS): its pinned Chat, a guest role, two guests and a
#      lock-check guest per run, a pending card, a dump table;
#   2. OUTSIDE the wall (the --cmd path Dannon's BFFs use today): the
#      suite through the SDK at the daemon's loopback port, run by node 24
#      from the pinned base image (podman, host network);
#   3. INSIDE the wall: tests/contract/fixture-app deployed guarded; its
#      GET /_contract runs the same suite through its gateway link.
#   The open path (lane 1's relaxed container) runs step 3 again with
#   `--engine open` once that engine exists (SDK_OPEN=1).

apps_e2e_sdk() {
	local v
	for v in IT K2HOME WS DPORT OWNER_TOKEN; do
		[ -n "${!v:-}" ] || { echo "FAIL apps_e2e_sdk: the runner must set $v" >&2; exit 1; }
	done
	for v in K2 fail ok info; do
		declare -F "$v" >/dev/null || { echo "FAIL apps_e2e_sdk: the runner must define $v" >&2; exit 1; }
	done
	local SRC="$IT/src"
	[ -f "$SRC/sdk/server/src/index.ts" ] || fail "apps_e2e_sdk: $SRC/sdk/server is missing (the runner copies sdk/server)"
	[ -f "$SRC/tests/contract/must-not-break.ts" ] || fail "apps_e2e_sdk: $SRC/tests/contract is missing (the runner copies tests/contract)"
	command -v podman >/dev/null || fail "apps_e2e_sdk: podman (box prep installs it)"

	echo "== lane 4: must-not-break through the server SDK (S23) =="
	local DB
	DB=""
	for v in "$K2HOME/.k2/k2.db" "$K2HOME/.k2/k2so.db"; do [ -f "$v" ] && { DB="$v"; break; }; done
	[ -n "$DB" ] || fail "apps_e2e_sdk: no daemon database under $K2HOME/.k2"
	sdk_owner() { # path json → body (fails on non-2xx)
		curl -sf -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:$DPORT$1?token=$OWNER_TOKEN"
	}

	# ── 1. the room ──────────────────────────────────────────────────
	local ROOM_ID ROOM_HANDLE
	read -r ROOM_ID ROOM_HANDLE < <(python3 - "$DB" "$WS" "$K2HOME" <<'PY'
import sqlite3, sys, uuid
db = sqlite3.connect(sys.argv[1], timeout=30)
row = db.execute("SELECT id, handle FROM projects WHERE path = ?", (sys.argv[2],)).fetchone()
if not row or not row[1]:
    rows = db.execute("SELECT id, handle, path FROM projects").fetchall()
    sys.exit(f"no registered workspace with a handle at {sys.argv[2]}; projects: {rows}")
pid, handle = row
pinned = db.execute("SELECT session_id FROM workspace_sessions WHERE project_id = ? AND session_id IS NOT NULL AND session_id != ''", (pid,)).fetchone()
conv = pinned[0] if pinned else str(uuid.uuid4())
if not pinned:
    db.execute(
        "INSERT INTO workspace_sessions (id, project_id, terminal_id, session_id, harness, owner, status, created_at) VALUES (?, ?, NULL, ?, 'claude', 'system', 'running', unixepoch()) "
        "ON CONFLICT(project_id) DO UPDATE SET session_id = excluded.session_id, harness = 'claude', status = 'running'",
        (f"ws-{conv}", pid, conv),
    )
    db.commit()
# A transcript for the pinned conversation (as Claude names it), so a
# guest's post wakes the agent onto THIS conversation instead of minting
# a fresh one and re-pinning (a real workspace's pinned Chat has one).
import os, pwd
real = os.path.realpath(sys.argv[2])
slug = "".join(c if (c.isascii() and c.isalnum()) or c == "-" else "-" for c in real)
d = os.path.join(sys.argv[3], ".claude", "projects", slug)
os.makedirs(d, exist_ok=True)
t = os.path.join(d, f"{conv}.jsonl")
if not os.path.exists(t):
    with open(t, "w") as f:
        f.write('{"type":"summary","summary":"contract fixture"}\n')
k2 = pwd.getpwnam("k2")
for root, dirs, files in os.walk(os.path.join(sys.argv[3], ".claude")):
    for n in dirs + files:
        os.chown(os.path.join(root, n), k2.pw_uid, k2.pw_gid)
os.chown(os.path.join(sys.argv[3], ".claude"), k2.pw_uid, k2.pw_gid)
print(pid, handle)
PY
	) || fail "apps_e2e_sdk: the room's id, handle and pinned Chat"
	[ -n "$ROOM_HANDLE" ] || fail "apps_e2e_sdk: room handle"
	info "room $ROOM_HANDLE ($ROOM_ID)"

	local APP=sdkc
	K2 app install "$APP" --memory 256M >/dev/null || fail "apps_e2e_sdk: k2 app install $APP"
	local SVC
	SVC="$(K2 app list --json | python3 -c "import json,sys; print([a for a in json.load(sys.stdin)['apps'] if a['appId']=='$APP'][0]['serviceId'])")" \
		|| fail "apps_e2e_sdk: the App's service id"
	K2 app role create sdk-guest --app "$APP" >/dev/null || fail "apps_e2e_sdk: role"
	K2 app role room sdk-guest "$ROOM_HANDLE" --caps thread:read,thread:post,files:read,files:write,store:read,store:write >/dev/null \
		|| fail "apps_e2e_sdk: role room"
	local PW="contract-horse-$RANDOM-staple" u
	for u in sdk-dana sdk-lock-out sdk-lock-in; do
		printf '%s' "$PW" | K2 app user add "$u" --app "$APP" --role sdk-guest --email "$u@contract.test" --password-stdin >/dev/null \
			|| fail "apps_e2e_sdk: guest $u"
	done
	ok "lane 4: the room, the App $APP ($SVC), a guest role and three guests"

	sdk_card() { # → a pending choice card id with option Yes
		sdk_owner /cli/thread/ask "{\"addr\":\"$ROOM_HANDLE\",\"prompt\":\"Ship it?\",\"options\":\"Yes,No\"}" \
			| python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("id") or v["card"]["id"])'
	}

	local TABLE="${SDK_DB_TABLE:-}"
	if [ -z "$TABLE" ]; then
		TABLE=contract_notes
		K2 db enable >/dev/null 2>&1 || true # idempotent; status below decides
		K2 db create --db-access write >/dev/null || fail "apps_e2e_sdk: k2 db create in the room (set SDK_DB_TABLE to use a prepared table)"
		local MIG="$WS/.k2/db/migrations"
		runuser -u k2 -- mkdir -p "$MIG"
		cat >"$MIG/0001_contract_notes.sql" <<'SQL'
CREATE TABLE contract_notes (
  id bigserial PRIMARY KEY,
  note text NOT NULL,
  owner uuid NOT NULL DEFAULT k2.skin_uid()
);
ALTER TABLE contract_notes ENABLE ROW LEVEL SECURITY;
CREATE POLICY contract_notes_guest ON contract_notes
  USING (owner = k2.skin_uid()) WITH CHECK (owner = k2.skin_uid());
SQL
		chown -R k2:k2 "$WS/.k2/db"
		K2 db migrate --dir .k2/db/migrations >/dev/null || fail "apps_e2e_sdk: k2 db migrate (the contract table)"
	fi
	ok "lane 4: the dump table $TABLE"

	sdk_fixture() { # lock-user card-id → fixture JSON
		python3 - "$ROOM_HANDLE" "$ROOM_ID" "$PW" "$1" "$2" "$TABLE" <<'PY'
import json, sys
handle, rid, pw, lock, card, table = sys.argv[1:7]
print(json.dumps({
    "room": {"handle": handle, "id": rid},
    "guest": {"username": "sdk-dana", "email": "sdk-dana@contract.test", "password": pw},
    "lockUser": {"username": lock, "email": f"{lock}@contract.test", "password": pw},
    "card": {"id": card, "answer": "Yes"},
    "db": {"table": table},
    "expectApp": "",
    "checkEnv": lock == "sdk-lock-in",
}))
PY
	}

	# ── 2. outside the wall: the --cmd path ─────────────────────────
	local NODE_IMAGE=docker.io/library/node:24-bookworm-slim
	local OUT=/tmp/sdk-contract-out
	rm -rf "$OUT" && mkdir -p "$OUT"
	cp -r "$SRC/sdk" "$SRC/tests" "$OUT/"
	sdk_fixture sdk-lock-out "$(sdk_card)" >"$OUT/fixture.json" || fail "apps_e2e_sdk: outside fixture"
	set +e
	podman run --rm --network=host -v "$OUT:/c:ro" -w /c \
		-e K2_GATEWAY_URL="http://127.0.0.1:$DPORT" -e K2_APP_ID="$SVC" \
		"$NODE_IMAGE" node tests/contract/run.ts fixture.json >/tmp/sdk-out.json 2>/tmp/sdk-out.err
	local rc=$?
	set -e
	[ "$rc" = 0 ] || { cat /tmp/sdk-out.err >&2; fail "apps_e2e_sdk: the must-not-break suite on the --cmd path (exit $rc)"; }
	ok "lane 4: must-not-break through the SDK on the --cmd path ($(tail -n 1 /tmp/sdk-out.err))"

	# ── 3. inside the wall ──────────────────────────────────────────
	sdk_inside() { # engine
		local F="$WS/sdk-contract-$1"
		rm -rf "$F" && mkdir -p "$F"
		cp -r "$SRC/sdk" "$SRC/tests" "$F/"
		cp "$SRC/tests/contract/fixture-app/server.mjs" "$F/server.mjs"
		printf '{"type":"module"}\n' >"$F/package.json"
		sdk_fixture sdk-lock-in "$(sdk_card)" >"$F/fixture.json" || fail "apps_e2e_sdk: inside fixture"
		printf '{"schema":1,"kind":"app","id":"%s","name":"SDK contract","version":"1.0.%s","caps":["thread:read","thread:post","files:read","files:write","store:read","store:write"],"host":{"mode":"cmd","start":["node","server.mjs"],"health":{"path":"/healthz"}}}\n' \
			"$APP" "$RANDOM" >"$F/manifest.json"
		chown -R k2:k2 "$F"
		K2 app deploy "$APP" "$F" --engine "$1" --no-wait >/dev/null || true
		local r=""
		for _ in $(seq 1 600); do
			r="$(K2 app deployment "$APP" --json 2>/dev/null | python3 -c 'import json,sys
try: print(json.load(sys.stdin)["deployment"]["result"])
except Exception: print("?")')"
			case "$r" in pending | starting | "?") sleep 1 ;; *) break ;; esac
		done
		[ "$r" = live ] || { K2 app logs "$APP" 2>&1 | tail -n 40 >&2; fail "apps_e2e_sdk: deploy $APP ($1): $r"; }
		local FP
		FP="$(K2 app list --json | python3 -c "import json,sys; print([a for a in json.load(sys.stdin)['apps'] if a['appId']=='$APP'][0]['frontPort'])")"
		curl -sf "http://127.0.0.1:$FP/_contract/app" >/tmp/sdk-app.json || fail "apps_e2e_sdk: asApp me.get from inside ($1)"
		python3 - /tmp/sdk-app.json "$ROOM_ID" <<'PY' || fail "apps_e2e_sdk: the App's own view ($1): $(cat /tmp/sdk-app.json)"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["me"]["kind"] == "app", v
assert [r["room"] for r in v["me"]["rooms"]] == [sys.argv[2]], v
assert v["gateway"] == "http://127.0.0.1:8730", v
PY
		curl -s --max-time 180 "http://127.0.0.1:$FP/_contract" >/tmp/sdk-in.json || fail "apps_e2e_sdk: /_contract ($1)"
		python3 - /tmp/sdk-in.json <<'PY' || fail "apps_e2e_sdk: the must-not-break suite inside the wall ($1): $(cat /tmp/sdk-in.json)"
import json, sys
v = json.load(open(sys.argv[1]))
bad = [f"{f['name']}: {f['detail']}" for f in v["failed"]] + [f"not run: {n['name']}: {n['why']}" for n in v["notRun"]]
assert v["ok"] and not v["notRun"], "\n".join(bad)
print(f"    {v['passed']} checks")
PY
		ok "lane 4: must-not-break through the SDK from inside the wall ($1)"
	}
	sdk_inside guarded
	if [ "${SDK_OPEN:-0}" = 1 ]; then
		sdk_inside open
	else
		info "lane 4: the open path runs with SDK_OPEN=1 once lane 1's open engine is in the runner"
	fi
}

# Run directly (not sourced): the runner's environment must already be set.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
	set -Eeuo pipefail
	apps_e2e_sdk
fi
