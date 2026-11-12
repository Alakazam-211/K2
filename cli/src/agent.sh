# ── 0.40.24: `k2 agent` — the agent management plane (S2) ───────────
#
# `k2 agent` is the management namespace over the workspace==agent
# identity; `k2 workspace` remains the substrate plumbing. Verbs and
# help text follow .k2/notes/agent-cli-mockup-0.40.24.md (the
# comprehension-test artifact): conf/get/set (S2), hire/list (S3),
# retire (S4).
#
# Output contract (PRD §4): data → stdout; errors → stderr as JSON
# {"error":{"code","hint"}}. Exit codes: 0 ok · 1 error · 2 bad usage ·
# 3 refused by a safety guard · 4 not found.

cmd_help_agent() {
    cat <<'EOF'
k2 agent — manage this server's agents (each agent lives in its own workspace directory)

USAGE
  k2 agent <command> [args] [flags]

COMMANDS
  hire <dir>              Provision a new agent in <dir> (create, configure, wire, launch) in one call
                          (--template <id>: from an agent type; <dir> then optional)
  template [list|show]    The agent types you can hire (general, developer, writer, …)
  conf [<name>]           Show all of an agent's settings (default: the agent in the current directory)
  get <name> <key>        Print a single setting's value (bare, script-friendly)
  set <name> [flags]      Change one or more settings (hire's settings flags)
  list                    List every agent on this server with mode + live status
  retire <name>           Safely decommission an agent (guards + archive; never deletes)
  restart <name>          Restart the agent's chat session (reloads charter + MCP config; same conversation)
  harness <name> <harness>  Move the agent's pinned chat (what heartbeats, Threads and msg reach) to
                          claude, codex, grok, gemini, cursor, pi or hermes. Changing the default
                          agent only changes NEW chats. People run it; an agent is refused (exit 3)
                          and asks its person (the refusal says how).
  refresh [<name>]        Start a new pinned chat on the same harness that carries on from this one,
                          with a note for it. The old chat stays in Chat history; the Thread comes along.
                          An agent's main session may, for its own chat, when a person allows it.
  identity [<name>]       Show or change the picture, color and name (an agent: its own color and picture)
  access set <ws> chat_manage on|off
                          "Manage Chat / Voice Hosting" for that workspace's agent (the owner or an admin)
  context <subcommand>    Manage the always-on AGENTS.md context stack (layers)
  team [served|link|unlink]  Team links: who serves this agent, who it serves (agents read their own)
  instances [show|hire|retire]  Hires made from an inline recipe (owner/admin; v1 API keys hire too)
  hire-root [set <dir>]   The folder API keys hire new agents into (owner sets it)

ADDRESSING
  <name> accepts an agent name or a path. Anything containing "/" is treated as a path.
  Names match case-insensitively; quote names containing spaces ("QA Bot").
  Unknown or ambiguous names fail with exit 4; the error says why (a near-miss name,
  several matches, or a retired handle) and names the workspace's handle and path
  to use instead. A full path always works. Commands that default to
  the current directory (conf) fail with exit 4 when it isn't a registered agent.

GLOBAL FLAGS
  --json                  Machine-readable output (single JSON object; mutations include ok/changed/actions)
  --dry-run               Print the exact action plan without changing anything (mutating commands).
                          Combines with --json: the plan prints as actions with status "pending".

EXIT CODES
  0 ok · 1 error · 2 bad usage · 3 refused by a safety guard · 4 not found

Run 'k2 agent <command> --help' for details on a command.
EOF
}

cmd_help_agent_set() {
    cat <<'EOF'
Change one or more of an agent's settings in a single call.
Uses the same SETTINGS flags as hire (--mode, --enabled, --name, --persona)
plus project membership (--add-project / --remove-project); set does not
wire connections or launch sessions. Unknown flags fail (exit 2) and list
the valid flags.

USAGE
  k2 agent set <name> [flags]

FLAGS
  --mode <mode>           off | custom | manager | k2 ("off" also sets enabled=false)
  --enabled <bool>        true | false (false pauses without changing mode)
  --name <new-name>       Rename the agent (display name; the directory does not move)
  --persona <file.md>     Replace the agent's persona with this file's contents.
                          The previous persona is backed up automatically to
                          <dir>/.k2/agent/agent-backups/.
  --add-project <p>       Add the agent to a project (name or id; repeatable).
                          Already a member = reported no-op. The FIRST member of
                          an empty project becomes its Point of Contact.
  --remove-project <p>    Remove the agent from a project (membership only;
                          repeatable). Removing the current PoC is refused
                          (poc_successor_required) until `k2 project poc`
                          names a successor.
  --dry-run               Show what would change; change nothing
  --json                  JSON output

EXAMPLES
  k2 agent set scout --mode manager
  k2 agent set scout --enabled false --persona ./scout-v2.md
  k2 agent set scout --add-project "Blue Harbor" --remove-project sandbox
EOF
}

cmd_help_agent_conf() {
    cat <<'EOF'
Show all of an agent's settings: name, path, mode, enabled, persona,
connections, project memberships (PoC marked), and live-session status.

USAGE
  k2 agent conf [<name>] [--json]

  <name> is an agent name or a path (anything containing "/" is a path).
  Omitted: the agent in the current directory (exit 4 when the current
  directory isn't a registered agent).

FLAGS
  --json                  Machine-readable output (single JSON object)
EOF
}

cmd_help_agent_get() {
    cat <<'EOF'
Print a single setting's value — bare on stdout, script-friendly.

USAGE
  k2 agent get <name> <key> [--json]

KEYS
  mode | enabled | name | path | persona | projects
  (persona prints the charter file's content; projects prints the
  agent's project names, one per line)

FLAGS
  --json                  Wrap the value in a JSON object instead
EOF
}

# Emit a contract-shaped error to stderr: {"error":{"code","hint"}}.
# Python prints to ITS stdout; bash redirects the whole thing to stderr
# (redirecting python's stderr would also swallow the message itself).
_agent_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}}))' >&2 \
        || echo "Error ($code): $hint" >&2
}

# 0.44.4 (noun-tiers A3 / VL3) — true when this CLI runs inside a K2 session
# that carries a session passport (agent cell: $K2_HOOK_SOCK + scoped token).
# The `agent` / `workspace` verbs re-source the disk owner token there
# (COMPAT-58), so the daemon cannot tell the agent from the owner; agent
# permission switches are refused here instead.
_cli_caller_is_agent_passport() {
    local hook_sock="${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}"
    [ -n "$hook_sock" ] && [ -n "${SCOPED_TOKEN:-}" ]
}

# Refuse an agent permission switch from inside an agent session (exit 3,
# the teaching refusal). $1 = the workspace setting, $2 = where a human
# changes it.
_cli_refuse_agent_self_grant() {
    local field="$1" where="$2"
    _agent_err "owner_only" "An agent can't grant itself '$field'. Ask a human (owner or admin) to change it: $where — or run this from a terminal outside a K2 agent session."
    exit 3
}

# Resolve an agent token (name | path; ""→PWD) and fetch its conf JSON
# from the daemon. Prints the conf object on stdout. Exit/return codes
# per the agent-CLI contract: 0 ok, 1 daemon/transport error, 4 not
# found (with the daemon's did-you-mean hint forwarded).
_agent_conf_fetch() {
    local token="${1:-}"
    [ -z "$token" ] && token="$PWD"
    local q="$token"
    case "$token" in
        # ADDRESSING: anything containing "/" is a path — absolutize so
        # relative paths and ~ resolve before hitting the daemon.
        */*)
            q=$(python3 -c 'import os,sys; print(os.path.abspath(os.path.expanduser(sys.argv[1])))' "$token" 2>/dev/null) || q="$token"
            ;;
    esac
    local resp
    resp=$(cli_request "/cli/agent/conf" "q=$(urlencode "$q")") || resp=""
    if [ -z "$resp" ]; then
        _agent_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        return 1
    fi
    RESP="$resp" python3 - <<'PYEOF'
import json, os, sys
raw = os.environ.get("RESP", "")
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write(json.dumps({"error": {"code": "bad_response",
        "hint": "daemon returned an unparseable response: %r" % raw[:200]}}) + "\n")
    sys.exit(1)
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    sys.exit(4 if err.get("code") in ("not_found", "ambiguous") else 1)
if not d.get("ok"):
    sys.stderr.write(json.dumps({"error": {"code": "error",
        "hint": "unexpected daemon response: %r" % raw[:200]}}) + "\n")
    sys.exit(1)
print(json.dumps(d))
PYEOF
}

cmd_agent_conf() {
    if _wants_help "$@"; then cmd_help_agent_conf; exit 0; fi
    local token="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent conf [<name>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$token" ]; then token="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent conf [<name>] [--json]"
                    exit 2
                fi
                shift ;;
        esac
    done
    local conf rc=0
    conf=$(_agent_conf_fetch "$token") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if $as_json; then
        printf '%s\n' "$conf"
        return 0
    fi
    CONF="$conf" python3 - <<'PYEOF'
import json, os, time
d = json.loads(os.environ["CONF"])

def ago(ts):
    s = int(time.time() - ts)
    if s < 60: return "just now"
    if s < 3600: return "%dm ago" % (s // 60)
    if s < 172800: return "%dh ago" % (s // 3600)
    return "%dd ago" % (s // 86400)

persona = d.get("personaPath")
if persona:
    detail = ""
    try:
        st = os.stat(persona)
        with open(persona, encoding="utf-8", errors="replace") as f:
            lines = sum(1 for _ in f)
        detail = " (%d lines, updated %s)" % (lines, ago(st.st_mtime))
    except OSError:
        pass
    persona_line = persona + detail
else:
    persona_line = "(none)"

conns = d.get("connections") or []
# Local edges are always mutual (one relation row per pair = mutual
# awareness); only cross-daemon edges are labeled differently.
connects = ", ".join(
    "%s (%s)" % (c.get("peer", "?"), "remote" if c.get("remote") else "bidirectional")
    for c in conns) or "(none)"

# Project-group memberships, the PoC role marked.
projects = ", ".join(
    "%s%s" % (p.get("name", "?"), " (PoC)" if p.get("isPoc") else "")
    for p in d.get("projects") or []) or "(none)"

live = d.get("live") or {}
if live.get("active"):
    line = "yes — session %s" % str(live.get("sessionId", ""))[:8]
    up = live.get("uptimeSec")
    if isinstance(up, (int, float)):
        h, m = int(up) // 3600, (int(up) % 3600) // 60
        line += " (%dh %dm)" % (h, m) if h else " (%dm)" % m
else:
    line = "no"

print("agent    : %s" % d.get("name", "?"))
print("path     : %s" % d.get("path", "?"))
print("mode     : %s" % d.get("mode", "?"))
print("enabled  : %s" % ("true" if d.get("enabled") else "false"))
print("persona  : %s" % persona_line)
print("connects : %s" % connects)
print("projects : %s" % projects)
print("live     : %s" % line)
# GH #40: the files the agent reads its charter from.
problems = d.get("charterProblems")
if problems is not None:
    if not problems:
        print("charter  : ok")
    else:
        bits = []
        for pr in problems:
            if pr.get("problem") == "dangling":
                bits.append("%s dangling → %s" % (pr.get("file"), pr.get("target")))
            else:
                bits.append("%s missing" % pr.get("file"))
        print("charter  : PROBLEM — %s (fix: cd %s && k2 agent context regen)" % ("; ".join(bits), d.get("path", ".")))
PYEOF
}

cmd_agent_get() {
    if _wants_help "$@"; then cmd_help_agent_get; exit 0; fi
    local name="" key="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent get <name> <key> [--json]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"
                elif [ -z "$key" ]; then key="$1"
                else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent get <name> <key>"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ] || [ -z "$key" ]; then
        _agent_err "bad_usage" "usage: k2 agent get <name> <key> — keys: mode, enabled, name, path, persona"
        exit 2
    fi
    case "$key" in
        mode|enabled|name|path|persona|projects) ;;
        *)
            _agent_err "bad_usage" "unknown key '$key' — valid keys: mode, enabled, name, path, persona, projects"
            exit 2 ;;
    esac
    local conf rc=0
    conf=$(_agent_conf_fetch "$name") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    CONF="$conf" KEY="$key" AS_JSON="$as_json" python3 - <<'PYEOF'
import json, os, sys
d = json.loads(os.environ["CONF"])
key = os.environ["KEY"]
as_json = os.environ.get("AS_JSON") == "true"

if key == "persona":
    p = d.get("personaPath")
    if not p:
        sys.stderr.write(json.dumps({"error": {"code": "no_persona",
            "hint": "%s has no persona file yet — set one with: k2 agent set %s --persona <file.md>"
                    % (d.get("name"), d.get("name"))}}) + "\n")
        sys.exit(1)
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            value = f.read()
    except OSError as e:
        sys.stderr.write(json.dumps({"error": {"code": "persona_unreadable", "hint": str(e)}}) + "\n")
        sys.exit(1)
elif key == "projects":
    value = d.get("projects") or []
else:
    value = d.get(key)

if as_json:
    print(json.dumps({"ok": True, "key": key, "value": value}))
elif key == "persona":
    sys.stdout.write(value)
elif key == "projects":
    # Bare names, one per line — script-friendly ($(...) capture).
    for p in value:
        print(p.get("name", "?"))
elif value is True:
    print("true")
elif value is False:
    print("false")
else:
    print(value)
PYEOF
}

cmd_agent_set() {
    if _wants_help "$@"; then cmd_help_agent_set; exit 0; fi
    local name="" mode="" enabled="" new_name="" persona="" dry_run=false as_json=false
    local add_projects=() remove_projects=()
    local valid_flags="--mode --enabled --name --persona --add-project --remove-project --dry-run --json"
    while [ $# -gt 0 ]; do
        case "$1" in
            --mode)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--mode requires a value (off | custom | manager | k2)"; exit 2; }
                mode="$2"; shift 2 ;;
            --enabled)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--enabled requires a value (true | false)"; exit 2; }
                enabled="$2"; shift 2 ;;
            --name)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--name requires a value"; exit 2; }
                new_name="$2"; shift 2 ;;
            --persona)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--persona requires a file path"; exit 2; }
                persona="$2"; shift 2 ;;
            --add-project)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--add-project requires a project name or id (see: k2 project list)"; exit 2; }
                add_projects+=("$2"); shift 2 ;;
            --remove-project)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--remove-project requires a project name or id (see: k2 project list)"; exit 2; }
                remove_projects+=("$2"); shift 2 ;;
            --dry-run) dry_run=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — valid flags: $valid_flags"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent set <name> [flags]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        _agent_err "bad_usage" "missing <name> — usage: k2 agent set <name> [flags] (valid flags: $valid_flags)"
        exit 2
    fi
    if [ -z "$mode" ] && [ -z "$enabled" ] && [ -z "$new_name" ] && [ -z "$persona" ] \
        && [ ${#add_projects[@]} -eq 0 ] && [ ${#remove_projects[@]} -eq 0 ]; then
        _agent_err "bad_usage" "nothing to set — pass at least one of: --mode --enabled --name --persona --add-project --remove-project"
        exit 2
    fi
    case "$mode" in
        ""|off|custom|manager|k2|k2so) ;;
        *)
            _agent_err "bad_usage" "invalid --mode '$mode' — valid: off, custom, manager, k2"
            exit 2 ;;
    esac
    case "$enabled" in
        ""|true|false) ;;
        *)
            _agent_err "bad_usage" "invalid --enabled '$enabled' — valid: true, false"
            exit 2 ;;
    esac
    if [ -n "$persona" ] && { [ ! -f "$persona" ] || [ ! -r "$persona" ]; }; then
        _agent_err "bad_usage" "persona file not found or unreadable: $persona"
        exit 2
    fi
    local conf rc=0
    conf=$(_agent_conf_fetch "$name") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    local add_projects_json remove_projects_json
    add_projects_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${add_projects[@]+"${add_projects[@]}"})
    remove_projects_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${remove_projects[@]+"${remove_projects[@]}"})
    CONF="$conf" MODE="$mode" ENABLED="$enabled" NEW_NAME="$new_name" \
    PERSONA_FILE="$persona" ADD_PROJECTS="$add_projects_json" \
    REMOVE_PROJECTS="$remove_projects_json" DRY_RUN="$dry_run" AS_JSON="$as_json" \
    APROJTOKEN="$(_k2_project_token)" \
    ABASE="$BASE_URL" ATOKEN="$TOKEN" python3 - <<'PYEOF'
import json, os, re, sys
import urllib.error, urllib.parse, urllib.request

conf = json.loads(os.environ["CONF"])
mode = os.environ.get("MODE", "")
enabled = os.environ.get("ENABLED", "")
new_name = os.environ.get("NEW_NAME", "")
persona_file = os.environ.get("PERSONA_FILE", "")
add_projects = json.loads(os.environ.get("ADD_PROJECTS", "[]"))
remove_projects = json.loads(os.environ.get("REMOVE_PROJECTS", "[]"))
dry_run = os.environ.get("DRY_RUN") == "true"
as_json = os.environ.get("AS_JSON") == "true"
base = os.environ["ABASE"]
token = os.environ["ATOKEN"]

path = conf["path"]
agent_label = conf.get("name") or os.path.basename(path)

def fail(code, hint, exit_code=1):
    sys.stderr.write(json.dumps({"error": {"code": code, "hint": hint}}) + "\n")
    sys.exit(exit_code)

def http_get(route, params):
    qs = urllib.parse.urlencode(dict(params, token=token))
    try:
        with urllib.request.urlopen(base + route + "?" + qs, timeout=30) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

# 0.44.4: state-changing routes refuse GET (405). Same query-string params,
# sent as POST with an empty body.
def http_post_query(route, params, timeout=30):
    qs = urllib.parse.urlencode(dict(params, token=token))
    req = urllib.request.Request(base + route + "?" + qs, data=b"", method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

# Project membership changes ride the session passport inside a K2
# terminal (0.45.6), so the "Manage K2 projects" switch applies to them.
proj_token = os.environ.get("APROJTOKEN") or token

def http_post_json(route, body, tok=None):
    url = base + route + "?token=" + urllib.parse.quote(tok or token, safe="")
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

# CLI-canonical mode spelling (k2so accepted as a back-compat alias).
req_mode = {"k2so": "k2"}.get(mode, mode)

# ── Build the plan ───────────────────────────────────────────────
actions = []
if mode:
    if req_mode == conf.get("mode"):
        actions.append({"step": "set-mode", "mode": req_mode,
                        "status": "skipped", "reason": "already %s" % req_mode})
    else:
        actions.append({"step": "set-mode", "mode": req_mode, "status": "pending",
                        "from": conf.get("mode")})
if enabled:
    want = enabled == "true"
    if want == bool(conf.get("enabled")):
        actions.append({"step": "set-enabled", "enabled": want,
                        "status": "skipped", "reason": "already %s" % enabled})
    else:
        actions.append({"step": "set-enabled", "enabled": want, "status": "pending",
                        "from": bool(conf.get("enabled"))})
if new_name:
    if new_name == conf.get("name"):
        actions.append({"step": "rename", "name": new_name,
                        "status": "skipped", "reason": "already named %s" % new_name})
    else:
        actions.append({"step": "rename", "name": new_name, "status": "pending",
                        "from": conf.get("name")})
persona_content = None
if persona_file:
    try:
        with open(persona_file, encoding="utf-8", errors="replace") as f:
            persona_content = f.read()
    except OSError as e:
        fail("bad_usage", "cannot read persona file %s: %s" % (persona_file, e), 2)
    current = None
    if conf.get("personaPath"):
        try:
            with open(conf["personaPath"], encoding="utf-8", errors="replace") as f:
                current = f.read()
        except OSError:
            current = None
    if current is not None and current == persona_content:
        actions.append({"step": "write-persona", "source": persona_file,
                        "status": "skipped", "reason": "identical content"})
    else:
        actions.append({"step": "write-persona", "source": persona_file, "status": "pending"})

# Project memberships. The DAEMON owns selector resolution (full id |
# exact-name NOCASE | unique prefix — the `k2 project` discipline);
# unresolvable selectors are exit 4 (fix the name and retry).
def resolve_project(sel):
    d = http_get("/cli/project-group/show", {"group": sel})
    err = d.get("error")
    if err:
        if isinstance(err, str):
            err = {"code": "error", "hint": err}
        code = err.get("code", "error")
        if code == "ambiguous_id":
            code = "ambiguous"
        if code in ("not_found", "ambiguous"):
            fail(code, err.get("hint")
                 or "no project matches '%s' — see: k2 project list" % sel, 4)
        fail("daemon_error", "cannot resolve project '%s': %s"
             % (sel, err.get("hint") or code))
    if not d.get("ok") or not d.get("id"):
        fail("daemon_error",
             "unexpected response resolving project '%s': %s" % (sel, json.dumps(d)[:200]))
    return d

# Current memberships from conf: {group id: isPoc}.
member_of = {p["id"]: bool(p.get("isPoc"))
             for p in (conf.get("projects") or []) if p.get("id")}

adds, removes = [], []
for sel in add_projects:
    d = resolve_project(sel)
    if all(p["id"] != d["id"] for p in adds):
        adds.append({"id": d["id"], "name": d.get("name") or sel,
                     "memberless": not d.get("members")})
for sel in remove_projects:
    d = resolve_project(sel)
    if all(p["id"] != d["id"] for p in removes):
        removes.append({"id": d["id"], "name": d.get("name") or sel})
for p in adds:
    if any(r["id"] == p["id"] for r in removes):
        fail("bad_usage",
             "project '%s' is in both --add-project and --remove-project" % p["name"], 2)

for p in adds:
    a = {"step": "join-project", "project": p["name"], "projectId": p["id"]}
    if p["id"] in member_of:
        a.update(status="skipped", reason="already a member")
    else:
        a["status"] = "pending"
        if p["memberless"]:
            # First-ever member of an empty project auto-becomes the
            # Point of Contact (confirmed from the response at apply).
            a["becamePoc"] = True
    actions.append(a)
for p in removes:
    a = {"step": "leave-project", "project": p["name"], "projectId": p["id"]}
    if p["id"] not in member_of:
        a.update(status="skipped", reason="not a member")
    else:
        a["status"] = "pending"
        if member_of[p["id"]]:
            # The daemon will refuse (poc_successor_required) — keep
            # the step pending so a dry-run previews the refusal.
            a["note"] = "current PoC — refused until a successor is named"
    actions.append(a)

def emit(ok, changed, extra_status=None):
    out = {"ok": ok, "changed": changed, "name": conf.get("name"), "path": path,
           "actions": actions}
    if dry_run:
        out["dryRun"] = True
    if as_json:
        print(json.dumps(out))
    else:
        label = agent_label
        if dry_run:
            print("DRY RUN — no changes made. Plan for %s:" % label)
        for a in actions:
            step, status = a["step"], a["status"]
            if step == "set-mode":
                line = "mode: %s" % a["mode"] if status == "skipped" else \
                       "mode: %s → %s" % (a.get("from"), a["mode"])
            elif step == "set-enabled":
                want = "true" if a["enabled"] else "false"
                line = "enabled: %s" % want if status == "skipped" else \
                       "enabled: %s → %s" % ("true" if a.get("from") else "false", want)
            elif step == "rename":
                line = "name: %s" % a["name"] if status == "skipped" else \
                       "name: %s → %s" % (a.get("from"), a["name"])
            elif step == "join-project":
                line = "join-project: %s" % a["project"]
                if a.get("becamePoc"):
                    line += " (first member — Point of Contact)"
                if status == "skipped" and a.get("reason"):
                    line += "  (%s)" % a["reason"]
            elif step == "leave-project":
                line = "leave-project: %s" % a["project"]
                if a.get("note"):
                    line += " (%s)" % a["note"]
                if status == "skipped" and a.get("reason"):
                    line += "  (%s)" % a["reason"]
            else:
                line = "persona: replace from %s (previous backed up)" % a["source"] \
                       if status != "skipped" else "persona: unchanged (identical content)"
            marker = {"pending": "would apply", "done": "done", "skipped": "skipped",
                      "failed": "FAILED"}[status]
            print("  %-14s %s" % ("[" + marker + "]", line))
        if not dry_run:
            n = sum(1 for a in actions if a["status"] == "done")
            print("%s: %s" % (label, ("%d change(s) applied." % n) if n else "no changes."))

if dry_run:
    emit(True, any(a["status"] == "pending" for a in actions))
    sys.exit(0)

# ── Apply ────────────────────────────────────────────────────────
changed = False

# 1. DB settings (mode/enabled) in one /cli/workspace/set call.
fields = {}
for a in actions:
    if a["status"] != "pending":
        continue
    if a["step"] == "set-mode":
        fields["agent_mode"] = a["mode"]
    elif a["step"] == "set-enabled":
        fields["agent_enabled"] = "1" if a["enabled"] else "0"
if fields:
    resp = http_post_json("/cli/workspace/set", {"project": path, "fields": fields})
    if resp.get("error") or not resp.get("ok"):
        for a in actions:
            if a["status"] == "pending" and a["step"] in ("set-mode", "set-enabled"):
                a["status"] = "failed"
        emit(False, changed)
        err = resp.get("error")
        hint = err if isinstance(err, str) else (err or {}).get("hint") or json.dumps(resp)[:300]
        fail("daemon_error", "settings write failed: %s" % hint)
    by_field = {a.get("field"): a.get("status") for a in resp.get("actions", [])}
    for a in actions:
        if a["status"] != "pending":
            continue
        if a["step"] == "set-mode":
            a["status"] = by_field.get("agent_mode", "done")
        elif a["step"] == "set-enabled":
            a["status"] = by_field.get("agent_enabled", "done")
        if a["status"] == "done":
            changed = True

# 2. Rename (display name — AGENT.md frontmatter + live label).
for a in actions:
    if a["step"] == "rename" and a["status"] == "pending":
        resp = http_post_query("/cli/workspace/set-agent-display-name",
                        {"project_path": path, "name": a["name"]})
        if resp.get("error") or not resp.get("success"):
            a["status"] = "failed"
            emit(False, changed)
            fail("daemon_error", "rename failed: %s" % resp.get("error", json.dumps(resp)[:300]))
        a["status"] = "done"
        changed = True

# 3. Persona (charter file) via the existing save+backup machinery.
for a in actions:
    if a["step"] == "write-persona" and a["status"] == "pending":
        # Scaffold `.k2/agent/ROLE.md` first when the workspace has no
        # persona yet — /cli/agents/create is a no-op when it exists,
        # and save-agent-md refuses to write into a missing agent dir.
        agent_name = re.sub(r"[^A-Za-z0-9_-]", "-", os.path.basename(path)) or "agent"
        if not conf.get("personaPath"):
            resp = http_post_query("/cli/agents/create",
                            {"project_path": path, "name": agent_name, "role": ""})
            if resp.get("error"):
                a["status"] = "failed"
                emit(False, changed)
                fail("daemon_error", "persona scaffold failed: %s" % resp["error"])
        resp = http_post_json("/cli/agents/save-agent-md",
                              {"project_path": path, "agent_name": agent_name,
                               "content": persona_content})
        if resp.get("error") or not resp.get("success"):
            a["status"] = "failed"
            emit(False, changed)
            fail("daemon_error", "persona write failed: %s" % resp.get("error", json.dumps(resp)[:300]))
        a["status"] = "done"
        changed = True

# 4. Project memberships (add-member is idempotent daemon-side; the
#    PoC invariants are enforced there too — poc_successor_required
#    surfaces here with the successor hint).
for a in actions:
    if a["step"] not in ("join-project", "leave-project") or a["status"] != "pending":
        continue
    route = ("/cli/project-group/add-member" if a["step"] == "join-project"
             else "/cli/project-group/remove-member")
    resp = http_post_json(route, {"group": a["projectId"], "workspace": path}, tok=proj_token)
    err = resp.get("error")
    if err or not resp.get("ok"):
        if isinstance(err, str):
            err = {"code": "daemon_error", "hint": err}
        err = err or {"code": "daemon_error", "hint": json.dumps(resp)[:300]}
        a["status"] = "failed"
        emit(False, changed)
        code = err.get("code") or "daemon_error"
        hint = err.get("hint") or code
        if code == "poc_successor_required":
            hint += " — name a successor first: k2 project poc %s <workspace>" % a["project"]
        fail(code, hint)
    if a["step"] == "join-project" and resp.get("alreadyMember"):
        a["status"] = "skipped"
        a["reason"] = "already a member"
        a.pop("becamePoc", None)
    else:
        a["status"] = "done"
        a.pop("note", None)
        if a["step"] == "join-project":
            if resp.get("becamePoc"):
                a["becamePoc"] = True
            else:
                a.pop("becamePoc", None)
        changed = True

emit(True, changed)
PYEOF
}

cmd_help_agent_hire() {
    cat <<'EOF'
Provision a new agent — one atomic call that replaces the manual dance:
create the folder, write the persona, register the workspace, set mode,
wire connections, and optionally launch + send a first message.

Re-running hire on the same <dir> is safe: it completes whatever is
missing and reports changed:false when there is nothing to do.

USAGE
  k2 agent hire <dir> [flags]
  k2 agent hire --template <id> --name <name> [flags]    (folder: <hire root>/<handle>)

ARGUMENTS
  <dir>                   Directory for the agent's workspace (created if missing).
                          Optional with --template: the agent goes in the hire
                          root (`k2 agent hire-root`), in a folder named from
                          --name in lowercase with dashes ("Rita Lee" → rita-lee),
                          else the template id. --dry-run prints the folder.

FLAGS
  --name <name>           Agent's display name (default: the folder's basename, as-is)
  --template <id>         Make the agent from an agent template: its charter
                          (ROLE.md), starter wiki notes and context layers.
                          Types: general, lead, developer, app-owner, designer,
                          researcher, reviewer, writer, support, ops (plus any
                          saved on this server: `k2 agent template list`;
                          `k2 agent template show <id>` shows what one gives).
                          The old ids still work: worker = general,
                          manager = lead, qa = reviewer.
                          Writes only what's missing and never replaces the
                          agent's own files. One template per agent: hiring an
                          agent made from one template with another is
                          refused (exit 3); re-running with the same one is safe.
                          A template only SUGGESTS permissions; nothing is
                          turned on for the agent by the hire.
  --param <key>=<value>   A value the template asks for (repeatable). Quote a
                          value with spaces: --param about="the billing service".
                          Every built-in takes `about` (optional: what to focus
                          on). `show` lists each template's params and which
                          are required.
  --answer <id>=<text>    Answer one of the template's first-conversation
                          questions now (repeatable). Each built-in asks its
                          person 3–5 short questions (a writer: audience,
                          samples, avoid, drafts); `k2 agent template show <id>`
                          lists the ids and the questions. The text is plain
                          words or a link, up to 1000 characters; quote it
                          like --param: --answer audience="Local customers".
                          It goes into the agent's ROLE.md ("What I know about
                          my work") or the starter wiki note the question
                          names. Answer only what you know; don't guess (if
                          you know part, say which part is unsure, and the
                          agent checks it with its person). On its first
                          start the agent asks its person the questions left
                          out, on the Thread, and its hello names them. An unknown
                          id is refused (exit 2) with the valid ids, in a
                          --dry-run too. Like --param, it is written once: a
                          re-run never changes what the agent has, so to
                          correct an answer later, tell the agent.
  --persona <file.md>     Use this file as the agent's persona (copied to
                          <dir>/.k2/agent/ROLE.md). Mutually exclusive with
                          --template (passing both = exit 2).
  --mode <mode>           Agent mode: off | custom | manager | k2   (default: custom).
                          "off" also sets --enabled false.
  --agent <preset>        Which AGENT PROGRAM this workspace launches — an agent
                          preset id, label, or command token (see `k2 preset list`).
                          Sets the workspace's default agent during hire. Without
                          this, an unset workspace falls back to the global default
                          and ultimately to Claude — hiring a "codex agent" without
                          --agent codex would silently launch Claude.
  --enabled <bool>        true | false (default: true). false pauses the agent
                          without changing its configured mode.
  --connect <name>        Wire a connection to another agent (repeatable): a
                          workspace here, or agent::host on a paired server.
                          Connections are always mutual (bidirectional). The
                          daemon resolves each one first (dry run too); one it
                          can't find is not_found (exit 4).
  --project <p>           Add the new agent to a project (name or id; repeatable —
                          see `k2 project list`). Already a member = reported
                          no-op. The FIRST member of an empty project becomes
                          its Point of Contact.
  --focus-group <g>       Assign a Focus Group (name or id). `none` / `off`
                          clears. Unknown group = exit 4.
  --color <hex|rgb>       Workspace color: #rrggbb, #rgb, rgb(r,g,b), or r,g,b.
                          Not limited to the GUI swatches.
  --context <path|catalog-id>
                          Seed an always-on AGENTS.md layer after open (repeatable).
                          Explicit paths must exist or hire fails. Catalog ids from
                          `k2 agent context catalog` (wiki:index|home|hygiene,
                          subagents:pack, browser:pack, manager:pack, kessel:pack,
                          connections|heartbeats|skills|users:roster) warn if the file
                          is missing and still stack. Same layer on re-hire = no-op.
                          Day-2: k2 agent context …
  --no-wiki               Do not seed .k2/wiki (Home.md + _Index.md). Wiki is
                          seeded by default on first register.
  --no-agents-md          Do not plant cwd AGENTS.md. Compose still writes
                          .k2/AGENTS.md. Generate is on by default for new agents.
  --fanout                Plant leftover harness names (CLAUDE.md, GEMINI.md,
                          AGENT.md, …). Off by default.
  --launch                Start the agent's session after provisioning. On a
                          template hire's FIRST create, the agent then says
                          hello on its Thread (once: without --launch, or on a
                          re-run, no hello is sent).
  --onboard <msg>         Send <msg> to the agent after it starts (implies --launch).
                          With a template's hello, <msg> goes after the hello.
  --db-access off|read|write
                          Create-only DB passport (default off). Gates k2 db
                          create, not list/dsn/store. Does not mint a
                          database — call k2 db create after hire.
  --dry-run               Show the full plan; change nothing (steps that would
                          fail are FAILED, and it exits non-zero). With
                          --template it names the folder it would use, every
                          file it would write, each catalog layer, and the
                          permissions it suggests.
  --json                  JSON output

  A failed step (a connect, a project, a context…) doesn't stop the others:
  every step's result is printed, then the exit is non-zero (the first
  failure's: 4 not_found, 2 usage, 3 refused, 1 other). Only a failure to
  make the workspace itself stops the rest (reported "not run").

EXAMPLES
  k2 agent hire --template writer --name "Max" --agent codex --dry-run
  k2 agent hire --template writer --name "Max" --agent codex --launch
  k2 agent hire ~/agents/coder --template developer --param about="the billing service"
  k2 agent hire --template writer --name "Max" \
      --answer audience="Small-business owners" --answer samples="https://example.k2.dev/blog" --launch
  k2 agent hire ~/agents/scout --template general --connect "Appa" --launch
  k2 agent hire ~/agents/qa-bot --persona ./qa-persona.md --name "QA Bot" \
      --connect "Manager" --onboard "Welcome aboard — read your persona and check in."
  k2 agent hire ~/agents/researcher --template researcher --project "Blue Harbor"
  k2 agent hire ~/agents/mgr --template lead --context wiki:index
  k2 agent hire ~/agents/ops --context wiki:hygiene --context connections:roster \
      --context skills:roster
  k2 agent hire ~/agents/ops --focus-group Ops --color "#22c55e"
EOF
}

cmd_help_agent_retire() {
    cat <<'EOF'
Safely decommission an agent. Retire NEVER deletes: the agent's folder is
archived, its registration and connections are removed.

SAFETY GUARDS — retire REFUSES (exit 3) instead of asking, when it finds:
  * uncommitted or unpushed git work in the agent's folder
  * likely secrets (.env files, key material) in the agent's folder
Pass --force to proceed anyway. Use --dry-run to preview.

An agent that is a project's Point of Contact cannot be retired at all
(poc_of_projects — NOT --force-overridable): reassign the PoC first with
`k2 project poc <project> <successor>`. Plain project memberships are
removed automatically as part of retirement.

USAGE
  k2 agent retire <name> [flags]

FLAGS
  --archive-to <dir>      Where to move the folder (default: ~/.k2/archive/<name>-<date>/)
  --force                 Override the safety guards
  --dry-run               Show what would happen; change nothing
  --json                  JSON output
EOF
}

cmd_help_agent_restart() {
    cat <<'EOF'
Restart an agent's pinned chat session. The running CLI process is stopped
and started again on the SAME conversation (resume), so it re-reads its
charter (AGENTS.md / CLAUDE.md) and workspace config (.mcp.json) without
losing history. Use it after editing a charter or adding an MCP server.

Only the pinned chat is restarted; sidecar tabs keep running. If the agent
has no live session, restart starts one.

USAGE
  k2 agent restart <name> [flags]

  <name> is the agent's handle, its name, or its full workspace path
  (/home/k2/hooli/hooli-manager; a trailing slash or a symlink to the
  folder works too). When a name matches more than one workspace, or an
  old handle that was retired, it exits 4 and the error names each
  workspace's handle and path: use one of those.

FLAGS
  --dry-run               Show which session would restart; change nothing
  --json                  JSON output: {ok, changed, name, path, oldSessionId,
                          sessionId, conversationId, resumedExisting, provider}
EOF
}

cmd_help_agent_harness() {
    cat <<'EOF'
Move a workspace's pinned chat to another harness (the coding CLI it runs:
claude, codex, grok, gemini, cursor, pi or hermes).

The pinned chat is the chat heartbeats, Threads and `k2 msg` go to. It keeps
the harness it started with: changing the workspace's Default Agent only
changes NEW chats. This command moves the pinned chat itself:

  1. K2 ends the current pinned chat and starts a new one on <harness>,
     seeded with a summary of the recent conversation (the same summary as
     Chat history → "Continue in a new chat…").
  2. Heartbeats on the pinned chat, Threads and messages go to the new chat.
  3. The old chat stays in Chat history.

Already on <harness>: nothing changes (exit 0), unless you name a --model:
then it starts a new chat on that model with the summary. <harness> not installed
on this computer: nothing changes either (exit 1, harness_not_installed);
the old chat keeps running and the error says how to install it.
--dry-run runs the same check and shows the program it would launch.

USAGE
  k2 agent harness <workspace> <harness> [flags]

  <workspace>   handle, name or path ("." = the current directory)
  <harness>     claude, codex, grok, gemini, cursor, pi or hermes

FLAGS
  --full                  Seed with the whole old conversation (default: the recent part)
  --notes-file <path>     Extra handover notes for the new chat
  --model <id>            Start the new chat on this model, even if the preset
                          names another. This start only: a later resume uses
                          the preset or workspace model again
  --dry-run               Show what would move; change nothing
  --json                  The daemon's JSON

WHO
  A person: the owner (the desktop app, or a terminal outside K2), an Admin
  login, or a person typing in a K2 shell tab. An agent can't move its own
  pinned chat (that restarts its own session): it gets exit 3 and asks its
  person to run this command, or, with permission cards on:
    k2 permission request agent.harness-switch harness=<harness> --why "…"
  In the app: the pinned chat's menu → "Move pinned chat to …", or answer
  "Also move the pinned chat?" after changing a workspace's Default Agent.

EXAMPLES
  k2 agent harness sales claude
  k2 agent harness ~/agents/ops claude --dry-run
  k2 agent harness ops codex --full --notes-file handover.md
  k2 agent harness ops codex --model gpt-6-astra

EXIT CODES
  0 ok (moved, or already there) · 1 error · 2 bad usage · 3 refused · 4 not found
EOF
}

# `k2 agent harness <workspace> <harness>` (0.46.6) — move the pinned chat
# to another harness. One daemon door: POST /cli/agent/harness. Inside a K2
# terminal the passport goes (an `id` tool), so an agent is refused with
# `harness_needs_person` (exit 3) instead of acting as the owner.
cmd_agent_harness() {
    if _wants_help "$@"; then cmd_help_agent_harness; exit 0; fi
    local usage="usage: k2 agent harness <workspace> <harness> [--full] [--notes-file <path>] [--model <id>] [--dry-run] [--json]  (harness: claude, codex, grok, gemini, cursor, pi or hermes)"
    local ws="" harness="" mode="recent" notes_file="" model="" dry_run=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --full) mode="full"; shift ;;
            --recent) mode="recent"; shift ;;
            --notes-file)
                [ -n "${2:-}" ] || { _agent_err "bad_usage" "--notes-file takes a path — $usage"; exit 2; }
                notes_file="$2"; shift 2 ;;
            --model)
                [ -n "${2:-}" ] || { _agent_err "bad_usage" "--model takes a model id — $usage"; exit 2; }
                model="$2"; shift 2 ;;
            --dry-run) dry_run=true; shift ;;
            --json) as_json=true; shift ;;
            -*) _agent_err "bad_usage" "unknown flag '$1' — $usage"; exit 2 ;;
            *)
                if [ -z "$ws" ]; then ws="$1"
                elif [ -z "$harness" ]; then harness="$1"
                else _agent_err "bad_usage" "unexpected extra argument '$1' — $usage"; exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$ws" ] || [ -z "$harness" ]; then
        _agent_err "bad_usage" "$usage"
        exit 2
    fi
    if [ -n "$notes_file" ] && [ ! -f "$notes_file" ]; then
        _agent_err "not_found" "notes file not found: $notes_file"
        exit 1
    fi
    # A directory goes as its absolute path; the daemon resolves handles,
    # names and ids itself.
    if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
    local body
    body=$(H_WS="$ws" H_HARNESS="$harness" H_MODE="$mode" H_NOTES="$notes_file" H_MODEL="$model" H_DRY="$dry_run" python3 -c '
import json, os
notes = ""
path = os.environ.get("H_NOTES", "")
if path:
    with open(path, encoding="utf-8", errors="replace") as f:
        notes = f.read()
print(json.dumps({
    "workspace": os.environ["H_WS"],
    "harness": os.environ["H_HARNESS"],
    "mode": os.environ["H_MODE"],
    "notes": notes,
    "model": os.environ.get("H_MODEL", ""),
    "dryRun": os.environ.get("H_DRY") == "true",
}))
') || { _agent_err "bad_usage" "could not read the notes file: $notes_file"; exit 1; }
    local raw
    raw="$(_K2_SEND_MAX_TIME=150 cli_send_json POST /cli/agent/harness "$body")"
    local status="${raw##*$'\n'}" resp="${raw%$'\n'*}"
    if [ "$status" = "000" ] || [ -z "$status" ]; then
        _agent_err "daemon_unreachable" "no answer from the K2 daemon"
        exit 1
    fi
    printf '%s' "$resp" | H_STATUS="$status" H_JSON="$as_json" python3 -c '
import json, os, sys
status = int(os.environ["H_STATUS"])
as_json = os.environ["H_JSON"] == "true"
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print(json.dumps({"error": {"code": "daemon_error", "hint": "unparseable answer (HTTP %d): %s" % (status, raw[:300])}}), file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print(json.dumps({"error": {"code": "daemon_error", "hint": "unexpected answer (HTTP %d): %s" % (status, raw[:300])}}), file=sys.stderr)
    sys.exit(1)
if status >= 300 or d.get("error"):
    err = d.get("error")
    if isinstance(err, dict):
        code, hint = err.get("code") or "error", err.get("hint") or err.get("message") or ""
    else:
        code, hint = "error", str(err or "HTTP %d" % status)
    if status == 404 and "route" in hint and "not found" in hint:
        hint = "this K2 daemon has no k2 agent harness yet (update it); the older door is: k2 workspace swap-canonical <workspace> --agent <harness>"
    if as_json:
        print(json.dumps(d), file=sys.stderr)
    else:
        print("Error (%s): %s" % (code, hint), file=sys.stderr)
        path = d.get("handoffPath") or ""
        if path:
            print("Handoff file: %s" % path, file=sys.stderr)
    sys.exit(3 if status in (401, 403) else 4 if status == 404 else 2 if code in ("usage", "unknown_harness") else 1)
if as_json:
    print(json.dumps(d))
    sys.exit(0)
ws = d.get("workspace") or ""
h = d.get("harness") or ""
if d.get("alreadyOn"):
    print("%s: the pinned chat is already on %s. Nothing changed." % (ws, h))
    sys.exit(0)
own = d.get("ownSessionHeartbeats") or []
def own_note():
    if own:
        print("Heartbeats with their own session (they do not follow the pinned chat): %s." % ", ".join(own))
        print("  Move one with: k2 heartbeat use-pinned-session on <name>")
if d.get("dryRun"):
    if d.get("fromHarness") == h:
        print("DRY RUN: would start %s'"'"'s pinned chat again on %s." % (ws, h))
    else:
        print("DRY RUN: would move %s'"'"'s pinned chat from %s to %s." % (ws, d.get("fromHarness") or "?", h))
    if d.get("model"):
        print("Model: %s" % d["model"])
    if d.get("launches"):
        print("%s is installed: %s" % (h, d["launches"]))
    print("The old chat would stay in Chat history; heartbeats, Threads and messages would go to the new chat.")
    own_note()
    sys.exit(0)
print("Moved %s'"'"'s pinned chat: %s -> %s." % (ws, d.get("fromHarness") or "?", h))
if d.get("model"):
    print("Model: %s (for this start; a later resume uses the preset or workspace model again)" % d["model"])
conv = d.get("claudeSessionId") or ""
sid = d.get("sessionId") or ""
if conv or sid:
    print("New chat: %s%s" % (conv or "(id assigned when it starts)", (" (terminal %s)" % sid) if sid else ""))
old = d.get("fromSessionId") or ""
if old:
    listed = d.get("oldChatInHistory")
    if listed is False:
        print("Old chat: %s (not listed in Chat history: it had no saved conversation)." % old)
    else:
        print("Old chat: %s (still in Chat history)." % old)
print("Heartbeats on the pinned chat, Threads and messages now go to the %s chat." % h)
own_note()
if d.get("messageDelivered"):
    print("The handover summary was sent into the new chat.")
else:
    print("The new chat is up, but the handover summary was not sent (%s)." % (d.get("messageReason") or "not delivered"))
    if d.get("messageHint"):
        print(d["messageHint"])
    if d.get("handoffPath"):
        print("It is on disk: %s" % d["handoffPath"])
if d.get("seedError"):
    print("Note: the old transcript was not copied (%s)." % d["seedError"])
'
}

cmd_help_agent_refresh() {
    cat <<'EOF'
k2 agent refresh — start a new pinned chat that carries on from this one

For a long chat that has gone stale. K2 ends the current pinned chat, starts a
new one on the same harness with the same launch options, and sends it a summary
of the old chat and your note once it is ready.

  Stops:         the current chat, at once. It does not wait: anything running
                 in it stops. If that is you, so does this command.
  Carries on:    the Thread, messages and heartbeats on the pinned chat, in the
                 new chat. Sidecars keep running.
  Stays:         the old chat, in Chat history.

USAGE
  k2 agent refresh [<workspace>] [flags]

  <workspace>   handle, name or path ("." = the current directory). Default:
                this workspace.

FLAGS
  --note <text>           A note for the new chat: what you were doing, what's next
  --notes-file <path>     The note, from a file (at most 32,000 characters)
  --full                  Summarize a longer part of the old chat (default: the recent part)
  --dry-run               Show what would happen, and if a person has to allow it;
                          change nothing. It keeps no note and gives no line to ask with
  --json                  The daemon's JSON

WHO
  A person: the owner (the desktop app, or a terminal outside K2), an Admin login,
  or a person typing in a K2 shell tab, for any workspace.
  An agent: its main session, for its own chat, with "Can refresh its own pinned
  chat" on. Off, it gets exit 3, and with permission cards on the error ends with
  the line to ask with: k2 permission request chat.refresh … --why "…". Save your
  work first: once you've asked, the chat can end at any moment. A sidecar can't
  refresh itself.
  In the app: the pinned chat's menu → "Continue in a new chat…".

Full page: k2 study chat-refresh

EXAMPLES
  k2 agent refresh --note "Next: finish the import; the failing test is tests/import.rs"
  k2 agent refresh sales --notes-file handover.md
  k2 agent refresh sales --dry-run

EXIT CODES
  0 ok · 1 error · 2 bad usage · 3 refused · 4 not found
EOF
}

# `k2 agent refresh [<workspace>]` (0.46.8): one daemon door, POST
# /cli/agent/refresh. Inside a K2 terminal the passport goes (an `id` tool), so
# an agent acts as itself, never as the owner.
cmd_agent_refresh() {
    if _wants_help "$@"; then cmd_help_agent_refresh; exit 0; fi
    local usage="usage: k2 agent refresh [<workspace>] [--note <text> | --notes-file <path>] [--full] [--dry-run] [--json]"
    local ws="" mode="recent" note="" note_set=false notes_file="" dry_run=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --full) mode="full"; shift ;;
            --note)
                [ -n "${2:-}" ] || { _agent_err "bad_usage" "--note takes text — $usage"; exit 2; }
                note="$2"; note_set=true; shift 2 ;;
            --notes-file)
                [ -n "${2:-}" ] || { _agent_err "bad_usage" "--notes-file takes a path — $usage"; exit 2; }
                notes_file="$2"; shift 2 ;;
            --dry-run) dry_run=true; shift ;;
            --json) as_json=true; shift ;;
            -*) _agent_err "bad_usage" "unknown flag '$1' — $usage"; exit 2 ;;
            *)
                if [ -z "$ws" ]; then ws="$1"
                else _agent_err "bad_usage" "unexpected extra argument '$1' — $usage"; exit 2
                fi
                shift ;;
        esac
    done
    if [ "$note_set" = true ] && [ -n "$notes_file" ]; then
        _agent_err "bad_usage" "--note and --notes-file are one note: pass one — $usage"
        exit 2
    fi
    if [ -n "$notes_file" ] && [ ! -f "$notes_file" ]; then
        _agent_err "not_found" "notes file not found: $notes_file"
        exit 1
    fi
    # No workspace named: this one. An agent's passport names it anyway.
    [ -n "$ws" ] || ws="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-}}"
    if [ -z "$ws" ] && [ -z "${SCOPED_TOKEN:-}" ]; then
        _agent_err "bad_usage" "name the workspace — $usage"
        exit 2
    fi
    if [ -n "$ws" ] && [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
    local body
    body=$(H_WS="$ws" H_MODE="$mode" H_NOTE="$note" H_NOTES="$notes_file" H_DRY="$dry_run" python3 -c '
import json, os
notes = os.environ.get("H_NOTE", "")
path = os.environ.get("H_NOTES", "")
if path:
    with open(path, encoding="utf-8", errors="replace") as f:
        notes = f.read()
print(json.dumps({
    "workspace": os.environ.get("H_WS", ""),
    "mode": os.environ["H_MODE"],
    "notes": notes,
    "dryRun": os.environ.get("H_DRY") == "true",
}))
') || { _agent_err "bad_usage" "could not read the notes file: $notes_file"; exit 1; }
    local raw
    raw="$(_K2_SEND_MAX_TIME=150 cli_send_json POST /cli/agent/refresh "$body")"
    local status="${raw##*$'\n'}" resp="${raw%$'\n'*}"
    if [ "$status" = "000" ] || [ -z "$status" ]; then
        _agent_err "daemon_unreachable" "no answer from the K2 daemon"
        exit 1
    fi
    printf '%s' "$resp" | H_STATUS="$status" H_JSON="$as_json" python3 -c '
import json, os, sys
status = int(os.environ["H_STATUS"])
as_json = os.environ["H_JSON"] == "true"
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print(json.dumps({"error": {"code": "daemon_error", "hint": "unparseable answer (HTTP %d): %s" % (status, raw[:300])}}), file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print(json.dumps({"error": {"code": "daemon_error", "hint": "unexpected answer (HTTP %d): %s" % (status, raw[:300])}}), file=sys.stderr)
    sys.exit(1)
if status >= 300 or d.get("error"):
    err = d.get("error")
    if isinstance(err, dict):
        code, hint = err.get("code") or "error", err.get("hint") or err.get("message") or ""
    else:
        code, hint = "error", str(err or "HTTP %d" % status)
    if status == 404 and "route" in hint and "not found" in hint:
        hint = "this K2 daemon has no k2 agent refresh yet (update it)"
    if as_json:
        print(json.dumps(d), file=sys.stderr)
    else:
        print("Error (%s): %s" % (code, hint), file=sys.stderr)
        path = d.get("handoffPath") or ""
        if path:
            print("Handoff file: %s" % path, file=sys.stderr)
    sys.exit(3 if status in (401, 403) else 4 if status == 404 else 2 if code == "usage" else 1)
if as_json:
    print(json.dumps(d))
    sys.exit(0)
ws = d.get("workspace") or ""
h = d.get("harness") or ""
own = d.get("ownSessionHeartbeats") or []
def own_note():
    if own:
        print("Heartbeats with their own session (they do not follow the pinned chat): %s." % ", ".join(own))
        print("  Move one with: k2 heartbeat use-pinned-session on <name>")
if d.get("dryRun"):
    print("DRY RUN: would start a new %s pinned chat for %s." % (h, ws))
    if d.get("needsPerson"):
        print("You can'"'"'t run it yourself: a person has to allow this. Save your work, then run this again without --dry-run, with your note, for what to do next.")
    if d.get("launches"):
        print("%s is installed: %s" % (h, d["launches"]))
    print("Note: %s characters." % d.get("noteChars", 0))
    print("Anything running in the current chat would stop. The old chat would stay in Chat history; the Thread, heartbeats and messages would carry on in the new chat; sidecars would keep running.")
    own_note()
    sys.exit(0)
print("Refreshed %s'"'"'s pinned chat on %s." % (ws, h))
conv = d.get("claudeSessionId") or ""
sid = d.get("sessionId") or ""
if conv or sid:
    print("New chat: %s%s" % (conv or "(id assigned when it starts)", (" (terminal %s)" % sid) if sid else ""))
old = d.get("fromSessionId") or ""
if old:
    listed = d.get("oldChatInHistory")
    if listed is False:
        print("Old chat: %s (not listed in Chat history: it had no saved conversation)." % old)
    else:
        print("Old chat: %s (still in Chat history)." % old)
print("The Thread, heartbeats on the pinned chat and messages carry on in the new chat.")
own_note()
if d.get("messageDelivered"):
    print("The summary and note were sent into the new chat.")
else:
    print("The new chat is up, but the summary and note were not sent (%s)." % (d.get("messageReason") or "not delivered"))
    if d.get("messageHint"):
        print(d["messageHint"])
    if d.get("handoffPath"):
        print("They are on disk: %s" % d["handoffPath"])
if d.get("seedError"):
    print("Note: the old transcript was not copied (%s)." % d["seedError"])
'
}

cmd_help_agent_list() {
    cat <<'EOF'
List every agent on this server: name, mode, enabled, live-session
status, and workspace path.

USAGE
  k2 agent list [--json]

FLAGS
  --json                  Machine-readable output, for scripts (names and paths can
                          contain spaces; never parse the table):
                          {"ok":true,"agents":[{"name","id","handle","mode","enabled",
                           "live","sessionId","backend","ptydAdopted","path"}]}
                          path: absolute; live: true/false; sessionId and backend
                          ("ptyd" or "in-process") are null when not live;
                          ptydAdopted: true when the session survived a daemon
                          restart (false when not live). Check "ok" first.
EOF
}

# `k2 agent hire <dir>` — S3. One atomic, idempotent call composing the
# EXISTING routes (PRD §3): mkdir → persona (template | file | the
# /cli/agents/create default scaffold) → /cli/workspace/open →
# /cli/workspace/set (mode/enabled) → set-agent-display-name (--name;
# writes BOTH name stores) → /cli/connections per edge →
# ensure-canonical-session (--launch) → /cli/workspace/msg (--onboard).
# Re-running converges: satisfied steps report skipped(reason) and the
# whole call reports changed:false — which is also the crash-recovery
# story (a failed run is re-run to finish).
cmd_agent_hire() {
    if _wants_help "$@"; then cmd_help_agent_hire; exit 0; fi
    local dir="" name="" template="" persona="" mode="" enabled="" onboard="" agent_preset=""
    local launch=false dry_run=false as_json=false no_wiki=false
    local no_agents_md=false fanout=false db_access="" color="" focus_group=""
    local take_app="" take_brief_file=""
    local connects=() projects=() contexts=() params=() answers=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --name)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--name requires a value"; exit 2; }
                name="$2"; shift 2 ;;
            --agent)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--agent requires a preset id/label (see: k2 preset list)"; exit 2; }
                agent_preset="$2"; shift 2 ;;
            --template)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--template requires a template id (see: k2 agent template list)"; exit 2; }
                template="$2"; shift 2 ;;
            --param)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--param requires key=value (see: k2 agent template show <id>)"; exit 2; }
                case "$2" in
                    *=*) ;;
                    *) _agent_err "bad_usage" "--param takes key=value, got '$2'"; exit 2 ;;
                esac
                params+=("$2"); shift 2 ;;
            --answer)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--answer requires question-id=text (see: k2 agent template show <id>)"; exit 2; }
                case "$2" in
                    ?*=*) ;;
                    *) _agent_err "bad_usage" "--answer takes question-id=text, got '$2' (see: k2 agent template show <id>)"; exit 2 ;;
                esac
                answers+=("$2"); shift 2 ;;
            --persona)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--persona requires a file path"; exit 2; }
                persona="$2"; shift 2 ;;
            --mode)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--mode requires a value (off | custom | manager | k2)"; exit 2; }
                mode="$2"; shift 2 ;;
            --enabled)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--enabled requires a value (true | false)"; exit 2; }
                enabled="$2"; shift 2 ;;
            --connect)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--connect requires an agent name"; exit 2; }
                connects+=("$2"); shift 2 ;;
            --project)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--project requires a project name or id (see: k2 project list)"; exit 2; }
                projects+=("$2"); shift 2 ;;
            --focus-group)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--focus-group requires a group name or id (or none)"; exit 2; }
                focus_group="$2"; shift 2 ;;
            --color)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--color requires a hex or rgb value"; exit 2; }
                color="$2"; shift 2 ;;
            --context)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--context requires a path or catalog id (see: k2 agent context catalog)"; exit 2; }
                contexts+=("$2"); shift 2 ;;
            --take-app)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--take-app requires an App id (k2 app list)"; exit 2; }
                take_app="$2"; shift 2 ;;
            --brief)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--brief requires a file (your notes for the new agent)"; exit 2; }
                take_brief_file="$2"; shift 2 ;;
            --no-wiki) no_wiki=true; shift ;;
            --no-agents-md) no_agents_md=true; shift ;;
            --fanout) fanout=true; shift ;;
            --launch) launch=true; shift ;;
            --onboard)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--onboard requires a message"; exit 2; }
                onboard="$2"; shift 2 ;;
            --dry-run) dry_run=true; shift ;;
            --db-access)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--db-access requires off|read|write"; exit 2; }
                db_access="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — see: k2 agent hire --help"
                exit 2 ;;
            *)
                if [ -z "$dir" ]; then dir="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent hire <dir> [flags]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$dir" ] && [ -z "$template" ]; then
        _agent_err "bad_usage" "missing <dir> — usage: k2 agent hire <dir> [flags], or k2 agent hire --template <id> --name <n> to hire under the hire root (see: k2 agent hire --help)"
        exit 2
    fi
    if [ -n "$template" ] && [ -n "$persona" ]; then
        _agent_err "bad_usage" "--template and --persona are mutually exclusive — pass one or the other"
        exit 2
    fi
    if [ ${#params[@]} -gt 0 ] && [ -z "$template" ]; then
        _agent_err "bad_usage" "--param goes with --template <id>"
        exit 2
    fi
    if [ ${#answers[@]} -gt 0 ] && [ -z "$template" ]; then
        _agent_err "bad_usage" "--answer goes with --template <id>"
        exit 2
    fi
    # 0.46.12 (prd-agent-templates-v1 AT1): the daemon holds every template
    # and writes the persona; the CLI sends the id and the params.
    if [ -n "$persona" ] && { [ ! -f "$persona" ] || [ ! -r "$persona" ]; }; then
        _agent_err "bad_usage" "persona file not found or unreadable: $persona"
        exit 2
    fi
    case "$mode" in
        ""|off|custom|manager|k2|k2so) ;;
        *)
            _agent_err "bad_usage" "invalid --mode '$mode' — valid: off, custom, manager, k2"
            exit 2 ;;
    esac
    case "$enabled" in
        ""|true|false) ;;
        *)
            _agent_err "bad_usage" "invalid --enabled '$enabled' — valid: true, false"
            exit 2 ;;
    esac
    case "$db_access" in
        ""|off|read|write) ;;
        *)
            _agent_err "bad_usage" "invalid --db-access '$db_access' — valid: off, read, write"
            exit 2 ;;
    esac
    # 0.44.4: an agent must not grant itself database access (the `agent`
    # verb runs with the owner token in-cell). Lowering to `off` is fine.
    if { [ "$db_access" = "read" ] || [ "$db_access" = "write" ]; } && _cli_caller_is_agent_passport; then
        _cli_refuse_agent_self_grant "db_agent_access" \
            "Settings → Workspaces → (workspace) → Allow this agent to create databases, or \`k2 agent hire <dir> --db-access write\` from their own terminal"
    fi
    # --onboard implies --launch (mockup contract).
    [ -n "$onboard" ] && launch=true
    # 0.46.5 --take-app: after the hire, the App's handover to the new agent.
    local take_brief=""
    if [ -n "$take_brief_file" ]; then
        [ -n "$take_app" ] || { _agent_err "bad_usage" "--brief goes with --take-app"; exit 2; }
        [ -f "$take_brief_file" ] || { _agent_err "bad_usage" "brief file not found: $take_brief_file"; exit 2; }
        take_brief="$(cat "$take_brief_file")"
    fi
    # 0.45.6: inside a K2 cell the hire core call carries the session
    # passport, so the DAEMON refuses an agent raising DB access (the check
    # above is the fast path; the daemon is the authority).
    local core_token=""
    if _cli_caller_is_agent_passport; then
        core_token="$SCOPED_TOKEN"
    fi

    local connects_json projects_json contexts_json
    connects_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${connects[@]+"${connects[@]}"})
    projects_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${projects[@]+"${projects[@]}"})
    contexts_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${contexts[@]+"${contexts[@]}"})
    local params_json
    params_json=$(python3 -c 'import json,sys; print(json.dumps(dict(a.split("=", 1) for a in sys.argv[1:])))' \
        ${params[@]+"${params[@]}"})
    # --answer id=text: a repeated id is a usage error (not a silent last-wins).
    local answers_json
    answers_json=$(python3 -c '
import json, sys
out = {}
for a in sys.argv[1:]:
    k, v = a.split("=", 1)
    k = k.strip()
    if k in out:
        sys.stderr.write(json.dumps({"error": {"code": "bad_usage", "hint": "--answer %s given twice" % k}}) + "\n")
        sys.exit(2)
    out[k] = v
print(json.dumps(out))' ${answers[@]+"${answers[@]}"}) || exit 2

    HIRE_DIR="$dir" NAME="$name" TEMPLATE="$template" TEMPLATE_PARAMS="$params_json" TEMPLATE_ANSWERS="$answers_json" \
    PERSONA_FILE="$persona" MODE="$mode" ENABLED="$enabled" CONNECTS="$connects_json" \
    PROJECTS="$projects_json" CONTEXTS="$contexts_json" \
    LAUNCH="$launch" ONBOARD="$onboard" DRY_RUN="$dry_run" AGENT_PRESET="$agent_preset" \
    NO_WIKI="$no_wiki" NO_AGENTS_MD="$no_agents_md" FANOUT="$fanout" \
    DB_ACCESS="$db_access" COLOR="$color" FOCUS_GROUP="$focus_group" \
    TAKE_APP="$take_app" TAKE_BRIEF="$take_brief" \
    APROJTOKEN="$(_k2_project_token)" \
    AS_JSON="$as_json" ABASE="$BASE_URL" ATOKEN="$TOKEN" PTOKEN="$core_token" python3 - <<'PYEOF'
import json, os, re, sys
import urllib.error, urllib.parse, urllib.request

dir_arg = os.environ["HIRE_DIR"]
name = os.environ.get("NAME", "")
template = os.environ.get("TEMPLATE", "").strip()
template_params = json.loads(os.environ.get("TEMPLATE_PARAMS") or "{}")
template_answers = json.loads(os.environ.get("TEMPLATE_ANSWERS") or "{}")
persona_file = os.environ.get("PERSONA_FILE", "")
mode = os.environ.get("MODE", "")
enabled = os.environ.get("ENABLED", "")
connects = json.loads(os.environ.get("CONNECTS", "[]"))
wanted_projects = json.loads(os.environ.get("PROJECTS", "[]"))
wanted_contexts = json.loads(os.environ.get("CONTEXTS", "[]"))
launch = os.environ.get("LAUNCH") == "true"
onboard = os.environ.get("ONBOARD", "")
agent_preset = os.environ.get("AGENT_PRESET", "")
dry_run = os.environ.get("DRY_RUN") == "true"
as_json = os.environ.get("AS_JSON") == "true"
no_wiki = os.environ.get("NO_WIKI") == "true"
no_agents_md = os.environ.get("NO_AGENTS_MD") == "true"
fanout = os.environ.get("FANOUT") == "true"
db_access = os.environ.get("DB_ACCESS", "")
color_arg = os.environ.get("COLOR", "")
focus_group_arg = os.environ.get("FOCUS_GROUP", "")
take_app = os.environ.get("TAKE_APP", "").strip()
take_brief = os.environ.get("TAKE_BRIEF", "")
base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
core_token = os.environ.get("PTOKEN", "") or token

def fail(code, hint, exit_code=1):
    sys.stderr.write(json.dumps({"error": {"code": code, "hint": hint}}) + "\n")
    sys.exit(exit_code)

def http_get(route, params, timeout=30):
    qs = urllib.parse.urlencode(dict(params, token=token))
    try:
        with urllib.request.urlopen(base + route + "?" + qs, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

# 0.44.4: state-changing routes refuse GET (405). Same query-string params,
# sent as POST with an empty body.
def http_post_query(route, params, timeout=30):
    qs = urllib.parse.urlencode(dict(params, token=token))
    req = urllib.request.Request(base + route + "?" + qs, data=b"", method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

# Project membership changes ride the session passport inside a K2
# terminal (0.45.6), so the "Manage K2 projects" switch applies to them.
proj_token = os.environ.get("APROJTOKEN") or token

def http_post_json(route, body, timeout=30, tok=None):
    url = base + route + "?token=" + urllib.parse.quote(tok or token, safe="")
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception as e:
        return {"error": "%s: %s" % (route, e)}

def template_body(extra):
    """The /cli/agents/hire body for a template hire (0.46.12)."""
    b = {"template": template, "params": template_params, "noWiki": no_wiki,
         "noAgentsMd": no_agents_md, "fanout": fanout}
    if template_answers:
        b["answers"] = template_answers
    if name:
        b["name"] = name
    b.update(extra)
    return b

def core_error(resp):
    err = resp.get("error")
    code = err.get("code") if isinstance(err, dict) else "daemon_error"
    hint = err.get("hint") if isinstance(err, dict) else (err or json.dumps(resp)[:300])
    return code or "daemon_error", hint

def template_exit(code):
    c = (code or "").lower()
    return {"usage": 2, "bad_template": 2, "unknown_template": 4, "not_found": 4,
            "template_conflict": 3, "folder_exists": 3, "owner_only": 3,
            "system_template": 3}.get(c, 1)

# 0.46.12: with --template, <dir> is optional: the daemon picks
# <hire root>/<handle> (a dry run of the hire names it; nothing written).
if dir_arg:
    path = os.path.abspath(os.path.expanduser(dir_arg))
else:
    probe = http_post_json("/cli/agents/hire", template_body({"dryRun": True}), tok=core_token)
    if probe.get("error") or not probe.get("path"):
        code, hint = core_error(probe)
        fail(code, hint, template_exit(code))
    path = probe["path"]

if os.path.exists(path) and not os.path.isdir(path):
    fail("bad_usage", "%s exists and is not a directory" % path, 2)

def normalize_color(raw):
    s = (raw or "").strip()
    if not s:
        return None
    hexpart = s[1:] if s.startswith("#") else s
    hexpart = hexpart.lower()
    if re.fullmatch(r"[0-9a-f]{6}", hexpart):
        return "#" + hexpart
    if re.fullmatch(r"[0-9a-f]{3}", hexpart):
        return "#" + "".join(c * 2 for c in hexpart)
    m = re.fullmatch(
        r"rgba?\(\s*([0-9]{1,3})\s*[, ]\s*([0-9]{1,3})\s*[, ]\s*([0-9]{1,3})(?:\s*[,/]\s*[0-9.]+\s*)?\)",
        s, re.I)
    if not m:
        m = re.fullmatch(r"([0-9]{1,3})\s*,\s*([0-9]{1,3})\s*,\s*([0-9]{1,3})", s)
    if m:
        parts = []
        for g in m.groups():
            n = int(g)
            if n > 255:
                return None
            parts.append("%02x" % n)
        return "#" + "".join(parts)
    return None

# ── Resolve --agent against the preset roster (fail fast) ────────
# Stored value = the matched preset's canonical ID (the resolver's
# preferred key; labels/command-tokens are accepted as INPUT sugar,
# same tolerance as workspace::agent_resolve). The preset must exist
# AND be enabled — the daemon-side resolver skips disabled presets,
# which would silently re-introduce the Claude fallback this flag
# exists to fix.
resolved_preset = None
if agent_preset:
    presets = http_get("/cli/presets/list", {})
    if not isinstance(presets, list):
        err = presets.get("error") if isinstance(presets, dict) else None
        fail("daemon_error", "cannot list agent presets: %s" % (err or "unexpected response"))
    want = agent_preset.strip()
    def preset_matches(p):
        first_token = (p.get("command") or "").split()[:1]
        return (p.get("id") == want
                or (p.get("label") or "").lower() == want.lower()
                or (first_token and first_token[0] == want))
    hits = [p for p in presets if preset_matches(p)]
    if not hits:
        fail("not_found",
             "no agent preset matches '%s' — run `k2 preset list` (id, label, or command token)" % want,
             4)
    if len(hits) > 1 and len({p.get("id") for p in hits}) > 1:
        fail("bad_usage",
             "'%s' is ambiguous across presets: %s — pass the preset id" %
             (want, ", ".join(sorted(p.get("id") or "?" for p in hits))), 2)
    resolved_preset = hits[0]
    if not resolved_preset.get("enabled"):
        fail("bad_usage",
             "preset '%s' is disabled — enable it first: k2 preset set %s --enable" %
             (want, resolved_preset.get("id")), 2)

# ── Resolve --project selectors (fail fast, mirroring --agent) ────
# The DAEMON owns selector resolution (full id | exact-name NOCASE |
# unique prefix — the `k2 project` discipline): resolve each via
# `show`; unresolvable names are exit 4 (fix the name and retry).
resolved_projects = []  # [{id, name, memberless}]
for sel in wanted_projects:
    d = http_get("/cli/project-group/show", {"group": sel})
    err = d.get("error")
    if err:
        if isinstance(err, str):
            err = {"code": "error", "hint": err}
        code = err.get("code", "error")
        if code == "ambiguous_id":
            code = "ambiguous"
        if code in ("not_found", "ambiguous"):
            fail(code, err.get("hint")
                 or "no project matches '%s' — see: k2 project list" % sel, 4)
        fail("daemon_error", "cannot resolve project '%s': %s"
             % (sel, err.get("hint") or code))
    if not d.get("ok") or not d.get("id"):
        fail("daemon_error",
             "unexpected response resolving project '%s': %s" % (sel, json.dumps(d)[:200]))
    if all(p["id"] != d["id"] for p in resolved_projects):
        resolved_projects.append({"id": d["id"], "name": d.get("name") or sel,
                                  "memberless": not d.get("members")})

wanted_color = None
if color_arg:
    wanted_color = normalize_color(color_arg)
    if wanted_color is None:
        fail("bad_usage",
             "invalid --color '%s' — use #rrggbb, #rgb, rgb(r,g,b), or r,g,b" % color_arg, 2)

wanted_focus_id = None  # None = not requested; "" = clear; uuid = assign
wanted_focus_label = None
if focus_group_arg:
    sel = focus_group_arg.strip()
    if sel.lower() in ("none", "off", "-"):
        wanted_focus_id = ""
        wanted_focus_label = "none"
    else:
        groups = http_get("/cli/focus-groups/list", {})
        if not isinstance(groups, list):
            err = groups.get("error") if isinstance(groups, dict) else None
            fail("daemon_error", "cannot list focus groups: %s" % (err or "unexpected response"))
        def group_matches(g):
            return (g.get("id") == sel
                    or (g.get("name") or "").lower() == sel.lower())
        hits = [g for g in groups if group_matches(g)]
        if not hits:
            prefixes = [g for g in groups
                        if (g.get("name") or "").lower().startswith(sel.lower())]
            if len(prefixes) == 1:
                hits = prefixes
        if not hits:
            fail("not_found",
                 "no focus group matches '%s' — create one in Settings → Workspaces" % sel, 4)
        if len(hits) > 1:
            fail("ambiguous",
                 "'%s' matches multiple focus groups: %s — pass the id" %
                 (sel, ", ".join(sorted((g.get("name") or g.get("id") or "?") for g in hits))), 4)
        wanted_focus_id = hits[0].get("id") or ""
        wanted_focus_label = hits[0].get("name") or sel

# ── Probe current state (this is what makes hire idempotent) ─────
conf = http_get("/cli/agent/conf", {"q": path})
probe_err = conf.get("error")
if probe_err:
    if isinstance(probe_err, dict) and probe_err.get("code") == "not_found":
        pass  # unregistered — hire is about to register it
    else:
        hint = probe_err.get("hint") if isinstance(probe_err, dict) else str(probe_err)
        fail("daemon_error", "cannot probe %s: %s" % (path, hint))
registered = bool(conf.get("ok"))

dir_exists = os.path.isdir(path)
basename = os.path.basename(path.rstrip("/")) or "agent"
# The technical agent name (frontmatter name:, /cli/agents/create) —
# same sanitization the S2 persona scaffold uses.
agent_name = re.sub(r"[^A-Za-z0-9_-]", "-", basename) or "agent"

persona_path = conf.get("personaPath") if registered else None
if not persona_path:
    for dot in (".k2", ".k2so"):
        # `fname`, not `name`: `name` is the --name value (A8 — a loop
        # variable called `name` renamed every new hire "AGENT.md").
        for fname in ("ROLE.md", "AGENT.md"):
            cand = os.path.join(path, dot, "agent", fname)
            if os.path.isfile(cand):
                persona_path = cand
                break
        if persona_path:
            break
persona_exists = bool(persona_path)

persona_content = None
if persona_file:
    try:
        with open(persona_file, encoding="utf-8", errors="replace") as f:
            persona_content = f.read()
    except OSError as e:
        fail("bad_usage", "cannot read persona file %s: %s" % (persona_file, e), 2)
# A template's persona is the daemon's to write (AT1).

desired_mode = mode or "custom"
# CLI-canonical spelling (k2so accepted as a back-compat alias).
desired_mode = {"k2so": "k2"}.get(desired_mode, desired_mode)

existing_peers = set()
existing_group_ids = set()
existing_context_paths = set()  # workspace-relative paths already stacked
existing_context_sources = set()
live = False
if registered:
    for c in conf.get("connections") or []:
        p = c.get("peer")
        if p:
            existing_peers.add(p.lower())
    for p in conf.get("projects") or []:
        if p.get("id"):
            existing_group_ids.add(p["id"])
    live = bool((conf.get("live") or {}).get("active"))
    # Probe context stack for hire --context idempotency.
    layers_resp = http_get("/cli/context/layers", {"project": path})
    if layers_resp.get("ok") or "layers" in layers_resp:
        for L in layers_resp.get("layers") or []:
            lp = (L.get("path") or "").replace("\\", "/").strip().lstrip("./")
            if lp:
                existing_context_paths.add(lp)
            src = (L.get("source") or "").strip()
            if src:
                existing_context_sources.add(src)

def context_is_path(token):
    """Match k2 agent context add heuristic: path-like vs catalog id."""
    t = token or ""
    return (
        "/" in t
        or t.startswith(".")
        or t.endswith(".md")
        or t.endswith(".markdown")
        or t.startswith("~")
    )

# ── Build the plan ───────────────────────────────────────────────
actions = []
if dir_exists:
    actions.append({"step": "create-dir", "status": "skipped", "reason": "exists"})
else:
    actions.append({"step": "create-dir", "status": "pending"})

wp = {"step": "write-persona"}
if template:
    wp["template"] = template
elif persona_file:
    wp["source"] = persona_file
else:
    wp["source"] = "default-scaffold"
if persona_exists:
    # Hire heals MISSING pieces; replacing an existing persona is
    # `k2 agent set --persona`'s job (with backups).
    wp.update(status="skipped", reason="exists")
else:
    wp["status"] = "pending"
actions.append(wp)

if template:
    # The template's own steps (starters, catalog, layers, lock): the
    # daemon looks first and writes only what's missing, on every run.
    actions.append({"step": "apply-template", "template": template, "status": "pending"})

if registered:
    actions.append({"step": "register-workspace", "status": "skipped",
                    "reason": "already-registered"})
else:
    actions.append({"step": "register-workspace", "status": "pending",
                    "wiki": "skip" if no_wiki else "seed"})

if no_agents_md:
    actions.append({"step": "generate-agents-md", "status": "skipped",
                    "reason": "no-agents-md"})
elif registered:
    actions.append({"step": "generate-agents-md", "status": "skipped",
                    "reason": "already"})
else:
    actions.append({"step": "generate-agents-md", "status": "pending"})

if not fanout:
    actions.append({"step": "fanout", "status": "skipped", "reason": "off"})
elif registered:
    actions.append({"step": "fanout", "status": "skipped", "reason": "already"})
else:
    actions.append({"step": "fanout", "status": "pending"})

if registered and conf.get("mode") == desired_mode:
    actions.append({"step": "set-mode", "mode": desired_mode,
                    "status": "skipped", "reason": "already %s" % desired_mode})
else:
    actions.append({"step": "set-mode", "mode": desired_mode, "status": "pending"})

if enabled:
    want = enabled == "true"
    if registered and bool(conf.get("enabled")) == want and (desired_mode == conf.get("mode")):
        actions.append({"step": "set-enabled", "enabled": want,
                        "status": "skipped", "reason": "already %s" % enabled})
    else:
        actions.append({"step": "set-enabled", "enabled": want, "status": "pending"})

if name:
    if registered and conf.get("name") == name:
        actions.append({"step": "set-name", "name": name,
                        "status": "skipped", "reason": "already named %s" % name})
    else:
        actions.append({"step": "set-name", "name": name, "status": "pending"})

if resolved_preset is not None:
    # Converge-detection happens at apply time (the /cli/workspace/set
    # route reports per-field skipped when the stored value already
    # matches) — conf doesn't expose default_agent, so a dry-run shows
    # this step as would-apply even when already set.
    actions.append({"step": "set-default-agent",
                    "preset": resolved_preset.get("id"),
                    "label": resolved_preset.get("label") or resolved_preset.get("id"),
                    "status": "pending"})

if db_access:
    actions.append({"step": "set-db-access", "access": db_access, "status": "pending"})

for peer in connects:
    # Connections are always mutual: the substrate stores ONE row per
    # pair (migration 0051 — a single row IS bidirectional awareness).
    if peer.lower() in existing_peers:
        actions.append({"step": "connect", "peer": peer, "status": "skipped",
                        "reason": "edge exists", "bidirectional": True})
        continue
    a = {"step": "connect", "peer": peer, "status": "pending", "bidirectional": True}
    # 0.46.4 (field report): the DAEMON resolves the target now, dry run
    # included — a local workspace, or agent::host on a paired server's
    # roster. A miss is reported up front instead of "would apply".
    r = http_get("/cli/connections", {"action": "resolve", "target": peer})
    rerr = r.get("error") if isinstance(r, dict) else None
    if rerr or not (isinstance(r, dict) and r.get("ok")):
        if not isinstance(rerr, dict):
            rerr = {"code": "daemon_error", "hint": str(rerr or json.dumps(r)[:200])}
        a.update(status="failed", error={"code": rerr.get("code") or "daemon_error",
                                         "hint": rerr.get("hint") or "cannot resolve '%s'" % peer})
    elif r.get("kind") == "local" and r.get("path") == path:
        a.update(status="failed", error={"code": "bad_usage",
                                         "hint": "cannot connect %s to itself" % peer})
    else:
        a["kind"] = r.get("kind")
        a["resolved"] = r.get("path") if r.get("kind") == "local" else r.get("address")
        if r.get("kind") == "remote" and (r.get("address") or "").lower() in existing_peers:
            a.update(status="skipped", reason="edge exists")
        if r.get("warning"):
            a["warn"] = r["warning"]
    actions.append(a)

for p in resolved_projects:
    # Membership is idempotent daemon-side too (alreadyMember is an Ok
    # no-op) — the plan-time skip keeps re-runs reporting changed:false.
    a = {"step": "join-project", "project": p["name"], "projectId": p["id"]}
    if p["id"] in existing_group_ids:
        a.update(status="skipped", reason="already a member")
    else:
        a["status"] = "pending"
        if p["memberless"]:
            # First-ever member of an empty project auto-becomes the
            # Point of Contact (confirmed from the response at apply).
            a["becamePoc"] = True
    actions.append(a)

if wanted_color is not None:
    current_color = (conf.get("color") or "").lower() if registered else ""
    a = {"step": "set-color", "color": wanted_color}
    if registered and current_color == wanted_color:
        a.update(status="skipped", reason="already %s" % wanted_color)
    else:
        a["status"] = "pending"
    actions.append(a)

if wanted_focus_id is not None:
    current_fg = conf.get("focusGroupId") if registered else None
    if current_fg is None:
        current_fg = ""
    a = {"step": "assign-focus-group", "group": wanted_focus_label,
         "focusGroupId": wanted_focus_id or None}
    if registered and current_fg == (wanted_focus_id or ""):
        a.update(status="skipped", reason="already %s" % (wanted_focus_label or "none"))
    else:
        a["status"] = "pending"
    actions.append(a)

# Context seeds (after open). Explicit path missing → fail now.
# Presets missing on disk → still planned; apply warns and continues.
for ctx in wanted_contexts:
    ctx = (ctx or "").strip()
    if not ctx:
        fail("bad_usage", "--context value must not be empty", 2)
    a = {"step": "seed-context", "context": ctx}
    is_path = context_is_path(ctx)
    if is_path:
        a["kind"] = "path"
        # Resolve relative to the hire workspace (not cwd of the CLI).
        if os.path.isabs(os.path.expanduser(ctx)):
            abs_ctx = os.path.abspath(os.path.expanduser(ctx))
        else:
            abs_ctx = os.path.abspath(os.path.join(path, ctx))
        a["resolvedPath"] = abs_ctx
        if not os.path.isfile(abs_ctx):
            fail(
                "not_found",
                "context path not found: %s (explicit --context paths must exist at hire time)"
                % ctx,
                4,
            )
        # Relative form for stack comparison / daemon body.
        try:
            rel = os.path.relpath(abs_ctx, path).replace("\\", "/")
        except ValueError:
            rel = abs_ctx
        if rel.startswith(".."):
            fail(
                "bad_usage",
                "context path escapes hire workspace: %s" % ctx,
                2,
            )
        a["path"] = rel
        if rel.lstrip("./") in existing_context_paths or rel in existing_context_paths:
            a.update(status="skipped", reason="already stacked")
        else:
            a["status"] = "pending"
    else:
        a["kind"] = "catalog"
        a["catalog"] = ctx
        # Source tags for built-in catalog entries (best-effort plan-time skip).
        # Keep in sync with daemon list_catalog() + aliases in resolve_catalog_id().
        # Live rosters + static packs + wiki path seeds.
        src_guess = {
            "wiki:index": "catalog:wiki-index",
            "wiki:home": "catalog:wiki-home",
            "wiki:hygiene": "catalog:wiki-hygiene",
            "wiki-hygiene": "catalog:wiki-hygiene",
            "hygiene": "catalog:wiki-hygiene",
            "subagents:pack": "catalog:subagents",
            "subagents": "catalog:subagents",
            "always-use-subagents": "catalog:subagents",
            "use-subagents": "catalog:subagents",
            "browser:pack": "catalog:browser",
            "browser": "catalog:browser",
            "browser-and-computer-use": "catalog:browser",
            "catalog:browser": "catalog:browser",
            "manager:pack": "catalog:manager",
            "manager": "catalog:manager",
            "workspace-manager": "catalog:manager",
            "k2:pack": "catalog:kessel", "kessel:pack": "catalog:kessel", "kessel": "catalog:kessel",
            "k2-agent": "catalog:kessel",
            "k2": "catalog:kessel",
            "connections:roster": "catalog:connections-roster",
            "connections": "catalog:connections-roster",
            "roster": "catalog:connections-roster",
            "heartbeats:roster": "catalog:heartbeats-roster",
            "heartbeats": "catalog:heartbeats-roster",
            "skills:roster": "catalog:skills-roster",
            "skills": "catalog:skills-roster",
            "skills-index": "catalog:skills-roster",
            "users:roster": "catalog:users-roster",
            "users": "catalog:users-roster",
            "users-roster": "catalog:users-roster",
            "people": "catalog:users-roster",
            "people-roster": "catalog:users-roster",
            "user-roster": "catalog:users-roster",
            "catalog:users-roster": "catalog:users-roster",
        }.get(ctx)
        if src_guess and src_guess in existing_context_sources:
            a.update(status="skipped", reason="already stacked")
        else:
            a["status"] = "pending"
    actions.append(a)

if launch:
    if live:
        actions.append({"step": "launch", "status": "skipped", "reason": "already live"})
    else:
        actions.append({"step": "launch", "status": "pending"})

if onboard:
    # A message is imperative, not converging state: it sends on every
    # run that asks for it.
    actions.append({"step": "onboard", "status": "pending"})

if take_app:
    # 0.46.5: the App's handover to this agent, filed only after the hire
    # succeeded (never inline; a failed hire files nothing).
    actions.append({"step": "take-app", "status": "pending", "app": take_app})

display_name = name or (conf.get("name") if registered else basename) or basename

def emit(ok, changed):
    out = {"ok": ok, "changed": changed, "name": display_name, "path": path,
           "actions": actions}
    if dry_run:
        out["dryRun"] = True
    if as_json:
        print(json.dumps(out))
        return
    if dry_run:
        print("DRY RUN — no changes made. Plan for %s (%s):" % (display_name, path))
    for a in actions:
        step, status = a["step"], a["status"]
        if step == "create-dir":
            line = "create-dir: %s" % path
        elif step == "write-persona":
            if "template" in a:
                line = "write-persona: template '%s'" % a["template"]
            elif a.get("source") == "default-scaffold":
                line = "write-persona: default scaffold"
            else:
                line = "write-persona: from %s" % a.get("source")
        elif step == "register-workspace":
            line = "register-workspace"
            if a.get("wiki") == "skip":
                line += " (no wiki seed)"
            elif a.get("wiki") == "seed":
                line += " (seed wiki)"
        elif step == "set-mode":
            line = "mode: %s" % a["mode"]
        elif step == "set-enabled":
            line = "enabled: %s" % ("true" if a["enabled"] else "false")
        elif step == "set-name":
            line = "name: %s" % a["name"]
        elif step == "set-default-agent":
            line = "default-agent: %s (%s)" % (a["label"], a["preset"])
        elif step == "set-db-access":
            line = "db-access: %s" % a["access"]
        elif step == "connect":
            line = "connect: %s (bidirectional)" % a["peer"]
            if a.get("warn"):
                line += "  (warn: %s)" % a["warn"]
        elif step == "join-project":
            line = "join-project: %s" % a["project"]
            if a.get("becamePoc"):
                line += " (first member — Point of Contact)"
        elif step == "set-color":
            line = "color: %s" % a["color"]
        elif step == "assign-focus-group":
            line = "focus-group: %s" % (a.get("group") or "none")
        elif step == "seed-context":
            if a.get("kind") == "catalog":
                line = "context: catalog %s" % a.get("catalog", a.get("context"))
            else:
                line = "context: %s" % (a.get("path") or a.get("context"))
            if a.get("warn"):
                line += "  (warn: %s)" % a["warn"]
        elif step == "launch":
            line = "launch"
            if a.get("sessionId"):
                line += " (session %s)" % str(a["sessionId"])[:8]
        elif step == "apply-template":
            t = a.get("info") or {}
            line = "template: %s" % a["template"]
            if t.get("id"):
                line += " (%s v%s)" % (t.get("id"), t.get("version"))
        elif step == "hello":
            line = "hello: the agent says hello on its Thread"
        elif step == "take-app":
            line = "take-app: hand %s to this agent" % a.get("app")
            h = a.get("handover") or {}
            if h.get("id"):
                line += " (handover %s %s%s)" % (h["id"], h.get("state"), (", card " + h["request"]) if h.get("request") else "")
        else:
            line = "onboard: deliver first message"
        if status in ("skipped", "not-run") and a.get("reason"):
            line += "  (%s)" % a["reason"]
        if status == "failed" and a.get("error"):
            line += "  (%s: %s)" % (a["error"].get("code"), a["error"].get("hint"))
        marker = {"pending": "would apply", "done": "done", "skipped": "skipped",
                  "failed": "FAILED", "not-run": "not run"}[status]
        print("  %-14s %s" % ("[" + marker + "]", line))
        if step == "apply-template":
            # The daemon's own rows (folder and register are the steps above).
            words = {"write": "would write", "done": "wrote", "skip": "skip", "yours": "yours, kept",
                     "suggest": "suggested"}
            for r in a.get("rows") or []:
                if r.get("step") in ("folder", "register"):
                    continue
                txt = "%s %s" % (r.get("step"), r.get("target"))
                if r.get("reason"):
                    txt += "  (%s)" % r["reason"]
                print("      %-12s %s" % (words.get(r.get("status"), r.get("status")), txt))
            if a.get("permissionsNote"):
                print("      %s" % a["permissionsNote"])
            # First-conversation questions, when this call writes ROLE.md
            # (a re-run writes nothing, so it has nothing to say).
            qs = a.get("questions") or []
            writes_role = any(r.get("step") == "role" and r.get("status") in ("write", "done") for r in a.get("rows") or [])
            if qs and writes_role:
                got = [q["id"] for q in qs if q.get("answered")]
                left = [q for q in qs if not q.get("answered")]
                if got:
                    print("      %-12s %s" % ("answered", ", ".join(got)))
                if left:
                    print("      %-12s %s  (the agent asks its person on its first start)" % ("open", ", ".join(q["id"] for q in left)))
                else:
                    print("      %-12s none: every question was answered" % "open")
    if not dry_run:
        n = sum(1 for a in actions if a["status"] == "done")
        print("%s: %s" % (display_name,
                          ("%d change(s) applied." % n) if n else "no changes — already converged."))
    nf = sum(1 for a in actions if a["status"] == "failed")
    if nf:
        print("%s: %d step(s) %s." % (display_name, nf, "would fail" if dry_run else "failed"))

def exit_code_for(code):
    # Align with agent-family exits when possible.
    c = (code or "").lower()
    if c in ("bad_usage", "path_escape", "catalog_unknown", "usage"):
        return 2
    if c in ("duplicate_layer", "refused", "precondition", "owner_only"):
        return 3
    if c in ("not_found", "ambiguous"):
        return 4
    return 1

def finish(changed):
    """Report every step, then exit non-zero when any step failed (the
    first failure's code and exit; the hint names every failed step)."""
    failed = [a for a in actions if a["status"] == "failed"]
    emit(not failed, changed)
    if failed:
        first = failed[0]["error"]
        what = "; ".join("%s %s: %s" % (a["step"], a.get("peer") or a.get("project") or a.get("context") or a.get("name") or "",
                                        a["error"].get("hint")) for a in failed)
        fail(first.get("code") or "daemon_error",
             "%d step(s) %s: %s" % (len(failed), "would fail" if dry_run else "failed", what),
             failed[0].get("exitCode") or exit_code_for(first.get("code")))
    sys.exit(0)

TEMPLATE_PERMS_NOTE = ("suggested switches are not turned on by this K2: a person turns them on "
                       "(Settings > Workspaces > the agent), or the agent asks on a card")

def take_template_rows(a, resp, applied):
    """Attach the daemon's template rows to the apply-template action."""
    a["rows"] = resp.get("rows") or []
    a["info"] = resp.get("template") or {}
    a["permissions"] = resp.get("permissions") or {}
    a["questions"] = resp.get("questions") or []
    if (a["permissions"].get("suggested") or a["permissions"].get("cards")) and not (a["permissions"].get("granted")):
        a["permissionsNote"] = TEMPLATE_PERMS_NOTE
    busy = [r for r in a["rows"] if r.get("status") in ("write", "done") and r.get("step") not in ("folder", "register")]
    if applied:
        a["status"] = "done" if busy else "skipped"
    else:
        a["status"] = "pending" if busy else "skipped"
    if a["status"] == "skipped":
        a["reason"] = "already applied"

if dry_run:
    ta_ = next((x for x in actions if x["step"] == "apply-template"), None)
    if ta_:
        pb = template_body({"path": path, "dryRun": True})
        if resolved_preset:
            pb["preset"] = resolved_preset.get("id")
        resp = http_post_json("/cli/agents/hire", pb, tok=core_token)
        if resp.get("error") or not resp.get("ok"):
            code, hint = core_error(resp)
            ta_["status"] = "failed"
            ta_["error"] = {"code": code, "hint": hint}
            ta_["exitCode"] = template_exit(code)
        else:
            take_template_rows(ta_, resp, applied=False)
    finish(any(a["status"] == "pending" for a in actions))

changed = False

def step_failed(action, code, hint, exit_code=None, fatal=False):
    """Mark one step failed and go on with the independent steps (0.46.4:
    a failed connect no longer stops --context, launch …). `fatal` (the
    workspace itself couldn't be made) stops here: what's left is not run."""
    action["status"] = "failed"
    action["error"] = {"code": code, "hint": hint}
    if exit_code is not None:
        action["exitCode"] = exit_code
    if fatal:
        for a in actions:
            if a["status"] == "pending":
                a["status"] = "not-run"
                a["reason"] = "the workspace step failed"
        emit(False, changed)
        fail(code, hint, exit_code if exit_code is not None else exit_code_for(code))

by_step = {}
for a in actions:
    by_step.setdefault(a["step"], []).append(a)

def one(step):
    lst = by_step.get(step) or [None]
    return lst[0]

# 1-3. create-dir + write-persona + register-workspace (+ DB access) on the
#      daemon's shared hire core (POST /cli/agents/hire, 0.45.6). Inside a
#      K2 cell it carries the session passport, so the daemon refuses an
#      agent raising DB access (the 0.44.4 check, now daemon-side).
core_steps = ("create-dir", "write-persona", "register-workspace", "set-db-access")
pending_core = [one(s) for s in core_steps if one(s) and one(s)["status"] == "pending"]
tpl_action = one("apply-template")
template_hello = None
if pending_core or tpl_action:
    if tpl_action:
        # AT1: the id and the params; the daemon writes the persona.
        core_body = template_body({"path": path})
        if resolved_preset:
            core_body["preset"] = resolved_preset.get("id")
    else:
        core_body = {"path": path, "noWiki": no_wiki, "noAgentsMd": no_agents_md,
                     "fanout": fanout, "role": ""}
    wp = one("write-persona")
    if persona_content is not None and wp and wp["status"] == "pending":
        core_body["personaRaw"] = persona_content
    dba_core = one("set-db-access")
    if dba_core and dba_core["status"] == "pending":
        core_body["dbAccess"] = dba_core["access"]
    if take_app:
        core_body["takeApp"] = take_app
        if take_brief:
            core_body["brief"] = take_brief
    resp = http_post_json("/cli/agents/hire", core_body, tok=core_token)
    err = resp.get("error")
    if err or not resp.get("ok"):
        code = err.get("code") if isinstance(err, dict) else "daemon_error"
        hint = err.get("hint") if isinstance(err, dict) else (err or json.dumps(resp)[:300])
        bad = dba_core if (code == "owner_only" and dba_core and dba_core["status"] == "pending") else (pending_core[0] if pending_core else tpl_action)
        # Fatal: every later step needs the workspace.
        exit_for = 3 if code == "owner_only" else (template_exit(code) if tpl_action else None)
        step_failed(bad, code or "daemon_error", hint, exit_for, fatal=True)
    by_core = {x.get("step"): x for x in resp.get("steps", [])}
    if tpl_action:
        take_template_rows(tpl_action, resp, applied=True)
        changed = changed or tpl_action["status"] == "done"
        template_hello = resp.get("hello")
    ta = one("take-app")
    if ta and isinstance(resp.get("handover"), dict):
        ta["handover"] = resp["handover"]
        herr = resp["handover"].get("error")
        if isinstance(herr, dict):
            # The hire itself succeeded; only the offer failed (reported, not fatal).
            step_failed(ta, herr.get("code") or "daemon_error", herr.get("hint") or "the handover was not filed")
        else:
            ta["status"] = "done"
    for name in core_steps:
        a = one(name)
        if a and a["status"] == "pending":
            got = by_core.get(name) or {}
            a["status"] = got.get("status") or "done"
            if got.get("reason"):
                a["reason"] = got["reason"]
            changed = changed or a["status"] == "done"
    ra = one("register-workspace")
    if ra and ra["status"] == "done":
        ga = one("generate-agents-md")
        if ga and ga["status"] == "pending":
            agents_path = os.path.join(path, "AGENTS.md")
            body = ""
            if os.path.isfile(agents_path) and not os.path.islink(agents_path):
                try:
                    with open(agents_path, encoding="utf-8", errors="replace") as fh:
                        body = fh.read()
                except OSError:
                    body = ""
            if os.path.islink(agents_path) or "GENERATED by K2" in body:
                ga["status"] = "done"
            else:
                ga["status"] = "skipped"
                ga["reason"] = "user AGENTS.md present"
        fa = one("fanout")
        if fa and fa["status"] == "pending":
            fa["status"] = "done"

# 4. set-mode + set-enabled + set-default-agent in ONE
#    /cli/workspace/set batch (the route applies agent_mode first so an
#    explicit enabled wins the coupling; default_agent is order-neutral).
fields = {}
ma = one("set-mode")
if ma and ma["status"] == "pending":
    fields["agent_mode"] = ma["mode"]
ea = one("set-enabled")
if ea and ea["status"] == "pending":
    fields["agent_enabled"] = "1" if ea["enabled"] else "0"
da = one("set-default-agent")
if da and da["status"] == "pending":
    fields["default_agent"] = da["preset"]
dba = one("set-db-access")
if dba and dba["status"] == "pending":
    fields["db_agent_access"] = dba["access"]
if fields:
    resp = http_post_json("/cli/workspace/set", {"project": path, "fields": fields})
    if resp.get("error") or not resp.get("ok"):
        err = resp.get("error")
        hint = err if isinstance(err, str) else (err or {}).get("hint") or json.dumps(resp)[:300]
        # One batch: every field in it failed together.
        for x in (ma, ea, da, dba):
            if x and x["status"] == "pending":
                step_failed(x, "daemon_error", "settings write failed: %s" % hint)
    by_field = {x.get("field"): x.get("status") for x in resp.get("actions", [])}
    if ma and ma["status"] == "pending":
        ma["status"] = by_field.get("agent_mode", "done")
        if ma["status"] == "skipped":
            ma["reason"] = "already %s" % ma["mode"]
        changed = changed or ma["status"] == "done"
    if ea and ea["status"] == "pending":
        ea["status"] = by_field.get("agent_enabled", "done")
        if ea["status"] == "skipped":
            ea["reason"] = "already set"
        changed = changed or ea["status"] == "done"
    if da and da["status"] == "pending":
        da["status"] = by_field.get("default_agent", "done")
        if da["status"] == "skipped":
            da["reason"] = "already set"
        changed = changed or da["status"] == "done"
    if dba and dba["status"] == "pending":
        dba["status"] = by_field.get("db_agent_access", "done")
        if dba["status"] == "skipped":
            dba["reason"] = "already set"
        changed = changed or dba["status"] == "done"

# 5. set-name (writes BOTH name stores: AGENT.md display_name +
#    projects.name — addressing follows the rename).
a = one("set-name")
if a and a["status"] == "pending":
    resp = http_post_query("/cli/workspace/set-agent-display-name",
                    {"project_path": path, "name": a["name"]})
    if resp.get("error") or not resp.get("success"):
        step_failed(a, "daemon_error",
                    "rename failed: %s" % resp.get("error", json.dumps(resp)[:300]))
    else:
        a["status"] = "done"
        changed = True

# 5b. color + focus group (need workspace id after register).
ca = one("set-color")
fa = one("assign-focus-group")
need_id = (ca and ca["status"] == "pending") or (fa and fa["status"] == "pending")
workspace_id = conf.get("id") if registered else None
if need_id and not workspace_id:
    conf2 = http_get("/cli/agent/conf", {"q": path})
    if conf2.get("ok") and conf2.get("id"):
        workspace_id = conf2.get("id")
        conf = conf2
if need_id and not workspace_id:
    listing = http_get("/cli/projects/list", {})
    rows = listing if isinstance(listing, list) else []
    for p in rows:
        if isinstance(p, dict) and p.get("path") == path and p.get("id"):
            workspace_id = p.get("id")
            break
if need_id and not workspace_id:
    for x in (ca, fa):
        if x and x["status"] == "pending":
            step_failed(x, "daemon_error", "cannot resolve workspace id after register")

a = one("set-color")
if a and a["status"] == "pending":
    resp = http_post_json("/cli/projects/update", {"id": workspace_id, "color": a["color"]})
    if resp.get("error"):
        err = resp.get("error")
        hint = err if isinstance(err, str) else (err or {}).get("hint") or json.dumps(resp)[:300]
        step_failed(a, "daemon_error", "color write failed: %s" % hint)
    else:
        a["status"] = "done"
        changed = True

a = one("assign-focus-group")
if a and a["status"] == "pending":
    body = {"projectId": workspace_id, "focusGroupId": a.get("focusGroupId")}
    resp = http_post_json("/cli/focus-groups/assign", body)
    if resp.get("error"):
        err = resp.get("error")
        hint = err if isinstance(err, str) else (err or {}).get("hint") or json.dumps(resp)[:300]
        step_failed(a, "daemon_error", "focus-group assign failed: %s" % hint)
    else:
        a["status"] = "done"
        changed = True

# 6. connect edges. ONE add per peer: the connections substrate stores
#    a single row per pair and that row IS mutual awareness (migration
#    0051; the add-side reverse-pair guard no-ops redundant adds), so
#    there is no second direction to wire.
for a in by_step.get("connect", []):
    if a["status"] != "pending":
        continue
    # Resolved at plan time by the daemon (a local path or agent::host);
    # an unresolvable target is already `failed` and never reaches here.
    peer = a["peer"]
    resp = http_post_query("/cli/connections",
                    {"action": "add", "project": path, "target": a.get("resolved") or peer})
    err = resp.get("error")
    if err:
        code = err.get("code") if isinstance(err, dict) else None
        hint = err.get("hint") if isinstance(err, dict) else str(err)
        step_failed(a, code or "daemon_error", "connect %s failed: %s" % (peer, hint),
                    3 if code == "agents_create_connections_disabled" else None)
        continue
    if resp.get("noop"):
        a["status"] = "skipped"
        a["reason"] = "edge exists"
        continue
    for k in ("bindWarning", "warning"):
        if resp.get(k):
            a["warn"] = resp[k]
    a["status"] = "done"
    changed = True

# 6b. join projects. add-member is idempotent daemon-side; the first
#     member of an empty project auto-becomes the PoC (becamePoc).
for a in by_step.get("join-project", []):
    if a["status"] != "pending":
        continue
    resp = http_post_json("/cli/project-group/add-member",
                          {"group": a["projectId"], "workspace": path}, tok=proj_token)
    err = resp.get("error")
    if err or not resp.get("ok"):
        hint = err if isinstance(err, str) else (err or {}).get("hint") or json.dumps(resp)[:300]
        step_failed(a, "daemon_error", "join project %s failed: %s" % (a["project"], hint))
        continue
    if resp.get("alreadyMember"):
        a["status"] = "skipped"
        a["reason"] = "already a member"
        a.pop("becamePoc", None)
    else:
        a["status"] = "done"
        if resp.get("becamePoc"):
            a["becamePoc"] = True
        else:
            a.pop("becamePoc", None)
        changed = True

# 6c. seed context layers (after workspace is open/registered).
#     Explicit paths already validated at plan time. Catalog missing file
#     → warn on stderr and continue. Duplicate → skipped (idempotent).
for a in by_step.get("seed-context", []):
    if a["status"] != "pending":
        continue
    body = {"project": path}
    if a.get("kind") == "catalog":
        body["catalog"] = a.get("catalog") or a.get("context")
    else:
        body["path"] = a.get("path") or a.get("context")
    resp = http_post_json("/cli/context/add", body)
    err = resp.get("error")
    if err:
        if isinstance(err, str):
            err = {"code": "error", "hint": err}
        code = (err.get("code") or "error").lower()
        hint = err.get("hint") or err.get("message") or code
        if code == "duplicate_layer":
            a["status"] = "skipped"
            a["reason"] = "already stacked"
            continue
        if code == "catalog_unknown":
            step_failed(a, "catalog_unknown",
                        "unknown context catalog id '%s': %s — see: k2 agent context catalog"
                        % (a.get("context"), hint))
            continue
        if a.get("kind") == "path":
            step_failed(a, code if code in ("not_found", "path_escape", "bad_usage") else "daemon_error",
                        "context seed failed for %s: %s" % (a.get("context"), hint))
            continue
        # Catalog soft-fail: warn + skip (do not abort hire).
        sys.stderr.write("warning: context seed skipped for catalog id '%s': %s\n"
                         % (a.get("context"), hint))
        a["status"] = "skipped"
        a["reason"] = hint
        a["warn"] = hint
        continue
    if not resp.get("ok") and "layer" not in resp:
        hint = json.dumps(resp)[:300]
        if a.get("kind") == "path":
            step_failed(a, "daemon_error", "context seed failed: %s" % hint)
            continue
        sys.stderr.write("warning: context seed unexpected response for '%s': %s\n"
                         % (a.get("context"), hint))
        a["status"] = "skipped"
        a["reason"] = "unexpected response"
        continue
    layer = resp.get("layer") or {}
    a["layerId"] = layer.get("id")
    a["path"] = layer.get("path") or a.get("path")
    exists = layer.get("exists")
    if a.get("kind") == "catalog" and exists is False:
        warn = "catalog pack file missing on disk: %s (layer stacked; fill file or regen later)" % (
            layer.get("path") or a.get("context"))
        sys.stderr.write("warning: %s\n" % warn)
        a["warn"] = warn
    a["status"] = "done"
    changed = True

# 7. launch
a = one("launch")
if a and a["status"] == "pending":
    resp = http_post_query("/cli/workspace/ensure-canonical-session", {"project": path},
                    timeout=120)
    if resp.get("error") or not resp.get("success"):
        step_failed(a, "daemon_error",
                    "launch failed: %s" % resp.get("error", json.dumps(resp)[:300]))
    else:
        a["status"] = "done"
        a["sessionId"] = resp.get("session_id")
        if resp.get("reused"):
            a["reused"] = True
        else:
            changed = True

# 7b. hello (0.46.12, prd-agent-templates-v1 §3): a template hire's first
#     create returns the template's hello; once the agent runs, it goes in
#     as the agent's first message (before --onboard). Without --launch it
#     isn't sent: hire never starts an agent you didn't ask to start.
if template_hello:
    ha = {"step": "hello", "status": "pending"}
    la = one("launch")
    if not la:
        ha.update(status="skipped", reason="not launched; pass --launch to have it say hello")
    elif la["status"] == "failed":
        ha.update(status="not-run", reason="launch failed")
    else:
        resp = http_post_query("/cli/workspace/msg",
                        {"workspace": path, "text": template_hello, "from": "owner",
                         "wake": "true", "wake_timeout": "120"},
                        timeout=180)
        if (isinstance(resp.get("error"), str) and resp["error"]) or not resp.get("success"):
            step_failed(ha, "daemon_error", "hello delivery failed: %s" % (resp.get("error") or resp.get("message") or json.dumps(resp)[:300]))
        else:
            ha["status"] = "done"
            changed = True
    at = next((i for i, x in enumerate(actions) if x["step"] in ("onboard", "take-app")), len(actions))
    actions.insert(at, ha)

# 8. onboard (delivered via the same throat `k2 talk` uses —
#    /cli/workspace/msg with wake, which waits for READY first). It needs
#    the launch: a failed launch leaves it not run.
a = one("onboard")
la = one("launch")
if a and a["status"] == "pending" and la and la["status"] == "failed":
    a["status"] = "not-run"
    a["reason"] = "launch failed"
elif a and a["status"] == "pending":
    resp = http_post_query("/cli/workspace/msg",
                    {"workspace": path, "text": onboard, "from": "owner",
                     "wake": "true", "wake_timeout": "120"},
                    timeout=180)
    if isinstance(resp.get("error"), str) and resp["error"]:
        step_failed(a, "daemon_error", "onboard delivery failed: %s" % resp["error"])
    elif not resp.get("success"):
        step_failed(a, "daemon_error",
                    "onboard delivery failed: %s" % (resp.get("message") or json.dumps(resp)[:300]))
    else:
        a["status"] = "done"
        changed = True

# 9. take-app on an agent that already existed (the core didn't run):
#    an ordinary offer (that agent accepts) or, from the owner, the card.
a = one("take-app")
if a and a["status"] == "pending":
    body = {"app": take_app, "to": path}
    if take_brief:
        body["brief"] = take_brief
    resp = http_post_json("/cli/app/handover", body, tok=core_token)
    if isinstance(resp.get("error"), dict):
        a["handover"] = {"error": resp["error"]}
        step_failed(a, resp["error"].get("code") or "daemon_error", resp["error"].get("hint") or "the handover was not filed")
    else:
        h = resp.get("handover") or {}
        r = resp.get("request")
        a["handover"] = {"id": h.get("id"), "state": h.get("state"), "request": r.get("id") if isinstance(r, dict) else None}
        a["status"] = "done"
    changed = True

# Every step's result; non-zero when any failed.
finish(changed)
PYEOF
}

# `k2 agent list` — S3. Reclaims the previously-deprecated arm with the
# real fleet-view semantics (PRD §3): every registered agent with mode,
# enabled, live-session status (same source as `conf`'s live), and path.
cmd_agent_list() {
    if _wants_help "$@"; then cmd_help_agent_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _agent_err "bad_usage" "unexpected argument '$1' — usage: k2 agent list [--json]"
                exit 2 ;;
        esac
    done
    local resp rc
    set +e
    resp=$(cli_request "/cli/agent/list")
    rc=$?
    set -e
    if [ -z "$resp" ]; then
        if [ "$rc" -eq "$_CLI_LOCAL_TRANSPORT_DENIED" ]; then
            _agent_err "local_transport_denied" "local restriction, daemon health unobserved"
        else
            _agent_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        fi
        exit 1
    fi
    RESP="$resp" AS_JSON="$as_json" python3 - <<'PYEOF'
import json, os, sys
raw = os.environ.get("RESP", "")
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write(json.dumps({"error": {"code": "bad_response",
        "hint": "daemon returned an unparseable response: %r" % raw[:200]}}) + "\n")
    sys.exit(1)
if d.get("error") or not d.get("ok"):
    err = d.get("error")
    if not isinstance(err, dict):
        err = {"code": "error", "hint": str(err or raw[:200])}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    sys.exit(1)
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
    sys.exit(0)

agents = d.get("agents") or []
if not agents:
    print("No agents registered on this server. Hire one: k2 agent hire <dir>")
    sys.exit(0)

home = os.path.expanduser("~")
def tilde(p):
    return "~" + p[len(home):] if p.startswith(home + "/") or p == home else p

rows = [("NAME", "MODE", "ENABLED", "LIVE", "PATH")]
for a in agents:
    rows.append((
        str(a.get("name", "?")),
        str(a.get("mode", "?")),
        "yes" if a.get("enabled") else "no",
        "yes" if a.get("live") else "no",
        tilde(str(a.get("path", "?"))),
    ))
widths = [max(len(r[i]) for r in rows) for i in range(4)]
for r in rows:
    print("%-*s  %-*s  %-*s  %-*s  %s" % (
        widths[0], r[0], widths[1], r[1], widths[2], r[2], widths[3], r[3], r[4]))
PYEOF
}

# `k2 agent retire <name>` — S4. Safe decommission via
# POST /cli/agent/retire: guards REFUSE (exit 3) on uncommitted/unpushed
# git work or likely secrets; --force overrides; --dry-run previews
# (mirroring the real verdict, including the exit code). On success the
# folder is ARCHIVED (never deleted), the registration + every
# connection edge + non-cascaded dependent rows are removed.
cmd_agent_retire() {
    if _wants_help "$@"; then cmd_help_agent_retire; exit 0; fi
    local name="" archive_to="" force=false dry_run=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --archive-to)
                [ $# -ge 2 ] || { _agent_err "bad_usage" "--archive-to requires a directory"; exit 2; }
                archive_to="$2"; shift 2 ;;
            --force) force=true; shift ;;
            --dry-run) dry_run=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent retire <name> [--archive-to <dir>] [--force] [--dry-run] [--json]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent retire <name> [flags]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        _agent_err "bad_usage" "missing <name> — usage: k2 agent retire <name> [flags]"
        exit 2
    fi
    # Resolve (name | path) with the standard exit-4 + did-you-mean.
    local conf rc=0
    conf=$(_agent_conf_fetch "$name") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    CONF="$conf" ARCHIVE_TO="$archive_to" FORCE="$force" DRY_RUN="$dry_run" \
    AS_JSON="$as_json" ABASE="$BASE_URL" ATOKEN="$TOKEN" python3 - <<'PYEOF'
import json, os, sys
import urllib.error, urllib.parse, urllib.request

conf = json.loads(os.environ["CONF"])
archive_to = os.environ.get("ARCHIVE_TO", "")
if archive_to:
    archive_to = os.path.abspath(os.path.expanduser(archive_to))
force = os.environ.get("FORCE") == "true"
dry_run = os.environ.get("DRY_RUN") == "true"
as_json = os.environ.get("AS_JSON") == "true"
base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
path = conf["path"]
label = conf.get("name") or os.path.basename(path)

url = base + "/cli/agent/retire?token=" + urllib.parse.quote(token, safe="")
body = json.dumps({"q": path, "force": force, "dryRun": dry_run,
                   "archiveTo": archive_to}).encode()
req = urllib.request.Request(url, data=body,
                             headers={"Content-Type": "application/json"}, method="POST")
try:
    with urllib.request.urlopen(req, timeout=120) as r:
        d = json.loads(r.read().decode("utf-8", "replace"))
except urllib.error.HTTPError as e:
    try:
        d = json.loads(e.read().decode("utf-8", "replace"))
    except Exception:
        sys.stderr.write(json.dumps({"error": {"code": "daemon_error",
            "hint": "HTTP %s on /cli/agent/retire" % e.code}}) + "\n")
        sys.exit(1)
except Exception as e:
    sys.stderr.write(json.dumps({"error": {"code": "daemon_unreachable",
        "hint": "no response from the K2 daemon: %s" % e}}) + "\n")
    sys.exit(1)

err = d.get("error")
if d.get("refused"):
    # Safety guard tripped — the contract's exit 3, never a prompt.
    hint = (err or {}).get("hint") or "retire refused by a safety guard"
    if as_json:
        sys.stderr.write(json.dumps(d) + "\n")
    else:
        sys.stderr.write("REFUSED: %s\n" % hint)
        if d.get("dryRun"):
            sys.stderr.write("(dry-run: this is the verdict a real run would produce)\n")
    sys.exit(3)
if err:
    if isinstance(err, str):
        err = {"code": "daemon_error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    sys.exit(4 if err.get("code") in ("not_found", "ambiguous") else 1)
if not d.get("ok"):
    sys.stderr.write(json.dumps({"error": {"code": "daemon_error",
        "hint": "unexpected response: %s" % json.dumps(d)[:300]}}) + "\n")
    sys.exit(1)

if as_json:
    print(json.dumps(d))
    sys.exit(0)

if d.get("dryRun"):
    print("DRY RUN — no changes made. Plan for retiring %s (%s):" % (label, path))
for a in d.get("actions", []):
    step, status = a.get("step", "?"), a.get("status", "?")
    detail = ""
    if step == "stop-session" and a.get("sessionId"):
        detail = " (session %s)" % str(a["sessionId"])[:8]
    elif step == "remove-connections" and a.get("removed") is not None:
        detail = " (%s edge(s))" % a["removed"]
    elif step == "remove-connections" and a.get("edges") is not None:
        detail = " (%s edge(s))" % a["edges"]
    elif step == "clean-dependents" and a.get("chatSessionNames") is not None:
        detail = " (%s chat-session row(s))" % a["chatSessionNames"]
    elif step == "archive-folder" and a.get("to"):
        detail = " → %s" % a["to"]
    if status == "skipped" and a.get("reason"):
        detail += "  (%s)" % a["reason"]
    marker = {"pending": "would apply", "done": "done", "skipped": "skipped",
              "failed": "FAILED"}.get(status, status)
    print("  %-14s %s%s" % ("[" + marker + "]", step, detail))
if not d.get("dryRun"):
    dest = d.get("archivedTo")
    if dest:
        print("%s retired — folder archived to %s (nothing was deleted)." % (label, dest))
    else:
        print("%s retired." % label)
PYEOF
}

# `k2 agent restart <name>` (GH #17) — bounce the agent's pinned chat
# so it reloads its charter / .mcp.json. One daemon door: POST
# /cli/workspace/ensure-pinned-chat with forceRespawn, which kills the
# live canonical PTY and respawns it on the same provider conversation.
# The old session id comes from `agent conf` (live.sessionId).
cmd_agent_restart() {
    if _wants_help "$@"; then cmd_help_agent_restart; exit 0; fi
    local name="" dry_run=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run) dry_run=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _agent_err "bad_usage" "unknown flag '$1' — usage: k2 agent restart <name> [--dry-run] [--json]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; else
                    _agent_err "bad_usage" "unexpected extra argument '$1' — usage: k2 agent restart <name> [flags]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        _agent_err "bad_usage" "missing <name> — usage: k2 agent restart <name> [flags]"
        exit 2
    fi
    local conf rc=0
    conf=$(_agent_conf_fetch "$name") || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    CONF="$conf" DRY_RUN="$dry_run" AS_JSON="$as_json" ABASE="$BASE_URL" ATOKEN="$TOKEN" \
        python3 - <<'PYEOF'
import json, os, sys
import urllib.error, urllib.parse, urllib.request

conf = json.loads(os.environ["CONF"])
dry_run = os.environ.get("DRY_RUN") == "true"
as_json = os.environ.get("AS_JSON") == "true"
path = conf["path"]
label = conf.get("name") or os.path.basename(path)
live = conf.get("live") or {}
old_id = live.get("sessionId") if live.get("active") else None

if dry_run:
    out = {"ok": True, "dryRun": True, "name": label, "path": path,
           "oldSessionId": old_id,
           "actions": [{"step": "restart-session", "status": "pending",
                        "sessionId": old_id}]}
    if as_json:
        print(json.dumps(out))
    elif old_id:
        print("DRY RUN — would restart %s (session %s) on the same conversation." % (label, old_id))
    else:
        print("DRY RUN — %s has no live session; restart would start one." % label)
    sys.exit(0)

url = os.environ["ABASE"] + "/cli/workspace/ensure-pinned-chat?token=" + \
    urllib.parse.quote(os.environ["ATOKEN"], safe="")
body = json.dumps({"project": path, "forceRespawn": True}).encode()
req = urllib.request.Request(url, data=body,
                             headers={"Content-Type": "application/json"}, method="POST")
try:
    with urllib.request.urlopen(req, timeout=120) as r:
        d = json.loads(r.read().decode("utf-8", "replace"))
except urllib.error.HTTPError as e:
    raw = e.read().decode("utf-8", "replace")
    try:
        msg = json.loads(raw).get("error")
    except Exception:
        msg = None
    if isinstance(msg, dict):
        msg = msg.get("hint") or msg.get("message")
    sys.stderr.write(json.dumps({"error": {"code": "daemon_error",
        "hint": "restart failed (HTTP %s): %s" % (e.code, msg or raw[:300])}}) + "\n")
    sys.exit(1)
except Exception as e:
    sys.stderr.write(json.dumps({"error": {"code": "daemon_unreachable",
        "hint": "no response from the K2 daemon: %s" % e}}) + "\n")
    sys.exit(1)

new_id = d.get("sessionId")
if not new_id:
    sys.stderr.write(json.dumps({"error": {"code": "daemon_error",
        "hint": "unexpected response: %s" % json.dumps(d)[:300]}}) + "\n")
    sys.exit(1)
conversation = d.get("claudeSessionId") or None
out = {"ok": True, "changed": True, "name": label, "path": path,
       "oldSessionId": old_id, "sessionId": new_id,
       "conversationId": conversation,
       "resumedExisting": bool(d.get("resumedExisting")),
       "provider": d.get("provider")}
if as_json:
    print(json.dumps(out))
    sys.exit(0)
if old_id:
    print("%s restarted: session %s → %s." % (label, old_id, new_id))
else:
    print("%s had no live session; started %s." % (label, new_id))
if conversation:
    how = "resumed" if out["resumedExisting"] else "new"
    print("Conversation %s (%s)." % (conversation, how))
PYEOF
}

