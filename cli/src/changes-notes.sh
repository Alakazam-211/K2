# ── k2 changes notes (C5 review notes) ──────────────────────────────

_changes_notes_usage() {
    cat <<'EOF'
k2 changes notes — review notes people left on changed lines (the Changes tab)

  k2 changes notes [list] [--status open|unsent|sent|resolved|all] [--file <path>] [--json]
                                      the notes on this workspace (default: open)
  k2 changes notes resolve <id>...    mark notes fixed (agents: the notes left for you)
  k2 changes notes reopen <id>...     people only: open a resolved note again
  k2 changes notes add <path> --line <n>[-<m>] [--old] -m "comment"
                                      people only: a note on lines of a changed file
                                      (--old = the text before the change)
  k2 changes notes edit <id> -m "comment"     people only (author or Admin)
  k2 changes notes delete <id>...             people only (author or Admin)
  k2 changes notes send [<id>...] [--no-wake] people only: send the open unsent
                                      notes to this workspace's agent as one message

The loop for an agent: notes arrive as one message (File / Line / Comment /
Note id). Fix each one, then `k2 changes notes resolve <id>`. Every command
takes --workspace <path> and --json. Exit 3 + code when the daemon refuses
(forbidden, not_found, not_author, nothing_to_send, not_delivered, …).
EOF
}

# _changes_json KEY=VALUE... → a JSON object; `path` from _CH_PATH when set.
# A KEY ending in [] collects a list, a KEY ending in # is a number, ? a bool.
_changes_json() {
    python3 - "$_CH_PATH" "$@" <<'PY'
import json, sys
path, pairs = sys.argv[1], sys.argv[2:]
d = {}
if path:
    d["path"] = path
for p in pairs:
    k, _, v = p.partition("=")
    if k.endswith("[]"):
        d.setdefault(k[:-2], []).append(v)
    elif k.endswith("#"):
        d[k[:-1]] = int(v)
    elif k.endswith("?"):
        d[k[:-1]] = v == "1"
    else:
        d[k] = v
print(json.dumps(d))
PY
}

_changes_notes_list() {
    local ws="" status="" file="" q
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --status) [ $# -ge 2 ] || _changes_die usage "--status needs open|unsent|sent|resolved|all" 2; status="$2"; shift 2 ;;
            --status=*) status="${1#*=}"; shift ;;
            --all) status="all"; shift ;;
            --file) [ $# -ge 2 ] || _changes_die usage "--file needs a path" 2; file="$2"; shift 2 ;;
            --file=*) file="${1#*=}"; shift ;;
            *) _changes_die usage "unknown argument for k2 changes notes: $1 (see k2 changes notes --help)" 2 ;;
        esac
    done
    _changes_target "$ws"
    q="$(_changes_q)"
    [ -n "$status" ] && q="${q}status=$(urlencode "$status")&"
    if [ -n "$file" ]; then
        _changes_files "$file"
        q="${q}file_path=$(urlencode "${_CH_FILES[0]}")&"
    fi
    _changes_call GET /cli/git/notes "${q%&}"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
notes, c = r.get("notes") or [], r.get("counts") or {}
print("Review notes on %s: %d open (%d not sent yet), %d resolved" % (
    r.get("workspacePath", ""), c.get("open", 0), c.get("unsent", 0), c.get("resolved", 0)))
if not notes:
    print("No notes to show.")
    sys.exit(0)
for n in notes:
    s = n.get("currentStartLine") or n.get("startLine")
    e = n.get("currentEndLine") or n.get("endLine")
    where = "%s:%s" % (n.get("path"), s if s == e else "%s-%s" % (s, e))
    if n.get("side") == "old":
        where += " (before the change)"
    tags = []
    if n.get("resolvedAt"):
        tags.append("resolved by %s" % (n.get("resolvedBy") or "?"))
    elif n.get("sentAt"):
        tags.append("sent")
    if n.get("outdated") and not n.get("resolvedAt"):
        tags.append("outdated: the lines changed")
    print()
    print("%s  %s  by %s%s" % (n.get("id"), where, n.get("authorName") or n.get("author"), ("  [" + ", ".join(tags) + "]") if tags else ""))
    for line in (n.get("body") or "").splitlines() or [""]:
        print("    " + line)
'
}

_changes_notes_ids() {
    local verb="$1" route="$2" extra="$3"; shift 3
    local ws="" ids=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            -*) _changes_die usage "unknown flag for k2 changes notes $verb: $1" 2 ;;
            *) ids+=("ids[]=$1"); shift ;;
        esac
    done
    [ "${#ids[@]}" -gt 0 ] || _changes_die usage "usage: k2 changes notes $verb <note id>..." 2
    _changes_target "$ws"
    if [ -n "$extra" ]; then
        _changes_call POST "$route" "$(_changes_json "${ids[@]}" "$extra")"
    else
        _changes_call POST "$route" "$(_changes_json "${ids[@]}")"
    fi
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
verb, r = sys.argv[1], json.load(sys.stdin)
if verb == "delete":
    print("Deleted %s" % ", ".join(r.get("deleted") or []))
else:
    done = r.get("changed") or []
    same = r.get("unchanged") or []
    word = "Resolved" if verb == "resolve" else "Reopened"
    if done:
        print("%s %s" % (word, ", ".join(done)))
    if same:
        print("Already %s: %s" % ("resolved" if verb == "resolve" else "open", ", ".join(same)))
' "$verb"
}

_changes_notes_add() {
    local ws="" file="" lines="" side="new" msg="" have_msg=0 start end
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --line|--lines|-L) [ $# -ge 2 ] || _changes_die usage "--line needs <n> or <n>-<m>" 2; lines="$2"; shift 2 ;;
            --line=*|--lines=*) lines="${1#*=}"; shift ;;
            --old) side="old"; shift ;;
            -m|--message) [ $# -ge 2 ] || _changes_die usage "-m needs the comment" 2
                if [ "$have_msg" = "1" ]; then msg="${msg}"$'\n\n'"$2"; else msg="$2"; fi
                have_msg=1; shift 2 ;;
            -*) _changes_die usage "unknown flag for k2 changes notes add: $1" 2 ;;
            *) [ -z "$file" ] || _changes_die usage "k2 changes notes add takes one file" 2; file="$1"; shift ;;
        esac
    done
    if [ -z "$file" ] || [ -z "$lines" ] || [ "$have_msg" != "1" ]; then
        _changes_die usage "usage: k2 changes notes add <path> --line <n>[-<m>] [--old] -m \"comment\"" 2
    fi
    case "$lines" in
        *-*) start="${lines%%-*}"; end="${lines#*-}" ;;
        *) start="$lines"; end="$lines" ;;
    esac
    [[ "$start" =~ ^[0-9]+$ && "$end" =~ ^[0-9]+$ ]] || _changes_die usage "--line takes <n> or <n>-<m> (numbers)" 2
    _changes_target "$ws"
    _changes_files "$file"
    _changes_call POST /cli/git/notes/add "$(_changes_json "filePath=${_CH_FILES[0]}" "side=$side" "startLine#=$start" "endLine#=$end" "body=$msg")"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
n = json.load(sys.stdin).get("note") or {}
s, e = n.get("startLine"), n.get("endLine")
print("Added note %s on %s:%s" % (n.get("id"), n.get("path"), s if s == e else "%s-%s" % (s, e)))'
}

_changes_notes_edit() {
    local ws="" id="" msg="" have_msg=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            -m|--message) [ $# -ge 2 ] || _changes_die usage "-m needs the comment" 2
                if [ "$have_msg" = "1" ]; then msg="${msg}"$'\n\n'"$2"; else msg="$2"; fi
                have_msg=1; shift 2 ;;
            -*) _changes_die usage "unknown flag for k2 changes notes edit: $1" 2 ;;
            *) [ -z "$id" ] || _changes_die usage "k2 changes notes edit takes one note id" 2; id="$1"; shift ;;
        esac
    done
    [ -n "$id" ] && [ "$have_msg" = "1" ] || _changes_die usage "usage: k2 changes notes edit <id> -m \"comment\"" 2
    _changes_target "$ws"
    _CH_PATH=""
    _changes_call POST /cli/git/notes/update "$(_changes_json "id=$id" "body=$msg")"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    echo "Updated note $id"
}

_changes_notes_send() {
    local ws="" wake=1 ids=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _CH_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _changes_die usage "--workspace needs a path" 2; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            --no-wake) wake=0; shift ;;
            -*) _changes_die usage "unknown flag for k2 changes notes send: $1" 2 ;;
            *) ids+=("ids[]=$1"); shift ;;
        esac
    done
    _changes_target "$ws"
    [ -z "$_CH_PATH" ] && _CH_PATH="$PROJECT"
    _changes_call POST /cli/git/notes/send "$(_changes_json ${ids[@]+"${ids[@]}"} "wake?=$wake")"
    if [ "${_CH_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_CH_BODY"
        return 0
    fi
    printf '%s' "$_CH_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
n = r.get("sent", 0)
line = "Sent %d note%s to the agent of %s" % (n, "" if n == 1 else "s", r.get("target", ""))
if r.get("deliveryId"):
    line += " (delivery %s)" % r["deliveryId"]
print(line)
if r.get("remaining"):
    print("%d more did not fit in one message; send again." % r["remaining"])'
}

_changes_notes() {
    local sub="${1:-}"
    case "$sub" in
        -h|--help|help) _changes_notes_usage; return 0 ;;
    esac
    case "$sub" in
        ""|-*) _changes_notes_list "$@" ;;
        list|ls) shift; _changes_notes_list "$@" ;;
        resolve|done) shift; _changes_notes_ids resolve /cli/git/notes/resolve "" "$@" ;;
        reopen) shift; _changes_notes_ids reopen /cli/git/notes/resolve "resolved?=0" "$@" ;;
        delete|rm) shift; _changes_notes_ids delete /cli/git/notes/delete "" "$@" ;;
        add) shift; _changes_notes_add "$@" ;;
        edit|update) shift; _changes_notes_edit "$@" ;;
        send) shift; _changes_notes_send "$@" ;;
        *)
            echo "Error: unknown k2 changes notes command: $sub (list, add, resolve, reopen, edit, delete, send)" >&2
            _changes_notes_usage >&2
            exit 2 ;;
    esac
}

cmd_changes() {
    _CH_JSON=0
    local sub="${1:-}"
    if _wants_help "$@"; then
        case "$sub" in
            notes|note) _changes_notes_usage ;;
            *) _changes_usage ;;
        esac
        return 0
    fi
    case "$sub" in
        ""|-*) _changes_list "$@" ;;
        list|ls|status) shift; _changes_list "$@" ;;
        diff) shift; _changes_diff "$@" ;;
        stage|add) shift; _changes_stage stage "$@" ;;
        unstage) shift; _changes_stage unstage "$@" ;;
        discard) shift; _changes_discard "$@" ;;
        commit) shift; _changes_commit "$@" ;;
        notes|note) shift; _changes_notes "$@" ;;
        open) shift; _changes_open "$@" ;;
        init) shift; _changes_init "$@" ;;
        help) _changes_usage ;;
        *)
            echo "Error: unknown k2 changes command: $sub (list, diff, stage, unstage, discard, commit, notes, open, init)" >&2
            _changes_usage >&2
            exit 2 ;;
    esac
}

# BEGIN_CLI_SIM
