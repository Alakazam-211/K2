#!/usr/bin/env bash
# hostmail upgrade (calendars S1: explicit Stalwart upgrade) CLI wiring —
# no daemon, no Stalwart, no root. Static wiring + help + schema + usage
# errors, then a local echo server checks the exact requests (dry-run,
# run + status polling, acknowledge) and the exit codes per outcome.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'upgrade) cmd_hostmail_upgrade' "$K2" || fail "cmd_hostmail must route upgrade"
grep -A40 'verb == "server_upgrade"' "$K2" | grep -q '"/cli/mail/server/upgrade"' \
  || fail "server_upgrade verb must use /cli/mail/server/upgrade"

# Help.
help="$("$K2" hostmail upgrade --help)"
for want in '--dry-run' '--acknowledge-failed' 'snapshot' '/var/lib/stalwart.k2-snap' \
            'rolled_back' 'rollback_failed' 'NEVER automatic' 'Owner/admin' \
            'never re-runs a Maildir import' 'Never uses hostmail disable/enable' \
            'mail helper' '1 GiB' '/cli/mail/server/upgrade'; do
  printf '%s' "$help" | grep -qiF -- "$want" || fail "upgrade --help must mention '$want'"
done
"$K2" hostmail --help | grep -q 'upgrade \[--dry-run\]' || fail "hostmail --help must list upgrade"
"$K2" hostmail enable --help | grep -q 'k2 hostmail upgrade' \
  || fail "enable --help must name k2 hostmail upgrade for a version mismatch"
"$K2" study mail | grep -q 'k2 hostmail upgrade' || fail "study mail must list hostmail upgrade as owner-only"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
grep -qF '"name": "hostmail upgrade"' <<<"$schema" || fail "schema missing hostmail upgrade"
grep -qF 'uninstall|upgrade|rotate-admin' <<<"$schema" || fail "schema hostmail usage must list upgrade"

# Usage errors exit 2 before any request (port 1 = would be unreachable).
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
  if printf '%s' "$out" | grep -q daemon_unreachable; then
    fail "'k2 $*' must not send anything: $out"
  fi
}
expect_usage hostmail upgrade --dry-run --acknowledge-failed
expect_usage hostmail upgrade --now
expect_usage hostmail upgrade 0.16.20

# Live: a local echo server records method, path and body; the reply
# depends on the scenario file.
WORK="$(mktemp -d -t k2-hostmail-upgrade-XXXXXX)"
SRV_PID=""
cleanup() {
  if [ -n "$SRV_PID" ]; then
    kill "$SRV_PID" 2>/dev/null || true
    wait "$SRV_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
  hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$WORK/srv.py" <<'PY'
import http.server, json, sys
log_path, scen_path = sys.argv[1], sys.argv[2]
gets = {"n": 0}
PLAN = {"ok": True, "dryRun": True, "ready": True, "blockers": [],
        "from": "0.16.10", "to": "0.16.20", "state": "running", "wasRunning": True,
        "dataBytes": 3221225472, "configBytes": 4096, "freeBytes": 53687091200,
        "snapshotDir": "/var/lib/stalwart.k2-snap",
        "outage": {"typicalSecs": 61, "worstCaseSecs": 312, "assumption": "copy at ~100 MiB/s"},
        "plan": ["stop Stalwart", "snapshot", "install Stalwart 0.16.20", "health check"],
        "rollback": ["restore the snapshot", "reinstall Stalwart 0.16.10"],
        "notes": ["never re-runs a Maildir import"]}
STEPS = [{"step": "stop", "ok": True, "detail": "stopped (inactive)"},
         {"step": "snapshot", "ok": True, "detail": "snapshot complete"}]
def post_reply(scen, body):
    if scen.startswith("dry"):
        p = dict(PLAN)
        if scen == "dry_blocked":
            p["ready"] = False
            p["blockers"] = [{"code": "mail_helper_outdated",
                              "hint": "the mail helper predates k2 hostmail upgrade — run as root: curl -fsSL https://example.com/install-mail-helper.sh | sudo bash"}]
        return 200, p
    if scen == "noop":
        return 200, {"ok": True, "outcome": "noop", "version": "0.16.20",
                     "hint": "Stalwart 0.16.20 is already the pinned version — nothing to do"}
    if scen == "refused":
        return 409, {"ok": False, "error": {"code": "mail_helper_missing",
                     "hint": "mail helper not installed — run as root: curl -fsSL https://example.com/x | sudo bash"}}
    if scen == "ack":
        return 200, {"ok": True, "outcome": "acknowledged",
                     "hint": "acknowledged — next: k2 hostmail upgrade --dry-run"}
    return 200, {"ok": True, "started": True, "from": "0.16.10", "to": "0.16.20",
                 "outage": {"typicalSecs": 61}}
def status_reply(scen):
    gets["n"] += 1
    if gets["n"] == 1:
        up = {"state": "running", "step": "install", "steps": STEPS[:1]}
    else:
        final = {"run_ok": "succeeded", "run_rb": "rolled_back", "run_rbf": "rollback_failed"}[scen]
        up = {"state": final, "from": "0.16.10", "to": "0.16.20", "steps": STEPS,
              "snapshotDir": "/var/lib/stalwart.k2-snap"}
        if final != "succeeded":
            up["error"] = "health check failed: JMAP ping failed"
        if final == "rollback_failed":
            up["manualSteps"] = ["sudo systemctl stop stalwart",
                                 "k2 hostmail upgrade --acknowledge-failed"]
    return {"ok": True, "consistent": True, "state": "running", "upgrade": up}
class H(http.server.BaseHTTPRequestHandler):
    def _send(self, code, obj, body):
        log = open(log_path, "a")
        log.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        log.close()
        out = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        scen = open(scen_path).read().strip()
        self._send(200, status_reply(scen), "-")
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        scen = open(scen_path).read().strip()
        gets["n"] = 0
        code, obj = post_reply(scen, body)
        self._send(code, obj, body)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
echo dry_ready >"$WORK/scenario"
python3 "$WORK/srv.py" "$WORK/requests" "$WORK/scenario" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    K2_HOSTMAIL_UPGRADE_POLL=0.05 "$K2" "$@"
}
run_rc() {
  # run_rc <scenario> <args…> → sets OUT, ERR, RC
  echo "$1" >"$WORK/scenario"
  shift
  set +e
  OUT="$(k2 "$@" 2>"$WORK/stderr")"
  RC=$?
  set -e
  ERR="$(cat "$WORK/stderr")"
}

run_rc dry_ready hostmail upgrade --dry-run
[ "$RC" -eq 0 ] || fail "ready dry-run must exit 0, got $RC: $OUT $ERR"
printf '%s' "$OUT" | grep -q 'Stalwart 0.16.10 -> 0.16.20  (ready)' || fail "dry-run header: $OUT"
printf '%s' "$OUT" | grep -q '~61s typical; up to 312s' || fail "dry-run must print the outage: $OUT"
printf '%s' "$OUT" | grep -q '3.0 GiB data' || fail "dry-run must print the data size: $OUT"
printf '%s' "$OUT" | grep -q 'restore the snapshot' || fail "dry-run must print the rollback plan: $OUT"

run_rc dry_blocked hostmail upgrade --dry-run
[ "$RC" -eq 1 ] || fail "blocked dry-run must exit 1, got $RC: $OUT"
printf '%s' "$OUT" | grep -q 'BLOCKED' || fail "blocked dry-run: $OUT"
printf '%s' "$OUT" | grep -q 'mail_helper_outdated' || fail "blocked dry-run must print the blocker: $OUT"

run_rc dry_ready hostmail upgrade --dry-run --json
[ "$RC" -eq 0 ] || fail "dry-run --json rc $RC"
printf '%s' "$OUT" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["dryRun"] and d["ready"], d' \
  || fail "dry-run --json must be the daemon body: $OUT"

run_rc run_ok hostmail upgrade
[ "$RC" -eq 0 ] || fail "succeeded upgrade must exit 0, got $RC: $OUT $ERR"
printf '%s' "$OUT" | grep -q 'upgraded : Stalwart 0.16.20' || fail "success output: $OUT"
printf '%s' "$OUT" | grep -q 'snapshot' || fail "success output must show steps: $OUT"

run_rc run_rb hostmail upgrade
[ "$RC" -eq 1 ] || fail "rolled_back must exit 1, got $RC: $OUT"
printf '%s' "$OUT" | grep -q 'result   : rolled_back' || fail "rolled_back output: $OUT"
printf '%s' "$OUT" | grep -q 'health check failed' || fail "rolled_back must print the error: $OUT"

run_rc run_rbf hostmail upgrade
[ "$RC" -eq 1 ] || fail "rollback_failed must exit 1, got $RC"
printf '%s' "$ERR" | grep -q 'ROLLBACK FAILED' || fail "rollback_failed must be loud on stderr: $ERR"
printf '%s' "$ERR" | grep -q 'sudo systemctl stop stalwart' || fail "manual steps on stderr: $ERR"

run_rc run_ok hostmail upgrade --json
[ "$RC" -eq 0 ] || fail "--json run rc $RC"
printf '%s' "$OUT" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["state"]=="succeeded", d' \
  || fail "--json must print the final record: $OUT"

run_rc noop hostmail upgrade
[ "$RC" -eq 0 ] || fail "noop must exit 0, got $RC"
printf '%s' "$OUT" | grep -q 'already the pinned version' || fail "noop output: $OUT"

run_rc refused hostmail upgrade
[ "$RC" -eq 1 ] || fail "refusal must exit 1, got $RC"
printf '%s' "$ERR" | grep -q 'mail_helper_missing' || fail "refusal must print the code: $ERR"
printf '%s' "$ERR" | grep -q 'run as root' || fail "refusal must print the root fix: $ERR"

run_rc ack hostmail upgrade --acknowledge-failed
[ "$RC" -eq 0 ] || fail "ack rc $RC"
printf '%s' "$OUT" | grep -q 'acknowledged' || fail "ack output: $OUT"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
posts = [(p, json.loads(b)) for m, p, b in rows if m == "POST"]
want = [{"dryRun": True}, {"dryRun": True}, {"dryRun": True}, {}, {}, {}, {}, {}, {},
        {"acknowledgeFailed": True}]
if [b for _, b in posts] != want:
    sys.exit("FAIL: POST bodies %r != %r" % ([b for _, b in posts], want))
if any(p != "/cli/mail/server/upgrade" for p, _ in posts):
    sys.exit("FAIL: every POST must hit /cli/mail/server/upgrade: %r" % posts)
gets = [p for m, p, _ in rows if m == "GET"]
if not gets or any(p != "/cli/mail/status" for p in gets):
    sys.exit("FAIL: the run must poll /cli/mail/status: %r" % gets)
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: hostmail upgrade CLI wiring"
