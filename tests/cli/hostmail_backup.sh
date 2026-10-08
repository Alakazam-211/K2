#!/usr/bin/env bash
# hostmail backup (S8 B1: the per-box backup choice) CLI wiring — no daemon,
# no Stalwart. Static wiring + help + schema + study + usage errors (exit 2
# before any request), then a local echo server checks the exact requests
# (GET status, GET plan with params, POST set body) and the human output.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'backup)  cmd_hostmail_backup' "$K2" || fail "cmd_hostmail must route backup"
grep -q '"/cli/mail/backup/set"' "$K2" || fail "backup set must POST /cli/mail/backup/set"
grep -q '"/cli/mail/backup/plan"' "$K2" || fail "backup plan must GET /cli/mail/backup/plan"

# Help.
group="$("$K2" hostmail backup --help)"
for want in 'unconfigured' 'RUNS NO BACKUPS' 'local' 'offsite-only' 'both' 'none' \
            'it-email agent' 'owner-only' 'NOT disk failure'; do
  printf '%s' "$group" | grep -qF -- "$want" || fail "backup --help must mention '$want'"
done
plan_help="$("$K2" hostmail backup plan --help)"
for want in '--bandwidth-mbps' 'planHash' 'max(5 GiB, 10% of the filesystem)' \
            'measured over N days' 'unmeasured' 'Changes nothing'; do
  printf '%s' "$plan_help" | grep -qF -- "$want" || fail "plan --help must mention '$want'"
done
set_help="$("$K2" hostmail backup set --help)"
for want in '--confirm-plan' '--reason' 'plan_changed' 'later release' '03:30-04:30' \
            '--offsite-keep-monthly' 'old → new'; do
  printf '%s' "$set_help" | grep -qF -- "$want" || fail "set --help must mention '$want'"
done
"$K2" hostmail backup status --help | grep -qF 'GET /cli/mail/backup' || fail "status --help"
"$K2" hostmail --help | grep -qF 'backup status|plan|set' || fail "hostmail --help must list backup"
"$K2" study mail | grep -qF 'k2 hostmail backup plan' || fail "study mail must list backup plan for MAIL_MANAGE agents"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for name in "hostmail backup status" "hostmail backup plan" "hostmail backup set"; do
  grep -qF "\"name\": \"$name\"" <<<"$schema" || fail "schema missing $name"
done
grep -qF 'doctor|backup|config' <<<"$schema" || fail "hostmail usage must list backup"

# Usage errors exit 2 before any request.
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail backup restore
expect_usage hostmail backup status extra
expect_usage hostmail backup plan --keep-daily seven
expect_usage hostmail backup plan --bandwidth-mbps
expect_usage hostmail backup set --confirm-plan abc
expect_usage hostmail backup set --mode weekly --confirm-plan abc
expect_usage hostmail backup set --mode none --confirm-plan abc
expect_usage hostmail backup set --mode local
expect_usage hostmail backup set --mode local --keep-weekly -1 --confirm-plan abc

WORK="$(mktemp -d -t k2-hostmail-backup-XXXXXX)"
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
log = open(sys.argv[1], "a")
G = 1 << 30
BACKUP = {"installed": True, "mode": "unconfigured", "running": False,
          "observer": {"status": "ok", "lastSuccess": 1900000000, "days": 2,
                       "churnPerDay": {"measured": False, "createdBytes": G, "deletedBytes": 2 * G,
                                       "maxDeletedBytes": 3 * G},
                       "storeBytes": 40 * G, "mutableBytes": 200 << 20, "fsFree": 300 * G,
                       "fsTotal": 512 * G, "sameFs": True},
          "note": "Backup not chosen — the box's it-email agent should run `k2 hostmail backup plan` and agree a mode with the client"}
MODE = {"available": True, "firstNightLocalBytes": 300 << 20, "steadyLocalBytes": 90 * G,
        "peakBytes": 90 * G, "freeAfterBytes": 210 * G, "downtimeSecs": 25,
        "downtimeLabel": "conservative estimate, not measured (B0)", "verdict": "ok",
        "protects": "logical damage"}
OFF = dict(MODE, available=False, unavailableReason="offsite destination not set — the owner sets it in a later release (B3).",
           offsite={"firstUploadBytes": {"low": 24 * G, "high": 40 * G}, "firstUploadSecs": 6900,
                    "nightlyUploadBytes": 4 * G, "nightlyUploadSecs": 700,
                    "firstTempBytes": 6 * G, "nightlyTempBytes": 6 * G})
PLAN = {"ok": True, "planHash": "a1b2c3d4e5f6",
        "store": {"path": "/var/lib/stalwart", "bytes": 40 * G, "immutableFiles": 900,
                  "mutableBytes": 200 << 20, "configBytes": 1 << 20, "k2HalfBytes": 50 << 20,
                  "fsFree": 300 * G, "fsTotal": 512 * G, "floorBytes": 51 * G, "sameFs": True,
                  "viewRoot": "/var/lib/stalwart.k2-views"},
        "churn": {"quality": "unmeasured", "label": "unmeasured — conservative bound (10% of the store per day)",
                  "days": 2, "deletedPerDay": 4 * G, "maxDeletedPerDay": 4 * G, "createdPerDay": 4 * G},
        "bandwidthMbps": 100.0,
        "retention": {"local": {"keepDaily": 3, "keepWeekly": 4, "maxLocalGb": None},
                      "offsite": {"keepDaily": 7, "keepWeekly": 4, "keepMonthly": 6}},
        "window": "03:30-04:30",
        "modes": {"local": MODE, "offsite-only": OFF, "both": OFF,
                  "none": {"available": True, "verdict": "ok", "peakBytes": 0}},
        "notes": ["B1 runs no backups: nothing stops Stalwart and nothing is copied until a later release."]}
SET = {"ok": True, "changed": True, "from": "unconfigured",
       "backup": dict(BACKUP, mode="local", configuredBy="agent:it-email", configuredAt=1900000000,
                      window="03:30-04:30",
                      retention={"local": {"keepDaily": 3, "keepWeekly": 4},
                                 "offsite": {"keepDaily": 7, "keepWeekly": 4, "keepMonthly": 6}},
                      note="mode chosen; backups start in a later release (B2: local frozen views, B3: offsite) — nothing runs yet")}
class H(http.server.BaseHTTPRequestHandler):
    def _reply(self, body, reply):
        log.write("%s %s %s\n" % (self.command, self.path.split("&token=")[0].split("?token=")[0], body))
        log.flush()
        out = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/cli/mail/backup/plan":
            self._reply("-", PLAN)
        else:
            self._reply("-", {"ok": True, "backup": BACKUP})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(self.rfile.read(n).decode(), SET)
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    "$K2" "$@"
}

st="$(k2 hostmail backup status)"
printf '%s' "$st" | grep -q '^mode     : unconfigured' || fail "status output: $st"
printf '%s' "$st" | grep -qF 'k2 hostmail backup plan' || fail "status must carry the unconfigured note: $st"
printf '%s' "$st" | grep -q '^churn    : deleted 2.0 GiB/day (worst 3.0 GiB), new 1.0 GiB/day — not yet measured' \
  || fail "status churn line: $st"

plan="$(k2 hostmail backup plan --bandwidth-mbps 100 --keep-daily 3)"
printf '%s' "$plan" | grep -q '^planHash : a1b2c3d4e5f6' || fail "plan must print planHash: $plan"
printf '%s' "$plan" | grep -q '^local        verdict OK' || fail "plan local verdict: $plan"
printf '%s' "$plan" | grep -qF 'offsite-only verdict OK  (not available yet: offsite destination not set' \
  || fail "plan must say offsite is not available yet: $plan"
printf '%s' "$plan" | grep -qF 'first upload 24.0 GiB–40.0 GiB (~115 min at 100.0 Mbit/s)' || fail "plan offsite line: $plan"
printf '%s' "$plan" | grep -qF 'unmeasured — conservative bound' || fail "plan churn quality: $plan"
printf '%s' "$plan" | grep -qF -- '--confirm-plan a1b2c3d4e5f6' || fail "plan must print the next command: $plan"
k2 hostmail backup plan --json | python3 -c 'import json,sys; assert json.load(sys.stdin)["planHash"]'

out="$(k2 hostmail backup set --mode local --keep-daily 3 --window 02:00-03:00 --max-local-gb 80 --confirm-plan a1b2c3d4e5f6)"
printf '%s' "$out" | grep -qF 'backup mode: unconfigured → local' || fail "set output: $out"
printf '%s' "$out" | grep -qF 'later release' || fail "set must say nothing runs yet: $out"
k2 hostmail backup set --mode none --reason "provider snapshots daily" --confirm-plan a1b2c3d4e5f6 --json >/dev/null

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/backup", None),
    ("GET", "/cli/mail/backup/plan?bandwidthMbps=100&keepDaily=3", None),
    ("GET", "/cli/mail/backup/plan", None),
    ("POST", "/cli/mail/backup/set", {"mode": "local", "confirmPlan": "a1b2c3d4e5f6", "keepDaily": 3,
                                      "window": "02:00-03:00", "maxLocalGb": 80.0}),
    ("POST", "/cli/mail/backup/set", {"mode": "none", "confirmPlan": "a1b2c3d4e5f6",
                                      "reason": "provider snapshots daily"}),
]
if len(rows) != len(want):
    sys.exit("FAIL: expected %d requests, got %r" % (len(want), rows))
for (m, p, b), (wm, wp, wb) in zip(rows, want):
    if (m, p) != (wm, wp):
        sys.exit("FAIL: expected %s %s, got %s %s" % (wm, wp, m, p))
    if wb is not None and json.loads(b) != wb:
        sys.exit("FAIL: %s %s body must be %r, got %s" % (m, p, wb, b))
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: hostmail backup CLI wiring"
