# ── Garden data in the guide (contract-garden-data-v1 §11.4, B3) ─────
_zen_guide_page_services() {
    cat <<'EOF'
k2 zen guide services — a widget's data (k2.store), on this computer only

Every service here is LOCAL: the data lives in a file K2 keeps on this
computer, never on a server, and no Connect login reaches it.

  k2.store.*  data:<widget>   shared with agents you grant; pushed live
  k2.save.*   save:<widget>   the widget's private saves; never shared. A phone
                              keeps its own copy (k2 zen guide phone)
  k2.viewer() {kind, name}    who is looking (for labels)

KEYS, REVS, CONFLICTS
  One key per object (obj/<id>, or project/<pid>/board/<bid>/obj/<id>);
  a value is JSON ≤ 60 KiB. Never inline data into the widget's code.
  Every write moves the space's rev. put(key, v, {ifRev: n}) writes only
  if the key is still at rev n (0 = only if missing); else `conflict`:
  read it again and re-apply just the fields you changed.
    const r = await k2.store.put('obj/7', note, {ifRev: seen.rev})
  patch(key, partial, {ifRev}) changes only the fields it names and keeps
  the rest (null removes one; an object merges into an object field):
    const {rev, value} = await k2.store.patch('obj/7', {done: true})

LIVE
  k2.store.subscribe(prefix, cb, onError) calls cb with
  [{key, rev, value | null, by}] for anyone's change (another window, an
  agent's `k2 store put --widget`), never half a batch. The first call is
  the current state under the prefix, however long ago it last changed.
  After a real gap K2 lists again by itself and hands {resync: true,
  items}: redraw from items. `by` is "you", "you via <w>" or
  "agent:<handle>".

BIG CHANGES
  store.batch(ops) ≤ 64 ops, one rev. k2.store.tx(): add up to 1,000 ops /
  1 MiB, then commit: all or nothing, one rev, one push.
  putChunked/getChunked carry text up to 1 MiB (images as data URLs).

WHO MAY TOUCH IT
  A version signed by you, K2 or an agent on the Garden's list runs with
  the Garden's data; any other keeps only its own keyspace (needs_editor).
  `k2 zen validate --widget <w> --sign` (or k2 zen widget sign <w>) signs a
  clean version as whoever runs it; a plain validate never signs. An agent
  on the list of every Garden that places the widget reaches its data; any
  other needs a grant from the person (Settings → Gardens → the Garden →
  Local Data, or k2 zen data grant): per agent (a wall), per K2 project
  (a convenience: any agent can join a project) or per key prefix. A
  grant admin grants inside its prefix.

LOCK AND SOURCES
  Lock freezes how a Garden works, not what's in it: data still changes.
  A widget asking sources:read can't write data (store:write dropped).

LIMITS
  20 MiB / 100,000 keys a widget (saves 5 MiB); 60 writes/s.

EXPORT, HISTORY, ORPHANS
  k2 store export --widget <w> [--at YYYY-MM-DD] > data.jsonl (daily
  snapshots: 7 days + 4 weeks). Deleting a Garden keeps the data. A
  renamed folder leaves "Data with no widget" in Settings until
  k2 zen data move <old> <new> or a delete.

TRY IT
  k2 zen guide example board-lite
EOF
}

_ZEN_GUIDE_WIDGET_EXAMPLES="board-lite"

_zen_widget_example_page() {
    cat <<'EOF'
k2 zen guide example board-lite — sticky notes in a widget's data

A sticky-note board kept in k2.store, in the format Board uses: one key per
note (<prefix>obj/<id> = {id, text, x, y, color, batch}), field-level
writes with ifRev (a conflict re-reads and re-applies only the changed
fields), a batch id per gesture, and subscribe for live changes.

1. Make the widget (from a terminal outside K2, it is signed as yours):
     k2 zen widget new board-lite --from board-lite
     k2 zen validate --widget board-lite --sign
2. Place it in a Garden (k2 zen garden list for the id):
     k2 zen guide example board-lite --toml > ~/.k2/zen/gardens/<id>.toml
     k2 zen validate --garden <id>
   Or paste only its [[widget]] table into your Garden's file.
   config.prefix (optional) puts the notes under a key prefix, e.g.
   "project/7/board/1/", so one prefix grant covers that board.
3. Let an agent help (the person, outside K2):
     k2 zen data grant board-lite --agent <handle> --level write
4. The agent's side: a script that checks the shape before it writes.
     k2 zen guide example board-lite --script > bin/board-lite
     chmod +x bin/board-lite
     bin/board-lite add "Ship it"          # a new note
     bin/board-lite move <id> 200 40       # field-level, retries a conflict
     bin/board-lite watch 0                # k2 store changes --follow
   The widget shows each change at once, labelled with the agent's handle.
5. Board + K2 chat: K2's own Agents and Conversation beside the board (its
   files, cards, unread counts and pushes), never a chat panel of yours.
   Add "agents:read" and "thread:post" to the widget's manifest caps, then
   in board-lite.js:
     const rows = await k2.agents.list()      // full addresses, handle::host
     await k2.agents.show(rows.filter(onBoard).map((r) => r.address))
     await k2.conversation.setContext('Board: ' + name)  // a chip they see
   and give the Garden two more columns the person can fold:
     [[layout.column]]                [[widget]]
     size = 55                        id = "crew"
     [[layout.column]]                kind = "agents"
     size = 15                        column = 1
     label = "Agents"                 [widget.props]
     collapsible = true               list-from = "board"
     [[layout.column]]                [[widget]]
     size = 30                        id = "talk"
     label = "Chat"                   kind = "conversation"
     collapsible = true               column = 2
                                      [widget.props]
                                      agents = "crew"
   The Agents column shows just the agents the board lists ("Nothing
   listed yet" until it does); k2 zen guide api has k2.layout.*.

If the widget says it needs an editor (needs_editor): k2 zen widget sign
board-lite, by you or an agent on the Garden's list. Data and grants:
k2 zen guide services.
EOF
}

_zen_widget_example_toml() {
    cat <<'EOF'
schema = 1

[layout]
kind = "columns"
[[layout.column]]
size = 100
min-width = 240

[[widget]]
id = "board"
kind = "custom"
widget = "board-lite"
column = 0
[widget.props]
config = { prefix = "" }
EOF
}

_zen_widget_example_script() {
    cat <<'EOF'
#!/usr/bin/env bash
# bin/board-lite: an agent's way to change Board Lite's notes. It checks the
# shape first (a raw `k2 store put` skips that), writes field by field with
# --if-rev, and retries a conflict (exit 5) by re-reading the note.
#   board-lite add <text> [--color yellow|blue|green|pink]
#   board-lite move <id> <x> <y>    board-lite text <id> <text>
#   board-lite rm <id>              board-lite list    board-lite watch [rev]
# BOARD_LITE_WIDGET (default board-lite), BOARD_LITE_PREFIX (e.g. project/7/board/1/).
set -euo pipefail
W="${BOARD_LITE_WIDGET:-board-lite}"
P="${BOARD_LITE_PREFIX:-}"
key() { printf '%sobj/%s' "$P" "$1"; }
die() { echo "board-lite: $*" >&2; exit 2; }

# change <id> <python that edits dict v>: read, edit, put --if-rev; 3 tries.
change() {
    local id="$1" edit="$2" try out rev val rc
    for try in 1 2 3; do
        out="$(k2 store get --widget "$W" "$(key "$id")" --json)"
        rev="$(printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); e=d.get("entry",d) if isinstance(d,dict) else None; print(e["rev"] if e else "")')"
        [ -n "$rev" ] || die "no note $id"
        val="$(printf '%s' "$out" | BL_EDIT="$edit" python3 -c '
import json, os, sys, uuid
d = json.load(sys.stdin); e = d.get("entry", d)
v = dict(e["value"]) if isinstance(e.get("value"), dict) else {}
exec(os.environ["BL_EDIT"])
v["batch"] = uuid.uuid4().hex[:12]
print(json.dumps(v))')"
        set +e
        k2 store put --widget "$W" "$(key "$id")" --value "$val" --if-rev "$rev" >/dev/null
        rc=$?
        set -e
        [ "$rc" = 5 ] || return "$rc"
    done
    die "note $id keeps changing; try again"
}

cmd="${1:-}"; shift || true
case "$cmd" in
    add)
        text="${1:-}"; [ -n "$text" ] || die "add <text>"; shift
        color="yellow"; [ "${1:-}" = "--color" ] && color="${2:-yellow}"
        case "$color" in yellow|blue|green|pink) ;; *) die "color is yellow, blue, green or pink" ;; esac
        id="$(python3 -c 'import uuid; print(uuid.uuid4().hex[:10])')"
        val="$(BL_T="$text" BL_C="$color" BL_ID="$id" python3 -c '
import json, os, uuid
t = os.environ["BL_T"][:2000]
print(json.dumps({"id": os.environ["BL_ID"], "text": t, "x": 20, "y": 20, "color": os.environ["BL_C"], "batch": uuid.uuid4().hex[:12]}))')"
        k2 store put --widget "$W" "$(key "$id")" --value "$val" --if-rev 0 >/dev/null
        echo "$id" ;;
    move)
        [ $# -eq 3 ] || die "move <id> <x> <y>"
        [[ "$2$3" =~ ^[0-9]+$ ]] || die "x and y are whole numbers"
        change "$1" "v['x'] = $2; v['y'] = $3" ;;
    text)
        [ $# -eq 2 ] || die "text <id> <text>"
        BL_T="$2" change "$1" "v['text'] = os.environ['BL_T'][:2000]" ;;
    rm)
        [ $# -eq 1 ] || die "rm <id>"
        k2 store rm --widget "$W" "$(key "$1")" ;;
    list) k2 store list --widget "$W" --prefix "${P}obj/" --all --json ;;
    watch) k2 store changes --widget "$W" --since "${1:-0}" --prefix "${P}obj/" --follow ;;
    *) die "add | move | text | rm | list | watch" ;;
esac
EOF
}

_zen_guide_page() {
    case "$1" in
        index) _zen_guide_index ;;
        services) _zen_guide_page_services ;;
        gardens) _zen_guide_page_gardens ;;
        files) _zen_guide_page_files ;;
        widgets) _zen_guide_page_widgets ;;
        bands) _zen_guide_page_bands ;;
        required) _zen_guide_page_required ;;
        menus) _zen_guide_page_menus ;;
        phone) _zen_guide_page_phone ;;
        themes) _zen_guide_page_themes ;;
        sync) _zen_guide_page_sync ;;
        examples) _zen_guide_page_examples ;;
        undo) _zen_guide_page_undo ;;
        safe-mode) _zen_guide_page_safe_mode ;;
        api) _zen_guide_page_api ;;
        libs) _zen_guide_page_libs ;;
        *) return 1 ;;
    esac
}

# The examples, in Cortana's order (FC65). Each is a whole Garden file.
_zen_example_toml() {
    case "$1" in
        rail-top)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# The nav rail as a row of icons in the top band, right of the Garden
# switcher. The rail is a CONTENT widget: it replaces the template's
# content (so agents and conversation are listed too), but NOT its Zen
# controls. This does NOT remove the switcher: the template's top band
# (switcher, usage, theme, Zen toggle) stays.

[[widget]]
kind = "nav-rail"
slot = "top"

[[widget]]
id = "agents"
kind = "agents"
column = 0
[widget.props]
home-picker = true

[[widget]]
id = "conversation"
kind = "conversation"
column = 1
[widget.props]
agents = "agents"
EOF
            ;;
        quiet-top)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# The template's top band without the usage chip. Placing any Zen control
# replaces all of the template's, so the switcher, theme and toggle are
# listed. usage isn't, so it isn't shown.

[[widget]]
kind = "garden-switcher"
slot = "top"

[[widget]]
kind = "theme-picker"
slot = "top"
align = "end"

[[widget]]
kind = "zen-toggle"
slot = "top"
align = "end"        # the last "end" item sits in the top-right corner
EOF
            ;;
        bottom-bar)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# The controls move to a bottom band: the Garden switcher bottom left, the
# theme and the Zen toggle bottom right. usage isn't placed, so it isn't
# shown. No top band: K2 keeps a thin empty strip to drag the window.

[[widget]]
kind = "garden-switcher"
slot = "bottom"      # align defaults to "start": the left end

[[widget]]
kind = "theme-picker"
slot = "bottom"
align = "end"

[[widget]]
kind = "zen-toggle"
slot = "bottom"
align = "end"        # the last "end" item sits in the bottom-right corner
EOF
            ;;
        menu-both)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# One menu button, top right, holds both required controls and the theme.
# A control in a menu always needs both keys: slot = "menu" and
# menu = "<the menu's id>".

[[widget]]
id = "more"
kind = "menu"
slot = "top"
align = "end"
[widget.props]
icon = "dots"
label = "More"       # the tooltip and the screen-reader name

[[widget]]
kind = "garden-switcher"
slot = "menu"
menu = "more"

[[widget]]
kind = "theme-picker"
slot = "menu"
menu = "more"

[[widget]]
kind = "zen-toggle"
slot = "menu"
menu = "more"
EOF
            ;;
        texting-chrome)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# The template's Zen controls written out in full: the page looks like
# Garden 1. Start here, then move, drop or add controls. Keep a
# zen-toggle and a garden-switcher somewhere (k2 zen guide required).

[[widget]]
kind = "garden-switcher"
slot = "top"
align = "start"

[[widget]]
kind = "usage"
slot = "top"
align = "end"

[[widget]]
kind = "theme-picker"
slot = "top"
align = "end"

[[widget]]
kind = "zen-toggle"
slot = "top"
align = "end"        # last in the "end" group: the top-right corner
EOF
            ;;
        menu-bottom-right)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# A bottom band holding only one menu, in the bottom-right corner, with
# every control inside it. No top band: K2 keeps a thin empty strip to
# drag the window.

[[widget]]
id = "all"
kind = "menu"
slot = "bottom"
align = "end"
[widget.props]
icon = "zen"
label = "Zen"

[[widget]]
kind = "garden-switcher"
slot = "menu"
menu = "all"

[[widget]]
kind = "usage"
slot = "menu"
menu = "all"

[[widget]]
kind = "theme-picker"
slot = "menu"
menu = "all"

[[widget]]
kind = "zen-toggle"
slot = "menu"
menu = "all"
EOF
            ;;
        column-corner)
            cat <<'EOF'
schema = 1
template = "k2.texting@1"

# The Zen toggle at the bottom-right corner of the conversation (column 1).
# slot = "column" with edge = "bottom" is the bottom of ONE column, inside
# its box. It is not the page's bottom band: that would be slot = "bottom".
# Likewise slot = "top" is the page's top band, not the top of a column.

[[widget]]
kind = "garden-switcher"
slot = "top"

[[widget]]
kind = "theme-picker"
slot = "top"
align = "end"

[[widget]]
kind = "zen-toggle"
slot = "column"
column = 1
edge = "bottom"
align = "end"
EOF
            ;;
        *) return 1 ;;
    esac
}

# One example with its notes (the page `k2 zen guide example <name>`).
_zen_example_page() {
    local name="$1"
    echo "k2 zen guide example $name — $(_zen_guide_title "$name")"
    echo
    echo "Write it over a Garden's file, then validate:"
    echo "  k2 zen guide example $name --toml > ~/.k2/zen/gardens/<id>.toml"
    echo "  k2 zen validate --garden <id>"
    echo
    _zen_example_toml "$name"
}

# stdin = body; prints {"id","title","body"}, plus "toml" for an example.
_zen_guide_emit_json() {
    python3 -c '
import json, sys
body = sys.stdin.read()
if body.endswith("\n"):
    body = body[:-1]
out = {"id": sys.argv[1], "title": sys.argv[2], "body": body}
if len(sys.argv) > 3:
    out["toml"] = sys.argv[3] + "\n"
print(json.dumps(out, separators=(",", ":"), ensure_ascii=False))
' "$@"
}

cmd_zen_guide() {
    local as_json=0 toml=0 list=0 script=0 p1="" p2="" extra=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1 ;;
            --toml) toml=1 ;;
            --list) list=1 ;;
            --script) script=1 ;;
            -h|--help)
                _zen_guide_usage
                exit 0
                ;;
            --*)
                echo "Error: unknown flag '$1' (see k2 zen guide --help)" >&2
                exit 2
                ;;
            *)
                if [ -z "$p1" ]; then p1="$1"
                elif [ -z "$p2" ]; then p2="$1"
                else extra="$1"
                fi
                ;;
        esac
        shift
    done
    if [ -n "$extra" ]; then
        echo "Error: unexpected extra argument '$extra' (see k2 zen guide --help)" >&2
        exit 2
    fi
    case "$p1" in
        example|examples)
            if [ "$list" = 1 ]; then
                if [ -n "$p2" ] || [ "$toml" = 1 ]; then
                    echo "Error: --list takes no example name" >&2
                    exit 2
                fi
                if [ "$as_json" = 1 ]; then
                    local n
                    for n in $_ZEN_GUIDE_EXAMPLES; do
                        printf '%s\t%s\n' "$n" "$(_zen_guide_title "$n")"
                    done | python3 -c '
import json, sys
ex = [dict(zip(("id", "title"), l.rstrip("\n").split("\t", 1))) for l in sys.stdin]
print(json.dumps({"examples": ex}, separators=(",", ":")))
'
                else
                    printf '%s\n' $_ZEN_GUIDE_EXAMPLES
                fi
                exit 0
            fi
            if [ -z "$p2" ]; then
                if [ "$toml" = 1 ]; then
                    echo "Error: --toml needs an example name (k2 zen guide example --list)" >&2
                    exit 2
                fi
                if [ "$as_json" = 1 ]; then
                    _zen_guide_page examples | _zen_guide_emit_json examples "$(_zen_guide_title examples)"
                else
                    _zen_guide_page examples
                fi
                exit 0
            fi
            # A widget example (Garden data, contract-garden-data-v1 §11.4):
            # the page, --toml (a Garden file placing it), --script (the
            # agent's bin/ script). Not in --list (Garden files only).
            if _zen_guide_is_in "$p2" $_ZEN_GUIDE_WIDGET_EXAMPLES; then
                if [ "$toml" = 1 ] && [ "$script" = 1 ]; then
                    echo "Error: --toml or --script, not both" >&2
                    exit 2
                fi
                if [ "$as_json" = 1 ]; then
                    _zen_widget_example_page | python3 -c '
import json, sys
body = sys.stdin.read().rstrip("\n")
print(json.dumps({"id": sys.argv[1], "title": sys.argv[2], "body": body, "toml": sys.argv[3] + "\n", "script": sys.argv[4] + "\n"}, separators=(",", ":"), ensure_ascii=False))
' "$p2" "$(_zen_guide_title "$p2")" "$(_zen_widget_example_toml)" "$(_zen_widget_example_script)"
                elif [ "$toml" = 1 ]; then
                    _zen_widget_example_toml
                elif [ "$script" = 1 ]; then
                    _zen_widget_example_script
                else
                    _zen_widget_example_page
                fi
                exit 0
            fi
            if [ "$script" = 1 ]; then
                echo "Error: --script is for k2 zen guide example board-lite" >&2
                exit 2
            fi
            if ! _zen_guide_is_in "$p2" $_ZEN_GUIDE_EXAMPLES; then
                echo "Error: no example '$p2'. Examples: $_ZEN_GUIDE_EXAMPLES $_ZEN_GUIDE_WIDGET_EXAMPLES" >&2
                exit 2
            fi
            if [ "$as_json" = 1 ]; then
                _zen_example_page "$p2" | _zen_guide_emit_json "$p2" "$(_zen_guide_title "$p2")" "$(_zen_example_toml "$p2")"
            elif [ "$toml" = 1 ]; then
                _zen_example_toml "$p2"
            else
                _zen_example_page "$p2"
            fi
            exit 0
            ;;
    esac
    if [ "$toml" = 1 ] || [ "$list" = 1 ] || [ "$script" = 1 ]; then
        echo "Error: --toml, --list and --script are for k2 zen guide example" >&2
        exit 2
    fi
    if [ -n "$p2" ]; then
        echo "Error: unexpected extra argument '$p2' (see k2 zen guide --help)" >&2
        exit 2
    fi
    local topic="${p1:-index}"
    if [ "$topic" != "index" ] && ! _zen_guide_is_in "$topic" $_ZEN_GUIDE_TOPICS; then
        echo "Error: no guide topic '$topic'" >&2
        _zen_guide_usage >&2
        exit 2
    fi
    if [ "$as_json" = 1 ]; then
        if [ "$topic" = "index" ]; then
            local t topics=""
            for t in $_ZEN_GUIDE_TOPICS; do
                topics="$topics$t=$(_zen_guide_title "$t");"
            done
            _zen_guide_index | ZEN_GUIDE_TOPICS="$topics" python3 -c '
import json, os, sys
body = sys.stdin.read().rstrip("\n")
topics = [dict(zip(("id", "title"), p.split("=", 1))) for p in os.environ["ZEN_GUIDE_TOPICS"].split(";") if p]
print(json.dumps({"id": "index", "title": sys.argv[1], "body": body, "topics": topics}, separators=(",", ":"), ensure_ascii=False))
' "$(_zen_guide_title index)"
        else
            _zen_guide_page "$topic" | _zen_guide_emit_json "$topic" "$(_zen_guide_title "$topic")"
        fi
    else
        _zen_guide_page "$topic"
    fi
}

# Shared formatter: verb, as_json, raw daemon body.
_zen_print() {
    ZEN_VERB="$1" ZEN_JSON="$2" ZEN_RAW="$3" python3 -c '
import json, os, sys
verb = os.environ["ZEN_VERB"]
as_json = os.environ.get("ZEN_JSON") == "true"
raw = os.environ.get("ZEN_RAW") or ""
NOT_SET_UP = "Zen isn\x27t set up on this computer. Turn it on with the Zen toggle in the K2 app\x27s top bar."
if not raw.strip():
    print("Error: no response from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %s" % raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err == "zen_not_set_up" or (verb in ("path", "garden-list") and d.get("setUp") is False):
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    print(d.get("message") or NOT_SET_UP, file=sys.stderr)
    sys.exit(3)
if err in ("garden_access_needed", "garden_delete_person", "session_gone"):
    # Garden access (G19): stop and let the person answer K2 card.
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    if err == "garden_delete_person" and isinstance(d.get("card"), dict) and d["card"].get("card"):
        # G19: the delete ask is out (a garden.delete card on the Thread).
        print("Asked the person to delete %s" % (d.get("name") or d.get("garden")), file=sys.stderr)
    print(d.get("message") or "Ask the person: k2 zen garden access <garden> ask", file=sys.stderr)
    sys.exit(3)
if err == "garden_locked" or (err == "owner_only" and verb == "garden-lock"):
    # prd-garden-lock-and-sync-v1 §6: forbidden, stop and ask the person.
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    print(d.get("message") or "This Garden is locked. Ask the person to unlock it in Settings → Gardens.", file=sys.stderr)
    sys.exit(3)
if err is not None:
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    if err == "last_garden":
        print(d.get("message") or "That\x27s your last Garden.", file=sys.stderr)
    elif err == "garden_has_changes":
        print("Error: Garden %s has its own changes (%s). Add --force to start it over; the file is kept in .history/." % (d.get("garden"), ", ".join(d.get("keys") or [])), file=sys.stderr)
    elif err == "zen_local_only" and verb.startswith(("sync", "news")):
        print("Error: only the person on this computer can change Garden sync and news. Ask the person: Settings → Gardens. (Inside a K2 terminal this runs as that session; a person can use Settings or a terminal outside K2.)", file=sys.stderr)
    elif err == "zen_local_only":
        print("Error: k2 zen talks only to the K2 on this computer, and this token is not its owner.", file=sys.stderr)
    elif isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("message") or err), file=sys.stderr)
    else:
        print("Error: %s" % (d.get("message") or err), file=sys.stderr)
    sys.exit(1)
def stale_runtime(d):
    """Old runtime errors validate didn\x27t count (0.46.3): an unplaced
    widget\x27s, or older than 24 h."""
    rt = d.get("runtime") if isinstance(d.get("runtime"), dict) else {}
    return int(d.get("staleRuntime") or 0) + int(rt.get("staleErrors") or 0)
if verb == "validate" and isinstance(d.get("locked"), dict):
    # §6: validating a locked Garden exits 3 with the message, so an agent
    # sees the lock even after a raw file edit K2 put back.
    if as_json:
        print(json.dumps(d, indent=2))
    print(d["locked"].get("message") or "This Garden is locked. Ask the person to unlock it in Settings → Gardens.", file=sys.stderr)
    sys.exit(3)
if as_json:
    print(json.dumps(d, indent=2))
    if verb == "validate" and d.get("ok") is False:
        sys.exit(1)
    # Loopable (decision-k2-runtime-v1 stage 0 (e)): "pending" = saved, not
    # run yet. 0.46.1: that is not a failure (exit 0); --strict keeps 2.
    if verb == "validate" and d.get("status") == "pending" and os.environ.get("ZEN_STRICT") == "true":
        sys.exit(2)
    # 0.46.3: old runtime errors are warnings; --strict treats them like
    # pending (not confirmed clean).
    if verb == "validate" and os.environ.get("ZEN_STRICT") == "true" and stale_runtime(d):
        sys.exit(2)
    sys.exit(0)

def diag(e, warn=False):
    s = "%s:%s:%s: %s%s" % (e.get("file"), e.get("line"), e.get("col"), "warning: " if warn else "", e.get("message"))
    if e.get("suggestion"):
        s += "\n  suggestion: %s" % e["suggestion"]
    return s

SYNC = {}
try:
    for row in (json.loads(os.environ.get("ZEN_SYNC_RAW") or "{}").get("gardens") or []):
        SYNC[row.get("id")] = row
except Exception:
    SYNC = {}

def sync_word(row):
    p = (row.get("page") or {}).get("mode")
    t = (row.get("theme") or {}).get("mode")
    if p == t:
        return "synced" if p == "synced" else "own copy"
    word = lambda m: "synced" if m == "synced" else "own copy"
    return "page: %s · theme: %s" % (word(p), word(t))

def garden_line(g):
    line = "%s %d  %s  %s  %s" % ("*" if g.get("theme") else " ", g.get("index"), g.get("id"), g.get("name"), g.get("template"))
    if g.get("id") in SYNC:
        line += "  " + sync_word(SYNC[g.get("id")])
    if g.get("locked"):
        line += "  locked (agents can\x27t change it)"
    return line

def part_line(name, part):
    mode = part.get("mode")
    if mode == "synced":
        return "%s synced" % name
    s = "%s own copy of %s" % (name, part.get("defaults"))
    if part.get("k2Version"):
        s += " (K2 %s)" % part["k2Version"]
    if part.get("newerDefault"):
        s += ", K2\x27s default has changed since"
    return s

def human(n):
    # K2\x27s limit words: 1 KB = 1,024 bytes ("740 KB", "1 MB", "3.5 MB").
    if n >= 1048576:
        t = (n * 10 + 524288) // 1048576
        return "%d MB" % (t // 10) if t % 10 == 0 else "%d.%d MB" % (t // 10, t % 10)
    if n >= 1024:
        return "%d KB" % ((n + 512) // 1024)
    return "%d bytes" % n

def size_line(s):
    # 0.46.5 WS10: one line after the findings.
    if not isinstance(s, dict) or not isinstance(s.get("code"), int):
        return None
    large = " (large)" if s["code"] > s.get("codeWarnAt", s["code"]) else ""
    libs = human(s.get("libs") or 0)
    if s.get("libsUnknown"):
        libs += " + CDN files"
    return "size: code %s of %s%s, page %s of %s, libraries %s (outside the limit)" % (
        human(s["code"]), human(s.get("codeMax") or 0), large, human(s.get("page") or 0), human(s.get("pageMax") or 0), libs)

if verb in ("validate", "reload"):
    errs = d.get("errors") or []
    for e in errs:
        print(diag(e))
    for w in d.get("warnings") or []:
        print(diag(w, True))
    if size_line(d.get("size")):
        print(size_line(d.get("size")))
    ty = d.get("types") if isinstance(d.get("types"), dict) else None
    if ty is not None:
        print("types: %s%s" % ("checked with %s" % ty["checker"] if ty.get("checker") else "not checked", "; %s" % ty["note"] if ty.get("note") else ""))
    if verb == "reload":
        print("reloaded: %s" % ("changed" if d.get("changed") else "no change"))
    if errs and any(str(f).startswith("staged") for f in (d.get("files") or [])):
        print("%d error(s) in the staged copy; nothing was written." % len(errs), file=sys.stderr)
        sys.exit(1)
    if errs:
        print("%d error(s). Zen is showing the last good version." % len(errs), file=sys.stderr)
        # Point at the guide page for each kind of error (FC35), unless the
        # message already names one.
        topics = []
        for e in errs:
            m = e.get("message") or ""
            f = e.get("file") or ""
            if "k2 zen guide" in m:
                continue
            if any(p in m for p in ("slot = \"menu\"", "a menu", "menu \x27", "menu = ", "menus", "menu.label")):
                t = "menus"
            elif any(w in m for w in ("slot", "band", "edge", "align", "column")):
                t = "bands"
            elif f.startswith("gardens/") and any(w in m for w in ("widget", "kind", "props")):
                t = "widgets"
            elif f.startswith("gardens/") and any(w in m for w in ("layout", "template", "schema")):
                t = "files"
            else:
                t = "themes"
            if t not in topics:
                topics.append(t)
        if topics:
            print("Help: %s" % ", ".join("k2 zen guide %s" % t for t in topics), file=sys.stderr)
        sys.exit(1)
    if verb == "validate":
        files = d.get("files") or []
        rt = d.get("runtime") if isinstance(d.get("runtime"), dict) else None
        # 0.46.3: with no target, one line per widget: where it is placed
        # and its saved version\x27s runtime state.
        for w in d.get("widgets") or []:
            where = ", ".join("%s/%s" % (p.get("gardenName") or p.get("garden"), p.get("placement")) for p in (w.get("placements") or [])) or "not placed"
            extra = ""
            if w.get("stale"):
                extra = "; %s old runtime error(s), not counted" % w["stale"]
            print("widget %s: %s (%s)%s" % (w.get("widget"), w.get("status"), where, extra))
        if rt and rt.get("placements"):
            print("placed: %s" % ", ".join("%s/%s" % (p.get("gardenName") or p.get("garden"), p.get("placement")) for p in rt["placements"]))
        if d.get("status") == "pending":
            print("pending: %s parses; %s" % (", ".join(files), rt.get("note") if rt else "it hasn\x27t run yet"))
            if os.environ.get("ZEN_STRICT") == "true":
                sys.exit(2)
            print("(exit 0: pending is not an error; --strict exits 2 until it has run)")
            sys.exit(0)
        print("ok: %s" % (", ".join(files) if files else "no files"))
        if rt and rt.get("note"):
            print("runtime: %s" % rt["note"])
        if stale_runtime(d):
            if os.environ.get("ZEN_STRICT") == "true":
                print("--strict: old runtime errors are logged; exit 2 until they are cleared (k2 zen widget errors <name> --clear)", file=sys.stderr)
                sys.exit(2)
            print("(exit 0: old runtime errors are warnings; --strict exits 2 while they are logged)")
elif verb == "reset":
    print("reset %s to %s" % (d.get("file"), d.get("restored")))
    if d.get("snapshot"):
        print("previous file kept as snapshot %s (k2 zen reset --to %s to undo)" % (d["snapshot"], d["snapshot"]))
elif verb == "history":
    for f in d.get("files") or []:
        snaps = f.get("snapshots") or []
        note = ""
        if f.get("deleted"):
            note = ", Garden deleted"
        elif not f.get("exists", True):
            note = ", file deleted"
        print("%s (%d snapshot%s%s)" % (f.get("file"), len(snaps), "" if len(snaps) == 1 else "s", note))
        for s in snaps:
            print("  %s  %s%s" % (s.get("name"), s.get("at"), "" if s.get("clean") else "  (has errors)"))
elif verb == "doctor":
    for c in d.get("checks") or []:
        print("%s %s: %s" % ("ok  " if c.get("ok") else "FAIL", c.get("name"), c.get("detail")))
    if not d.get("ok"):
        sys.exit(1)
elif verb == "path":
    print(d.get("path"))
elif verb == "garden-list":
    for g in d.get("gardens") or []:
        print(garden_line(g))
elif verb == "garden-new":
    g = d.get("garden") or {}
    print(g.get("id"))
    print(d.get("path"))
    if d.get("made") and d.get("message"):
        print(d["message"], file=sys.stderr)
elif verb == "garden-rename":
    g = d.get("garden") or {}
    print("Garden %s is now “%s”%s" % (g.get("id"), g.get("name"), "" if d.get("changed") else " (unchanged)"))
elif verb == "garden-reorder":
    for g in d.get("gardens") or []:
        print(garden_line(g))
elif verb == "garden-template":
    g = d.get("garden") or {}
    if d.get("changed"):
        print("Garden %s (“%s”) is now %s" % (g.get("id"), g.get("name"), d.get("template")))
        if d.get("snapshot"):
            print("its previous file is kept as snapshot %s (k2 zen reset --garden %s --to %s to undo)" % (d["snapshot"], g.get("id"), d["snapshot"]))
    else:
        print("Garden %s (“%s”) is already %s (unchanged)" % (g.get("id"), g.get("name"), d.get("template")))
elif verb == "garden-lock":
    row = d.get("garden") or {}
    state = "locked: agents can\x27t change it" if d.get("locked") else "unlocked: agents can change it again"
    print("%s (%s) is %s%s" % (row.get("name"), row.get("id"), state, "" if d.get("changed") else " (unchanged)"))
elif verb == "garden-delete":
    snap = d.get("snapshot")
    print("deleted Garden %s (“%s”)%s" % (d.get("deleted"), d.get("name"), "; its page is kept in .history as %s" % snap if snap else ""))
elif verb == "theme-list":
    for t in d.get("themes") or []:
        tags = []
        if t.get("builtin"):
            tags.append("built in")
        if t.get("user"):
            tags.append("your override" if t.get("builtin") else "yours")
        line = "%s %s  %s  (%s)" % ("*" if t.get("active") else " ", t.get("name"), t.get("label") or t.get("name"), ", ".join(tags))
        if t.get("summary"):
            line += "  " + t["summary"]
        print(line)
    if d.get("garden"):
        print("Garden %s: %s" % (d["garden"], ("its own pick" if d.get("gardenTheme") else "follows this computer")))
    if d.get("missing"):
        print("theme \x27%s\x27 doesn\x27t exist any more; showing %s" % (d["missing"], d.get("active")), file=sys.stderr)
elif verb == "theme-switch":
    if d.get("garden") and d.get("scope") == "garden":
        print("theme %s for Garden %s" % (d.get("theme"), d["garden"]))
    elif d.get("garden"):
        print("Garden %s follows this computer: theme %s" % (d["garden"], d.get("theme")))
    else:
        print("theme %s for this computer" % d.get("theme"))
elif verb == "sync-list":
    for row in d.get("gardens") or []:
        own = row.get("ownChanges")
        extra = []
        if own:
            extra.append("own changes: %s" % ", ".join(own))
        elif own is None:
            extra.append("its file doesn\x27t parse")
        if row.get("undo"):
            extra.append("undo: %s" % ", ".join(row["undo"].get("parts") or []))
        kp = row.get("keepPrevious") or {}
        if kp.get("page") or kp.get("theme"):
            extra.append("keep-previous offered")
        print("%d  %s  %s  %s; %s%s" % (row.get("index"), row.get("id"), row.get("name"), part_line("page", row.get("page") or {}), part_line("theme", row.get("theme") or {}), ("  [" + "; ".join(extra) + "]") if extra else ""))
    print("K2 defaults: %s (K2 %s)%s" % (d.get("liveDefaults"), d.get("k2Version"), ("; previous %s" % d["previousDefaults"]) if d.get("previousDefaults") else ""))
elif verb == "sync-change":
    row = d.get("garden") or {}
    if d.get("changed"):
        print("%s (%s): %s; %s" % (row.get("name"), row.get("id"), part_line("page", row.get("page") or {}), part_line("theme", row.get("theme") or {})))
        if row.get("undo"):
            print("undo with: k2 zen sync %s undo" % row.get("id"))
    else:
        print("%s (%s): unchanged (%s; %s)" % (row.get("name"), row.get("id"), part_line("page", row.get("page") or {}), part_line("theme", row.get("theme") or {})))
elif verb == "sync-preview":
    g = d.get("garden") or {}
    page = d.get("page") or {}
    theme = d.get("theme") or {}
    s = d.get("sync") or {}
    parts = {"page": "page", "theme": "theme", "both": "page and theme"}.get(s.get("preview"), s.get("preview"))
    print("preview of %s (%s) with K2\x27s default for its %s (nothing written)" % (g.get("name"), g.get("id"), parts))
    print("  template %s, layout %s, %d widget(s)" % (page.get("template"), (page.get("layout") or {}).get("split"), len(page.get("widgets") or [])))
    print("  theme %s, accent %s / %s" % (theme.get("name"), ((theme.get("tokens") or {}).get("colors") or {}).get("light", {}).get("accent"), ((theme.get("tokens") or {}).get("colors") or {}).get("dark", {}).get("accent")))
    print("  page now: %s; theme now: %s" % ((s.get("page") or {}).get("mode"), (s.get("theme") or {}).get("mode")))
elif verb == "news":
    items = d.get("items") or []
    if not items:
        print("no unseen Garden news")
    for it in items:
        if it.get("kind") == "notice":
            print("%s  %s. %s" % (it.get("id"), it.get("title"), it.get("body")))
            for line in it.get("detail") or []:
                print("    %s" % line)
        elif it.get("kind") == "catalog":
            print("%s  New in the Garden catalog: %s. %s" % (it.get("id"), it.get("label"), it.get("description")))
        elif it.get("kind") == "update":
            print("%s  %s has a fresh look (%s)%s" % (it.get("id"), it.get("gardenName"), it.get("part"), "; keep-previous offered" if it.get("keepPrevious") else ""))
    n = d.get("copiesWithNewerDefault") or 0
    if n:
        print("%d own cop%s didn\x27t change; K2\x27s default is newer (k2 zen sync)" % (n, "y" if n == 1 else "ies"))
elif verb == "news-seen":
    seen = d.get("seen") or []
    print("marked %d item(s) seen%s" % (len(seen), (": " + ", ".join(seen)) if seen else ""))
elif verb == "theme-new":
    print("made %s from %s" % (d.get("path"), d.get("from")))
    if d.get("copiedImage"):
        print("copied its background %s" % d["copiedImage"])
    print("edit it, check with k2 zen validate, use it with k2 zen theme set %s" % d.get("name"))
elif verb == "widget-new":
    name = d.get("name")
    print(d.get("path"))
    files = d.get("files") or []
    if files:
        print("files: %s" % ", ".join(files))
    print("next: edit it, place it in a Garden with kind = \"custom\" and widget = \"%s\"," % name)
    print("check with k2 zen validate --widget %s; it talks to your agents as soon as it\x27s placed" % name)
elif verb == "widget-list":
    ws = d.get("widgets") or []
    if not ws:
        print("no custom widgets yet (k2 zen widget new <name> --from k2:diary)")
    for w in ws:
        caps = ", ".join(w.get("caps") or []) or "no caps"
        title = w.get("title") or w.get("name")
        print("%s  %s  (%s)  caps: %s" % (w.get("name"), w.get("state"), title, caps))
        for e in (w.get("errors") or [])[:3]:
            print("  " + diag(e))
        placements = w.get("placements") or []
        if not placements:
            print("  not placed in any Garden")
        for p in placements:
            paused = "posting paused (too many posts; the person clicks Resume on it)" if p.get("paused") else "running"
            print("  %s/%s  %s" % (p.get("garden"), p.get("placement"), paused))
'
}

# k2 zen widget <new|list> (prd-zen-user-widgets-v2 UW41). Custom widgets
# are folders under ~/.k2/zen/widgets/. They need no permissions (Rosson
# 2026-10-08): `grant` and `revoke` only say so.
# `k2 zen widget errors <name> [--since <seq>] [--follow] [--json]` — the
# runtime errors a widget hit in Zen windows (decision-k2-runtime-v1 stage 0
# (d)). Each has a suggestion; `current` marks errors of the version on disk.
cmd_zen_widget_errors() {
    local name="" since="" follow=false as_json=false clear=false all=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --since) since="${2-}"; [ $# -ge 2 ] && shift 2 || shift ;;
            --since=*) since="${1#*=}"; shift ;;
            --follow|-f) follow=true; shift ;;
            --clear) clear=true; shift ;;
            --all) all=true; shift ;;
            --json) as_json=true; shift ;;
            -h|--help)
                echo "Usage: k2 zen widget errors <name> [--since <seq>] [--follow] [--json]"
                echo "       k2 zen widget errors <name> --clear [--all] [--json]"
                echo "Runtime errors your widget hit in a Zen window, oldest first, each with a suggestion."
                echo "--since <seq> shows only newer ones; --follow keeps printing new ones (Ctrl+C stops)."
                echo "Validate counts only a placed widget's errors on its saved version from the last 24 h;"
                echo "the rest are marked old. --clear drops those (--all: every error). Ready marks stay."
                echo "Exit 0 none counted, 1 some (or the widget is unknown)."
                exit 0 ;;
            -*) echo "Error: unexpected '$1' (k2 zen widget errors --help)" >&2; exit 2 ;;
            *) [ -z "$name" ] && name="$1" || { echo "Error: one widget name" >&2; exit 2; }; shift ;;
        esac
    done
    [ -n "$name" ] || { echo "Error: k2 zen widget errors needs a widget name" >&2; exit 2; }
    case "$since" in ''|*[!0-9]*) [ -z "$since" ] || { echo "Error: --since is a sequence number" >&2; exit 2; } ;; esac
    if [ "$all" = true ] && [ "$clear" != true ]; then
        echo "Error: --all goes with --clear" >&2; exit 2
    fi
    local raw code
    if [ "$clear" = true ]; then
        { [ -z "$since" ] && [ "$follow" != true ]; } || { echo "Error: --clear takes no --since or --follow" >&2; exit 2; }
        local body
        body=$(ZW="$name" ZA="$all" python3 -c 'import json, os; print(json.dumps({"widget": os.environ["ZW"], "all": os.environ["ZA"] == "true"}))')
        raw=$(_zen_session_post "/cli/zen/widget/errors/clear" "$body") || raw=""
        ZE_RAW="$raw" ZE_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ.get("ZE_RAW") or ""
try:
    d = json.loads(raw)
except Exception:
    print("Error: no usable answer from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict) or d.get("error") is not None:
    print("Error: %s" % ((d.get("message") if isinstance(d, dict) else None) or raw[:200]), file=sys.stderr)
    sys.exit(1)
if os.environ.get("ZE_JSON") == "true":
    print(json.dumps(d, indent=2))
    sys.exit(0)
print("cleared %s runtime error(s) from widgets/%s%s; %s left" % (d.get("cleared"), d.get("widget"), " (all)" if d.get("all") else "", d.get("left")))
if d.get("left"):
    print("the ones left still count: a placed widget, this version, the last 24 h (k2 zen widget errors %s)" % d.get("widget"))
'
        exit $?
    fi
    while :; do
        local -a q=("widget=$(urlencode "$name")")
        [ -n "$since" ] && q+=("since=$since")
        raw=$(cli_request "/cli/zen/widget/errors" "${q[@]}") || raw=""
        code=0
        ZE_RAW="$raw" ZE_JSON="$as_json" ZE_FOLLOW="$follow" ZE_SINCE="$since" python3 -c '
import json, os, sys
raw = os.environ.get("ZE_RAW") or ""
try:
    d = json.loads(raw)
except Exception:
    print("Error: no usable answer from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(3)
if not isinstance(d, dict) or d.get("error") is not None:
    print("Error: %s" % ((d.get("message") if isinstance(d, dict) else None) or raw[:200]), file=sys.stderr)
    sys.exit(3)
errs = d.get("errors") or []
if os.environ.get("ZE_JSON") == "true":
    print(json.dumps(d, indent=None if os.environ.get("ZE_FOLLOW") == "true" else 2))
else:
    def text(e):
        if e.get("message") is not None:
            return e["message"]
        # Garden v3 (GV49): a widget that reads data sources logs fixed fields only.
        t = "refused %s" % (e.get("class") or "") if e.get("kind") == "refusal" else (e.get("class") or "error")
        if e.get("code"):
            t += " (%s)" % e["code"]
        # 0.46.1: the refused source, when a placement config names it.
        if e.get("address"):
            t += " for %s" % e["address"]
        elif e.get("addressWithheld"):
            t += " for an address no placement of this widget names (withheld)"
        if e.get("line") is not None:
            t += " at line %s" % e["line"]
        if e.get("redacted"):
            t += " [text withheld: this widget reads data sources]"
        return t
    OLD = {"older-version": " (older version)", "unplaced": " (not placed: not counted)", "old": " (over 24 h old: not counted)"}
    for e in errs:
        print("#%s %s%s %s" % (e.get("seq"), e.get("at"), OLD.get(e.get("staleReason"), "" if e.get("current") else " (older version)"), text(e)))
        for line in (e.get("stack") or "").splitlines()[:6]:
            print("    %s" % line)
        if e.get("suggestion"):
            print("  suggestion: %s" % e["suggestion"])
    if not errs and os.environ.get("ZE_FOLLOW") != "true":
        # 0.46.1: say what was checked ("since that one" named no seq).
        since = os.environ.get("ZE_SINCE") or ""
        print("no runtime errors after #%s" % since if since else "no runtime errors logged")
    if any(not e.get("counted", e.get("current")) for e in errs):
        print("old ones aren\x27t counted by validate; drop them with k2 zen widget errors %s --clear" % d.get("widget"))
# 0.46.3: exit 1 only for errors validate counts.
sys.exit(1 if any(e.get("counted", e.get("current")) for e in errs) else 0)
' || code=$?
        [ "$code" -eq 3 ] && exit 1
        if [ "$follow" != true ]; then exit "$code"; fi
        since=$(ZE_RAW="$raw" python3 -c 'import json,os; print(json.loads(os.environ["ZE_RAW"]).get("last", 0))' 2>/dev/null || echo "$since")
        sleep 2
    done
}

# `k2 zen widget show <name> [--json]` (Garden v3, contract-sources-v1 §7):
# each placement (Garden, locked, paused, the source addresses it reads),
# the caps it asked for and runs with, and the reach note when a widget that
# reads sources may also post to the Thread or save its shared data (0.46.1).
cmd_zen_widget_show() {
    local name="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -h|--help)
                echo "Usage: k2 zen widget show <name> [--json]"
                echo "Where a widget is placed, the caps it runs with, the source addresses it reads, and its runtime record."
                exit 0 ;;
            -*) echo "Error: unexpected '$1' (k2 zen widget show --help)" >&2; exit 2 ;;
            *) [ -z "$name" ] && name="$1" || { echo "Error: one widget name" >&2; exit 2; }; shift ;;
        esac
    done
    [ -n "$name" ] || { echo "Error: k2 zen widget show needs a widget name" >&2; exit 2; }
    local raw
    raw=$(cli_request "/cli/zen/widget/show" "widget=$(urlencode "$name")") || raw=""
    ZS_RAW="$raw" ZS_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ.get("ZS_RAW") or ""
try:
    d = json.loads(raw)
except Exception:
    print("Error: no usable answer from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict) or d.get("error") is not None:
    print("Error: %s" % ((d.get("message") if isinstance(d, dict) else None) or raw[:200]), file=sys.stderr)
    sys.exit(4 if isinstance(d, dict) and d.get("error") == "not_found" else 1)
if os.environ.get("ZS_JSON") == "true":
    print(json.dumps(d, indent=2))
    sys.exit(0)
print("%s (%s): %s" % (d.get("widget"), d.get("name"), d.get("state")))
print("  asks for: %s" % (", ".join(d.get("requested") or []) or "nothing"))
print("  runs with: %s" % (", ".join(d.get("caps") or []) or "nothing"))
if d.get("reach"):
    print("  note: %s" % d["reach"])
src = d.get("sources")
if src is not None:
    print("  sources: %s" % (src if isinstance(src, str) else ", ".join(src)))
if d.get("reads"):
    print("  reads data of: %s (read-only)" % ", ".join(d["reads"]))
rt = d.get("runtime")
if isinstance(rt, dict):
    print("  this version: %s, %s runtime error(s) since saved" % ("has run" if rt.get("ran") else "not run yet", rt.get("errorsSinceSave", 0)))
def human(n):
    # K2\x27s limit words: 1 KB = 1,024 bytes.
    if n >= 1048576:
        t = (n * 10 + 524288) // 1048576
        return "%d MB" % (t // 10) if t % 10 == 0 else "%d.%d MB" % (t // 10, t % 10)
    if n >= 1024:
        return "%d KB" % ((n + 512) // 1024)
    return "%d bytes" % n
s = d.get("size")
if isinstance(s, dict) and isinstance(s.get("code"), int):
    # 0.46.5 WS10: the size line.
    large = " (large)" if s["code"] > s.get("codeWarnAt", s["code"]) else ""
    libs = human(s.get("libs") or 0) + (" + CDN files" if s.get("libsUnknown") else "")
    print("  size: code %s of %s%s, page %s of %s, libraries %s (outside the limit)" % (
        human(s["code"]), human(s.get("codeMax") or 0), large, human(s.get("page") or 0), human(s.get("pageMax") or 0), libs))
pl = d.get("placements") or []
if not pl:
    print("  not placed in any Garden")
for p in pl:
    flags = []
    if p.get("locked"):
        flags.append("locked")
    if p.get("paused"):
        flags.append("posting paused")
    print("  - %s / %s%s: %s" % (p.get("gardenName"), p.get("placement"), (" [" + ", ".join(flags) + "]") if flags else "", p.get("note")))
    if p.get("sources"):
        print("      reads: %s" % ", ".join(p["sources"]))
'
}

cmd_zen_widget() {
    local verb="${1:-}"
    shift || true
    if [ "$verb" = "put" ]; then
        cmd_zen_widget_put "$@"
        return
    fi
    if [ "$verb" = "apply" ]; then
        cmd_zen_widget_apply "$@"
        return
    fi
    if [ "$verb" = "preview" ]; then
        cmd_zen_widget_preview "$@"
        return
    fi
    if [ "$verb" = "errors" ]; then
        cmd_zen_widget_errors "$@"
        return
    fi
    if [ "$verb" = "show" ]; then
        cmd_zen_widget_show "$@"
        return
    fi
    if [ "$verb" = "delete" ]; then
        cmd_zen_widget_delete "$@"
        return
    fi
    # Garden access (G19). A K2 older than 0.46.3 has no History route: there
    # `history` stays the unknown subcommand it was.
    if [ "$verb" = "history" ] && _zen_access_on; then
        cmd_zen_access_history widget "$@"
        return
    fi
    local as_json=false from="" widget="" raw body ask=false
    local -a pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            # 0.46.5: k2 zen widget sign <w> --ask puts K2's zen.sign card on
            # your Thread with the person instead of signing.
            --ask) ask=true; shift ;;
            --from)
                [ -n "${2:-}" ] || { echo "Error: --from needs an example: k2:diary, hello or arcade" >&2; exit 2; }
                from="$2"; shift 2 ;;
            --from=*) from="${1#*=}"; shift ;;
            --widget)
                [ -n "${2:-}" ] || { echo "Error: --widget needs a widget folder name" >&2; exit 2; }
                widget="$2"; shift 2 ;;
            --widget=*) widget="${1#*=}"; shift ;;
            -h|--help) cmd_help_zen; exit 0 ;;
            -*) echo "Error: unexpected '$1' (see k2 zen --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local n=${#pos[@]}
    if [ "$verb" != "new" ] && [ -n "$from" ]; then
        echo "Error: --from is for k2 zen widget new" >&2
        exit 2
    fi
    if [ -n "$widget" ] && [ "$verb" != "revoke" ] && [ "$verb" != "grant" ] && [ "$verb" != "allow" ]; then
        echo "Error: --widget is for k2 zen validate, history and reset" >&2
        exit 2
    fi
    case "$verb" in
        new)
            [ "$n" -eq 1 ] || { echo "Error: k2 zen widget new needs one name (lower-case letters, digits, - and _)" >&2; exit 2; }
            body=$(ZN="${pos[0]}" ZF="$from" python3 -c '
import json, os
b = {"name": os.environ["ZN"]}
if os.environ.get("ZF"):
    b["from"] = os.environ["ZF"]
print(json.dumps(b))
')
            # Garden data (contract-garden-data-v1 §4.3): inside a K2 terminal
            # the session's passport, so the new folder is stamped as made by
            # this agent; outside K2, by you.
            raw=$(_zen_session_post "/cli/zen/widget/new" "$body")
            _zen_print widget-new "$as_json" "$raw"
            ;;
        sign|claim)
            [ "$n" -eq 1 ] || { echo "Error: k2 zen widget $verb needs one widget folder name" >&2; exit 2; }
            if [ "$ask" = true ] && [ "$verb" != "sign" ]; then
                echo "Error: --ask is for k2 zen widget sign" >&2
                exit 2
            fi
            body=$(ZW="${pos[0]}" ZA="$ask" python3 -c '
import json, os
b = {"widget": os.environ["ZW"]}
if os.environ.get("ZA") == "true":
    b["ask"] = True
print(json.dumps(b))
')
            if [ "$verb" = "sign" ]; then
                raw=$(_zen_session_post "/cli/zen/widget/sign" "$body")
            else
                raw=$(_zen_person_post "/cli/zen/widget/claim" "$body")
                # Garden access (G20): a K2 with access lists answers
                # `retired`. Keep as mine maps onto signing this version
                # (the person outside K2, or an agent on the list), so
                # claim does that, with the notice (removed in 0.47.0).
                if _zen_is_retired "$raw"; then
                    local repl="k2 zen widget sign ${pos[0]}"
                    raw=$(_zen_session_post "/cli/zen/widget/sign" "$body")
                    if [ "$as_json" = true ]; then
                        raw=$(_zen_retired_tag "$raw" "k2 zen widget claim" "$repl")
                    else
                        _zen_retired_notice "k2 zen widget claim" "$repl"
                    fi
                    _zen_sign_print sign "$as_json" "$raw"
                    return
                fi
            fi
            _zen_sign_print "$verb" "$as_json" "$raw"
            ;;
        list)
            [ "$n" -eq 0 ] || { echo "Error: k2 zen widget list takes only --json" >&2; exit 2; }
            raw=$(cli_request "/cli/zen/widgets")
            _zen_print widget-list "$as_json" "$raw"
            ;;
        grant|allow|revoke)
            echo "Widgets in your own Gardens need no permissions: they can talk to your agents right away. To stop one, take its [[widget]] out of the Garden file."
            exit 0
            ;;
        ""|help|--help|-h)
            cmd_help_zen
            ;;
        *)
            echo "Unknown 'k2 zen widget' subcommand: $verb (new, list, show, errors, sign, claim)" >&2
            exit 2
            ;;
    esac
}

# Garden data authorship (contract-garden-data-v1 §4.3, §8): print a
# widget/sign or widget/claim answer. Exit 3 when only the person can
# (claim from inside a K2 terminal), 4 for no such widget, 1 otherwise.
_zen_sign_print() {
    ZEN_VERB="$1" ZEN_JSON="$2" ZEN_RAW="$3" python3 -c '
import json, os, sys
verb = os.environ["ZEN_VERB"]
as_json = os.environ.get("ZEN_JSON") == "true"
raw = os.environ.get("ZEN_RAW") or ""
try:
    d = json.loads(raw) if raw.strip() else None
except Exception:
    d = None
if not isinstance(d, dict):
    print("Error: no answer from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err is not None:
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    code = err.get("code") if isinstance(err, dict) else err
    msg = d.get("message") or (err.get("hint") if isinstance(err, dict) else None) or str(code)
    if code in ("owner_only", "zen_local_only"):
        print("Only the person can %s this widget: click It\x27s mine in Settings → Gardens → the Garden → Local Data, or run k2 zen widget claim outside K2." % verb if verb == "claim" else "Error: %s" % msg, file=sys.stderr)
        sys.exit(3)
    if code == "sign_needs_person":
        # 0.46.5: your signature can\x27t cover this version where it is
        # placed; K2 put its zen.sign card on your Thread with the person.
        if not as_json:
            print("Error: %s" % msg, file=sys.stderr)
            if d.get("changes"):
                print("The card shows: %s" % d.get("changes"), file=sys.stderr)
            if d.get("card"):
                print("Card: %s (K2 tells you when the person answers)" % d.get("card"), file=sys.stderr)
        sys.exit(3)
    print("Error: %s" % msg, file=sys.stderr)
    sys.exit(4 if code in ("unknown_widget", "not_found") else 1)
if as_json:
    print(json.dumps(d, indent=2))
    sys.exit(0)
if d.get("asked"):
    if d.get("card"):
        print("Asked the person to sign %s: K2 put a card on your Thread%s." % (d.get("widget"), " (one was already waiting)" if d.get("waiting") else ""))
    elif d.get("note"):
        print("You have no Thread with the person, so K2 left them an inbox note to sign %s." % d.get("widget"))
    else:
        print("No new card: K2 already asked the person about this version of %s." % d.get("widget"))
    if d.get("changes"):
        print("The card shows: %s" % d.get("changes"))
    sys.exit(0)
m = d.get("madeBy") or {}
kind = m.get("kind")
who = "you" if kind == "owner" else ("K2" if kind == "k2" else (m.get("handle") or "an agent"))
whose = "your" if kind in ("owner", "k2") else "its"
if verb == "claim":
    print("It\x27s yours: this version of %s now reads and writes data with your rights." % d.get("widget"))
elif "vouched" in d and kind != "k2":
    # Garden access (G9): a sign by the person or a listed agent
    # vouches for this version; the signer gate checks the list per Garden.
    where = "in every Garden" if kind == "owner" else "in each Garden whose list has %s" % who
    print("%s %s: this version of %s runs with the Garden\x27s data %s." % ("Signed as" if d.get("stamped") else "Already signed by", who, d.get("widget"), where))
elif kind == "k2":
    print("%s is a built-in: it is K2\x27s and needs no signing." % d.get("widget"))
elif d.get("stamped"):
    print("Signed as %s: this widget now reads and writes data with %s grants." % (who, whose))
else:
    print("Already signed by %s (the first signer of a version keeps it): this widget reads and writes data with %s grants." % (who, whose))
r = d.get("review")
if isinstance(r, dict) and r.get("ask"):
    # 0.46.5: the upgrade kept this version signed although its manifest
    # had changed; the person hasn\x27t looked yet.
    print("Note: %s %s. Ask the person to look: %s" % (d.get("widget"), (r.get("changes") or "").rstrip(".").replace("Since it was signed, this widget", "changed since it was signed: it"), r.get("ask")))
'
}

_zen_no_home() {
    echo "Error: --home is gone: Zen pages are Gardens now. Use --garden <name|id> (k2 zen garden list)." >&2
    exit 2
}

# Parse --garden/--file/--theme/--to/--json into ZEN_* globals.
_zen_args() {
    ZEN_AS_JSON=false; ZEN_GARDEN_SEL=""; ZEN_FILE_SEL=""; ZEN_TO=""; ZEN_THEME_SEL=""; ZEN_WIDGET_SEL=""; ZEN_NO_SIGN=false; ZEN_TYPES=""
    # validate only (0.46.1 S3): a staged folder or Garden page, --strict, --sign.
    ZEN_DIR_SEL=""; ZEN_AS=""; ZEN_FROM=""; ZEN_STRICT=false; ZEN_SIGN=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --no-sign) ZEN_NO_SIGN=true; shift ;;
            --sign) ZEN_SIGN=true; shift ;;
            --strict) ZEN_STRICT=true; shift ;;
            --dir)
                [ -n "${2:-}" ] || { echo "Error: --dir needs a staged widget folder" >&2; exit 2; }
                ZEN_DIR_SEL="$2"; shift 2 ;;
            --dir=*) ZEN_DIR_SEL="${1#*=}"; shift ;;
            --as)
                [ -n "${2:-}" ] || { echo "Error: --as needs a widget name" >&2; exit 2; }
                ZEN_AS="$2"; shift 2 ;;
            --as=*) ZEN_AS="${1#*=}"; shift ;;
            --from)
                [ -n "${2:-}" ] || { echo "Error: --from needs a staged Garden page (or - for stdin)" >&2; exit 2; }
                ZEN_FROM="$2"; shift 2 ;;
            --from=*) ZEN_FROM="${1#*=}"; shift ;;
            --widget)
                [ -n "${2:-}" ] || { echo "Error: --widget needs a widget folder name (see k2 zen widget list)" >&2; exit 2; }
                ZEN_WIDGET_SEL="$2"; shift 2 ;;
            --widget=*) ZEN_WIDGET_SEL="${1#*=}"; shift ;;
            --types) ZEN_TYPES=1; shift ;;
            --types=all) ZEN_TYPES=all; shift ;;
            --json) ZEN_AS_JSON=true; shift ;;
            --garden)
                [ -n "${2:-}" ] || { echo "Error: --garden needs a Garden name or id (see k2 zen garden list)" >&2; exit 2; }
                ZEN_GARDEN_SEL="$2"; shift 2 ;;
            --garden=*) ZEN_GARDEN_SEL="${1#*=}"; shift ;;
            --home|--home=*) _zen_no_home ;;
            --file)
                [ -n "${2:-}" ] || { echo "Error: --file needs zen.toml, gardens/<id>.toml or themes/<name>/theme.toml" >&2; exit 2; }
                ZEN_FILE_SEL="$2"; shift 2 ;;
            --file=*) ZEN_FILE_SEL="${1#*=}"; shift ;;
            --to)
                [ -n "${2:-}" ] || { echo "Error: --to needs a snapshot name (see k2 zen history)" >&2; exit 2; }
                ZEN_TO="$2"; shift 2 ;;
            --to=*) ZEN_TO="${1#*=}"; shift ;;
            --theme)
                [ -n "${2:-}" ] || { echo "Error: --theme needs a theme name (see k2 zen theme list)" >&2; exit 2; }
                ZEN_THEME_SEL="$2"; shift 2 ;;
            --theme=*) ZEN_THEME_SEL="${1#*=}"; shift ;;
            *) echo "Error: unexpected '$1' (see k2 zen --help)" >&2; exit 2 ;;
        esac
    done
    local n=0
    [ -n "$ZEN_GARDEN_SEL" ] && n=$((n + 1))
    [ -n "$ZEN_FILE_SEL" ] && n=$((n + 1))
    [ -n "$ZEN_THEME_SEL" ] && n=$((n + 1))
    [ -n "$ZEN_WIDGET_SEL" ] && n=$((n + 1))
    [ -n "$ZEN_DIR_SEL" ] && n=$((n + 1))
    if [ "$n" -gt 1 ]; then
        echo "Error: pass one of --garden, --theme, --widget, --dir or --file" >&2
        exit 2
    fi
}

# The validate-only flags (0.46.1 S3) on any other verb are a usage error.
_zen_validate_only_flags() {
    local sub="$1"
    [ "$sub" = "validate" ] && return 0
    if [ -n "$ZEN_DIR_SEL$ZEN_AS$ZEN_FROM" ] || [ "$ZEN_STRICT" = true ] || [ "$ZEN_SIGN" = true ]; then
        echo "Error: --dir, --as, --from, --strict and --sign are for k2 zen validate" >&2
        exit 2
    fi
}

