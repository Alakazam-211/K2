# ── Context management stack — `k2 agent context …` (prd-context-hamburger-v1) ──
#
# Day-2 stack management for always-on AGENTS.md layers. Daemon SSOT via
# /cli/context/* (owner token + project= like other agent verbs).
# Exit codes match the agent family: 0 ok · 1 runtime · 2 usage ·
# 3 precondition (duplicate_layer etc.) · 4 not found.

cmd_help_agent_context() {
    cat <<'EOF'
k2 agent context — manage the always-on AGENTS.md context management stack

The host library is Settings → Context Catalog (pack.toml + layer.md). This
command stacks those packs onto a workspace; it does not author the library.

System (pinned) layers: Agent persona, PROJECT.md, Tooling footer — toggleable
with on/off (ids pinned:agent|pinned:project|pinned:tooling, or short aliases
agent|project|tooling). Optional layers are ordered path references you can
add, toggle, reorder, or remove. The daemon composes them into .k2/AGENTS.md
on every mutate. Hire can seed layers: k2 agent hire <dir> --context <path|catalog-id>

USAGE
  k2 agent context <command> [args] [flags]

COMMANDS
  list                    Show system layers (on/off) + optional layers
  add <path|id>           Add an optional layer (workspace path or catalog id —
                          run `catalog` for the full local list)
  remove <id|path>        Drop an optional layer (file on disk is kept)
  on <id|path|system>     Enable optional or system layer
  off <id|path|system>    Disable optional or system layer
  move <id|path> <where>  Reorder: absolute position n, or up|down|top|bottom
  show [--outline]        Preview composed AGENTS.md (or section outline)
  regen                   Force-compose AGENTS.md from the current stack
  catalog                 List the local context catalog (path / static / live)

LOCAL CATALOG (also hire --context)
  Path:   wiki:index, wiki:home
  Static: wiki:hygiene, subagents:pack, browser:pack, manager:pack, kessel:pack
  Live:   connections:roster, heartbeats:roster, skills:roster, users:roster
  (Aliases work too — e.g. manager, connections, hygiene, subagents, browser.)

WORKSPACE
  Resolved from K2_PROJECT_PATH / K2SO_PROJECT_PATH / PWD (same as other
  agent verbs). Override with --workspace / --project (path, name, or uuid)
  so an orchestrator can recompose a peer without cwd-switching.

FLAGS
  --workspace|--project <ws>  Target workspace (path, name, or uuid)
  --label <name>          With add: optional display label
  --outline               With show: section headings only
  --json                  Machine-readable output (catalog: full metadata)

EXIT CODES
  0 ok · 1 error · 2 bad usage · 3 precondition (e.g. duplicate layer) · 4 not found

EXAMPLES
  k2 agent context list
  k2 agent context catalog
  k2 agent context catalog --json
  k2 agent context add wiki:index
  k2 agent context add wiki:hygiene
  k2 agent context add subagents:pack
  k2 agent context add browser:pack
  k2 agent context add manager:pack
  k2 agent context add connections:roster
  k2 agent context add skills:roster
  k2 agent context add users:roster
  k2 agent context add docs/architecture.md --label arch
  k2 agent context off wiki-index
  k2 agent context off pinned:tooling
  k2 agent context on agent
  k2 agent context move 0 top
  k2 agent context show --outline
  k2 agent context regen
  k2 agent context regen --workspace julie
  k2 agent context list --workspace /path/to/peer
EOF
}

# Map daemon error.code → agent-family exit status.
_context_exit_for_code() {
    case "${1:-}" in
        bad_usage|path_escape|catalog_unknown|usage|invalid_move|invalid_position)
            return 2 ;;
        duplicate_layer|refused|precondition|agents_create_connections_disabled)
            return 3 ;;
        not_found)
            return 4 ;;
        *)
            return 1 ;;
    esac
}

# Absolute workspace path for context routes (matches $PROJECT resolution).
_context_project_path() {
    python3 -c 'import os; print(os.path.abspath(os.path.expanduser(os.environ.get("K2_PROJECT_PATH") or os.environ.get("K2SO_PROJECT_PATH") or os.getcwd())))' 2>/dev/null \
        || echo "${PROJECT:-$PWD}"
}

# Soft size / other soft warnings from daemon JSON → stderr (non-fatal).
_context_print_soft_warn() {
    local raw="$1"
    RAW="$raw" python3 - <<'PYEOF'
import json, os, sys
raw = os.environ.get("RAW", "")
try:
    d = json.loads(raw) if raw else {}
except Exception:
    sys.exit(0)
warn = d.get("softWarn") or d.get("soft_warn") or d.get("warn")
if not warn:
    sys.exit(0)
if isinstance(warn, list):
    for w in warn:
        sys.stderr.write("warning: %s\n" % w)
elif isinstance(warn, dict):
    sys.stderr.write("warning: %s\n" % (warn.get("hint") or warn.get("message") or json.dumps(warn)))
else:
    sys.stderr.write("warning: %s\n" % warn)
PYEOF
}

# Shared HTTP + error handling for context routes.
# Env: ABASE ATOKEN APROJECT METHOD ROUTE (path only) QS (extra query, optional)
#      BODY (JSON string for POST; empty for GET)
# Prints response body on stdout; maps errors to exit codes.
_context_http() {
    ABASE="$BASE_URL" ATOKEN="$TOKEN" APROJECT="$1" METHOD="$2" ROUTE="$3" \
    QS="${4:-}" BODY="${5:-}" python3 - <<'PYEOF'
import json, os, sys
import urllib.error, urllib.parse, urllib.request

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
project = os.environ["APROJECT"]
method = os.environ["METHOD"].upper()
route = os.environ["ROUTE"]
qs_extra = os.environ.get("QS", "")
body = os.environ.get("BODY", "")

def fail(code, hint, exit_code=1):
    sys.stderr.write(json.dumps({"error": {"code": code, "hint": hint}}) + "\n")
    sys.exit(exit_code)

def exit_for(code):
    c = (code or "").lower()
    if c in ("bad_usage", "path_escape", "catalog_unknown", "usage",
             "invalid_move", "invalid_position"):
        return 2
    if c in ("duplicate_layer", "refused", "precondition"):
        return 3
    if c == "not_found":
        return 4
    return 1

params = {"token": token, "project": project}
if qs_extra:
    for part in qs_extra.split("&"):
        if not part:
            continue
        if "=" in part:
            k, v = part.split("=", 1)
            params[k] = urllib.parse.unquote(v)
        else:
            params[part] = ""

url = base + route + "?" + urllib.parse.urlencode(params)
try:
    if method == "GET":
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=30) as r:
            raw = r.read().decode("utf-8", "replace")
            status = r.status
    else:
        data = (body or "{}").encode("utf-8")
        req = urllib.request.Request(
            url, data=data,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=60) as r:
            raw = r.read().decode("utf-8", "replace")
            status = r.status
except urllib.error.HTTPError as e:
    status = e.code
    try:
        raw = e.read().decode("utf-8", "replace")
    except Exception:
        raw = ""
    if not raw:
        fail("daemon_error", "HTTP %s on %s" % (status, route), 1)
except Exception as e:
    fail("daemon_unreachable", "no response from the K2 daemon (is it running?): %s" % e, 1)

if not raw.strip():
    fail("daemon_error", "empty response from %s (HTTP %s)" % (route, status), 1)

try:
    d = json.loads(raw)
except Exception:
    fail("bad_response", "daemon returned unparseable response from %s: %r" % (route, raw[:200]), 1)

err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code") or "error"
    hint = err.get("hint") or err.get("message") or code
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    sys.exit(exit_for(code))

# Some routes use ok:false without error object
if d.get("ok") is False and not d.get("layers") and "pinned" not in d and "catalog" not in d and "content" not in d and "outline" not in d and "text" not in d:
    fail("daemon_error", "unexpected response: %s" % json.dumps(d)[:300], 1)

# 404 with HTML / empty structured body
if status == 404 and "ok" not in d and "layers" not in d and "catalog" not in d:
    fail("not_found", "route %s not found (is the context-stack daemon build running?)" % route, 4)

print(raw)
PYEOF
}

# Resolve id|path|label token against GET /cli/context/layers → layer id.
# System layers (pinned:agent|project|tooling and short aliases) pass through
# without needing a DB row — set-enabled accepts them on the daemon.
# Prints id on stdout; uses _agent_err + exit on failure.
_context_resolve_layer_id() {
    local token="$1"
    local project="$2"
    # Fast path: system / pinned toggles (no HTTP needed for id shape).
    case "$token" in
        pinned:agent|agent) echo "pinned:agent"; return 0 ;;
        pinned:project|project) echo "pinned:project"; return 0 ;;
        pinned:tooling|tooling) echo "pinned:tooling"; return 0 ;;
    esac
    local layers_raw rc=0
    layers_raw=$(_context_http "$project" GET "/cli/context/layers") || rc=$?
    [ "$rc" -ne 0 ] && return "$rc"
    TOKEN_Q="$token" LAYERS_RAW="$layers_raw" python3 - <<'PYEOF'
import json, os, sys

token = os.environ["TOKEN_Q"]
raw = os.environ["LAYERS_RAW"]
d = json.loads(raw)
layers = d.get("layers") or d.get("optional") or []
pinned = d.get("pinned") or []

def norm(p):
    return (p or "").replace("\\", "/").strip().lstrip("./")

# 0) system / pinned ids returned from the list payload
sys_map = {
    "agent": "pinned:agent",
    "pinned:agent": "pinned:agent",
    "project": "pinned:project",
    "pinned:project": "pinned:project",
    "tooling": "pinned:tooling",
    "pinned:tooling": "pinned:tooling",
}
if token in sys_map:
    print(sys_map[token]); sys.exit(0)
for p in pinned:
    pid = str(p.get("id") or "")
    if pid == token or pid == sys_map.get(token, ""):
        print(pid or token); sys.exit(0)
    lab = (p.get("label") or "").strip().lower()
    if lab and lab == token.lower():
        print(pid or token); sys.exit(0)

nt = norm(token)
# 1) exact id
for L in layers:
    if str(L.get("id") or "") == token:
        print(L["id"]); sys.exit(0)
# 2) exact / suffix path
for L in layers:
    p = norm(L.get("path"))
    if p == nt or p.endswith("/" + nt) or (L.get("path") or "") == token:
        print(L["id"]); sys.exit(0)
# 3) label (case-insensitive)
for L in layers:
    lab = (L.get("label") or "").strip()
    if lab and lab.lower() == token.lower():
        print(L["id"]); sys.exit(0)
# 4) source suffix e.g. wiki-index from preset:wiki-index
for L in layers:
    src = (L.get("source") or "")
    if src == token or src.endswith(":" + token) or src.replace("preset:", "") == token:
        print(L["id"]); sys.exit(0)

sys.stderr.write(json.dumps({
    "error": {
        "code": "not_found",
        "hint": "no context layer matches '%s' — see: k2 agent context list" % token,
    }
}) + "\n")
sys.exit(4)
PYEOF
}

# Human list formatter for pinned (with on/off) + optional layers.
_context_format_list() {
    RAW="$1" python3 - <<'PYEOF'
import json, os, sys

d = json.loads(os.environ["RAW"])
pinned = d.get("pinned") or []
layers = d.get("layers") or d.get("optional") or []

def on_off(en):
    return "on " if en in (True, 1, "1", "true", "on", None) else "off"

print("PINNED  (system — on|off via: k2 agent context on|off pinned:agent|project|tooling)")
if not pinned:
    # Sensible defaults when daemon omits explicit pinned rows
    print("  on   Role (persona)     .k2/agent/ROLE.md")
    print("  on   Project   .k2/PROJECT.md")
    print("  on   Tooling   (generated k2-cli pointer)")
else:
    for p in pinned:
        label = (p.get("label") or p.get("name") or p.get("key") or p.get("id") or "layer").strip()
        # Shorten "Agent (persona)" / "Role" → first word for alignment
        short = label.split()[0] if label else "layer"
        path = p.get("path") or p.get("detail") or ""
        if not path and (p.get("generated") or p.get("kind") == "tooling" or "tooling" in label.lower()):
            path = "(generated k2-cli pointer)"
        state = on_off(p.get("enabled", True))
        print("  %s  %-9s %s" % (state, short[:9], path))

print("")
print("OPTIONAL  (order = AGENTS.md)")
if not layers:
    print("  (none — add with: k2 agent context add <path|catalog-id>)")
else:
    # Sort by position if present
    def pos_key(L):
        try:
            return int(L.get("position", 0))
        except (TypeError, ValueError):
            return 0
    for L in sorted(layers, key=pos_key):
        pos = L.get("position", "?")
        state = on_off(L.get("enabled", True))
        label = (L.get("label") or "").strip()
        if not label:
            src = (L.get("source") or "")
            if src.startswith("catalog:"):
                label = src.split(":", 1)[1]
            else:
                path = (L.get("path") or "")
                label = path.rsplit("/", 1)[-1].rsplit(".", 1)[0] if path else (L.get("id") or "?")[:12]
        path = L.get("path") or ""
        missing = ""
        exists = L.get("exists")
        if exists is False or exists == 0 or exists == "false":
            missing = "  MISSING"
        print("  %s  %s  %-12s %s%s" % (pos, state, label[:12], path, missing))
PYEOF
}

_context_format_catalog() {
    RAW="$1" python3 - <<'PYEOF'
import json, os, sys
d = json.loads(os.environ["RAW"])
items = d.get("catalog") or d.get("items") or []
if isinstance(d, list):
    items = d
if not items:
    print("(no context catalog items registered)")
    sys.exit(0)
# Prefer daemon kind when present; otherwise leave blank.
print("%-18s  %-6s  %-36s  %s" % ("ID", "KIND", "PATH", "LABEL"))
for p in items:
    if not isinstance(p, dict):
        print(p)
        continue
    pid = p.get("id") or "?"
    path = p.get("path") or p.get("defaultPath") or ""
    label = p.get("label") or p.get("name") or ""
    kind = (p.get("kind") or "").strip()
    if not kind:
        # Infer for older daemons without kind on the wire.
        src = (p.get("source") or "")
        if "roster" in pid or src.endswith("-roster") or "connections-roster" in src:
            kind = "live"
        elif pid.startswith("wiki:index") or pid.startswith("wiki:home") or "/wiki/" in path:
            kind = "path"
        elif "hygiene" in pid or "pack" in pid or "catalog/" in path:
            kind = "static"
        else:
            kind = "?"
    print("%-18s  %-6s  %-36s  %s" % (pid, kind, path, label))
    desc = (p.get("description") or "").strip()
    if desc:
        # Soft-wrap one indent line for Browse-friendly reading in terminal.
        print("    %s" % desc)
PYEOF
}

_context_format_show() {
    local outline_flag="${2:-0}"
    RAW="$1" OUTLINE="$outline_flag" python3 - <<'PYEOF'
import json, os, sys
d = json.loads(os.environ["RAW"])
outline = os.environ.get("OUTLINE") == "1"
# Prefer explicit fields from daemon
if outline:
    items = d.get("outline") or d.get("sections") or d.get("headings")
    if items:
        for it in items:
            if isinstance(it, dict):
                level = it.get("level") or it.get("depth") or 1
                title = it.get("title") or it.get("heading") or it.get("text") or ""
                print("%s%s" % ("  " * max(0, int(level) - 1), title))
            else:
                print(it)
        sys.exit(0)
text = d.get("content") or d.get("text") or d.get("body") or d.get("preview") or ""
if text:
    print(text, end="" if text.endswith("\n") else "\n")
    sys.exit(0)
# Fallback: pretty-print remaining JSON (minus ok)
out = {k: v for k, v in d.items() if k != "ok"}
print(json.dumps(out, indent=2))
PYEOF
}

cmd_agent_context() {
    if _wants_help "$@" || [ $# -eq 0 ]; then
        cmd_help_agent_context
        exit 0
    fi
    # Global --workspace/--project so every subcommand can target a peer
    # (Scout: k2 agent context has no workspace flag).
    local ctx_args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace|--project)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "$1 requires a workspace path, name, or uuid"; exit 2; }
                _wiki_apply_project "$2" || { _agent_err "not_found" "unknown workspace '$2'"; exit 4; }
                export K2_PROJECT_PATH="$PROJECT"
                shift 2 ;;
            --workspace=*|--project=*)
                _wiki_apply_project "${1#*=}" || { _agent_err "not_found" "unknown workspace '${1#*=}'"; exit 4; }
                export K2_PROJECT_PATH="$PROJECT"
                shift ;;
            *) ctx_args+=("$1"); shift ;;
        esac
    done
    set -- "${ctx_args[@]}"
    if [ $# -eq 0 ]; then
        cmd_help_agent_context
        exit 0
    fi
    local sub="$1"
    shift
    case "$sub" in
        list)     cmd_agent_context_list "$@" ;;
        add)      cmd_agent_context_add "$@" ;;
        remove|rm|delete)
                  cmd_agent_context_remove "$@" ;;
        on)       cmd_agent_context_set_enabled true "$@" ;;
        off)      cmd_agent_context_set_enabled false "$@" ;;
        move)     cmd_agent_context_move "$@" ;;
        show)     cmd_agent_context_show "$@" ;;
        regen)    cmd_agent_context_regen "$@" ;;
        catalog)  cmd_agent_context_catalog "$@" ;;
        --help|-h)
            cmd_help_agent_context
            exit 0
            ;;
        *)
            _agent_err "bad_usage" "unknown context subcommand '$sub' — see: k2 agent context --help"
            exit 2
            ;;
    esac
}

cmd_agent_context_list() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context list [--json]"
                exit 2 ;;
            *)
                _agent_err "bad_usage" "unexpected argument '$1' — usage: k2 agent context list [--json]"
                exit 2 ;;
        esac
    done
    local project raw rc=0
    project=$(_context_project_path)
    raw=$(_context_http "$project" GET "/cli/context/layers") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    _context_format_list "$raw"
}

cmd_agent_context_add() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local target="" label="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --label)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--label requires a value"; exit 2; }
                label="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context add <path|catalog-id> [--label name] [--json]"
                exit 2 ;;
            *)
                if [ -z "$target" ]; then target="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent context add <path|catalog-id> [--label name] [--json]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$target" ]; then
        _agent_err "bad_usage" "missing <path|catalog-id> — usage: k2 agent context add <path|catalog-id> [--label name] [--json]"
        exit 2
    fi
    local project body raw rc=0
    project=$(_context_project_path)
    # Catalog-id heuristic: paths with "/" or ending in .md are always paths;
    # otherwise treat the token as a catalog id (wiki:index, manager:pack, …).
    body=$(TARGET="$target" LABEL="$label" PROJECT="$project" python3 - <<'PYEOF'
import json, os
target = os.environ["TARGET"]
label = os.environ.get("LABEL") or ""
project = os.environ["PROJECT"]
body = {"project": project}
# path if looks like a filesystem path
is_path = (
    "/" in target
    or target.startswith(".")
    or target.endswith(".md")
    or target.endswith(".markdown")
    or target.startswith("~")
)
if is_path:
    body["path"] = target
else:
    # catalog id (wiki:index, manager:pack, …)
    body["catalog"] = target
if label:
    body["label"] = label
print(json.dumps(body))
PYEOF
)
    raw=$(_context_http "$project" POST "/cli/context/add" "" "$body") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    RAW="$raw" python3 - <<'PYEOF'
import json, os
d = json.loads(os.environ["RAW"])
layer = d.get("layer") or d.get("added") or {}
path = layer.get("path") or d.get("path") or ""
lab = layer.get("label") or ""
lid = layer.get("id") or d.get("id") or ""
bits = ["added"]
if lab:
    bits.append(lab)
if path:
    bits.append(path)
if lid and lid not in bits:
    bits.append("(id %s)" % lid)
print(" ".join(bits))
if d.get("regenerated") is False:
    print("(AGENTS.md not regenerated — run: k2 agent context regen)")
elif d.get("regen") or d.get("regenerated") or d.get("agentsMd"):
    print("AGENTS.md regenerated.")
PYEOF
}

cmd_agent_context_remove() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local token="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context remove <id|path> [--json]"
                exit 2 ;;
            *)
                if [ -z "$token" ]; then token="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$token" ]; then
        _agent_err "bad_usage" "missing <id|path> — usage: k2 agent context remove <id|path> [--json]"
        exit 2
    fi
    local project lid body raw rc=0
    project=$(_context_project_path)
    lid=$(_context_resolve_layer_id "$token" "$project") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    body=$(python3 -c 'import json,sys; print(json.dumps({"project":sys.argv[1],"id":sys.argv[2]}))' "$project" "$lid")
    raw=$(_context_http "$project" POST "/cli/context/remove" "" "$body") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    echo "removed layer $lid"
}

cmd_agent_context_set_enabled() {
    local enabled="$1"
    shift
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local token="" as_json=false
    local verb="on"
    [ "$enabled" = "false" ] && verb="off"
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context $verb <id|path> [--json]"
                exit 2 ;;
            *)
                if [ -z "$token" ]; then token="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$token" ]; then
        _agent_err "bad_usage" "missing <id|path> — usage: k2 agent context $verb <id|path> [--json]"
        exit 2
    fi
    local project lid body raw rc=0
    project=$(_context_project_path)
    lid=$(_context_resolve_layer_id "$token" "$project") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    body=$(ENABLED="$enabled" PROJECT="$project" LID="$lid" python3 - <<'PYEOF'
import json, os
print(json.dumps({
    "project": os.environ["PROJECT"],
    "id": os.environ["LID"],
    "enabled": os.environ["ENABLED"] == "true",
}))
PYEOF
)
    raw=$(_context_http "$project" POST "/cli/context/set-enabled" "" "$body") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    if [ "$enabled" = "true" ]; then
        echo "enabled layer $lid"
    else
        echo "disabled layer $lid"
    fi
}

cmd_agent_context_move() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local token="" where="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context move <id|path> <n|up|down|top|bottom> [--json]"
                exit 2 ;;
            *)
                if [ -z "$token" ]; then token="$1"
                elif [ -z "$where" ]; then where="$1"
                else
                    _agent_err "bad_usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$token" ] || [ -z "$where" ]; then
        _agent_err "bad_usage" "usage: k2 agent context move <id|path> <n|up|down|top|bottom> [--json]"
        exit 2
    fi
    if ! printf '%s' "$where" | grep -Eq '^(up|down|top|bottom|[0-9]+)$'; then
        _agent_err "bad_usage" "invalid move target '$where' — use n (0-based), up, down, top, or bottom"
        exit 2
    fi
    local project lid body raw rc=0
    project=$(_context_project_path)
    lid=$(_context_resolve_layer_id "$token" "$project") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    body=$(WHERE="$where" PROJECT="$project" LID="$lid" python3 - <<'PYEOF'
import json, os
where = os.environ["WHERE"]
body = {"project": os.environ["PROJECT"], "id": os.environ["LID"]}
if where.isdigit():
    body["position"] = int(where)
else:
    body["direction"] = where
print(json.dumps(body))
PYEOF
)
    raw=$(_context_http "$project" POST "/cli/context/move" "" "$body") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    echo "moved layer $lid → $where"
}

cmd_agent_context_show() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local as_json=false outline=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --outline) outline=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context show [--outline] [--json]"
                exit 2 ;;
            *)
                _agent_err "bad_usage" "unexpected argument '$1' — usage: k2 agent context show [--outline] [--json]"
                exit 2 ;;
        esac
    done
    local project qs="" raw rc=0
    project=$(_context_project_path)
    if $outline; then qs="outline=1"; fi
    raw=$(_context_http "$project" GET "/cli/context/show" "$qs") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    if $outline; then
        _context_format_show "$raw" 1
    else
        _context_format_show "$raw" 0
    fi
}

cmd_agent_context_regen() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context regen [--json]"
                exit 2 ;;
            *)
                _agent_err "bad_usage" "unexpected argument '$1' — usage: k2 agent context regen [--json]"
                exit 2 ;;
        esac
    done
    local project body raw rc=0
    project=$(_context_project_path)
    body=$(python3 -c 'import json,sys; print(json.dumps({"project":sys.argv[1]}))' "$project")
    raw=$(_context_http "$project" POST "/cli/context/regen" "" "$body") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    _context_print_soft_warn "$raw"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    RAW="$raw" python3 - <<'PYEOF'
import json, os
d = json.loads(os.environ["RAW"])
path = d.get("path") or d.get("agentsMd") or d.get("agents_md") or ".k2/AGENTS.md"
bytes_ = d.get("bytes") or d.get("size")
if bytes_ is not None:
    print("regenerated %s (%s bytes)" % (path, bytes_))
else:
    print("regenerated %s" % path)
PYEOF
}

cmd_agent_context_catalog() {
    if _wants_help "$@"; then cmd_help_agent_context; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent context catalog [--json]"
                exit 2 ;;
            *)
                _agent_err "bad_usage" "unexpected argument '$1' — usage: k2 agent context catalog [--json]"
                exit 2 ;;
        esac
    done
    local project raw rc=0
    project=$(_context_project_path)
    raw=$(_context_http "$project" GET "/cli/context/catalog") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if $as_json; then
        printf '%s\n' "$raw"
        return 0
    fi
    _context_format_catalog "$raw"
}

cmd_help_skills() {
    cat <<'EOF'
k2 skills <subcommand> [options]

Manage documentation profiles (skills) for the workspace. Skills are
markdown files (SKILL.md) describing roles, personas, and instructions
your harness loads when spawning sub-agents.

  list                              Skill profiles in this workspace
  create <name> [--template <src>]  Create a new skill profile
                                    (optionally seeded from another skill)
  remove <name>                     Delete a skill profile (sent to Trash)
  profile <name>                    Read the skill's SKILL.md
  regenerate                        Refresh every skill's SKILL.md

SKILLS vs AGENT
  An agent IS a workspace (1:1). A skill is a documentation profile
  the agent (or your harness) can reference when applying a specific
  role to work.

  Spawning sessions pre-loaded with a skill is your harness's job
  (Claude Code's sub-agent spawning, Cursor's worktree management).
  K2 no longer provides a `delegate` verb — see `k2 help-deprecated`.

  Skills live at `.k2/skills/<name>/SKILL.md` (consolidated home as of
  Phase 2.5b). The legacy `.k2so/agents/` and `.k2so/agent-templates/`
  folders are migrated into here on first daemon boot per upgraded
  workspace. Originals go to the macOS Recycle Bin (recoverable).

TEMPLATING (post-Phase-2.5b)
  Any existing skill can seed a new one — there is no separate
  "template" namespace. `--template <name>` means "copy this skill as
  the starting point for the new skill." Edit the new file freely from
  there; the seed is unaffected.

EXAMPLES
  k2 skills list
  k2 skills create backend-eng --template rust-eng
  k2 skills profile backend-eng
  k2 skills regenerate
EOF
}

cmd_help_settings() {
    cat <<'EOF'
k2 settings [--mode <off|custom|manager|k2>]
k2 settings set <key> <true|false>

Show this workspace's settings (name, mode, heartbeat, pinned). With
--mode, set the workspace mode. With set, turn one of this server's
preview switches on or off.

WORKSPACE MODE
  --mode <off|custom|manager|k2>
                            off: K2 ignores this workspace
                            custom: the workspace has its own agent
                            manager: the workspace coordinates other workspaces
                            k2: K2's own agent (k2so is accepted too)

SERVER SWITCHES (the owner only)
  k2 settings set <key> <true|false>      on and off work too; nothing else
    voiceRoomsPreview       voice rooms on this server (k2 voice, the
                            top-bar headset, Settings → Voice Rooms)
    voiceTunnelPreview      voice through K2's tunnel
                            (k2 hostchat media tier tunnel)
    computePreview          compute nodes (same as k2 compute preview on|off)
    ciPreview               CI on K2 Compute: k2 ci, node rosters, lanes,
                            seats, owners consent, signer pairing, lane
                            secrets, --receipt-v2 (needs computePreview on
                            too)
    boardFilePreview        board files open in a tab
    videoRoomsPreview       video in voice rooms: camera and screen share
    usageSingleRecord       one usage percent per token (preview; off = today's)
                            in the K2 desktop app's call panel (needs
                            voiceRoomsPreview on too)
  All of them are off until the owner turns them on.
  Run it as the owner in a plain terminal on the server (not a K2 tab): the
  CLI picks up the owner's credential there by itself. These are the only
  keys it takes: any other key, or a value that isn't true/false/on/off,
  exits 2 with this list and sends nothing. Agents, Admins and Members get
  exit 3: the daemon checks that the caller is the owner. An agent asks
  its human (k2 thread). The change applies at once, with no restart.

EXIT
  0 ok · 1 no usable answer · 2 bad usage, an unknown key or a bad value
  (checked first, before anything is sent) · 3 the daemon refused you
  (set: only the owner; --mode: the owner or an admin)

EXAMPLES
  k2 settings
  k2 settings --mode manager
  k2 settings set voiceRoomsPreview true
  k2 settings set voiceTunnelPreview off
EOF
}

cmd_help_heartbeat() {
    cat <<'EOF'
k2 heartbeat [<family> <subcommand>]

Manage workspace-scoped scheduled wakes.
Some people call these routines, scheduled tasks or cron jobs: if you were
looking for those, heartbeats are it. For a wake that happens once (a
reminder), use `k2 heartbeat remind` below.

DEFAULT (no args)
  k2 heartbeat                                Lists active heartbeat schedules
                                                (= `heartbeat schedule list`)

THREE FAMILIES:

SCHEDULE (CRUD on heartbeat definitions)
  heartbeat schedule add --name <n> <spec> [--instructions "..."]
       <spec> is one of:
         --daily --time HH:MM
         --weekly --days mon,wed,fri --time HH:MM
         --monthly --days 1,15 --time HH:MM
         --yearly --months jan,jul --time HH:MM
         --hourly --start 09:00 --end 17:00 --every 30 --unit minutes
       --instructions "text" | --instructions-file <path>
         write the WAKEUP.md (the job that runs on fire) at create time;
         the daemon writes it on its own machine. Without it the
         heartbeat stays enabled and waits ("needs instructions"), then
         fires at the next scheduled slot after a body is added
       New schedules land in the PINNED chat. For a trained flow prefer
       a sidecar: k2 heartbeat session <n> --set sales/reviewer
       (UUID form still works: --set <session-id> --provider <p>)
  heartbeat schedule list [--archived] [--json]  (--json: raw array)
  heartbeat schedule list --workspace <ws>       Another workspace's (people only)
  heartbeat schedule remove <n> [--purge]
       Default archives (restorable). --purge hard-deletes and is
       owner-only: agents archive instead.
  heartbeat schedule unarchive <n>
  heartbeat schedule edit <n> [<new-spec>] [--instructions "..."]
       spec flags change the schedule; --instructions/--instructions-file
       have the daemon rewrite the WAKEUP.md (either or both)
  heartbeat schedule rename <old> <new>
  heartbeat schedule enable <n> [--workspace <ws>]   Resume scheduling
  heartbeat schedule disable <n> [--workspace <ws>]  Pause scheduling
       (also: heartbeat enable|disable <n> [--workspace <ws>])
       --workspace <ws>: a heartbeat of ANOTHER workspace (handle, name or
       path; <ws>/<n> means the same). Without it: your own workspace.
       Who may pause or resume another workspace's heartbeat: the server's
       owner, an Admin login, or a person typing in a K2 shell tab. An agent
       (an LLM chat in K2, like you) may not. K2 checks the names first
       (exit 4: no such workspace; exit 1: no such heartbeat there: ask
       your person for the right name), then refuses an agent with exit 3
       (other_workspace). With permission cards on, that error ends with an
       "Ask with:" line; run it as printed, filling in only --why:
         k2 permission request heartbeat.pause-other target=<ws> heartbeat=<n> --why "…"
       (key=value pairs in any order; heartbeat.resume-other to resume).
       Your person gets a card on your Thread; when they tap Allow, K2
       pauses it as them and you get a message saying so (or that they
       declined). Don't run disable again: it stays paused until someone
       resumes it. No "Ask with:" line (cards off): ask your person to run
       the same command themselves.

REMIND (a one-time heartbeat: fires once, then goes away)
  heartbeat remind "<text>" (--in 45m | --at 13:00 | --on 2026-10-30 [--at 09:00]) [--tz Area/City]
       Short form: k2 remind. Also: heartbeat remind list [--all] | show <n> |
       snooze <n> <when> | cancel <n>. Full details: k2 heartbeat remind --help

SIGNAL (immediate one-shot triggers)
  heartbeat signal fire <n> [--force]
                                 Fire one heartbeat now (skips schedule
                                 window; --force: fire even if disabled)
  heartbeat signal wakeup <n>    Print/edit the WAKEUP.md
  heartbeat signal wake          Auto-wake (no name needed)

DELIVERY (where a fire's wake message lands)
  heartbeat session <n>          Show the current delivery target
  heartbeat session <n> --pinned Deliver into the workspace pinned chat
  heartbeat session <n> --auto   Own session (a new one on the next fire)
  heartbeat session <n> --set <session-id|workspace/handle> [--provider <p>]
                                 Deliver into a specific saved session
                                 (handle: sales/reviewer or 1; UUID form
                                 still wants --provider claude|grok|…)

RESTART (the heartbeat's own session)
  heartbeat restart <n> [--workspace <ws>] [--json]
       Stops the heartbeat's own session and forgets its saved
       conversation. The schedule and WAKEUP.md stay as they are. Nothing
       fires now: the next fire starts a new session (to fire at once:
       heartbeat signal fire <n>). Use it when the session is stuck: out
       of its usage limit, signed out, or on an old LLM token.
       The new session uses the token a new chat here would use: the
       workspace's pinned token, else the server default. If that token is
       out too, the fire fails again (heartbeat status <n> shows it): ask
       your person to pick another token, or wait for the limit to reset.
       <n> is the heartbeat's name (hb:<n> works too). Run from inside that
       heartbeat's own session, it stops you. It never stops the pinned
       chat or a sidecar. A heartbeat set to a chosen session (heartbeat
       session <n> --set) keeps that choice and restart stops only the
       heartbeat's own terminal; to let K2 start its own session instead:
       heartbeat session <n> --auto.
       An agent may restart its own workspace's heartbeats; no switch is
       needed (k2 sessions stop refuses hb: runs). --workspace <ws>
       (another workspace's) is for people only.
       Fires also restart a stuck session on their own when the CLI's last
       answer was a limit or sign-in error, or the token changed:
       heartbeat status <n> then shows "restarted: weekly limit",
       "restarted: usage limit", "restarted: signed out" or "restarted:
       token changed". If the new session fails too, the fire is recorded
       as an error. A chosen session is never replaced: when it is stuck,
       the fire is recorded as an error that says so.
       Exit: 0 done · 1 error (no such heartbeat) · 2 bad usage or not a
       heartbeat name · 3 refused (you named another workspace) · 4
       workspace not found

INSPECTION (read-only)
  heartbeat show <n> [--workspace <ws>] [--json]  Single heartbeat details + instructions
  heartbeat status <n> [-n N]    Recent history: fires and changes, with
                                 who did it (owner, user, agent, app)
  heartbeat log [-n N]           Scheduler decisions

EXAMPLES
  k2 heartbeat schedule add --name deploy-check --daily --time 09:00 \
      --instructions "Check the deploy dashboard and report anomalies to the workspace chat."
  k2 heartbeat schedule add --name roundup --hourly --every 1 --unit hours
  k2 heartbeat schedule list
  k2 heartbeat schedule disable deploy-check
  k2 heartbeat schedule disable triage --workspace companion
  k2 heartbeat signal fire deploy-check
  k2 heartbeat log

EXIT CODES (enable/disable)
  0 done · 1 error (no such heartbeat: the error names the workspace) ·
  2 bad usage · 3 refused · 4 workspace not found
EOF
}

cmd_help_deprecated() {
    cat <<'EOF'
K2 retired verbs → new equivalents

HARD-DEPRECATED (verb removed; commands exit non-zero with pointer)

  state list|get|set              → (removed — Workspace States retired)
  app-update                      → update --app
  status "msg"                    → organization update "<news>"
  checkin                         → organization bulletin|update, thread, tickets, inbox
  checkin --done                  → done [--blocked "reason"]

  # 0.40.31 — legacy per-agent adaptive heartbeat control removed
  # (heartbeat.json; nothing consumed it since 0.39.0d). Named
  # workspace heartbeat schedules are the only heartbeat surface.

  heartbeat set --agent <n> ...   → heartbeat schedule add/edit --name <n> <spec>
  heartbeat get --agent <n>       → heartbeat show <name> [--json]
  heartbeat force --agent <n>     → heartbeat signal fire <name>
  heartbeat noop|action           → (removed — adaptive backoff is gone;
                                       the user owns wake cadence via schedules)

  --agent <name> flag             → --workspace <path>  (everywhere)
                                       Hard-removed; pass workspace explicitly.

RETIRED (the CLI no longer runs these; each answers with its pointer and
exits 1; /cli/agents/work and /cli/work/inbox answer 410 and point here)

EOF
    _retired_verbs_map
    cat <<'EOF'

EXAMPLES OF MIGRATION

  # Sending a message
  Old: k2 msg --agent scout_v3 "hello"
  New: k2 msg scout_v3 "hello"            # workspace-implicit; --agent removed

  # Marking a task done
  Old: k2 checkin --done --blocked "Stripe API access"
  New: k2 done --blocked "Stripe API access"
EOF
}

cmd_help_internal() {
    cat <<'EOF'
K2 CLI — Internal surface (rare; used by agent runtimes)

These verbs exist for orchestrators and tools that integrate with K2.
Humans rarely run them directly.

TERMINAL (raw PTY I/O)
  terminal spawn --command "..."                Spawn a sub-terminal
  terminal write <id> "text"                    Paste + Enter to PTY by id
  terminal read <id> [--lines N]                Last N lines from buffer

SESSIONS (low-level session lifecycle)
  sessions list [--json]                        Raw session map
  sessions live                                 Subscribe to session events
  sessions compact                              Compact archive ring

AGENT (single-item ops on the workspace's primary agent)
  agent profile [<path>]                        Equivalent to `workspace profile`
  agent update --field <f> --value <v>          Equivalent to `workspace update`
  agent complete --file <f>                     Mark work item complete

HOOKS (Claude Code / Cursor / Gemini integration state)
  hooks status [--limit N] [--json]             Hook pipeline state
                                                (same as `daemon hooks status`)
  hooks install [--json]                        Re-run the daemon's hook installer
  hooks uninstall [--json]                      Remove K2's hook entries (others kept)
  hooks emit <event> [--json '<obj>']           A custom program reports its lifecycle
                                                (session passport only; k2 study hooks)

For "what's running" use `workspace list --running` (daily tier).
For workspace-scoped ops use the `workspace` verb (daily tier).
EOF
}

# Phase 2.1 — workspace launch (smart cascade placeholder).
# Calls existing /cli/workspace/ensure-canonical-session which provides
# the attach|wake|spawn semantics the mock describes.
cmd_workspace_launch() {
    local target="${1:-$PROJECT}"
    local json="false" no_attach="false"
    shift 2>/dev/null || true
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) json="true"; shift ;;
            --no-attach) no_attach="true"; shift ;;
            *) shift ;;
        esac
    done
    if [[ ! -d "$target" ]]; then
        # Possibly a workspace name; resolve via daemon.
        local resolved
        resolved=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
            | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("path",""))
except: pass' 2>/dev/null)
        [[ -n "$resolved" ]] && target="$resolved"
    fi
    cli_request_post "/cli/workspace/ensure-canonical-session" "project=$(urlencode "$target")"
}

cmd_workspace_profile() {
    local target="${1:-$PROJECT}"
    cli_request "/cli/workspace/agent-display-name" "project=$(urlencode "$target")"
}

cmd_workspace_update() {
    local field="" value=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --field) field="$2"; shift 2 ;;
            --value) value="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [[ -z "$field" || -z "$value" ]]; then
        echo "Usage: k2 workspace update --field <f> --value <v>" >&2
        exit 1
    fi
    # 0.40.24 (B1): `update --field` edits the AGENT.md profile — it has
    # NEVER touched the projects DB. Passing a DB column name here used to
    # silently append a useless `## agent_mode` section to AGENT.md while
    # the real setting stayed unchanged. Fail loudly and point at the
    # settings plane instead.
    case "$field" in
        id|name|path|color|tab_order|last_opened_at|worktree_mode|icon_url|focus_group_id|pinned|manually_active|last_interaction_at|created_at|agent_enabled|heartbeat_enabled|agent_mode|state_id|tier_id|heartbeat_mode|heartbeat_schedule|heartbeat_last_fire|allow_remote_instruct|use_session_stream|sandbox_fs_mode|anthropic_api_key)
            echo "Error: '$field' is a workspace SETTING (a projects DB column), not an AGENT.md profile field." >&2
            echo "'k2 workspace update' only edits the agent's AGENT.md; it would not change the real setting." >&2
            echo "Use: k2 agent set <name> [--mode <m>] [--enabled <bool>] [--name <new>]  (see 'k2 agent set --help')" >&2
            exit 2
            ;;
    esac
    # Forward to agent profile update (workspace agent IS the workspace).
    # The agent name is the workspace's primary agent — daemon resolves it.
    local agent_name
    agent_name=$(cli_request "/cli/workspace/agent-display-name" 2>/dev/null \
        | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("display_name",""))
except: pass' 2>/dev/null)
    if [[ -z "$agent_name" ]]; then
        # 0.39.0f: workspace folder basename is the canonical fallback
        # for the workspace's primary agent name (matches the post-
        # unification invariant: workspace == agent). Replaces the
        # pre-unification `__lead__` sentinel that the daemon no
        # longer recognizes as a routing key.
        agent_name="$(basename "$PROJECT")"
    fi
    cli_request_post "/cli/agent/update" \
        "agent=$(urlencode "$agent_name")" \
        "field=$(urlencode "$field")" \
        "value=$(urlencode "$value")"
}

# BEGIN_WORKSPACE_RESOURCES
# Absolute path from the caller's cwd. Does not require the file to exist
# and does not resolve symlinks — the daemon does the tree check.
_workspace_resources_abs() {
    python3 -c 'import os, sys; print(os.path.abspath(sys.argv[1]))' "$1"
}

# stdin: daemon JSON. MODE=list|add|remove. WANT_JSON=1 prints the body unchanged.
_workspace_resources_emit() {
    MODE="${MODE}" WANT_JSON="${WANT_JSON:-0}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    data = json.loads(raw)
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % raw[:300])
    sys.exit(1)
if not isinstance(data, dict) or data.get("ok") is not True:
    err = data.get("error") if isinstance(data, dict) else None
    if isinstance(err, dict):
        code = err.get("code") or "error"
        hint = err.get("hint") or code
        # Daemon hints already include the code (not_a_file: …). Print the
        # hint once so ok:false is not a success line.
        sys.stderr.write("%s\n" % hint)
    else:
        sys.stderr.write("Error: %s\n" % raw[:300])
    sys.exit(1)
if os.environ.get("WANT_JSON") == "1":
    sys.stdout.write(raw)
    if not raw.endswith("\n"):
        sys.stdout.write("\n")
    sys.exit(0)
mode = os.environ.get("MODE") or ""
if mode == "list":
    docs = data.get("docs")
    if not isinstance(docs, list):
        sys.stderr.write("Error: list response missing docs\n")
        sys.exit(1)
    for row in docs:
        if not isinstance(row, dict):
            sys.stderr.write("Error: list row is not an object\n")
            sys.exit(1)
        name = row.get("fileName") if isinstance(row.get("fileName"), str) else ""
        path = row.get("filePath") if isinstance(row.get("filePath"), str) else ""
        sys.stdout.write("%s\t%s\n" % (name, path))
elif mode in ("add", "remove"):
    path = data.get("filePath") if isinstance(data.get("filePath"), str) else ""
    if not path:
        sys.stderr.write("Error: response missing filePath\n")
        sys.exit(1)
    sys.stdout.write(path + "\n")
else:
    sys.stderr.write("Error: unknown resources mode\n")
    sys.exit(1)
'
}

cmd_workspace_resources() {
    local sub="${1:-}"
    if [ $# -gt 0 ]; then
        shift
    fi
    local json=0
    local ws=""
    local file=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json=1; shift ;;
            --workspace|--project)
                if [ $# -lt 2 ] || [ -z "${2:-}" ]; then
                    echo "Usage: k2 workspace resources ${sub:-<list|add|remove>} --workspace <path>" >&2
                    exit 2
                fi
                ws="$2"
                shift 2
                ;;
            --)
                shift
                break
                ;;
            -*)
                echo "Unknown flag: $1" >&2
                echo "Usage: k2 workspace resources <list|add|remove> [--workspace <path>] [--json] [file]" >&2
                exit 2
                ;;
            *)
                if [ -n "$file" ]; then
                    echo "Usage: k2 workspace resources <list|add|remove> [--workspace <path>] [--json] [file]" >&2
                    exit 2
                fi
                file="$1"
                shift
                ;;
        esac
    done
    # One file after `--`. A second positional is usage, not a silent drop.
    if [ $# -gt 1 ] || { [ $# -eq 1 ] && [ -n "$file" ]; }; then
        echo "Usage: k2 workspace resources <list|add|remove> [--workspace <path>] [--json] [file]" >&2
        exit 2
    fi
    if [ $# -eq 1 ]; then
        file="$1"
    fi
    case "$sub" in
        list|add|remove) ;;
        "")
            echo "Usage: k2 workspace resources <list|add|remove> [--workspace <path>] [--json] [file]" >&2
            exit 2
            ;;
        *)
            echo "Unknown 'workspace resources' subcommand: $sub" >&2
            echo "Usage: k2 workspace resources <list|add|remove> [--workspace <path>] [--json] [file]" >&2
            exit 2
            ;;
    esac
    if [ -z "$ws" ]; then
        ws="$PROJECT"
    fi
    if [ -z "$ws" ]; then
        echo "workspace: missing workspace (name | path | UUID)" >&2
        exit 1
    fi
    local mode="$sub"
    local raw=""
    case "$sub" in
        list)
            if [ -n "$file" ]; then
                echo "Usage: k2 workspace resources list [--workspace <path>] [--json]" >&2
                exit 2
            fi
            raw=$(cli_send cli_request "/cli/workspace/resources" "workspace=$(urlencode "$ws")")
            ;;
        add|remove)
            if [ -z "$file" ]; then
                echo "Usage: k2 workspace resources $sub <file> [--workspace <path>] [--json]" >&2
                exit 2
            fi
            local abs
            abs="$(_workspace_resources_abs "$file")"
            local endpoint="/cli/workspace/resources/${sub}"
            raw=$(cli_send cli_post_form "$endpoint" \
                "workspace=$(urlencode "$ws")" \
                "path=$(urlencode "$abs")")
            ;;
    esac
    printf '%s' "$raw" | MODE="$mode" WANT_JSON="$json" _workspace_resources_emit
}
# END_WORKSPACE_RESOURCES

cmd_workspace_list() {
    # 0.39.45 (#26d): this used to hit /cli/workspaces/list, which is a
    # PER-PROJECT branch-workspace lookup requiring project_id — the
    # call always got `{"error": "Missing 'project_id'..."}` back, the
    # python renderer crashed iterating a dict, stderr was swallowed by
    # 2>/dev/null, and the command exited 1 with no output. "Workspace"
    # at the CLI surface means a registered project: list those.
    local running="false" as_json="false"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --running) running="true"; shift ;;
            # 0.46.6: scripts read JSON, never the table (a path with a
            # space used to split the columns). The same rows as
            # `k2 agent list --json` (workspace == agent).
            --json) as_json="true"; shift ;;
            -h|--help)
                echo "Usage: k2 workspace list [--running] [--json]"
                echo "  --json   {ok, workspaces:[{name, id, handle, path, mode, enabled, live, sessionId, backend, ptydAdopted}]}"
                exit 0 ;;
            *) shift ;;
        esac
    done
    if [ "$as_json" = "true" ]; then
        cli_request "/cli/agent/list" | RUNNING_ONLY="$running" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print(json.dumps({"error": {"code": "bad_response", "hint": "daemon returned an unparseable response: %r" % raw[:200]}}), file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict) or d.get("error") or not d.get("ok"):
    err = d.get("error") if isinstance(d, dict) else None
    if not isinstance(err, dict):
        err = {"code": "error", "hint": str(err or raw[:200])}
    print(json.dumps({"error": err}), file=sys.stderr)
    sys.exit(1)
rows = d.get("agents") or []
if os.environ.get("RUNNING_ONLY") == "true":
    rows = [r for r in rows if r.get("enabled")]
print(json.dumps({"ok": True, "workspaces": rows}))
'
        return
    fi
    cli_request "/cli/projects/list" | RUNNING_ONLY="$running" python3 -c "
import json, os, sys
raw = sys.stdin.read()
try:
    rows = json.loads(raw)
except Exception:
    sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])
    sys.exit(1)
if isinstance(rows, dict):
    sys.stdout.write('Error: %s\n' % rows.get('error', raw[:200]))
    sys.exit(1)
if os.environ.get('RUNNING_ONLY') == 'true':
    rows = [r for r in rows if r.get('agentEnabled')]
    if not rows:
        print('No agent-enabled workspaces.')
        sys.exit(0)
if not rows:
    print('No workspaces registered.')
    sys.exit(0)
for r in rows:
    name = r.get('name') or '?'
    mode = r.get('agentMode') or 'off'
    path = r.get('path', '')
    print(f'  {name:<24} [{mode:<8}] {path}')
"
}

cmd_workspace_triage() {
    if [ $# -gt 0 ]; then
        echo "Usage: k2 workspace triage" >&2
        echo "This workspace's inbox list (same as k2 inbox list). Extra arguments are not accepted." >&2
        exit 2
    fi
    # 0.39.0f Phase 2.1b: /cli/work/inbox is HTTP 410 now. Use the
    # inbox-first endpoint and the inbox formatter so the output
    # matches the rest of the inbox surface.
    cli_request "/cli/inbox/list" | format_inbox
}

