#!/usr/bin/env bash
# A7 (0.45.1): a federated connection belongs to the remote WORKSPACE, so
# every name the peer's roster lists for it (handle, alias, display name)
# reaches it, and `k2 msg` says which name it resolved to.
#
# One sandbox daemon (temp HOME, worktree build, air-gapped, federation on)
# plus a loopback stand-in peer (python) that serves a roster and accepts
# inbound envelopes. The peer is paired for real (pair/request + owner
# confirm with the SAS). Loud: every check must pass; nothing is skipped.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }
for tool in jq openssl python3 curl; do
    command -v "$tool" >/dev/null || { echo "FAIL: $tool is required" >&2; exit 1; }
done

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  PASS: $*"; pass=$((pass + 1)); }

echo "== note helper wording (shared with the Thread-rename note, TR17) =="
eval "$(sed -n '/^_k2_addr_note()/,/^}/p' "$K2_CLI")"
declare -F _k2_addr_note >/dev/null || fail "_k2_addr_note missing from cli/k2"
[ "$(_k2_addr_note "seoca::q.k2.dev" "quillify-website::q.k2.dev" 2>&1)" = \
  "note: seoca::q.k2.dev is quillify-website::q.k2.dev" ] || fail "synonym wording"
[ "$(_k2_addr_note "k2/3" "k2/reviewer" renamed 2>&1)" = "note: k2/3 is now k2/reviewer" ] \
    || fail "rename wording"
[ -z "$(_k2_addr_note "same" "same" 2>&1)" ] || fail "no note when nothing changed"
ok "note: <a> is <b> / note: <a> is now <b>"

# Never let a caller's K2 session leak into the sandbox.
for v in $(env | cut -d= -f1 | grep -E '^(K2|K2SO)_' || true); do unset "$v"; done
export K2_AIRGAP=1 K2_FEDERATION=1
SHIM_DIR="$(mktemp -d -t k2-a7-shim-XXXXXX)"
export K2_TEST_AGENT_SHIM_DIR="$SHIM_DIR"

source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start
PEER_PID=""
trap '[ -n "$PEER_PID" ] && kill "$PEER_PID" 2>/dev/null; _sandbox_daemon_cleanup; rm -rf "$SHIM_DIR"' EXIT
BASE="http://127.0.0.1:${K2SO_PORT}"

# ── the stand-in peer ────────────────────────────────────────────────
W_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
PEER_DIR="$SANDBOX_HOME/peer"
mkdir -p "$PEER_DIR"
python3 -c "import json,sys; json.dump({'agents':[{'workspace_id':sys.argv[1],'workspace_name':'Seoca','agent':'quillify-website','address':sys.argv[1]+'::quillify-website','aliases':[]}]}, open(sys.argv[2],'w'))" \
    "$W_ID" "$PEER_DIR/roster.json"
cat >"$PEER_DIR/peer.py" <<'PY'
import http.server, json, os, sys
ROOT = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, body):
        b = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith("/cli/federation/roster"):
            self._send(200, open(os.path.join(ROOT, "roster.json")).read())
        else:
            self._send(404, '{"error":"not found"}')
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n)
        if self.path.startswith("/cli/federation/inbound"):
            with open(os.path.join(ROOT, "inbound.log"), "ab") as f:
                f.write(body + b"\n")
            self._send(200, '{"delivered":true,"mode":"live"}')
        else:
            self._send(404, '{"error":"not found"}')
s = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
open(os.path.join(ROOT, "port"), "w").write(str(s.server_address[1]))
s.serve_forever()
PY
python3 "$PEER_DIR/peer.py" "$PEER_DIR" &
PEER_PID=$!
for _ in $(seq 1 50); do [ -s "$PEER_DIR/port" ] && break; sleep 0.1; done
[ -s "$PEER_DIR/port" ] || fail "stand-in peer never started"
PEER_PORT="$(cat "$PEER_DIR/port")"
PHOST="127.0.0.1:${PEER_PORT}"

# Pair it: the peer's key asks, the owner confirms with the SAS.
openssl ecparam -name prime256v1 -genkey -noout -out "$PEER_DIR/key.pem" 2>/dev/null
openssl ec -in "$PEER_DIR/key.pem" -pubout -out "$PEER_DIR/pub.pem" 2>/dev/null
req="$(python3 -c "import json,sys; print(json.dumps({'label':'quillify','subdomain':'quillify','base_url':sys.argv[1],'public_key_pem':open(sys.argv[2]).read()}))" \
    "http://${PHOST}" "$PEER_DIR/pub.pem")"
pr="$(curl -sf -X POST -H 'Content-Type: application/json' -d "$req" "$BASE/cli/federation/pair/request")" \
    || fail "pair/request failed"
fp="$(jq -r .fingerprint <<<"$pr")"; sas="$(jq -r .sas <<<"$pr")"
curl -sf -X POST -H 'Content-Type: application/json' \
    -d "{\"fingerprint\":\"$fp\",\"sas\":\"$sas\"}" \
    "$BASE/cli/federation/pair/confirm?token=${K2SO_TOKEN}" >/dev/null || fail "pair/confirm failed"
ok "stand-in peer paired at $PHOST"

# ── the source workspace ─────────────────────────────────────────────
SRC="$SANDBOX_HOME/work/news-desk"
mkdir -p "$SRC"
SRC="$(cd "$SRC" && pwd -P)" # the daemon stores the physical path (/private/var)
(cd "$SRC" && git init -q && git -c user.name=t -c user.email=t@t commit --allow-empty -q -m init) \
    || fail "git init source"
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1]}))' "$SRC")" \
    "$BASE/cli/projects/add-from-path?token=${K2SO_TOKEN}")"
[ "$(jq -r .path <<<"$reg")" = "$SRC" ] && [ -n "$(jq -r '.id // empty' <<<"$reg")" ] \
    || fail "register source: $reg"

# The source workspace is explicit: the sandbox HOME holds the daemon's own
# `.k2`, so a walk up from $SRC would stop there.
k2() {
    (cd "$SRC" && env -i HOME="$SANDBOX_HOME" PATH="$PATH" K2_AIRGAP=1 \
        K2_PORT="$K2SO_PORT" K2_PROJECT_PATH="$SRC" "$K2_CLI" "$@")
}

echo "== connections add binds to the workspace (CA8) =="
out="$(k2 connections add "seoca::${PHOST}" 2>&1)" || fail "add: $out"
grep -Fq "reaches quillify-website::${PHOST} (Seoca)" <<<"$out" || fail "add must say what it reaches: $out"
ok "add seoca::host → reaches quillify-website (Seoca)"

out="$(k2 connections add "nobody::${PHOST}" 2>&1)" || fail "add nobody: $out"
grep -Fq "isn't on ${PHOST}'s roster" <<<"$out" || fail "add must warn on an unknown name: $out"
grep -Fq "quillify-website" <<<"$out" || fail "warning lists the roster's names: $out"
k2 connections remove "nobody::${PHOST}" >/dev/null || fail "remove nobody"
ok "add of an unknown name warns with the roster's names (row kept, then removed)"

echo "== connections list shows both names =="
js="$(k2 connections list --json)"
row="$(jq -c --arg a "seoca::${PHOST}" '.connections[] | select(.address == $a)' <<<"$js")"
[ -n "$row" ] || fail "remote row missing: $js"
[ "$(jq -r .remoteWorkspaceId <<<"$row")" = "$W_ID" ] || fail "remoteWorkspaceId: $row"
[ "$(jq -r .handle <<<"$row")" = "quillify-website" ] || fail "handle: $row"
[ "$(jq -r .displayName <<<"$row")" = "Seoca" ] || fail "displayName: $row"
txt="$(k2 connections list)"
grep -Fq "seoca::${PHOST} → quillify-website (Seoca)" <<<"$txt" || fail "text list: $txt"
ok "list --json has remoteWorkspaceId/handle/displayName; text shows → handle (display)"

echo "== k2 msg by either name; note only when the name differs =="
err="$SANDBOX_HOME/msg.err"
k2 msg "quillify-website::${PHOST}" "by handle" 2>"$err" >/dev/null \
    || fail "msg by handle (the reply address) must succeed: $(cat "$err")"
if grep -q '^note:' "$err"; then fail "no note when typed name is the handle: $(cat "$err")"; fi
ok "msg quillify-website::host → exit 0, no note"

k2 msg "seoca::${PHOST}" "by display name" 2>"$err" >/dev/null \
    || fail "msg by display name must succeed: $(cat "$err")"
grep -Fxq "note: seoca::${PHOST} is quillify-website::${PHOST}" "$err" \
    || fail "note line missing: $(cat "$err")"
ok "msg seoca::host → exit 0 + note: seoca::host is quillify-website::host"

[ "$(wc -l <"$PEER_DIR/inbound.log" | tr -d ' ')" = "2" ] \
    || fail "peer must have received exactly 2 envelopes: $(wc -l <"$PEER_DIR/inbound.log")"
ok "both messages reached the peer"

echo ""
echo "Results: $pass passed, 0 failed"
