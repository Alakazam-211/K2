#!/usr/bin/env bash
# `k2 db run` puts the connection string, password included, in the
# command's environment on purpose (that is the feature). It is never on
# any argv, ours or a helper's, and never in a line k2 prints or logs.
#
# A curl shim and a python3 shim first on PATH log every argv they get,
# then run the real program. A fake daemon answers /cli/db/dsn with a
# connection string holding a password marker. The command dumps its own
# environment to a file. Checks:
#   - the command has DATABASE_URL (and, with --as NAME, NAME) equal to
#     the connection string, password included;
#   - no curl or python3 argv holds the password;
#   - nothing k2 prints (stdout and stderr) holds it, and no file under
#     the temp HOME (where a log would go) holds it.
# And `k2 db run --help` says the connection string, password included, is
# in the command's environment, and names DATABASE_URL.
#
# Hermetic: temp HOME, a fake daemon, no network. Loud.
# Run with: bash tests/cli/db_run_dsn_off_argv.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-db-run-argv-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

PW="PWmarker0123456789secret"
DSN="postgres://ws_a_agent:${PW}@127.0.0.1:5432/ws_a"
REAL_CURL="$(command -v curl)"
REAL_PY="$(command -v python3)"
ARGV_LOG="$WORK/argv.log"
: >"$ARGV_LOG"
mkdir -p "$WORK/shim" "$WORK/ws"
cat >"$WORK/shim/curl" <<EOF
#!/bin/sh
printf 'curl %s\n' "\$*" >> '$ARGV_LOG'
exec '$REAL_CURL' "\$@"
EOF
cat >"$WORK/shim/python3" <<EOF
#!/bin/sh
printf 'python3 %s\n' "\$*" >> '$ARGV_LOG'
exec '$REAL_PY' "\$@"
EOF
chmod +x "$WORK/shim/curl" "$WORK/shim/python3"

"$REAL_PY" -I - "$WORK" "$DSN" <<'PY' &
import http.server, json, sys, urllib.parse
work, dsn = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n: self.rfile.read(n)
        u = urllib.parse.urlparse(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        if u.path == "/cli/db/dsn" and q.get("reveal") == "1":
            out = {"ok": True, "name": "ws_a", "dsn": dsn, "dsnHasPassword": True}
        else:
            out = {"ok": True}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    do_GET = do_POST = _go
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
open(work + "/port", "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
FAKE_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || { echo "FAIL: the fake daemon never started" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN=owner-token
export PATH="$WORK/shim:$PATH"
[ "$(command -v curl)" = "$WORK/shim/curl" ] && [ "$(command -v python3)" = "$WORK/shim/python3" ] \
    || { echo "FAIL: the shims are not first on PATH" >&2; exit 1; }

# run_db <env-name> <args...>: the command writes its env to $WORK/env.<name>.
run_db() {
    local name="$1"; shift
    set +e
    OUT="$("$K2" db run "$@" --workspace "$WORK/ws" -- /bin/sh -c "env > '$WORK/env.$name'" 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

echo "== k2 db run: the command's environment =="
run_db DATABASE_URL
[ "$RC" -eq 0 ] && ok "db run: exit 0" || { bad "db run: exit $RC: $ERR"; }
grep -qxF "DATABASE_URL=$DSN" "$WORK/env.DATABASE_URL" && ok "the command has DATABASE_URL, password included" \
    || bad "no DATABASE_URL=<dsn> in the command's env"
run_db PG_URL --as PG_URL
[ "$RC" -eq 0 ] && ok "db run --as: exit 0" || bad "db run --as: exit $RC: $ERR"
grep -qxF "PG_URL=$DSN" "$WORK/env.PG_URL" && ok "--as PG_URL names the variable" || bad "no PG_URL=<dsn> in the command's env"

echo "== nowhere else =="
grep -q '^curl ' "$ARGV_LOG" || grep -q '^python3 ' "$ARGV_LOG" \
    || bad "the shims saw nothing: the test would prove nothing"
if grep -qF "$PW" "$ARGV_LOG"; then
    bad "the password is on an argv:"; grep -F "$PW" "$ARGV_LOG" | sed 's/^/      | /' >&2
else
    ok "no curl or python3 argv holds the password ($(wc -l <"$ARGV_LOG" | tr -d ' ') argvs)"
fi
case "$OUT$ERR" in *"$PW"*) bad "k2 printed the password" ;; *) ok "k2 printed nothing with the password" ;; esac
if grep -rlF "$PW" "$HOME" 2>/dev/null | grep -q .; then
    bad "a file under HOME holds the password: $(grep -rlF "$PW" "$HOME")"
else
    ok "no file under HOME holds the password"
fi

echo "== the help says so =="
set +e
HELP="$("$K2" db --help 2>&1)"
set -e
case "$HELP" in
    *"run a command with the connection string,"*"password included, in its environment as"*'$DATABASE_URL (or NAME)'*) ok "k2 db --help says the connection string, password included, is in the command's environment as DATABASE_URL" ;;
    *) bad "k2 db --help doesn't say where the connection string goes" ;;
esac

echo ""
echo "db_run_dsn_off_argv: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
