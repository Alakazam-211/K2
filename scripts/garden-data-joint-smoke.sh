#!/usr/bin/env bash
# Garden data joint smoke (contract-garden-data-v1 §12.4): Baden's Board flow
# end to end, run by the integrator on a dev build of the integration branch
# (and steps 3, 5, 7 again on the signed build).
#
# The "agents" are FAKES: K2 shell tabs (agent rules, GL42) in three
# workspaces — board-builder, board-keeper and board-stranger. Never a real
# model CLI. Each step says which terminal it needs:
#   person    a terminal OUTSIDE K2 (the owner token; no K2_HOOK_TOKEN)
#   builder   a K2 shell tab in board-builder
#   keeper    a K2 shell tab in board-keeper
#   stranger  a K2 shell tab in board-stranger (no grants)
#
#   scripts/garden-data-joint-smoke.sh list        the steps, in order
#   scripts/garden-data-joint-smoke.sh <step>      run one (1, 2, 3, 3b, …)
#
# Use a temp HOME for the daemon under test (never the real ~/.k2). Steps
# share state in $K2_GD_SMOKE_DIR (default /tmp/k2-gd-smoke). The script
# checks every CLI answer it can (exit codes, error codes, output); what
# only the screen shows is a y/N question. Any FAIL exits 1.

set -uo pipefail

STATE="${K2_GD_SMOKE_DIR:-/tmp/k2-gd-smoke}"
mkdir -p "$STATE"
W=board-lite
PFX="project/7/board/1/"
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
run() {
    OUT="$("$@" 2>&1)"
    RC=$?
}
expect_rc() { if [ "$RC" = "$2" ]; then ok "$1 (exit $RC)"; else bad "$1 (exit $RC, want $2): $OUT"; fi; }
expect_has() { if grep -Fq -- "$2" <<<"$OUT"; then ok "$1"; else bad "$1 (missing $(printf %q "$2") in: $OUT)"; fi; }
confirm() {
    local a=""
    printf '  ? %s [y/N] ' "$1"
    read -r a
    case "$a" in y|Y|yes) ok "$1" ;; *) bad "$1" ;; esac
}
pause() {
    printf '  > %s (Enter when done) ' "$1"
    read -r _
}
need() {
    case "$1" in
        person)
            if [ -n "${K2_HOOK_TOKEN:-}${K2SO_HOOK_TOKEN:-}" ]; then
                echo "This step is the PERSON's: run it in a terminal outside K2." >&2
                exit 2
            fi ;;
        *)
            if [ -z "${K2_HOOK_TOKEN:-}" ]; then
                echo "This step runs in $1's K2 shell tab (board-$1)." >&2
                exit 2
            fi
            echo "  (this terminal: $(k2 whoami 2>/dev/null | head -1); want board-$1)" ;;
    esac
}
save() { printf '%s' "$2" > "$STATE/$1"; }
load() { cat "$STATE/$1" 2>/dev/null; }
rev_of() {
    k2 store get --widget "$W" "$1" --json 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
e = d.get("entry", d) if isinstance(d, dict) else None
print(e["rev"] if e else 0)'
}
widget_dir() { printf '%s/.k2/zen/widgets/%s' "$HOME" "$W"; }

steps() {
    cat <<'EOF'
1   person    make board-lite, edit it unsigned, place it: author_unknown; Settings offers It's mine
2   builder   edit + k2 zen validate --widget --sign: "Signed as board-builder"; widget gets not_granted
3   person    k2 zen data admin board-lite --agent board-builder --level write; add a note
3b  builder   the note shows in k2 store list --widget board-lite
4   builder   grant board-keeper write on project/7/; self_grant refused (exit 3)
4b  stranger  an ungranted agent's grant → not_granted (exit 3)
5   keeper    changes --follow sees a drag; --if-rev put shows live; a stale --if-rev exits 5
6   keeper    outside project/7/ → not_granted; grants lists project/7/ write (agent)
7   builder   a 300-op template: one batch, one rev, all at once in the widget
8   person    putChunked/getChunked 150 KB (B2's SDK; widget console)
9   person    project q4 read grant prints the convenience note; add board-stranger
9b  stranger  reads on its next call
9c  person    remove board-stranger from q4
9d  stranger  not_granted again
10  person    lock the Garden: the widget still saves; board-keeper still writes (10b)
10b keeper    a write while the Garden is locked
11  person    unadmin board-builder: "goes with it"; keeper's changes --wait → not_granted; widget not_granted
12  person    a sources:read + store:write widget: store:write dropped; save.set works
13  person    export --at today after a forced snapshot; Garden delete keeps data; orphan + move back
EOF
}

step_1() {
    need person
    run k2 zen guide example "$W"
    expect_rc "the guide page" 0
    run k2 zen widget new "$W" --from "$W"
    if [ "$RC" = 0 ]; then ok "widget new"; else expect_has "widget new (already there)" "widget_exists"; fi
    printf '\n// smoke: an unsigned edit\n' >> "$(widget_dir)/board-lite.js"
    local g=""
    printf '  Garden id to place it in (k2 zen garden list): '
    read -r g
    [ -n "$g" ] || { bad "a Garden id"; return; }
    save garden "$g"
    k2 zen guide example "$W" --toml | sed "s|prefix = \"\"|prefix = \"$PFX\"|" > "$HOME/.k2/zen/gardens/$g.toml"
    run k2 zen validate --garden "$g"
    expect_rc "the Garden validates" 0
    run k2 zen validate --widget "$W" --no-sign
    expect_rc "the edited widget validates (not signed)" 0
    confirm "In Zen, that Garden's board says nobody signed this version (author_unknown)"
    confirm "Settings → Gardens → Local Data: board-lite offers It's mine"
}

step_2() {
    need builder
    printf '\n// smoke: edited by board-builder\n' >> "$(widget_dir)/board-lite.js"
    run k2 zen validate --widget "$W" --sign
    expect_rc "validate --widget" 0
    expect_has "signed as board-builder" "Signed as board-builder"
    confirm "The board now says its maker has no grant (not_granted)"
    confirm "Settings offers 'Let board-builder use this data'"
}

step_3() {
    need person
    run k2 zen data admin "$W" --agent board-builder --level write
    expect_rc "admin board-builder" 0
    pause "The board loads now: add one note with + Note"
}

step_3b() {
    need builder
    run k2 store list --widget "$W" --all --json
    expect_rc "list" 0
    expect_has "the person's note is listed" "${PFX}obj/"
}

step_4() {
    need builder
    run k2 store grant --widget "$W" --agent board-keeper --level write --prefix project/7/
    expect_rc "grant board-keeper write on project/7/" 0
    run k2 store grant --widget "$W" --agent board-builder --level write
    expect_rc "a self-grant" 3
    expect_has "self_grant" "self_grant"
}

step_4b() {
    need stranger
    run k2 store grant --widget "$W" --agent board-keeper --level read
    expect_rc "an ungranted agent's grant" 3
    expect_has "not_granted" "not_granted"
}

step_5() {
    need keeper
    local feed="$STATE/feed.jsonl" pid key="${PFX}obj/9" rev old
    : > "$feed"
    k2 store changes --widget "$W" --since 0 --follow > "$feed" 2>&1 &
    pid=$!
    sleep 2
    local before
    before="$(wc -l < "$feed" | tr -d ' ')"
    pause "Drag a note in the board now"
    sleep 1
    if [ "$(wc -l < "$feed" | tr -d ' ')" -gt "$before" ]; then ok "the drag reached changes --follow"; else bad "no line after the drag"; fi
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    run k2 store put --widget "$W" "$key" --value '{"id":"9","text":"from board-keeper","x":40,"y":40}' --if-rev 0
    if [ "$RC" != 0 ]; then expect_rc "create obj/9 (or it exists)" 5; fi
    rev="$(rev_of "$key")"
    run k2 store put --widget "$W" "$key" --value '{"id":"9","text":"from board-keeper","x":220,"y":60}' --if-rev "$rev"
    expect_rc "--if-rev put" 0
    confirm "The note moved in the board with no reload, labelled board-keeper"
    old="$rev"
    run k2 store put --widget "$W" "$key" --value '{"id":"9","text":"stale"}' --if-rev "$old"
    expect_rc "a stale --if-rev" 5
}

step_6() {
    need keeper
    run k2 store put --widget "$W" project/8/obj/1 --value '1'
    expect_rc "outside project/7/" 3
    expect_has "not_granted" "not_granted"
    run k2 store grants --widget "$W"
    expect_rc "grants" 0
    expect_has "project/7/" "project/7/"
    expect_has "write" "write"
}

step_7() {
    need builder
    python3 -c '
import json
print(json.dumps([{"op": "put", "key": "'"$PFX"'obj/t%03d" % i, "value": {"id": "t%03d" % i, "text": "template %d" % i, "x": 10 + (i % 20) * 30, "y": 10 + (i // 20) * 24}} for i in range(300)]))' > "$STATE/template-300-ops.json"
    run k2 store batch --widget "$W" --json < "$STATE/template-300-ops.json"
    expect_rc "a 300-op batch" 0
    expect_has "one rev" '"rev"'
    confirm "The board shows all 300 template notes at once (one change, not 300)"
}

step_8() {
    need person
    echo "  putChunked/getChunked are SDK helpers (B2): from the widget's frame, run"
    echo "    await k2.store.putChunked('img/1', 'x'.repeat(150000)); (await k2.store.getChunked('img/1')).length"
    confirm "getChunked answers 150000 and a broken part is reported (zen-custom-bridge.test.ts covers the rest)"
}

step_9() {
    need person
    run k2 project create q4
    if [ "$RC" != 0 ]; then expect_has "project q4 (exists)" "exists"; fi
    run k2 zen data grant "$W" --project q4 --level read
    expect_rc "project read grant" 0
    expect_has "the convenience note" "convenience"
    run k2 project add q4 board-stranger
    expect_rc "add board-stranger to q4" 0
}

step_9b() {
    need stranger
    run k2 store list --widget "$W" --json
    expect_rc "a project member reads" 0
}

step_9c() {
    need person
    run k2 project remove q4 board-stranger
    expect_rc "remove board-stranger from q4" 0
}

step_9d() {
    need stranger
    run k2 store get --widget "$W" "${PFX}obj/9"
    expect_rc "removed: next call refused" 3
    expect_has "not_granted" "not_granted"
}

step_10() {
    need person
    local g
    g="$(load garden)"
    run k2 zen garden lock "$g"
    expect_rc "lock the Garden" 0
    pause "Move a note in the locked Garden's board"
    confirm "The move saved (reload the Garden: the note stays where you put it)"
    echo "  Now run step 10b in board-keeper's tab, then unlock: k2 zen garden unlock $g"
}

step_10b() {
    need keeper
    local key="${PFX}obj/9"
    run k2 store put --widget "$W" "$key" --value '{"id":"9","text":"written while locked","x":60,"y":160}' --if-rev "$(rev_of "$key")"
    expect_rc "a keeper write in a locked Garden" 0
}

step_11() {
    need person
    OUT="$(printf 'n\n' | k2 zen data unadmin "$W" --agent board-builder 2>&1)"
    RC=$?
    expect_has "the cascade is named (dry run, answered no)" "it made go with it"
    expect_rc "no changes on 'n'" 1
    run k2 zen data unadmin "$W" --agent board-builder --yes
    expect_rc "unadmin board-builder" 0
    echo "  In board-keeper's tab: k2 store changes --widget $W --since 0 --wait 30 → not_granted (exit 3)"
    confirm "board-keeper's next changes --wait answered not_granted"
    confirm "The board (board-builder-signed) shows not_granted until It's mine"
}

step_12() {
    need person
    local d="$HOME/.k2/zen/widgets/gd-taint"
    run k2 zen widget new gd-taint --from "$W"
    if [ "$RC" != 0 ]; then expect_has "gd-taint (exists)" "widget_exists"; fi
    python3 -c '
import json, sys
p = sys.argv[1]
m = json.load(open(p))
m["caps"] = ["sources:read", "store:read", "store:write"]
m["reasons"]["sources:read"] = "smoke"
m["sources"] = "config"
json.dump(m, open(p, "w"), indent=2)' "$d/manifest.json"
    run k2 zen widget show gd-taint
    expect_has "store:write dropped" "store:write"
    confirm "Placed, gd-taint's store.put answers cap_not_granted and save.set works"
}

step_13() {
    need person
    local today g
    today="$(date +%Y-%m-%d)"
    g="$(load garden)"
    echo "  The daemon under test must be a DEBUG build (cargo build -p k2-daemon; the hook is"
    echo "  cfg!(debug_assertions) only, and scripts/dev-build.sh builds release) started with"
    echo "  K2_GARDEN_DATA_SNAPSHOT_NOW=1. On a release build, wait for the next day's snapshot."
    run k2 zen data export "$W"
    expect_rc "live export" 0
    printf '%s\n' "$OUT" | tail -n +2 | python3 -c 'import json,sys; print("\n".join(sorted(json.loads(l)["key"] for l in sys.stdin if l.strip())))' > "$STATE/live.keys"
    run k2 store export --widget "$W" --at "$today"
    expect_rc "export --at today" 0
    printf '%s\n' "$OUT" | tail -n +2 | python3 -c 'import json,sys; print("\n".join(sorted(json.loads(l)["key"] for l in sys.stdin if l.strip())))' > "$STATE/at.keys"
    if cmp -s "$STATE/live.keys" "$STATE/at.keys"; then ok "--at today matches the live keys"; else bad "--at today differs from live"; fi
    run k2 zen garden delete "$g"
    expect_rc "delete the Garden" 0
    run k2 zen data show "$W" --json
    expect_rc "the data is still there" 0
    mv "$(widget_dir)" "$HOME/.k2/zen/widgets/board-lite-old"
    pause "Open Settings → Gardens (a refresh marks orphans)"
    confirm "Settings lists board-lite under Data with no widget"
    mv "$HOME/.k2/zen/widgets/board-lite-old" "$HOME/.k2/zen/widgets/board-lite-new"
    run k2 zen data move "$W" board-lite-new
    expect_rc "move the data to the renamed folder" 0
    confirm "Settings shows board-lite-new's data line (no orphan)"
}

case "${1:-list}" in
    list|"") steps; exit 0 ;;
    1|2|3|3b|4|4b|5|6|7|8|9|9b|9c|9d|10|10b|11|12|13)
        echo "== step $1 =="
        "step_$1"
        ;;
    *) echo "unknown step '$1'" >&2; steps >&2; exit 2 ;;
esac
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
