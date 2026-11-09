#!/usr/bin/env bash
# `k2 vault run` (prd-vault-for-agents-v1 A1), hermetic: no real daemon.
#   * `k2 vault run --help` has an example and needs no daemon;
#   * no `--`, no command, or no secret is exit 2 and nothing is asked of a
#     daemon; the child's own flags (after `--`) are never read as ours;
#   * against a stub daemon: the CLI asks GET /cli/vault/run-helper, then
#     execs that helper with --vault-run and only names on its argv, with the
#     session's socket and passport in its env, and the helper's exit code
#     is the command's.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d -t k2-vault-run-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && { kill "$STUB_PID" 2>/dev/null; wait "$STUB_PID" 2>/dev/null || true; }; rm -rf "$WORK"' EXIT

# ── no daemon: no port, no token, empty HOME ────────────────────────────
k2n() { env HOME="$WORK" "$K2_CLI" "$@"; }

out="$(k2n vault run --help)" || fail "vault run --help exited non-zero: $out"
printf '%s\n' "$out" | head -1 | grep -Fq "k2 vault run " || fail "run --help first line: $out"
printf '%s' "$out" | grep -Fq "Example: k2 vault run --secret" || fail "run --help has no example: $out"
printf '%s' "$out" | grep -Fq -- "--no-mask" || fail "run --help lacks --no-mask: $out"
printf '%s' "$out" | grep -Fq "not masked" || fail "run --help doesn't say --no-mask stops masking: $out"
printf '%s' "$out" | grep -Fq "hygiene, not a wall" || fail "run --help doesn't say what masking isn't: $out"
if printf '%s' "$out" | grep -Fq "k2 vault — secrets for agents"; then fail "run --help printed the top help"; fi
ok "k2 vault run --help: its own page, no daemon needed, with an example"

# The CLI wants a port and token before any verb but --help; port 1 is dead,
# so a usage error that tried the network would show up as exit 1, not 2.
k2d() { env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }

for args in "--secret A" "--secret A --" "-- ls" "node app.js"; do
    set +e
    # shellcheck disable=SC2086 # word-splitting the case is the point
    err="$(k2d vault run $args 2>&1)"; rc=$?
    set -e
    [ "$rc" = 2 ] || fail "vault run $args: exit $rc ($err)"
    printf '%s' "$err" | grep -Fq "usage: k2 vault run" || fail "vault run $args: $err"
done
set +e
err="$(k2d vault run 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] || fail "vault run with nothing: exit $rc ($err)"
ok "no --, no command or no secret is exit 2 (no daemon needed)"

set +e
err="$(k2d vault run --bogus --secret A -- ls 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && printf '%s' "$err" | grep -Fq "unknown flag '--bogus'" || fail "unknown flag: exit $rc ($err)"
set +e
err="$(k2d vault run --value hunter2 -- ls 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && printf '%s' "$err" | grep -Fq "value_on_argv" || fail "--value: exit $rc ($err)"
if printf '%s' "$err" | grep -Fq "hunter2"; then fail "--value echoed the value: $err"; fi
ok "an unknown flag, and --value, are exit 2; the value is not echoed"

# ── a stub daemon and a fake helper ─────────────────────────────────────
HELPER="$WORK/fake-helper"
cat >"$HELPER" <<'EOF'
#!/usr/bin/env bash
{
    for a in "$@"; do printf 'ARG=%s\n' "$a"; done
    printf 'SOCK=%s\n' "${K2_HOOK_SOCK:-}"
    printf 'TOKEN=%s\n' "${K2_HOOK_TOKEN:-}"
} >"$FAKE_HELPER_LOG"
exit "${FAKE_HELPER_EXIT:-0}"
EOF
chmod +x "$HELPER"
HITS="$WORK/hits.log"

python3 - "$WORK/port" "$HELPER" "$HITS" <<'PY' &
import http.server, json, os, socketserver, sys
port_file, helper, hits = sys.argv[1:4]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _go(self):
        with open(hits, "a") as f: f.write("%s %s\n" % (self.command, self.path.split("?")[0]))
        if self.path.split("?")[0] == "/cli/vault/run-helper":
            data = json.dumps({"helper": helper}).encode()
            self.send_response(200)
        else:
            data = b'{"error":{"code":"not_found","hint":"no route"}}'
            self.send_response(404)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    do_GET = _go
    do_POST = _go
s = socketserver.TCPServer(("127.0.0.1", 0), H)
open(port_file + ".tmp", "w").write(str(s.server_address[1]))
os.rename(port_file + ".tmp", port_file)
s.serve_forever()
PY
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/port" ] && break; sleep 0.05; done
[ -s "$WORK/port" ] || fail "stub daemon never started"
PORT="$(cat "$WORK/port")"

# The stub is on TCP, so no socket is passed for `_sidecar_send`; the helper
# still gets the session's values from the CLI.
k2s() { env HOME="$WORK" K2_PORT="$PORT" K2_HOOK_TOKEN=passport-abc FAKE_HELPER_LOG="$WORK/helper.log" "$K2_CLI" "$@"; }

rm -f "$HITS"
out="$(k2s vault run --secret A --secret B=OTHER --env-file .env.k2 --scope server --no-mask -- grep --help --secret x)" || fail "vault run: $out"
[ "$(cat "$HITS")" = "GET /cli/vault/run-helper" ] || fail "the CLI's only call should be run-helper: $(cat "$HITS")"
python3 - "$WORK/helper.log" <<'PY' || fail "helper argv: $(cat "$WORK/helper.log")"
import sys
lines = open(sys.argv[1]).read().splitlines()
args = [l[4:] for l in lines if l.startswith("ARG=")]
assert args == ["--vault-run", "--secret", "A", "--secret", "B=OTHER", "--env-file", ".env.k2", "--scope", "server", "--no-mask", "--", "grep", "--help", "--secret", "x"], args
assert "TOKEN=passport-abc" in lines, lines
PY
ok "the CLI execs the daemon's helper; flags before -- only, the child's words stay the child's"

out="$(k2s vault run API_KEY -- ls)" || fail "vault run NAME: $out"
grep -Fxq "ARG=API_KEY" "$WORK/helper.log" || fail "NAME: $(cat "$WORK/helper.log")"
ok "NAME alone means --secret NAME"

set +e
FAKE_HELPER_EXIT=7 k2s vault run --secret A -- false; rc=$?
set -e
[ "$rc" = 7 ] || fail "the helper's exit code wasn't passed on: $rc"
ok "the exit code is the helper's (exec, nothing after it)"

set +e
err="$(env HOME="$WORK" FAKE_HELPER_LOG="$WORK/helper.log" K2_PORT="$PORT" "$K2_CLI" vault exec --help 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && printf '%s' "$err" | grep -Fq "did you mean run? (k2 vault run --help)" || fail "vault exec: exit $rc ($err)"
ok "k2 vault exec points at run"

echo "vault_run: $pass passed"
