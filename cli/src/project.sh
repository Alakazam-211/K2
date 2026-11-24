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
    P_AGENT_NAME="$(_envn AGENT_NAME)" \
    P_VERB="${P_VERB:-}" P_JSON="${P_JSON:-false}" \
    P_NAME="${P_NAME:-}" P_ARG_PROJECT="${P_ARG_PROJECT:-}" \
    P_ARG_WORKSPACE="${P_ARG_WORKSPACE:-}" P_TEXT="${P_TEXT:-}" \
    P_MSG_FORM="${P_MSG_FORM:-}" P_TEXT_STDIN="${P_TEXT_STDIN:-false}" \
    P_AFTER="${P_AFTER:-}" P_LIMIT="${P_LIMIT:-}" P_VIEW="${P_VIEW:-}" P_FILES="${P_FILES:-}" P_ALL="${P_ALL:-}" \
    P_PERMISSION_NOTE="${_K2_PERMISSION_NOTE:-}" \
    _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)" python3 - <<'PYEOF'
import json, os, sys, time
import urllib.error, urllib.parse, urllib.request
# Loopback and plain http skip the proxy; remote https may use it.
exec(os.environ["_K2_PY_PROXY_RULE"])
exec(os.environ.pop("_K5_PY_HTTP"))

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
    p = dict(p, command="k2 <tail>".replace("<tail>", p["tail"])) if isinstance(p, dict) and isinstance(p.get("tail"), str) and p["tail"] else p  # the rules of _permission_closing_lines
    if not isinstance(p, dict):
        return []
    cmd = p.get("command")
    if isinstance(cmd, str) and cmd:
        return ["Ask with: " + cmd]
    if p.get("requestable") is False:
        hint = p.get("hint")
        return ([hint] if isinstance(hint, str) and hint else []) + ["It can't be requested.", __import__("os").environ["_K2_NO_CARD_FEEDBACK"]]
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
    qs = urllib.parse.urlencode(params or {})
    url = base + route + ("?" + qs if qs else "")
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(k5_request(req), timeout=30) as r:
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
        lines = ["(deleted)"] if m.get("deleted") else (str(m.get("body", "")).splitlines() or [""])
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
             body=dict({"group": show.get("id"), "body": text, "author": author}, **({"attachments": json.loads(os.environ["P_FILES"])} if os.environ.get("P_FILES") else {})))
    if os.environ.get("P_FILES") and "attachments" not in d:
        die({"code": "server_too_old", "hint": "this server's K2 doesn't take files on project lines (it needs project chats on the Thread)"})
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
        if d.get("attachments"):
            out["attachments"] = d.get("attachments")
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
    for a in ([] if as_json else d.get("attachments") or []):
        print("  [file] %s%s" % (a.get("path", ""), (":%s" % a["line"] + ("-%s" % a["lineEnd"] if a.get("lineEnd") else "")) if a.get("line") else ""))

elif verb == "read":
    proj = os.environ.get("P_ARG_PROJECT", "")
    show = fetch_show(proj) if proj else detect_project()
    params = {"group": show.get("id")}
    if os.environ.get("P_AFTER"):
        params["after"] = os.environ["P_AFTER"]
    if os.environ.get("P_LIMIT"):
        params["limit"] = os.environ["P_LIMIT"]
    if os.environ.get("P_ALL") == "true":
        params["include_deleted"] = "1"
    d = call("GET", "/cli/project-group/messages", params=params)
    msgs = d.get("messages", [])
    name = show.get("name") or "?"
    truncated = bool(d.get("truncated"))
    if as_json:
        out = {"ok": True, "project": name,
               "messages": [dict({"id": m.get("id"), "author": m.get("author"),
                             "body": m.get("body"), "at": m.get("createdAt")}, **({"deleted": True} if m.get("deleted") else {}))
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
    target = os.environ.get("P_ARG_WORKSPACE", "")
    if target:
        # 0302: one member agent's own view (`members[].defaultView` is the
        # view its tile opens in, `ownDefaultView` its own or null). The
        # member is found here, by id, name, agent name or path (same rules
        # as <workspace>), so a print and a set address the same agent.
        members = show.get("members", [])
        if members and any("ownDefaultView" not in m for m in members):
            die({"code": "unsupported",
                 "hint": "this K2 server is too old for one agent's own view; update K2 on it"})
        low = target.lower()
        found = None
        for m in members:
            if (m.get("workspaceId") == target
                    or (m.get("name") or "").lower() == low
                    or (m.get("agentName") or "").lower() == low
                    or ("/" in target and m.get("path") and same_path(m["path"], target))):
                found = m
                break
        if found is None:
            die({"code": "not_a_member",
                 "hint": "'%s' is not a member of %s (see: k2 project show %s)" % (target, name, name)})
        agent = mname(found)
        if not want:
            cur = found.get("defaultView")
            own = found.get("ownDefaultView")
            if as_json:
                print(dumps({"ok": True, "project": name, "agent": agent,
                             "defaultView": cur, "ownDefaultView": own}))
            else:
                print("%s in %s opens in: %s (%s)" % (
                    agent, name, cur, "its own view" if own else "the project's view"))
        else:
            d = call("POST", "/cli/project-group/set-member-default-view",
                     body={"group": show.get("id"), "workspace": found.get("workspaceId"),
                           "view": None if want == "default" else want})
            cur = d.get("defaultView")
            own = d.get("ownDefaultView")
            if as_json:
                print(dumps({"ok": True, "project": name, "agent": agent,
                             "defaultView": cur, "ownDefaultView": own}))
            elif own:
                print("%s in %s now opens in: %s" % (agent, name, cur))
            else:
                print("%s in %s now follows the project's view: %s" % (agent, name, cur))
    elif not want:
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
    local project="" view="" agent="" as_json=false lower
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --agent)
                [ $# -ge 2 ] && [ -n "$2" ] || { _project_err "usage" "--agent requires an agent's name, folder path or workspace id"; exit 2; }
                agent="$2"; shift 2 ;;
            -*)
                _project_err "usage" "unknown flag '$1' — usage: k2 project view <project> [terminal|thread|chatter|default] [--agent <name>] [--json]"
                exit 2 ;;
            *)
                if [ -z "$project" ]; then project="$1"
                elif [ -z "$view" ]; then
                    lower="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
                    case "$lower" in
                        terminal|thread|chatter|default) view="$lower" ;;
                        split)
                            _project_err "usage" "split is not a project view: a project's agents open in terminal, thread or chatter"
                            exit 2 ;;
                        *)
                            _project_err "usage" "view must be terminal, thread or chatter, not '$1'"
                            exit 2 ;;
                    esac
                else
                    _project_err "usage" "unexpected extra argument '$1' — usage: k2 project view <project> [terminal|thread|chatter|default] [--agent <name>] [--json]"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$project" ]; then
        _project_err "usage" "view requires a <project> (add terminal, thread or chatter to set it)"
        exit 2
    fi
    if [ "$view" = "default" ] && [ -z "$agent" ]; then
        _project_err "usage" "default clears one agent's own view, so it needs --agent <name>; a project's own view is terminal, thread or chatter"
        exit 2
    fi
    local P_VERB="view" P_JSON="$as_json" P_ARG_PROJECT="$project" P_VIEW="$view" P_ARG_WORKSPACE="$agent"
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
    # After a bare `--` every word is a positional, as typed.
    local as_json=false count=0 first="" second="" joined="" dashes=false
    local -a files=()
    while [ $# -gt 0 ]; do
        if [ "$dashes" = false ] && [ "$1" = "--" ]; then
            dashes=true
        elif [ "$dashes" = false ] && [ "$1" = "--json" ]; then
            as_json=true
        elif [ "$dashes" = false ] && [ "$1" = "--file" ]; then
            # project-chat-thread-v1: files ride the line (path[:line[-end]],
            # as `k2 thread --file`), kept in the PoC's workspace.
            [ $# -ge 2 ] && [ -n "$2" ] || { _project_err "usage" "--file needs a path (path[:line[-end]])"; exit 2; }
            files+=("$2"); shift
        elif [ "$dashes" = false ] && [ "$1" != "-" ] && [ "${1#-}" != "$1" ]; then
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
    if [ "$count" -eq 0 ] && [ "${#files[@]}" -eq 0 ]; then
        _project_err "usage" "msg requires <text> (or \"-\" to read it from stdin)"
        exit 2
    fi
    if [ "$count" -ge 3 ]; then
        _project_err "usage" "too many arguments — did you mean: k2 project msg \"$joined\"?"
        exit 2
    fi
    local project="" text="" form="one" from_stdin=false
    if [ "$count" -le 1 ]; then
        text="$first"
    else
        form="two"; project="$first"; text="$second"
    fi
    if [ "$text" = "-" ]; then
        text="$(cat)"; from_stdin=true
    fi
    local files_json=""
    if [ "${#files[@]}" -gt 0 ]; then
        files_json="$(_thread_attachments_json "${files[@]}")" || exit 1
    fi
    local P_VERB="msg" P_JSON="$as_json" P_ARG_PROJECT="$project" P_TEXT="$text" \
          P_MSG_FORM="$form" P_TEXT_STDIN="$from_stdin" P_FILES="$files_json"
    _project_py
}

cmd_project_read() {
    if _wants_help "$@"; then cmd_help_project_read; exit 0; fi
    local project="" after="" limit="" as_json=false all=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --all) all=true; shift ;;
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
    local P_VERB="read" P_JSON="$as_json" P_ARG_PROJECT="$project" P_AFTER="$after" P_LIMIT="$limit" P_ALL="$all"
    _project_py
}

cmd_help_project() {
    cat <<'EOF'
%include cli/text/help/project.txt
EOF
}

cmd_help_project_access() {
    cat <<'EOF'
%include cli/text/help/project/access.txt
EOF
}

cmd_help_project_rename() {
    cat <<'EOF'
%include cli/text/help/project/rename.txt
EOF
}

cmd_help_project_delete() {
    cat <<'EOF'
%include cli/text/help/project/delete.txt
EOF
}

cmd_help_project_pin() {
    cat <<'EOF'
%include cli/text/help/project/pin.txt
EOF
}

cmd_help_project_view() {
    cat <<'EOF'
%include cli/text/help/project/view.txt
EOF
}

cmd_help_project_create() {
    cat <<'EOF'
%include cli/text/help/project/create.txt
EOF
}

cmd_help_project_list() {
    cat <<'EOF'
%include cli/text/help/project/list.txt
EOF
}

cmd_help_project_show() {
    cat <<'EOF'
%include cli/text/help/project/show.txt
EOF
}

cmd_help_project_add() {
    cat <<'EOF'
%include cli/text/help/project/add.txt
EOF
}

cmd_help_project_remove() {
    cat <<'EOF'
%include cli/text/help/project/remove.txt
EOF
}

cmd_help_project_poc() {
    cat <<'EOF'
%include cli/text/help/project/poc.txt
EOF
}

cmd_help_project_msg() {
    cat <<'EOF'
%include cli/text/help/project/msg.txt
EOF
}

cmd_help_project_read() {
    cat <<'EOF'
%include cli/text/help/project/read.txt
EOF
}

