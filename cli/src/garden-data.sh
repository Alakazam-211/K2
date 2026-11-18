# ── Garden data — k2 store --widget / k2 zen data (contract-garden-data-v1 §8) ──
# A widget's own keyspace on THIS computer (`data:<widget>`), shared with the
# agents the person grants. `store` is a locked id tool, so inside a K2
# terminal these always send the session passport (grants apply per call);
# outside K2 the owner token is the person. Routes are /cli/zen/data/*, never
# /cli/store/* (GL28). C0 owns this C0 block, the dispatch, help and
# --schema; B1 edits only BEGIN/END_GARDEN_DATA_B1 and _ZEN_DATA_B1, B3 only
# the _B3 regions.
# BEGIN_GARDEN_DATA_C0

# Print a Garden data refusal and exit with the contract code (§8):
# 3 person/grant refusals, 4 not found, 5 conflict, 1 anything else.
_gd_fail() {
    local status="$1" body="$2" code=""
    code="$(printf '%s' "$body" | python3 -c '
import json, sys
try:
    v = json.loads(sys.stdin.read())
    e = v.get("error") if isinstance(v, dict) else None
    print(e.get("code", "") if isinstance(e, dict) else (e or ""))
except Exception:
    print("")
' 2>/dev/null)"
    if [ "${_GD_JSON:-0}" = "1" ] && [ -n "$body" ]; then
        printf '%s\n' "$body"
    elif [ -n "$body" ]; then
        printf '%s' "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    e = json.loads(raw).get("error")
    if isinstance(e, dict):
        print("Error: %s: %s" % (e.get("code", "error"), e.get("hint", "")))
    else:
        print("Error: %s" % e)
except Exception:
    print("Error: daemon returned: %s" % raw[:200])
' >&2
    else
        echo "Error: no answer from the K2 daemon (status ${status:-none})" >&2
    fi
    case "$code" in
        person_only|owner_only)
            echo "This needs a person. Run it outside K2, or use Settings → Gardens → the Garden → Local Data." >&2
            exit 3 ;;
        not_granted|self_grant|level_too_high|not_own|author_unknown|core_off) exit 3 ;;
        space_not_found|agent_not_found|project_not_found|grant_not_found|widget_not_found|no_snapshot|blob_missing) exit 4 ;;
        conflict) exit 5 ;;
        *) exit 1 ;;
    esac
}

# _gd_call METHOD PATH [PAYLOAD]: GET payload is a query string, POST payload
# is JSON. Anything but 200 fails (and exits); the body lands in _GD_BODY.
_gd_call() {
    local raw
    raw="$(_compute_send "$@")"
    _GD_STATUS="${raw##*$'\n'}"
    _GD_BODY="${raw%$'\n'*}"
    [ "$_GD_STATUS" = "$raw" ] && _GD_BODY=""
    [ -z "$_GD_STATUS" ] && _GD_STATUS="000"
    [ "$_GD_STATUS" = "200" ] || _gd_fail "$_GD_STATUS" "$_GD_BODY"
}

_gd_not_built() {
    echo "Error: not_built: k2 $1 is not built yet" >&2
    exit 1
}

# True when the arguments name --widget (k2 store's Garden data mode).
_gd_has_widget() {
    local a
    for a in "$@"; do [ "$a" = "--widget" ] && return 0; done
    return 1
}

# k2 store <verb> --widget <name> …  (flags may come in any order).
# Sets _GD_WIDGET and _GD_JSON, passes the rest to the verb.
cmd_store_widget() {
    local sub="${1:-}"
    shift || true
    _GD_WIDGET="" _GD_JSON=0
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --widget) _GD_WIDGET="${2:-}"; shift 2 || { echo "Error: --widget takes a name" >&2; exit 2; } ;;
            --json) _GD_JSON=1; shift ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    if [ -z "$_GD_WIDGET" ]; then
        echo "Error: --widget takes a widget name (k2 zen widget list)" >&2
        exit 2
    fi
    set -- ${rest[@]+"${rest[@]}"}
    case "$sub" in
        get) cmd_store_widget_get "$@" ;;
        put) cmd_store_widget_put "$@" ;;
        patch) cmd_store_widget_patch "$@" ;;
        rm|delete) cmd_store_widget_rm "$@" ;;
        list|ls) cmd_store_widget_list "$@" ;;
        changes) cmd_store_widget_changes "$@" ;;
        batch) cmd_store_widget_batch "$@" ;;
        grants) cmd_store_widget_grants "$@" ;;
        grant) cmd_store_widget_grant "$@" ;;
        revoke) cmd_store_widget_revoke "$@" ;;
        export) cmd_store_widget_export "$@" ;;
        blob) cmd_store_widget_blob "$@" ;;
        history) cmd_store_widget_history "$@" ;;
        restore) cmd_store_widget_restore "$@" ;;
        snapshot) cmd_store_widget_snapshot "$@" ;;
        *)
            echo "Error: unknown 'k2 store $sub --widget' command — see k2 store --help" >&2
            exit 2 ;;
    esac
}

# k2 zen data <verb> … (person-only admin). Goes through _compute_send, so
# inside a K2 terminal it sends the passport and the daemon refuses
# (person_only, exit 3).
cmd_zen_data() {
    local sub="${1:-}"
    shift || true
    _GD_JSON=0
    local a
    for a in "$@"; do [ "$a" = "--json" ] && _GD_JSON=1; done
    case "$sub" in
        ""|help|--help|-h) cmd_help_zen_data; return 0 ;;
        list|ls) cmd_zen_data_list "$@" ;;
        show) cmd_zen_data_show "$@" ;;
        grant) cmd_zen_data_grant "$@" ;;
        revoke) cmd_zen_data_revoke "$@" ;;
        admin) cmd_zen_data_admin "$@" ;;
        unadmin) cmd_zen_data_unadmin "$@" ;;
        delete|rm) cmd_zen_data_delete "$@" ;;
        move|mv) cmd_zen_data_move "$@" ;;
        export) cmd_zen_data_export "$@" ;;
        import) cmd_zen_data_import "$@" ;;
        restore) cmd_zen_data_restore "$@" ;;
        *)
            echo "Error: unknown 'k2 zen data $sub'" >&2
            cmd_help_zen_data >&2
            exit 2 ;;
    esac
}

cmd_help_zen_data() {
    cat <<'EOF'
k2 zen data — a widget's data on this computer (the person only)

Each custom widget has its own keyspace (data:<widget>) that it reads and
writes with k2.store, and that agents you allow read and write with
`k2 store --widget`. These commands are for you, the person: inside a K2
terminal they're refused (run them outside K2, or use a Garden's Local
Data in Settings → Gardens).

USAGE
  k2 zen data list [--json]
  k2 zen data show <widget> [--json]
  k2 zen data grant <widget> --agent <h> | --project <p> --level read|write [--prefix p/]
  k2 zen data revoke <widget> --agent <h> | --project <p> [--prefix p/]
  k2 zen data admin <widget> --agent <h> --level read|write [--prefix p/]
  k2 zen data unadmin <widget> --agent <h> [--prefix p/] [--yes]
  k2 zen data delete <widget> [--yes]
  k2 zen data move <old> <new>
  k2 zen data export <widget> [--at YYYY-MM-DD] [--blobs <dir>] > data.jsonl
  k2 zen data import <widget> [--replace] [--blobs <dir>] < data.jsonl
  k2 zen data restore <widget> (--rev N | --at 2026-10-26T17:30 | --snapshot <ref>)
                      [--prefix p/]... [--except r/]... [--dry-run]

GRANTS
  An agent grant is a wall: only that agent gets it. A --project grant is a
  convenience, not a wall: any agent can join a K2 project. admin makes one
  agent a grant admin: it may grant others inside its prefix, up to its own
  level, never itself; unadmin removes it and every grant it made.
EOF
}
# END_GARDEN_DATA_C0

# BEGIN_GARDEN_DATA_B1
# Flags for k2 store --widget and k2 zen data (bash 3.2: no empty arrays).
_gd_args() {
    _GD_POS="" _GD_POS2="" _GD_VALUE="" _GD_HAS_VALUE=0 _GD_FILE="" _GD_IFREV="" _GD_PREFIX="" _GD_AFTER=""
    _GD_LIMIT="" _GD_ALL=0 _GD_SINCE="" _GD_WAIT="" _GD_FOLLOW=0 _GD_AGENT="" _GD_PROJECT="" _GD_LEVEL="" _GD_YES=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --value) _GD_VALUE="${2-}"; _GD_HAS_VALUE=1; shift 2 || { echo "Error: --value takes JSON" >&2; exit 2; } ;;
            --file) _GD_FILE="${2:-}"; shift 2 || { echo "Error: --file takes a path" >&2; exit 2; } ;;
            --if-rev) _GD_IFREV="${2:-}"; shift 2 || { echo "Error: --if-rev takes a number" >&2; exit 2; } ;;
            --prefix) _GD_PREFIX="${2-}"; shift 2 || { echo "Error: --prefix takes a prefix" >&2; exit 2; } ;;
            --after) _GD_AFTER="${2-}"; shift 2 || { echo "Error: --after takes a key" >&2; exit 2; } ;;
            --limit) _GD_LIMIT="${2:-}"; shift 2 || { echo "Error: --limit takes a number" >&2; exit 2; } ;;
            --since) _GD_SINCE="${2:-}"; shift 2 || { echo "Error: --since takes a rev" >&2; exit 2; } ;;
            --wait) _GD_WAIT="${2:-}"; shift 2 || { echo "Error: --wait takes seconds" >&2; exit 2; } ;;
            --agent) _GD_AGENT="${2:-}"; shift 2 || { echo "Error: --agent takes a handle" >&2; exit 2; } ;;
            --project) _GD_PROJECT="${2:-}"; shift 2 || { echo "Error: --project takes a name" >&2; exit 2; } ;;
            --level) _GD_LEVEL="${2:-}"; shift 2 || { echo "Error: --level takes read or write" >&2; exit 2; } ;;
            --all) _GD_ALL=1; shift ;;
            --follow) _GD_FOLLOW=1; shift ;;
            --yes|-y) _GD_YES=1; shift ;;
            --json) _GD_JSON=1; shift ;;
            --*) echo "Error: unknown flag $1" >&2; exit 2 ;;
            *) if [ -z "$_GD_POS" ]; then _GD_POS="$1"; elif [ -z "$_GD_POS2" ]; then _GD_POS2="$1"; else echo "Error: unexpected '$1'" >&2; exit 2; fi; shift ;;
        esac
    done
    case "$_GD_IFREV$_GD_LIMIT$_GD_SINCE$_GD_WAIT" in
        *[!0-9]*) echo "Error: --if-rev, --limit, --since and --wait take whole numbers" >&2; exit 2 ;;
    esac
}

# A JSON body from the parsed flags. $1 = kind (put|patch|rm|grant|revoke|admin|unadmin).
_gd_body() {
    K2_GD_KIND="$1" K2_GD_WIDGET="${_GD_WIDGET:-}" K2_GD_KEY="$_GD_POS" K2_GD_VALUE="$_GD_VALUE" \
    K2_GD_HAS_VALUE="$_GD_HAS_VALUE" K2_GD_FILE="$_GD_FILE" K2_GD_IFREV="$_GD_IFREV" K2_GD_AGENT="$_GD_AGENT" \
    K2_GD_PROJECT="$_GD_PROJECT" K2_GD_LEVEL="$_GD_LEVEL" K2_GD_PREFIX="$_GD_PREFIX" K2_GD_DRY="${2:-}" python3 -c '
import json, os, sys
e = os.environ.get
kind = e("K2_GD_KIND")
b = {"widget": e("K2_GD_WIDGET")}
def die(m):
    sys.stderr.write("Error: %s\n" % m)
    sys.exit(2)
if kind in ("put", "patch", "rm"):
    if not e("K2_GD_KEY"):
        die("a key is required (k2 store %s --widget <w> <key>)" % kind)
    b["key"] = e("K2_GD_KEY")
    if e("K2_GD_IFREV"):
        b["ifRev"] = int(e("K2_GD_IFREV"))
if kind in ("put", "patch"):
    if e("K2_GD_FILE"):
        text = open(e("K2_GD_FILE")).read()
    elif e("K2_GD_HAS_VALUE") == "1":
        text = e("K2_GD_VALUE")
    else:
        die("%s takes --value <json> or --file <path>" % kind)
    try:
        b["value"] = json.loads(text)
    except Exception as x:
        die("the value is not JSON: %s" % x)
    if kind == "patch" and not isinstance(b["value"], dict):
        die("patch takes a JSON object of the fields to change (null removes a field)")
if kind in ("grant", "revoke", "admin", "unadmin"):
    if kind in ("admin", "unadmin") and e("K2_GD_PROJECT"):
        die("a grant admin is one agent (--agent)")
    if bool(e("K2_GD_AGENT")) == bool(e("K2_GD_PROJECT")):
        die("name exactly one of --agent or --project")
    if e("K2_GD_AGENT"):
        b["agent"] = e("K2_GD_AGENT")
    if e("K2_GD_PROJECT"):
        b["project"] = e("K2_GD_PROJECT")
    b["prefix"] = e("K2_GD_PREFIX") or ""
    if kind in ("grant", "admin"):
        if e("K2_GD_LEVEL") not in ("read", "write"):
            die("--level is read or write")
        b["level"] = e("K2_GD_LEVEL")
    if kind == "unadmin" and e("K2_GD_DRY") == "dry":
        b["dryRun"] = True
print(json.dumps(b))
'
}

# Print the answer: raw with --json, else what the python snippet $1 makes of it.
_gd_out() {
    if [ "${_GD_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_GD_BODY"
        return 0
    fi
    printf '%s' "$_GD_BODY" | python3 -c "$1"
}

_gd_q() {
    local q="widget=$(urlencode "$_GD_WIDGET")" pair
    for pair in "$@"; do
        [ -n "${pair#*=}" ] && q="$q&${pair%%=*}=$(urlencode "${pair#*=}")"
    done
    printf '%s' "$q"
}

cmd_store_widget_get() {
    _gd_args "$@"
    [ -n "$_GD_POS" ] || { echo "Error: usage: k2 store get --widget <w> <key>" >&2; exit 2; }
    _gd_call GET /cli/zen/data/get "$(_gd_q "key=$_GD_POS")"
    if [ "${_GD_JSON:-0}" = "1" ]; then printf '%s\n' "$_GD_BODY"; return 0; fi
    printf '%s' "$_GD_BODY" | python3 -c '
import json, sys
e = json.loads(sys.stdin.read())["entry"]
if e is None:
    sys.stderr.write("No such key.\n")
    sys.exit(4)
print(json.dumps(e["value"], indent=2, ensure_ascii=False))
sys.stderr.write("rev %s by %s\n" % (e["rev"], e["by"]))
'
}

cmd_store_widget_put() {
    _gd_args "$@"
    local body; body="$(_gd_body put)" || exit 2
    _gd_call POST /cli/zen/data/put "$body"
    _gd_out 'import json, sys; print("rev %s" % json.loads(sys.stdin.read())["rev"])'
}

# 0.46.4: change some fields of one key and keep the rest (JSON Merge
# Patch: null removes a field). Prints the new rev and the merged document.
cmd_store_widget_patch() {
    _gd_args "$@"
    local body; body="$(_gd_body patch)" || exit 2
    _gd_call POST /cli/zen/data/patch "$body"
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
print(json.dumps(v["value"], indent=2, ensure_ascii=False))
sys.stderr.write(("rev %s\n" % v["rev"]) if v["changed"] else ("rev %s (nothing changed)\n" % v["rev"]))'
}

cmd_store_widget_rm() {
    _gd_args "$@"
    local body; body="$(_gd_body rm)" || exit 2
    _gd_call POST /cli/zen/data/delete "$body"
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
print("rev %s" % v["rev"] if v["deleted"] else "No such key (nothing changed).")'
}

cmd_store_widget_list() {
    _gd_args "$@"
    local after="$_GD_AFTER" all_items="" page_json
    while :; do
        _gd_call GET /cli/zen/data/list "$(_gd_q "prefix=$_GD_PREFIX" "after=$after" "limit=$_GD_LIMIT")"
        if [ "$_GD_ALL" = "1" ]; then
            all_items="$all_items$(printf '%s' "$_GD_BODY" | python3 -c 'import json,sys; [print(json.dumps(i, ensure_ascii=False)) for i in json.loads(sys.stdin.read())["items"]]')
"
            after="$(printf '%s' "$_GD_BODY" | python3 -c 'import json,sys; n=json.loads(sys.stdin.read())["next"]; print(n or "")')"
            [ -n "$after" ] || break
        else
            break
        fi
    done
    if [ "$_GD_ALL" = "1" ]; then
        page_json="$(printf '%s' "$all_items" | python3 -c '
import json, sys
print(json.dumps({"items": [json.loads(l) for l in sys.stdin if l.strip()], "next": None}, ensure_ascii=False))')"
        _GD_BODY="$page_json"
    fi
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
for i in v["items"]:
    print("%s\trev %s\t%s" % (i["key"], i["rev"], json.dumps(i["value"], ensure_ascii=False)[:200]))
if v.get("next"):
    sys.stderr.write("more: --after %s\n" % v["next"])'
}

# One changes page from (since, after); prints one JSON line per change and
# sets _GD_NEXT_SINCE / _GD_NEXT_AFTER (empty = caught up). Exit 9 = resync.
_gd_changes_page() {
    local since="$1" after="$2" wait="$3" raw status body
    raw="$(_compute_send GET /cli/zen/data/changes "$(_gd_q "since=$since" "after=$after" "prefix=$_GD_PREFIX" "wait=$wait")")"
    status="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    [ "$status" = "$raw" ] && body=""
    if [ "$status" = "409" ] && printf '%s' "$body" | grep -q '"resync"'; then
        _GD_NEXT_SINCE="$(printf '%s' "$body" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["error"].get("reapedRev", 0))')"
        _GD_NEXT_AFTER=""
        return 9
    fi
    [ "$status" = "200" ] || _gd_fail "$status" "$body"
    local out
    out="$(K2_GD_SINCE="$since" K2_GD_AFTER="$after" python3 -c '
import json, os, sys
v = json.loads(sys.stdin.read())
for c in v["items"]:
    print(json.dumps(c, ensure_ascii=False))
n = v.get("next")
if n:
    print("#NEXT %s %s" % (n["since"], n.get("after") or ""))
elif v["items"]:
    print("#NEXT %s " % v["items"][-1]["rev"])
else:
    print("#NEXT %s %s" % (os.environ["K2_GD_SINCE"], os.environ["K2_GD_AFTER"]))
print("#MORE %s" % ("1" if n else "0"))
' <<<"$body")"
    printf '%s\n' "$out" | grep -v '^#'
    local nl; nl="$(printf '%s\n' "$out" | grep '^#NEXT ')"
    nl="${nl#\#NEXT }"
    _GD_NEXT_SINCE="${nl%% *}"
    _GD_NEXT_AFTER="${nl#* }"
    _GD_MORE="$(printf '%s\n' "$out" | grep '^#MORE ' | cut -d' ' -f2)"
    return 0
}

# Every key under --prefix now, one JSON line each (after a resync).
_gd_dump_all() {
    local after="" next
    while :; do
        _gd_call GET /cli/zen/data/list "$(_gd_q "prefix=$_GD_PREFIX" "after=$after" "limit=500")"
        next="$(printf '%s' "$_GD_BODY" | python3 -c '
import json, sys
v = json.loads(sys.stdin.read())
for i in v["items"]:
    print(json.dumps(i, ensure_ascii=False))
print("#NEXT " + (v["next"] or ""))')"
        printf '%s\n' "$next" | grep -v '^#NEXT'
        after="$(printf '%s\n' "$next" | grep '^#NEXT' | cut -c7-)"
        [ -n "$after" ] || break
    done
}

cmd_store_widget_changes() {
    _gd_args "$@"
    local since="${_GD_SINCE:-0}" after="" wait="${_GD_WAIT:-0}" rc
    if [ "${_GD_JSON:-0}" = "1" ] && [ "$_GD_FOLLOW" = "0" ]; then
        _gd_call GET /cli/zen/data/changes "$(_gd_q "since=$since" "prefix=$_GD_PREFIX" "wait=$wait")"
        printf '%s\n' "$_GD_BODY"
        return 0
    fi
    [ "$_GD_FOLLOW" = "1" ] && wait=30
    while :; do
        _gd_changes_page "$since" "$after" "$wait"; rc=$?
        if [ "$rc" = "9" ]; then
            # The cursor is older than kept history: say so, print every
            # key as it is now, and go on from the reaped rev.
            echo '{"resync": true}'
            _gd_dump_all
            since="$_GD_NEXT_SINCE"; after=""
            [ "$_GD_FOLLOW" = "1" ] || exit 1
            continue
        fi
        since="$_GD_NEXT_SINCE"; after="$_GD_NEXT_AFTER"
        if [ "$_GD_MORE" = "1" ]; then continue; fi
        [ "$_GD_FOLLOW" = "1" ] || break
    done
}

# Read ops (a JSON array or JSONL) on stdin. ≤ 64 ops and ≤ 60 KiB: one
# batch; otherwise a transaction in parts of ≤ 64 ops / 60 KiB (≤ 1,000 ops
# and 1 MiB in all), one rev either way.
cmd_store_widget_batch() {
    _gd_args "$@"
    local plan
    plan="$(python3 -c '
import json, sys
raw = sys.stdin.read().strip()
if not raw:
    sys.stderr.write("Error: k2 store batch reads ops on stdin (a JSON array or one op per line)\n"); sys.exit(2)
try:
    ops = json.loads(raw) if raw.startswith("[") else [json.loads(l) for l in raw.splitlines() if l.strip()]
except Exception as e:
    sys.stderr.write("Error: the ops are not JSON: %s\n" % e); sys.exit(2)
if not isinstance(ops, list) or not ops:
    sys.stderr.write("Error: no ops\n"); sys.exit(2)
if len(ops) > 1000:
    sys.stderr.write("Error: at most 1,000 ops in one batch\n"); sys.exit(2)
parts, cur, size = [], [], 0
for op in ops:
    b = len(json.dumps(op.get("value"), separators=(",", ":"))) if "value" in op else 0
    if cur and (len(cur) == 64 or size + b > 60 * 1024):
        parts.append(cur); cur, size = [], 0
    cur.append(op); size += b
parts.append(cur)
for p in parts:
    print(json.dumps(p, separators=(",", ":")))
')" || exit 2
    local nparts; nparts="$(printf '%s\n' "$plan" | grep -c .)"
    local wjson; wjson="$(K2_GD_WIDGET="$_GD_WIDGET" python3 -c 'import json,os; print(json.dumps(os.environ["K2_GD_WIDGET"]))')"
    if [ "$nparts" = "1" ]; then
        _gd_call POST /cli/zen/data/batch "{\"widget\":$wjson,\"ops\":$plan}"
        _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("rev %s (%s ops)" % (v["rev"], v["ops"]))'
        return 0
    fi
    _gd_call POST /cli/zen/data/tx/begin "{\"widget\":$wjson}"
    local tx; tx="$(printf '%s' "$_GD_BODY" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["txId"])')"
    local part raw status body
    while IFS= read -r part; do
        [ -n "$part" ] || continue
        raw="$(_compute_send POST /cli/zen/data/tx/add "{\"txId\":\"$tx\",\"ops\":$part}")"
        status="${raw##*$'\n'}"; body="${raw%$'\n'*}"
        if [ "$status" != "200" ]; then
            # Best effort, soft: an abort with no answer never ends the
            # command before _gd_fail names the add that failed.
            _K2_NOANSWER_SOFT=1 _compute_send POST /cli/zen/data/tx/abort "{\"txId\":\"$tx\"}" >/dev/null 2>&1 || true
            _gd_fail "$status" "$body"
        fi
    done <<<"$plan"
    _gd_call POST /cli/zen/data/tx/commit "{\"txId\":\"$tx\"}"
    _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("rev %s (%s ops)" % (v["rev"], v["ops"]))'
}

cmd_store_widget_grants() {
    _gd_args "$@"
    _gd_call GET /cli/zen/data/grants "$(_gd_q)"
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
if not v["grants"]:
    print("No access to %s. Ask the person (k2 feedback or the Thread)." % v["space"])
for g in v["grants"]:
    f = g.get("from") or {}
    src = {"agent": "you", "delegation": "grant admin"}.get(f.get("kind"), "project %s" % f.get("name", "?"))
    if "from" not in g:
        src = "%s %s" % (g["subjectKind"], g["subjectId"])
    print("%-24s %-5s %s%s" % (g["prefix"] or "(everything)", g["level"], src, " (admin)" if g.get("admin") else ""))
for g in v.get("made") or []:
    print("made: %-18s %-5s -> %s %s" % (g["prefix"] or "(everything)", g["level"], g["subjectKind"], g["subjectId"]))'
}

_gd_project_note() {
    if printf '%s' "$_GD_BODY" | grep -q 'project_grant_convenience'; then
        echo "Note: any agent can join project \"$_GD_PROJECT\", so this grant is a convenience, not a wall. For a wall, grant agents one by one or name a grant admin." >&2
    fi
}

cmd_store_widget_grant() {
    _gd_args "$@"
    local body; body="$(_gd_body grant)" || exit 2
    _gd_call POST /cli/zen/data/grant "$body"
    _gd_project_note
    _gd_out 'import json, sys; g = json.loads(sys.stdin.read())["grant"]; print("Granted %s on %s to %s %s" % (g["level"], g["prefix"] or "everything", g["subjectKind"], g["subjectId"]))'
}

cmd_store_widget_revoke() {
    _gd_args "$@"
    local body; body="$(_gd_body revoke)" || exit 2
    _gd_call POST /cli/zen/data/revoke "$body"
    _gd_out 'print("Revoked.")'
}
# 0.46.5 (prd-garden-board-needs-v1 GB16): files in a widget's data.
#   k2 store blob put <file> --widget <w> [--mime m] [--name n] [--json]
#   k2 store blob get <ref> --widget <w> --out <file> [--px 256|512] [--json]
#   k2 store blob ls --widget <w> [--json]
# Bytes go in 2 MiB chunks (the cell socket takes 4 MiB bodies); the daemon
# hashes them, sniffs the type and keeps the file until no key names it.
_GD_BLOB_CHUNK=2097152

# _gd_blob_put_file WIDGET FILE MIME NAME → _GD_BODY is the BlobInfo.
_gd_blob_put_file() {
    local widget="$1" file="$2" mime="$3" name="$4" size offset=0 upload="" body last
    [ -f "$file" ] && [ -r "$file" ] || { echo "Error: can't read '$file'" >&2; exit 2; }
    size="$(python3 -c 'import os, sys; print(os.path.getsize(sys.argv[1]))' "$file")"
    if [ "$size" -gt 16777216 ]; then
        echo "Error: too_large: a file is at most 16 MiB; '$file' is $size bytes" >&2
        exit 1
    fi
    [ -n "$name" ] || name="$(basename "$file")"
    while :; do
        body="$(GD_W="$widget" GD_F="$file" GD_OFF="$offset" GD_N="$_GD_BLOB_CHUNK" GD_UP="$upload" GD_MIME="$mime" GD_NAME="$name" python3 -c '
import base64, json, os
e = os.environ
off, n = int(e["GD_OFF"]), int(e["GD_N"])
size = os.path.getsize(e["GD_F"])
with open(e["GD_F"], "rb") as f:
    f.seek(off)
    chunk = f.read(n)
b = {"widget": e["GD_W"], "offset": off, "base64": base64.b64encode(chunk).decode(), "last": off + len(chunk) >= size}
if e["GD_UP"]:
    b["upload"] = e["GD_UP"]
if e["GD_MIME"]:
    b["mime"] = e["GD_MIME"]
if e["GD_NAME"]:
    b["name"] = e["GD_NAME"]
print(json.dumps(b))
')" || { echo "Error: can't read '$file'" >&2; exit 1; }
        last="$(printf '%s' "$body" | python3 -c 'import json, sys; print("1" if json.loads(sys.stdin.read())["last"] else "0")')"
        _gd_call POST /cli/zen/data/blob/put "$body"
        [ "$last" = "1" ] && break
        upload="$(printf '%s' "$_GD_BODY" | python3 -c 'import json, sys; print(json.loads(sys.stdin.read())["upload"])')"
        offset=$((offset + _GD_BLOB_CHUNK))
    done
}

# _gd_blob_get_file WIDGET REF OUT PX → writes OUT whole (or not at all);
# _GD_BODY is the last answer without its bytes.
_gd_blob_get_file() {
    local widget="$1" ref="$2" out="$3" px="$4" offset=0 tmp eof q
    tmp="$(mktemp "${out}.k2part.XXXXXX")" || { echo "Error: can't write next to '$out'" >&2; exit 1; }
    # A refusal exits from _gd_call: never leave a half file behind.
    trap 'rm -f "$tmp"' EXIT
    while :; do
        q="$(GD_W="$widget" GD_R="$ref" GD_OFF="$offset" GD_N="$_GD_BLOB_CHUNK" GD_PX="$px" python3 -c '
import os, urllib.parse
e = os.environ
pairs = [("widget", e["GD_W"]), ("ref", e["GD_R"]), ("offset", e["GD_OFF"]), ("len", e["GD_N"])]
if e["GD_PX"]:
    pairs.append(("px", e["GD_PX"]))
print(urllib.parse.urlencode(pairs))
')"
        _gd_call GET /cli/zen/data/blob/get "$q"
        eof="$(printf '%s' "$_GD_BODY" | GD_T="$tmp" python3 -c '
import base64, json, os, sys
v = json.loads(sys.stdin.read())
with open(os.environ["GD_T"], "ab") as f:
    f.write(base64.b64decode(v["base64"]))
print("%s %s" % ("1" if v["eof"] else "0", v["len"]))
')" || { rm -f "$tmp"; echo "Error: the daemon's answer wasn't readable" >&2; exit 1; }
        offset=$((offset + ${eof#* }))
        [ "${eof%% *}" = "1" ] && break
        [ "${eof#* }" -gt 0 ] || { rm -f "$tmp"; echo "Error: the daemon stopped before the end" >&2; exit 1; }
    done
    mv -f "$tmp" "$out" || { rm -f "$tmp"; echo "Error: can't write '$out'" >&2; exit 1; }
    trap - EXIT
    _GD_BODY="$(printf '%s' "$_GD_BODY" | python3 -c 'import json, sys; v = json.loads(sys.stdin.read()); v.pop("base64", None); v.pop("offset", None); v.pop("len", None); v.pop("eof", None); print(json.dumps(v))')"
}

cmd_store_widget_blob() {
    local sub="${1:-}" pos="" mime="" name="" out="" px="" after="" limit=""
    shift || true
    while [ $# -gt 0 ]; do
        case "$1" in
            --mime) mime="${2:-}"; shift 2 || { echo "Error: --mime takes a type" >&2; exit 2; } ;;
            --name) name="${2:-}"; shift 2 || { echo "Error: --name takes a file name" >&2; exit 2; } ;;
            --out|-o) out="${2:-}"; shift 2 || { echo "Error: --out takes a path" >&2; exit 2; } ;;
            --px) px="${2:-}"; shift 2 || { echo "Error: --px takes 256 or 512" >&2; exit 2; } ;;
            --after) after="${2:-}"; shift 2 || { echo "Error: --after takes a ref" >&2; exit 2; } ;;
            --limit) limit="${2:-}"; shift 2 || { echo "Error: --limit takes a number" >&2; exit 2; } ;;
            --json) _GD_JSON=1; shift ;;
            --*) echo "Error: unknown flag $1" >&2; exit 2 ;;
            *) if [ -z "$pos" ]; then pos="$1"; shift; else echo "Error: unexpected '$1'" >&2; exit 2; fi ;;
        esac
    done
    case "$px" in ""|256|512) ;; *) echo "Error: --px is 256 or 512" >&2; exit 2 ;; esac
    case "$limit" in *[!0-9]*) echo "Error: --limit takes a whole number" >&2; exit 2 ;; esac
    case "$sub" in
        put)
            [ -n "$pos" ] || { echo "Error: usage: k2 store blob put <file> --widget <w> [--mime m] [--name n]" >&2; exit 2; }
            _gd_blob_put_file "$_GD_WIDGET" "$pos" "$mime" "$name"
            _gd_out 'import json, sys; print(json.loads(sys.stdin.read())["ref"])'
            ;;
        get)
            [ -n "$pos" ] && [ -n "$out" ] || { echo "Error: usage: k2 store blob get <ref> --widget <w> --out <file> [--px 256|512]" >&2; exit 2; }
            _gd_blob_get_file "$_GD_WIDGET" "$pos" "$out" "$px"
            _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("%s bytes, %s" % (v["size"], v["mime"]))'
            ;;
        ls|list)
            _gd_call GET /cli/zen/data/blob/list "$(_gd_q "after=$after" "limit=$limit")"
            _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
for b in v["items"]:
    flag = "pending" if b["pending"] else "%d key%s" % (b["refs"], "" if b["refs"] == 1 else "s")
    print("%s  %9d  %-24s %-10s %s" % (b["ref"], b["size"], b["mime"], flag, b["name"] or ""))
print("%d of %d MiB used" % (v["bytes"] // 1048576, v["quota"] // 1048576), file=sys.stderr)
if v["next"]:
    print("more: --after %s" % v["next"], file=sys.stderr)'
            ;;
        *)
            echo "Error: k2 store blob put|get|ls --widget <w> (see k2 store --help)" >&2
            exit 2 ;;
    esac
}

# 0.46.6 (prd-garden-board-needs-v1 §6, GH7): every change kept (7 days),
# restore and snapshots.
#   k2 store history  --widget <w> [--key k | --prefix p/] [--limit N] [--all] [--json]
#   k2 store restore  --widget <w> (--rev N | --at <when> | --snapshot <ref>)
#                     --prefix p/ [--prefix q/] [--except r/] [--dry-run] [--json]
#   k2 store snapshot --widget <w> [--prefix p/]… [--except r/]… [--label l] [--json]
#   k2 zen data restore <widget> (--rev N | --at <when> | --snapshot <ref>) [--prefix p/]… [--except r/]… [--dry-run]
# <when> is 2026-10-26T17:30 (this computer's local time) or seconds since 1970.

# _gd_restore_body WIDGET ARGS… → the restore or snapshot JSON body.
_gd_restore_body() {
    local kind="$1" widget="$2"
    shift 2
    GD_KIND="$kind" GD_W="$widget" python3 -c '
import json, os, re, sys
args = sys.argv[1:]
def die(m):
    sys.stderr.write("Error: %s\n" % m)
    sys.exit(2)
kind = os.environ["GD_KIND"]
b = {"widget": os.environ["GD_W"]}
pre, exc = [], []
i = 0
while i < len(args):
    a = args[i]
    def val():
        if i + 1 >= len(args):
            die("%s takes a value" % a)
        return args[i + 1]
    if a == "--prefix":
        pre.append(val()); i += 2
    elif a == "--except":
        exc.append(val()); i += 2
    elif a == "--label" and kind == "snapshot":
        b["label"] = val(); i += 2
    elif a == "--rev" and kind == "restore":
        v = val()
        if not v.isdigit():
            die("--rev takes a rev (a whole number; see k2 store history)")
        b["toRev"] = int(v); i += 2
    elif a == "--at" and kind == "restore":
        v = val()
        if v.isdigit():
            b["at"] = int(v)
        elif re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d(:\d\d)?$", v):
            b["atLocal"] = v
        else:
            die("--at takes 2026-10-26T17:30 (local time) or seconds since 1970")
        i += 2
    elif a == "--snapshot" and kind == "restore":
        b["snapshot"] = val(); i += 2
    elif a == "--dry-run" and kind == "restore":
        b["dryRun"] = True; i += 1
    else:
        die("unexpected %s (see k2 store --help)" % a)
if kind == "restore" and sum(k in b for k in ("toRev", "at", "atLocal", "snapshot")) != 1:
    die("name exactly one of --rev, --at or --snapshot")
if pre:
    b["prefixes"] = pre
if exc:
    b["except"] = exc
print(json.dumps(b))
' "$@"
}

_GD_RESTORE_OUT='
import json, sys
v = json.loads(sys.stdin.read())
fam = ", ".join(v["families"]) or "nothing"
if v["id"] is None and v["rev"] is None and (v["written"] or v["deleted"]):
    print("Would write %d and delete %d key(s) in %s:" % (v["written"], v["deleted"], fam))
    for k in v["keys"]:
        print("  " + k)
elif v["rev"] is None:
    print("Nothing to change.")
else:
    print("Restored: wrote %d, deleted %d (%s) as rev %d, restore %s." % (v["written"], v["deleted"], fam, v["rev"], v["id"]))
    print("Undo it with --rev %d." % (v["rev"] - 1), file=sys.stderr)'

cmd_store_widget_history() {
    local key="" prefix="" limit="" all=0 before="" after="" q
    while [ $# -gt 0 ]; do
        case "$1" in
            --key) key="${2-}"; shift 2 || { echo "Error: --key takes a key" >&2; exit 2; } ;;
            --prefix) prefix="${2-}"; shift 2 || { echo "Error: --prefix takes a prefix" >&2; exit 2; } ;;
            --limit) limit="${2:-}"; shift 2 || { echo "Error: --limit takes a number" >&2; exit 2; } ;;
            --before) before="${2:-}"; shift 2 || { echo "Error: --before takes a rev" >&2; exit 2; } ;;
            --all) all=1; shift ;;
            --json) _GD_JSON=1; shift ;;
            *) echo "Error: unexpected '$1' (k2 store history --widget <w> [--key k | --prefix p/] [--limit N] [--all])" >&2; exit 2 ;;
        esac
    done
    case "$limit$before" in *[!0-9]*) echo "Error: --limit and --before take whole numbers" >&2; exit 2 ;; esac
    while :; do
        q="$(_gd_q "key=$key" "prefix=$prefix" "limit=$limit" "before=$before" "after=$after")"
        _gd_call GET /cli/zen/data/history "$q"
        _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
for i in v["items"]:
    what = "deleted" if i["deleted"] else ("made" if i["prevKind"] == "none" else "changed")
    mark = "  (restore %s)" % i["restore"] if i.get("restore") else ""
    print("rev %-6d %-8s %s  by %s%s" % (i["rev"], what, i["key"], i["by"], mark))'
        [ "$all" = "1" ] || break
        before="$(printf '%s' "$_GD_BODY" | python3 -c 'import json, sys; n = json.loads(sys.stdin.read())["next"]; print(n["before"] if n else "")')"
        after="$(printf '%s' "$_GD_BODY" | python3 -c 'import json, sys; n = json.loads(sys.stdin.read())["next"]; print((n or {}).get("after", ""))')"
        [ -n "$before" ] || break
    done
    [ "${_GD_JSON:-0}" = "1" ] || printf '%s' "$_GD_BODY" | python3 -c '
import json, sys
v = json.loads(sys.stdin.read())
print("head rev %d; restorable back to rev %d (7 days of changes)" % (v["head"], v["oldest"]), file=sys.stderr)
if v["next"]:
    print("more: --before %d (or --all)" % v["next"]["before"], file=sys.stderr)'
}

cmd_store_widget_restore() {
    local body
    local -a pass=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _GD_JSON=1; shift ;;
            *) pass+=("$1"); shift ;;
        esac
    done
    body="$(_gd_restore_body restore "$_GD_WIDGET" ${pass[@]+"${pass[@]}"})" || exit 2
    _gd_call POST /cli/zen/data/restore "$body"
    _gd_out "$_GD_RESTORE_OUT"
}

cmd_store_widget_snapshot() {
    local body
    local -a pass=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _GD_JSON=1; shift ;;
            *) pass+=("$1"); shift ;;
        esac
    done
    body="$(_gd_restore_body snapshot "$_GD_WIDGET" ${pass[@]+"${pass[@]}"})" || exit 2
    _gd_call POST /cli/zen/data/snapshot "$body"
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
print(v["ref"])
print("%d key(s) at rev %d, %d bytes. Keep the ref under a key (it goes after 24 h otherwise)." % (v["keys"], v["rev"], v["bytes"]), file=sys.stderr)'
}

# The person's restore (any key set, the whole space too).
cmd_zen_data_restore() {
    local widget="" body
    local -a pass=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _GD_JSON=1; shift ;;
            --*) pass+=("$1"); [ "$1" = "--dry-run" ] && { shift; continue; }; pass+=("${2-}"); shift 2 || true ;;
            *) if [ -z "$widget" ]; then widget="$1"; shift; else echo "Error: unexpected '$1'" >&2; exit 2; fi ;;
        esac
    done
    [ -n "$widget" ] || { echo "Error: usage: k2 zen data restore <widget> (--rev N | --at 2026-10-26T17:30 | --snapshot <ref>) [--prefix p/]… [--except r/]… [--dry-run]" >&2; exit 2; }
    body="$(_gd_restore_body restore "$widget" ${pass[@]+"${pass[@]}"})" || exit 2
    _gd_call POST /cli/zen/data/restore "$body"
    _gd_out "$_GD_RESTORE_OUT"
}
# END_GARDEN_DATA_B1

# BEGIN_GARDEN_DATA_B3
# Export (contract §4.5, §8): page GET /cli/zen/data/export until `next` is
# null and write JSONL to stdout, the header line first. A person gets every
# key; an agent only keys under its current read prefixes (also for --at,
# which reads the newest daily snapshot on or before that date).
_gd_export_jsonl() {
    local widget="$1" at="$2" after="" first=1 q nextf
    nextf="$(mktemp "${TMPDIR:-/tmp}/k2-gd-next.XXXXXX")" || { echo "Error: can't make a temp file" >&2; exit 1; }
    while :; do
        q="$(GD_W="$widget" GD_AT="$at" GD_AFTER="$after" python3 -c '
import os, urllib.parse
pairs = [("widget", os.environ["GD_W"]), ("at", os.environ["GD_AT"]), ("after", os.environ["GD_AFTER"])]
print(urllib.parse.urlencode([(k, v) for k, v in pairs if v]))
')"
        _gd_call GET /cli/zen/data/export "$q"
        printf '%s' "$_GD_BODY" | GD_FIRST="$first" GD_NEXT="$nextf" python3 -c '
import json, os, sys
page = json.loads(sys.stdin.read())
out = sys.stdout
if os.environ["GD_FIRST"] == "1":
    out.write(json.dumps(page["header"], separators=(",", ":"), ensure_ascii=False) + "\n")
for it in page["items"]:
    row = {"key": it["key"], "rev": it["rev"], "value": it["value"], "updatedAt": it["updatedAt"], "by": it["by"]}
    out.write(json.dumps(row, separators=(",", ":"), ensure_ascii=False) + "\n")
with open(os.environ["GD_NEXT"], "w") as f:
    f.write(page.get("next") or "")
' || { rm -f "$nextf"; echo "Error: the export page wasn't readable JSON" >&2; exit 1; }
        first=0
        after="$(cat "$nextf")"
        [ -n "$after" ] || break
    done
    rm -f "$nextf"
}

# _gd_take_at ARGS… → sets _GD_AT; refuses anything else (usage $1).
_gd_take_at() {
    local usage="$1"
    shift
    _GD_AT=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --at)
                _GD_AT="${2:-}"
                [ -n "$_GD_AT" ] || { echo "Error: --at takes a date like 2026-10-13" >&2; exit 2; }
                shift 2 ;;
            --json) shift ;;
            *) echo "Error: unexpected '$1' ($usage)" >&2; exit 2 ;;
        esac
    done
    if [ -n "$_GD_AT" ] && ! [[ "$_GD_AT" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
        echo "Error: --at takes a date like 2026-10-13 (YYYY-MM-DD, this computer's time zone)" >&2
        exit 2
    fi
}

cmd_store_widget_export() {
    _gd_take_at "k2 store export --widget <w> [--at YYYY-MM-DD] > data.jsonl" "$@"
    _gd_export_jsonl "$_GD_WIDGET" "$_GD_AT"
}

# `k2 study garden-data` (contract-garden-data-v1 §8): bounded, daemon-optional.
_study_page_design() {
    cat <<'EOF'
%include cli/text/study/design.txt
EOF
}

# Permission cards (Beta). Reached only while both switches are on (see
# _study_topic_ids_all). The same text is the k2-permissions skill
# (k2-core skills/content.rs K2_PERMISSIONS_SKILL_BODY); a test keeps the
# two byte-identical, so edit both.
_study_page_permissions() {
    cat <<'EOF'
%include cli/text/study/permissions.txt
EOF
}

# Garden access lists (prd-garden-access-v1 G24): how Gardens work since
# 0.46.3. Static docs, always listed (no daemon asked).
_study_page_garden_access() {
    cat <<'EOF'
%include cli/text/study/garden-access.txt
EOF
}

# Garden access (prd-garden-access-v1), 0.46.3: what changed, what's
# retired (with the real verbs and cards) and what an agent updates in its
# workspace before 0.47.0 removes the retired verbs. Static docs.
_study_page_garden_changes() {
    cat <<'EOF'
%include cli/text/study/garden-changes.txt
EOF
}

_study_page_garden_data() {
    cat <<'EOF'
%include cli/text/study/garden-data.txt
EOF
}
# END_GARDEN_DATA_B3

# BEGIN_ZEN_DATA_B1
# `k2 zen data <verb> <widget> …`: the widget is the first positional.
_gd_zen_widget() {
    _GD_WIDGET="$_GD_POS"
    [ -n "$_GD_WIDGET" ] || { echo "Error: usage: k2 zen data $1 <widget> …" >&2; exit 2; }
}

cmd_zen_data_list() {
    _gd_args "$@"
    _gd_call GET /cli/zen/data/spaces ""
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
if not v["spaces"] and not v["saves"]:
    print("No widget data on this computer yet.")
def size(n):
    return "%d B" % n if n < 1024 else ("%.1f KB" % (n / 1024) if n < 1048576 else "%.1f MB" % (n / 1048576))
for s in v["spaces"]:
    m = s.get("madeBy") or {}
    made = {"owner": "you", "k2": "K2", "unclaimed": "unsigned"}.get(m.get("kind"), m.get("handle") or "-")
    flags = " (no widget)" if s.get("orphanedAt") else ""
    admins = (" admins: " + ", ".join(s["admins"])) if s["admins"] else ""
    print("%-24s %8s %6d keys  %d grants  made by %s%s%s" % (s["widget"], size(s["bytes"]), s["keys"], s["grants"], made, admins, flags))
for s in v["saves"]:
    print("%-24s %8s %6d saved keys (private)" % (s["widget"], size(s["bytes"]), s["keys"]))'
}

cmd_zen_data_show() {
    _gd_args "$@"; _gd_zen_widget show
    _gd_call GET /cli/zen/data/show "$(_gd_q)"
    _gd_out '
import json, sys
v = json.loads(sys.stdin.read())
s = v["space"]
print("%s: %d keys, %d bytes, rev %d%s" % (s["widget"], s["keys"], s["bytes"], s["rev"], " (no widget folder)" if s.get("orphanedAt") else ""))
m = v.get("madeBy") or {}
print("made by: %s" % ({"owner": "you", "k2": "K2", "unclaimed": "unsigned"}.get(m.get("kind"), m.get("handle") or "-")))
for g in v["grants"]:
    by = "" if g["grantedBy"] == "owner" else " (by %s)" % g["grantedBy"]
    print("  %-8s %-24s %-5s %s%s%s" % (g["subjectKind"], g["subjectName"], g["level"], g["prefix"] or "(everything)", " ADMIN" if g["admin"] else "", by))'
}

cmd_zen_data_grant() {
    _gd_args "$@"; _gd_zen_widget grant
    local body; body="$(_gd_body grant)" || exit 2
    _gd_call POST /cli/zen/data/grant "$body"
    _gd_project_note
    _gd_out 'import json, sys; g = json.loads(sys.stdin.read())["grant"]; print("Granted %s on %s to %s %s" % (g["level"], g["prefix"] or "everything", g["subjectKind"], g["subjectId"]))'
}

cmd_zen_data_revoke() {
    _gd_args "$@"; _gd_zen_widget revoke
    local body; body="$(_gd_body revoke)" || exit 2
    _gd_call POST /cli/zen/data/revoke "$body"
    _gd_out 'print("Revoked.")'
}

cmd_zen_data_admin() {
    _gd_args "$@"; _gd_zen_widget admin
    local body; body="$(_gd_body admin)" || exit 2
    _gd_call POST /cli/zen/data/admin "$body"
    _gd_out 'import json, sys; g = json.loads(sys.stdin.read())["grant"]; print("%s is now grant admin (%s) on %s. It can grant others inside that, up to %s, never itself." % (g["subjectId"], g["level"], g["prefix"] or "everything", g["level"]))'
}

cmd_zen_data_unadmin() {
    _gd_args "$@"; _gd_zen_widget unadmin
    local body; body="$(_gd_body unadmin dry)" || exit 2
    _gd_call POST /cli/zen/data/admin/revoke "$body"
    local n; n="$(printf '%s' "$_GD_BODY" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["cascaded"])')"
    if [ "$_GD_YES" != "1" ]; then
        printf '%s grant(s) it made go with it. Go ahead? [y/N] ' "$n" >&2
        local answer=""; read -r answer || true
        case "$answer" in y|Y|yes) ;; *) echo "Nothing changed." >&2; exit 1 ;; esac
    fi
    body="$(_gd_body unadmin)" || exit 2
    _gd_call POST /cli/zen/data/admin/revoke "$body"
    _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("Grant admin ended; %s grant(s) it made went with it." % v["cascaded"])'
}

cmd_zen_data_delete() {
    _gd_args "$@"; _gd_zen_widget delete
    if [ "$_GD_YES" != "1" ]; then
        printf 'Delete all of %s'"'"'s data and its grants (a snapshot is kept)? Type the widget name: ' "$_GD_WIDGET" >&2
        local answer=""; read -r answer || true
        [ "$answer" = "$_GD_WIDGET" ] || { echo "Nothing changed." >&2; exit 1; }
    fi
    local body; body="$(K2_GD_W="$_GD_WIDGET" python3 -c 'import json,os; w=os.environ["K2_GD_W"]; print(json.dumps({"widget": w, "confirm": w}))')"
    _gd_call POST /cli/zen/data/space/delete "$body"
    _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("Deleted %s (%s grants)%s" % (v["deleted"], v["grantsRemoved"], "; snapshot " + v["snapshot"] if v.get("snapshot") else ""))'
}

cmd_zen_data_move() {
    _gd_args "$@"
    [ -n "$_GD_POS" ] && [ -n "$_GD_POS2" ] || { echo "Error: usage: k2 zen data move <old> <new>" >&2; exit 2; }
    local body; body="$(K2_GD_A="$_GD_POS" K2_GD_B="$_GD_POS2" python3 -c 'import json,os; print(json.dumps({"from": os.environ["K2_GD_A"], "to": os.environ["K2_GD_B"]}))')"
    _gd_call POST /cli/zen/data/space/move "$body"
    _gd_out 'import json, sys; v = json.loads(sys.stdin.read()); print("Moved, with %s grant(s)." % v["grantsMoved"])'
}
# END_ZEN_DATA_B1

# BEGIN_ZEN_DATA_B3
# --blobs <dir> (prd-garden-board-needs-v1 GB17): export also copies every
# file a key in the export names into <dir>/<hash> (read-only); import
# uploads the files in <dir> first, so the refs in the data are held.
_gd_take_blobs() {
    _GD_BLOBS="" _GD_REST=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --blobs) _GD_BLOBS="${2:-}"; [ -n "$_GD_BLOBS" ] || { echo "Error: --blobs takes a folder" >&2; exit 2; }; shift 2 ;;
            *) _GD_REST+=("$1"); shift ;;
        esac
    done
}

cmd_zen_data_export() {
    local widget="${1:-}" tmp ref
    case "$widget" in ""|-*) echo "Error: k2 zen data export <widget> [--at YYYY-MM-DD] [--blobs <dir>] > data.jsonl" >&2; exit 2 ;; esac
    shift
    _gd_take_blobs "$@"
    _gd_take_at "k2 zen data export <widget> [--at YYYY-MM-DD] [--blobs <dir>]" ${_GD_REST[@]+"${_GD_REST[@]}"}
    if [ -z "$_GD_BLOBS" ]; then
        _gd_export_jsonl "$widget" "$_GD_AT"
        return 0
    fi
    mkdir -p "$_GD_BLOBS" || { echo "Error: can't make '$_GD_BLOBS'" >&2; exit 1; }
    tmp="$(mktemp "${TMPDIR:-/tmp}/k2-gd-export.XXXXXX")" || { echo "Error: can't make a temp file" >&2; exit 1; }
    _gd_export_jsonl "$widget" "$_GD_AT" > "$tmp"
    cat "$tmp"
    for ref in $(python3 -c '
import json, re, sys
REF = re.compile(r"^k2blob:sha256-[0-9a-f]{64}$")
found = set()
def walk(v):
    if isinstance(v, str):
        if REF.match(v):
            found.add(v)
    elif isinstance(v, list):
        for x in v:
            walk(x)
    elif isinstance(v, dict):
        for x in v.values():
            walk(x)
with open(sys.argv[1]) as f:
    for n, line in enumerate(f):
        if n == 0 or not line.strip():
            continue
        walk(json.loads(line).get("value"))
for r in sorted(found):
    print(r)
' "$tmp"); do
        local hash="${ref#k2blob:sha256-}"
        [ -f "$_GD_BLOBS/$hash" ] && continue
        _gd_blob_get_file "$widget" "$ref" "$_GD_BLOBS/$hash" ""
        chmod 0444 "$_GD_BLOBS/$hash"
    done
    rm -f "$tmp"
}

# k2 zen data import <widget> [--replace] < data.jsonl: the person only. A
# snapshot first, then one write at one new rev. Reads the JSONL an export
# writes (the header line is skipped) or a JSON array of {key, value}.
cmd_zen_data_import() {
    local widget="${1:-}" mode="merge" body blobs="" f
    case "$widget" in ""|-*) echo "Error: k2 zen data import <widget> [--replace] [--blobs <dir>] < data.jsonl" >&2; exit 2 ;; esac
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
            --replace) mode="replace"; shift ;;
            --merge) mode="merge"; shift ;;
            --blobs) blobs="${2:-}"; [ -d "$blobs" ] || { echo "Error: --blobs takes a folder of files named by their sha256" >&2; exit 2; }; shift 2 ;;
            --json) shift ;;
            *) echo "Error: unexpected '$1' (k2 zen data import <widget> [--replace] < data.jsonl)" >&2; exit 2 ;;
        esac
    done
    if [ -t 0 ]; then
        echo "Error: pipe the data in: k2 zen data import $widget < data.jsonl" >&2
        exit 2
    fi
    body="$(GD_W="$widget" GD_MODE="$mode" python3 -c '
import json, os, sys
text = sys.stdin.read()
items = []
s = text.lstrip()
if s.startswith("["):
    rows = json.loads(s)
else:
    rows = []
    for n, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except ValueError:
            sys.stderr.write("Error: line %d is not JSON\n" % n)
            sys.exit(2)
for n, r in enumerate(rows, 1):
    if not isinstance(r, dict):
        sys.stderr.write("Error: row %d is not a JSON object\n" % n)
        sys.exit(2)
    if "space" in r and "key" not in r:
        continue
    if "key" not in r or "value" not in r:
        sys.stderr.write("Error: row %d needs \"key\" and \"value\"\n" % n)
        sys.exit(2)
    items.append({"key": r["key"], "value": r["value"]})
print(json.dumps({"widget": os.environ["GD_W"], "mode": os.environ["GD_MODE"], "items": items}, separators=(",", ":"), ensure_ascii=False))
')" || exit $?
    if [ -n "$blobs" ]; then
        for f in "$blobs"/*; do
            [ -f "$f" ] || continue
            local base; base="$(basename "$f")"
            case "$base" in *[!0-9a-f]*) continue ;; esac
            [ "${#base}" -eq 64 ] || continue
            _gd_blob_put_file "$widget" "$f" "" ""
            if [ "$(printf '%s' "$_GD_BODY" | python3 -c 'import json, sys; print(json.loads(sys.stdin.read())["ref"])')" != "k2blob:sha256-$(basename "$f")" ]; then
                echo "Error: $(basename "$f") isn't named by its sha256; nothing was imported" >&2
                exit 1
            fi
        done
    fi
    _gd_call POST /cli/zen/data/import "$body"
    if [ "$_GD_JSON" = "1" ]; then
        printf '%s\n' "$_GD_BODY"
        return 0
    fi
    printf '%s' "$_GD_BODY" | GD_W="$widget" python3 -c '
import json, os, sys
r = json.loads(sys.stdin.read())
print("Imported %d key(s) into %s at rev %d; %d deleted. A snapshot was taken first." % (r["written"], os.environ["GD_W"], r["rev"], r["deleted"]))
'
}
# END_ZEN_DATA_B3

