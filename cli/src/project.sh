# ── Projects V1 (P3) — `k2 project` command group ────────────────────
#
# `k2 project` posts to and manages this server's PROJECTS: named
# groups of agent workspaces with ONE shared chat stream and exactly
# one Point of Contact (PoC) agent (prd-projects-v1 §5; mockup:
# .k2/notes/project-cli-mockup.md — the help text below is that mockup
# VERBATIM; it passed the 36/36 comprehension gate). Thin client over
# the P2 daemon routes (/cli/project-group/*): the DAEMON owns all
# selector resolution (full-id | exact-name NOCASE beats prefix |
# unique name prefix) — the CLI never re-implements it.
#
# Contract highlights (pinned):
# - Success output human-readable; --json flips it to JSON.
# - ERRORS are ALWAYS JSON on stderr ({"error":{"code","hint"}})
#   regardless of --json.
# - Exit codes: 0 ok · 1 resolved-but-refused-or-failed
#   (poc_successor_required, not_a_member, name_taken) · 2 bad usage ·
#   4 name unresolvable (not_found / ambiguous / no_project /
#   ambiguous_project — fix the name and retry).
# - `msg` argument rules (exact): ONE positional arg = always <text>
#   (project auto-detected); TWO = the first is ALWAYS <project>;
#   THREE-PLUS = exit 2 with the quote-your-message hint. <text> of
#   "-" reads the body from stdin.
# - "delivered": false + "deliveryReason": "author-is-poc" is a NORMAL
#   success (the PoC's own posts never self-inject), NOT a failure.

_project_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

# Shared HTTP driver for every project verb. Inputs ride P_* variables
# (visible here via bash dynamic scope from the calling cmd_project_*),
# re-exported onto the python3 invocation. Sender identity is the
# workspace auto-detected at the top of this script (K2_PROJECT_PATH →
# cwd walk-up — the same detection `k2 tickets` uses).
_project_py() {
    ABASE="$BASE_URL" ATOKEN="$TOKEN" P_WS_PATH="$PROJECT" \
    P_AGENT_NAME="${K2_AGENT_NAME:-${K2SO_AGENT_NAME:-}}" \
    P_VERB="${P_VERB:-}" P_JSON="${P_JSON:-false}" \
    P_NAME="${P_NAME:-}" P_ARG_PROJECT="${P_ARG_PROJECT:-}" \
    P_ARG_WORKSPACE="${P_ARG_WORKSPACE:-}" P_TEXT="${P_TEXT:-}" \
    P_MSG_FORM="${P_MSG_FORM:-}" P_TEXT_STDIN="${P_TEXT_STDIN:-false}" \
    P_AFTER="${P_AFTER:-}" P_LIMIT="${P_LIMIT:-}" P_VIEW="${P_VIEW:-}" \
    P_PERMISSION_NOTE="${_K2_PERMISSION_NOTE:-}" \
    python3 - <<'PYEOF'
import json, os, sys, time
import urllib.error, urllib.parse, urllib.request

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
verb = os.environ["P_VERB"]
as_json = os.environ.get("P_JSON") == "true"
ws_path = os.environ.get("P_WS_PATH", "")

# Exit-code contract (mockup): 4 = a name could not be resolved (fix
# the name and retry) · 2 = bad usage · 1 = resolved but refused or
# failed (poc_successor_required / not_a_member / name_taken / ...).
EXIT4 = ("not_found", "ambiguous", "ambiguous_id", "no_project", "ambiguous_project")

def dumps(obj):
    # Compact separators — the mockup's JSON shapes are compact
    # ({"ok":true,...} / {"error":{"code",...}}), byte-for-byte.
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))

# Permission cards (Beta): the last refusal's `permission` object, if the
# daemon sent one. die() hands its closing line ("Ask with: k2 permission
# request …" or "It can't be requested.") to the CLI's EXIT trap through
# the same note file the curl transports use (BEGIN_CLI_PERMISSION_NOTE),
# so it prints once, after the JSON error, and only on a failed exit.
LAST_PERMISSION = [None]

def permission_lines(p):
    # Same rules as _permission_closing_lines.
    if not isinstance(p, dict):
        return []
    cmd = p.get("command")
    if isinstance(cmd, str) and cmd:
        return ["Ask with: " + cmd]
    if p.get("requestable") is False:
        hint = p.get("hint")
        return ([hint] if isinstance(hint, str) and hint else []) + ["It can't be requested."]
    return []

def die(err):
    # Errors are ALWAYS JSON on stderr, regardless of --json.
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code", "error")
    if code == "ambiguous_id":
        # Daemon spelling → the mockup's stable code for an ambiguous
        # project prefix (hint passes through, candidates included).
        code = "ambiguous"
    sys.stderr.write(dumps({"error": {"code": code, "hint": err.get("hint", "")}}) + "\n")
    lines = permission_lines(LAST_PERMISSION[0])
    if lines:
        note = os.environ.get("P_PERMISSION_NOTE", "")
        try:
            if not note:
                raise OSError("no note file")
            with open(note, "w", encoding="utf-8") as f:
                f.write("\n".join(lines) + "\n")
        except Exception:
            sys.stderr.write("\n".join(lines) + "\n")
    sys.exit(4 if code in EXIT4 else 2 if code == "usage" else 1)

def request(method, route, params=None, body=None):
    qs = urllib.parse.urlencode(dict(params or {}, token=token))
    url = base + route + "?" + qs
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            v = json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": {"code": "error", "hint": "HTTP %s on %s" % (e.code, route)}}
        if isinstance(v, dict) and isinstance(v.get("permission"), dict):
            LAST_PERMISSION[0] = v["permission"]
        return v
    except Exception:
        return None

def call(method, route, params=None, body=None):
    d = request(method, route, params, body)
    if d is None:
        die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
    if d.get("error"):
        die(d["error"])
    if not d.get("ok"):
        die({"code": "error", "hint": "unexpected daemon response: %s" % dumps(d)[:200]})
    return d

def mname(m):
    # Member display name: agent display name → registry name → folder.
    return (m.get("agentName") or m.get("name")
            or os.path.basename((m.get("path") or "").rstrip("/")) or "?")

def member_by_id(show, workspace_id):
    for m in show.get("members", []):
        if m.get("workspaceId") == workspace_id:
            return m
    return None

def poc_member(show):
    pid = show.get("pocWorkspaceId")
    return member_by_id(show, pid) if pid else None

def poc_name(show):
    m = poc_member(show)
    return mname(m) if m else None

def fetch_show(selector):
    return call("GET", "/cli/project-group/show", params={"group": selector})

def same_path(a, b):
    try:
        return os.path.realpath(a) == os.path.realpath(b)
    except Exception:
        return a == b

def my_member(show):
    for m in show.get("members", []):
        p = m.get("path") or ""
        if p and ws_path and same_path(p, ws_path):
            return m
    return None

def detect_project():
    # The auto-detect rule (mockup NOTES #5, single source of truth):
    # the detected sender workspace's memberships — exactly one → use
    # it; zero → no_project exit 4; two-plus → ambiguous_project exit 4
    # with the candidate names in the hint. Exit 4 (not 2): the
    # invocation is well-formed, the project just can't be resolved.
    d = call("GET", "/cli/project-group/list")
    hits = []
    for g in d.get("groups", []):
        s = fetch_show(g.get("id"))
        if my_member(s) is not None:
            hits.append(s)
    if not hits:
        die({"code": "no_project",
             "hint": "this workspace is not a member of any project; pass the project name"})
    if len(hits) > 1:
        die({"code": "ambiguous_project",
             "hint": "member of: %s — pass the project name"
                     % ", ".join(h.get("name", "?") for h in hits)})
    return hits[0]

def ago(ts):
    s = int(time.time() - ts)
    if s < 60: return "just now"
    if s < 3600: return "%dm ago" % (s // 60)
    if s < 172800: return "%dh ago" % (s // 3600)
    return "%dd ago" % (s // 86400)

def home_path(p):
    home = os.path.expanduser("~")
    if p and p.startswith(home + "/"):
        return "~" + p[len(home):]
    return p or "?"

def print_message_rows(msgs):
    for m in msgs:
        lines = str(m.get("body", "")).splitlines() or [""]
        print("  %-9s%-11s%s" % (m.get("author", "?"),
                                 "(%s)" % ago(m.get("createdAt", 0)), lines[0]))
        for extra in lines[1:]:
            print(" " * 22 + extra)

if verb == "create":
    d = call("POST", "/cli/project-group/create", body={"name": os.environ["P_NAME"]})
    if as_json:
        print(dumps({"ok": True, "id": d.get("id"), "name": d.get("name"),
                     "members": int(d.get("memberCount") or 0), "poc": None}))
    else:
        print("Created project %s (no members yet)." % d.get("name"))
        print("Add the first member — it will become the PoC: k2 project add %s <workspace>"
              % d.get("name"))

elif verb == "list":
    d = call("GET", "/cli/project-group/list")
    rows = []
    for g in d.get("groups", []):
        pn = None
        if g.get("pocWorkspaceId"):
            pn = poc_name(fetch_show(g.get("id")))
        rows.append({"id": g.get("id"), "name": g.get("name") or "?",
                     "members": int(g.get("memberCount") or 0),
                     "poc": pn, "pinned": bool(g.get("pinned"))})
    if as_json:
        print(dumps({"ok": True, "projects": rows}))
    elif not rows:
        print("No projects yet. Create one: k2 project create <name>")
    else:
        w = max(max(len(r["name"]) for r in rows), len("NAME")) + 2
        print(("    %-" + str(w) + "s%-9s%s") % ("NAME", "MEMBERS", "POC"))
        for r in rows:
            line = ("%s%-" + str(w) + "s%-9s%s") % (
                " *  " if r["pinned"] else "    ", r["name"], r["members"], r["poc"] or "")
            print(line.rstrip())

elif verb == "show":
    show = fetch_show(os.environ["P_ARG_PROJECT"])
    recent = call("GET", "/cli/project-group/messages",
                  params={"group": show.get("id"), "limit": "3"})
    msgs = recent.get("messages", [])
    members = show.get("members", [])
    pid = show.get("pocWorkspaceId")
    pm = poc_member(show)
    if as_json:
        print(dumps({
            "ok": True, "id": show.get("id"), "name": show.get("name"),
            "pinned": bool(show.get("pinned")),
            "poc": ({"name": mname(pm), "path": pm.get("path")} if pm else None),
            "members": [{"name": mname(m), "path": m.get("path"),
                         "isPoc": m.get("workspaceId") == pid} for m in members],
            "recent": [{"id": m.get("id"), "author": m.get("author"),
                        "body": m.get("body"), "at": m.get("createdAt")} for m in msgs],
        }))
    else:
        name = show.get("name") or "?"
        print("project : %s%s" % (name, "  (pinned)" if show.get("pinned") else ""))
        print("poc     : %s" % (poc_name(show) or "(none)"))
        print("members : %d" % len(members))
        if members:
            names = [mname(m) for m in members]
            paths = [home_path(m.get("path")) for m in members]
            nw = max(len(n) for n in names) + 4
            pw = max(len(p) for p in paths) + 4
            for m, n, p in zip(members, names, paths):
                marker = "(PoC)" if m.get("workspaceId") == pid else ""
                line = ("  %-" + str(nw) + "s%-" + str(pw) + "s%s") % (n, p, marker)
                print(line.rstrip())
        if msgs:
            print()
            print("last messages (see `k2 project read %s` for more):" % name)
            print_message_rows(msgs)

elif verb == "add":
    d = call("POST", "/cli/project-group/add-member",
             body={"group": os.environ["P_ARG_PROJECT"],
                   "workspace": os.environ["P_ARG_WORKSPACE"]})
    show = fetch_show(d.get("groupId"))
    m = member_by_id(show, d.get("workspaceId"))
    wsname = mname(m) if m else os.environ["P_ARG_WORKSPACE"]
    name = show.get("name") or "?"
    count = len(show.get("members", []))
    pn = poc_name(show)
    already = bool(d.get("alreadyMember"))
    became = bool(d.get("becamePoc"))
    if as_json:
        print(dumps({"ok": True, "changed": not already, "project": name,
                     "workspace": wsname, "members": count, "poc": pn,
                     "becamePoc": became}))
    elif already:
        print("%s is already a member of %s — nothing to do." % (wsname, name))
    elif became:
        print("Added %s to %s (first member — %s is now the PoC)." % (wsname, name, wsname))
    else:
        print("Added %s to %s (member %d of %d · PoC: %s)."
              % (wsname, name, count, count, pn or "?"))

elif verb == "remove":
    # show BEFORE the removal: the member's display name is gone after.
    show = fetch_show(os.environ["P_ARG_PROJECT"])
    d = call("POST", "/cli/project-group/remove-member",
             body={"group": show.get("id"),
                   "workspace": os.environ["P_ARG_WORKSPACE"]})
    m = member_by_id(show, d.get("workspaceId"))
    wsname = mname(m) if m else os.environ["P_ARG_WORKSPACE"]
    name = show.get("name") or "?"
    if as_json:
        print(dumps({"ok": True, "project": name, "workspace": wsname}))
    else:
        print("Removed %s from %s." % (wsname, name))

elif verb == "poc":
    show = fetch_show(os.environ["P_ARG_PROJECT"])
    target = os.environ.get("P_ARG_WORKSPACE", "")
    name = show.get("name") or "?"
    if not target:
        pn = poc_name(show)
        if as_json:
            print(dumps({"ok": True, "project": name, "poc": pn}))
        elif pn:
            # Bare name on stdout — safe for $(...) capture.
            print(pn)
    else:
        old = poc_name(show)
        d = call("POST", "/cli/project-group/set-poc",
                 body={"group": show.get("id"), "workspace": target})
        m = member_by_id(show, d.get("pocWorkspaceId"))
        new = mname(m) if m else target
        if as_json:
            print(dumps({"ok": True, "project": name, "poc": new}))
        else:
            print("PoC of %s: %s → %s. New chat posts now inject into %s's session."
                  % (name, old or "(none)", new, new))

elif verb == "msg":
    text = os.environ["P_TEXT"]
    proj = os.environ.get("P_ARG_PROJECT", "")
    form = os.environ.get("P_MSG_FORM", "one")
    from_stdin = os.environ.get("P_TEXT_STDIN") == "true"
    if proj:
        d = request("GET", "/cli/project-group/show", {"group": proj})
        if d is None:
            die({"code": "daemon_unreachable",
                 "hint": "no response from the K2 daemon (is it running?)"})
        err = d.get("error")
        if err:
            if isinstance(err, str):
                err = {"code": "error", "hint": err}
            if err.get("code") == "not_found" and form == "two" and not from_stdin:
                # Two-arg call whose first token doesn't resolve as a
                # project: suggest the fully-quoted form too (mockup).
                whole = ("%s %s" % (proj, text)).replace('"', '\\"')
                err = dict(err, hint='%s — if that was part of your message, '
                           'quote the whole thing: k2 project msg "%s"'
                           % (err.get("hint", ""), whole))
            die(err)
        show = d
    else:
        show = detect_project()
    me = my_member(show)
    if me is not None:
        # Registry name round-trips through the daemon's workspace
        # resolver (membership is re-checked route-side).
        author = me.get("name") or mname(me)
    else:
        # Not a member here (or an unregistered directory) — send the
        # best identity we have and let the daemon refuse (not_a_member).
        author = (os.environ.get("P_AGENT_NAME")
                  or os.path.basename(ws_path.rstrip("/")) or "?")
    d = call("POST", "/cli/project-group/msg",
             body={"group": show.get("id"), "body": text, "author": author})
    name = d.get("groupName") or show.get("name") or "?"
    delivered = bool(d.get("delivered"))
    reason = d.get("deliveryReason")
    # "queued" (0.46.8): the daemon answered before the PoC inject finished
    # (it only does that for an agent's post when the inject runs very long).
    if not delivered and reason is not None and reason not in ("author-is-poc", "no_poc", "queued"):
        reason = "delivery-failed"  # the mockup's stable failure value
    if as_json:
        out = {"ok": True, "id": d.get("id"), "project": name,
               "author": d.get("author"), "delivered": delivered}
        if not delivered and reason:
            out["deliveryReason"] = reason
        if d.get("deliveredSessionId"):
            out["deliveredSessionId"] = d.get("deliveredSessionId")
        print(dumps(out))
    elif delivered:
        print("Posted to %s — delivered to PoC %s." % (name, poc_name(show) or "?"))
    elif reason == "author-is-poc":
        print("Posted to %s. (You are the PoC — your own posts are never injected back to you.)"
              % name)
    elif reason == "delivery-failed":
        print("Posted to %s — stored, but the PoC's session could not be reached" % name)
        print("(it will catch up with `k2 project read`).")
    elif reason == "queued":
        print("Posted to %s — on its way to PoC %s (still being delivered)." % (name, poc_name(show) or "?"))
    else:
        print("Posted to %s." % name)

elif verb == "read":
    proj = os.environ.get("P_ARG_PROJECT", "")
    show = fetch_show(proj) if proj else detect_project()
    params = {"group": show.get("id")}
    if os.environ.get("P_AFTER"):
        params["after"] = os.environ["P_AFTER"]
    if os.environ.get("P_LIMIT"):
        params["limit"] = os.environ["P_LIMIT"]
    d = call("GET", "/cli/project-group/messages", params=params)
    msgs = d.get("messages", [])
    name = show.get("name") or "?"
    truncated = bool(d.get("truncated"))
    if as_json:
        out = {"ok": True, "project": name,
               "messages": [{"id": m.get("id"), "author": m.get("author"),
                             "body": m.get("body"), "at": m.get("createdAt")}
                            for m in msgs]}
        if truncated:
            out["truncated"] = True
            if msgs:
                out["hint"] = ("more messages remain — continue with --after %s"
                               % msgs[-1].get("createdAt"))
        print(dumps(out))
    else:
        print("project %s — %d most recent messages (oldest first):" % (name, len(msgs)))
        print()
        print_message_rows(msgs)
        if truncated and msgs:
            print()
            print("(more messages remain — continue with: k2 project read %s --after %s)"
                  % (name, msgs[-1].get("createdAt")))

elif verb == "rename":
    d = call("POST", "/cli/project-group/rename",
             body={"group": os.environ["P_ARG_PROJECT"], "name": os.environ["P_NAME"]})
    if as_json:
        print(dumps({"ok": True, "id": d.get("id"), "name": d.get("name")}))
    else:
        print("Renamed to %s." % d.get("name"))

elif verb == "delete":
    show = fetch_show(os.environ["P_ARG_PROJECT"])
    d = call("POST", "/cli/project-group/delete", body={"group": show.get("id")})
    if as_json:
        print(dumps({"ok": True, "id": d.get("id"), "name": show.get("name")}))
    else:
        print("Deleted project %s, its dashboards and its chat. Its agents and their folders are untouched."
              % show.get("name"))

elif verb == "pin":
    pinned = os.environ["P_NAME"] != "off"
    d = call("POST", "/cli/project-group/pin",
             body={"group": os.environ["P_ARG_PROJECT"], "pinned": pinned})
    if as_json:
        print(dumps({"ok": True, "id": d.get("id"), "name": d.get("name"),
                     "pinned": bool(d.get("pinned"))}))
    else:
        print("%s %s for everyone on this server." % (
            d.get("name"), "is pinned" if d.get("pinned") else "is no longer pinned"))

elif verb == "view":
    # prd-projects-threads-v1 §4.3: the view a project's agent tiles open in
    # (`defaultView` on show). No view: print it. A view (checked by the
    # shell half before any request): set it, owner/admin or an agent with
    # 'Manage K2 projects' (the daemon decides).
    show = fetch_show(os.environ["P_ARG_PROJECT"])
    name = show.get("name") or "?"
    if "defaultView" not in show:
        die({"code": "unsupported",
             "hint": "this K2 server is too old for a project's view; update K2 on it"})
    want = os.environ.get("P_VIEW", "")
    if not want:
        cur = show.get("defaultView")
        if as_json:
            print(dumps({"ok": True, "project": name, "defaultView": cur}))
        else:
            print("%s opens agents in: %s" % (name, cur))
    else:
        d = call("POST", "/cli/project-group/set-default-view",
                 body={"group": show.get("id"), "view": want})
        name = d.get("name") or name
        new = d.get("defaultView") or want
        if as_json:
            print(dumps({"ok": True, "project": name, "defaultView": new}))
        else:
            print("%s now opens agents in: %s" % (name, new))

elif verb == "access":
    # The per-agent "Manage K2 projects" switch (0162). Owner/Admin only:
    # inside a K2 terminal this sends the session passport, which the
    # daemon refuses (an agent never flips its own switch).
    on = os.environ.get("P_NAME") == "on"
    target = os.environ.get("P_ARG_WORKSPACE") or ws_path
    if not target:
        die({"code": "usage", "hint": "pass --workspace <workspace>"})
    d = call("POST", "/cli/agent-access/set",
             body={"workspace": target, "toggle": "projects_manage", "value": on})
    wsname = d.get("workspace") or target
    if as_json:
        print(dumps({"ok": True, "workspace": wsname,
                     "agentsCanManageProjects": bool(d.get("agentsCanManageProjects"))}))
    elif on:
        print("Manage K2 projects is ON for %s: its agent may create, rename and delete"
              " projects and change their members." % wsname)
    else:
        print("Manage K2 projects is OFF for %s: its agent can read projects and post in"
              " its project chat, but not change them." % wsname)

else:
    die({"code": "usage", "hint": "unknown project verb '%s'" % verb})
PYEOF
}

cmd_project_access() {
    if _wants_help "$@"; then cmd_help_project_access; exit 0; fi
    local state="" workspace="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            on|off)
                [ -z "$state" ] || { _project_err "usage" "pass on or off once"; exit 2; }
                state="$1"; shift ;;
            --workspace)
                [ $# -ge 2 ] || { _project_err "usage" "--workspace requires a workspace name or path"; exit 2; }
                workspace="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            *)
                _project_err "usage" "unexpected argument '$1' — usage: k2 project access on|off [--workspace <ws>] [--json]"
                exit 2 ;;
        esac
    done
    if [ -z "$state" ]; then
        _project_err "usage" "access needs on or off — usage: k2 project access on|off [--workspace <ws>] [--json]"
        exit 2
    fi
    local P_VERB="access" P_JSON="$as_json" P_NAME="$state" P_ARG_WORKSPACE="$workspace"
    _project_py
}

cmd_project_rename() {
    if _wants_help "$@"; then cmd_help_project_rename; exit 0; fi
    local project="" name="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project rename <project> <new-name> [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$name" ]; then name="$1"
                else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ] || [ -z "$name" ]; then
        _project_err "usage" "rename requires <project> and <new-name>"
        exit 2
    fi
    local P_VERB="rename" P_JSON="$as_json" P_ARG_PROJECT="$project" P_NAME="$name"
    _project_py
}

cmd_project_delete() {
    if _wants_help "$@"; then cmd_help_project_delete; exit 0; fi
    local project="" yes=false as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --yes|-y) yes=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project delete <project> --yes [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"; else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "delete requires a <project>"
        exit 2
    fi
    if [ "$yes" != true ]; then
        _project_err "usage" "delete removes the project, its dashboards and its whole chat; pass --yes to confirm"
        exit 2
    fi
    local P_VERB="delete" P_JSON="$as_json" P_ARG_PROJECT="$project"
    _project_py
}

cmd_project_pin() {
    if _wants_help "$@"; then cmd_help_project_pin; exit 0; fi
    local project="" state="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project pin <project> [on|off] [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$state" ]; then
                    case "$1" in
                        on|off) state="$1" ;;
                        *) _project_err "usage" "pin takes on or off, not '$1'"; exit 2 ;;
                    esac
                else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "pin requires a <project>"
        exit 2
    fi
    local P_VERB="pin" P_JSON="$as_json" P_ARG_PROJECT="$project" P_NAME="${state:-on}"
    _project_py
}

# prd-projects-threads-v1 §4.3: print or set the view a project opens its
# agents in. The view is checked here, before any request: only terminal,
# thread or chatter (no split: a project tile shows one view).
cmd_project_view() {
    if _wants_help "$@"; then cmd_help_project_view; exit 0; fi
    local project="" view="" as_json=false lower
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project view <project> [terminal|thread|chatter] [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$view" ]; then
                    lower="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
                    case "$lower" in
                        terminal|thread|chatter) view="$lower" ;;
                        split)
                            _project_err "usage" "split is not a project view: a project's agents open in terminal, thread or chatter"
                            exit 2 ;;
                        *)
                            _project_err "usage" "view must be terminal, thread or chatter, not '$1'"
                            exit 2 ;;
                    esac
                else
                    _project_err "usage" "unexpected extra argument '$1' — usage: k2 project view <project> [terminal|thread|chatter] [--json]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "view requires a <project> (add terminal, thread or chatter to set it)"
        exit 2
    fi
    local P_VERB="view" P_JSON="$as_json" P_ARG_PROJECT="$project" P_VIEW="$view"
    _project_py
}

cmd_project_create() {
    if _wants_help "$@"; then cmd_help_project_create; exit 0; fi
    local name="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project create <name> [--json]"
                exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        _project_err "usage" "create requires a <name> (quote names containing spaces)"
        exit 2
    fi
    local P_VERB="create" P_JSON="$as_json" P_NAME="$name"
    _project_py
}

cmd_project_list() {
    if _wants_help "$@"; then cmd_help_project_list; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _project_err "usage" "unexpected argument '$1' — usage: k2 project list [--json]"
                exit 2 ;;
        esac
    done
    local P_VERB="list" P_JSON="$as_json"
    _project_py
}

cmd_project_show() {
    if _wants_help "$@"; then cmd_help_project_show; exit 0; fi
    local project="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project show <project> [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"; else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "show requires a <project> (a name; a unique prefix works)"
        exit 2
    fi
    local P_VERB="show" P_JSON="$as_json" P_ARG_PROJECT="$project"
    _project_py
}

cmd_project_add() {
    if _wants_help "$@"; then cmd_help_project_add; exit 0; fi
    local project="" workspace="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project add <project> <workspace> [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$workspace" ]; then workspace="$1"
                else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces (\"QA Bot\")"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ] || [ -z "$workspace" ]; then
        _project_err "usage" "add requires <project> and <workspace>"
        exit 2
    fi
    local P_VERB="add" P_JSON="$as_json" P_ARG_PROJECT="$project" P_ARG_WORKSPACE="$workspace"
    _project_py
}

cmd_project_remove() {
    if _wants_help "$@"; then cmd_help_project_remove; exit 0; fi
    local project="" workspace="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project remove <project> <workspace> [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$workspace" ]; then workspace="$1"
                else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces (\"QA Bot\")"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ] || [ -z "$workspace" ]; then
        _project_err "usage" "remove requires <project> and <workspace>"
        exit 2
    fi
    local P_VERB="remove" P_JSON="$as_json" P_ARG_PROJECT="$project" P_ARG_WORKSPACE="$workspace"
    _project_py
}

cmd_project_poc() {
    if _wants_help "$@"; then cmd_help_project_poc; exit 0; fi
    local project="" workspace="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project poc <project> [<workspace>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$workspace" ]; then workspace="$1"
                else
                    _project_err "usage" "unexpected extra argument '$1' — quote names containing spaces (\"QA Bot\")"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "poc requires a <project> (add <workspace> to reassign)"
        exit 2
    fi
    local P_VERB="poc" P_JSON="$as_json" P_ARG_PROJECT="$project" P_ARG_WORKSPACE="$workspace"
    _project_py
}

cmd_project_msg() {
    if _wants_help "$@"; then cmd_help_project_msg; exit 0; fi
    # Argument rules (exact, mockup): one positional → it is ALWAYS
    # <text>; two → the first is ALWAYS <project>; three or more →
    # exit 2 with the quote-your-message hint. "-" (the stdin sentinel)
    # is a positional, never a flag.
    local as_json=false count=0 first="" second="" joined=""
    while [ $# -gt 0 ]; do
        if [ "$1" = "--json" ]; then
            as_json=true
        elif [ "$1" != "-" ] && [ "${1#-}" != "$1" ]; then
            _project_err "usage" "unknown flag '$1' — usage: k2 project msg [<project>] <text> [--json]"
            exit 2
        else
            count=$((count + 1))
            if [ "$count" -eq 1 ]; then first="$1"
            elif [ "$count" -eq 2 ]; then second="$1"; fi
            joined="${joined}${joined:+ }$1"
        fi
        shift
    done
    if [ "$count" -eq 0 ]; then
        _project_err "usage" "msg requires <text> (or \"-\" to read it from stdin)"
        exit 2
    fi
    if [ "$count" -ge 3 ]; then
        _project_err "usage" "too many arguments — did you mean: k2 project msg \"$joined\"?"
        exit 2
    fi
    local project="" text="" form="one" from_stdin=false
    if [ "$count" -eq 1 ]; then
        text="$first"
    else
        form="two"; project="$first"; text="$second"
    fi
    if [ "$text" = "-" ]; then
        text="$(cat)"; from_stdin=true
    fi
    local P_VERB="msg" P_JSON="$as_json" P_ARG_PROJECT="$project" P_TEXT="$text" \
          P_MSG_FORM="$form" P_TEXT_STDIN="$from_stdin"
    _project_py
}

cmd_project_read() {
    if _wants_help "$@"; then cmd_help_project_read; exit 0; fi
    local project="" after="" limit="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --after)
                [ $# -ge 2 ] || { _project_err "usage" "--after requires a unix-seconds timestamp"; exit 2; }
                after="$2"; shift 2 ;;
            --limit)
                [ $# -ge 2 ] || { _project_err "usage" "--limit requires a number"; exit 2; }
                limit="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project read [<project>] [--after <ts>] [--limit <n>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"; else
                    _project_err "usage" "unexpected extra argument '$1' — usage: k2 project read [<project>] [--after <ts>] [--limit <n>] [--json]"
                    exit 2
                fi
                shift ;;
        esac
    done
    local P_VERB="read" P_JSON="$as_json" P_ARG_PROJECT="$project" P_AFTER="$after" P_LIMIT="$limit"
    _project_py
}

cmd_help_project() {
    cat <<'EOF'
k2 project — post to and manage this server's projects (named groups of agents)

A project groups agent workspaces into one effort with ONE shared chat stream.
Every project has exactly one Point of Contact (PoC) agent: anything anyone
ELSE posts to the chat is delivered straight into the PoC's live session, so
the PoC always hears the room (the PoC's own posts are never echoed back).
If you are a member agent: `msg` posts to the chat, `read` catches you up.
The other commands manage the group itself.

USAGE
  k2 project <command> [args] [flags]

COMMANDS
  create <name>                  Create an empty project
  rename <project> <new-name>    Rename a project
  pin <project> [on|off]         Show it in everyone's Pinned section (default on)
  view <project> [<view>]        Print or set the view the project's agents
                                 open in: terminal, thread or chatter
  delete <project> --yes         Delete a project with its dashboards and chat
                                 (owner or admin only; agents can't)
  list                           List this server's projects (members, PoC, pinned)
  show <project>                 One project: members, PoC, last few messages
  add <project> <workspace>      Add a member workspace (the FIRST member added
                                 automatically becomes the PoC)
  remove <project> <workspace>   Remove a member. Removing the current PoC is
                                 REFUSED until you name a successor with `poc`
  poc <project> [<workspace>]    Print the current PoC — or, with <workspace>,
                                 reassign the PoC to that member
  msg [<project>] <text>         Post to the project chat. <project> may be
                                 omitted when your workspace belongs to exactly
                                 ONE project (rule spelled out in `msg --help`)
  read [<project>]               Read the chat, oldest first (--after to catch up
                                 on messages since your last read)
  access on|off [--workspace <ws>]
                                 Owner/admin: turn a workspace agent's "Manage
                                 K2 projects" switch on or off

WHO MAY CHANGE PROJECTS
  Projects are open by default: any agent may create, rename and pin
  projects and add or remove members. Deleting a project (and its chat) and
  setting the view its agents open in (`view`) are for the owner, an admin,
  or an agent with "Manage K2 projects" on: a Member login gets exit 1 with
  code forbidden. Each workspace has that per-agent switch, "Manage K2
  projects" (on by default; Settings → Workspaces → <ws> → Agent, or
  `k2 project access`). Off: that agent gets exit 1 with code
  agent_permission_off on every change, but `list`, `show`, `read` and `msg`
  still work. Only the owner or an admin can flip it; an agent never can.
  Every project created or deleted and every member change is logged
  (who, what, when). More: `k2 study projects`.

ADDRESSING
  <project> is a project name. Names match case-insensitively, and any UNIQUE
  PREFIX works ("0.41" matches "0.41-release"). An ambiguous prefix fails with
  exit 4 and lists the matching candidates in the hint. An EXACT name match
  ALWAYS wins, even when it is also a prefix of other names ("Atlas" resolves
  to Atlas even with Atlas-docs present); prefix resolution only engages when
  there is no exact match.
  <workspace> is an agent name or a path — same rules as `k2 agent`: anything
  containing "/" is treated as a path; names match case-insensitively; quote
  names containing spaces ("QA Bot").

WHO YOU ARE
  msg and read act AS your workspace, auto-detected from K2_PROJECT_PATH
  (falling back to the current directory) — the same detection `k2 tickets`
  uses. Only members (and the server owner via the app) can post.

GLOBAL FLAGS
  --json                 Machine-readable output (single JSON object)

OUTPUT CONTRACT
  Success output is human-readable by default; --json switches it to JSON.
  ERRORS are ALWAYS JSON on stderr — {"error":{"code","hint"}} — regardless
  of --json, so scripts can rely on the shape.
  Success means ok:true. On msg, "delivered": false with "deliveryReason":
  "author-is-poc" is NORMAL (your own post never injects back to you) —
  it is NOT a failure.

EXIT CODES
  0 ok · 1 error (including refusals like removing the PoC or posting as a
  non-member) · 2 bad usage · 4 not found / ambiguous (candidates in the hint)
  4 = a name could not be resolved (fix the name and retry) · 1 = resolved
  but refused or failed

Run 'k2 project <command> --help' for details on a command.
EOF
}

cmd_help_project_access() {
    cat <<'EOF'
Turn a workspace agent's "Manage K2 projects" switch on or off. Projects are
open by default (the switch starts ON). Off: that workspace's agent can't
create, rename or delete projects or change their members or settings
(code agent_permission_off); it can still list, show and read projects and
post in its project chat. You (the owner) and people are never limited.

Owner or admin only. Inside a K2 terminal this sends the session passport,
which the daemon refuses: an agent can never flip its own switch. Run it from
a terminal outside K2, or use Settings → Workspaces → <ws> → Agent.

USAGE
  k2 project access on|off [--workspace <ws>] [--json]

FLAGS
  --workspace <ws>       The workspace (name or path); default: this one
  --json                 JSON output

EXAMPLES
  k2 project access off --workspace scout
  k2 project access on --workspace ~/agents/scout
EOF
}

cmd_help_project_rename() {
    cat <<'EOF'
Rename a project. Names are unique per server, case-insensitively.

USAGE
  k2 project rename <project> <new-name> [--json]

EXAMPLES
  k2 project rename 0.41-release 0.42-release
EOF
}

cmd_help_project_delete() {
    cat <<'EOF'
Delete a project: its member list, dashboards and its whole chat. The agents
and their folders are never touched. Only the owner, an admin, or an agent
with "Manage K2 projects" on can delete a project; a Member login is refused
(exit 1, code forbidden), and an agent with the switch off gets
agent_permission_off.

USAGE
  k2 project delete <project> --yes [--json]

FLAGS
  --yes, -y              Required: confirms you mean it
  --json                 JSON output
EOF
}

cmd_help_project_pin() {
    cat <<'EOF'
Pin a project so it shows in the Pinned section of the Projects nav for
everyone on this server, or unpin it with off.

USAGE
  k2 project pin <project> [on|off] [--json]

EXAMPLES
  k2 project pin 0.46.1
  k2 project pin 0.46.1 off
EOF
}

cmd_help_project_view() {
    cat <<'EOF'
Print or set the view a project opens its agents in: terminal, thread or
chatter. Every agent on the project's dashboards opens in this view. New
projects start on thread.

Anyone can switch one agent with the view picker in its bar. That switch
is only for them, and it ends when they restart the app or when this
default changes; the agent then goes back to the project's default view.

With no <view>, prints the current default, like this:
  sales opens agents in: thread
Anyone may read it; no permission is needed. After a set it prints:
  sales now opens agents in: terminal

Setting it is for the owner, an admin, or an agent with "Manage K2
projects" on (on by default). A Member login gets exit 1 with code
forbidden; an agent with the switch off gets exit 1 with code
agent_permission_off.

USAGE
  k2 project view <project> [terminal|thread|chatter] [--json]

VIEWS
  thread       The agent's Thread: you and the agent, like a text chat (default)
  terminal     The agent's live terminal
  chatter      Messages between this agent and other agents
  There is no split view for a project. Any other view (split, for one) is
  refused with exit 2, code usage, before the daemon is asked, so before
  any permission check.

FLAGS
  --json       JSON output: {"ok":true,"project":"sales","defaultView":"thread"}

EXAMPLES
  k2 project view sales
  k2 project view sales terminal

EXIT CODES
  0 ok · 1 refused · 2 bad usage (an unknown view) · 4 project not found or ambiguous
EOF
}

cmd_help_project_create() {
    cat <<'EOF'
Create an empty project. Add members with `k2 project add` — the first member
you add automatically becomes the Point of Contact (reassign later with `poc`).

Project names are unique per server, case-insensitively ("Kessel" and "kessel"
collide). Pick something short: everyone will type it in `msg`.

USAGE
  k2 project create <name> [flags]

ARGUMENTS
  <name>                 The project's name (quote names containing spaces)

FLAGS
  --json                 JSON output

EXAMPLES
  k2 project create 0.41-release
  k2 project create "Kessel Perf"
EOF
}

cmd_help_project_list() {
    cat <<'EOF'
List this server's projects: member count, PoC, and a pinned marker (*) for
projects pinned in the app's Projects nav.

USAGE
  k2 project list [flags]

FLAGS
  --json                 JSON output
EOF
}

cmd_help_project_show() {
    cat <<'EOF'
Show one project: its members (PoC marked), and the last few chat messages.
For the full chat stream use `k2 project read`.

USAGE
  k2 project show <project> [flags]

FLAGS
  --json                 JSON output

EXAMPLES
  k2 project show 0.41-release
  k2 project show 0.41            (unique prefix works)
EOF
}

cmd_help_project_add() {
    cat <<'EOF'
Add a member workspace to a project. A workspace can belong to many projects.

The FIRST member added to an empty project automatically becomes the PoC —
the output says so when it happens. Adding an existing member again is a
no-op (reported, exit 0).

USAGE
  k2 project add <project> <workspace> [flags]

FLAGS
  --json                 JSON output

EXAMPLES
  k2 project add 0.41-release Appa
  k2 project add 0.41-release ~/agents/scout
EOF
}

cmd_help_project_remove() {
    cat <<'EOF'
Remove a member workspace from a project. The workspace itself is untouched —
this only ends its membership (and its right to post to this project's chat).

Removing the CURRENT PoC is refused (exit 1, code "poc_successor_required"):
first hand off with `k2 project poc <project> <successor>`, then remove.
A project's SOLE member is its PoC and cannot be removed (there is no
successor to name); delete the project from the app's Settings page instead.

USAGE
  k2 project remove <project> <workspace> [flags]

FLAGS
  --json                 JSON output

EXAMPLES
  k2 project remove 0.41-release "QA Bot"
EOF
}

cmd_help_project_poc() {
    cat <<'EOF'
Without <workspace>: print the current PoC's name — bare, script-friendly
(like `k2 agent get`). With <workspace>: reassign the PoC to that member.

The PoC is the project's injection target: every chat post by anyone else is
delivered into the PoC's session. The target must already be a member (add it
first). Reassignment takes effect for the NEXT message.

USAGE
  k2 project poc <project> [<workspace>] [flags]

FLAGS
  --json                 JSON output

EXAMPLES
  k2 project poc 0.41-release            (prints: Appa)
  k2 project poc 0.41-release scout      (scout is now the PoC)
EOF
}

cmd_help_project_msg() {
    cat <<'EOF'
Post a message to a project's chat. This is the command member agents use to
report progress, hand off, or raise something to the team.

You post AS your workspace (auto-detected from K2_PROJECT_PATH, falling back
to the current directory). Your message is stored in the project chat and —
unless you ARE the PoC — delivered live into the PoC's session, waking it if
dormant. The PoC's own posts are stored for everyone to read but never
injected back into itself. A successful post is ok:true; "delivered": false
with "deliveryReason": "author-is-poc" is NORMAL — not a failure.

USAGE
  k2 project msg [<project>] <text> [flags]

ARGUMENT RULES (exact)
  two arguments   → the first is ALWAYS <project>, the second is <text>
  one argument    → it is ALWAYS <text>; the project is auto-detected
  three or more   → exit 2 (usage). Almost always an unquoted message — the
                    hint suggests the quoted form
                    (did you mean: k2 project msg "build done"?)
  <text> of "-"   → the message body is read from stdin (works in both forms;
                    to send a literal "-", send it via stdin)

QUOTING
  <text> is ONE argument — always quote multi-word messages ("build done",
  not build done). If a two-argument call's first token doesn't resolve as a
  project, the error's hint suggests the fully-quoted form too.

WHICH PROJECT? (when <project> is omitted)
  Your workspace's own memberships decide:
    member of exactly ONE project → that project is used (echoed in the output)
    member of NONE                → {"code":"no_project"}        exit 4
    member of TWO OR MORE         → {"code":"ambiguous_project"} exit 4, with
                                    the candidate names in the hint
  Passing <project> explicitly always works and is the safe form in scripts.

FLAGS
  --json                 JSON output (includes the delivery outcome:
                         delivered / deliveryReason / deliveredSessionId)

EXAMPLES
  k2 project msg "Build is green on all three targets."
  k2 project msg 0.41-release "Starting the notarize pass now."
  git log --oneline -5 | k2 project msg 0.41-release -
EOF
}

cmd_help_project_read() {
    cat <<'EOF'
Read a project's chat, oldest first. Default: the 20 most recent messages.

The project may be omitted under the same rule as `msg`: if your workspace
belongs to exactly ONE project, that one is read; none = exit 4 "no_project";
several = exit 4 "ambiguous_project" with candidates in the hint.

CATCHING UP (the agent pattern)
  Each message carries an `at` timestamp (unix seconds, in --json). Keep the
  last `at` you saw and pass it back: `read --after <ts>` returns only newer
  messages. There is no --follow; poll with --after when you wake.
  With --after, the DEFAULT limit becomes the max (500), not 20 — a catch-up
  read is meant to return everything new. If more than the limit match, the
  output carries "truncated": true and a hint to continue from the last
  message's `at`.

USAGE
  k2 project read [<project>] [flags]

FLAGS
  --after <ts>           Only messages with `at` greater than <ts> (unix seconds)
  --limit <n>            How many of the most recent messages to show
                         (default 20 — or 500 when --after is given; max 500)
  --json                 JSON output

EXAMPLES
  k2 project read 0.41-release
  k2 project read --after 1720000000 --json
EOF
}

