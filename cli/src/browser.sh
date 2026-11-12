# ── Design mode: k2 browser note (prd-browser-design-mode-v1 D-A13) ───────
# Files a page note into a workspace's Thread exactly like a Design tab's
# Send: ONE Thread item (design origin) that wakes + injects the pinned
# Chat. Agents post only on their own workspace (passport); people need a
# login (Connect logins also need the workspace's remote-instruct opt-in).
cmd_help_browser() {
    cat <<'EOF'
Usage:
  k2 browser note --url <page-url> [--workspace <ws>] [--selector <css>]
                  [--html-file <file>] [--text <element text>] [--title <page title>]
                  [--intent change|question] [--json] "what to change"
  k2 browser redeliver <threadItemId> [--workspace <ws>] [--json]

note        Post one Design note (same route as a Design tab's Send:
            POST /cli/browser/notes). The note lands in the workspace's
            Thread and is delivered to its pinned Chat once. The page URL
            loses its query and hash; HTML is cleaned (scripts, handlers and
            secret-looking attributes removed) and clamped by the daemon.
            Without --workspace an agent posts to its own workspace.
redeliver   Deliver a stored notes post again when the first delivery
            failed ("delivered": false). Never posts a second Thread item.

Exit codes: 0 ok · 1 refused or failed · 2 usage.
EOF
}

cmd_browser() {
    case "${1:-}" in
        note)       shift; cmd_browser_note "$@" ;;
        redeliver)  shift; cmd_browser_redeliver "$@" ;;
        ""|-h|--help|help) cmd_help_browser ;;
        *)
            echo "Error: unknown browser subcommand '${1}'" >&2
            cmd_help_browser >&2
            exit 2
            ;;
    esac
}

# Print a /cli/browser/notes reply; exit 1 on an error body.
_browser_notes_reply() {
    local json="$1" raw="$2"
    printf '%s' "$raw" | python3 -c '
import json, sys
as_json = sys.argv[1] == "true"
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err or not d.get("ok"):
    if isinstance(err, dict):
        err = err.get("hint") or err.get("code") or err
    msg = d.get("message") or err or raw[:200]
    print("Error: %s" % msg, file=sys.stderr)
    sys.exit(1)
if as_json:
    print(json.dumps(d))
    sys.exit(0)
if d.get("delivered"):
    print("Sent to %s (Thread item %s)." % (d.get("addr"), d.get("threadItemId")))
else:
    print("Posted to the %s Thread (item %s) but not delivered to the agent yet." % (d.get("addr"), d.get("threadItemId")))
    print("Retry: k2 browser redeliver %s --workspace %s" % (d.get("threadItemId"), d.get("addr")))
' "$json"
}

cmd_browser_note() {
    local workspace="" url="" selector="" html_file="" text="" title="" intent="change" json="false" comment=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace|-w) workspace="${2:-}"; shift 2 ;;
            --url) url="${2:-}"; shift 2 ;;
            --selector) selector="${2:-}"; shift 2 ;;
            --html-file) html_file="${2:-}"; shift 2 ;;
            --text) text="${2:-}"; shift 2 ;;
            --title) title="${2:-}"; shift 2 ;;
            --intent) intent="${2:-}"; shift 2 ;;
            --json) json="true"; shift ;;
            -h|--help) cmd_help_browser; return 0 ;;
            --*) echo "Error: unknown flag '$1'" >&2; cmd_help_browser >&2; exit 2 ;;
            *)
                if [ -n "$comment" ]; then
                    echo "Error: one note text only (quote it)" >&2
                    exit 2
                fi
                comment="$1"; shift ;;
        esac
    done
    if [ -z "$url" ] || [ -z "$comment" ]; then
        echo "Error: --url and the note text are required" >&2
        cmd_help_browser >&2
        exit 2
    fi
    case "$intent" in
        change|question) ;;
        *) echo "Error: --intent is change or question, got: $intent" >&2; exit 2 ;;
    esac
    if [ -n "$html_file" ] && [ ! -r "$html_file" ]; then
        echo "Error: cannot read --html-file $html_file" >&2
        exit 2
    fi
    [ -z "$workspace" ] && workspace="${PROJECT:-}"
    local body
    body=$(python3 -c '
import json, sys
ws, url, selector, html_file, text, title, intent, comment = sys.argv[1:9]
note = {"comment": comment, "intent": intent}
if selector or html_file or text:
    html = ""
    if html_file:
        with open(html_file, encoding="utf-8", errors="replace") as f:
            html = f.read(64 * 1024)
    note["payload"] = {"selector": selector, "html": html, "text": text}
print(json.dumps({"workspace": ws, "page": {"url": url, "title": title}, "notes": [note]}))
' "$workspace" "$url" "$selector" "$html_file" "$text" "$title" "$intent" "$comment") || exit 2
    local raw
    raw="$(cli_send cli_post_json "/cli/browser/notes" "$body")"
    _browser_notes_reply "$json" "$raw"
}

cmd_browser_redeliver() {
    local workspace="" item="" json="false"
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace|-w) workspace="${2:-}"; shift 2 ;;
            --json) json="true"; shift ;;
            -h|--help) cmd_help_browser; return 0 ;;
            --*) echo "Error: unknown flag '$1'" >&2; exit 2 ;;
            *) item="$1"; shift ;;
        esac
    done
    if [ -z "$item" ]; then
        echo "Error: the Thread item id is required" >&2
        cmd_help_browser >&2
        exit 2
    fi
    [ -z "$workspace" ] && workspace="${PROJECT:-}"
    local body
    body=$(python3 -c 'import json,sys; print(json.dumps({"workspace": sys.argv[1], "threadItemId": sys.argv[2]}))' "$workspace" "$item")
    local raw
    raw="$(cli_send cli_post_json "/cli/browser/notes/redeliver" "$body")"
    _browser_notes_reply "$json" "$raw"
}

# The workspace's pinned Design and Mobile tabs (Workspace settings → Tabs):
#   k2 workspace design-tab get|set <ws> [on|off]        (0179; alias design-mode)
#   k2 workspace mobile-tab get|set <ws> [on|off|auto]   (0250; NULL = auto)
# Both write `/cli/workspace/set`; every client of the room follows.
_workspace_pinned_tab_help() {
    case "$1" in
        design)
            cat <<'EOF'
Usage: k2 workspace design-tab get <workspace>
       k2 workspace design-tab set <workspace> on|off

"Show the Design tab": keeps the workspace's Design tab pinned next to
Chat and Inbox, for every person and window on this server. Default on.
Off removes the pinned tab. A Design tab can still be opened from the
+ menu while it is off (an ordinary tab you can close).
`get` prints `on` or `off` (`on` when nobody has set it).
`design-mode` is the old name of this command and still works.

Examples:
  k2 workspace design-tab get my-site
  k2 workspace design-tab set my-site off
EOF
            ;;
        mobile)
            cat <<'EOF'
Usage: k2 workspace mobile-tab get <workspace>
       k2 workspace mobile-tab set <workspace> on|off|auto

"Show the Mobile tab": keeps the workspace's Mobile tab (its phone) pinned
next to Chat and Inbox, for every person and window on this server.
  auto  the default: pinned whenever this server can run a phone
  on    pinned (still only where this server can run a phone); today it
        acts like auto, it just records that someone chose it
  off   not pinned; the Mobile tab still opens from + or Cmd+Shift+M
        as an ordinary tab you can close
`get` prints the setting, then whether the tab shows on this server:
  auto (shown)
  off (not shown)
  on (not shown: this server cannot run a phone)

Examples:
  k2 workspace mobile-tab get my-app
  k2 workspace mobile-tab set my-app off
  k2 workspace mobile-tab set my-app auto
EOF
            ;;
    esac
}

cmd_workspace_pinned_tab() {
    local kind="$1" name="$2"
    shift 2
    local sub="${1:-}"
    shift || true
    local target="${1:-}"
    local words="on|off"
    [ "$kind" = "mobile" ] && words="on|off|auto"
    case "$sub" in
        get|set)
            if [ -z "$target" ]; then
                echo "Usage: k2 workspace $name get|set <workspace> [$words]" >&2
                exit 2
            fi
            ;;
        ""|-h|--help|help)
            _workspace_pinned_tab_help "$kind" >&2
            [ -z "$sub" ] && exit 2
            return 0
            ;;
        *) echo "Usage: k2 workspace $name get|set <workspace> [$words]" >&2; exit 2 ;;
    esac
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    [ -z "$resolved_path" ] && resolved_path="$target"
    if [ "$sub" = "get" ]; then
        cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
kind = sys.argv[1]
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    print("Error: %s" % (err.get("hint") or err.get("code") or err if isinstance(err, dict) else err), file=sys.stderr)
    sys.exit(1)
if kind == "design":
    if "designModeEnabled" not in d:
        print("Error: this daemon does not know the Design tab setting (update it)", file=sys.stderr)
        sys.exit(1)
    print("on" if d["designModeEnabled"] else "off")
    sys.exit(0)
if "mobileTab" not in d or "mobileTabShown" not in d:
    print("Error: this daemon does not know the Mobile tab setting (update it)", file=sys.stderr)
    sys.exit(1)
word = d["mobileTab"]
if d["mobileTabShown"]:
    print("%s (shown)" % word)
elif word == "off":
    print("off (not shown)")
else:
    print("%s (not shown: this server cannot run a phone)" % word)
' "$kind"
        return
    fi
    local mode="${2:-}" val=""
    case "$(printf '%s' "$mode" | tr '[:upper:]' '[:lower:]')" in
        on|1|true|yes)  val="1" ;;
        off|0|false|no) val="0" ;;
        auto|default)
            if [ "$kind" = "mobile" ]; then val="auto"; fi
            ;;
    esac
    if [ -z "$val" ]; then
        echo "Error: expected $words, got: ${mode:-<nothing>}" >&2
        exit 2
    fi
    local field="design_mode_enabled" label="Design tab"
    if [ "$kind" = "mobile" ]; then field="mobile_tab_enabled"; label="Mobile tab"; fi
    local body
    body=$(python3 -c 'import json,sys
print(json.dumps({"project": sys.argv[1], "fields": {sys.argv[2]: sys.argv[3]}}))' "$resolved_path" "$field" "$val")
    cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
field, val, label = sys.argv[1], sys.argv[2], sys.argv[3]
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        err = err.get("hint") or err.get("code") or err
    print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
state = {"1": "on", "0": "off", "auto": "auto"}[val]
for a in d.get("actions") or []:
    if a.get("field") == field and a.get("status") == "skipped":
        print("%s already %s (unchanged)." % (label, state))
        sys.exit(0)
print("%s %s." % (label, state))
' "$field" "$val" "$label"
}

# k2 thread export|import|imports (prd-clone-carries-threads-and-vault-v1
# T4): move a workspace's Threads outside a clone. People only (Owner/Admin);
# imports land read-only and re-runs are no-ops.
cmd_thread_transfer() {
    local sub="" ws="" path="" to="" since="" json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            export|import|imports) [ -z "$sub" ] && sub="$1" || ws="$1"; shift ;;
            --to) to="${2:-}"; shift 2 ;;
            --since-days|--threads-since) since="${2:-}"; shift 2 ;;
            --json) json=1; shift ;;
            -*) echo "usage: k2 thread export <ws> [--since-days N] | import <file> --to <ws> | imports <ws>" >&2; exit 2 ;;
            *) if [ "$sub" = "import" ]; then path="$1"; else ws="$1"; fi; shift ;;
        esac
    done
    case "$sub" in
        export)
            [ -n "$ws" ] || { echo "usage: k2 thread export <ws> [--since-days N]" >&2; exit 2; }
            _clone_person_guard
            _compute_call POST /cli/thread/export "$(_compute_json K2_CJ_ws="$ws" K2_CJ_sinceDays_n="$since")" tcp
            if [ "$json" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("summary") or {}
print("%s: %s Thread(s), %s line(s) → %s" % (v.get("workspace"), s.get("conversations"), s.get("docs"), v.get("path")))'
            ;;
        import)
            [ -n "$path" ] && [ -n "$to" ] || { echo "usage: k2 thread import <file> --to <ws>" >&2; exit 2; }
            _clone_person_guard
            local abs
            abs="$(cd "$(dirname "$path")" 2>/dev/null && pwd)/$(basename "$path")"
            _compute_call POST /cli/thread/import "$(_compute_json K2_CJ_path="$abs" K2_CJ_to="$to")" tcp
            if [ "$json" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("report") or {}
print("%s: imported %s line(s) into %s Thread(s) (%s already there)%s" % (v.get("workspace"), r.get("docsImported"),
      len(r.get("conversations") or []), r.get("docsSkipped"), " — main Thread waits for the first chat" if r.get("pending") else ""))'
            ;;
        imports)
            [ -n "$ws" ] || { echo "usage: k2 thread imports <ws>" >&2; exit 2; }
            _compute_call GET /cli/thread/imports "ws=$(urlencode "$ws")" tcp
            if [ "$json" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("imports") or []
if not rows:
    print("No imported Threads."); sys.exit(0)
for r in rows:
    print("%-44s %-17s %5s lines  from %s on %s%s" % (r["localConversationId"], r["role"], r["docCount"], r["sourceHandle"], r["sourceHost"],
          (" (" + r["name"] + ")") if r.get("name") else ""))'
            ;;
    esac
}

# thread-attachments-v1: `--file path[:line[-end]]` values → the JSON the
# daemon takes ([{path, line?, lineEnd?}]). A `:N` / `:N-M` suffix is a
# line spec only when it is numeric AND `path:suffix` itself is not a file
# (colons in names). A path that exists here is sent absolute (resolved
# against $PWD); anything else goes as typed and the daemon resolves it
# against the workspace root. The daemon makes every decision (inside the
# workspace, a file, not a secret).
_thread_attachments_json() {
    python3 - "$PWD" "$@" <<'PY'
import json, os, re, sys
cwd = sys.argv[1]
out = []
for spec in sys.argv[2:]:
    path, line, end = spec, None, None
    here = lambda p: os.path.join(cwd, os.path.expanduser(p))
    if not os.path.exists(here(spec)):
        m = re.match(r"^(.+?):(\d+)(?:-(\d+))?$", spec)
        if m:
            path, line = m.group(1), int(m.group(2))
            end = int(m.group(3)) if m.group(3) else None
    if os.path.exists(here(path)):
        path = os.path.abspath(here(path))
    item = {"path": path}
    if line is not None:
        item["line"] = line
    if end is not None:
        item["lineEnd"] = end
    out.append(item)
print(json.dumps(out))
PY
}

# ticket-attachments-v1 (0.46.6): the files a ticket write carried. A
# daemon older than 0.46.6 drops an `attachments` field it doesn't know and
# answers success; when this CLI sent files and the answer has no
# `attachments` key, it fails loudly (exit 1, server_too_old), never a
# silent drop (prd-ticket-attachments-v1 TA3).
_ticket_files_check_py='
def ticket_files_check(sent, d):
    if sent and isinstance(d, dict) and "attachments" not in d:
        sys.stderr.write(json.dumps({"error": {"code": "server_too_old",
            "hint": "this server'"'"'s K2 doesn'"'"'t take ticket files (needs 0.46.6)"}}) + "\n")
        sys.exit(1)

def ticket_size(n):
    if n < 1024: return "%d B" % n
    if n < 1024 * 1024: return "%.1f KB" % (n / 1024.0)
    return "%.1f MB" % (n / (1024.0 * 1024.0))

def ticket_file_lines(atts, root, indent="  "):
    out = []
    for a in atts or []:
        if not isinstance(a, dict):
            continue
        if a.get("state") == "hidden":
            out.append("%s[file] (not shared with you)" % indent)
            continue
        rel = str(a.get("path", ""))
        shown = os.path.join(root, rel) if root else rel
        if a.get("line"):
            shown += ":%s" % a["line"] + ("-%s" % a["lineEnd"] if a.get("lineEnd") else "")
        extra = {"changed": " (changed since sent)", "gone": " (gone)"}.get(a.get("state"), "")
        out.append("%s[file] %s (%s)%s" % (indent, shown, ticket_size(int(a.get("size") or 0)), extra))
    return out
'

cmd_thread() {
    for arg in "$@"; do
        case "$arg" in
            --help|-h)
                if [[ "${1:-}" == "secret" ]]; then cmd_help_thread_secret; else cmd_help_thread; fi
                return 0
                ;;
        esac
    done

    local first_pos="" skip_next=0
    for arg in "$@"; do
        if [[ $skip_next -eq 1 ]]; then skip_next=0; continue; fi
        case "$arg" in
            --from|--options|--option|--name|--prompt|--dest|--timeout|--style|--limit|--before-seq|--seq|--file|--scope|--for|--delivery|--kind|--ttl|--inject-dst|--inject-header|--inject-format|--why|--reason)
                skip_next=1
                continue
                ;;
            --*) continue ;;
            *) first_pos="$arg"; break ;;
        esac
    done
    case "$first_pos" in
        ask) cmd_thread_ask "$@"; return $? ;;
        secret) cmd_thread_secret "$@"; return $? ;;
        void|withdraw|dismiss) cmd_thread_void "$@"; return $? ;;
        delete) cmd_thread_delete "$@"; return $? ;;
        export|import|imports) cmd_thread_transfer "$@"; return $? ;;
        unread) cmd_thread_unread "$@"; return $? ;;
        read-all) cmd_thread_read_all "$@"; return $? ;;
        post)
            echo "Error: no 'post' subcommand — use  k2 thread <addr> \"text\"" >&2
            exit 1 ;;
    esac

    local target=""
    local text=""
    local json="false"
    local explicit_from=""
    local limit=""
    local all="false"
    local before_seq=""
    local one_seq=""
    local -a files=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json="true"; shift ;;
            --from) explicit_from="$2"; shift 2 ;;
            --limit) limit="$2"; shift 2 ;;
            --all) all="true"; shift ;;
            --before-seq) before_seq="$2"; shift 2 ;;
            --seq)
                [[ -z "${2:-}" ]] && { echo "Error: --seq needs a message number (the [N] in k2 thread <addr>)" >&2; exit 1; }
                one_seq="$2"; shift 2 ;;
            --file)
                [[ -z "${2:-}" ]] && { echo "Error: --file needs a path (path[:line[-end]])" >&2; exit 1; }
                files+=("$2"); shift 2 ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 thread --help')" >&2
                exit 1 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                else text="$1"
                fi
                shift ;;
        esac
    done

    [[ -z "$target" ]] && {
        echo "Error: address required" >&2
        echo "Usage: k2 thread <addr> \"text\"     # write overlay text" >&2
        echo "       k2 thread <addr> [\"text\"] --file path[:line[-end]]   # attach files" >&2
        echo "       k2 thread <addr> [--json] [--limit N|--all] [--before-seq N]" >&2
        echo "       k2 thread ask <addr> \"prompt\" --options \"a,b,c\"" >&2
        echo "       k2 thread secret <addr> --name NAME" >&2
        echo "       k2 thread withdraw <addr> <card-id> [--why \"…\"]" >&2
        echo "       k2 thread dismiss <addr> <card-id> [--why \"…\"]" >&2
        exit 1
    }

    # --seq N: exactly message N, whole (a person's `↩ #N` reply names it).
    # The plain read of the newest 1 below N+1; any other seq is "not here".
    if [[ -n "$one_seq" ]]; then
        if [[ ! "$one_seq" =~ ^[0-9]+$ ]] || [[ $((10#$one_seq)) -lt 1 ]]; then
            echo "Error: --seq needs a whole number >= 1 (the [N] in k2 thread <addr>)" >&2
            exit 1
        fi
        if [[ -n "$text" || ${#files[@]} -gt 0 || -n "$limit" || "$all" == "true" || -n "$before_seq" ]]; then
            echo "Error: --seq reads one message: no text, --file, --limit, --all or --before-seq with it" >&2
            exit 1
        fi
        one_seq=$((10#$one_seq))
        before_seq=$((one_seq + 1))
        limit=1
    fi

    local from
    from="$(_thread_sender_from "$explicit_from")"

    if [[ ${#files[@]} -gt 10 ]]; then
        echo "Error: at most 10 files per message" >&2
        exit 1
    fi
    if [[ -n "$text" || ${#files[@]} -gt 0 ]]; then
        local params="addr=$(urlencode "$target")"
        params="${params}&text=$(urlencode "$text")"
        params="${params}&from=$(urlencode "$from")"
        params="${params}&via=thread"
        if [[ ${#files[@]} -gt 0 ]]; then
            local attachments_json
            attachments_json="$(_thread_attachments_json "${files[@]}")" || exit 1
            params="${params}&attachments=$(urlencode "$attachments_json")"
        fi
        local response
        response=$(cli_send cli_post_form "/cli/thread/post" "$params")
        local rc=$?
        _k2_moved_from_note "$response"
        if [[ "$json" == "true" ]]; then
            echo "$response"
        else
            echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
if d.get("ok") is not True:
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
print("%s  seq=%s  from=%s" % (d.get("id",""), d.get("seq",""), d.get("from","")))
def size(n):
    n=int(n or 0)
    for unit in ("B","KB","MB","GB"):
        if n < 1024 or unit == "GB":
            return ("%d %s" % (n, unit)) if unit == "B" else ("%.0f %s" % (n, unit))
        n=n/1024.0
for a in d.get("attachments") or []:
    line=""
    if a.get("line"):
        line=":%s" % a["line"] + (("-%s" % a["lineEnd"]) if a.get("lineEnd") else "")
    print("  [file] %s%s (%s)" % (a.get("path",""), line, size(a.get("size"))))
'
            rc=$?
            if [[ $rc -eq 0 ]]; then
                _thread_md_nudge
            fi
        fi
        return $rc
    fi

    local params="addr=$(urlencode "$target")"
    if [[ "$all" == "true" ]]; then
        params="${params}&limit=0"
    elif [[ -n "$limit" ]]; then
        params="${params}&limit=$(urlencode "$limit")"
    fi
    if [[ -n "$before_seq" ]]; then
        params="${params}&before_seq=$(urlencode "$before_seq")"
    fi
    local response
    response=$(cli_send cli_request "/cli/thread" "$params")
    local rc=$?
    if [[ "$json" == "true" && -z "$one_seq" ]]; then
        echo "$response"
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
want=int(sys.argv[1]) if sys.argv[1] else 0
as_json=sys.argv[2]=="true"
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
if d.get("ok") is not True:
    if as_json:
        print(raw.strip())
        sys.exit(2)
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
items=d.get("items") or []
if want:
    items=[it for it in items if it.get("seq")==want]
    if not items:
        sys.stderr.write("No message %d on this Thread (k2 thread %s lists them as [N]).\n" % (want, sys.argv[3]))
        sys.exit(1)
    if as_json:
        d["items"]=items
        d["has_more"]=False
        print(json.dumps(d))
        sys.exit(0)
if not items:
    print("(empty)")
    sys.exit(0)
def size(n):
    n=int(n or 0)
    for unit in ("B","KB","MB","GB"):
        if n < 1024 or unit == "GB":
            return ("%d %s" % (n, unit)) if unit == "B" else ("%.0f %s" % (n, unit))
        n=n/1024.0
LINE_BREAKS=("\r\n","\r","\x0b","\x0c","\x1c","\x1d","\x1e","\x85","\u2028","\u2029")
def body_lines(s):
    # Every break kind is a new line; each line after the first is quoted
    # (re-review L6) so no body can print a line that reads as its own
    # "[N] from ..." header, a [file] or a [context] line.
    for b in LINE_BREAKS:
        s=s.replace(b,"\n")
    return s.split("\n")
for it in items:
    doc=it.get("doc") or {}
    body=doc.get("body") or ""
    frm=doc.get("from") or ""
    author=doc.get("author") if isinstance(doc.get("author"), dict) else {}
    if author.get("kind") == "guest":
        # An App guest or a Voice & Chat only login: never a person on this
        # box, never the owner (re-review L6).
        frm="guest %s" % (author.get("name") or author.get("login") or frm or "?")
    seq=it.get("seq")
    lines=body_lines(body)
    print("[%s] from %s: %s" % (seq, frm, lines[0]))
    for l in lines[1:]:
        print("  | %s" % l)
    ctx=doc.get("context")
    if isinstance(ctx, dict) and isinstance(ctx.get("text"), str):
        # thread-context-v1: a line a Garden widget set, never words the person typed.
        print("  [context from Garden widget %s, not typed by %s] %s" % (ctx.get("placement",""), frm, json.dumps(ctx["text"])))
    for a in doc.get("attachments") or []:
        st=a.get("state") or "ok"
        if st == "hidden":
            print("  [file] (not shared with you)")
            continue
        line=""
        if a.get("line"):
            line=":%s" % a["line"] + (("-%s" % a["lineEnd"]) if a.get("lineEnd") else "")
        tag="" if st == "ok" else " (%s)" % st
        print("  [file] %s%s (%s)%s" % (a.get("path",""), line, size(a.get("size")), tag))
' "$one_seq" "$json" "$target"
}

cmd_thread_ask() {
    local target="" prompt="" json="false" explicit_from=""
    local options_csv="" allow_custom="false" expires="" no_expiry="false"
    local option_list=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            ask) shift ;;
            --json) json="true"; shift ;;
            --from)
                [[ $# -ge 2 ]] || { echo "Error: --from requires a value" >&2; exit 1; }
                explicit_from="$2"; shift 2 ;;
            --options)
                [[ $# -ge 2 ]] || { echo "Error: --options requires a comma-separated list" >&2; exit 1; }
                if [[ ${#option_list[@]} -gt 0 ]]; then
                    echo "Error: mix of --options and --option is not allowed" >&2
                    exit 1
                fi
                options_csv="$2"; shift 2 ;;
            --option)
                [[ $# -ge 2 ]] || { echo "Error: --option requires a label" >&2; exit 1; }
                if [[ -n "$options_csv" ]]; then
                    echo "Error: mix of --options and --option is not allowed" >&2
                    exit 1
                fi
                option_list+=("$2"); shift 2 ;;
            --allow-custom) allow_custom="true"; shift ;;
            --expires)
                [[ $# -ge 2 ]] || { echo "Error: --expires requires a time frame (2h, 3d, 90m)" >&2; exit 1; }
                expires="$2"; shift 2 ;;
            --no-expiry) no_expiry="true"; shift ;;
            --wait|--timeout)
                echo "Error: thread cards are fire-and-forget; no --wait / --timeout" >&2
                exit 1 ;;
            --style)
                echo "Error: no --style in v1" >&2
                exit 1 ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 thread --help')" >&2
                exit 1 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                elif [[ -z "$prompt" ]]; then prompt="$1"
                else
                    echo "Error: unexpected extra argument '$1'" >&2
                    exit 1
                fi
                shift ;;
        esac
    done
    [[ -n "$target" ]] || { echo "Error: address required" >&2; echo "Usage: k2 thread ask <addr> \"<prompt>\" --options \"a,b,c\"" >&2; exit 1; }
    [[ -n "$prompt" ]] || { echo "Error: prompt required" >&2; echo "Usage: k2 thread ask <addr> \"<prompt>\" --options \"a,b,c\"" >&2; exit 1; }
    if [[ -z "$options_csv" && ${#option_list[@]} -eq 0 ]]; then
        echo "Error: ask requires --options or --option" >&2
        exit 1
    fi
    if [[ -n "$expires" && "$no_expiry" == "true" ]]; then
        echo "Error: use --expires or --no-expiry, not both" >&2
        exit 1
    fi
    local from option_json=""
    from="$(_thread_sender_from "$explicit_from")"
    if [[ ${#option_list[@]} -gt 0 ]]; then
        option_json=$(printf '%s\n' "${option_list[@]}" | python3 -c 'import json,sys; print(json.dumps([ln.rstrip("\n") for ln in sys.stdin]))')
    fi
    local body
    body=$(ADDR="$target" PROMPT="$prompt" FROM="$from" OPTIONS_CSV="$options_csv" \
        OPTIONS_JSON="$option_json" ALLOW="$allow_custom" EXPIRES="$expires" NO_EXPIRY="$no_expiry" python3 -c 'import json,os
obj={"addr":os.environ["ADDR"],"prompt":os.environ["PROMPT"],"from":os.environ.get("FROM","k2")}
csv=os.environ.get("OPTIONS_CSV") or ""
optj=os.environ.get("OPTIONS_JSON") or ""
if csv:
    obj["options"]=csv
if optj:
    obj["option"]=json.loads(optj)
if os.environ.get("ALLOW")=="true":
    obj["allow_custom"]=True
if os.environ.get("EXPIRES"):
    obj["expires"]=os.environ["EXPIRES"]
if os.environ.get("NO_EXPIRY")=="true":
    obj["no_expiry"]=True
print(json.dumps(obj))
')
    local response
    response=$(cli_send cli_post_json "/cli/thread/ask" "$body")
    local rc=$?
    _k2_moved_from_note "$response"
    if [[ "$json" == "true" ]]; then
        echo "$response"
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
if d.get("ok") is not True:
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
print(d.get("id",""))
'
    rc=$?
    if [[ $rc -eq 0 ]]; then
        _thread_md_nudge
    fi
    return $rc
}

# prd-popup-close-and-card-dismiss-v1 (0.46.11): close a waiting card
# without answering it. `withdraw` (the asking agent; `void` is its old
# name) and `dismiss` (a person) hit the same route; the daemon decides
# which from the caller's credential, never from the verb.
cmd_thread_void() {
    local target="" card="" json="false" why="" verb="withdraw"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            void|withdraw|dismiss) [[ -z "$target" && "$verb" == "withdraw" ]] && verb="$1"; shift ;;
            --json) json="true"; shift ;;
            --why|--reason)
                [[ $# -ge 2 ]] || { echo "Error: $1 requires a value (one line, at most 200 characters)" >&2; exit 2; }
                why="$2"; shift 2 ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 thread --help')" >&2
                exit 2 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                elif [[ -z "$card" ]]; then card="$1"
                else
                    echo "Error: unexpected extra argument '$1'" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    [[ "$verb" == "void" ]] && verb="withdraw"
    [[ -n "$target" ]] || { echo "Error: address required" >&2; echo "Usage: k2 thread $verb <addr> <card-id> [--why \"…\"]" >&2; exit 2; }
    [[ -n "$card" ]] || { echo "Error: card id required (the id k2 thread ask / secret printed)" >&2; echo "Usage: k2 thread $verb <addr> <card-id> [--why \"…\"]" >&2; exit 2; }
    local body
    body=$(ADDR="$target" CARD="$card" WHY="$why" python3 -c 'import json,os
obj={"addr":os.environ["ADDR"],"id":os.environ["CARD"]}
if os.environ.get("WHY"):
    obj["reason"]=os.environ["WHY"]
print(json.dumps(obj))
')
    local response
    response=$(cli_send cli_post_json "/cli/thread/void" "$body")
    local rc=$?
    _k2_moved_from_note "$response"
    if [[ "$json" == "true" ]]; then
        echo "$response"
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
if d.get("ok") is not True:
    err=d.get("error") or d
    code=""
    if isinstance(err, dict):
        code=err.get("code") or "error"
        sys.stderr.write("%s: %s\n" % (code, err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    if code in ("card_not_yours", "action_card_person_only", "forbidden", "card_not_pending", "request_not_pending"):
        sys.exit(3)
    if code == "not_found":
        sys.exit(4)
    sys.exit(2)
# "<card-id>  dismissed" / "withdrawn" (an older daemon: "voided").
print("%s  %s" % (d.get("id",""), d.get("closedAs") or d.get("status","")))
'
}

# thread-message-delete-v1 (0.46.11): a PERSON deletes a message they sent
# (the owner: anyone's). The daemon checks who sent it; an agent passport
# gets person_only (exit 3).
cmd_thread_delete() {
    local target="" which="" json="false"
    [[ "${1:-}" == "delete" ]] && shift
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json="true"; shift ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 thread --help')" >&2
                exit 2 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                elif [[ -z "$which" ]]; then which="$1"
                else
                    echo "Error: unexpected extra argument '$1'" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    [[ -n "$target" ]] || { echo "Error: address required" >&2; echo "Usage: k2 thread delete <addr> <seq|id> [--json]" >&2; exit 2; }
    [[ -n "$which" ]] || { echo "Error: which message: its number (the [N] in k2 thread <addr>) or its id" >&2; echo "Usage: k2 thread delete <addr> <seq|id> [--json]" >&2; exit 2; }
    local body
    body=$(ADDR="$target" WHICH="$which" python3 -c 'import json,os
w=os.environ["WHICH"].strip().lstrip("#")
obj={"addr":os.environ["ADDR"]}
if w.isdigit():
    obj["seq"]=int(w)
else:
    obj["id"]=w
print(json.dumps(obj))
')
    local response
    response=$(cli_send cli_post_json "/cli/thread/message/delete" "$body")
    local rc=$?
    _k2_moved_from_note "$response"
    echo "$response" | JSON_OUT="$json" python3 -c 'import json,os,sys
raw=sys.stdin.read()
as_json=os.environ.get("JSON_OUT")=="true"
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(2)
if as_json:
    print(raw.strip())
if d.get("ok") is not True:
    err=d.get("error") or d
    code=""
    if isinstance(err, dict):
        code=err.get("code") or "error"
        if not as_json:
            sys.stderr.write("%s: %s\n" % (code, err.get("hint") or err))
    elif not as_json:
        sys.stderr.write(str(err)+"\n")
    if code in ("person_only", "not_your_message", "not_a_persons_message", "forbidden", "imported_read_only"):
        sys.exit(3)
    if code == "not_found":
        sys.exit(4)
    sys.exit(2)
if not as_json:
    note="already deleted" if d.get("already") else "deleted; the agent was told (never what it said)"
    print("[%s] %s  %s" % (d.get("seq",""), d.get("id",""), note))
'
}

cmd_thread_secret() {
    local target="" json="false" explicit_from="" name="" prompt="" dest=""
    local v_scope="" v_for="" v_delivery="" v_kind="" v_ttl="" v_once=""
    local v_inject_dst="" v_inject_header="" v_inject_format=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            secret) shift ;;
            # prd-secret-card-injection-v1: how K2 sends it (fetch only).
            # The same flags and checks as `k2 vault set`.
            --inject-dst|--inject-header|--inject-format)
                [[ $# -ge 2 ]] || { echo "Error: $1 requires a value" >&2; exit 1; }
                case "$1" in
                    --inject-dst) v_inject_dst="$2" ;; --inject-header) v_inject_header="$2" ;;
                    --inject-format) v_inject_format="$2" ;;
                esac
                shift 2 ;;
            --json) json="true"; shift ;;
            --from)
                [[ $# -ge 2 ]] || { echo "Error: --from requires a value" >&2; exit 1; }
                explicit_from="$2"; shift 2 ;;
            --name)
                [[ $# -ge 2 ]] || { echo "Error: --name requires a value" >&2; exit 1; }
                name="$2"; shift 2 ;;
            --prompt)
                [[ $# -ge 2 ]] || { echo "Error: --prompt requires a value" >&2; exit 1; }
                prompt="$2"; shift 2 ;;
            --dest)
                [[ $# -ge 2 ]] || { echo "Error: --dest requires a value" >&2; exit 1; }
                dest="$2"; shift 2 ;;
            # K2 vault (prd-k2-vault-v1 §10.1): what this agent proposes.
            # The person can change all of it on the card.
            --scope|--for|--delivery|--kind|--ttl)
                [[ $# -ge 2 ]] || { echo "Error: $1 requires a value" >&2; exit 1; }
                case "$1" in
                    --scope) v_scope="$2" ;; --for) v_for="$2" ;; --delivery) v_delivery="$2" ;;
                    --kind) v_kind="$2" ;; --ttl) v_ttl="$2" ;;
                esac
                shift 2 ;;
            --wait|--timeout)
                echo "Error: thread cards are fire-and-forget; no --wait / --timeout" >&2
                exit 1 ;;
            --once) v_once="1"; shift ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 thread --help')" >&2
                exit 1 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                else
                    echo "Error: unexpected extra argument '$1'" >&2
                    exit 1
                fi
                shift ;;
        esac
    done
    [[ -n "$target" ]] || { echo "Error: address required" >&2; echo "Usage: k2 thread secret <addr> --name NAME" >&2; exit 1; }
    [[ -n "$name" ]] || { echo "Error: secret --name is required" >&2; exit 1; }
    if [[ -n "$dest" && "$dest" != "vault" ]]; then
        echo "Error: --dest vault only" >&2
        exit 1
    fi
    if [[ -n "$v_inject_dst" && -n "$v_inject_header" ]]; then
        echo "Error: give --inject-dst or --inject-header, not both" >&2
        exit 1
    fi
    [[ -n "$v_inject_header" ]] && v_inject_dst="header:$v_inject_header"
    local from
    from="$(_thread_sender_from "$explicit_from")"
    local body
    body=$(ADDR="$target" NAME="$name" FROM="$from" PROMPT="$prompt" V_SCOPE="$v_scope" V_FOR="$v_for" V_DELIVERY="$v_delivery" V_KIND="$v_kind" V_TTL="$v_ttl" V_ONCE="$v_once" V_IDST="$v_inject_dst" V_IFMT="$v_inject_format" python3 -c 'import json,os
obj={"addr":os.environ["ADDR"],"name":os.environ["NAME"],"from":os.environ.get("FROM","k2"),"dest":"vault"}
p=os.environ.get("PROMPT") or ""
if p:
    obj["prompt"]=p
for k, env in (("scope","V_SCOPE"),("for","V_FOR"),("delivery","V_DELIVERY"),("kind","V_KIND"),("ttl","V_TTL")):
    v=os.environ.get(env) or ""
    if v:
        obj[k]=v
if os.environ.get("V_ONCE"):
    obj["once"]=True
inj={}
if os.environ.get("V_IDST"):
    inj["dst"]=os.environ["V_IDST"]
if os.environ.get("V_IFMT"):
    inj["fmt"]=os.environ["V_IFMT"]
if inj:
    obj["inject"]=inj
print(json.dumps(obj))
')
    local response
    response=$(cli_send cli_post_json "/cli/thread/secret" "$body")
    local rc=$?
    _k2_moved_from_note "$response"
    if [[ "$json" == "true" ]]; then
        echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
# Never stdout a secret value — drop any accidental fields.
d.pop("value", None)
d.pop("secret", None)
print(json.dumps(d))
'
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(0)
if d.get("ok") is not True:
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
print(d.get("id",""))
# What the card says, in one line (stderr: stdout stays the card id).
if d.get("sends"):
    sys.stderr.write("card: %s\n" % d["sends"])
'
}

# thread-read-all-v1 (0.46.5): `k2 thread read-all <agent>` — Mark all as
# read: every Thread of one agent (its pinned Chat, sidecars, and earlier
# chats' Threads no address reaches any more) read for the caller's person
# (POST /cli/thread/read-all). Waiting cards stay waiting. Agent passports
# get 403 (agents have no read state).
cmd_thread_read_all() {
    local agent="" json="false"
    shift # "read-all"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json="true"; shift ;;
            -*) echo "Error: unknown flag '$1' (k2 thread read-all <agent> [--json])" >&2; exit 1 ;;
            *)
                if [[ -n "$agent" ]]; then
                    echo "Error: one agent at a time (k2 thread read-all <agent> [--json])" >&2; exit 1
                fi
                agent="$1"; shift ;;
        esac
    done
    if [[ -z "$agent" ]]; then
        echo "Error: agent required" >&2
        echo "Usage: k2 thread read-all <agent> [--json]   # mark all of an agent's Thread messages read for you" >&2
        exit 1
    fi
    local body
    body="$(python3 -c 'import json,sys; print(json.dumps({"agent": sys.argv[1]}))' "$agent")" || exit 2
    local response
    response=$(cli_send cli_post_json "/cli/thread/read-all" "$body")
    local rc=$?
    if [[ "$json" == "true" ]]; then
        echo "$response"
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
agent=sys.argv[1]
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(2)
if d.get("ok") is not True:
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
n=len(d.get("marked") or [])
if n:
    print("%s: marked %d Thread%s read; unread now %s" % (agent, n, "" if n == 1 else "s", d.get("unread",0)))
else:
    print("%s: nothing unread" % agent)
w=d.get("waiting",0)
if w:
    print("  %s card%s still waiting on you (answer %s to clear)" % (w, "" if w == 1 else "s", "it" if w == 1 else "them"))
' "$agent"
}

# thread-read-v1 (0.46.1): `k2 thread unread` — read-only, a person's own
# unread / waiting counts (GET /cli/thread/unread). Agent passports get 403.
cmd_thread_unread() {
    local kind="" json="false"
    shift # "unread"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --kind) kind="$2"; shift 2 ;;
            --json) json="true"; shift ;;
            *) echo "Error: unknown argument '$1' (k2 thread unread [--kind thread|project] [--json])" >&2; exit 1 ;;
        esac
    done
    local params=""
    [[ -n "$kind" ]] && params="kind=$(urlencode "$kind")"
    local response
    response=$(cli_send cli_request "/cli/thread/unread" "$params")
    local rc=$?
    if [[ "$json" == "true" ]]; then
        echo "$response"
        return $rc
    fi
    echo "$response" | python3 -c 'import json,sys
raw=sys.stdin.read()
try:
    d=json.loads(raw)
except Exception:
    sys.stdout.write(raw)
    sys.exit(2)
if d.get("ok") is not True:
    err=d.get("error") or d
    if isinstance(err, dict):
        sys.stderr.write("%s: %s\n" % (err.get("code") or "error", err.get("hint") or err))
    else:
        sys.stderr.write(str(err)+"\n")
    sys.exit(2)
print("%s: %s unread+waiting (badge), %s waiting" % (d.get("person",""), d.get("total",0), d.get("waitingTotal",0)))
for c in d.get("byConversation") or []:
    n = c.get("unread",0)
    shown = "99+" if n > 99 else str(n)
    flags = []
    if c.get("waiting"):
        flags.append("%s waiting" % c["waiting"])
    if c.get("muted"):
        flags.append("muted")
    print("  %-7s %s  unread=%s%s" % (c.get("kind",""), c.get("id",""), shown, ("  (" + ", ".join(flags) + ")") if flags else ""))
'
}

cmd_help_thread() {
    cat <<'EOF'
k2 thread — user-agent overlay (not PTY inject; that is k2 msg)

  k2 thread <addr> "text"                         # overlay text (not PTY; that's k2 msg)
  k2 thread <addr> "text" --file path[:line[-end]]  # attach a workspace file (repeatable, ≤10)
  k2 thread <addr> --file out/shot.png            # text is optional when a file rides
  k2 thread <addr> [--limit N|--all] [--before-seq N]  # read newest 25
  k2 thread <addr> --seq N                        # read message N (the [N] in a read), whole
  k2 thread ask <addr> "prompt" --options "a,b,c" # choice card; do not wait
      [--expires 2h|3d|90m] [--no-expiry]          # open 7 days unless you pick a time frame
  k2 thread secret <addr> --name NAME             # secret field; you never see the value
      [--prompt "…"] (≤ 200 chars) [--for api.stripe.com[,…]] [--delivery fetch|proxy|file|env|agent]
      [--scope workspace|server|session] [--kind api_key|password|…] [--ttl 24h]
      [--inject-dst basic|header:Name|query:param] [--inject-format 'user:{}']
      The value goes into the K2 vault; you get a k2v:// reference and use it with
      `k2 vault fetch --secret NAME …`. YOU choose the sites, scope, delivery and how
      K2 sends it: the card shows your choice in one sentence and the person just
      clicks Save. If they say it's wrong, `k2 thread withdraw` it and ask again. Exact
      hosts only. All flags: k2 thread secret --help. More: k2 study vault
  k2 thread withdraw <addr> <card-id> [--why "…"] # take back your own card (no longer relevant)
      (void is the old name and still works)
  k2 thread dismiss <addr> <card-id> [--why "…"]  # for a PERSON: close a card without answering
  k2 thread delete <addr> <seq|id> [--json]       # for a PERSON: delete a message you sent
      (the owner: anyone's). Its text and files go; "Message deleted" stays in its
      place. The agent gets one line: [thread:<addr>] <who> deleted a message they
      sent at <time>. Don't repeat, store or act on what it said. Agents get exit 3
      (person_only): only the person who sent it can delete it.
  k2 thread unread [--kind thread|project] [--json]  # a PERSON's unread + waiting counts
  k2 thread read-all <agent> [--json]             # mark all of an agent's Thread messages read
      (for you: every chat's Thread of that agent; waiting cards stay waiting)
      (owner token or a K2 login; agents have no read state and get 403)

<addr> is the same first-arg grammar as k2 msg: sales / sales/reviewer / sales/1.
sales = live alias for pinned Chat now. sales/reviewer is durable.

gb:<bot name> (or gb:<agent id>) reads a local Grok Bot's collected messages:
people signed in to this server only, while Local Grok Bots → Show is on. Agents
and App guests get 403, and nobody posts there (message the bot from its chat).

Post and continue; you get a later turn when they answer. Do not --wait. Do not
emit widget JSON.

A card stays open until the person taps it (or dismisses it), you withdraw it, or
its time frame runs out (choice cards: 7 days unless --expires / --no-expiry; secret
cards: 7 days). Pick a time frame that fits the decision. People may discuss a card
before answering: keep it open and answer their questions in prose;
never treat their prose as the card's answer; talking never closes or shortens it.
Withdraw your own card when its question no longer matters
(k2 thread withdraw <addr> <card-id> [--why "…"]; --why is optional, one line), or
post a new card if the options changed. The person then sees the card as Withdrawn,
with your reason; nobody is woken. A secret card: only one your own session asked
for (K2's own secret cards, from a clone or a reconnect, are the person's to answer
or dismiss). A permission card: your own requests on it (the same as k2 permission
cancel).

Closed cards stay on the Thread, marked Dismissed or Withdrawn. When the person
dismisses your card you get a turn like an answer:
  [thread:<addr>] dismissed card <id> without answering: "<their reason>"
It means "not needed now", not yes and not no: don't ask the same thing again unless
something changed. A dismissed permission card is not a denial: nothing was applied,
and you may ask again (k2 permission request …) if you still need it.
k2 vault file NAME --wait 10m (see k2 vault file --help) on a secret card exits 5
when it was dismissed or withdrawn (card_dismissed / card_withdrawn) and 3 when it
expired (card_expired). An expired card says so on the Thread; ask again if you
still need it.

withdraw/dismiss exit 0 (prints "<card-id>  withdrawn|dismissed"), 2 bad usage, 3
refused, 4 not found. On 3 the error line names why: card_not_yours (not your card),
card_not_pending (already answered, dismissed, withdrawn or expired) or
action_card_person_only (a settings card: only the person closes it).

When a person deletes a message they sent, you get that one line. Treat what the
message said as gone: don't repeat it, quote it, store it or act on it.
delete exits 0 (prints "[N] <id>  deleted…"; again: "already deleted"), 2 bad usage,
3 refused (person_only, not_your_message, not_a_persons_message), 4 not found.

Text (and ask prompts) render markdown: **bold**, lists, `code`, fences, links.

Attach the files you mention (--file): the person taps them to open (desktop and
phone). A file is a pointer to this workspace's copy, not a copy: it shows
"changed" when the file changes and "not found" when it moves. Only files inside
this workspace (copy others into it first, e.g. out/); secrets (.env, keys,
tokens, credentials in the text) are refused: secrets go through k2 vault.
path:12 or path:12-40 opens at that line.

--json on any verb. Ask --option repeatable; --allow-custom; mix with --options is 400.
EOF
}

cmd_help_thread_secret() {
    cat <<'EOF'
k2 thread secret — ask a person for a secret on a card; you never see the value

  k2 thread secret <addr> --name NAME [flags]

<addr> is the Thread the card goes to, as in k2 thread <addr>: your own
workspace's name (ops) or a sidecar (ops/reviewer). NAME is how the secret is
saved: letters, digits, _ . - (HETZNER_KEY).

The person pastes the value into the card and clicks Save. It goes into the
K2 vault and you get a k2v:// reference. You choose the flags below; the card
says them in one plain sentence, and the person can change them under Advanced.

WHERE IT GOES
  --for HOST[,HOST]          the exact sites K2 may send it to: host names only,
                             no https:// or path (api.example.com)
  --delivery fetch|proxy|file|env|agent  fetch (default): K2 makes the HTTPS call and adds the
                             secret; you never see it. Use it with
                             k2 vault fetch --secret NAME https://HOST/path
                             (GET by default; k2 vault fetch --help)
                             proxy: as fetch, and new sessions also get a
                             placeholder key and a local base URL in their env.
                             file: $K2_SECRETS_DIR/NAME. env: $NAME (upper case,
                             - and . become _) in new sessions. You can read both.
                             agent (an ssh key, with --kind ssh_key, workspace
                             scope): ssh and git sign with it on $SSH_AUTH_SOCK
                             in new sessions; you never see the key.
  --scope workspace|server|session  who keeps it (default workspace; ws:<this
                             workspace> means the same). session: one
                             running K2 session, with fetch or file (env never
                             arrives): the one that asks, or this chat's running
                             session if that one ended before Save. It stays with
                             this chat (the same conversation, also after --resume)
                             until --ttl runs out: after a restart, the chat's next
                             session gets it within a minute (k2 vault sync: now).
                             Another chat never does. Workspace scope keeps it for
                             good. server (an Owner or Admin saves it): kept on
                             the server and shared with your workspace on Save;
                             other workspaces get it only when a person shares
                             it (k2 vault grant NAME --to ws:<name> --scope
                             server). Name it with --scope server when you use
                             it (k2 vault env-file NAME --scope server).
                             If a guest or Member saves it
                             and a workspace secret has the same name, a bare name
                             keeps the workspace one: use it with --scope session.
  --kind api_key|password|generic|…  what it is, so the card names it (default
                             api_key; password for a password)
  --ttl 24h                  session scope only: how long it lasts (90s, 10m, 2h,
                             7d or seconds). Default 24h;
                             longer than 7d is cut to 7d, and to 24h when a guest
                             answers. Other scopes ignore it.
  --once                     a one-time code a tool reads once (needs --scope
                             session --delivery file). The card says "used
                             once, then deleted"; it is never put in
                             $K2_SECRETS_DIR (unlike other file secrets), and
                             k2 vault file NAME is the only way to read it. A
                             tool can block until it is saved:
                             k2 vault file NAME --wait 10m
  --prompt "…"               your note, shown on the card beside its sentence
                             (at most 200 characters)

HOW K2 SENDS IT (fetch only)
  Leave these out and K2 uses the format it knows for the key, else
  Authorization: Bearer <secret>.
  --inject-dst basic         a Basic login. The person pastes user:password (the
                             user ends at the first :, the password may hold more).
                             K2 builds the Authorization: Basic header.
  --inject-dst basic --inject-format 'USER:{}'
                             a Basic login as USER; the person pastes only the password
  --inject-header NAME       the secret, as is, is the value of header NAME (X-Api-Key)
  --inject-dst query:PARAM   the secret, as is, is the URL parameter PARAM (sites can
                             log URLs)
  --inject-format 'FORMAT'   how the value is written; {} is the secret, once
                             ('Token {}'). It combines with --inject-dst or
                             --inject-header; alone, it goes in Authorization.
  Once saved, only a person can change how it is sent. Cards take no presets
  (those are for k2 vault set).

EXAMPLES
  k2 thread secret ops --name STRIPE_KEY --for api.stripe.com
  k2 thread secret ops --name JIRA_LOGIN --kind password --for acme.atlassian.net \
      --inject-dst basic --prompt "Your Jira email and API token, as email:token"
  k2 thread secret ops --name MAIL_API --for api.mail.example \
      --inject-dst basic --inject-format 'api:{}'      # user api; paste only the key
  k2 thread secret ops --name HOOK_TOKEN --for hooks.example.com \
      --inject-header X-Hook-Token --inject-format 'Token {}'
  k2 thread secret ops --name WEATHER_KEY --for api.weather.example --inject-dst query:appid
  k2 thread secret ops --name GITHUB_CLI --delivery env
  k2 thread secret ops --name LOGIN_CODE --scope session --delivery file --once

It prints the card id, and on stderr what the card says. Don't wait: you get a
Thread turn when it is saved. Wrong? k2 thread withdraw <addr> <card-id> and
ask again. --json prints the whole answer. More: k2 study vault
EOF
}

