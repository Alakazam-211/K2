#!/usr/bin/env bash
# `k2 sim` (prd-mobile-dev-v1 §4, S2): the REAL cli/k2 against a stub daemon
# that records each request and answers in Builder A's frozen shapes
# (k2-core::sim::api: StatusResp, SimView, DevicesResp, TreeResp, ActReq with
# ActSpec tagged "verb", ActResp, LogsResp, WaitResp; errors {error, code,
# message} with sim_switch_off / sim_held / sim_stale_element).
#
#   1. tree: GET /cli/sim/tree with workspace + all/find/boxes; prints the
#      daemon's text as is.
#   2. tap e5 / tap --at x,y / type --into e3 "…" / type --secret NAME /
#      swipe / button / rotate / location / open-url → POST /cli/sim/act
#      bodies; each prints the daemon's one line.
#   3. boot / stop / install (absolute path) / launch / screenshot / logs /
#      wait / status / list-devices: routes, bodies and output.
#   4. exits: held (409 sim_held) → 2 with the wait hint; a refusal with a
#      permission card → 3 and the "Ask with:" line; stale ids → 3; daemon
#      down → 1; --json prints the body.
#   5. usage errors exit 2 and never reach the daemon (bad ids, both an id
#      and --at, text and --secret, bad durations, erase without --yes).
#   6. discoverable: --schema lists the family; k2 study sim; help offline.
#
# No daemon build; never touches ~/.k2 (env carries port + token).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

WORK="$(mktemp -d -t k2-sim-cli-XXXXXX)"
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

def mode():
    p = os.path.join(work, "mode")
    return open(p).read().strip() if os.path.exists(p) else "ok"

PROFILE = {"id": "pixel_9", "name": "Pixel 9", "widthPt": 412, "heightPt": 915, "scale": 2.625,
           "os": "android-35 google_apis x86_64", "default": True}
SIM = {"sim": "s1", "workspace": "p-docs", "workspaceHandle": "docs", "lane": "android",
       "state": "running", "profile": PROFILE, "appId": "com.acme.app", "lastUsedAt": 1,
       "hold": {"kind": "person", "by": "rosson", "ms": 2000}, "viewers": 1}
TREE = ("sim s1 · android · Pixel 9 · com.acme.app · snapshot t7 · 412x915 dp\n"
        "e1  toolbar \"Settings\"\ne5  button \"Save\"\n(2 of 48 elements; --all for the rest)\n")

class H(BaseHTTPRequestHandler):
    def reply(self, obj, code=200):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, path, body):
        n["i"] += 1
        with open(os.path.join(work, "req-%d.json" % n["i"]), "w") as f:
            json.dump({"path": path, "body": body}, f)

    def refusal(self):
        m = mode()
        if m == "held":
            return self.reply({"error": "held", "code": "sim_held",
                               "message": "held: a person is using the device. Try again in a moment (`k2 sim wait`)."}, 409)
        if m == "off":
            return self.reply({"error": "forbidden", "code": "sim_switch_off",
                               "message": "Mobile devices are off for this agent (Settings > Agent > Allow mobile devices)",
                               "permission": {"command": "k2 permission request sim.use --why \"<what you need it for>\""}}, 403)
        if m == "stale":
            return self.reply({"error": "stale", "code": "sim_stale_element",
                               "message": "e9 is not on screen in snapshot t8. Run `k2 sim tree` and use an id from it"}, 409)
        return None

    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items() if k != "token"}
        self.record(u.path, q)
        if self.refusal() is not None:
            return
        if u.path == "/cli/sim/tree":
            return self.reply({"snapshot": {"id": "t7"}, "text": TREE})
        if u.path == "/cli/sim/status":
            return self.reply({"lanes": [{"lane": "android", "usable": True, "ready": False,
                                          "needs": [{"kind": "licence", "id": "android-sdk-license", "title": "Android SDK licence"}]},
                                         {"lane": "ios", "usable": False, "ready": False, "detail": "not a Mac"}],
                               "sim": SIM, "running": 1, "limits": {}, "agentsAllowed": True})
        if u.path == "/cli/sim/devices":
            return self.reply({"lane": "android", "devices": [PROFILE]})
        if u.path == "/cli/sim/logs":
            return self.reply({"lines": ["I/acme: ready", "I/acme: saved"], "truncated": True})
        if u.path == "/cli/workspace/resolve":
            return self.reply({"path": "/ws/" + q.get("q", "")})
        self.reply({"error": "stub: unknown GET " + u.path}, 404)

    def do_POST(self):
        u = urlparse(self.path)
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        self.record(u.path, body)
        if self.refusal() is not None:
            return
        if u.path == "/cli/sim/act":
            a = body.get("action") or {}
            return self.reply({"ok": True, "snapshot": "t8", "tookMs": 140, "diff": ["+e9 text \"Saved\""],
                               "line": "ok %s %s · 140ms · +e9 text \"Saved\"" % (a.get("verb"), a.get("element", ""))})
        if u.path == "/cli/sim/boot":
            return self.reply(dict(SIM, state="booting"))
        if u.path == "/cli/sim/stop":
            return self.reply(dict(SIM, state="stopping"))
        if u.path == "/cli/sim/erase":
            return self.reply({"ok": True})
        if u.path in ("/cli/sim/launch", "/cli/sim/terminate"):
            return self.reply({"ok": True, "line": "ok %s %s · 300ms" % (u.path.rsplit("/", 1)[1], body.get("app")), "tookMs": 300})
        if u.path == "/cli/sim/install":
            return self.reply({"appId": "com.acme.app", "replaced": False, "line": "ok install com.acme.app · 2.1s"})
        if u.path == "/cli/sim/screenshot":
            return self.reply({"path": "/ws/docs/.k2/sim/shots/s1-0003.png", "width": 412, "height": 915})
        if u.path == "/cli/sim/wait":
            return self.reply({"ok": True, "line": "ok wait · 1.2s · \"Saved\" is on screen", "tookMs": 1200})
        self.reply({"error": "stub: unknown POST " + u.path}, 404)

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
export K2_PROJECT_PATH="/ws/docs"

PASS=0
FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
last_req() { python3 -c 'import glob,os,re,sys; fs=glob.glob(os.path.join(sys.argv[1],"req-*.json")); print(max(fs, key=lambda f: int(re.search(r"req-(\d+)\.json$", f).group(1))) if fs else "")' "$WORK"; }
req_count() { ls -1 "$WORK"/req-*.json 2>/dev/null | wc -l | tr -d ' '; }
req() { python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["path"], json.dumps(r["body"], sort_keys=True))' "$(last_req)"; }
k2() { "$K2_CLI" "$@"; }

# 1. tree.
out="$(k2 sim tree --all --find save --boxes 2>&1)" || bad "1: tree exit non-zero: $out"
[ "$(req)" = '/cli/sim/tree {"all": "1", "boxes": "1", "find": "save", "workspace": "/ws/docs"}' ] && ok "1: tree query" || bad "1: tree query: $(req)"
[ "$out" = "$(printf 'sim s1 · android · Pixel 9 · com.acme.app · snapshot t7 · 412x915 dp\ne1  toolbar "Settings"\ne5  button "Save"\n(2 of 48 elements; --all for the rest)')" ] \
    && ok "1: tree prints the daemon's text" || bad "1: tree text: $out"
k2 sim tree >/dev/null 2>&1
[ "$(req)" = '/cli/sim/tree {"workspace": "/ws/docs"}' ] && ok "1: plain tree sends only the workspace" || bad "1: plain tree: $(req)"

# 2. act verbs.
check_act() {
    local label="$1" want="$2"
    shift 2
    local o
    o="$(k2 sim "$@" 2>&1)" || { bad "2: $label exit non-zero: $o"; return; }
    [ "$(req)" = "/cli/sim/act $want" ] && ok "2: $label body" || bad "2: $label body: $(req)"
    grep -qF '· 140ms · +e9 text "Saved"' <<<"$o" && ok "2: $label prints the line" || bad "2: $label line: $o"
}
check_act "tap e5" '{"action": {"element": "e5", "verb": "tap"}, "workspace": "/ws/docs"}' tap e5
check_act "tap t7:e5" '{"action": {"element": "t7:e5", "verb": "tap"}, "workspace": "/ws/docs"}' tap t7:e5
check_act "tap --at" '{"action": {"verb": "tap", "x": 120, "y": 300.5}, "workspace": "/ws/docs"}' tap --at 120,300.5
check_act "type --into" '{"action": {"into": "e3", "text": "a \"quoted\" $HOME", "verb": "type"}, "workspace": "/ws/docs"}' type --into e3 'a "quoted" $HOME'
check_act "type --secret" '{"action": {"into": "e3", "secret": "gh-pass", "verb": "type"}, "workspace": "/ws/docs"}' type --into e3 --secret gh-pass
check_act "swipe" '{"action": {"direction": "up", "from": "e7", "verb": "swipe"}, "workspace": "/ws/docs"}' swipe up --from e7
check_act "button" '{"action": {"button": "home", "verb": "button"}, "workspace": "/ws/docs"}' button home
check_act "button dev-menu" '{"action": {"button": "dev-menu", "verb": "button"}, "workspace": "/ws/docs"}' button dev-menu
check_act "key" '{"action": {"key": "enter", "verb": "key"}, "workspace": "/ws/docs"}' key enter
check_act "rotate" '{"action": {"turn": "left", "verb": "rotate"}, "workspace": "/ws/docs"}' rotate left
check_act "appearance" '{"action": {"appearance": "dark", "verb": "appearance"}, "workspace": "/ws/docs"}' appearance dark
check_act "location" '{"action": {"lat": 37.33, "lon": -122.03, "verb": "location"}, "workspace": "/ws/docs"}' location 37.33,-122.03
check_act "open-url" '{"action": {"url": "myapp://settings", "verb": "open-url"}, "workspace": "/ws/docs"}' open-url myapp://settings
check_act "--workspace name" '{"action": {"element": "e5", "verb": "tap"}, "workspace": "/ws/other"}' tap e5 --workspace other

# 3. the device and its files.
out="$(k2 sim boot --platform android --device "Pixel 9" --no-tab 2>&1)" || bad "3: boot exit: $out"
[ "$(req)" = '/cli/sim/boot {"device": "Pixel 9", "noTab": true, "platform": "android", "workspace": "/ws/docs"}' ] && ok "3: boot body" || bad "3: boot body: $(req)"
grep -qF 'Booting: sim s1 · android · Pixel 9 (booting). Next: k2 sim wait, then k2 sim tree' <<<"$out" && ok "3: boot output" || bad "3: boot output: $out"
k2 sim boot >/dev/null 2>&1
[ "$(req)" = '/cli/sim/boot {"workspace": "/ws/docs"}' ] && ok "3: bare boot" || bad "3: bare boot: $(req)"
out="$(k2 sim stop 2>&1)" || bad "3: stop exit: $out"
[ "$(req)" = '/cli/sim/stop {"workspace": "/ws/docs"}' ] && grep -qF 'Stopped. Its data is kept' <<<"$out" && ok "3: stop" || bad "3: stop: $(req) $out"
k2 sim erase --yes >/dev/null 2>&1 || bad "3: erase --yes exit"
[ "$(req)" = '/cli/sim/erase {"workspace": "/ws/docs"}' ] && ok "3: erase --yes" || bad "3: erase: $(req)"
mkdir -p "$WORK/build"
printf 'apk' > "$WORK/build/app-debug.apk"
out="$(cd "$WORK/build" && k2 sim install app-debug.apk 2>&1)" || bad "3: install exit: $out"
want_path="$(cd "$WORK/build" && pwd)/app-debug.apk"
[ "$(req)" = "/cli/sim/install {\"path\": \"$want_path\", \"workspace\": \"/ws/docs\"}" ] && ok "3: install sends an absolute path" || bad "3: install: $(req)"
grep -qF 'ok install com.acme.app' <<<"$out" && ok "3: install line" || bad "3: install out: $out"
out="$(k2 sim launch com.acme.app 2>&1)"
[ "$out" = "ok launch com.acme.app · 300ms" ] && ok "3: launch prints the daemon's line" || bad "3: launch out: $out"
[ "$(req)" = '/cli/sim/launch {"app": "com.acme.app", "workspace": "/ws/docs"}' ] && ok "3: launch" || bad "3: launch: $(req)"
k2 sim terminate com.acme.app >/dev/null 2>&1
[ "$(req)" = '/cli/sim/terminate {"app": "com.acme.app", "workspace": "/ws/docs"}' ] && ok "3: terminate" || bad "3: terminate: $(req)"
out="$(k2 sim screenshot --region e7 2>&1)"
[ "$(req)" = '/cli/sim/screenshot {"region": "e7", "workspace": "/ws/docs"}' ] && [ "$out" = "/ws/docs/.k2/sim/shots/s1-0003.png" ] && ok "3: screenshot prints the path" || bad "3: screenshot: $(req) $out"
out="$(k2 sim logs --app com.acme.app --since 2m --grep acme --lines 50 2>"$WORK/err")"
[ "$(req)" = '/cli/sim/logs {"app": "com.acme.app", "grep": "acme", "maxLines": "50", "sinceSecs": "120", "workspace": "/ws/docs"}' ] && ok "3: logs query" || bad "3: logs: $(req)"
[ "$out" = "$(printf 'I/acme: ready\nI/acme: saved')" ] && grep -qF 'cut at the line/byte cap' "$WORK/err" && ok "3: logs lines + truncated note" || bad "3: logs out: $out / $(cat "$WORK/err")"
out="$(k2 sim wait --for Saved --timeout 30s 2>&1)"
[ "$(req)" = '/cli/sim/wait {"for": "Saved", "timeoutMs": 30000, "workspace": "/ws/docs"}' ] && grep -qF 'ok wait' <<<"$out" && ok "3: wait" || bad "3: wait: $(req) $out"
out="$(k2 sim status 2>&1)"
grep -qF 'lane android   needs a step' <<<"$out" \
    && grep -qF '  the owner must accept the Android SDK licence [android-sdk-license] (a card; never you)' <<<"$out" \
    && grep -qF 'lane ios       not available (not a Mac)' <<<"$out" \
    && grep -qF 'device: sim s1 · android · Pixel 9 · running · 412x915 pt · com.acme.app' <<<"$out" \
    && grep -qF 'held: rosson is using the device' <<<"$out" \
    && grep -qF 'agents: allowed (Allow mobile devices is on)' <<<"$out" && ok "3: status" || bad "3: status: $out"
out="$(k2 sim list-devices --platform android 2>&1)"
[ "$(req)" = '/cli/sim/devices {"platform": "android", "workspace": "/ws/docs"}' ] && ok "3: list-devices query" || bad "3: list-devices: $(req)"
grep -qE '^pixel_9 +Pixel 9 +412x915 pt  android-35 google_apis x86_64  \(default\)$' <<<"$out" && ok "3: list-devices" || bad "3: list-devices: $out"

# 4. exits.
echo held > "$WORK/mode"
set +e
err="$(k2 sim tap e5 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" -eq 2 ] && grep -qF 'Error: held: a person is using the device. Try again in a moment' <<<"$err" && grep -qF 'Run `k2 sim wait`' <<<"$err" \
    && ok "4: held exits 2 with the wait hint" || bad "4: held rc=$rc err=$err"
echo off > "$WORK/mode"
set +e
err="$(k2 sim boot 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" -eq 3 ] && grep -qF 'Allow mobile devices' <<<"$err" && grep -qF 'Ask with: k2 permission request sim.use' <<<"$err" \
    && ok "4: switch off exits 3 with Ask with" || bad "4: off rc=$rc err=$err"
echo stale > "$WORK/mode"
set +e
err="$(k2 sim tap e9 --json 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" -eq 3 ] && grep -qF '"code": "sim_stale_element"' <<<"$err" && ! grep -qF 'Error:' <<<"$err" \
    && ok "4: stale id exits 3; --json errors are JSON only" || bad "4: stale rc=$rc err=$err"
echo ok > "$WORK/mode"
out="$(k2 sim tree --json 2>&1)"
python3 -c 'import json,sys; assert json.loads(sys.argv[1])["snapshot"]["id"] == "t7"' "$out" && ok "4: --json prints the body" || bad "4: --json: $out"
set +e
K2_PORT=1 k2 sim tree >/dev/null 2>&1; rc=$?
set -e
[ "$rc" -eq 1 ] && ok "4: daemon down exits 1" || bad "4: daemon down rc=$rc"

# 5. usage errors exit 2 offline.
before="$(req_count)"
fails=""
for args in "tap" "tap save" "tap e5 --at 1,2" "tap --at 1x2" "type" "type hi --secret x" "type --into foo hi" \
    "swipe sideways" "button power" "key space" "rotate up" "location 1" "open-url nope" "logs --since soon" \
    "logs --lines 0" "wait --timeout 5m" "wait --timeout x" "boot --platform windows" "install" \
    "install /no/such/app.apk" "erase" "status extra" "list-devices --platform windows" "frobnicate"; do
    set +e
    # shellcheck disable=SC2086
    k2 sim $args </dev/null >/dev/null 2>&1; rc=$?
    set -e
    [ "$rc" -eq 2 ] || fails="$fails [$args → $rc]"
done
[ -z "$fails" ] && [ "$(req_count)" = "$before" ] && ok "5: usage errors exit 2 and never reach the daemon" \
    || bad "5: $fails; reqs $before→$(req_count)"

# 6. discoverable, and help needs no daemon.
schema="$(k2 --schema 2>/dev/null)"
for name in '"name": "sim"' '"name": "sim tree"' '"name": "sim tap"' '"name": "sim install"' '"name": "sim screenshot"'; do
    grep -qF "$name" <<<"$schema" || bad "6: schema lacks $name"
done
ok "6: --schema lists the family"
grep -qF 'k2 study sim — the mobile device' <<<"$(k2 study sim 2>/dev/null)" && ok "6: k2 study sim" || bad "6: study page"
grep -qF '  sim                 The mobile device' <<<"$(k2 study 2>/dev/null)" && ok "6: study catalog" || bad "6: study catalog"
help="$(K2_PORT=1 k2 sim --help 2>&1)" && grep -qF 'k2 sim — drive this workspace' <<<"$help" && ok "6: help offline" || bad "6: help: $help"
grep -qF 'k2 sim — drive' <<<"$(K2_PORT=1 k2 help sim 2>&1)" && ok "6: k2 help sim" || bad "6: k2 help sim"

echo
echo "sim_cli: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
