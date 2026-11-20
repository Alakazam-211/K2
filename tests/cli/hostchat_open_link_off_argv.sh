#!/usr/bin/env bash
# Privacy check: k2 hostchat open never puts the one-use owner link on a
# command line or in a child's environment. Every account on this computer
# can read a command line, so the browser opener gets only the path of a
# small redirect page that holds the link.
#
# The real CLI runs against two stubs: one on the vault socket that answers
# the confirm, and one on the daemon's TCP port that answers the status and
# mints a fresh owner code each time. open, xdg-open, python3, curl and the
# small tools the CLI starts (mktemp, chmod, find, rm, sleep, id, uname) are
# wrapped so each logs its argv and its environment. open and xdg-open also
# record the path they were given, its mode, its folder's mode and a copy of
# its content (the CLI removes the page later).
#
# Checks:
#   1. --print prints the link and opens nothing.
#   2. open (macOS) or xdg-open (elsewhere) gets one argument: a page in a
#      private folder (0700, ours) under TMPDIR, the page 0600, holding the
#      link. The page is still there when the CLI returns (the browser may
#      not have read it yet) and gone after the wait.
#   3. A page left by an earlier run (older than a minute) is removed at the
#      start of the next run; one from a run still inside its wait is kept.
#   4. Neither the link nor its code is in any child's argv or environment.
#
# The wait is 30 s in use. K2_TEST_HOSTCHAT_OPEN_WAIT shortens it here so the
# test takes seconds; the CLI accepts only 0 to 30, so it can never keep the
# page longer than in use.
#
# Runs with the system bash on macOS (3.2). No daemon build; a throwaway HOME
# and TMPDIR; never touches the real ~/.k2 and never opens a real browser.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl not found" >&2; exit 1; }
REAL_ENV="$(command -v env)" || { echo "FAIL: env not found" >&2; exit 1; }

# Short: the vault socket's path must fit a unix socket address.
W="$(mktemp -d /tmp/k2hco-XXXXXX)"
STUB_PID=""; TCP_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    [ -n "$TCP_PID" ] && kill "$TCP_PID" 2>/dev/null || true
    rm -rf "$W"
}
trap cleanup EXIT

ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
OPENLOG="$W/opened.log"
TCPLOG="$W/tcp-requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$OPENLOG"; : >"$TCPLOG"

APP_URL="https://chat.example.test"
CODE_BASE="k2sho_HCOPENSENTINEL"

# Wrappers that log argv and environment, then run the real program.
mkdir -p "$W/bin"
for prog in python3 curl mktemp chmod find rm sleep id uname; do
    if [ "$prog" = python3 ]; then real="$REAL_PY"
    elif [ "$prog" = curl ]; then real="$REAL_CURL"
    else real="$(command -v "$prog")" || { echo "FAIL: $prog not found" >&2; exit 1; }
    fi
    cat >"$W/bin/$prog" <<WRAP
#!/bin/bash
printf '$prog %s\n' "\$*" >>"$ARGLOG"
{ printf '== $prog\n'; "$REAL_ENV"; } >>"$ENVLOG"
exec "$real" "\$@"
WRAP
    chmod +x "$W/bin/$prog"
done
# The openers: log like the others, then record what they were given. They
# never start a browser.
for prog in open xdg-open; do
    cat >"$W/bin/$prog" <<WRAP
#!/bin/bash
printf '$prog %s\n' "\$*" >>"$ARGLOG"
{ printf '== $prog\n'; "$REAL_ENV"; } >>"$ENVLOG"
"$REAL_PY" - "$OPENLOG" "$W" '$prog' "\$@" <<'PYEOF'
import json, os, shutil, stat, sys
log, work, prog, args = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
rec = {"prog": prog, "argc": len(args), "arg": args[0] if args else ""}
p = rec["arg"]
if os.path.isfile(p):
    st = os.lstat(p)
    d = os.path.dirname(p)
    dst = os.lstat(d)
    rec.update(isfile=stat.S_ISREG(st.st_mode), mode="%o" % stat.S_IMODE(st.st_mode),
               uid=st.st_uid, dir=d, dirmode="%o" % stat.S_IMODE(dst.st_mode), diruid=dst.st_uid)
    n = sum(1 for _ in open(log))
    copy = os.path.join(work, "opened-content-%d" % n)
    shutil.copyfile(p, copy)
    rec["copy"] = copy
with open(log, "a") as f:
    f.write(json.dumps(rec) + "\n")
PYEOF
WRAP
    chmod +x "$W/bin/$prog"
done
CLI_PATH="$W/bin:$PATH"

# A throwaway HOME and TMPDIR. Nothing may mark this a K2 session, or the
# person-only command refuses before the confirm.
for v in $(env | grep -oE '^(K5|K2|K2SO)_[A-Za-z0-9_]+' || true); do unset "$v"; done
export HOME="$W/home"
mkdir -p "$HOME/.k2" "$W/tmp"
TMPD="$W/tmp"
SOCK="$HOME/.k2/vault.sock"

# The vault stub: confirm/start offers the vault password, confirm/finish
# answers a token.
"$REAL_PY" - "$SOCK" "$W" <<'PYEOF' &
import json, os, socket, sys, threading
sock_path, work = sys.argv[1], sys.argv[2]
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(16)
open(os.path.join(work, "ready"), "w").close()

def handle(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            break
        data += chunk
    head, _, rest = data.partition(b"\r\n\r\n")
    n = 0
    for line in head.split(b"\r\n")[1:]:
        k, _, v = line.partition(b":")
        if k.strip().lower() == b"content-length":
            n = int(v.strip())
    while len(rest) < n:
        chunk = conn.recv(65536)
        if not chunk:
            break
        rest += chunk
    path = head.split(b"\r\n", 1)[0].decode("latin1").split(" ")[1].split("?")[0]
    if path.endswith("confirm/start"):
        out = {"methods": ["vault-password"], "expiresIn": 60}
    elif path.endswith("confirm/finish"):
        out = {"token": "k2vc_HCOPEN_confirm", "expiresIn": 60}
    else:
        out = {"error": "unexpected route " + path}
    payload = json.dumps(out).encode()
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (len(payload), payload))
    conn.close()

while True:
    c, _ = srv.accept()
    threading.Thread(target=handle, args=(c,), daemon=True).start()
PYEOF
STUB_PID=$!

# The daemon stub on TCP: logs each request; mints a fresh code each time.
"$REAL_PY" - "$W/tcp-port" "$TCPLOG" "$APP_URL" "$CODE_BASE" <<'PYEOF' &
import http.server, json, os, sys, threading
port_path, log_path, app_url, code_base = sys.argv[1:5]
lock = threading.Lock()
minted = [0]
class H(http.server.BaseHTTPRequestHandler):
    def _answer(self, status, obj):
        out = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = self.path.split("?")[0]
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path, "body": raw}) + "\n")
        if path == "/cli/sew/status":
            return self._answer(200, {"ok": True, "supported": True, "state": "running", "appUrl": app_url})
        if path == "/cli/sew/owner-handoff":
            with lock:
                minted[0] += 1
                k = minted[0]
            return self._answer(200, {"ok": True, "code": "%s%03d" % (code_base, k), "expiresInMs": 60000})
        return self._answer(404, {"error": "route not found"})
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
PYEOF
TCP_PID=$!
for _ in $(seq 1 100); do [ -f "$W/ready" ] && [ -s "$W/tcp-port" ] && break; sleep 0.1; done
[ -f "$W/ready" ] || { echo "FAIL: vault stub never came up" >&2; exit 1; }
[ -s "$W/tcp-port" ] || { echo "FAIL: tcp stub never came up" >&2; exit 1; }
cat "$W/tcp-port" >"$HOME/.k2/heartbeat.port"
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# k2run WAIT ARGS…: the CLI with the logging PATH and the test TMPDIR; the
# vault password (empty) on stdin. Sets rc, out, err.
k2run() {
    local wait="$1"; shift
    set +e
    out="$(printf '\n' | PATH="$CLI_PATH" TMPDIR="$TMPD" K2_TEST_HOSTCHAT_OPEN_WAIT="$wait" \
        "$K2_CLI" "$@" 2>"$W/err")"; rc=$?
    set -e
    err="$(cat "$W/err")"
}
mints() { grep -c '"/cli/sew/owner-handoff"' "$TCPLOG" || true; }
pages() { find "$TMPD" -mindepth 1 -maxdepth 1 -name 'k2-hostchat-open.*' | sort; }

# The link and code are secrets: in no child's argv or environment.
assert_off_children() {
    local label="$1" value="$2"
    if grep -Fq "$value" "$ARGLOG"; then
        bad "$label: in a child's argv"
        grep -F "$value" "$ARGLOG" | head -3 >&2
    else
        ok "$label: in no child's argv"
    fi
    if grep -Fq "$value" "$ENVLOG"; then
        bad "$label: in a child's environment"
        grep -F "$value" "$ENVLOG" | head -3 >&2
    else
        ok "$label: in no child's environment"
    fi
}

if [ "$(uname -s)" = "Darwin" ]; then OPENER=open; else OPENER=xdg-open; fi

echo "== --print: prints the link, opens nothing =="
k2run 5 hostchat open --print
CODE1="${CODE_BASE}001"
LINK1="$APP_URL/k2/owner-handoff#code=$CODE1"
if [ "$rc" = 0 ]; then ok "--print exits 0"; else bad "--print exits $rc: $err"; fi
if [ "$out" = "$LINK1" ]; then ok "--print prints the link"; else bad "--print printed $(printf %q "$out")"; fi
if [ "$(mints)" = 1 ]; then ok "--print minted one code"; else bad "--print minted $(mints) codes"; fi
if [ -s "$OPENLOG" ]; then bad "--print ran an opener: $(cat "$OPENLOG")"; else ok "--print opens nothing"; fi
if grep -Eq '^(open|xdg-open) ' "$ARGLOG"; then bad "--print: an opener is in the argv log"; else ok "--print: no opener in the argv log"; fi
if [ -z "$(pages)" ]; then ok "--print writes no page"; else bad "--print wrote a page: $(pages)"; fi
if grep -q '^python3 ' "$ARGLOG" && grep -q '^curl ' "$ARGLOG"; then ok "--print: children were logged"; else bad "--print: no python3 or curl child was logged"; fi
assert_off_children "--print: the code" "$CODE1"

echo "== open: the link reaches the opener only through a private page =="
# A page from an earlier run, two hours old: removed at the start of the run.
OLD="$TMPD/k2-hostchat-open.OLDRUN"
mkdir -p "$OLD"; chmod 700 "$OLD"
printf 'stale link\n' >"$OLD/open.html"; chmod 600 "$OLD/open.html"
touch -t 202001010000 "$OLD/open.html" "$OLD"
# A page from a run still inside its wait: kept.
BUSY="$TMPD/k2-hostchat-open.BUSYRUN"
mkdir -p "$BUSY"; chmod 700 "$BUSY"
printf 'busy link\n' >"$BUSY/open.html"; chmod 600 "$BUSY/open.html"
# Not ours by name: kept.
OTHER="$TMPD/k2-other.OLDRUN"
mkdir -p "$OTHER"; touch -t 202001010000 "$OTHER"

: >"$ARGLOG"; : >"$ENVLOG"
k2run 5 hostchat open
CODE2="${CODE_BASE}002"
LINK2="$APP_URL/k2/owner-handoff#code=$CODE2"
if [ "$rc" = 0 ]; then ok "open exits 0"; else bad "open exits $rc: $err"; fi
case "$err" in *"Opened chat as its owner"*) ok "open says it opened chat" ;; *) bad "open said $(printf %q "$err")" ;; esac
if [ "$(mints)" = 2 ]; then ok "open minted one code"; else bad "minted $(mints) codes after open"; fi
# Right after the CLI returns the page is still there.
PAGE="$("$REAL_PY" -c 'import json,sys; r=[json.loads(l) for l in open(sys.argv[1])]; print(r[-1]["arg"] if len(r)==1 else "")' "$OPENLOG")"
if [ -n "$PAGE" ] && [ -f "$PAGE" ]; then ok "the page is still there when the CLI returns"; else bad "no page right after the CLI returned ($(printf %q "$PAGE"))"; fi

if "$REAL_PY" - "$OPENLOG" "$OPENER" "$TMPD" "$LINK2" "$(id -u)" <<'PYEOF'
import json, os, re, sys
log, opener, tmpd, link, uid = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
rows = [json.loads(l) for l in open(log)]
problems = []
if len(rows) != 1:
    problems.append("want one opener call, got %d" % len(rows))
r = rows[-1] if rows else {}
if r.get("prog") != opener:
    problems.append("opener %r, want %r" % (r.get("prog"), opener))
if r.get("argc") != 1:
    problems.append("opener got %r arguments, want 1" % r.get("argc"))
arg = r.get("arg", "")
if not re.fullmatch(re.escape(tmpd) + r"/k2-hostchat-open\.[A-Za-z0-9]{6}/open\.html", arg):
    problems.append("opener got %r, want a page in a fresh k2-hostchat-open.XXXXXX folder under TMPDIR" % arg)
if not r.get("isfile"):
    problems.append("the opener's argument is not a file")
if r.get("mode") != "600":
    problems.append("page mode %r, want 600" % r.get("mode"))
if r.get("dirmode") != "700":
    problems.append("folder mode %r, want 700" % r.get("dirmode"))
if r.get("uid") != uid or r.get("diruid") != uid:
    problems.append("page or folder not ours")
content = open(r["copy"]).read() if r.get("copy") else ""
if link not in content:
    problems.append("the page does not hold the link")
if "location.replace(" not in content or "http-equiv=\"refresh\"" not in content:
    problems.append("the page does not redirect")
for p in problems:
    print("  " + p, file=sys.stderr)
sys.exit(1 if problems else 0)
PYEOF
then ok "the opener got one private 0600 page in a 0700 folder, holding the link"
else bad "the opener's page"; fi

if [ -e "$OLD" ]; then bad "the earlier run's page was not removed"; else ok "the earlier run's page was removed"; fi
if [ -f "$BUSY/open.html" ]; then ok "a page from a run inside its wait is kept"; else bad "a page from a run inside its wait was removed"; fi
if [ -d "$OTHER" ]; then ok "a folder with another name is kept"; else bad "a folder with another name was removed"; fi

# Gone after the wait (5 s here).
PAGEDIR="$(dirname "$PAGE")"
for _ in $(seq 1 60); do [ -e "$PAGEDIR" ] || break; sleep 0.5; done
if [ -e "$PAGEDIR" ]; then bad "the page is still there 30 s after a 5 s wait"; else ok "the page is gone after the wait"; fi

if grep -q '^python3 ' "$ARGLOG" && grep -q '^curl ' "$ARGLOG" && grep -q "^$OPENER " "$ARGLOG"; then
    ok "open: python3, curl and the opener were logged"
else
    bad "open: a child was not logged"
fi
if grep -q '^rm ' "$ARGLOG"; then ok "open: the removal was logged"; else bad "open: no rm was logged"; fi
assert_off_children "open: the code" "$CODE2"
assert_off_children "open: the link" "$LINK2"

echo
echo "hostchat open link off argv: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
