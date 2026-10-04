#!/usr/bin/env bash
# k2 zen (prd-zen-mode-v1 Z17, T3.1) against a REAL headless daemon.
#
# Boots the worktree's k2-daemon under a temp HOME (never the real ~/.k2,
# never a production daemon), then:
#   - before Zen is set up: validate/path/pages exit 3 with the sentence;
#   - zen.toml with `acent` at line 7 → `zen.toml:7:3: unknown key 'acent'`, exit 1;
#   - clean → exit 0; history lists snapshots; reset --to restores one;
#   - --home resolves a Home by name; doctor reports the watcher;
#   - there is no grant verb, and nothing writes grants.json;
#   - themes: list/next/prev/set/new, per-Home picks, an unknown name exits 1,
#     reset --theme clears an override, and a curl-only switch (no CLI, no
#     client) shows up in /cli/zen/get.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
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

SANDBOX="$(mktemp -d -t k2-zen-cli-XXXXXX)"
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
HOME="$SANDBOX" K2_TEST_AGENT_SHIM_DIR="$SANDBOX/shim" K2SO_WATCHDOG_DISABLED=1 \
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
for _ in $(seq 1 300); do
    phase="$(curl -s "http://127.0.0.1:$PORT/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || { echo "FAIL: daemon never reached ready" >&2; exit 1; }

ZEN="$SANDBOX/.k2/zen"

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_PROJECT_PATH \
        HOME="$SANDBOX" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" \
        "$K2_CLI" "$@"
}

capture() {
    set +e
    out="$(run_k2 "$@" 2>&1)"
    rc=$?
    set -e
}

echo "== help =="
capture zen --help
assert_eq "zen --help exit" "$rc" "0"
assert_contains "help names validate" "$out" "k2 zen validate"
assert_contains "help states the grant rule" "$out" "Never write grants.json"
capture help zen
assert_eq "k2 help zen exit" "$rc" "0"
assert_contains "k2 help zen" "$out" "k2 zen reset"

echo "== not set up =="
capture zen theme list
assert_eq "zen theme list before setup exits 3" "$rc" "3"
for verb in validate path pages history reload; do
    capture zen "$verb"
    assert_eq "zen $verb before setup exits 3" "$rc" "3"
    assert_contains "zen $verb before setup says so" "$out" "Zen isn't set up on this computer. Turn it on from Home in the K2 app."
done
[ ! -e "$ZEN" ] && ok "no CLI verb creates ~/.k2/zen" || bad "a CLI verb created ~/.k2/zen"

echo "== set up (as the app does) =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/page/ensure?token=$TOKEN" \
    -H 'Content-Type: application/json' --data-raw '{"homeId":"home-1","name":"Work"}')"
assert_contains "page/ensure created the folder" "$resp" '"createdFolder":true'
capture zen path
assert_eq "zen path exit" "$rc" "0"
assert_eq "zen path prints the folder" "$out" "$ZEN"
capture zen pages
assert_eq "zen pages exit" "$rc" "0"
assert_contains "zen pages lists the Home" "$out" "home-1  Work  page"
capture zen validate
assert_eq "fresh setup validates" "$rc" "0"
assert_contains "validate ok line" "$out" "ok: zen.toml, pages/home-1.toml"

echo "== T3.1 error with file:line:col =="
printf 'schema = 1\n[theme]\nscheme = "auto"\n\n[colors.light]\ncanvas = "#faf7f2"\n  acent = "#fff"\n' >"$ZEN/zen.toml"
capture zen validate
assert_eq "broken validate exits 1" "$rc" "1"
assert_contains "error line" "$out" "zen.toml:7:3: unknown key 'acent'"
assert_contains "last good note" "$out" "Zen is showing the last good version."
capture zen validate --json
assert_eq "broken validate --json exits 1" "$rc" "1"
line="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["errors"][0]["line"])')"
assert_eq "--json carries the line" "$line" "7"
capture zen reload
assert_eq "reload with errors exits 1" "$rc" "1"
assert_contains "reload prints the error" "$out" "zen.toml:7:3:"

printf 'schema = 1\n[shape]\nradius = 9\n' >"$ZEN/zen.toml"
capture zen validate
assert_eq "clean validate exits 0" "$rc" "0"
capture zen reload
assert_eq "clean reload exits 0" "$rc" "0"

echo "== page by Home name =="
printf 'schema = 1\ntemplate = "k2.texting@1"\n[font]\nsize = 40\n' >"$ZEN/pages/home-1.toml"
capture zen validate --home Work
assert_eq "--home Work validate exits 1" "$rc" "1"
assert_contains "page error names the page file" "$out" "pages/home-1.toml:4:8: font.size = 40 is out of range"
capture zen validate --home Nowhere
assert_eq "unknown Home exits 1" "$rc" "1"
assert_contains "unknown Home message" "$out" "no Home 'Nowhere'"
capture zen reset --home Work
assert_eq "reset --home Work exits 0" "$rc" "0"
assert_contains "reset page to its stub" "$out" "reset pages/home-1.toml to default"
capture zen validate
assert_eq "after page reset everything validates" "$rc" "0"

echo "== history and reset --to =="
capture zen history --json
assert_eq "history --json exit" "$rc" "0"
snap="$(printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
z = [f for f in d["files"] if f["file"] == "zen.toml"][0]
clean = [s for s in z["snapshots"] if s["clean"]]
print(clean[-1]["name"])')"
[ -n "$snap" ] && ok "history lists a clean zen.toml snapshot ($snap)" || bad "no zen.toml snapshot"
capture zen reset --to "$snap"
assert_eq "reset --to exits 0" "$rc" "0"
assert_contains "reset names the snapshot" "$out" "reset zen.toml to $snap"
assert_contains "reset keeps the previous file" "$out" "previous file kept as snapshot"
want="$(cat "$ZEN/.history/zen.toml/$snap")"
assert_eq "zen.toml is the snapshot" "$(cat "$ZEN/zen.toml")" "$want"
capture zen reset --to 19990101T000000000Z-000
assert_eq "unknown snapshot exits 1" "$rc" "1"
assert_contains "unknown snapshot message" "$out" "no snapshot"

echo "== themes =="
capture zen theme list
assert_eq "theme list exit" "$rc" "0"
assert_contains "list marks the active default" "$out" "* default  (built in)"
assert_contains "list shows paper" "$out" "  paper  (built in)"
assert_contains "list shows midnight" "$out" "  midnight  (built in)"
capture zen theme next
assert_eq "theme next exit" "$rc" "0"
assert_eq "next goes to paper" "$out" "theme paper for this computer"
capture zen theme next
assert_eq "next goes to midnight" "$out" "theme midnight for this computer"
capture zen theme next
assert_eq "next wraps to default" "$out" "theme default for this computer"
capture zen theme prev
assert_eq "prev wraps to midnight" "$out" "theme midnight for this computer"
capture zen theme set neon
assert_eq "unknown theme exits 1" "$rc" "1"
assert_contains "unknown theme says so" "$out" "no theme 'neon' on this computer; themes: default, paper, midnight"
capture zen theme list --json
active="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["active"])')"
assert_eq "an unknown name changes nothing" "$active" "midnight"
capture zen theme set
assert_eq "set with no name exits 2" "$rc" "2"
capture zen theme set paper --home Work
assert_eq "set --home exit" "$rc" "0"
assert_eq "set --home says so" "$out" "theme paper for Home home-1"
capture zen theme list --home Work
assert_contains "list --home marks paper" "$out" "* paper"
assert_contains "list --home notes the pick" "$out" "Home home-1: its own pick"
capture zen theme set --home Work --clear
assert_eq "clear exit" "$rc" "0"
assert_eq "clear says so" "$out" "Home home-1 follows this computer: theme midnight"

capture zen theme new sunset
assert_eq "theme new exit" "$rc" "0"
assert_contains "theme new names the file" "$out" "made $ZEN/themes/sunset/theme.toml from default"
[ -f "$ZEN/themes/sunset/theme.toml" ] && ok "theme new wrote the bundle" || bad "no themes/sunset/theme.toml"
capture zen theme new sunset
assert_eq "theme new never overwrites" "$rc" "1"
assert_contains "theme new conflict message" "$out" "already exists"
capture zen theme new "Not OK"
assert_eq "bad theme name exits 1" "$rc" "1"
capture zen validate --theme sunset
assert_eq "the starter validates" "$rc" "0"
printf 'schema = 1\n[terminal.dark]\nbluee = "#00f"\n' >"$ZEN/themes/sunset/theme.toml"
capture zen validate --theme sunset
assert_eq "bad theme token exits 1" "$rc" "1"
assert_contains "theme error has file:line:col" "$out" "themes/sunset/theme.toml:3:1: unknown key 'bluee'"
printf 'schema = 1\n[colors.dark]\naccent = "#ff9e64"\n' >"$ZEN/themes/sunset/theme.toml"
capture zen theme list
assert_contains "list shows the user theme last" "$out" "  sunset  (yours)"

echo "== headless switch via curl =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{"name":"sunset"}')"
assert_contains "curl theme/set answers" "$resp" '"theme":"sunset"'
got="$(curl -s "http://127.0.0.1:$PORT/cli/zen/get?token=$TOKEN&home=home-1" | python3 -c '
import json, sys
d = json.load(sys.stdin)
t = d["theme"]
print(t["name"], t["builtin"], t["tokens"]["colors"]["dark"]["accent"], t["font"]["family"], "bg" if "background" in t else "nobg", ",".join(x["name"] for x in d["themes"]))')"
assert_eq "get shows the curl switch" "$got" "sunset False #ff9e64 system nobg default,paper,midnight,sunset"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN")"
assert_eq "GET theme/set is 405" "$code" "405"
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN" --data-raw '{"name":"neon"}')"
assert_eq "curl unknown theme is 404" "$code" "404"

echo "== reset --theme clears an override =="
capture zen theme new paper
assert_eq "override paper" "$rc" "0"
assert_contains "override copies K2's paper" "$out" "from paper"
capture zen theme list
assert_contains "list marks the override" "$out" "paper  (built in, your override)"
capture zen reset --theme paper
assert_eq "reset --theme exit" "$rc" "0"
assert_contains "reset drops the override" "$out" "reset themes/paper/theme.toml to builtin"
[ ! -f "$ZEN/themes/paper/theme.toml" ] && ok "the override file is gone" || bad "override still there"
capture zen reset --theme neon
assert_eq "reset an unknown theme exits 1" "$rc" "1"

echo "== doctor =="
capture zen doctor
assert_eq "doctor exit" "$rc" "0"
assert_contains "doctor reports the watcher" "$out" "ok   watcher"

echo "== agents can't grant =="
capture zen grant thread:post
assert_eq "no grant verb" "$rc" "2"
assert_contains "grant refusal says why" "$out" "Agents never grant permissions"
capture zen reset --file grants.json
assert_eq "reset can't name grants.json" "$rc" "1"
capture zen validate --file homes.json
assert_eq "validate can't name homes.json" "$rc" "1"
[ ! -e "$ZEN/grants.json" ] && ok "nothing wrote grants.json" || bad "grants.json exists"

echo "== a token that isn't this computer's owner =="
# Empty HOME so the CLI can't discover this daemon's owner token on disk.
mkdir -p "$SANDBOX/otherhome"
set +e
out="$(env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN HOME="$SANDBOX/otherhome" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="not-the-owner" "$K2_CLI" zen validate 2>&1)"
rc=$?
set -e
assert_eq "non-owner token exits 1" "$rc" "1"
assert_contains "non-owner message" "$out" "talks only to the K2 on this computer"

echo
echo "zen_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
