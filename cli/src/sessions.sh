# ── k2 sessions (prd-workspace-sessions-cli-v1) ──────────────────────
# Who is working in this workspace, past conversations to reopen, and
# helpers to start or stop. The daemon owns the list (GET
# /cli/sessions/roster|past|show, POST /cli/sessions/open) and the one-
# runner rule; `new`/`stop` are `k2 sidecar new|stop`. An `id` tool: inside
# a K2 terminal the request carries this session's passport.

_sessions_usage() {
    cat <<'EOF'
k2 sessions — who is working in this workspace, and past conversations to reopen

  k2 sessions [--all] [--json]                 live sessions here (* marks you), with their state
  k2 sessions --past [--search <text>] [--days N] [--cli <cli>] [--limit N] [--json]
                                               past conversations in this workspace, newest first
  k2 sessions show <id|address> [--tail N] [--json]
                                               what it was about and its last messages
  k2 sessions open <id> [--name <name>] [--brief <file>|-] [--brief-text "…"] [--force] [--json]
                                               reopen a past conversation as a sidecar, same history
  k2 sessions new <name> [--cli <cli>] [--brief <file>|-] [--brief-text "…"] [--json]
                                               start a helper (a sidecar); prints its address <ws>/<name>
  k2 sessions stop <name|address> [--force]    stop is sleep: its history stays (open or msg wakes it)
  k2 sessions main [--json]                    which chat is pinned as this workspace's main chat, and whether K2 can find it
  k2 sessions main set <id> [--json]           choose the main chat, an <id> from --past (people only); takes effect the next time it starts

Talk to any of them: k2 msg <address> "…"      (a sleeping one wakes)
                     k2 msg --session <id> "…" (delivers to whoever holds it; a past one is reopened first)
Replies arrive in your terminal as [from <address>] …; answer with k2 msg <address>.
One conversation never runs twice: opening a live one prints its address.
<id> is the short id from --past (6+ characters), the full id, or an address.
Agents see their own workspace only. Opening, starting and stopping follow
"Allow hiring and managing agents" and the main-session rule (k2 study sessions).
CLIs: claude, codex, grok, gemini, cursor (cursor-agent), pi, hermes.
Owner: --workspace <ws> on every verb. Also: archives (on-disk archives),
live <ws> (the owner's raw PTY view), compact, set-label.
More: k2 study sessions
EOF
}

# Render a `k2 sessions` answer (`<body>\n<status>`), then exit.
_sessions_render() {
    local verb="$1" as_json="${2:-0}"
    _sidecar_split "${3:-}"
    if [ "$_SC_STATUS" != "200" ]; then
        if [ "$_SC_STATUS" = "404" ] && [[ "$_SC_BODY" == *"route not found"* || "$_SC_BODY" == *"Not Found"* ]]; then
            echo "daemon_too_old: this K2 daemon has no k2 sessions routes yet (update K2: k2 update --app)" >&2
            exit 1
        fi
        _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$_SC_BODY"
        exit 0
    fi
    printf '%s' "$_SC_BODY" | K2_SS_VERB="$verb" python3 -c '
import json, os, sys, time
verb = os.environ["K2_SS_VERB"]
v = json.load(sys.stdin)
now = time.time()
def ago(t):
    if not t:
        return "-"
    d = max(0, int(now - t))
    if d < 60: return "now"
    if d < 3600: return "%dm" % (d // 60)
    if d < 86400: return "%dh" % (d // 3600)
    return "%dd" % (d // 86400)
def cut(s, n):
    s = (s or "").replace("\n", " ").strip()
    return s if len(s) <= n else s[: n - 1] + "…"
if verb == "roster":
    rows = v.get("sessions", [])
    sc = v.get("sidecars") or {}
    ws = (v.get("workspace") or {}).get("handle") or "?"
    live = [r for r in rows if r.get("state") != "asleep"]
    print("%s — %d live (cap %d sidecars; %d in use)" % (ws, len(live), sc.get("cap", 0), sc.get("live", 0)))
    if rows:
        fmt = "%s %-16s %-9s %-7s %-8s %-7s %s"
        print(fmt % (" ", "ADDRESS", "ROLE", "CLI", "STATE", "ACTIVE", "TITLE / BRIEF"))
        for r in rows:
            addr = r.get("address") or r.get("handle") or "-"
            title = cut(r.get("title"), 60)
            print(fmt % ("*" if r.get("you") else " ", addr, r.get("role") or "-", r.get("cli") or "-",
                         r.get("state") or "-", ago(r.get("lastActiveAt")), title))
        example = next((r.get("address") for r in rows if r.get("address") and not r.get("you")), None)
        hint = ("Message one: k2 msg %s \"…\"" % example) if example else "Message one: k2 msg <address> \"…\""
        print(("* = you.  " if any(r.get("you") for r in rows) else "") + hint)
    else:
        print("(nobody is running here)")
    asleep = v.get("asleep", 0)
    tail = []
    if asleep and not any(r.get("state") == "asleep" for r in rows):
        tail.append("%d asleep (k2 sessions --all)" % asleep)
    tail.append("past conversations: k2 sessions --past")
    print(" · ".join(tail))
elif verb == "past":
    rows = v.get("conversations", [])
    ws = v.get("workspace") or "?"
    what = "last %d days" % v.get("days", 14)
    if v.get("search"):
        what += ", matching \"%s\"" % v["search"]
    if v.get("cli"):
        what += ", %s only" % v["cli"]
    print("Past conversations in %s (%s):" % (ws, what))
    if not rows:
        print("  (none found; try --days 90, or another --search)")
    else:
        fmt = "  %-10s %-7s %-5s %-24s %-34s %s"
        print(fmt % ("ID", "CLI", "LAST", "WHERE", "TITLE", "NOW"))
        for r in rows:
            run = r.get("runner") or {}
            if run:
                now_s = "%s as %s" % (run.get("state") or "live", run.get("address") or "-")
            else:
                now_s = "-"
            title = r.get("customName") or r.get("title")
            print(fmt % (r.get("id"), r.get("cli"), ago(r.get("lastActiveAt")), cut(r.get("worktree") or "(root)", 24), cut(title, 34), now_s))
            if r.get("match"):
                print("             match: %s" % cut(r["match"], 110))
        first = next((r for r in rows if not r.get("runner")), None)
        if first:
            print("Open one: k2 sessions open %s --name <name>   ·   Read first: k2 sessions show %s" % (first["id"], first["id"]))
        else:
            print("Every one listed is held already: k2 msg its address (NOW column).")
    if v.get("truncated"):
        sc = v.get("scanned") or {}
        print("(search stopped after %d files; narrow with --days or --cli)" % sc.get("files", 0))
    for u in v.get("unsupported") or []:
        print("note: %s: %s" % (u.get("cli"), u.get("why")), file=sys.stderr)
elif verb == "show":
    c = v.get("conversation") or {}
    where = c.get("worktree") or "root"
    head = "%s  %s  \"%s\"   last active %s ago" % (c.get("id"), c.get("cli"), cut(c.get("customName") or c.get("title"), 60), ago(c.get("lastActiveAt")))
    if v.get("messageCount") is not None:
        head += " · %d messages" % v["messageCount"]
    head += " · %s" % where
    if c.get("branch"):
        head += " · branch %s" % c["branch"]
    print(head)
    run = c.get("runner")
    if v.get("unsupported"):
        print("(%s)" % v["unsupported"])
    else:
        if v.get("firstAsk"):
            print("Asked first: \"%s\"" % cut(v["firstAsk"], 200))
        msgs = v.get("messages") or []
        print("--- last %d messages ---" % len(msgs))
        for m in msgs:
            print("[%s %s]  %s" % (m.get("role"), ago(m.get("at")) if m.get("at") else "", m.get("text", "")))
        if v.get("toolCalls"):
            print("(%d tool calls not shown)" % v["toolCalls"])
    if run and run.get("address"):
        print("It is %s as %s. Message it: k2 msg %s \"…\"" % (run.get("state"), run["address"], run["address"]))
    else:
        print("Reopen: k2 sessions open %s   (or k2 msg --session %s \"…\")" % (c.get("id"), c.get("id")))
elif verb == "open":
    print(v.get("address"))
    if v.get("opened"):
        what = "resumed its tab" if v.get("outcome") == "resumed_row" else "reopened"
        sys.stderr.write("%s %s (%s, same conversation). Message it: k2 msg %s \"…\"\n" % (what, v["address"], v.get("cli"), v["address"]))
    elif v.get("alreadyLive"):
        sys.stderr.write("already running as %s. Message it: k2 msg %s \"…\"\n" % (v["address"], v["address"]))
    else:
        sys.stderr.write("%s is the main session (asleep). Wake it: %s\n" % (v["address"], v.get("wakeWith") or ("k2 msg %s \"…\"" % v["address"])))
elif verb == "main":
    m = v.get("main") or {}
    ws = v.get("workspace") or "?"
    state = m.get("state")
    if state == "none":
        print("%s has no main chat pinned yet (it starts on the next message)." % ws)
    elif state == "missing":
        sys.stderr.write("%s main chat %s (%s) CANNOT BE FOUND: no conversation on disk in any of its folders.\n" % (ws, m.get("conversationId"), m.get("cli")))
        sys.stderr.write("K2 kept it pinned and did not start or switch to another chat.\n")
        sys.stderr.write("Choose one: k2 sessions --past, then k2 sessions main set <id>\n")
        sys.exit(1)
    elif state == "unstarted":
        print("%s main chat %s (%s): pre-allocated, no first turn yet." % (ws, m.get("conversationId"), m.get("cli")))
    elif state == "custom":
        print("%s main chat %s (%s): a custom preset; K2 cannot check it on disk." % (ws, m.get("conversationId"), m.get("cli")))
    else:
        print("%s main chat %s (%s)%s" % (ws, m.get("conversationId"), m.get("cli"), "\n  " + m["path"] if m.get("path") else ""))
elif verb == "main_set":
    m = v.get("main") or {}
    print("%s main chat is now %s (%s) \"%s\"" % (v.get("workspace"), m.get("conversationId"), m.get("cli"), cut(v.get("title"), 60)))
    if v.get("previous") and v.get("previous") != m.get("conversationId"):
        print("Was: %s (its history stays; k2 sessions open %s)" % (v["previous"], v["previous"][:8]))
    print("It takes effect the next time the chat starts. Restart it now: %s" % v.get("restartWith"))
'
    exit $?
}

cmd_sessions_roster() {
    local workspace="" as_json=0 all=0 past=0 search="" days="" cli="" limit="" archived=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --all|-a) all=1; shift ;;
            --json) as_json=1; shift ;;
            --past) past=1; shift ;;
            --search) search="${2:-}"; past=1; shift 2 ;;
            --days) days="${2:-}"; past=1; shift 2 ;;
            --cli) cli="${2:-}"; past=1; shift 2 ;;
            --limit) limit="${2:-}"; past=1; shift 2 ;;
            --archived) archived=1; past=1; shift ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            -h|--help) _sessions_usage; exit 0 ;;
            *) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;
        esac
    done
    [ -z "$workspace" ] && workspace="$PROJECT"
    local q raw
    if [ "$past" = "1" ]; then
        q="workspace=$(urlencode "$workspace")"
        [ -n "$search" ] && q="${q}&search=$(urlencode "$search")"
        [ -n "$days" ] && q="${q}&days=$(urlencode "$days")"
        [ -n "$cli" ] && q="${q}&cli=$(urlencode "$cli")"
        [ -n "$limit" ] && q="${q}&limit=$(urlencode "$limit")"
        [ "$archived" = "1" ] && q="${q}&archived=1"
        raw="$(_sidecar_send GET /cli/sessions/past "$q")" || true
        _sessions_render past "$as_json" "$raw"
    fi
    q="workspace=$(urlencode "$workspace")"
    [ "$all" = "1" ] && q="${q}&all=1"
    raw="$(_sidecar_send GET /cli/sessions/roster "$q")" || true
    _sessions_render roster "$as_json" "$raw"
}

# Which chat is the workspace's pinned main chat, and choosing it.
cmd_sessions_main() {
    local action="" id="" workspace="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) workspace="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            set)
                if [ -z "$action" ]; then action="set"; shift; continue; fi
                id="$1"; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;
            *)
                if [ "$action" = "set" ] && [ -z "$id" ]; then id="$1"; shift; else
                    echo "usage: k2 sessions main [--json]  |  k2 sessions main set <id> [--json]" >&2; exit 2
                fi ;;
        esac
    done
    [ -z "$workspace" ] && workspace="$PROJECT"
    local raw
    if [ "$action" = "set" ]; then
        if [ -z "$id" ]; then
            echo "usage: k2 sessions main set <id>  (ids: k2 sessions --past)" >&2; exit 2
        fi
        local body
        body="$(K2_SS_ID="$id" K2_SS_WS="$workspace" python3 -c 'import json, os; print(json.dumps({"workspace": os.environ["K2_SS_WS"], "id": os.environ["K2_SS_ID"]}))')"
        raw="$(_sidecar_send POST /cli/sessions/main/set "$body")" || true
        _sessions_render main_set "$as_json" "$raw"
    fi
    raw="$(_sidecar_send GET /cli/sessions/main "workspace=$(urlencode "$workspace")")" || true
    _sessions_render main "$as_json" "$raw"
}

cmd_sessions_show() {
    local id="" workspace="" tail="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tail) tail="${2:-}"; shift 2 ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    echo "usage: k2 sessions show <id|address> [--tail N] [--json]" >&2; exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        echo "usage: k2 sessions show <id|address> [--tail N] [--json]  (ids: k2 sessions --past)" >&2
        exit 2
    fi
    [ -z "$workspace" ] && workspace="$PROJECT"
    local q="workspace=$(urlencode "$workspace")&id=$(urlencode "$id")"
    [ -n "$tail" ] && q="${q}&tail=$(urlencode "$tail")"
    local raw
    raw="$(_sidecar_send GET /cli/sessions/show "$q")" || true
    _sessions_render show "$as_json" "$raw"
}

# Read --brief <file>|- / --brief-text into a temp file path ($_SS_BRIEF_SRC)
# for the JSON builder. Exit 2 on a usage error.
_sessions_brief_src() {
    local brief_file="$1" have_text="$2"
    _SS_BRIEF_SRC=""
    if [ -n "$brief_file" ] && [ "$have_text" = "1" ]; then
        echo "usage: pass --brief or --brief-text, not both" >&2
        exit 2
    fi
    if [ -n "$brief_file" ]; then
        if [ "$brief_file" = "-" ]; then
            _SS_BRIEF_SRC="$(mktemp -t k2-sessions-brief-XXXXXX)"
            cat > "$_SS_BRIEF_SRC"
        elif [ -f "$brief_file" ]; then
            _SS_BRIEF_SRC="$brief_file"
        else
            echo "usage: brief file not found: $brief_file" >&2
            exit 2
        fi
    fi
}

cmd_sessions_open() {
    local id="" name="" brief_file="" brief_text="" have_text=0 workspace="" harness="" model="" force=0 as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --name) name="${2:-}"; shift 2 ;;
            --brief) brief_file="${2:-}"; shift 2 ;;
            --brief-text) brief_text="${2:-}"; have_text=1; shift 2 ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            --harness) harness="${2:-}"; shift 2 ;;
            --model) model="${2:-}"; shift 2 ;;
            --force) force=1; shift ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    echo "usage: k2 sessions open <id> [--name <name>] [--brief <file>|-] [--force]" >&2; exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        echo "usage: k2 sessions open <id> [--name <name>] [--brief <file>|-] [--force]  (ids: k2 sessions --past)" >&2
        exit 2
    fi
    _sessions_brief_src "$brief_file" "$have_text"
    [ -z "$workspace" ] && workspace="$PROJECT"
    local body rc=0
    body="$(K2_SS_ID="$id" K2_SS_NAME="$name" K2_SS_WS="$workspace" K2_SS_HARNESS="$harness" K2_SS_MODEL="$model" \
        K2_SS_FORCE="$force" K2_SS_BRIEF_FILE="$_SS_BRIEF_SRC" K2_SS_BRIEF_TEXT="$brief_text" K2_SS_HAVE_TEXT="$have_text" python3 -c '
import json, os, sys
b = {"workspace": os.environ["K2_SS_WS"], "id": os.environ["K2_SS_ID"], "force": os.environ["K2_SS_FORCE"] == "1"}
for k, e in (("name", "K2_SS_NAME"), ("harness", "K2_SS_HARNESS"), ("model", "K2_SS_MODEL")):
    if os.environ.get(e):
        b[k] = os.environ[e]
path = os.environ.get("K2_SS_BRIEF_FILE")
if path:
    try:
        b["brief"] = open(path, "rb").read().decode("utf-8")
    except UnicodeDecodeError:
        sys.stderr.write("brief_not_text: the brief is not UTF-8 text\n"); sys.exit(2)
elif os.environ.get("K2_SS_HAVE_TEXT") == "1":
    b["brief"] = os.environ.get("K2_SS_BRIEF_TEXT", "")
print(json.dumps(b))
')" || rc=$?
    [ "$brief_file" = "-" ] && rm -f "$_SS_BRIEF_SRC"
    [ "$rc" -ne 0 ] && exit 2
    local raw
    raw="$(_sidecar_send POST /cli/sessions/open "$body")" || true
    _sessions_render open "$as_json" "$raw"
}

cmd_sessions_new() {
    local name="" cli="" harness="" brief_file="" brief_text="" have_text=0 workspace="" model="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --cli) cli="${2:-}"; shift 2 ;;
            --harness) harness="${2:-}"; shift 2 ;;
            --brief) brief_file="${2:-}"; shift 2 ;;
            --brief-text) brief_text="${2:-}"; have_text=1; shift 2 ;;
            --workspace) workspace="${2:-}"; shift 2 ;;
            --model) model="${2:-}"; shift 2 ;;
            --worktree|--base)
                echo "usage: k2 sessions new has no $1. For a sidecar in its own git worktree: k2 sidecar new <name> --harness <preset> --worktree [--base <ref>] (k2 sidecar --help)" >&2
                exit 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sessions --help)" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; else
                    echo "usage: one name only; quote names with spaces (k2 sessions --help)" >&2; exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        echo "usage: k2 sessions new <name> [--cli <cli>] [--brief <file>|-] [--brief-text \"…\"]" >&2
        exit 2
    fi
    if [ -n "$cli" ] && [ -n "$harness" ]; then
        echo "usage: pass --cli or --harness, not both" >&2
        exit 2
    fi
    # --cli names the CLI (cursor runs cursor-agent); --harness names a preset.
    case "$cli" in
        "") ;;
        cursor) harness="cursor-agent" ;;
        claude|codex|grok|gemini|cursor-agent|pi|hermes) harness="$cli" ;;
        *) echo "usage: unknown --cli '$cli' (claude, codex, grok, gemini, cursor, pi, hermes)" >&2; exit 2 ;;
    esac
    _sessions_brief_src "$brief_file" "$have_text"
    [ -z "$workspace" ] && workspace="$PROJECT"
    local body rc=0
    body="$(K2_SC_NAME="$name" K2_SC_HARNESS="$harness" K2_SC_WS="$workspace" K2_SC_MODEL="$model" \
        K2_SC_BRIEF_FILE="$_SS_BRIEF_SRC" K2_SC_BRIEF_TEXT="$brief_text" K2_SC_HAVE_TEXT="$have_text" python3 -c '
import json, os, sys
b = {"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"],
     "harness": os.environ["K2_SC_HARNESS"], "defaultHarness": not os.environ["K2_SC_HARNESS"]}
if os.environ.get("K2_SC_MODEL"):
    b["model"] = os.environ["K2_SC_MODEL"]
path = os.environ.get("K2_SC_BRIEF_FILE")
if path:
    try:
        b["brief"] = open(path, "rb").read().decode("utf-8")
    except UnicodeDecodeError:
        sys.stderr.write("brief_not_text: the brief is not UTF-8 text\n"); sys.exit(2)
elif os.environ.get("K2_SC_HAVE_TEXT") == "1":
    b["brief"] = os.environ.get("K2_SC_BRIEF_TEXT", "")
print(json.dumps(b))
')" || rc=$?
    [ "$brief_file" = "-" ] && rm -f "$_SS_BRIEF_SRC"
    [ "$rc" -ne 0 ] && exit 2
    local raw
    raw="$(_sidecar_send POST /cli/sidecar/new "$body")" || true
    _sidecar_render new "$as_json" "$raw"
}

cmd_sessions_stop() {
    cmd_sidecar_stop "$@"
}

