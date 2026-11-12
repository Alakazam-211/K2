# ── Awareness Bus CLI verbs (0.34.0 Phase 3) ─────────────────────────
#
# Low-level signal emission + roster query. The primitives every
# agent uses to collaborate: "who's around?" + "tell them something
# right now" + "send them a notice for later."

# Emit a signal to another agent.
#
# Usage:
#   k2 signal <target-agent> <kind> <json-payload> [--inbox] [--from <sender>]
#
# Defaults:
#   kind     — "msg" if omitted
#   delivery — Live (real-time 1-on-1) unless --inbox present
#   from     — $K2SO_AGENT_NAME if set, else "cli"
cmd_signal() {
    local target="${1:-}"
    local kind="${2:-msg}"
    local raw_payload="${3:-{\}}"
    shift 3 2>/dev/null || true
    local delivery="live"
    local from="${K2SO_AGENT_NAME:-cli}"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --inbox) delivery="inbox"; shift ;;
            --from)  from="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    [[ -z "$target" ]] && {
        echo "Usage: k2 signal <target> <kind> <json-payload> [--inbox] [--from <sender>]" >&2
        echo "  Default kind is 'msg'; default delivery is 'live' (interrupts running session)." >&2
        echo "  --inbox sends as an intentional-async notice (target reads on their schedule)." >&2
        exit 1
    }

    # Build the AgentSignal wire format. Matches
    # k2so_core::awareness::AgentSignal serialization.
    local now
    now=$(python3 -c 'from datetime import datetime, timezone; print(datetime.now(timezone.utc).isoformat(timespec="microseconds"))' 2>/dev/null || date -u +"%Y-%m-%dT%H:%M:%S.000000+00:00")
    local signal_id
    signal_id=$(python3 -c 'import uuid; print(uuid.uuid4())' 2>/dev/null || echo "00000000-0000-0000-0000-000000000000")

    # Resolve the project UUID so the audit row's FK to projects.id
    # is satisfied. Fall back to the raw path if the lookup fails —
    # egress's _orphan sentinel catches that case and still writes
    # the audit row, just bucketed under _orphan instead of a real
    # project. Without this lookup the CLI would send the absolute
    # path as the workspace id, which FK-violates against
    # projects.id (UUIDs) and silently drops audit.
    local workspace_id="$PROJECT"
    # Endgame Stage A + dual-real heal (same as k2_core::db::resolve_home_db_path).
    local _k2_db
    _k2_db="$(_k2_home_db)"
    if [ -f "$_k2_db" ] && command -v sqlite3 >/dev/null 2>&1; then
        # SQL-escape single quotes in the path (doubled per SQL spec).
        local _safe_path="${PROJECT//\'/\'\'}"
        local _resolved
        _resolved=$(sqlite3 "$(_k2_db_uri "$_k2_db")" \
            "SELECT id FROM projects WHERE path = '${_safe_path}' LIMIT 1;" 2>/dev/null || true)
        if [ -n "$_resolved" ]; then
            workspace_id="$_resolved"
        fi
        unset _safe_path _resolved
    fi
    unset _k2_db

    local body
    body=$(python3 <<PY
import json, sys
kind = "$kind"
raw = """$raw_payload"""
# Map CLI kind argument → SignalKind JSON shape.
kind_map = {
    "msg":      lambda d: {"kind":"msg","data":{"text": d.get("text", "")}},
    "status":   lambda d: {"kind":"status","data":{"text": d.get("text", "")}},
    "presence": lambda d: {"kind":"presence","data":{"state": d.get("state", "active")}},
}
try:
    payload_data = json.loads(raw) if raw.strip() else {}
except json.JSONDecodeError:
    payload_data = {"text": raw}
if kind in kind_map:
    kind_json = kind_map[kind](payload_data)
else:
    kind_json = {"kind":"custom","data":{"kind": kind, "payload": payload_data}}

signal = {
    "id": "$signal_id",
    "from": {"scope":"agent","workspace":"$workspace_id","name":"$from"},
    "to":   {"scope":"agent","workspace":"$workspace_id","name":"$target"},
    "kind": kind_json,
    "priority": "normal",
    "delivery": "$delivery",
    "at": "$now",
}
print(json.dumps(signal))
PY
)

    cli_send cli_post_json "/cli/awareness/publish" "$body"
    echo
}

# `k2 sessions spawn` posted to /cli/sessions/spawn, a route removed in
# 0.39.0 (prd-k2-sidecar-cli-v1 SC28). It now only says where it moved.
cmd_sessions_spawn() {
    echo "k2 sessions spawn moved to: k2 sessions new <name> [--cli <cli>] (see k2 sessions --help)" >&2
    exit 2
}

# List every session this project's filesystem knows about, by
# walking $PROJECT/.k2so/sessions/ and summarizing each directory.
# No daemon round-trip — archive dirs are the source of truth.
#
# Output columns: session id, total archive bytes, segment count,
# active-segment size, last-modified wall-clock timestamp.
# Pass --json to get a machine-parseable array.
#
# Usage:
#   k2 sessions list [--json]
cmd_sessions_list() {
    local json_mode=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json_mode="1"; shift ;;
            --) shift; break ;;
            *) shift ;;
        esac
    done
    local sessions_dir="$(_ws_dot_dir "$PROJECT")/sessions"
    if [ ! -d "$sessions_dir" ]; then
        if [ -n "$json_mode" ]; then
            echo "[]"
        else
            echo "No sessions yet for $PROJECT" >&2
        fi
        return 0
    fi
    python3 - "$sessions_dir" "$json_mode" <<'PY'
import json, os, sys, time
sessions_dir = sys.argv[1]
as_json = sys.argv[2] == "1"

rows = []
for name in sorted(os.listdir(sessions_dir)):
    session_dir = os.path.join(sessions_dir, name)
    if not os.path.isdir(session_dir):
        continue
    total = 0
    segments = 0
    active_bytes = 0
    latest_mtime = 0
    for f in os.listdir(session_dir):
        path = os.path.join(session_dir, f)
        try:
            st = os.stat(path)
        except OSError:
            continue
        if not (f == "archive.ndjson" or f.startswith("archive.") and (f.endswith(".ndjson") or f.endswith(".ndjson.gz"))):
            continue
        total += st.st_size
        segments += 1
        latest_mtime = max(latest_mtime, int(st.st_mtime))
        if f == "archive.ndjson":
            active_bytes = st.st_size
    rows.append({
        "session_id": name,
        "total_bytes": total,
        "segments": segments,
        "active_bytes": active_bytes,
        "last_modified_epoch": latest_mtime,
    })

def human_bytes(n):
    if n < 1024:
        return f"{n} B"
    if n < 1024 * 1024:
        return f"{n / 1024:.2f} KB"
    if n < 1024 * 1024 * 1024:
        return f"{n / (1024 * 1024):.2f} MB"
    return f"{n / (1024 * 1024 * 1024):.2f} GB"

if as_json:
    print(json.dumps(rows, indent=2))
else:
    if not rows:
        print("No sessions yet", file=sys.stderr)
        sys.exit(0)
    print(f"{'SESSION':38}  {'SEGMENTS':>8}  {'TOTAL':>12}  {'ACTIVE':>12}  LAST MODIFIED")
    for r in rows:
        ts = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(r["last_modified_epoch"])) if r["last_modified_epoch"] else "-"
        print(f"{r['session_id']:38}  {r['segments']:>8}  {human_bytes(r['total_bytes']):>12}  {human_bytes(r['active_bytes']):>12}  {ts}")
PY
}

# Compact a session's rotated segments by gzipping them in place.
# NEVER touches the active segment (archive.ndjson). Idempotent —
# re-running is a no-op once every rotated segment is already
# gzipped.
#
# Usage:
#   k2 sessions compact <session-id>
cmd_sessions_compact() {
    local session_id="${1:-}"
    [[ -z "$session_id" ]] && {
        echo "Usage: k2 sessions compact <session-id>" >&2
        echo "(use 'k2 sessions list' to find session ids)" >&2
        exit 1
    }
    local session_dir="$(_ws_dot_dir "$PROJECT")/sessions/${session_id}"
    if [ ! -d "$session_dir" ]; then
        echo "No archive directory for session $session_id at $session_dir" >&2
        exit 1
    fi
    if ! command -v gzip >/dev/null 2>&1; then
        echo "gzip not found in PATH — cannot compact" >&2
        exit 1
    fi

    local count=0
    local compacted_bytes_before=0
    local compacted_bytes_after=0
    # Loop through every rotated segment that isn't yet gzipped.
    # Filename pattern: archive.NNN.ndjson (NOT archive.ndjson,
    # NOT *.ndjson.gz).
    while IFS= read -r -d '' seg; do
        local before_size
        before_size=$(stat -f '%z' "$seg" 2>/dev/null || stat -c '%s' "$seg" 2>/dev/null || echo 0)
        compacted_bytes_before=$((compacted_bytes_before + before_size))

        # `gzip -f` overwrites if a *.gz already exists; we checked
        # above that the .gz form doesn't exist for this segment.
        if gzip -f "$seg"; then
            local after_size
            after_size=$(stat -f '%z' "$seg.gz" 2>/dev/null || stat -c '%s' "$seg.gz" 2>/dev/null || echo 0)
            compacted_bytes_after=$((compacted_bytes_after + after_size))
            count=$((count + 1))
        else
            echo "gzip failed for $seg" >&2
        fi
    done < <(find "$session_dir" -maxdepth 1 -type f \
        -name 'archive.[0-9][0-9][0-9].ndjson' -print0 | sort -z)

    if [ "$count" -eq 0 ]; then
        echo "Nothing to compact for session $session_id (no uncompressed rotated segments)." >&2
        return 0
    fi

    local saved=$((compacted_bytes_before - compacted_bytes_after))
    echo "{\"sessionId\":\"$session_id\",\"compacted\":$count,\"bytesBefore\":$compacted_bytes_before,\"bytesAfter\":$compacted_bytes_after,\"bytesSaved\":$saved}"
}

# Sessions subcommand dispatcher — handles `k2 sessions spawn`,
# 0.37.11 — List LIVE v2 sessions (daemon memory) for a workspace.
# Wraps /cli/sessions/list-for-workspace.  Distinct from
# `k2 sessions list` (which inspects on-disk archive segments) —
# this verb answers "what's the daemon currently holding open for
# this workspace?", which is what you need to verify focus-window
# adoption (sessions count shouldn't change when a focus window
# opens on top of an existing workspace).
#
# 0.46.6: a refusal or error body (an object, not the list) prints the
# daemon's error and exits 3 (refused) or 1, instead of a header and a
# python AttributeError. Values reach python through the environment,
# never spliced into its source. A workspace name or handle is resolved
# to its path first (a directory is taken as is).
#
# Usage:
#   k2 sessions live <workspace>          # human table
#   k2 sessions live <workspace> --json   # raw JSON from daemon
#   k2 sessions live <workspace> --count  # just the integer count
cmd_sessions_live() {
    local path="${1:-}"
    local mode="table"
    shift 2>/dev/null || true
    while [ $# -gt 0 ]; do
        case "$1" in
            --json)  mode="json"; shift ;;
            --count) mode="count"; shift ;;
            *) shift ;;
        esac
    done
    if [ -z "$path" ]; then
        echo "Usage: k2 sessions live <workspace> [--json|--count]" >&2
        echo "  Lists live daemon-side v2 sessions whose cwd is under the workspace (a path, handle or name)." >&2
        exit 1
    fi
    if [ -d "$path" ]; then
        path="$(cd "$path" && pwd)"
    else
        local resolved
        resolved=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$path")" 2>/dev/null \
            | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    d = {}
print(d.get("path", "") if isinstance(d, dict) else "")' 2>/dev/null || true)
        [ -n "$resolved" ] && path="$resolved"
    fi
    # Try the targeted endpoint first (post-0.37.11 daemons); fall back
    # to `/cli/agents/running` with client-side filtering for older
    # daemons that haven't picked up the new route yet.
    local response
    response=$(cli_request "/cli/sessions/list-for-workspace" "path=$(urlencode "$path")" 2>/dev/null)
    local fallback=0
    if echo "$response" | grep -q '"route not found"' 2>/dev/null; then
        fallback=1
        response=$(cli_request "/cli/agents/running")
    fi
    printf '%s' "$response" | SL_MODE="$mode" SL_PATH="$path" SL_FALLBACK="$fallback" python3 -c '
import json, os, sys
mode = os.environ["SL_MODE"]
path = os.environ["SL_PATH"]
fallback = os.environ["SL_FALLBACK"] == "1"
raw = sys.stdin.read()
try:
    data = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
if not isinstance(data, list):
    err = data.get("error") if isinstance(data, dict) else None
    code = ""
    if isinstance(err, dict):
        code = str(err.get("code") or "")
        msg = err.get("hint") or err.get("message") or code or json.dumps(err)
    elif err:
        msg = str(err)
        code = msg
    else:
        msg = "unexpected answer: %s" % raw[:300]
    print("Error: %s" % msg, file=sys.stderr)
    refused = isinstance(data, dict) and (
        "permission" in data
        or any(w in (code + " " + msg).lower() for w in ("owner", "forbidden", "credential", "token", "person", "not allowed"))
    )
    if refused:
        print("This is the owner'"'"'s raw PTY view. To see who is working in your workspace, run: k2 sessions", file=sys.stderr)
    sys.exit(3 if refused else 1)
def matches(cwd):
    if not cwd:
        return False
    p = path.rstrip("/")
    c = cwd.rstrip("/")
    return c == p or cwd.startswith(p + "/")
if fallback:
    sessions = [
        {
            "sessionId": s.get("terminalId"),
            "agentName": s.get("agentName"),
            "command": s.get("command"),
            "args": [],
            "cwd": s.get("cwd") or "",
            # agents/running only returns live sessions; v2 is assumed.
            "isV2": True,
        }
        for s in data
        if isinstance(s, dict) and matches(s.get("cwd") or "")
    ]
    note = "  (using /cli/agents/running fallback — daemon pre-0.37.11)"
else:
    sessions = [s for s in data if isinstance(s, dict)]
    note = ""
if mode == "json":
    print(json.dumps(sessions, indent=2) if fallback else raw)
    sys.exit(0)
if mode == "count":
    print(len(sessions))
    sys.exit(0)
if not sessions:
    print("No live sessions in this workspace." + note)
    sys.exit(0)
print("%d live session(s):%s" % (len(sessions), note))
print("  %-38s  %-10s  %-22s  %-24s  %-14s  CWD" % ("TERMINAL_ID", "KIND", "HANDLE", "AGENT", "CMD"))
for s in sessions:
    sid = s.get("sessionId") or s.get("terminalId") or "?"
    kind = (s.get("kind") or "")[:10]
    handle = (s.get("handle") or "")[:22]
    name = (s.get("agentName") or "?")[:24]
    cmd = (s.get("command") or "shell")[:14]
    cwd = s.get("cwd") or ""
    print("  %-38s  %-10s  %-22s  %-24s  %-14s  %s" % (sid, kind, handle, name, cmd, cwd))
'
}

# `k2 sessions list`, `k2 sessions compact`, and any future
# `sessions *` verbs.
cmd_sessions() {
    local sub="${1:-}"
    case "$sub" in
        help|--help|-h) _sessions_usage; return 0 ;;
        ""|-*) cmd_sessions_roster "$@"; return ;;
    esac
    shift
    case "$sub" in
        list|ls|past|show|open|new|stop|main)
            if _wants_help "$@"; then
                _sessions_usage
                return 0
            fi ;;
    esac
    case "$sub" in
        main)      cmd_sessions_main "$@" ;;
        list|ls)   cmd_sessions_roster "$@" ;;
        past)      cmd_sessions_roster --past "$@" ;;
        show)      cmd_sessions_show "$@" ;;
        open)      cmd_sessions_open "$@" ;;
        new)       cmd_sessions_new "$@" ;;
        stop)      cmd_sessions_stop "$@" ;;
        archives)  cmd_sessions_list "$@" ;;
        spawn)     cmd_sessions_spawn "$@" ;;
        live)      cmd_sessions_live "$@" ;;
        compact)   cmd_sessions_compact "$@" ;;
        set-label) cmd_sessions_set_label "$@" ;;
        *)
            echo "usage: unknown sessions verb '$sub' (k2 sessions --help)" >&2
            exit 2
            ;;
    esac
}

