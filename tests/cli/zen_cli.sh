#!/usr/bin/env bash
# k2 zen (prd-zen-mode-v1 Z17, T3.1; prd-zen-gardens-v1 G35, TG2.1)
# against a REAL headless daemon.
#
# Boots the worktree's k2-daemon under a temp HOME (never the real ~/.k2,
# never a production daemon), then:
#   - before Zen is set up: every verb exits 3 with the top-bar sentence;
#   - setup over curl (as the app does) makes Garden 1 (texting) and Garden 2 (empty);
#   - garden new/list/rename/reorder/delete; a name clash and the last
#     Garden exit 1; a Garden made over curl gets the empty template;
#   - zen.toml with `acent` at line 7 → `zen.toml:7:3: unknown key 'acent'`, exit 1;
#   - --garden resolves a Garden by name; --home and `pages` are gone;
#   - history (deleted Gardens included) and reset --to;
#   - garden template: Start with the default (texting) and back to blank,
#     same id/name/place, the old file kept in history, idempotent, a file
#     with its own widgets needs --force, GET is 405;
#   - themes (K2's are basic, paper, midnight; a saved `default` pick reads
#     as basic, quietly): list/next/prev/set/new, per-Garden picks, an unknown name
#     exits 1, reset --theme clears an override, and a curl-only switch (no
#     CLI, no client) shows up in /cli/zen/get;
#   - there is no grant verb, and nothing writes grants.json.
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
assert_contains "help names garden new" "$out" "k2 zen garden new <name>"
assert_contains "help names garden template" "$out" "k2 zen garden template <garden> texting|blank [--force]"
assert_contains "help states the grant rule" "$out" "Never write grants.json"
capture help zen
assert_eq "k2 help zen exit" "$rc" "0"
assert_contains "k2 help zen" "$out" "k2 zen reset"

echo "== not set up (TG2.1) =="
NOT_SET_UP="Zen isn't set up on this computer. Turn it on with the Zen toggle in the K2 app's top bar."
capture zen theme list
assert_eq "zen theme list before setup exits 3" "$rc" "3"
for verb in validate path history reload "garden list" "garden new Notes" "garden rename Notes Home" "garden reorder Notes 1" "garden template Notes texting" "garden delete Notes"; do
    # shellcheck disable=SC2086
    capture zen $verb
    assert_eq "zen $verb before setup exits 3" "$rc" "3"
    assert_contains "zen $verb before setup says so" "$out" "$NOT_SET_UP"
done
[ ! -e "$ZEN" ] && ok "no CLI verb creates ~/.k2/zen" || bad "a CLI verb created ~/.k2/zen"

echo "== set up (as the app does) =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup created the folder" "$resp" '"createdFolder":true'
DEFAULT_ID="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["gardens"][0]["id"])')"
SECOND_ID="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["gardens"][1]["id"])')"
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup again makes nothing new" "$resp" '"createdDefault":false'
capture zen path
assert_eq "zen path exit" "$rc" "0"
assert_eq "zen path prints the folder" "$out" "$ZEN"
capture zen garden list
assert_eq "garden list exit" "$rc" "0"
assert_eq "garden list shows Garden 1 and Garden 2" "$out" "  1  $DEFAULT_ID  Garden 1  k2.texting@1
  2  $SECOND_ID  Garden 2  k2.blank@1"
capture zen validate
assert_eq "fresh setup validates" "$rc" "0"
assert_contains "validate ok line" "$out" "ok: zen.toml, gardens/$DEFAULT_ID.toml"
assert_contains "validate covers Garden 2" "$out" "gardens/$SECOND_ID.toml"

echo "== gardens (TG2.1) =="
capture zen garden new Notes
assert_eq "garden new exit" "$rc" "0"
NOTES_ID="$(printf '%s\n' "$out" | sed -n 1p)"
case "$NOTES_ID" in g-????????) ok "garden new prints a g- id ($NOTES_ID)" ;; *) bad "garden new id: $(printf %q "$out")" ;; esac
assert_eq "garden new prints the file" "$(printf '%s\n' "$out" | sed -n 2p)" "$ZEN/gardens/$NOTES_ID.toml"
[ -f "$ZEN/gardens/$NOTES_ID.toml" ] && ok "the stub is on disk" || bad "no stub for $NOTES_ID"
capture zen garden list --json
assert_eq "garden list --json exit" "$rc" "0"
got="$(printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(",".join("%s:%s:%s" % (g["index"], g["name"], g["template"]) for g in d["gardens"]))')"
assert_eq "list --json parses with the new entry" "$got" "1:Garden 1:k2.texting@1,2:Garden 2:k2.blank@1,3:Notes:k2.blank@1"
capture zen garden new notes
assert_eq "a name clash exits 1" "$rc" "1"
assert_contains "clash message" "$out" "You already have a Garden called"
capture zen garden new Launch room --texting --at 1
assert_eq "garden new --texting --at exit" "$rc" "0"
LAUNCH_ID="$(printf '%s\n' "$out" | sed -n 1p)"
capture zen garden list
assert_contains "--at 1 puts it first" "$(printf '%s\n' "$out" | sed -n 1p)" "1  $LAUNCH_ID  Launch room  k2.texting@1"
capture zen garden rename "Launch room" Mornings
assert_eq "rename exit" "$rc" "0"
assert_contains "rename says so" "$out" "Garden $LAUNCH_ID is now"
capture zen garden reorder Mornings 4
assert_eq "reorder exit" "$rc" "0"
assert_contains "reorder lists the new order" "$(printf '%s\n' "$out" | sed -n 4p)" "4  $LAUNCH_ID  Mornings"
capture zen garden reorder Mornings 9
assert_eq "reorder out of range exits 1" "$rc" "1"
capture zen garden reorder Mornings first
assert_eq "reorder needs a number" "$rc" "2"
capture zen garden new
assert_eq "garden new with no name exits 2" "$rc" "2"
capture zen garden rename Nowhere x
assert_eq "an unknown Garden exits 1" "$rc" "1"
assert_contains "unknown Garden message" "$out" "no Garden 'Nowhere'"

echo "== headless curl create → empty template =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/garden/new?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{"name":"Curl made"}')"
CURL_ID="$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin)["garden"]["id"])')"
got="$(curl -s "http://127.0.0.1:$PORT/cli/zen/get?token=$TOKEN&garden=$CURL_ID" | python3 -c '
import json, sys
d = json.load(sys.stdin)
p = d["page"]
print(p["template"], ",".join(w["kind"] for w in p["widgets"]), ",".join(c["kind"] for c in p["controls"]), d["garden"]["name"])')"
assert_eq "get returns the empty template" "$got" "k2.blank@1 garden-empty garden-switcher,drag-region,zen-toggle Curl made"

echo "== garden template: Start with the default =="
two_snaps() { python3 -c 'import os,sys; d=sys.argv[1]; print(len([n for n in os.listdir(d) if n.endswith(".toml")]) if os.path.isdir(d) else 0)' "$ZEN/.history/gardens/$SECOND_ID.toml"; }
before_snaps="$(two_snaps)"
capture zen garden template "Garden 2" texting
assert_eq "template texting exit" "$rc" "0"
assert_contains "template says what it did" "$out" "Garden $SECOND_ID (“Garden 2”) is now k2.texting@1"
assert_contains "template names the kept snapshot" "$out" "its previous file is kept as snapshot"
grep -Fq 'template = "k2.texting@1"' "$ZEN/gardens/$SECOND_ID.toml" && ok "Garden 2's file is the texting stub" || bad "Garden 2's file: $(cat "$ZEN/gardens/$SECOND_ID.toml")"
[ "$(two_snaps)" -gt 0 ] && ok "the old file is in .history ($before_snaps -> $(two_snaps))" || bad "no snapshot of Garden 2"
capture zen garden list
assert_contains "same id, name and place, now texting" "$out" "  2  $SECOND_ID  Garden 2  k2.texting@1"
got="$(curl -s "http://127.0.0.1:$PORT/cli/zen/get?token=$TOKEN&garden=$SECOND_ID" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["page"]["template"], ",".join(w["kind"] for w in d["page"]["widgets"]), len(d["errors"]))')"
assert_eq "get shows Garden 1's page on Garden 2" "$got" "k2.texting@1 agents,conversation,nav-rail 0"
capture zen garden template "$SECOND_ID" texting
assert_eq "template again exits 0" "$rc" "0"
assert_contains "template again is a no-op" "$out" "is already k2.texting@1 (unchanged)"
capture zen garden template "Garden 2" blank --json
assert_eq "template blank exit" "$rc" "0"
assert_contains "template blank --json" "$out" '"template": "k2.blank@1"'
grep -Fq 'template = "k2.blank@1"' "$ZEN/gardens/$SECOND_ID.toml" && ok "Garden 2 is empty again" || bad "Garden 2 not blank"
capture zen garden template "Garden 2" fancy
assert_eq "an unknown template exits 2" "$rc" "2"
capture zen garden template "Garden 2"
assert_eq "template with no template exits 2" "$rc" "2"
capture zen garden template Nowhere texting
assert_eq "template on an unknown Garden exits 1" "$rc" "1"
assert_contains "unknown Garden says so" "$out" "no Garden 'Nowhere'"
capture zen garden rename "Garden 2" x --force
assert_eq "--force is only for template" "$rc" "2"
printf 'schema = 1\ntemplate = "k2.blank@1"\n[[widget]]\nid = "work"\nkind = "agents"\ncolumn = 0\n' >"$ZEN/gardens/$CURL_ID.toml"
capture zen reload
capture zen garden template "Curl made" texting
assert_eq "a Garden with its own widgets needs --force" "$rc" "1"
assert_contains "the refusal names what it has" "$out" "has its own changes (widget)"
grep -Fq '[[widget]]' "$ZEN/gardens/$CURL_ID.toml" && ok "a refused template wrote nothing" || bad "the refused template changed the file"
capture zen garden template "Curl made" texting --force
assert_eq "--force goes ahead" "$rc" "0"
grep -Fq 'template = "k2.texting@1"' "$ZEN/gardens/$CURL_ID.toml" && ok "--force wrote the texting stub" || bad "no texting stub"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/garden/template?token=$TOKEN")"
assert_eq "GET garden/template is 405" "$code" "405"

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

echo "== a Garden page by name =="
printf 'schema = 1\n[[widget]]\nid = "work"\nkind = "agents"\ncolumn = 0\n[widget.props]\nhom = "Work"\n' >"$ZEN/gardens/$NOTES_ID.toml"
capture zen validate --garden Notes
assert_eq "--garden Notes validate exits 1" "$rc" "1"
assert_contains "page error names the Garden file" "$out" "gardens/$NOTES_ID.toml:7:1: unknown key 'hom'"
printf 'schema = 1\n[[widget]]\nid = "work"\nkind = "agents"\ncolumn = 0\n[widget.props]\nhome = "Work"\nagent = "cortana"\n' >"$ZEN/gardens/$NOTES_ID.toml"
capture zen validate --garden "$NOTES_ID"
assert_eq "a one-agent widget validates" "$rc" "0"
capture zen validate --garden Nowhere
assert_eq "unknown Garden exits 1" "$rc" "1"
capture zen reset --garden Notes
assert_eq "reset --garden Notes exits 0" "$rc" "0"
assert_contains "reset the page to its stub" "$out" "reset gardens/$NOTES_ID.toml to default"
grep -Fq 'template = "k2.blank@1"' "$ZEN/gardens/$NOTES_ID.toml" && ok "a blank Garden resets to the blank stub" || bad "stub template"
capture zen validate
assert_eq "after page reset everything validates" "$rc" "0"
capture zen validate --home Work
assert_eq "--home is gone (exit 2)" "$rc" "2"
assert_contains "--home points at --garden" "$out" "Use --garden"
capture zen pages
assert_eq "zen pages is gone (exit 2)" "$rc" "2"
assert_contains "zen pages points at garden list" "$out" "k2 zen garden list"

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

echo "== delete =="
capture zen garden delete "Curl made"
assert_eq "delete exit" "$rc" "0"
assert_contains "delete names the Garden" "$out" "deleted Garden $CURL_ID"
assert_contains "delete says where the page went" "$out" "its page is kept in .history"
[ ! -f "$ZEN/gardens/$CURL_ID.toml" ] && ok "the page left gardens/" || bad "page still in gardens/"
capture zen history --garden "$CURL_ID"
assert_eq "history of a deleted Garden by id" "$rc" "0"
assert_contains "history marks it deleted" "$out" "Garden deleted"
for g in "$LAUNCH_ID" Notes "Garden 2"; do
    capture zen garden delete "$g"
    assert_eq "delete $g" "$rc" "0"
done
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup never brings a deleted Garden 2 back" "$resp" '"createdDefault":false'
capture zen garden list
assert_eq "only Garden 1 is left" "$out" "  1  $DEFAULT_ID  Garden 1  k2.texting@1"
capture zen garden delete "Garden 1"
assert_eq "the last Garden exits 1" "$rc" "1"
assert_eq "the last Garden says so" "$out" "That's your last Garden."

echo "== themes =="
# Rosson's test machine saved the old name: it reads as basic, quietly.
printf '{"version":2,"theme":"default","gardens":{}}\n' >"$ZEN/active.json"
capture zen theme list
assert_eq "theme list exit" "$rc" "0"
assert_contains "a saved default pick reads as basic" "$out" "* basic  Basic  (built in)"
case "$out" in *"doesn't exist"*) bad "the default alias warned: $out" ;; *) ok "the default alias is quiet" ;; esac
case "$out" in *" default "*) bad "list still names default: $out" ;; *) ok "no theme is called default any more" ;; esac
assert_contains "list shows paper" "$out" "  paper  Paper  (built in)"
assert_contains "list shows midnight" "$out" "  midnight  Midnight  (built in)"
capture zen theme next
assert_eq "theme next exit" "$rc" "0"
assert_eq "next goes to paper" "$out" "theme paper for this computer"
capture zen theme next
assert_eq "next goes to midnight" "$out" "theme midnight for this computer"
capture zen theme next
assert_eq "next wraps to basic" "$out" "theme basic for this computer"
capture zen theme prev
assert_eq "prev wraps to midnight" "$out" "theme midnight for this computer"
capture zen theme set neon
assert_eq "unknown theme exits 1" "$rc" "1"
assert_contains "unknown theme says so" "$out" "no theme 'neon' on this computer; themes: basic, paper, midnight"
capture zen theme list --json
active="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["active"])')"
assert_eq "an unknown name changes nothing" "$active" "midnight"
capture zen theme set
assert_eq "set with no name exits 2" "$rc" "2"
capture zen theme set paper --garden "Garden 1"
assert_eq "set --garden exit" "$rc" "0"
assert_eq "set --garden says so" "$out" "theme paper for Garden $DEFAULT_ID"
capture zen theme list --garden "Garden 1"
assert_contains "list --garden marks paper" "$out" "* paper"
assert_contains "list --garden notes the pick" "$out" "Garden $DEFAULT_ID: its own pick"
capture zen garden list
assert_contains "garden list stars a Garden with its own theme" "$out" "* 1  $DEFAULT_ID  Garden 1"
capture zen theme set --garden "Garden 1" --clear
assert_eq "clear exit" "$rc" "0"
assert_eq "clear says so" "$out" "Garden $DEFAULT_ID follows this computer: theme midnight"
capture zen theme set paper --home Work
assert_eq "theme --home is gone (exit 2)" "$rc" "2"

capture zen theme new sunset
assert_eq "theme new exit" "$rc" "0"
assert_contains "theme new names the file" "$out" "made $ZEN/themes/sunset/theme.toml from basic"
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
assert_contains "list shows the user theme last" "$out" "  sunset  Sunset  (yours)"

echo "== headless switch via curl =="
resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{"name":"sunset"}')"
assert_contains "curl theme/set answers" "$resp" '"theme":"sunset"'
got="$(curl -s "http://127.0.0.1:$PORT/cli/zen/get?token=$TOKEN&garden=$DEFAULT_ID" | python3 -c '
import json, sys
d = json.load(sys.stdin)
t = d["theme"]
print(t["name"], t["builtin"], t["tokens"]["colors"]["dark"]["accent"], t["font"]["family"], "bg" if "background" in t else "nobg", ",".join(x["name"] for x in d["themes"]))')"
assert_eq "get shows the curl switch" "$got" "sunset False #ff9e64 system nobg basic,paper,midnight,sunset"
code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN")"
assert_eq "GET theme/set is 405" "$code" "405"
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/zen/theme/set?token=$TOKEN" --data-raw '{"name":"neon"}')"
assert_eq "curl unknown theme is 404" "$code" "404"
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/cli/zen/page/ensure?token=$TOKEN" --data-raw '{"homeId":"h","name":"x"}')"
assert_eq "the old page/ensure route is gone" "$code" "405"

echo "== reset --theme clears an override =="
capture zen theme new paper
assert_eq "override paper" "$rc" "0"
assert_contains "override copies K2's paper" "$out" "from paper"
capture zen theme list
assert_contains "list marks the override" "$out" "paper  Paper  (built in, your override)"
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
assert_contains "doctor reports the Garden list" "$out" "ok   gardens.json: 1 Garden(s): Garden 1"
assert_contains "doctor reports the templates" "$out" "ok   templates"

echo "== agents can't grant =="
capture zen grant thread:post
assert_eq "no grant verb" "$rc" "2"
assert_contains "grant refusal says why" "$out" "Agents never grant permissions"
capture zen reset --file grants.json
assert_eq "reset can't name grants.json" "$rc" "1"
capture zen validate --file gardens.json
assert_eq "validate can't name gardens.json" "$rc" "1"
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
