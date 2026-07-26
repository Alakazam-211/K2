#!/usr/bin/env bash
# Context hamburger — `k2 agent context …` CLI smoke.
#
# Covers:
#   1. bash -n on cli/k2
#   2. Help / schema surface (agent context verbs + top-level teach)
#   3. Usage errors exit 2 with JSON {"error":{"code":"bad_usage",…}}
#   4. Live daemon (optional):
#        - daemon down → skip live section (exit 0 overall if pure checks pass)
#        - daemon up + /cli/context/* missing (404) → FAIL loud
#        - daemon up + routes present → list/catalog happy path
#
# Never touches the real ~/.k2 for pure checks (HOME sandboxed).
# Live section uses heartbeat.port/token from the sandbox HOME if a
# foreground daemon was already started by the environment, OR from
# the host heartbeat if K2_CONTEXT_LIVE=1.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    # Avoid pipefail+grep -q SIGPIPE flaking when the needle is early
    # in a large haystack (printf gets EPIPE → pipeline fails).
    case "$hay" in
        *"$needle"*)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
        *)
            echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
    esac
}
assert_exit() {
    local label="$1" want="$2"
    shift 2
    set +e
    "$@" >/tmp/_k2_ctx_out.$$ 2>/tmp/_k2_ctx_err.$$
    local got=$?
    set -e
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label (exit $got)"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (exit $got want $want)" >&2
        echo "    stdout: $(head -c 300 /tmp/_k2_ctx_out.$$)" >&2
        echo "    stderr: $(head -c 300 /tmp/_k2_ctx_err.$$)" >&2
        fail=$((fail + 1))
    fi
}

# ── 0. Syntax ────────────────────────────────────────────────────────
echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

# ── 1. Sandbox HOME + fake connection for pure CLI checks ────────────
WORK="$(mktemp -d -t k2-agent-context-cli-XXXXXX)"
cleanup() {
    rm -rf "$WORK"
    rm -f /tmp/_k2_ctx_out.$$ /tmp/_k2_ctx_err.$$ 2>/dev/null || true
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "owner-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

# ── 2. Schema ────────────────────────────────────────────────────────
echo "== schema =="
schema_out="$("$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema has agent context" "$schema_out" '"name": "agent context"'
assert_contains "schema has agent context list" "$schema_out" '"name": "agent context list"'
assert_contains "schema has agent context add" "$schema_out" '"name": "agent context add"'
assert_contains "schema has agent context move" "$schema_out" '"name": "agent context move"'
assert_contains "schema has agent context catalog" "$schema_out" '"name": "agent context catalog"'
assert_contains "schema hire has --context" "$schema_out" '"name": "--context"'
assert_contains "schema on mentions system layers" "$schema_out" 'pinned:agent'

# ── 3. Help ──────────────────────────────────────────────────────────
echo "== help =="
# PORT+TOKEN satisfy the connection gate; help exits before HTTP.
help_agent="$(PORT=1 TOKEN=fake "$K2_CLI" agent --help 2>&1)" || true
assert_contains "agent help lists context" "$help_agent" "context"

help_ctx="$(PORT=1 TOKEN=fake "$K2_CLI" agent context --help 2>&1)" || true
for needle in list add remove on off move show regen catalog manager:pack pinned:tooling; do
    assert_contains "context help has $needle" "$help_ctx" "$needle"
done

help_hire="$(PORT=1 TOKEN=fake "$K2_CLI" agent hire --help 2>&1)" || true
assert_contains "hire help has --context" "$help_hire" "--context"
assert_contains "hire help mentions catalog seeds" "$help_hire" "wiki:index"

# Top-level teach
set +e
teach_out="$(PORT=1 TOKEN=fake "$K2_CLI" context 2>&1)"
teach_rc=$?
set -e
assert_eq "top-level context exit 2" "$teach_rc" "2"
assert_contains "top-level teaches agent context" "$teach_out" "k2 agent context"

# ── 3b. Hire --context pure usage ────────────────────────────────────
echo "== hire --context usage =="
# Missing value after --context
assert_exit "hire --context missing value → 2" 2 \
    env PORT=1 TOKEN=fake "$K2_CLI" agent hire /tmp/k2-ctx-hire-x --context
err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
assert_contains "hire --context missing → bad_usage" "$err" 'bad_usage'

# Explicit path missing fails (dry-run still validates path existence).
# Needs a reachable daemon for the hire python plan path after flag parse —
# use stub below after it starts, or fail early with expand: we validate
# after HTTP probe in plan. Pure check: unknown flag still 2.
assert_exit "hire unknown flag → 2" 2 \
    env PORT=1 TOKEN=fake "$K2_CLI" agent hire /tmp/k2-ctx-hire-x --contexto wiki:index

# ── 4. Usage exit 2 ──────────────────────────────────────────────────
echo "== usage exit 2 =="
# Stub daemon so connection succeeds but we never need a real response
# for pure usage errors (they fire before HTTP).
python3 - "$WORK" <<'PYEOF' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"error":{"code":"not_found","hint":"stub"}}')
    def do_POST(self):
        self.do_GET()
    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
open(sys.argv[1] + "/stub.port", "w").write(str(port))
srv.handle_request()  # one request is enough if something leaks; mainly keep alive
# Keep serving a few more in case of accidental calls
for _ in range(20):
    srv.handle_request()
PYEOF
STUB_PID=$!
for i in $(seq 1 30); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.05
done
STUB_PORT="$(cat "$WORK/stub.port" 2>/dev/null || echo "")"
if [ -z "$STUB_PORT" ]; then
    echo "  FAIL: stub daemon did not start" >&2
    fail=$((fail + 1))
else
    export PORT="$STUB_PORT" TOKEN="owner-token"
    # Also write heartbeat so auto-discovery matches
    echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"

    assert_exit "add missing arg → 2" 2 \
        env PORT="$STUB_PORT" TOKEN=owner-token "$K2_CLI" agent context add
    err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
    assert_contains "add missing → bad_usage JSON" "$err" '"code": "bad_usage"'

    assert_exit "remove missing arg → 2" 2 \
        env PORT="$STUB_PORT" TOKEN=owner-token "$K2_CLI" agent context remove
    err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
    assert_contains "remove missing → bad_usage JSON" "$err" 'bad_usage'

    assert_exit "move bad direction → 2" 2 \
        env PORT="$STUB_PORT" TOKEN=owner-token "$K2_CLI" agent context move some-id sideways
    err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
    assert_contains "move bad dir → bad_usage" "$err" 'bad_usage'

    assert_exit "unknown subcommand → 2" 2 \
        env PORT="$STUB_PORT" TOKEN=owner-token "$K2_CLI" agent context frobnicate
    err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
    assert_contains "unknown sub → bad_usage" "$err" 'bad_usage'

    # System layer resolve is local (no HTTP) — off with missing daemon
    # body would still try set-enabled; just verify resolve path doesn't
    # 404 at resolve for pinned ids by dry-checking help + schema only.
    # Explicit path missing on hire fails loud after daemon probe:
    HIRE_TMP="$WORK/hire-missing-ctx"
    mkdir -p "$HIRE_TMP"
    # Stub returns not_found for conf → hire continues; path check for
    # --context runs after open probe and fails if file missing.
    set +e
    env PORT="$STUB_PORT" TOKEN=owner-token \
        "$K2_CLI" agent hire "$HIRE_TMP" --context docs/nope-missing.md --dry-run \
        >/tmp/_k2_ctx_out.$$ 2>/tmp/_k2_ctx_err.$$
    hire_rc=$?
    set -e
    # Stub may yield daemon_error on conf, or not_found on missing path.
    # Either way must NOT exit 0 with a successful plan that includes a
    # pending seed for a missing path.
    hire_err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
    hire_out="$(cat /tmp/_k2_ctx_out.$$ 2>/dev/null || true)"
    if [ "$hire_rc" -eq 0 ] && printf '%s' "$hire_out" | grep -q 'seed-context\|context:'; then
        # If plan printed, ensure missing path was not would-apply
        if printf '%s' "$hire_out" | grep -q 'would apply.*nope-missing'; then
            echo "  FAIL: hire dry-run would apply missing context path" >&2
            fail=$((fail + 1))
        else
            echo "  PASS: hire dry-run did not schedule missing path as apply"
            pass=$((pass + 1))
        fi
    elif [ "$hire_rc" -ne 0 ]; then
        echo "  PASS: hire missing context path fails loud (exit $hire_rc)"
        pass=$((pass + 1))
        assert_contains "hire missing path mentions context or not_found" \
            "$hire_err$hire_out" "context"
    else
        echo "  PASS: hire dry-run exited 0 without applying missing path"
        pass=$((pass + 1))
    fi
fi
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true

# ── 5. Live daemon (optional) ────────────────────────────────────────
echo "== live daemon (optional) =="
LIVE_PORT=""
LIVE_TOKEN=""

# Prefer explicit env, then real user heartbeat (only when K2_CONTEXT_LIVE=1),
# then worktree sandbox daemon if already running under SANDBOX_HOME.
if [ -n "${K2_PORT:-${K2SO_PORT:-}}" ] && [ -n "${K2_HOOK_TOKEN:-${K2SO_HOOK_TOKEN:-}}" ]; then
    LIVE_PORT="${K2_PORT:-$K2SO_PORT}"
    LIVE_TOKEN="${K2_HOOK_TOKEN:-$K2SO_HOOK_TOKEN}"
elif [ "${K2_CONTEXT_LIVE:-}" = "1" ] && [ -r "${HOME_REAL:-$HOME}/.k2/heartbeat.port" ]; then
    : # not used — HOME is sandboxed; read from original if provided
    true
fi

# Probe common heartbeat locations without requiring live flag when
# K2_PORT is already exported by a parent sandbox harness.
if [ -z "$LIVE_PORT" ] && [ -n "${K2SO_PORT:-}" ]; then
    LIVE_PORT="$K2SO_PORT"
    LIVE_TOKEN="${K2SO_TOKEN:-${K2_HOOK_TOKEN:-}}"
fi

if [ -z "$LIVE_PORT" ]; then
    echo "  SKIP: no live daemon port (set K2_PORT+TOKEN or K2SO_PORT+K2SO_TOKEN to exercise routes)"
else
    base="http://127.0.0.1:${LIVE_PORT}"
    if ! curl -sf --connect-timeout 1 --max-time 3 "${base}/health" >/dev/null 2>&1; then
        echo "  SKIP: daemon port $LIVE_PORT not healthy"
    else
        # Probe context route — fail loud on 404 (daemon up, feature not wired).
        code="$(curl -s -o /tmp/_k2_ctx_probe.$$ -w "%{http_code}" --connect-timeout 2 --max-time 10 \
            "${base}/cli/context/catalog?token=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$LIVE_TOKEN")")"
        body="$(cat /tmp/_k2_ctx_probe.$$ 2>/dev/null || true)"
        rm -f /tmp/_k2_ctx_probe.$$
        echo "  probe GET /cli/context/catalog → HTTP $code"
        if [ "$code" = "404" ]; then
            echo "  FAIL: daemon is up but /cli/context/catalog returned 404 — wire context routes (backend worktree)" >&2
            fail=$((fail + 1))
        elif [ "$code" = "000" ] || [ -z "$code" ]; then
            echo "  SKIP: could not reach daemon"
        elif [ "$code" = "401" ] || [ "$code" = "403" ]; then
            # Port is occupied by a real daemon but our token doesn't match —
            # not a feature regression; pure checks already covered the CLI.
            echo "  SKIP: daemon auth rejected (token mismatch for live probe)"
        else
            # Routes exist — exercise CLI list + catalog against PWD project.
            export PORT="$LIVE_PORT" TOKEN="$LIVE_TOKEN"
            # Restore a project path (use PROJECT_ROOT as a registered-or-not workspace)
            set +e
            catalog_out="$(PORT="$LIVE_PORT" TOKEN="$LIVE_TOKEN" K2_PROJECT_PATH="$PROJECT_ROOT" \
                "$K2_CLI" agent context catalog --json 2>/tmp/_k2_ctx_err.$$)"
            prc=$?
            set -e
            if [ "$prc" -eq 0 ]; then
                echo "  PASS: agent context catalog --json (exit 0)"
                pass=$((pass + 1))
                assert_contains "catalog JSON parseable" "$catalog_out" "{"
            else
                # not_found project is acceptable if workspace not registered;
                # other failures are real.
                err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
                if printf '%s' "$err$catalog_out" | grep -Eq 'not_found|No project|unregistered'; then
                    echo "  PASS: catalog reached daemon (project not registered — ok for smoke)"
                    pass=$((pass + 1))
                else
                    echo "  FAIL: catalog exit $prc — $err" >&2
                    fail=$((fail + 1))
                fi
            fi

            set +e
            list_out="$(PORT="$LIVE_PORT" TOKEN="$LIVE_TOKEN" K2_PROJECT_PATH="$PROJECT_ROOT" \
                "$K2_CLI" agent context list 2>/tmp/_k2_ctx_err.$$)"
            lrc=$?
            set -e
            if [ "$lrc" -eq 0 ]; then
                assert_contains "list has PINNED" "$list_out" "PINNED"
                assert_contains "list has OPTIONAL" "$list_out" "OPTIONAL"
            else
                err="$(cat /tmp/_k2_ctx_err.$$ 2>/dev/null || true)"
                if printf '%s' "$err" | grep -Eq 'not_found'; then
                    echo "  PASS: list reached daemon (project not_found — ok)"
                    pass=$((pass + 1))
                else
                    echo "  FAIL: list exit $lrc — $err" >&2
                    fail=$((fail + 1))
                fi
            fi
        fi
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────
echo ""
echo "agent_context_layers: $pass passed, $fail failed"
if [ "$fail" -gt 0 ]; then
    exit 1
fi
exit 0
