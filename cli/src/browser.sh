# ── Design mode: k2 browser note (prd-browser-design-mode-v1 D-A13) ───────
# Files a page note into a workspace's Thread exactly like a Design tab's
# Send: ONE Thread item (design origin) that wakes + injects the pinned
# Chat. Agents post only on their own workspace (passport); people need a
# login (Connect logins also need the workspace's remote-instruct opt-in).
cmd_help_browser() {
    cat <<'EOF'
%include cli/text/help/browser.txt
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
            --)
                # The rest is the note text, as typed.
                shift
                [ $# -le 1 ] || { echo "Error: one note text only (quote it)" >&2; exit 2; }
                [ -z "$comment" ] || { echo "Error: one note text only (quote it)" >&2; exit 2; }
                [ $# -eq 0 ] || { comment="$1"; shift; } ;;
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
%include cli/text/help/workspace/design-tab.txt
EOF
            ;;
        mobile)
            cat <<'EOF'
%include cli/text/help/workspace/mobile-tab.txt
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
        [ "$arg" = "--" ] && break
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
            --from|--options|--option|--name|--prompt|--dest|--timeout|--style|--limit|--before-seq|--seq|--file|--scope|--for|--delivery|--kind|--ttl|--inject-dst|--inject-header|--inject-format|--why|--reason|--idempotency-key)
                skip_next=1
                continue
                ;;
            --) break ;;
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
    local idem_key=""
    local -a files=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json="true"; shift ;;
            --idempotency-key)
                [[ -n "${2:-}" ]] || { echo "Error: --idempotency-key needs the key a 'may have been delivered' line printed" >&2; exit 2; }
                idem_key="$2"; shift 2 ;;
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
            --)
                # The rest is the address and the text, as typed.
                shift
                while [[ $# -gt 0 ]]; do
                    if [[ -z "$target" ]]; then target="$1"; else text="$1"; fi
                    shift
                done ;;
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
        echo "       k2 thread ask <addr> \"prompt\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")" >&2
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
        # 0.46.15: one key per post, as the Thread's own clientMsgId
        # (post-idempotency-v1): a retry after no answer posts once.
        _k2_idem_init "$idem_key"
        local params="addr=$(urlencode "$target")"
        params="${params}&text=$(urlencode "$text")"
        params="${params}&from=$(urlencode "$from")"
        params="${params}&via=thread"
        params="${params}&clientMsgId=$(urlencode "$_K2_IDEM_KEY")"
        if [[ ${#files[@]} -gt 0 ]]; then
            local attachments_json
            attachments_json="$(_thread_attachments_json "${files[@]}")" || exit 1
            params="${params}&attachments=$(urlencode "$attachments_json")"
        fi
        local response
        _permission_note_reset
        response=$(_k2_idem_send cli_post_form "/cli/thread/post" "$params")
        local rc=$?
        _k2_moved_from_note "$response"
        _k2_idem_replay_note "$response"
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
    if doc.get("deleted") is True:
        # thread-message-delete-v1: its sender deleted it; the server keeps
        # no words, so say that rather than print an empty message.
        body="(message deleted)"
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

# A card's --options value splits on ';' only; a comma is part of a label.
# A value with a comma and no ';' stays one option: print the one note that
# says so (never a refusal, never a guess at where it should split).
_k2_options_one_note() {
    if [[ "$1" == *,* && "$1" != *\;* ]]; then
        echo "--options splits on ';'; this became one option. Use ';' between options, or repeat --option" >&2
    fi
    return 0
}

cmd_thread_ask() {
    local target="" prompt="" json="false" explicit_from=""
    local options_csv="" allow_custom="false" expires="" no_expiry="false" idem_key=""
    local option_list=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            ask) shift ;;
            --json) json="true"; shift ;;
            --idempotency-key)
                [[ -n "${2:-}" ]] || { echo "Error: --idempotency-key needs the key a 'may have been delivered' line printed" >&2; exit 2; }
                idem_key="$2"; shift 2 ;;
            --from)
                [[ $# -ge 2 ]] || { echo "Error: --from requires a value" >&2; exit 1; }
                explicit_from="$2"; shift 2 ;;
            --options)
                [[ $# -ge 2 ]] || { echo "Error: --options requires a list with ';' between options" >&2; exit 1; }
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
            --)
                # The rest is the address and the prompt, as typed.
                shift
                while [[ $# -gt 0 ]]; do
                    if [[ -z "$target" ]]; then target="$1"
                    elif [[ -z "$prompt" ]]; then prompt="$1"
                    else
                        echo "Error: unexpected extra argument '$1'" >&2
                        exit 1
                    fi
                    shift
                done ;;
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
    [[ -n "$target" ]] || { echo "Error: address required" >&2; echo "Usage: k2 thread ask <addr> \"<prompt>\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")" >&2; exit 1; }
    [[ -n "$prompt" ]] || { echo "Error: prompt required" >&2; echo "Usage: k2 thread ask <addr> \"<prompt>\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")" >&2; exit 1; }
    if [[ -z "$options_csv" && ${#option_list[@]} -eq 0 ]]; then
        echo "Error: ask requires --option \"…\" (repeatable) or --options \"a; b; c\"" >&2
        exit 1
    fi
    if [[ -n "$expires" && "$no_expiry" == "true" ]]; then
        echo "Error: use --expires or --no-expiry, not both" >&2
        exit 1
    fi
    local from option_json=""
    from="$(_thread_sender_from "$explicit_from")"
    # The daemon splits --options on ';'. A value that starts with '[' is a
    # JSON list the daemon reads whole, so it gets no note.
    local options_lead="${options_csv#"${options_csv%%[![:space:]]*}"}"
    [[ "$options_lead" == \[* ]] || _k2_options_one_note "$options_csv"
    if [[ ${#option_list[@]} -gt 0 ]]; then
        option_json=$(printf '%s\n' "${option_list[@]}" | python3 -c 'import json,sys; print(json.dumps([ln.rstrip("\n") for ln in sys.stdin]))')
    fi
    # 0.46.15: one card per invocation (BEGIN_CLI_SEND_ONCE).
    _k2_idem_init "$idem_key"
    local body
    body=$(ADDR="$target" PROMPT="$prompt" FROM="$from" OPTIONS_CSV="$options_csv" \
        OPTIONS_JSON="$option_json" ALLOW="$allow_custom" EXPIRES="$expires" NO_EXPIRY="$no_expiry" \
        IDEM_KEY="$_K2_IDEM_KEY" python3 -c 'import json,os
obj={"addr":os.environ["ADDR"],"prompt":os.environ["PROMPT"],"from":os.environ.get("FROM","k2"),"idempotency_key":os.environ["IDEM_KEY"]}
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
    _permission_note_reset
    response=$(_k2_idem_send cli_post_json "/cli/thread/ask" "$body")
    local rc=$?
    _k2_moved_from_note "$response"
    _k2_idem_replay_note "$response"
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
# The number names the card in the answer line: chose "<option>" on card #<seq>.
seq=d.get("seq")
print("%s  card #%s" % (d.get("id",""), seq) if seq is not None else d.get("id",""))
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
%include cli/text/help/thread.txt
EOF
}

cmd_help_thread_secret() {
    cat <<'EOF'
%include cli/text/help/thread/secret.txt
EOF
}

