#!/usr/bin/env bash
# k2 zen guide (prd-zen-freeform-chrome S4: FC33–FC35, FC-T15, FC-T16,
# FC-T18; Cortana's FC59–FC67) against a REAL headless daemon.
#
#   - No daemon (empty HOME, no PORT/TOKEN): the index, every topic, --json,
#     every example (--toml too), k2 zen / help / --help and k2 study zen
#     exit 0; an unknown topic or example exits 2; every page is at most 80
#     lines; the examples come in Cortana's order (FC65).
#   - Then the worktree's k2-daemon boots under a temp HOME (never the real
#     ~/.k2, never a production daemon). Zen is set up over curl, and a
#     SCRATCH Garden is made in that temp folder (FC67). Every example from
#     `example --list` is piped into the scratch Garden's file and must pass
#     `k2 zen validate`, load with `errors: []`, and place its controls where
#     this test's table says (a name the table doesn't know fails).
#   - The wrong files the guide shows (widgets, required, bands) are written
#     and must FAIL validate with the line the guide quotes.
#   - Every safe-mode line the guide quotes is a line the app can show
#     (src/renderer/lib/zen/zen-view.ts).
#   - The daemon is stopped and the guide still works.
# Build first: cargo build -p k2-daemon (CARGO_TARGET_DIR honoured).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
ZEN_VIEW="$PROJECT_ROOT/src/renderer/lib/zen/zen-view.ts"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
[ -f "$ZEN_VIEW" ] || { echo "FAIL: $ZEN_VIEW not found" >&2; exit 1; }
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

SANDBOX="$(mktemp -d -t k2-zen-guide-XXXXXX)"
DAEMON_PID=""
stop_daemon() {
    if [ -n "$DAEMON_PID" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in $(seq 1 30); do
            kill -0 "$DAEMON_PID" 2>/dev/null || break
            sleep 0.1
        done
        kill -9 "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
        DAEMON_PID=""
    fi
}
cleanup() {
    stop_daemon
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

TOPICS="gardens files widgets bands required menus themes examples undo safe-mode"
# FC34's names in Cortana's order (FC65), menu-bottom-right before the
# later column-corner.
WANT_EXAMPLES="rail-top quiet-top bottom-bar menu-both texting-chrome menu-bottom-right column-corner"
MAX_LINES=80

# ── No daemon (FC-T16) ──────────────────────────────────────────────────
mkdir -p "$SANDBOX/nodaemon"
nd() {
    env -i HOME="$SANDBOX/nodaemon" PATH="/usr/bin:/bin:/usr/sbin:/sbin" "$K2_CLI" "$@"
}
ndcap() {
    set +e
    out="$(nd "$@" 2>&1)"
    rc=$?
    set -e
}
line_count() { printf '%s\n' "$1" | wc -l | tr -d ' '; }
assert_short() {
    local n
    n="$(line_count "$2")"
    if [ "$n" -le "$MAX_LINES" ]; then ok "$1 is $n lines (<= $MAX_LINES)"; else bad "$1 is $n lines (> $MAX_LINES)"; fi
}

echo "== no daemon: the index and every topic =="
ndcap zen guide
assert_eq "guide (index) exits 0" "$rc" "0"
assert_contains "the index says where to start" "$out" "Start here: k2 zen guide gardens"
assert_short "the index" "$out"
for t in $TOPICS; do
    assert_contains "the index lists $t" "$out" "  $t "
done
INDEX_JSON="$(nd zen guide --json)"
got="$(printf '%s' "$INDEX_JSON" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["id"], " ".join(t["id"] for t in d["topics"]), "body" if d["body"].startswith("k2 zen guide") else "nobody")')"
assert_eq "guide --json: id, topics in order, body" "$got" "index $TOPICS body"
for t in $TOPICS; do
    ndcap zen guide "$t"
    assert_eq "guide $t exits 0" "$rc" "0"
    assert_contains "guide $t starts with its name" "$(printf '%s\n' "$out" | sed -n 1p)" "k2 zen guide $t —"
    assert_short "guide $t" "$out"
    page="$out"
    ndcap zen guide "$t" --json
    assert_eq "guide $t --json exits 0" "$rc" "0"
    got="$(printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(d["id"], bool(d["title"]))')"
    assert_eq "guide $t --json has id and title" "$got" "$t True"
    body="$(printf '%s' "$out" | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["body"])')"
    assert_eq "guide $t --json body is the page" "$body" "$page"
done
ndcap zen guide nosuch
assert_eq "an unknown topic exits 2" "$rc" "2"
assert_contains "an unknown topic says so" "$out" "no guide topic 'nosuch'"
ndcap zen guide bands --toml
assert_eq "--toml on a topic exits 2" "$rc" "2"
ndcap zen guide bands extra
assert_eq "an extra argument exits 2" "$rc" "2"
ndcap zen guide --bogus
assert_eq "an unknown flag exits 2" "$rc" "2"
ndcap zen guide --help
assert_eq "guide --help exits 0" "$rc" "0"
assert_contains "guide --help shows the pipe" "$out" "k2 zen guide example bottom-bar --toml > ~/.k2/zen/gardens/<id>.toml"

echo "== no daemon: the examples =="
EXAMPLES="$(nd zen guide example --list | tr '\n' ' ' | sed 's/ $//')"
assert_eq "example --list is FC34's names in FC65's order" "$EXAMPLES" "$WANT_EXAMPLES"
for n in $EXAMPLES; do
    ndcap zen guide example "$n"
    assert_eq "example $n exits 0" "$rc" "0"
    assert_short "example $n" "$out"
    assert_contains "example $n shows how to pipe it" "$out" "k2 zen guide example $n --toml > ~/.k2/zen/gardens/<id>.toml"
    ndcap zen guide example "$n" --toml
    assert_eq "example $n --toml exits 0" "$rc" "0"
    assert_eq "example $n --toml is a whole file" "$(printf '%s\n' "$out" | sed -n 1p)" "schema = 1"
    toml="$out"
    ndcap zen guide examples "$n" --json
    assert_eq "examples $n --json exits 0" "$rc" "0"
    got="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.stdout.write(d["id"] + "\n" + d["toml"])')"
    assert_eq "example $n --json carries the same toml" "$got" "$n
$toml"
done
ndcap zen guide examples
assert_eq "guide examples (the page) exits 0" "$rc" "0"
for n in $WANT_EXAMPLES; do
    assert_contains "the examples page lists $n" "$out" "  $n "
done
assert_contains "rail-top says it keeps the switcher (FC65)" "$(nd zen guide example rail-top --toml)" "This does NOT remove the switcher"
assert_contains "column-corner explains edge vs slot (FC65)" "$(nd zen guide example column-corner --toml)" "It is not the page's bottom band: that would be slot = \"bottom\"."
ndcap zen guide example nosuch
assert_eq "an unknown example exits 2" "$rc" "2"
ndcap zen guide example --toml
assert_eq "--toml without a name exits 2" "$rc" "2"
ndcap zen guide example bottom-bar --list
assert_eq "--list with a name exits 2" "$rc" "2"
got="$(nd zen guide example --list --json | python3 -c 'import json,sys; print(" ".join(e["id"] for e in json.load(sys.stdin)["examples"]))')"
assert_eq "example --list --json" "$got" "$WANT_EXAMPLES"

echo "== no daemon: help hooks and k2 study zen (FC35, FC55) =="
for args in "zen" "zen help" "zen --help" "zen -h" "zen validate --help" "help zen"; do
    # shellcheck disable=SC2086
    ndcap $args
    if [ "$args" = "help zen" ]; then
        # `k2 help` stays daemon-gated (tests/cli/study.sh pins that).
        assert_eq "k2 help zen still needs a daemon" "$rc" "1"
        continue
    fi
    assert_eq "k2 $args exits 0 with no daemon" "$rc" "0"
    assert_contains "k2 $args names the guide" "$out" "k2 zen guide [<topic>] [--json]"
    assert_contains "k2 $args says where to start" "$out" "New to Gardens? k2 zen guide."
done
ndcap zen validate
assert_eq "a daemon verb still needs a daemon" "$rc" "1"
assert_contains "a daemon verb says it can't connect" "$out" "Cannot connect to K2"
ndcap study zen
assert_eq "k2 study zen exits 0" "$rc" "0"
assert_contains "study zen states the one rule" "$out" "Every page keeps two buttons the human can reach in one click"
assert_contains "study zen points to the guide" "$out" "k2 zen guide                       short pages; no daemon needed"
assert_short "k2 study zen" "$out"
assert_contains "study catalog lists zen" "$(nd study)" "  zen                 Zen Gardens"
got="$(nd study zen --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["id"], "k2 zen guide" in d["body"])')"
assert_eq "study zen --json" "$got" "zen True"
got="$(nd --schema | python3 -c '
import json, sys
d = json.load(sys.stdin)
cmds = d["commands"] if isinstance(d, dict) else d
print([c["usage"] for c in cmds if c.get("name") == "zen guide"][0].split(" |")[0])')"
assert_eq "--schema lists zen guide" "$got" "k2 zen guide [<topic>] [--json]"

# ── FC-T18 drift: the pages name every grammar value ─────────────────────
echo "== drift: pages name the grammar =="
BANDS_PAGE="$(nd zen guide bands)"
for v in '"column"' '"top"' '"bottom"' '"menu"' '"start"' '"center"' '"end"' 'edge = "top" | "bottom"' 'drag-region'; do
    assert_contains "bands names $v" "$BANDS_PAGE" "$v"
done
# FC59: the two `top`s side by side, on the same lines.
assert_contains "bands: slot = top beside slot = column (FC59)" "$BANDS_PAGE" '  slot = "top"                       slot = "column"'
assert_contains "bands: edge = top under slot = column (FC59)" "$BANDS_PAGE" '                                     edge = "top"'
assert_contains "bands: the last end item is in the corner (FC60)" "$BANDS_PAGE" 'In an "end" group the LAST item in the file sits
  in the corner.'
assert_contains "bands: never place drag-region (FC62)" "$BANDS_PAGE" "Never place drag-region. The empty space in every band IS the drag"
WIDGETS_PAGE="$(nd zen guide widgets)"
for k in agents conversation nav-rail garden-switcher zen-toggle usage theme-picker menu; do
    assert_contains "widgets names $k" "$WIDGETS_PAGE" "$k"
done
assert_contains "widgets opens with the replace rules (FC63)" "$(printf '%s\n' "$WIDGETS_PAGE" | sed -n 3,4p)" "two groups"
MENUS_PAGE="$(nd zen guide menus)"
for k in zen-toggle garden-switcher theme-picker usage '"dots"' '"bars"' '"zen"' "1 to 6 items" "3 menus"; do
    assert_contains "menus names $k" "$MENUS_PAGE" "$k"
done
REQUIRED_PAGE="$(nd zen guide required)"
for k in zen-toggle garden-switcher drag-region "12 content widgets, 10 Zen controls, 8 items per
    band, 4 per column edge, 3 menus"; do
    assert_contains "required names $k" "$REQUIRED_PAGE" "$k"
done
UNDO_PAGE="$(nd zen guide undo)"
assert_contains "undo: an older app keeps the old strip (FC64)" "$UNDO_PAGE" "the app is older than K2 on this computer (the daemon)"
assert_contains "undo: don't edit K2 source (FC64)" "$UNDO_PAGE" "Don't edit K2's source to force it."
assert_contains "undo: chrome-only files need --force (FC49)" "$UNDO_PAGE" "A file with its own layout, widgets (Zen controls count) or theme
  tables needs --force."

# ── The daemon (FC-T15) ─────────────────────────────────────────────────
echo "== boot a headless daemon in a temp HOME =="
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
phase=""
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

resp="$(curl -s -X POST "http://127.0.0.1:$PORT/cli/zen/setup?token=$TOKEN" -H 'Content-Type: application/json' --data-raw '{}')"
assert_contains "setup made the temp Zen folder" "$resp" '"createdFolder":true'
[ "$(cd "$ZEN" && pwd -P)" = "$(cd "$SANDBOX/.k2/zen" && pwd -P)" ] && ok "the Zen folder is under the temp HOME" || bad "Zen folder outside the sandbox"
capture zen garden new "Guide scratch"
assert_eq "a scratch Garden for the examples (FC67)" "$rc" "0"
SCRATCH="$(printf '%s\n' "$out" | sed -n 1p)"
case "$SCRATCH" in g-????????) ok "scratch Garden id $SCRATCH" ;; *) echo "FAIL: scratch id: $out" >&2; exit 1 ;; esac
SCRATCH_FILE="$ZEN/gardens/$SCRATCH.toml"
case "$SCRATCH_FILE" in "$SANDBOX"/*) ok "the scratch file is in the sandbox" ;; *) echo "FAIL: $SCRATCH_FILE outside the sandbox" >&2; exit 1 ;; esac

# Where each example puts things, read from GET /cli/zen/get:
# chrome.from; then each band as start/center/end; then column edges; then
# menus. "-" is an empty group or a band that isn't drawn.
expected_layout() {
    case "$1" in
        rail-top) echo "from=template top=garden-switcher,nav-rail/-/usage,theme-picker,zen-toggle bottom=- edges=- menus=-" ;;
        quiet-top) echo "from=garden top=garden-switcher/-/theme-picker,zen-toggle bottom=- edges=- menus=-" ;;
        bottom-bar) echo "from=garden top=- bottom=garden-switcher/-/theme-picker,zen-toggle edges=- menus=-" ;;
        menu-both) echo "from=garden top=-/-/more bottom=- edges=- menus=more:garden-switcher,theme-picker,zen-toggle" ;;
        texting-chrome) echo "from=garden top=garden-switcher/-/usage,theme-picker,zen-toggle bottom=- edges=- menus=-" ;;
        menu-bottom-right) echo "from=garden top=- bottom=-/-/all edges=- menus=all:garden-switcher,usage,theme-picker,zen-toggle" ;;
        column-corner) echo "from=garden top=garden-switcher/-/theme-picker bottom=- edges=1:bottom:-/-/zen-toggle menus=-" ;;
        *) return 1 ;;
    esac
}
layout_of() {
    curl -s "http://127.0.0.1:$PORT/cli/zen/get?token=$TOKEN&garden=$SCRATCH" | python3 -c '
import json, sys
d = json.load(sys.stdin)
p = d["page"]
def g(x):
    return ",".join(x) or "-"
def grp(b):
    return "-" if b is None else "%s/%s/%s" % (g(b["start"]), g(b["center"]), g(b["end"]))
edges = ";".join("%s:%s:%s" % (e["column"], e["edge"], grp(e)) for e in p["edges"]) or "-"
menus = ";".join("%s:%s" % (k, g(v)) for k, v in sorted(p["menus"].items())) or "-"
print("errors=%d from=%s top=%s bottom=%s edges=%s menus=%s" % (len(d["errors"]), p["chrome"]["from"], grp(p["bands"]["top"]), grp(p["bands"]["bottom"]), edges, menus))'
}

echo "== every example validates in the scratch Garden (FC-T15) =="
for n in $EXAMPLES; do
    want="$(expected_layout "$n" || true)"
    if [ -z "$want" ]; then
        bad "example $n is not in this test's table: add its expected placement"
        continue
    fi
    nd zen guide example "$n" --toml >"$SCRATCH_FILE"
    capture zen validate --garden "$SCRATCH"
    assert_eq "example $n: validate --garden exits 0" "$rc" "0"
    assert_contains "example $n: validate ok line" "$out" "ok: "
    assert_contains "example $n: validate covered the scratch file" "$out" "gardens/$SCRATCH.toml"
    capture zen validate --garden "$SCRATCH" --json
    got="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(len(d["errors"]), len(d.get("warnings") or []))')"
    assert_eq "example $n: no errors, no warnings" "$got" "0 0"
    capture zen reload
    assert_eq "example $n: reload exits 0" "$rc" "0"
    assert_eq "example $n: placed as documented" "$(layout_of)" "errors=0 $want"
done

echo "== the wrong files fail with the line the guide quotes =="
# The guide writes <id>; validate prints the scratch Garden's id.
quoted_error() {
    printf '%s\n' "$1" | grep -E '^  gardens/<id>\.toml:' | sed -e 's/^  //' -e "s/<id>/$SCRATCH/"
}
# widgets (FC63): three lines, not a whole file; the page says "under schema = 1".
wrong="$(printf '%s\n' "$WIDGETS_PAGE" | sed -n '/^WRONG FILE/,/^k2 zen validate prints/p' | grep -E '^    ' | sed 's/^    //')"
assert_eq "the widgets wrong file is three lines (FC63)" "$(line_count "$wrong")" "3"
printf 'schema = 1\n%s\n' "$wrong" >"$SCRATCH_FILE"
want="$(quoted_error "$WIDGETS_PAGE")"
[ -n "$want" ] && ok "widgets quotes a validate line" || bad "widgets quotes no validate line"
capture zen validate --garden "$SCRATCH"
assert_eq "the widgets wrong file fails validate" "$rc" "1"
assert_contains "the widgets wrong file gives the quoted error" "$out" "$want"
assert_contains "the error's own hint names the required page" "$out" "See k2 zen guide required."
# required (FC66): a whole failing file, still not pipeable.
wrong="$(printf '%s\n' "$REQUIRED_PAGE" | sed -n '/^FAILING FILE/,/^k2 zen validate prints/p' | grep -E '^    ' | sed 's/^    //')"
assert_eq "the required failing file starts with schema = 1" "$(printf '%s\n' "$wrong" | sed -n 1p)" "schema = 1"
printf '%s\n' "$wrong" >"$SCRATCH_FILE"
want="$(quoted_error "$REQUIRED_PAGE")"
[ -n "$want" ] && ok "required quotes a validate line" || bad "required quotes no validate line"
capture zen validate --garden "$SCRATCH"
assert_eq "the required failing file fails validate" "$rc" "1"
assert_contains "the required failing file gives the quoted error" "$out" "$want"
# ...and the fix the page shows makes it pass.
fix="$(printf '%s\n' "$REQUIRED_PAGE" | sed -n '/Fix: add$/,/^$/p' | grep -E '^    ' | sed 's/^    //')"
printf '%s\n%s\n' "$wrong" "$fix" >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_eq "the required page's fix validates" "$rc" "0"
# bands (FC62): the drag-region sentence is the real one.
printf 'schema = 1\n[[widget]]\nkind = "drag-region"\nslot = "top"\n' >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_eq "drag-region fails validate" "$rc" "1"
quote="$(printf '%s\n' "$BANDS_PAGE" | grep -F 'K2 makes the empty space' | sed 's/^ *//')"
assert_contains "bands quotes the real drag-region error" "$out" "$quote"
# menus: slot = "menu" needs both keys.
printf 'schema = 1\n[[widget]]\nkind = "garden-switcher"\nslot = "top"\n[[widget]]\nkind = "zen-toggle"\nslot = "menu"\n' >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_eq "slot = menu without menu fails" "$rc" "1"
assert_contains "and the hint names the menus page" "$out" "Help: k2 zen guide menus"

echo "== validate hints point at the guide (FC35) =="
printf 'schema = 1\n[[widget]]\nid = "a"\nkind = "agents"\nslot = "top"\n' >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_eq "agents in the top band fails" "$rc" "1"
assert_contains "a slot error points at bands" "$out" "Help: k2 zen guide bands"
printf 'schema = 1\n[[widget]]\nid = "a"\nkind = "agents"\ncolumn = 0\n[widget.props]\nhom = "Work"\n' >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_contains "a prop error points at widgets" "$out" "Help: k2 zen guide widgets"
printf 'schema = 1\n[colors.light]\nacent = "#fff"\n' >"$SCRATCH_FILE"
capture zen validate --garden "$SCRATCH"
assert_contains "a theme error points at themes" "$out" "Help: k2 zen guide themes"
capture zen validate --garden "$SCRATCH" --json
case "$out" in *"Help:"*) bad "--json must stay pure JSON: $out" ;; *) ok "--json carries no hint line" ;; esac
capture zen reset --garden "$SCRATCH"
assert_eq "reset the scratch Garden" "$rc" "0"
capture zen validate
assert_eq "everything validates after the reset" "$rc" "0"

echo "== safe-mode lines are the app's own (FC-T18) =="
SAFE_PAGE="$(nd zen guide safe-mode)"
# The app's sentences, with typographic apostrophes made plain.
app_text="$(sed "s/’/'/g" "$ZEN_VIEW")"
lines="$(printf '%s\n' "$SAFE_PAGE" | sed -n '/^THE LINES$/,/^THE WAYS OUT/p' | grep -E '^  [A-Z]' | grep -v '^  When ')"
assert_eq "safe-mode quotes nine lines" "$(line_count "$lines")" "9"
while IFS= read -r l; do
    l="${l#  }"
    l="${l%: <message>}"
    case "$l" in
        "The Zen toggle "*|"The Garden switcher "*)
            who="${l% isn*}"
            problem="isn${l#* isn}"
            problem="${problem%.}"
            assert_contains "safe-mode control name '$who' is the app's" "$app_text" "'$who'"
            assert_contains "safe-mode problem '$problem' is the app's" "$app_text" "'$problem'"
            ;;
        *)
            assert_contains "safe-mode line '$l' is the app's" "$app_text" "$l"
            ;;
    esac
done <<< "$lines"
assert_contains "safe-mode quotes the banner title" "$app_text" "$(printf '%s\n' "$SAFE_PAGE" | sed -n '/The banner says "/,/"/p' | tr '\n' ' ' | sed -e 's/.*The banner says "//' -e 's/",.*//' -e 's/  */ /g')"
for way in "Ctrl+Cmd+Z on macOS, Ctrl+Alt+Z on Linux." "View > Exit Zen Mode" "Hold Shift while turning Zen on" "k2 zen reset --garden <id>"; do
    assert_contains "safe-mode lists the way out: $way" "$SAFE_PAGE" "$way"
done

echo "== the daemon stopped: the guide still works (FC-T16) =="
stop_daemon
set +e
out="$(env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK HOME="$SANDBOX" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" "$K2_CLI" zen guide required 2>&1)"
rc=$?
set -e
assert_eq "guide with a stopped daemon exits 0" "$rc" "0"
assert_eq "and prints the same page" "$out" "$REQUIRED_PAGE"
set +e
out="$(env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK HOME="$SANDBOX" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="$TOKEN" "$K2_CLI" zen guide example bottom-bar --toml 2>&1)"
rc=$?
set -e
assert_eq "an example with a stopped daemon exits 0" "$rc" "0"
assert_eq "and prints the same file" "$out" "$(nd zen guide example bottom-bar --toml)"

echo
echo "zen_guide: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
