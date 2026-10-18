#!/usr/bin/env bash
# `k2 sources` 0.46.1 (Garden and data-sources polish, S1 + S2): what the
# REAL cli/k2 sends and prints, against a stub daemon that records every
# request.
#   1. propose: --expect-status, --list/--list-key/--list-field, --count,
#      --agg count:<list>, and --rule with @field references, ageGt
#      durations, versionBehind with a distance, warnAfter/critAfter.
#   2. try sends every flag through the same parser, plus --targets N
#      (tryTargets); `try --name X --targets 3` tries the saved source.
#   3. --targets outside try is a usage error (exit 2, no request).
#   4. history health [--item] sends field/item and prints the changes.
#   5. pending with "scope": "own" says these are your proposals.
#   6. read prints "soft crit 2/3" for a waiting reading (top and items).
#   7. per-verb help: propose/history/export/pending; the noun help names
#      failAfter and the rule operators. No daemon needed for help.
#
# No daemon build; never touches ~/.k2 (env carries port + token).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-sources-rules-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

work = sys.argv[1]
n = {"i": 0}

def record(method, path, body, query):
    n["i"] += 1
    with open(os.path.join(work, "req-%03d.json" % n["i"]), "w") as f:
        json.dump({"method": method, "path": path, "body": body, "query": query}, f)

SOFT_VIEW = {
    "name": "ops", "label": "Ops", "kind": "http-json", "status": "active", "health": "warn", "phase": "live",
    "display": "warn", "reason": None, "reasonText": None, "httpStatus": 200,
    "soft": {"count": 2, "of": 3, "level": "crit"}, "serverNow": 1000, "updatedAt": 990, "checkedAt": 990,
    "nextAt": None, "hardSince": 900, "recoveredAt": None, "gap": None, "muted": None, "simulated": None,
    "pinned": False, "keep": "values", "masked": False, "link": None, "fields": {},
    "items": [{"key": "ohana", "health": "warn", "reason": None, "since": 900,
               "soft": {"count": 2, "of": 3, "level": "crit"}, "fields": {"version": "0.44.4"}},
              {"key": "certs/k2.dev", "list": "certs", "health": "ok", "reason": None, "since": None,
               "soft": {"count": 0, "of": 3, "level": None}, "fields": {"cert_days": 60}}],
    "counts": {"ok": 1, "warn": 1, "crit": 0, "unknown": 0}, "rev": 4,
}

class H(BaseHTTPRequestHandler):
    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items() if k != "token"}
        record("GET", u.path, None, q)
        if u.path == "/cli/sources/history":
            if q.get("field") == "health":
                return self.reply({"field": "health", "item": q.get("item"), "events": [
                    {"t": 100, "kind": "went_warn", "from": "ok", "to": "warn", "reason": None, "item": q.get("item"), "incident": "i1"},
                    {"t": 160, "kind": "went_crit", "from": "warn", "to": "crit", "reason": None, "item": q.get("item"), "incident": "i1"}]})
            return self.reply({"field": q.get("field"), "res": "raw", "points": [{"t": 100, "v": 2}]})
        if u.path == "/cli/sources/pending":
            return self.reply({"sources": [{"name": "mine", "approval": "one_click", "everySecs": 60, "targets": 1,
                                            "origins": ["https://k2.dev:443"], "madeBy": "agent:p1", "maker": {"handle": "k2"}}],
                               "triggers": [], "scope": "own"})
        if u.path == "/cli/sources/read":
            return self.reply(SOFT_VIEW)
        self.reply({"error": {"code": "stub", "hint": "unknown GET " + u.path}}, 404)

    def do_POST(self):
        u = urlparse(self.path)
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        record("POST", u.path, body, {})
        if u.path == "/cli/sources/propose":
            return self.reply({"name": body.get("name"), "status": "proposed", "approval": "one_click", "warnings": []})
        if u.path == "/cli/sources/try":
            return self.reply({"ok": True, "status": 200, "elapsedMs": 3, "tree": [], "targets": [], "warnings": [],
                               "view": {"display": "ok", "counts": {"ok": 1, "warn": 0, "crit": 0, "unknown": 0}}})
        self.reply({"error": {"code": "stub", "hint": "unknown POST " + u.path}}, 404)

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"
for _ in $(seq 1 50); do [ -f "$WORK/stub.port" ] && break; sleep 0.1; done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
last_req() { ls -1 "$WORK"/req-*.json 2>/dev/null | sort | tail -1; }
req_count() { ls -1 "$WORK"/req-*.json 2>/dev/null | wc -l | tr -d ' '; }
k2() { "$K2_CLI" "$@"; }
check() { # label, python expression over r (the last request) → must print True
    local got
    got="$(python3 -c "
import json, sys
r = json.load(open(sys.argv[1]))
b = r['body']
print($2)
" "$(last_req)")"
    [ "$got" = "True" ] && ok "$1" || bad "$1 (got $got; request $(cat "$(last_req)"))"
}

# 1. propose with every 0.46.1 flag.
out="$(k2 sources propose --name ops --label Ops --url 'https://k2.dev/api/ops/fleet?part=boxes' \
    --expect-status 200,401 \
    --field target_version=/target_version --field ts=/ts --count past_due_n=/past_due \
    --items /boxes --item-key /name --item-field version=/version \
    --list certs=/certs --list-key certs=/host --list-field certs.cert_days=/days_left \
    --agg certs_n=count:certs --agg behind=count-where:version:versionBehind:@target_version \
    --rule 'ts:warn:ageGt:5m,crit:ageGt:15m,ok:exists:true' \
    --rule 'version:warn:versionBehind:@target_version,crit:versionBehind:@target_version~minor=1,ok:versionAtLeast:@target_version,critAfter:3' \
    --rule 'cert_days:crit:lt:7,warn:lt:@warn_days,warnAfter:2' 2>&1)" || bad "1: exit non-zero: $out"
check "1: path" "r['path'] == '/cli/sources/propose'"
check "1: expect" "b['expect'] == {'status': [200, 401]}"
check "1: count" "b['extract']['counts'] == {'past_due_n': '/past_due'}"
check "1: list" "b['extract']['lists'] == {'certs': {'pointer': '/certs', 'itemKey': '/host', 'itemFields': {'cert_days': '/days_left'}}}"
check "1: count over a list" "b['extract']['aggregates']['certs_n'] == {'op': 'count', 'list': 'certs'}"
check "1: countWhere versionBehind @field" "b['extract']['aggregates']['behind']['cond'] == {'versionBehind': {'field': 'target_version'}}"
check "1: ageGt durations" "b['rules'][0] == {'field': 'ts', 'warn': {'ageGt': 300}, 'crit': {'ageGt': 900}, 'ok': {'exists': True}}"
check "1: versionBehind distance + critAfter" "b['rules'][1] == {'field': 'version', 'warn': {'versionBehind': {'field': 'target_version'}}, 'crit': {'versionBehind': {'field': 'target_version', 'minor': 1}}, 'ok': {'versionAtLeast': {'field': 'target_version'}}, 'critAfter': 3}"
check "1: lt @field + warnAfter" "b['rules'][2] == {'field': 'cert_days', 'crit': {'lt': 7}, 'warn': {'lt': {'field': 'warn_days'}}, 'warnAfter': 2}"
grep -qF 'ops: proposed' <<<"$out" && ok "1: prints proposed" || bad "1: output: $out"

# 2. try: the same parser, --targets N.
k2 sources try --url https://k2.dev/x --expect-status 401 --rule 'v:warn:versionBehind:0.45.7' --targets 5 >/dev/null 2>&1 || bad "2: try exit non-zero"
check "2: try path" "r['path'] == '/cli/sources/try'"
check "2: try tryTargets" "b['tryTargets'] == 5"
check "2: try keeps expect and the literal version" "b['expect'] == {'status': [401]} and b['rules'][0]['warn'] == {'versionBehind': '0.45.7'}"
k2 sources try --name fleet --targets 3 >/dev/null 2>&1 || bad "2: try saved exit non-zero"
check "2: try a saved source with --targets" "b == {'name': 'fleet', 'tryTargets': 3}"

# 3. --targets is try's only.
before="$(req_count)"
set +e
k2 sources propose --name x --label X --url https://a.example/ --targets 3 >"$WORK/o" 2>&1
rc=$?
set -e
[ "$rc" = "2" ] && ok "3: --targets on propose exits 2" || bad "3: exit $rc: $(cat "$WORK/o")"
[ "$(req_count)" = "$before" ] && ok "3: no request sent" || bad "3: a request was sent"
set +e
k2 sources propose --name x --label X --url https://a.example/ --rule 'v:warn:versionBehind:@t~nope=1' >"$WORK/o" 2>&1
rc=$?
set -e
[ "$rc" = "2" ] && ok "3: a bad version distance exits 2" || bad "3: exit $rc: $(cat "$WORK/o")"

# 4. history health.
out="$(k2 sources history ops health --item ohana 2>&1)" || bad "4: exit non-zero: $out"
check "4: history query" "r['path'] == '/cli/sources/history' and r['query']['field'] == 'health' and r['query']['item'] == 'ohana'"
grep -qF 'went_crit ohana' <<<"$out" && grep -qF 'warn → crit' <<<"$out" && ok "4: prints the changes" || bad "4: output: $out"

# 5. pending scope own.
out="$(k2 sources pending 2>&1)" || bad "5: exit non-zero: $out"
grep -qF 'Your proposals waiting' <<<"$out" && grep -qF 'A person approves them' <<<"$out" && ok "5: own proposals" || bad "5: output: $out"

# 6. read: soft crit 2/3.
out="$(k2 sources read ops 2>&1)" || bad "6: exit non-zero: $out"
[ "$(grep -cF 'soft crit 2/3' <<<"$out")" = "2" ] && ok "6: soft crit 2/3 on the source and the item" || bad "6: output: $out"

# 7. help, no daemon needed.
export K2_PORT=1
out="$(k2 sources propose --help)"
for want in '--expect-status 200,401' 'critAfter:N' '--fail-after N' 'versionBehind' 'ageGt ageLt' '--list-field list.field=/pointer' '--count name=/pointer'; do
    grep -qF -- "$want" <<<"$out" && ok "7: propose --help has $want" || bad "7: propose --help lacks $want"
done
out="$(k2 sources try --help)"
grep -qF -- '--targets N' <<<"$out" && ok "7: try --help has --targets" || bad "7: try --help: $out"
out="$(k2 sources history --help)"
grep -qF 'health' <<<"$out" && grep -qF -- '--item' <<<"$out" && ok "7: history --help" || bad "7: history --help: $out"
out="$(k2 sources export --help)"
grep -qF 'propose --from' <<<"$out" && ok "7: export --help" || bad "7: export --help: $out"
out="$(k2 sources pending --help)"
grep -qF 'only its own' <<<"$out" && ok "7: pending --help" || bad "7: pending --help: $out"
out="$(k2 sources --help)"
grep -qF 'failAfter' <<<"$out" && grep -qF 'versionBehind' <<<"$out" && ok "7: noun help names failAfter and the operators" || bad "7: noun help: $out"
grep -qF 'opts the source in' <<<"$out" && grep -qF 'behaves as before 0.46.1' <<<"$out" && ok "7: noun help says thresholds are opt-in" || bad "7: noun help lacks the opt-in"
out="$(k2 sources propose --help)"
grep -qF 'Opt-in: setting warnAfter or critAfter on ANY rule' <<<"$out" && ok "7: propose --help explains the opt-in" || bad "7: propose --help lacks the opt-in"
out="$(k2 study data-sources)"
grep -qF 'RULES (0.46.1)' <<<"$out" && grep -qF 'k2 sources history <name> health' <<<"$out" && ok "7: study page teaches rules and history" || bad "7: study page"

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
