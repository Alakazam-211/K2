#!/usr/bin/env bash
# k2 remote-session's secret inputs: --token-file PATH (every verb), and
# write's text from stdin (-) or a file (--text-file PATH). A secret given
# these ways reaches the daemon and nothing else:
#
#   - no child process's argv or environment. Every program on PATH is
#     replaced by a stand-in that logs its argv and its environment, then
#     runs the real one; on a system with /proc, a scanner also reads
#     /proc/<pid>/cmdline and environ of every process while the CLI runs
#     (that also sees a program started by its full path);
#   - not the URL of the request (a daemon or proxy may log a URL): the
#     text rides the form body, the token the Authorization header;
#   - not the CLI's stdout or stderr, and no file under HOME (where K2 and
#     its CLI keep their logs) or TMPDIR;
#   - the daemon gets the bytes, less one trailing newline by default
#     (of two, one stays; a final CRLF goes as a pair) and exactly with
#     --keep-newline: a CR, a NUL, `&`, `=`, `%`, `+` and UTF-8 too.
#
# Also: --token-file refuses a file its group or others can read (0644,
# 0640, 0604) with exit 2 and sends nothing; 0600 and 0400 are taken. The
# old forms still work: the text word (in the URL, as before), --token,
# and $K2_REMOTE_TOKEN with --host. And the usage errors (two text forms,
# - with --token-stdin, two token forms, empty stdin, --keep-newline with
# no text or with the text word) send nothing.
#
# Runs on the CLI under test (cli/k2, or K5_CLI_UNDER_TEST). A stub daemon
# on TCP, a throwaway HOME; never touches the real ~/.k2.
#
# GARDEN_BITE (unset in normal runs) is passed through to the CLI: a job
# that proves each check bites sets it for a CLI copy patched to undo one
# fix when it names that fix.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

TEXT_SENTINEL="k2txt_SECRETINPUT_7Kq"
TOKEN_SENTINEL="k2rs_SECRETINPUT_tok9"
STDIN_TOKEN_SENTINEL="k2rs_SECRETINPUT_stdin4"

W="$(mktemp -d "${TMPDIR:-/tmp}/k2rss-XXXXXX")"
W="$(cd "$W" && pwd -P)"
STUB_PID=""
SCAN_PID=""
cleanup() {
    if [ -n "$SCAN_PID" ]; then kill "$SCAN_PID" 2>/dev/null || true; fi
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$W"
}
trap cleanup EXIT

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
REAL_BASH="$(command -v bash)"
REAL_ENV="$(command -v env)"
ORIG_PATH="$PATH"

mkdir -p "$W/bin" "$W/logs" "$W/in" "$W/out" "$W/home/.k2" "$W/tmp" "$W/cwd"
ARGLOG="$W/logs/child-argv.log"
ENVLOG="$W/logs/child-env.log"
TCPLOG="$W/logs/tcp.jsonl"
PROCHITS="$W/logs/proc-hits.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"; : >"$PROCHITS"

# The stand-ins: one per program name on PATH (the first one found wins,
# as a lookup would), written with builtins only.
IFS=: read -r -a path_dirs <<<"$ORIG_PATH"
for d in "${path_dirs[@]}"; do
    [ -n "$d" ] && [ -d "$d" ] || continue
    for f in "$d"/*; do
        n="${f##*/}"
        case "$n" in ''|*[!A-Za-z0-9._+-]*) continue ;; esac
        [ -e "$W/bin/$n" ] && continue
        [ -f "$f" ] && [ -x "$f" ] || continue
        printf '%s\n' \
            "#!$REAL_BASH" \
            "{ printf '%s' '$n'; printf ' %s' \"\$@\"; printf '\\n'; } >>'$ARGLOG'" \
            "{ printf '== %s\\n' '$n'; '$REAL_ENV'; } >>'$ENVLOG'" \
            "exec '$f' \"\$@\"" >"$W/bin/$n"
    done
done
chmod +x "$W/bin"/*
STAND_INS="$(find "$W/bin" -type f | wc -l | tr -d ' ')"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# The stand-ins log: a check of them is not vacuous.
PATH="$W/bin:$ORIG_PATH" python3 -c pass STANDIN_SELFCHECK
if grep -q '^python3 -c pass STANDIN_SELFCHECK$' "$ARGLOG" && grep -q '^== python3$' "$ENVLOG"; then
    ok "the $STAND_INS stand-ins log argv and environment"
else
    bad "the stand-ins log nothing"
fi

# The daemon stub: one JSON line per request (method, the request target
# as sent, Authorization, Content-Type, the body in base64).
"$REAL_PY" - "$W/port" "$TCPLOG" <<'PYEOF' &
import base64, http.server, json, os, sys, threading
port_path, log_path = sys.argv[1], sys.argv[2]
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({
                "method": self.command, "target": self.path,
                "path": self.path.split("?", 1)[0],
                "auth": self.headers.get("Authorization", ""),
                "ctype": self.headers.get("Content-Type", ""),
                "body": base64.b64encode(raw).decode(),
            }) + "\n")
        path = self.path.split("?", 1)[0]
        if path == "/cli/terminal/write":
            out = {"success": True}
        elif path == "/cli/remote-session/status":
            out = {"ok": True, "enabled": True, "activeGrants": 0, "activeSessions": [], "recentDenials": []}
        else:
            out = {"ok": True}
        body = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$W/port" ] && break; sleep 0.1; done
[ -s "$W/port" ] || { echo "FAIL: the stub daemon never came up" >&2; exit 1; }
PORT="$(cat "$W/port")"

export HOME="$W/home"
printf '%s\n' "$PORT" >"$HOME/.k2/heartbeat.port"
printf 'test-owner-token\n' >"$HOME/.k2/heartbeat.token"
printf 'test-owner-token\n' >"$HOME/.k2/daemon.token"
# Nothing may mark this a K2 session or name another daemon.
for v in $(compgen -e); do
    case "$v" in K2_*|K5_*|K2SO_*) [ "$v" = K5_CLI_UNDER_TEST ] || unset "$v" ;; esac
done

# The secrets, outside HOME and TMPDIR (those are searched for them).
"$REAL_PY" - "$W/in" "$TEXT_SENTINEL" "$TOKEN_SENTINEL" <<'PYEOF'
import os, sys
d, text, tok = sys.argv[1], sys.argv[2].encode(), sys.argv[3].encode()
body = b"sudo pw " + text + b" \"q\" & = % + \xc3\xa9 \r\x00 end\n\n"
for name, data in (("text.bin", body), ("text.want", body[:-1]),
                   ("crlf.bin", b"pw " + text + b"\r\n"), ("crlf.want", b"pw " + text),
                   ("nl.bin", b"\n")):
    with open(os.path.join(d, name), "wb") as f:
        f.write(data)
for name, mode in (("tok600", 0o600), ("tok400", 0o400), ("tok644", 0o644),
                   ("tok640", 0o640), ("tok604", 0o604)):
    p = os.path.join(d, name)
    with open(p, "wb") as f:
        f.write(tok + b"\n")
    os.chmod(p, mode)
open(os.path.join(d, "empty"), "wb").close()
PYEOF
printf '%s\n' "$TEXT_SENTINEL" "$TOKEN_SENTINEL" "$STDIN_TOKEN_SENTINEL" >"$W/logs/secrets.lst"

HAVE_PROC=0
if [ -r /proc/self/cmdline ]; then HAVE_PROC=1; fi

# The /proc scanner: every process's cmdline and environ, over and over,
# until the stop file appears.
scan_start() {
    rm -f "$W/logs/scan-stop" "$W/logs/scan-up"
    "$REAL_PY" - "$W/logs/secrets.lst" "$PROCHITS" "$W/logs/scan-stop" "$W/logs/scan-up" <<'PYEOF' &
import os, sys
secrets = [l.strip().encode() for l in open(sys.argv[1]) if l.strip()]
hits_path, stop, up = sys.argv[2], sys.argv[3], sys.argv[4]
me = str(os.getpid())
open(up, "w").close()
while True:
    last = os.path.exists(stop)
    for pid in os.listdir("/proc"):
        if not pid.isdigit() or pid == me:
            continue
        for what in ("cmdline", "environ"):
            try:
                with open("/proc/%s/%s" % (pid, what), "rb") as f:
                    data = f.read()
            except OSError:
                continue
            for s in secrets:
                if s in data:
                    with open(hits_path, "a") as h:
                        h.write("%s %s %r\n" % (pid, what, data.replace(b"\0", b" ")[:300]))
    if last:
        break
PYEOF
    SCAN_PID=$!
    for _ in $(seq 1 100); do [ -f "$W/logs/scan-up" ] && break; sleep 0.05; done
}
scan_stop() {
    : >"$W/logs/scan-stop"
    wait "$SCAN_PID" 2>/dev/null || true
    SCAN_PID=""
}

fresh() {
    : >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"; : >"$PROCHITS"
    rm -rf "$W/tmp"; mkdir -p "$W/tmp"
}

# k2run STDIN_FILE ARGS...: the CLI with the stand-ins first on PATH, its
# own TMPDIR, stdin from the file; RC, out and err kept.
k2run() {
    local in="$1"; shift
    if [ "$HAVE_PROC" = 1 ]; then scan_start; fi
    RC=0
    (cd "$W/cwd" && PATH="$W/bin:$ORIG_PATH" TMPDIR="$W/tmp" "$K2_CLI" "$@" \
        <"$in" >"$W/out/stdout" 2>"$W/out/stderr") || RC=$?
    if [ "$HAVE_PROC" = 1 ]; then scan_stop; fi
}

# The requests to PATH the stub got.
requests_to() {
    "$REAL_PY" -c '
import json, sys
n = sum(1 for l in open(sys.argv[1]) if l.strip() and json.loads(l)["path"] == sys.argv[2])
print(n)
' "$TCPLOG" "$1"
}

assert_rc() {
    if [ "$RC" = "$2" ]; then ok "$1: exit $2"; else bad "$1: exit $RC, want $2"; sed 's/^/    /' "$W/out/stderr" >&2; fi
}

assert_no_request() {
    local n; n="$("$REAL_PY" -c 'import sys; print(sum(1 for l in open(sys.argv[1]) if l.strip()))' "$TCPLOG")"
    if [ "$n" = 0 ]; then ok "$1: nothing was sent"; else bad "$1: $n requests were sent"; fi
}

# assert_secret_kept LABEL VALUE WHAT: VALUE is in no child's argv or
# environment, no process's /proc cmdline or environ, not the CLI's
# output, no file under HOME or TMPDIR, and not a request's URL.
assert_secret_kept() {
    local label="$1" value="$2" what="$3"
    if grep -Fq -- "$value" "$ARGLOG"; then
        bad "$label: the $what is in a child's argv"; grep -m 3 -F -- "$value" "$ARGLOG" >&2 || true
    else ok "$label: the $what is in no child's argv"; fi
    if grep -Fq -- "$value" "$ENVLOG"; then
        bad "$label: the $what is in a child's environment"; grep -m 3 -F -- "$value" "$ENVLOG" >&2 || true
    else ok "$label: the $what is in no child's environment"; fi
    if [ "$HAVE_PROC" = 1 ]; then
        if grep -Fq -- "$value" "$PROCHITS"; then
            bad "$label: the $what is in a process's /proc cmdline or environ"; grep -m 3 -F -- "$value" "$PROCHITS" >&2 || true
        else ok "$label: the $what is in no process's /proc cmdline or environ"; fi
    fi
    if grep -Fq -- "$value" "$W/out/stdout" "$W/out/stderr"; then
        bad "$label: the $what is in the CLI's output"
    else ok "$label: the $what is not in the CLI's output"; fi
    if grep -rlF -- "$value" "$HOME" "$W/tmp" >/dev/null 2>&1; then
        bad "$label: the $what is in a file under HOME or TMPDIR"; grep -rlF -- "$value" "$HOME" "$W/tmp" >&2
    else ok "$label: the $what is in no file under HOME or TMPDIR"; fi
    if "$REAL_PY" -c '
import json, sys
v = sys.argv[2]
sys.exit(1 if any(v in json.loads(l)["target"] for l in open(sys.argv[1]) if l.strip()) else 0)
' "$TCPLOG" "$value"; then ok "$label: the $what is in no request's URL"
    else bad "$label: the $what is in a request's URL"; fi
}

# assert_write_got LABEL TOKEN [WANT]: one POST to /cli/terminal/write,
# the token as its Bearer, a form body whose id is s1 and whose message is
# exactly WANT's bytes (text.want: text.bin less one newline).
assert_write_got() {
    local label="$1" token="$2" want="${3:-$W/in/text.want}" why=""
    why="$("$REAL_PY" -c '
import base64, json, sys, urllib.parse
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == "/cli/terminal/write"]
if len(rows) != 1:
    print("%d writes, want 1" % len(rows)); sys.exit(0)
r = rows[0]
if r["method"] != "POST":
    print("method %s" % r["method"]); sys.exit(0)
if r["auth"] != "Bearer " + sys.argv[3]:
    print("Authorization is not the token"); sys.exit(0)
if not r["ctype"].startswith("application/x-www-form-urlencoded"):
    print("Content-Type %r" % r["ctype"]); sys.exit(0)
pairs = {}
for part in base64.b64decode(r["body"]).split(b"&"):
    k, _, v = part.partition(b"=")
    pairs[k] = urllib.parse.unquote_to_bytes(v)
want = open(sys.argv[2], "rb").read()
if pairs.get(b"id") != b"s1":
    print("id %r" % pairs.get(b"id")); sys.exit(0)
if pairs.get(b"message") != want:
    print("message %r, want %r" % (pairs.get(b"message"), want)); sys.exit(0)
' "$TCPLOG" "$want" "$token")"
    if [ -z "$why" ]; then ok "$label: the daemon got exactly ${want##*/}'s bytes, in the form body, with the token"
    else bad "$label: $why"; fi
    if grep -Fxq "wrote to s1" "$W/out/stdout"; then ok "$label: wrote to s1"
    else bad "$label: no 'wrote to s1' line"; fi
}

NULL_IN="/dev/null"

# 1. write --text-file with --token-file.
fresh
k2run "$NULL_IN" remote-session write s1 --text-file "$W/in/text.bin" --token-file "$W/in/tok600"
assert_rc "write --text-file --token-file" 0
assert_write_got "write --text-file --token-file" "$TOKEN_SENTINEL"
assert_secret_kept "write --text-file --token-file" "$TEXT_SENTINEL" "text"
assert_secret_kept "write --text-file --token-file" "$TOKEN_SENTINEL" "token"

# 2. write - (stdin) with --token-file.
fresh
k2run "$W/in/text.bin" remote-session write s1 - --token-file "$W/in/tok600"
assert_rc "write - --token-file" 0
assert_write_got "write - --token-file" "$TOKEN_SENTINEL"
assert_secret_kept "write - --token-file" "$TEXT_SENTINEL" "text"
assert_secret_kept "write - --token-file" "$TOKEN_SENTINEL" "token"

# 3. write --text-file with the token on stdin.
printf '%s\n' "$STDIN_TOKEN_SENTINEL" >"$W/in/stdin-token"
fresh
k2run "$W/in/stdin-token" remote-session write s1 --text-file "$W/in/text.bin" --token-stdin
assert_rc "write --text-file --token-stdin" 0
assert_write_got "write --text-file --token-stdin" "$STDIN_TOKEN_SENTINEL"
assert_secret_kept "write --text-file --token-stdin" "$TEXT_SENTINEL" "text"
assert_secret_kept "write --text-file --token-stdin" "$STDIN_TOKEN_SENTINEL" "token"

# 3b. --keep-newline: every byte, from a file and from stdin.
fresh
k2run "$NULL_IN" remote-session write s1 --text-file "$W/in/text.bin" --keep-newline --token-file "$W/in/tok600"
assert_rc "write --text-file --keep-newline" 0
assert_write_got "write --text-file --keep-newline" "$TOKEN_SENTINEL" "$W/in/text.bin"
assert_secret_kept "write --text-file --keep-newline" "$TEXT_SENTINEL" "text"
fresh
k2run "$W/in/text.bin" remote-session write s1 - --keep-newline --token-file "$W/in/tok600"
assert_rc "write - --keep-newline" 0
assert_write_got "write - --keep-newline" "$TOKEN_SENTINEL" "$W/in/text.bin"
assert_secret_kept "write - --keep-newline" "$TEXT_SENTINEL" "text"

# 3c. A final CRLF goes as a pair; a lone newline is no text, unless kept.
fresh
k2run "$NULL_IN" remote-session write s1 --text-file "$W/in/crlf.bin" --token-file "$W/in/tok600"
assert_rc "write --text-file (CRLF)" 0
assert_write_got "write --text-file (CRLF)" "$TOKEN_SENTINEL" "$W/in/crlf.want"
assert_secret_kept "write --text-file (CRLF)" "$TEXT_SENTINEL" "text"
fresh
k2run "$NULL_IN" remote-session write s1 --text-file "$W/in/nl.bin" --token-file "$W/in/tok600"
assert_rc "write --text-file (a lone newline)" 2
assert_no_request "write --text-file (a lone newline)"
grep -Fq "k2 remote-session write: --text-file $W/in/nl.bin: no text in the file" "$W/out/stderr" \
    && ok "write --text-file (a lone newline): the line" || bad "write --text-file (a lone newline): no line"
fresh
k2run "$W/in/nl.bin" remote-session write s1 - --keep-newline --token-file "$W/in/tok600"
assert_rc "write - --keep-newline (a lone newline)" 0
assert_write_got "write - --keep-newline (a lone newline)" "$TOKEN_SENTINEL" "$W/in/nl.bin"

# 4. --token-file's mode: group- or world-readable is refused, before any
#    request; owner-only is taken.
for m in 644 640 604; do
    fresh
    k2run "$NULL_IN" remote-session status --token-file "$W/in/tok$m"
    assert_rc "--token-file mode 0$m" 2
    assert_no_request "--token-file mode 0$m"
    if grep -Fq "k2 remote-session: --token-file $W/in/tok$m: its group or others can read it (mode 0$m); run: chmod 600 $W/in/tok$m" "$W/out/stderr"; then
        ok "--token-file mode 0$m: the refusal names the mode and the fix"
    else
        bad "--token-file mode 0$m: no refusal line"; sed 's/^/    /' "$W/out/stderr" >&2
    fi
    assert_secret_kept "--token-file mode 0$m" "$TOKEN_SENTINEL" "token"
done
for m in 600 400; do
    fresh
    k2run "$NULL_IN" remote-session status --token-file "$W/in/tok$m"
    assert_rc "--token-file mode 0$m" 0
    if [ "$(requests_to /cli/remote-session/status)" = 1 ] \
        && "$REAL_PY" -c '
import json, sys
sys.exit(0 if any(json.loads(l)["auth"] == "Bearer " + sys.argv[2] for l in open(sys.argv[1]) if l.strip()) else 1)
' "$TCPLOG" "$TOKEN_SENTINEL"; then
        ok "--token-file mode 0$m: status went with the file's token"
    else
        bad "--token-file mode 0$m: no status request with the file's token"
    fi
    assert_secret_kept "--token-file mode 0$m" "$TOKEN_SENTINEL" "token"
done

# 5. Usage errors send nothing.
fresh
k2run "$NULL_IN" remote-session write s1 - --text-file "$W/in/text.bin" --token-file "$W/in/tok600"
assert_rc "write: - and --text-file" 2
assert_no_request "write: - and --text-file"
grep -Fq 'k2 remote-session write: give the text one way: "text", - (stdin) or --text-file PATH' "$W/out/stderr" \
    && ok "write: - and --text-file: the line" || bad "write: - and --text-file: no line"

fresh
k2run "$W/in/text.bin" remote-session write s1 - --token-stdin
assert_rc "write: - and --token-stdin" 2
assert_no_request "write: - and --token-stdin"
grep -Fq 'k2 remote-session write: - and --token-stdin both read stdin; give the token with --token-file' "$W/out/stderr" \
    && ok "write: - and --token-stdin: the line" || bad "write: - and --token-stdin: no line"
assert_secret_kept "write: - and --token-stdin" "$TEXT_SENTINEL" "text"

fresh
k2run "$NULL_IN" remote-session status --token-file "$W/in/tok600" --token k2rs_other
assert_rc "--token-file and --token" 2
assert_no_request "--token-file and --token"
grep -Fq 'k2 remote-session: --token-file, --token-stdin or --token: give one' "$W/out/stderr" \
    && ok "--token-file and --token: the line" || bad "--token-file and --token: no line"

fresh
k2run "$NULL_IN" remote-session write s1 --keep-newline --token-file "$W/in/tok600"
assert_rc "write --keep-newline with no text" 2
assert_no_request "write --keep-newline with no text"
grep -Fq 'Usage: k2 remote-session write <session-id> ("text" | - | --text-file PATH) [--keep-newline]' "$W/out/stderr" \
    && ok "write --keep-newline with no text: the usage line" || bad "write --keep-newline with no text: no usage line"

fresh
k2run "$NULL_IN" remote-session write s1 "ls" --keep-newline --token-file "$W/in/tok600"
assert_rc "write \"text\" --keep-newline" 2
assert_no_request "write \"text\" --keep-newline"
grep -Fq 'k2 remote-session write: --keep-newline goes with - or --text-file' "$W/out/stderr" \
    && ok "write \"text\" --keep-newline: the line" || bad "write \"text\" --keep-newline: no line"

fresh
k2run "$W/in/empty" remote-session write s1 - --token-file "$W/in/tok600"
assert_rc "write -: empty stdin" 2
assert_no_request "write -: empty stdin"
grep -Fq 'k2 remote-session write: -: no text on stdin' "$W/out/stderr" \
    && ok "write -: empty stdin: the line" || bad "write -: empty stdin: no line"

fresh
k2run "$NULL_IN" remote-session write s1 --text-file "$W/in/no-such-file" --token-file "$W/in/tok600"
assert_rc "write --text-file: no such file" 2
assert_no_request "write --text-file: no such file"
grep -Fq "k2 remote-session write: --text-file $W/in/no-such-file: No such file or directory" "$W/out/stderr" \
    && ok "write --text-file: no such file: the line" || bad "write --text-file: no such file: no line"

# 6. The old forms still work: the text word (in the URL, as before),
#    --token, and $K2_REMOTE_TOKEN with --host.
fresh
k2run "$NULL_IN" remote-session write s1 "ls -la" --token k2rs_argvform
assert_rc "write \"text\" --token" 0
if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == "/cli/terminal/write"]
sys.exit(0 if len(rows) == 1 and "message=ls%20-la" in rows[0]["target"] and rows[0]["auth"] == "Bearer k2rs_argvform" else 1)
' "$TCPLOG"; then ok "write \"text\" --token: sent as before"
else bad "write \"text\" --token: not sent as before"; fi

fresh
export K2_REMOTE_TOKEN=k2rt_envform
k2run "$NULL_IN" remote-session status --host "http://127.0.0.1:$PORT"
unset K2_REMOTE_TOKEN
assert_rc "status --host with K2_REMOTE_TOKEN" 0
if "$REAL_PY" -c '
import json, sys
sys.exit(0 if any(json.loads(l)["auth"] == "Bearer k2rt_envform" for l in open(sys.argv[1]) if l.strip()) else 1)
' "$TCPLOG"; then ok "status --host with K2_REMOTE_TOKEN: sent with it"
else bad "status --host with K2_REMOTE_TOKEN: not sent with it"; fi

if [ "$HAVE_PROC" = 1 ]; then
    echo "remote_session_secret_input: PATH stand-ins and the /proc scanner"
else
    echo "remote_session_secret_input: PATH stand-ins (this system has no /proc; the Linux runs add the scanner)"
fi
echo "remote_session_secret_input: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
