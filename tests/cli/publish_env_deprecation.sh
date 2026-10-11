#!/usr/bin/env bash
# Published services from before 0.45.4 keep the daemon's inherited
# environment in 0.45.x, with a notice that it ends in 0.46.0 (Rosson
# 2026-10-10). Agents mostly use `k2 publish …`, so the CLI warns there
# (0.45.4): `ps`/`list` tag legacy rows and add one summary line, `run`/
# `start` print one stderr line (JSON stdout stays clean), `--json` carries
# envMode/envRemovedIn, and `k2 study vault` explains the switch. A legacy
# and a clean service, against a stub daemon.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
unset K2SO_PORT K2SO_HOOK_TOKEN K2_HOOK_SOCK K2SO_HOOK_SOCK K2_HOST \
    K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL K2_PORT K2_HOOK_TOKEN || true

WORK="$(mktemp -d -t k2-publish-envdep-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
NOTICE="This service still inherits the K2 daemon's whole environment because it existed before 0.45.4. That ends in K2 0.46.0: switch with k2 publish env shop clean"
cat >"$WORK/stub.py" <<'PY2'
import http.server, json, sys
NOTICE = sys.argv[2]
def svc(name, mode):
    legacy = mode == "legacy"
    return {"id": name + "-id", "projectId": "p", "name": name, "cmd": "node s.js", "cwd": ".",
            "port": 3000, "expose": "local", "desired": "running", "status": "running",
            "pid": 1, "url": None, "target": "127.0.0.1:3000", "error": None,
            "lastExitCode": None, "kind": "cmd", "skinRoot": "", "frameAncestors": "",
            "framePolicy": None, "frameWarning": None,
            "envDeprecation": NOTICE if legacy else None,
            "envMode": mode, "envRemovedIn": "0.46.0" if legacy else None}
MODES = {"shop": "legacy", "fresh": "clean"}
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, out):
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if "/cli/vault/service" in self.path:
            self.reply({"service": "shop", "id": "shop-id", "mode": "legacy", "inheritedNames": ["AWS_SECRET"], "secrets": []})
        else:
            self.reply({"services": [svc("shop", "legacy"), svc("fresh", "clean")]})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        b = json.loads(self.rfile.read(n) or b"{}")
        name = b.get("name") or "shop"
        self.reply({"service": svc(name, MODES.get(name, "clean"))})
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY2
python3 "$WORK/stub.py" "$PORT" "$NOTICE" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
k2s() { env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"; }
WARN="warning: shop still inherits the daemon environment; this ends in K2 0.46.0. Switch with: k2 publish env shop clean (grant secrets with k2 publish secret add)."

for verb in ps list; do
	k2s publish "$verb" >"$WORK/out" 2>"$WORK/err" || fail "publish $verb: $(cat "$WORK/out" "$WORK/err")"
	out="$(cat "$WORK/out")"
	printf '%s\n' "$out" | grep -E '^shop ' | grep -Fq "[legacy env (ends 0.46.0)]" || fail "$verb: shop row lacks the tag: $out"
	if printf '%s\n' "$out" | grep -E '^fresh ' | grep -Fq "legacy env"; then fail "$verb: a clean row must carry no tag: $out"; fi
	printf '%s\n' "$out" | grep -Fxq "warning: 1 service still inherits the daemon environment (shop); this ends in K2 0.46.0. Switch each with: k2 publish env <service> clean (grant secrets with k2 publish secret add <service> NAME)." \
		|| fail "$verb: summary line missing: $out"
	[ "$(printf '%s\n' "$out" | grep -c '^warning:')" = 1 ] || fail "$verb: expected exactly one warning line: $out"
	ok "k2 publish $verb: legacy row tagged, clean row not, one summary line"
done

k2s publish ps --json >"$WORK/out" 2>"$WORK/err" || fail "ps --json: $(cat "$WORK/err")"
[ ! -s "$WORK/err" ] || fail "ps --json wrote to stderr: $(cat "$WORK/err")"
python3 - "$WORK/out" <<'PY2' || fail "ps --json shape: $(cat "$WORK/out")"
import json, sys
rows = {r["name"]: r for r in json.load(open(sys.argv[1]))["services"]}
assert rows["shop"]["envMode"] == "legacy" and rows["shop"]["envRemovedIn"] == "0.46.0", rows["shop"]
assert rows["fresh"]["envMode"] == "clean" and rows["fresh"]["envRemovedIn"] is None, rows["fresh"]
PY2
ok "k2 publish ps --json: envMode legacy/clean, envRemovedIn 0.46.0 on the legacy row"

k2s publish start shop >"$WORK/out" 2>"$WORK/err" || fail "start shop: $(cat "$WORK/out" "$WORK/err")"
[ "$(cat "$WORK/err")" = "$WARN" ] || fail "start shop stderr: $(cat "$WORK/err")"
if grep -q "inherits" "$WORK/out"; then fail "start shop: the warning belongs on stderr only: $(cat "$WORK/out")"; fi
grep -q "^Started shop" "$WORK/out" || fail "start shop stdout: $(cat "$WORK/out")"
ok "k2 publish start <legacy>: one stderr warning line"

k2s publish start shop --json >"$WORK/out" 2>"$WORK/err" || fail "start --json: $(cat "$WORK/err")"
[ "$(cat "$WORK/err")" = "$WARN" ] || fail "start --json stderr: $(cat "$WORK/err")"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["service"]["envMode"]=="legacy"' "$WORK/out" \
	|| fail "start --json stdout is not clean JSON: $(cat "$WORK/out")"
ok "k2 publish start --json: stdout stays JSON, warning on stderr"

k2s publish run shop --cmd "node s.js" --port 3000 --no-tunnel >"$WORK/out" 2>"$WORK/err" || fail "run shop: $(cat "$WORK/out" "$WORK/err")"
[ "$(cat "$WORK/err")" = "$WARN" ] || fail "run shop stderr: $(cat "$WORK/err")"
ok "k2 publish run <legacy>: one stderr warning line"

k2s publish start fresh >"$WORK/out" 2>"$WORK/err" || fail "start fresh: $(cat "$WORK/out" "$WORK/err")"
[ ! -s "$WORK/err" ] || fail "a clean service must not warn: $(cat "$WORK/err")"
if grep -q "inherits\|deprecated" "$WORK/out"; then fail "clean start stdout: $(cat "$WORK/out")"; fi
ok "k2 publish start <clean>: no warning"

out="$(k2s publish env shop 2>&1)" || fail "publish env: $out"
printf '%s' "$out" | grep -q "deprecated: inheriting the daemon environment ends in K2 0.46.0 — switch with: k2 publish env shop clean" \
	|| fail "publish env lacks the notice: $out"
ok "k2 publish env names the 0.46.0 end and the switch"

out="$(k2s study vault 2>&1)" || fail "study vault: $out"
printf '%s' "$out" | grep -q "^PUBLISHED SERVICES" || fail "study vault lacks the section: $out"
printf '%s' "$out" | grep -Fq "k2 publish env <name> clean" || fail "study vault lacks the switch: $out"
ok "k2 study vault explains the switch"

echo "PASS: publish env deprecation ($pass checks)"
