#!/usr/bin/env bash
# Codex shared app-server — the CLI refuses a K2 session passport that is not
# this tab's, with a clear error instead of a bare 401, and never falls back
# to the owner token.
#
# Codex 0.155+ runs tool commands under ONE detached `codex app-server
# --listen unix://` per CODEX_HOME. It keeps the env of whichever K2 session
# started it, so a Codex tool in another tab carries that session's (or a
# dead session's) K2_HOOK_SOCK / K2_HOOK_TOKEN.
#
# Covers:
#   1. Pure ancestry matcher over fake `ps` tables: the 0.161 and 0.160
#      shared-server shapes match; a --no-daemon TUI and a private stdio
#      app-server do not.
#   2. e2e: ID verb under a shared app-server (a stand-in ps) with
#      K2_CODEX_PASSPORT_BIND=0 (today's id-tool gate) → exit 3 + "shared
#      Codex app-server", nothing sent; with the switch on, the Codex tab
#      gate refuses first, naming the shared server, from the stand-in and
#      from the real process tree (`exec -a`).
#   3. e2e: K2_HOOK_SOCK gone + daemon rejects the scoped token → exit 3 +
#      "credentials are stale"; only the scoped whoami probe was sent.
#   4. e2e: K2_HOOK_SOCK gone but the daemon accepts the token (socket bind
#      failed) → the verb proceeds with the scoped token.
#   5. Outside Codex, open verbs and non-cell shells are untouched.
#   6. Codex tab binding (K2_CODEX_TAB_ISOLATED, switch on by default), one
#      case per source: a confirmed tab sends today's credential plus the
#      conversation headers (socket and TCP); hand-typed codex (outside K2
#      and in a K2 shell tab), a tab from before the marker, a Codex K2
#      could not probe (blank marker), a shared ancestor and a marker from
#      another tab each exit 3 with nothing sent, open and id verbs alike.
#      The owner token is never sent and the headers are never in argv.
#   7. K2_CODEX_PASSPORT_BIND=0 restores today's behaviour (no headers).
#   8. Unconfirmed: help, schema, version and explain run; `<verb> --help`
#      and `-h` print `k2 help <verb>` and send nothing; explain with a
#      Markdown kit id (answered by the daemon) is refused.
#   9. The headers' sources: _k2_codex_cfg, _k5_py_headers, and _k2_curl
#      (config on stdin, or a config file of its own for a stdin body).
#  10. Where ps can't run: the parent walk from python3 decides; every way
#      refused means the marker alone (one note); garbage or a partial
#      answer counts as shared.
#
# No daemon build; HTTP is a tiny python stub. HOME is sandboxed.
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_BASH_SOURCE"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if [[ "$hay" == *"$needle"* ]]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if [[ "$hay" != *"$needle"* ]]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}

# ── 1. Pure ancestry matcher ─────────────────────────────────────────
echo "== ancestry matcher =="
eval "$(sed -n '/^# BEGIN_CLI_STALE_PASSPORT/,/^# END_CLI_STALE_PASSPORT/p' "$K2_CLI")"

matches() {
    if printf '%s\n' "$2" | _cli_ancestry_has_shared_codex_server "$1"; then echo yes; else echo no; fi
}

# z3mbpZ, Codex 0.161: pid-update-loop parent, unix listener child.
Z3="$(cat <<'EOF'
    1     0 /sbin/launchd
  200     1 /Users/u/.codex/packages/app-server-daemon/releases/0.161.0-x/bin/codex app-server daemon pid-update-loop
  300   200 /Users/u/.codex/packages/app-server-daemon/releases/0.161.0-x/bin/codex app-server --listen unix://
  400   300 /bin/zsh -lc k2 thread k2/cortana hi
  500   400 /bin/bash /usr/local/bin/k2 thread k2/cortana hi
EOF
)"
assert_eq "0.161 shared server matches" "$(matches 500 "$Z3")" "yes"

# teachcast, Codex 0.160: detached managed daemon.
TC="$(cat <<'EOF'
    1     0 /sbin/init
  300     1 codex app-server --listen unix:// --managed-daemon
  400   300 /bin/bash -c k2 msg x hi
  500   400 /bin/bash /usr/bin/k2 msg x hi
EOF
)"
assert_eq "0.160 managed daemon matches" "$(matches 500 "$TC")" "yes"

# K2-launched Codex with --no-daemon: tool shell → TUI → k2-daemon.
ND="$(cat <<'EOF'
    1     0 /sbin/launchd
  100     1 /Applications/K2.app/Contents/MacOS/k2-daemon
  250   100 codex --no-daemon --yolo resume 0199
  400   250 /bin/zsh -lc k2 thread k2/cortana hi
  500   400 /bin/bash /usr/local/bin/k2 thread k2/cortana hi
EOF
)"
assert_eq "--no-daemon TUI does not match" "$(matches 500 "$ND")" "no"

# A private stdio app-server owned by the TUI is this session's.
PS="$(cat <<'EOF'
    1     0 /sbin/launchd
  100     1 /Applications/K2.app/Contents/MacOS/k2-daemon
  250   100 codex --no-daemon
  300   250 codex app-server --listen stdio://
  500   300 /bin/bash /usr/local/bin/k2 msg x hi
EOF
)"
assert_eq "private stdio app-server does not match" "$(matches 500 "$PS")" "no"

# Unknown start pid → no match (never loops).
assert_eq "unknown pid does not match" "$(matches 999 "$Z3")" "no"

# ── e2e setup ────────────────────────────────────────────────────────
WORK="$(mktemp -d -t k2-codex-shared-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-aaaa"
SCOPED="sess42.scoped-secret-bbbb"
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
echo "9999" >"$HOME/.k2/heartbeat.port"
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
# `reject` present → the stub answers whoami with the daemon's auth refusal.
: >"$WORK/requests.log"

python3 - "$WORK" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    via = "tcp"

    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {
            "via": self.via,
            "method": method,
            "path": parsed.path,
            "conversation": self.headers.get("X-K2-Conversation") or "",
            "thread": self.headers.get("X-K2-Thread") or "",
            "token_query": (q.get("token") or [""])[0],
            "authorization": self.headers.get("Authorization") or "",
            "body": body.decode("utf-8", "replace"),
        }
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        if parsed.path == "/cli/whoami" and os.path.exists(os.path.join(work, "reject")):
            data = json.dumps({"error": "Invalid or missing auth token"}).encode()
            self.send_response(403)
        else:
            data = json.dumps({"ok": True, "delivered": True, "role": "agent"}).encode()
            self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

# The same answers on a unix socket, standing in for the tab's cell socket.
class UH(H):
    via = "sock"

    def address_string(self):
        return "cell-socket"

class US(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

us = US(os.path.join(work, "cell.sock"), UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

GONE_SOCK="$WORK/run/cells/dead-session.sock"

# Run k2 in a cell env. Extra env assignments come first as NAME=VALUE args.
cell() {
    env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        -u CODEX_SESSION_ID -u K2_CODEX_TAB_ISOLATED -u K2_CODEX_PASSPORT_BIND -u K5_HOOK_SOCK -u K5_HOOK_TOKEN \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_PROJECT_PATH="$WORK/proj" "$@"
}
requests() { cat "$WORK/requests.log"; }

# ── 2. Under a shared Codex app-server ───────────────────────────────
echo "== under a shared Codex app-server =="
# A stand-in `ps` puts a shared server above the call (some hosts, such as
# a sandboxed macOS build node, don't let a job run the real one). The k2
# shell runs ps as its own child, so the stand-in names it by $PPID.
mkdir -p "$WORK/psbin"
cat >"$WORK/psbin/ps" <<'EOF'
#!/bin/sh
printf '%s\n' "    1     0 /sbin/init" \
    "  300     1 codex app-server --listen unix://" \
    "  400   300 /bin/zsh -lc k2" \
    "$PPID   400 /bin/bash k2"
EOF
chmod +x "$WORK/psbin/ps"

# With the switch off (today's behaviour), the id-tool check refuses.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" CODEX_THREAD_ID="thread-1" K2_CODEX_PASSPORT_BIND=0 PATH="$WORK/psbin:$PATH" \
    "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "shared server: exit 3" "$rc" "3"
assert_contains "shared server: names the cause" "$(cat "$WORK/err")" "shared Codex app-server started by another session"
assert_contains "shared server: says what to do" "$(cat "$WORK/err")" "Restart Codex in this tab"
assert_eq "shared server: nothing reached the daemon" "$(requests)" ""
assert_absent "shared server: no bare token error" "$(cat "$WORK/err")" "Invalid or missing auth token"

# Switch on (the default): the Codex tab gate refuses first, even with a
# matching marker, and names the shared server.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" CODEX_THREAD_ID="thread-1" K2_CODEX_TAB_ISOLATED="sess42" PATH="$WORK/psbin:$PATH" \
    "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "shared server, switch on: exit 3" "$rc" "3"
assert_contains "shared server, switch on: reopen from K2" "$(cat "$WORK/err")" "reopen it from K2"
assert_contains "shared server, switch on: names the shared server" "$(cat "$WORK/err")" "It runs under a shared Codex app-server"
assert_eq "shared server, switch on: nothing reached the daemon" "$(requests)" ""

# The real process tree: `exec -a` names the parent bash like the shared
# server (the trailing exit keeps bash from exec'ing k2 in its place). Read
# by the real ps, or by the parent walk where ps can't run.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" CODEX_THREAD_ID="thread-1" K2_CODEX_TAB_ISOLATED="sess42" K2="$K2_CLI" \
    bash -c 'exec -a "codex app-server --listen unix://" bash -c "\"\$K2\" thread k2/cortana hello; exit \$?"' \
    >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "real tree, switch on: exit 3" "$rc" "3"
assert_contains "real tree, switch on: names the shared server" "$(cat "$WORK/err")" "It runs under a shared Codex app-server"
assert_eq "real tree, switch on: nothing reached the daemon" "$(requests)" ""

# Same tree without CODEX_* (not a Codex tool) → no ancestry refusal.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" K2="$K2_CLI" \
    bash -c 'exec -a "codex app-server --listen unix://" bash -c "\"\$K2\" thread k2/cortana hello; exit \$?"' \
    >"$WORK/out" 2>"$WORK/err"
set -e
assert_absent "no CODEX_* env: no shared-server refusal" "$(cat "$WORK/err")" "shared Codex app-server started by another session"

# ── 3. Dead session: socket gone + token rejected ────────────────────
echo "== dead session passport =="
: >"$WORK/requests.log"
touch "$WORK/reject"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_eq "dead session: exit 3" "$rc" "3"
assert_contains "dead session: names the cause" "$(cat "$WORK/err")" "credentials are stale"
assert_contains "dead session: names the socket" "$(cat "$WORK/err")" "$GONE_SOCK"
assert_contains "dead session: Codex hint" "$(cat "$WORK/err")" "restart Codex in this tab"
n_req="$(grep -c . "$WORK/requests.log" || true)"
assert_eq "dead session: only the whoami probe was sent" "$n_req" "1"
assert_contains "dead session: probe is whoami" "$(requests)" '"path": "/cli/whoami"'
assert_contains "dead session: probe carries the scoped Bearer" "$(requests)" "Bearer $SCOPED"
assert_absent "dead session: owner token never sent" "$(requests)" "$OWNER"

# ID verb from mail too (locked tool) → same refusal.
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" msg cortana hello >/dev/null 2>"$WORK/err"
rc=$?
set -e
assert_eq "dead session msg: exit 3" "$rc" "3"
assert_absent "dead session msg: owner token never sent" "$(requests)" "$OWNER"

# ── 4. Socket gone but token still valid (bind failed) → proceeds ────
echo "== socket missing, token valid =="
rm -f "$WORK/reject"
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" thread k2/cortana hello >"$WORK/out" 2>"$WORK/err"
set -e
assert_absent "valid token: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
n_req="$(grep -c . "$WORK/requests.log" || true)"
if [ "$n_req" -ge 2 ]; then
    echo "  PASS: valid token: the verb's own request followed the probe"; pass=$((pass + 1))
else
    echo "  FAIL: valid token: expected probe + verb request, got $n_req ($(requests))" >&2
    fail=$((fail + 1))
fi
assert_absent "valid token: owner token never sent" "$(requests)" "$OWNER"

# ── 5. Outside Codex, open verbs and non-cell shells are untouched ───
# A Codex call is not: section 6 refuses an open verb in a Codex tab K2
# can't confirm.
echo "== open verbs / non-cell, not Codex =="
touch "$WORK/reject"
: >"$WORK/requests.log"
set +e
cell K2_HOOK_SOCK="$GONE_SOCK" "$K2_CLI" activity >/dev/null 2>"$WORK/err"
set -e
assert_absent "open verb: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
assert_absent "open verb: no whoami probe" "$(requests)" '"/cli/whoami"'

: >"$WORK/requests.log"
set +e
env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2SO_PORT \
    K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj" \
    "$K2_CLI" thread k2/cortana hello >/dev/null 2>"$WORK/err"
set -e
assert_absent "external shell: no stale refusal" "$(cat "$WORK/err")" "credentials are stale"
assert_absent "external shell: no whoami probe" "$(requests)" '"/cli/whoami"'

# ── 6. Codex tab binding, one case per source ────────────────────────
echo "== codex tab binding =="
rm -f "$WORK/reject"
LIVE_SOCK="$WORK/cell.sock"
[ -S "$LIVE_SOCK" ] || { echo "FAIL: the stub's cell socket is missing: $LIVE_SOCK" >&2; exit 1; }
MARK="${SCOPED%%.*}"
REAL_CURL="$(command -v curl)"
# A curl on PATH that logs its argv, then runs the real one.
mkdir -p "$WORK/bin"
cat >"$WORK/bin/curl" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >>"$WORK/curl-argv.log"
exec "$REAL_CURL" "\$@"
EOF
chmod +x "$WORK/bin/curl"
CODEX_ENV=(CODEX_THREAD_ID=thread-1 CODEX_SESSION_ID=root-1 PATH="$WORK/bin:$PATH")
argv_log() { cat "$WORK/curl-argv.log"; }
reset_logs() { : >"$WORK/requests.log"; : >"$WORK/curl-argv.log"; }

# k2 as a Codex tool call in a cell: run_codex NAME=VALUE... -- ARGS...
run_codex() {
    local -a envs=()
    while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    reset_logs
    set +e
    cell "${CODEX_ENV[@]}" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}
# k2 as a Codex tool call in a terminal outside K2: no passport, no socket.
run_codex_outside() {
    reset_logs
    set +e
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K5_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K5_HOOK_TOKEN \
        -u K2SO_PORT -u K2_CODEX_TAB_ISOLATED -u K2_CODEX_PASSPORT_BIND \
        K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj" "${CODEX_ENV[@]}" \
        "$K2_CLI" "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}
# k2 as a Codex tool call with a shared Codex server above it (the
# stand-in ps of section 2), its marker matching its passport.
run_codex_shared() {
    reset_logs
    set +e
    cell "${CODEX_ENV[@]}" K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="$MARK" PATH="$WORK/psbin:$WORK/bin:$PATH" \
        "$K2_CLI" "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}
assert_refused() {
    assert_eq "$1: exit 3" "$rc" "3"
    assert_contains "$1: says to reopen from K2" "$(cat "$WORK/err")" \
        "this Codex tab was started before this update or outside K2: reopen it from K2"
    assert_contains "$1: names K2_CODEX_NO_DAEMON=1" "$(cat "$WORK/err")" "K2_CODEX_NO_DAEMON=1"
    assert_eq "$1: nothing reached the daemon" "$(requests)" ""
    assert_eq "$1: curl never ran" "$(argv_log)" ""
    assert_absent "$1: owner token never sent" "$(requests)" "$OWNER"
}
# "<requests> <requests with wrong headers>": WANT on = both conversation
# headers on every request, off = none on any.
hdr_summary() {
    python3 - "$WORK/requests.log" "$1" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
on = sys.argv[2] == "on"
if on:
    bad = [r for r in rows if (r["conversation"], r["thread"]) != ("root-1", "thread-1")]
else:
    bad = [r for r in rows if r["conversation"] or r["thread"]]
print("%d %d" % (len(rows), len(bad)))
PY
}
# The Authorization values the daemon saw, sorted, one line.
auths() {
    python3 - "$WORK/requests.log" <<'PY'
import json, sys
print(",".join(sorted({json.loads(l)["authorization"] for l in open(sys.argv[1]) if l.strip()})))
PY
}
assert_headers_on() {
    local s n bad
    s="$(hdr_summary on)"; n="${s% *}"; bad="${s#* }"
    if [ "$n" -ge 1 ] && [ "$bad" = 0 ]; then
        echo "  PASS: $1: every request ($n) carries X-K2-Conversation and X-K2-Thread"; pass=$((pass + 1))
    else
        echo "  FAIL: $1: requests=$n without both headers=$bad ($(requests))" >&2; fail=$((fail + 1))
    fi
    assert_absent "$1: root id never in curl argv" "$(argv_log)" "root-1"
    assert_absent "$1: thread id never in curl argv" "$(argv_log)" "thread-1"
    assert_absent "$1: no header in curl argv" "$(argv_log)" "X-K2-"
    if [ -s "$WORK/curl-argv.log" ]; then
        echo "  PASS: $1: curl ran through the argv logger"; pass=$((pass + 1))
    else
        echo "  FAIL: $1: the argv logger saw no curl call" >&2; fail=$((fail + 1))
    fi
}

# A confirmed tab, id verb over the cell socket: its passport, the headers.
run_codex K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="$MARK" -- msg cortana hello
assert_absent "confirmed tab, socket: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
assert_contains "confirmed tab, socket: sent over the cell socket" "$(requests)" '"via": "sock"'
assert_contains "confirmed tab, socket: sends its passport" "$(requests)" "Bearer $SCOPED"
assert_absent "confirmed tab, socket: owner token never sent" "$(requests)" "$OWNER"
assert_headers_on "confirmed tab, socket"

# A confirmed tab, open verb over TCP: today's credential, plus the headers.
run_codex K2_HOOK_SOCK="$GONE_SOCK" K2_CODEX_TAB_ISOLATED="$MARK" -- activity
assert_absent "confirmed tab, TCP: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
assert_contains "confirmed tab, TCP: sent over TCP" "$(requests)" '"via": "tcp"'
assert_headers_on "confirmed tab, TCP"
on_auth="$(auths)"
run_codex K2_HOOK_SOCK="$GONE_SOCK" K2_CODEX_TAB_ISOLATED="$MARK" K2_CODEX_PASSPORT_BIND=0 -- activity
off_auth="$(auths)"
assert_eq "confirmed tab, TCP: today's credential (as with the switch off)" "$on_auth" "$off_auth"
[ -n "$on_auth" ] || { echo "  FAIL: confirmed tab, TCP: no credential was sent" >&2; fail=$((fail + 1)); }

# Hand-typed codex in a terminal outside K2: the owner token on disk is
# never used; nothing is sent.
run_codex_outside activity
assert_refused "hand-typed codex outside K2, open verb"

# Hand-typed codex in a K2 shell tab: the tab's passport, no marker.
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- thread k2/cortana hello
assert_refused "hand-typed codex in a K2 shell tab"

# A tab K2 opened before this update: no marker, open and id verbs alike.
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- activity
assert_refused "tab from before the update, open verb"
run_codex K2_HOOK_SOCK="$GONE_SOCK" -- activity
assert_refused "tab from before the update, open verb over TCP"
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- msg cortana hello
assert_refused "tab from before the update, id verb"

# A Codex K2 could not probe for --no-daemon: the spawner left the marker blank.
run_codex K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED= -- thread k2/cortana hello
assert_refused "unprobed Codex (blank marker)"

# A shared Codex app-server above the call, even with a matching marker.
run_codex_shared activity
assert_refused "shared ancestor, open verb"
assert_contains "shared ancestor: names the shared server" "$(cat "$WORK/err")" "It runs under a shared Codex app-server"

# A marker from another tab (another passport's session).
run_codex K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="sess99" -- activity
assert_refused "marker from another tab, open verb"
run_codex K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="sess99" -- msg cortana hello
assert_refused "marker from another tab, id verb"

# ── 7. K2_CODEX_PASSPORT_BIND=0: today's behaviour ───────────────────
echo "== switch off =="
run_codex K2_HOOK_SOCK="$GONE_SOCK" K2_CODEX_PASSPORT_BIND=0 -- activity
assert_absent "switch off: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
s="$(hdr_summary off)"
if [ "${s% *}" -ge 1 ] && [ "${s#* }" = 0 ]; then
    echo "  PASS: switch off: the open verb is sent, with no conversation headers"; pass=$((pass + 1))
else
    echo "  FAIL: switch off: requests/with headers = $s ($(requests))" >&2; fail=$((fail + 1))
fi
assert_eq "switch off: today's credential" "$(auths)" "$off_auth"

# ── 8. Unconfirmed: only local words; --help prints k2 help <verb> ───
echo "== unconfirmed: local words =="
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- help msg
assert_eq "k2 help msg: exit 0" "$rc" "0"
assert_eq "k2 help msg: nothing sent" "$(requests)" ""
want_msg="$(cat "$WORK/out")"
[ -n "$want_msg" ] || { echo "  FAIL: k2 help msg printed nothing" >&2; fail=$((fail + 1)); }
for form in "msg --help" "msg cortana hello -h" "msg -h"; do
    # shellcheck disable=SC2086
    run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- $form
    assert_eq "k2 $form: exit 0" "$rc" "0"
    assert_eq "k2 $form: prints k2 help msg" "$(cat "$WORK/out")" "$want_msg"
    assert_eq "k2 $form: nothing sent" "$(requests)" ""
    assert_eq "k2 $form: curl never ran" "$(argv_log)" ""
done
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- help agent
want_agent="$(cat "$WORK/out")"
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- agent list -h
assert_eq "k2 agent list -h: exit 0" "$rc" "0"
assert_eq "k2 agent list -h: prints k2 help agent" "$(cat "$WORK/out")" "$want_agent"
assert_eq "k2 agent list -h: nothing sent" "$(requests)" ""
for form in "help" "--help" "schema" "version" "explain"; do
    # shellcheck disable=SC2086
    run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- $form
    assert_eq "k2 $form: exit 0" "$rc" "0"
    assert_eq "k2 $form: nothing sent" "$(requests)" ""
    assert_eq "k2 $form: curl never ran" "$(argv_log)" ""
done
run_codex K2_HOOK_SOCK="$LIVE_SOCK" -- explain MPC-E001
assert_refused "explain with a Markdown kit id"

# ── 9. Where the headers come from ───────────────────────────────────
echo "== header sources =="
eval "$(sed -n '/^# BEGIN_CLI_CURL_CFG/,/^# END_CLI_CURL_CFG/p' "$K2_CLI")"
eval "$(sed -n '/^# BEGIN_CLI_CODEX_TAB_BIND/,/^# END_CLI_CODEX_TAB_BIND/p' "$K2_CLI")"
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; _k2_codex_cfg; printf '%s' "$_K2_CODEX_CFG")"
assert_eq "_k2_codex_cfg: both headers as config lines" "$got" \
    "$(printf 'header = "X-K2-Conversation: root-1"\nheader = "X-K2-Thread: thread-1"')"
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; _k5_py_headers)"
assert_eq "_k5_py_headers: the embedded Python gets both" "$got" \
    "$(printf 'X-K2-Conversation: root-1\nX-K2-Thread: thread-1')"
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; K2_CODEX_PASSPORT_BIND=0; _k5_py_headers; _k2_codex_cfg; printf '%s' "$_K2_CODEX_CFG")"
assert_eq "switch off: no headers" "$got" ""
got="$(unset CODEX_THREAD_ID; CODEX_SESSION_ID=root-1; _k2_codex_cfg; printf '%s' "$_K2_CODEX_CFG")"
assert_eq "not a Codex call: no headers" "$got" ""
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID="$(printf 'root\nX-Evil: 1')"; _k5_py_headers)"
assert_eq "an id with a line break is not sent" "$got" "X-K2-Thread: thread-1"

# _k2_curl with a stand-in curl: CFG lines (curl -K -) or, for a stdin
# body with no CFG, a config file of its own that is gone afterwards.
mkdir -p "$WORK/tmpd"
fake_curl='curl() {
    local f=""
    while [ $# -gt 0 ]; do case "$1" in -K) f="$2"; shift 2 ;; *) shift ;; esac; done
    printf "cfg:%s\n" "$f"
    if [ -n "$f" ]; then cat "$f"; fi
    printf "body:"
    if [ -n "$f" ] && [ "$f" != - ]; then cat; fi
}'
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; TMPDIR="$WORK/tmpd"; eval "$fake_curl"
    printf 'the-body' | _k2_curl "" -s -X POST --data-binary @- "http://127.0.0.1:1/cli/x")"
assert_contains "stdin body: headers in a config file" "$got" 'header = "X-K2-Conversation: root-1"'
assert_contains "stdin body: thread header too" "$got" 'header = "X-K2-Thread: thread-1"'
assert_contains "stdin body: the body still reaches curl" "$got" "body:the-body"
assert_absent "stdin body: not config on stdin" "$got" "cfg:-"
assert_eq "stdin body: the config file is removed" "$(ls -A "$WORK/tmpd")" ""
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; eval "$fake_curl"
    _k2_curl 'url = "http://127.0.0.1:1/cli/x"' -s)"
assert_contains "with a config: added to it" "$got" "$(printf 'cfg:-\nurl = "http://127.0.0.1:1/cli/x"\nheader = "X-K2-Conversation: root-1"')"
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; eval "$fake_curl"
    _k2_curl "" -s "http://127.0.0.1:1/cli/x")"
assert_contains "no config: the headers become one" "$got" "$(printf 'cfg:-\nheader = "X-K2-Conversation: root-1"')"
got="$(CODEX_THREAD_ID=thread-1; CODEX_SESSION_ID=root-1; _K2_CURL_NOT_DAEMON=1; eval "$fake_curl"
    _k2_curl "" -s "https://example.invalid/")"
assert_absent "not the daemon: no headers" "$got" "X-K2-"

# ── 10. The process check where ps can't run ─────────────────────────
# Codex's own macOS sandbox refuses `ps` (codex-cli 0.154.0, exit 127 under
# zsh). The check then walks the parent chain from python3; it counts as
# shared unless a walk is complete, and takes the tab marker alone only
# when every way to look was refused.
echo "== process check without ps =="
REAL_PY="$(command -v python3)"
mkdir -p "$WORK/psdeny" "$WORK/psjunk" "$WORK/pybin"
cat >"$WORK/psdeny/ps" <<'EOF'
#!/bin/sh
echo "/bin/ps: Operation not permitted" >&2
exit 126
EOF
cat >"$WORK/psjunk/ps" <<'EOF'
#!/bin/sh
echo "ps: this is not a process table"
EOF
# python3 as itself, except the ancestry walk when K2T_ANCESTRY says so.
cat >"$WORK/pybin/python3" <<EOF
#!/bin/sh
if [ "\${3:-}" = k2-codex-ancestry ]; then
    case "\${K2T_ANCESTRY:-}" in
        denied) echo "PermissionError: [Errno 1] Operation not permitted" >&2; exit 2 ;;
        broken) echo "1 2" ; exit 3 ;;
    esac
fi
exec "$REAL_PY" "\$@"
EOF
chmod +x "$WORK/psdeny/ps" "$WORK/psjunk/ps" "$WORK/pybin/python3"
CONFIRMED=(K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="$MARK")
note_lines() { grep -c "lets nothing look at the processes" "$WORK/err" || true; }

# ps refused, the walk from python3 works, a shared server above: refused.
reset_logs
set +e
cell "${CODEX_ENV[@]}" "${CONFIRMED[@]}" PATH="$WORK/psdeny:$WORK/bin:$PATH" K2="$K2_CLI" \
    bash -c 'exec -a "codex app-server --listen unix://" bash -c "\"\$K2\" activity; exit \$?"' \
    >"$WORK/out" 2>"$WORK/err"
rc=$?
set -e
assert_refused "ps refused, python walk, shared server above"
assert_contains "ps refused, python walk: finds the shared server" "$(cat "$WORK/err")" "It runs under a shared Codex app-server"

# ps refused, the walk from python3 works, no shared server: sent.
run_codex "${CONFIRMED[@]}" PATH="$WORK/psdeny:$WORK/bin:$PATH" -- msg cortana hello
assert_absent "ps refused, python walk, clear: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
assert_eq "ps refused, python walk, clear: no marker-only note" "$(note_lines)" "0"
assert_contains "ps refused, python walk, clear: sent with its passport" "$(requests)" "Bearer $SCOPED"

# ps garbage, the walk from python3 works: the walk decides (sent).
run_codex "${CONFIRMED[@]}" PATH="$WORK/psjunk:$WORK/bin:$PATH" -- msg cortana hello
assert_absent "ps garbage, python walk: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
assert_contains "ps garbage, python walk: sent" "$(requests)" "Bearer $SCOPED"

# Every way refused: the marker alone, one note on stderr, then sent.
run_codex "${CONFIRMED[@]}" PATH="$WORK/psdeny:$WORK/pybin:$WORK/bin:$PATH" K2T_ANCESTRY=denied -- msg cortana hello
assert_absent "all refused: no refusal" "$(cat "$WORK/err")" "reopen it from K2"
assert_eq "all refused: one marker-only note" "$(note_lines)" "1"
assert_contains "all refused: sent with its passport" "$(requests)" "Bearer $SCOPED"
assert_headers_on "all refused"

# ...but a marker from another tab is still refused there.
run_codex K2_HOOK_SOCK="$LIVE_SOCK" K2_CODEX_TAB_ISOLATED="sess99" PATH="$WORK/psdeny:$WORK/pybin:$WORK/bin:$PATH" K2T_ANCESTRY=denied -- activity
assert_refused "all refused, marker from another tab"

# ps garbage and the walk refused: not every way was refused: shared.
run_codex "${CONFIRMED[@]}" PATH="$WORK/psjunk:$WORK/pybin:$WORK/bin:$PATH" K2T_ANCESTRY=denied -- activity
assert_refused "ps garbage, walk refused"
assert_contains "ps garbage, walk refused: says it can't rule it out" "$(cat "$WORK/err")" "couldn't read the processes above this call"
assert_eq "ps garbage, walk refused: no marker-only note" "$(note_lines)" "0"

# ps refused and the walk partial: shared.
run_codex "${CONFIRMED[@]}" PATH="$WORK/psdeny:$WORK/pybin:$WORK/bin:$PATH" K2T_ANCESTRY=broken -- msg cortana hello
assert_refused "ps refused, walk partial"
assert_contains "ps refused, walk partial: says it can't rule it out" "$(cat "$WORK/err")" "couldn't read the processes above this call"

# ps from the stand-in table missing this process: partial, shared, even
# with the walk refused.
cat >"$WORK/psjunk/ps" <<'EOF'
#!/bin/sh
printf '%s\n' "    1     0 /sbin/init" "  400     1 /bin/zsh"
EOF
run_codex "${CONFIRMED[@]}" PATH="$WORK/psjunk:$WORK/pybin:$WORK/bin:$PATH" K2T_ANCESTRY=denied -- activity
assert_refused "ps table without this process, walk refused"

echo
echo "codex_shared_appserver_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
