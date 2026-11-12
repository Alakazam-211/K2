# ── k2 db / k2 store (workspace data sidecar) ─────────────────────────

_db_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

_db_py() {
    ABASE="$BASE_URL" ATOKEN="$TOKEN" D_WS_PATH="$PROJECT" \
    D_VERB="${D_VERB:-}" D_JSON="${D_JSON:-false}" \
    D_ID="${D_ID:-}" D_NAME="${D_NAME:-}" D_OUT="${D_OUT:-}" \
    D_FILE="${D_FILE:-}" D_DIR="${D_DIR:-}" D_YES="${D_YES:-false}" \
    D_HEALTH="${D_HEALTH:-false}" D_DOC_ID="${D_DOC_ID:-}" \
    D_DOC_JSON="${D_DOC_JSON:-}" D_WHERE="${D_WHERE:-}" D_LIMIT="${D_LIMIT:-}" \
    D_GRANTEE="${D_GRANTEE:-}" D_DB="${D_DB:-}" D_LEVEL="${D_LEVEL:-}" \
    D_MANAGE="${D_MANAGE:-false}" D_ROLE="${D_ROLE:-}" D_ACCESS="${D_ACCESS:-}" \
    D_RELATIONS="${D_RELATIONS:-[]}" D_MAJOR="${D_MAJOR:-}" \
    D_REVEAL="${D_REVEAL:-false}" D_AS="${D_AS:-}" D_PERM_NOTE="${_K2_PERMISSION_NOTE:-}" \
    python3 - <<'PYEOF'
import json, os, sys, urllib.error, urllib.parse, urllib.request

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
verb = os.environ["D_VERB"]
as_json = os.environ.get("D_JSON") == "true"
ws_path = os.environ.get("D_WS_PATH", "")

def dumps(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))

def die(err, exit_code=None):
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    code = err.get("code", "error")
    hint = err.get("hint", "")
    if code in ("forbidden", "owner_only") and "ask your human" not in hint:
        hint = (hint + " — " if hint else "") + "requires owner/admin — ask your human"
    sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
    refused = ("gated", "forbidden", "owner_only", "unknown_major", "unsupported_major", "preview_major",
               "major_eol", "not_upward", "cluster_refused", "helper_needed", "db_upgrade_off",
               "major_mismatch")
    if exit_code is None:
        exit_code = 3 if code in refused else 2 if code == "usage" else 1
    sys.exit(exit_code)

def request(method, route, params=None, body=None):
    qs = urllib.parse.urlencode(dict(params or {}, token=token))
    url = base + route + "?" + qs
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            raw = resp.read().decode("utf-8", "replace")
            return resp.status, raw
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        return e.code, raw
    except Exception as e:
        die({"code": "error", "hint": "daemon unreachable: %s" % e})

def load(raw):
    try:
        return json.loads(raw) if raw else {}
    except Exception:
        return {"raw": raw}

status, raw = 0, ""
d = {}

if verb == "status":
    params = {}
    if os.environ.get("D_HEALTH") == "true":
        params["health"] = "1"
    status, raw = request("GET", "/cli/db/status", params)
    d = load(raw)
elif verb == "enable":
    status, raw = request("POST", "/cli/db/server/enable", body={})
    d = load(raw)
elif verb == "disable":
    status, raw = request("POST", "/cli/db/server/disable", body={})
    d = load(raw)
elif verb == "uninstall":
    status, raw = request("POST", "/cli/db/server/uninstall", body={})
    d = load(raw)
elif verb == "doctor":
    status, raw = request("POST", "/cli/db/doctor", body={})
    d = load(raw)
elif verb == "create":
    body = {"project": ws_path}
    if os.environ.get("D_ID"):
        body["clientId"] = os.environ["D_ID"]
    if os.environ.get("D_NAME"):
        body["name"] = os.environ["D_NAME"]
    if os.environ.get("D_ACCESS"):
        body["access"] = os.environ["D_ACCESS"]
    if os.environ.get("D_TEST") == "true":
        body["test"] = True
    if os.environ.get("D_MAJOR"):
        body["major"] = int(os.environ["D_MAJOR"])
    status, raw = request("POST", "/cli/db/create", body=body)
    d = load(raw)
elif verb == "clusters":
    status, raw = request("GET", "/cli/db/status", {})
    d = load(raw)
    if isinstance(d, dict) and d.get("ok") is not False:
        d = {"ok": True, "clusters": d.get("clusters") or [], "databases": d.get("databases") or [],
             "defaultMajor": d.get("defaultMajor"), "defaultMajorPinned": d.get("defaultMajorPinned"),
             "defaultMajorSource": d.get("defaultMajorSource"),
             "upgradeAvailable": d.get("upgradeAvailable") or []}
        # 0.46.11: the helper's view, named clusters and (--health) each K2
        # cluster's health block, from GET /cli/db/clusters.
        cstatus, craw = request("GET", "/cli/db/clusters", {"health": "1"} if os.environ.get("D_HEALTH") == "true" else {})
        cd = load(craw)
        if cstatus == 200 and isinstance(cd, dict) and isinstance(cd.get("clusters"), list):
            d["clusters"] = cd["clusters"]
            if cd.get("helperView"):
                d["helperView"] = cd["helperView"]
elif verb == "default_major":
    status, raw = request("POST", "/cli/db/server/default-major", body={"major": int(os.environ.get("D_MAJOR") or 0)})
    d = load(raw)
elif verb == "list":
    status, raw = request("GET", "/cli/db/list", {"project": ws_path})
    d = load(raw)
elif verb == "dsn":
    params = {"project": ws_path}
    if os.environ.get("D_TEST") == "true":
        params["test"] = "1"
    if os.environ.get("D_ACTOR"):
        params["actor"] = os.environ["D_ACTOR"]
    if os.environ.get("D_REF") == "true":
        params["ref"] = "1"
    if os.environ.get("D_DB"):
        params["db"] = os.environ["D_DB"]
    if os.environ.get("D_REVEAL") == "true":
        params["reveal"] = "1"
    if os.environ.get("D_AS"):
        params["as"] = os.environ["D_AS"]
    status, raw = request("GET", "/cli/db/dsn", params)
    d = load(raw)
elif verb == "migrate":
    body = {"project": ws_path}
    if os.environ.get("D_DIR"):
        body["dir"] = os.environ["D_DIR"]
    if os.environ.get("D_DB"):
        body["db"] = os.environ["D_DB"]
    status, raw = request("POST", "/cli/db/migrate", body=body)
    d = load(raw)
elif verb == "dump":
    body = {"project": ws_path}
    if os.environ.get("D_OUT"):
        body["out"] = os.environ["D_OUT"]
    if os.environ.get("D_DB"):
        body["db"] = os.environ["D_DB"]
    status, raw = request("POST", "/cli/db/dump", body=body)
    d = load(raw)
elif verb == "restore":
    body = {"project": ws_path, "file": os.environ.get("D_FILE", "")}
    if os.environ.get("D_DB"):
        body["db"] = os.environ["D_DB"]
    status, raw = request("POST", "/cli/db/restore", body=body)
    d = load(raw)
elif verb == "drop":
    body = {"project": ws_path, "yes": os.environ.get("D_YES") == "true"}
    if os.environ.get("D_TEST") == "true":
        body["test"] = True
    if os.environ.get("D_DB"):
        body["db"] = os.environ["D_DB"]
    status, raw = request("POST", "/cli/db/drop", body=body)
    d = load(raw)
elif verb == "grant":
    try:
        relations = json.loads(os.environ.get("D_RELATIONS") or "[]")
    except Exception:
        die({"code": "usage", "hint": "relations must be a JSON array"})
    if not isinstance(relations, list) or not all(isinstance(x, str) for x in relations):
        die({"code": "usage", "hint": "relations must be a JSON array of strings"})
    body = {
        "project": os.environ.get("D_GRANTEE", ""),
        "db": os.environ.get("D_DB", ""),
        "level": os.environ.get("D_LEVEL", ""),
        "manage": os.environ.get("D_MANAGE") == "true",
        "relations": relations,
    }
    status, raw = request("POST", "/cli/db/grant", body=body)
    d = load(raw)
elif verb == "revoke":
    body = {
        "project": os.environ.get("D_GRANTEE", ""),
        "db": os.environ.get("D_DB", ""),
    }
    status, raw = request("POST", "/cli/db/revoke", body=body)
    d = load(raw)
elif verb == "bind":
    body = {"project": ws_path, "role": os.environ.get("D_ROLE", "")}
    if os.environ.get("D_DB"):
        body["db"] = os.environ["D_DB"]
    status, raw = request("POST", "/cli/db/bind", body=body)
    d = load(raw)
elif verb == "store_create":
    status, raw = request("POST", "/cli/store/create", body={"project": ws_path, "name": os.environ.get("D_NAME", "")})
    d = load(raw)
elif verb == "store_list":
    status, raw = request("GET", "/cli/store/list", {"project": ws_path})
    d = load(raw)
elif verb == "store_put":
    doc = {}
    rawj = os.environ.get("D_DOC_JSON") or "{}"
    try:
        doc = json.loads(rawj)
    except Exception:
        die({"code": "usage", "hint": "--json body must be JSON"})
    body = {"project": ws_path, "name": os.environ.get("D_NAME", ""), "id": os.environ.get("D_DOC_ID", ""), "json": doc}
    status, raw = request("POST", "/cli/store/put", body=body)
    d = load(raw)
elif verb == "store_get":
    status, raw = request("GET", "/cli/store/get", {"project": ws_path, "name": os.environ.get("D_NAME", ""), "id": os.environ.get("D_DOC_ID", "")})
    d = load(raw)
elif verb == "store_query":
    params = {"project": ws_path, "name": os.environ.get("D_NAME", "")}
    if os.environ.get("D_LIMIT"):
        params["limit"] = os.environ["D_LIMIT"]
    status, raw = request("GET", "/cli/store/query", params)
    d = load(raw)
elif verb == "store_rm":
    status, raw = request("POST", "/cli/store/rm", body={"project": ws_path, "name": os.environ.get("D_NAME", ""), "id": os.environ.get("D_DOC_ID", "")})
    d = load(raw)
elif verb == "store_drop":
    status, raw = request("POST", "/cli/store/drop", body={"project": ws_path, "name": os.environ.get("D_NAME", "")})
    d = load(raw)
else:
    die({"code": "usage", "hint": "unknown db/store verb"})

# 0.46.11 (field report F4): a refusal's `permission` object (the daemon
# adds it while permission cards are on for this agent) becomes the one
# closing line every other family prints: "Ask with: k2 permission request
# …" or "It can't be requested." The CLI's exit trap prints it after the
# error line (the note file is _permission_note's).
def note_permission(v):
    p = v.get("permission") if isinstance(v, dict) else None
    path = os.environ.get("D_PERM_NOTE") or ""
    if not isinstance(p, dict) or not path:
        return
    lines = []
    cmd = p.get("command")
    if isinstance(cmd, str) and cmd:
        lines.append("Ask with: " + cmd)
    elif p.get("requestable") is False:
        if isinstance(p.get("hint"), str) and p["hint"]:
            lines.append(p["hint"])
        lines.append("It can't be requested.")
    if lines:
        try:
            with open(path, "w") as f:
                f.write("\n".join(lines) + "\n")
        except Exception:
            sys.stderr.write("\n".join(lines) + "\n")

if isinstance(d, dict) and d.get("ok") is False and isinstance(d.get("error"), dict):
    note_permission(d)
    die(d["error"])
if status and int(status) >= 400:
    note_permission(d)
    if isinstance(d, dict) and isinstance(d.get("error"), dict):
        die(d["error"])
    die({"code": "error", "hint": raw[:400] or ("HTTP %s" % status)})

if as_json:
    print(dumps(d) if not isinstance(d, str) else d)
    sys.exit(0)

if verb == "status":
    print("supported: %s" % d.get("supported"))
    print("state:     %s" % d.get("state"))
    if d.get("installedMajor") is not None:
        print("pg major:  %s" % d.get("installedMajor"))
    if d.get("listen"):
        print("listen:    %s" % d.get("listen"))
    port = d.get("port")
    if port is not None:
        print("port:      %s" % port)
    if d.get("publishHint"):
        print("off-box:   %s" % d["publishHint"])
    if d.get("lastError"):
        print("last error: %s" % d["lastError"])
    if d.get("defaultMajor") is not None:
        src = {"set": " (set: k2 db default-major)", "primary": " (this box's 5432 cluster)", "new-install": " (K2's new-install default)"}
        print("new dbs:   Postgres %s%s" % (d.get("defaultMajor"), src.get(d.get("defaultMajorSource"), "")))
    for c in d.get("clusters") or []:
        print("cluster:   Postgres %s on %s (%s, %s)" % (c.get("major"), c.get("port"), c.get("role"), c.get("state")))
    for u in d.get("upgradeAvailable") or []:
        if u.get("check"):
            print("upgrade:   %s on %s → %s available: %s" % (u.get("name"), u.get("from"), "/".join(str(x) for x in u.get("to") or []), u.get("check")))
        else:
            print("upgrade:   %s on %s → %s available, once Postgres runs" % (u.get("name"), u.get("from"), "/".join(str(x) for x in u.get("to") or [])))
    blocked = d.get("upgradeBlocked")
    if isinstance(blocked, dict) and (d.get("upgradeAvailable") or []):
        print("first:     %s" % blocked.get("hint"))
elif verb == "clusters":
    cl = d.get("clusters") or []
    print("%s cluster(s) · new databases go on Postgres %s%s" % (len(cl), d.get("defaultMajor"), " (pinned)" if d.get("defaultMajorPinned") else ""))
    for c in cl:
        label = c.get("label") or ("%s/%s" % (c.get("major"), c.get("name") or "main"))
        named = (c.get("name") or "main") != "main"
        if named:
            n = len([x for x in d.get("databases") or [] if x.get("cluster") == label])
        else:
            n = len([x for x in d.get("databases") or [] if x.get("major") == c.get("major") and not x.get("cluster")])
        extra = ""
        if c.get("role") == "foreign":
            extra = "  made outside K2: never touched"
        elif named:
            extra = "  named%s" % (", WAL archive" if c.get("walArchive") else "")
        print("  %-14s port %s  %s  %s/%s  %s database(s)%s" % (label, c.get("port"), c.get("unit"), c.get("role"), c.get("state"), n, extra))
        h = c.get("health")
        if isinstance(h, dict):
            if h.get("error"):
                print("      health: %s" % h["error"])
            elif h.get("online") is False:
                print("      health: not running")
            else:
                sizes = ", ".join("%s %s MB" % (x.get("name"), round((x.get("bytes") or 0) / 1048576)) for x in h.get("databases") or [])
                a = h.get("archiver") or {}
                print("      %s/%s connections · %s" % (h.get("connections"), h.get("maxConnections"), sizes or "no databases"))
                if h.get("archiveMode") == "on":
                    print("      WAL archive: %s archived (last %s), %s failed (last %s)" % (a.get("archivedCount"), a.get("lastArchivedAt") or "-", a.get("failedCount"), a.get("lastFailedAt") or "-"))
    for u in d.get("upgradeAvailable") or []:
        print("  upgrade available: %s (%s → %s)" % (u.get("name"), u.get("from"), "/".join(str(x) for x in u.get("to") or [])))
elif verb == "default_major":
    print("new databases go on Postgres %s" % d.get("defaultMajor"))
elif verb in ("enable", "disable", "uninstall"):
    print("state: %s" % d.get("state"))
elif verb == "create":
    print("created %s%s" % (d.get("name"), (" on Postgres %s" % d["major"]) if d.get("major") else ""))
    if d.get("majorNote"):
        print("note: %s" % d["majorNote"])
    if d.get("existing"):
        print("(already there: same --id or name)")
    if d.get("dsn"):
        print("dsn: %s" % d.get("dsn"))
    if d.get("migratorDsn"):
        print("migratorDsn: %s" % d.get("migratorDsn"))
    if d.get("dsnHint"):
        print("no password shown; %s" % d["dsnHint"])
    kept = d.get("kept") or {}
    if kept.get("ref"):
        print("kept:     %s as %s (this workspace's default database)" % (kept.get("ref"), kept.get("name")))
    elif kept.get("error"):
        print("kept:     no DATABASE_URL reference (%s)" % kept["error"])
    cap = d.get("cap") or {}
    if cap:
        print("cap %s/%s used" % (cap.get("used"), cap.get("cap") if cap.get("cap") else "unlimited"))
    for a in d.get("apps") or []:
        print("app:      %s" % a.get("note"))
    if d.get("next"):
        print("next:     %s (an App's own login and DATABASE_URL, checked live)" % d["next"])
elif verb == "dsn":
    if d.get("ref"):
        print(d["ref"])
        if d.get("name") and d.get("name") != "DATABASE_URL":
            print("kept as %s" % d["name"], file=sys.stderr)
    else:
        print(d.get("dsn") or "")
        if d.get("dsnHasPassword") is False and d.get("dsnHint"):
            print(d["dsnHint"], file=sys.stderr)
        elif d.get("dsnHasPassword") is True:
            print("this printed a password: keep it out of chats, logs, notes and git (k2 db run -- <cmd> needs no copy)", file=sys.stderr)
elif verb == "list":
    dbs = d.get("databases") or []
    print("%s database(s)" % len(dbs))
    for row in dbs:
        print("  %s  %s" % (row.get("name"), row.get("status")))
elif verb == "migrate":
    applied = d.get("applied") or []
    discovered = d.get("discovered") or 0
    if applied:
        print("applied: %s" % ", ".join(applied))
    elif d.get("noop") and discovered > 0:
        print("migrations: already applied")
    else:
        print("applied: %s" % ", ".join(applied))
elif verb == "dump":
    print("dump: %s" % d.get("path"))
elif verb == "restore":
    print("restored %s" % d.get("restored"))
elif verb == "drop":
    print("dropped %s" % d.get("dropped"))
    for a in d.get("apps") or []:
        print("app:      %s" % a.get("note"))
elif verb == "grant":
    print("granted %s on %s to %s" % (d.get("level"), d.get("name"), d.get("projectId")))
    if d.get("canManage"):
        print("can_manage: true")
    for a in d.get("apps") or []:
        print("app:      %s" % a.get("note"))
elif verb == "revoke":
    print("revoked %s from %s" % (d.get("name"), d.get("projectId")))
    for a in d.get("apps") or []:
        print("app:      %s" % a.get("note"))
elif verb == "bind":
    print("bind role: %s" % d.get("bindRole"))
elif verb == "doctor":
    print("ok:       %s" % d.get("ok"))
    print("baked:    %s" % d.get("baked"))
    print("unit:     %s" % d.get("unit"))
    print("select1:  %s" % d.get("select1"))
    print("major:    recorded %s · live %s%s" % (d.get("installedMajor"), d.get("liveMajor"),
          "  MISMATCH" if d.get("majorMismatch") else ""))
    for c in d.get("clusters") or []:
        print("cluster:  Postgres %s port %s %s (%s) · %s database(s)" % (
            c.get("major"), c.get("port"), c.get("unit"), c.get("unitState"), c.get("databases")))
    ram = d.get("ram") or {}
    cur = ram.get("currentBytes")
    mx = ram.get("maxBytes")
    print("rss:      %s / %s  (MemoryHigh=%s MemoryMax=%s withinCap=%s)" % (
        cur, mx, ram.get("high"), ram.get("max"), ram.get("withinCap")))
    print("ram unit: %s  oom=%s  dropinOk=%s" % (
        ram.get("unit"), ram.get("oomTarget"), ram.get("dropinOk")))
    gucs = d.get("gucs") or {}
    print("gucsOk:   %s" % d.get("gucsOk"))
    for k in ("shared_buffers", "work_mem", "max_connections", "max_parallel_workers",
              "statement_timeout", "idle_in_transaction_session_timeout", "temp_file_limit"):
        if k in gucs:
            print("  %s = %s" % (k, gucs.get(k)))
    if d.get("hint"):
        print("hint:     %s" % d.get("hint"))
else:
    print(dumps(d))
sys.exit(0)
PYEOF
}

cmd_help_db() {
    cat <<'EOF'
k2 db — workspace Postgres sidecar (SQL + JSON documents in the same DB)

Global: --workspace|--project <ws>  path, handle, or uuid (same as wiki/context).
Sets this command's workspace so GET /cli/db/dsn?project=… is that LOGIN.
Owner token only for a foreign workspace; a cell stamp still pins identity.

OWNER (engine — server-side owner/admin gate; agents exit 3)
  status [--health] [--json]     supported, state, pg major, clusters, upgrades available
  clusters [--health] [--json]   Postgres clusters: each major's main one (5432-5449), the
                                 named ones K2 made, and any made outside K2 (foreign:
                                 never touched). --health: connections, sizes, WAL archive
  cluster create <M> --name <n> [--port <p>] [--wal-archive]
                                 a named cluster (17/feedback) for this workspace, via the
                                 root helper (protocol 15): band port, K2's settings; K2
                                 never takes over or removes a cluster. Owner, or an agent
                                 with "Allow this agent to manage Postgres versions"
  cluster restart <name>         restart that one named cluster (same rule)
  default-major <M> [--json]     owner: the major new databases go on (new boxes: 17;
                                 existing boxes keep their current major until set)
  enable [--json]                catalog + start unit if baked (--bake --with-db)
  disable [--json]               stop unit, keep PGDATA
  doctor [--json]                RSS vs MemoryMax + GUC caps (work_mem, max_connections, …)
  uninstall [--json]             drop catalog row + stop unit
  bind --role <pg_role> [--db …] assistant PG role (default ws_<id>_agent; no secrets)
  versions-access on|off [--workspace <ws>]
                                 "Allow this agent to manage Postgres versions"
                                 (owner turns on; owner or Admin turns off; never an agent)
  cap [<n>|default] [--workspace <ws>] [--json]
                                 how many ACTIVE databases this workspace (its agent)
                                 may hold: default 25, 0 = unlimited, test DBs don't count.
                                 Anyone may read it (no number); only the owner (or a
                                 person signed in to this server) sets it; an agent's
                                 own terminal is refused (exit 3)

K2'S OWN DATABASE (~/.k2/k2.db; k2so.db is a link to it)
  home [--json]                  which file K2 uses and what the 0.46.6 switch did
                                 (owner or Admin)
  home --keep k2.db|k2so.db      owner: when both files hold workspaces, keep this one;
                                 at the next K2 start the other moves to
                                 ~/.k2/db-quarantine (never deleted).
  Peek at it with sqlite3 -readonly ~/.k2/k2.db while K2 runs, or
  sqlite3 'file:<home>/.k2/k2.db?mode=rw' when K2 is stopped. Never a plain
  sqlite3 <file>: on a missing file it creates an empty one. Copies: .backup.

AGENT (workspace — identity from token, never body project=)
  create [--id <client-id>] [--db-access off|read|write] [--json]
                                 needs db_agent_access=write (create-only passport).
                                 Up to the cap (default 25) per workspace: a new --id
                                 makes another database (ws_<id>_<client-id>); the same
                                 --id answers the one it made. No --id, no --name: the
                                 workspace's default database, kept in the vault as
                                 DATABASE_URL (as k2 app install --pg-major does). Shows
                                 no password. For an App: k2 app db connect <app> --db <name>
  create --test                  disposable scratch DB (not cap; owner/cell only)
  create --cluster <name> [--name <db>] [--id <id>]
                                 a database in this workspace's named cluster (k2 db
                                 cluster create); then connect the App to it:
                                 k2 app db connect <app> --db <db>
  create --major 16|17           pick the Postgres major (default: the box's: 17 on new
                                 boxes, the current major on existing ones until
                                 k2 db default-major); another major needs "Allow this agent to
                                 manage Postgres versions". --test follows the live DB
  list [--json]                  active + test (no dropped tombstones)
  dsn | creds [--json] [--workspace <ws>]
                                 owned or granted; never logged. Prints NO password
                                 (0.46.11): --reveal prints the full DSN, on purpose
  dsn --db <name|id>             one database by name, id or create --id (owned or
                                 granted). Without --db, dsn/migrate/dump/restore/drop
                                 use the workspace's default database (the oldest one)
  dsn --ref [--db <name|id>] [--as NAME]
                                 keep it in the vault and print its k2v:// reference:
                                 DATABASE_URL (the default DB), DATABASE_URL_<NAME>
                                 with --db, or NAME
  dsn --test                     agent DSN on the test DB
  dsn --test --actor migrator    migrator DSN on the test DB (two-role tests)
  run [--db <name|id>] [--test [--actor migrator]] [--as NAME] -- <command…>
                                 run a command with the DSN in $DATABASE_URL (or
                                 NAME), never on argv or stdout:
                                 k2 db run -- node migrate.js (it reads
                                 process.env.DATABASE_URL)
  migrate [--dir .k2/db/migrations] [--db <name|id>]
  dump  [--out .k2/db/dumps/<ts>.dump] [--db <name|id>]
  restore <file> [--db <name|id>]
                                 workspace-relative (no .. / abs)
  drop --yes [--db <name|id>]    a live database (default: the default one).
                                 --db on migrate/dump/restore/drop: one this
                                 workspace owns (a grant reaches dsn --db only)
  drop --test --yes              test DB only (404 if missing; never the live Skin)
  grant <workspace> --db … --level read|write [--relation schema.name …] [--manage]
  revoke <workspace> --db …

AN APP'S DATABASE (k2 app db --help; k2 study app-databases)
  k2 app db connect <app> --new [--major N] | --db <name>
                                 the App's own login, DATABASE_URL in the App, its
                                 door open, re-hosted, checked live (PASS/FAIL)

TWO ACTORS
  migrate runs as {db}_migrator (owner). k2 db dsn is {db}_agent.
  After each migration file, K2 GRANTs USAGE + table DML + sequences on
  every migrator-owned schema to the agent (do not hard-code ws_*_agent;
  do not CREATE ROLE / CREATE USER in .k2/db/migrations — roles are
  cluster-wide). Comments may mention FORCE RLS; executable FORCE is Usage.

Passport: db_agent_access (off/read/write, default off) only gates create.
list/dsn/migrate/dump/restore/drop/store work when this workspace owns a DB
or has a sql_grants row covering the verb.
--test create/dsn/drop and migrator DSN are owner or owning-workspace cell
only (Connect members and grants: 403). Guests never receive a DSN.

--test does not consume cap. One test DB per workspace. Live and test
cannot share a name. No migrate --test (apply SQL via the migrator DSN).

Postgres stays loopback. Off-box *.k2.dev:
  k2 publish subdomain create <label> --target localhost:<port>
(port already listening — do not `k2 publish run` Postgres; no `k2 db expose`).

Documents: k2 store … (same DB). Files: workspace FS. No k2 hostdb.

Skin helpers (ensured on create and migrate; POLICY is still copy-in):
  k2.skin_uid() / k2.set_principal(uuid) / k2.clear_principal() /
  k2.principal_hygiene(). set_principal is SET LOCAL. A username in the
  GUC errors (does not degrade). Hygiene true means the GUC is empty now
  — not anti-spoof; the BFF LOGIN can still SET another uuid.
  k2.actor_kind() / k2.actor() / k2.actor_info(): who is calling on the
  brokered app lane ('guest' = a person's own login session, 'agent' =
  a login the owner marked with `k2 app user agent`); NULL on any DSN or
  set_principal lane. `k2 study db` ACTOR has example policies.
  crates/k2-daemon/src/sql/skin_rls_template.sql — GUC POLICY example;
  no CREATE ROLE skin_*; no FORCE RLS; not auto-applied; no SET ROLE.

A refusal ends with the line to ask for it ("Ask with: k2 permission
request …") or "It can't be requested." while permission cards are on.

Exit: 0 ok · 1 error · 2 usage · 3 owner-only / gated / refused major
EOF
}

cmd_help_store() {
    cat <<'EOF'
k2 store — JSONB documents in this workspace's SQL database

Works when this workspace owns a DB or has a grant. db_agent_access only
gates k2 db create — not list/get/put.

Global: --workspace|--project <ws>  path, handle, or uuid (same as k2 db dsn).
k2 store list --workspace I uses I's PG LOGIN (owner token; cell stamp pins).

  create <name>
  list
  put  <name> --id <id> --json '{…}'
  get  <name> <id>
  query <name> [--limit N]
  rm   <name> <id>
  drop <name>

OWNER vs AGENT: create-new-DB needs db_agent_access=write. list/get/put
work when you own a DB or have a grant covering the verb.

GARDEN DATA (--widget): a Garden widget's own data on THIS computer.
Agents see only the keys the person (or a grant admin) granted them.
  get     --widget <w> <key> [--json]
  put     --widget <w> <key> --value '{…}' | --file f.json [--if-rev N]
  patch   --widget <w> <key> --value '{"done":true}' [--if-rev N]
                                       (change only these fields, keep the rest;
                                        null removes a field)
  rm      --widget <w> <key> [--if-rev N]
  list    --widget <w> [--prefix p/] [--after <key>] [--limit N] [--all]
  changes --widget <w> --since N [--prefix p/] [--wait 30] [--follow]
  batch   --widget <w> < ops.json      (≤ 1,000 ops, all or nothing, one rev)
          ops: {"op":"put","key":k,"value":v}, {"op":"patch","key":k,"value":{fields}}
               or {"op":"delete","key":k}; each may add "ifRev":N. A conflict or a
               bad patch writes nothing and names the op (its index and key).
  grants  --widget <w>                 (what can I access?)
  grant   --widget <w> --agent <h> | --project <p> --level read|write [--prefix p/]
  revoke  --widget <w> --agent <h> | --project <p> [--prefix p/]
  export  --widget <w> [--at YYYY-MM-DD] > data.jsonl
  blob put <file> --widget <w> [--mime m] [--name n]   (prints the ref; ≤ 16 MiB)
  blob get <ref> --widget <w> --out <file> [--px 256|512]
  blob ls  --widget <w>                (files the widget's data holds)
          A file stays while a key's value names its ref ("k2blob:sha256-…").
  history  --widget <w> [--key k | --prefix p/] [--limit N] [--all]
                                       (every change in the last 7 days, newest first)
  restore  --widget <w> (--rev N | --at 2026-10-26T17:30 | --snapshot <ref>)
           --prefix p/ [--prefix q/] [--except r/] [--dry-run]
                                       (one new change you can undo; an agent always
                                        names --prefix: the whole widget is the person's)
  snapshot --widget <w> [--prefix p/]... [--except r/]... [--label l]
                                       (prints a ref; keep it under a key)
grant/revoke are for a grant admin the person named (k2 zen data admin).
Exit codes: 3 not granted / needs a person, 4 not found or blob_missing, 5 conflict (re-read, retry).
The person's admin commands: k2 zen data --help.
EOF
}

# R10: strip --workspace|--project into global PROJECT for _db_py (D_WS_PATH).
# Remaining argv in _db_ws_args. bash 3.2 + set -u rejects empty arrays.
_db_ws_args=()
_db_take_workspace_flags() {
    _db_ws_args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace|--project)
                if [ $# -lt 2 ] || [ -z "${2:-}" ] || [ "${2:0:1}" = "-" ]; then
                    _db_err "usage" "$1 requires a workspace path, handle, or uuid"
                    exit 2
                fi
                _wiki_apply_project "$2"
                export K2_PROJECT_PATH="$PROJECT"
                shift 2 ;;
            --workspace=*|--project=*)
                local sel="${1#*=}"
                if [ -z "$sel" ]; then
                    _db_err "usage" "${1%%=*} requires a workspace path, handle, or uuid"
                    exit 2
                fi
                _wiki_apply_project "$sel"
                export K2_PROJECT_PATH="$PROJECT"
                shift ;;
            *) _db_ws_args+=("$1"); shift ;;
        esac
    done
}

# `k2 db home [--keep k2.db|k2so.db] [--json]` — where K2 keeps its own
# database (prd-k2-db-rename-v1). Reading: the owner or an Admin. --keep
# (owner only) chooses the copy to keep when two hold workspaces; the
# next K2 start moves the other into ~/.k2/db-quarantine (never deleted).
_db_home() {
    local keep="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --keep) keep="${2:-}"; shift 2 || { _db_err "usage" "--keep needs k2.db or k2so.db"; exit 2; } ;;
            --keep=*) keep="${1#--keep=}"; shift ;;
            --json) as_json=true; shift ;;
            -h|--help|help) cmd_help_db; exit 0 ;;
            *) _db_err "usage" "k2 db home [--keep k2.db|k2so.db] [--json]"; exit 2 ;;
        esac
    done
    local out rc=0
    if [ -n "$keep" ]; then
        case "$keep" in k2.db|k2so.db) ;; *) _db_err "usage" "--keep must be k2.db or k2so.db"; exit 2 ;; esac
        out="$(cli_request_post "/cli/db-home/keep" "keep=$(urlencode "$keep")")" || rc=$?
    else
        out="$(cli_request "/cli/db-home")" || rc=$?
    fi
    [ "$rc" -eq 0 ] || exit "$rc"
    D_OUT="$out" D_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ["D_OUT"]
try:
    v = json.loads(raw)
except Exception:
    print(raw); sys.exit(1)
if os.environ["D_JSON"] == "true":
    print(json.dumps(v, ensure_ascii=False)); sys.exit(0 if v.get("ok", True) else 3)
err = v.get("error")
if err:
    print("Error (%s): %s" % (err.get("code"), err.get("hint")), file=sys.stderr)
    sys.exit(2 if err.get("code") == "usage" else 3)
if "message" in v:
    print(v["message"]); sys.exit(0)
h = v.get("homeDb") or {}
print("K2 database:  %s (%s)" % (h.get("file") or "?", h.get("outcome") or "not started"))
if h.get("reason"): print("why:          %s" % h["reason"])
if h.get("markerAt"): print("switched at:  %s" % h["markerAt"])
if h.get("held"): print("on hold:      %s/db-switch-hold exists (the switch waits)" % h.get("dir", "~/.k2"))
b = h.get("blocked") or {}
if b.get("message"): print("\n" + b["message"])
c = h.get("choice") or {}
if c.get("keep"): print("\nChosen: keep %s (applies at the next K2 start)." % c["keep"])
'
}

# `--db` needs a value (a database name, id or create --id; k2 db list).
_db_need_db_value() {
    if [ $# -lt 2 ] || [ -z "${2:-}" ] || [ "${2:0:1}" = "-" ]; then
        _db_err "usage" "--db needs a database name or id (k2 db list)"
        exit 2
    fi
}

# `k2 db cap [<n>|default] [--workspace WS] [--json]`: show or set how many
# ACTIVE databases a workspace (= its agent) may hold (projects.db_active_cap;
# unset = the default 25; 0 = unlimited; test databases don't count).
# Showing: anyone who can list the workspace's databases. Setting: the
# owner (the owner token, or a person signed in to this server) through
# POST /cli/workspace/set; an agent's passport is refused (exit 3): an
# agent never raises its own cap.
_db_cap() {
    local val="$1" as_json="$2"
    local ws_path="${PROJECT:-}"
    [ -n "$ws_path" ] || ws_path="$(pwd)"
    if [ -z "$val" ]; then
        local out rc=0
        out="$(cli_request "/cli/db/list" "project=$(urlencode "$ws_path")")" || rc=$?
        [ "$rc" -eq 0 ] || exit "$rc"
        D_OUT="$out" D_JSON="$as_json" python3 -c '
import json, os, sys
raw = os.environ["D_OUT"]
try:
    v = json.loads(raw)
except Exception:
    print(raw); sys.exit(1)
err = v.get("error")
if err:
    e = err if isinstance(err, dict) else {"code": "error", "hint": str(err)}
    print(json.dumps({"error": e}), file=sys.stderr)
    sys.exit(3 if e.get("code") in ("forbidden", "owner_only", "unsupported") else 1)
cap = v.get("cap")
if os.environ["D_JSON"] == "true":
    print(json.dumps({"ok": True, "cap": cap, "unlimited": cap == 0})); sys.exit(0)
print("database cap: %s" % ("unlimited (0)" if cap == 0 else cap))
print("the owner sets it: k2 db cap <n> [--workspace WS]   (0 = unlimited, default = back to 25)")
'
        return
    fi
    local stored
    case "$val" in
        default) stored="default" ;;
        *[!0-9]*) _db_err "usage" "k2 db cap takes a whole number (0 = unlimited) or 'default'"; exit 2 ;;
        *)
            [ "${#val}" -le 6 ] || { _db_err "usage" "k2 db cap: at most 999999 (0 = unlimited)"; exit 2; }
            stored="$((10#$val))" ;;
    esac
    local body out rc=0
    body="$(python3 -c 'import json,sys
print(json.dumps({"project": sys.argv[1], "fields": {"db_active_cap": sys.argv[2]}}))' "$ws_path" "$stored")"
    out="$(cli_post_json "/cli/workspace/set" "$body")" || rc=$?
    [ "$rc" -eq 0 ] || { _db_err "error" "could not reach the K2 daemon"; exit 1; }
    D_OUT="$out" D_JSON="$as_json" D_STORED="$stored" python3 -c '
import json, os, sys
raw = os.environ["D_OUT"]
try:
    v = json.loads(raw)
except Exception:
    print(json.dumps({"error": {"code": "error", "hint": raw[:200] or "no answer from the K2 daemon"}}), file=sys.stderr); sys.exit(1)
err = v.get("error")
if err or v.get("ok") is False:
    hint = (err.get("hint") or err.get("code")) if isinstance(err, dict) else str(err or v.get("message") or raw[:200])
    if "token" in hint or v.get("error") == "role_required":
        print(json.dumps({"error": {"code": "owner_only", "hint": "only the owner sets a workspace database cap (an agent never raises its own) — ask your human to run: k2 db cap <n> --workspace <ws>"}}), file=sys.stderr)
        sys.exit(3)
    print(json.dumps({"error": {"code": "usage" if "db_active_cap" in hint else "error", "hint": hint}}), file=sys.stderr)
    sys.exit(2 if "db_active_cap" in hint else 1)
stored = os.environ["D_STORED"]
cap = None if stored == "default" else int(stored)
out = {"ok": True, "cap": 25 if cap is None else cap, "default": cap is None, "unlimited": cap == 0}
if os.environ["D_JSON"] == "true":
    print(json.dumps(out)); sys.exit(0)
print("database cap: %s%s" % ("unlimited (0)" if cap == 0 else out["cap"], " (the default)" if cap is None else ""))
'
}

# `POST /cli/db/clusters` (named Postgres clusters, 0.46.11): print the
# refusal ({"error":{code,hint}} on stderr; exit 2 usage, 3 refused) or the
# answer (`--json` raw).
_db_named_post() {
    local body="$1" as_json="$2" what="$3"
    _compute_split "$(K2_CP_MAX_TIME=900 _compute_send POST /cli/db/clusters "$body")"
    if [ "$_CP_STATUS" != "200" ]; then
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
raw=sys.stdin.read()
try: v=json.loads(raw)
except Exception: v={"error":{"code":"daemon_unreachable","hint":raw[:200] or "no answer from the K2 daemon"}}
e=v.get("error")
if not isinstance(e, dict): e={"code": e or "error", "hint": v.get("hint", "")}
print(json.dumps({"error": {"code": e.get("code"), "hint": e.get("hint", "")}}))' >&2
        case "$_CP_STATUS" in 400) exit 2 ;; 404) exit 1 ;; *) exit 3 ;; esac
    fi
    if [ "$as_json" = true ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_DN_WHAT="$what" python3 -c 'import json,os,sys
v=json.load(sys.stdin)
w=os.environ["K2_DN_WHAT"]
if w == "cluster":
    c=v.get("cluster") or {}
    print("cluster %s on port %s (%s, %s)" % (c.get("label"), c.get("port"), c.get("unit"), c.get("state")))
    if c.get("walSpool"):
        print("WAL archive: %s (a backup ships and prunes it)" % c["walSpool"])
    print("next: k2 db create --cluster %s --name <db>" % (c.get("name")))
elif w == "restart":
    print("restarted %s" % v.get("cluster"))
else:
    print("created %s in %s" % (v.get("name"), v.get("cluster") or "?"))
    if v.get("existing"):
        print("(already there: same --id or name)")
    if v.get("dsn"):
        print("dsn: %s" % v.get("dsn"))
    if v.get("dsnHint"):
        print("no password shown; %s" % v["dsnHint"])'
}

cmd_db() {
    if [ $# -eq 0 ] || _wants_help "$@"; then cmd_help_db; exit 0; fi
    if [ "$1" = "home" ]; then shift; _db_home "$@"; exit $?; fi
    _db_take_workspace_flags "$@"
    if [ ${#_db_ws_args[@]} -eq 0 ]; then cmd_help_db; exit 0; fi
    set -- "${_db_ws_args[@]}"
    local sub="$1"; shift
    local as_json=false health=false yes=false
    local client_id="" name="" out="" file="" dir=""
    case "$sub" in
        status)
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --health) health=true; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            D_VERB="status" D_JSON="$as_json" D_HEALTH="$health" _db_py ;;
        enable)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="enable" D_JSON="$as_json" _db_py ;;
        disable)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="disable" D_JSON="$as_json" _db_py ;;
        uninstall)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="uninstall" D_JSON="$as_json" _db_py ;;
        doctor)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="doctor" D_JSON="$as_json" _db_py ;;
        versions-access)
            # "Allow this agent to manage Postgres versions" (db_upgrade,
            # 0201): the owner turns it on; the owner or an Admin turns it
            # off. Inside a K2 terminal this sends the passport, which the
            # daemon refuses (an agent never sets it, on or off).
            local action="${1:-}"
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    *) _db_err "usage" "k2 db versions-access on|off [--workspace WS] [--json]"; exit 2 ;;
                esac
            done
            case "$action" in
                on|off) ;;
                *) _db_err "usage" "k2 db versions-access on|off [--workspace WS] [--json]"; exit 2 ;;
            esac
            local ws; ws="$(_compute_ws "")"
            _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle=db_upgrade K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
            if $as_json; then printf '%s\n' "$_CP_BODY"; exit 0; fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
on=v["agentsCanUpgradePostgres"]
print("Allow this agent to manage Postgres versions: %s for %s%s" % ("on" if on else "off", v["workspace"], " (check, move, roll back and create --major on its own and its Apps databases; each act is in Access Audit; deleting the old copy stays yours)" if on else ""))'
            ;;
        create)
            local access="" test=false major="" cluster=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --id) client_id="${2:-}"; shift 2 ;;
                    --name) name="${2:-}"; shift 2 ;;
                    --db-access|--access) access="${2:-}"; shift 2 ;;
                    --test) test=true; shift ;;
                    --major) major="${2:-}"; shift 2 ;;
                    --major=*) major="${1#*=}"; shift ;;
                    --cluster) cluster="${2:-}"; shift 2 ;;
                    --cluster=*) cluster="${1#*=}"; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            if [ -n "$cluster" ]; then
                if $test || [ -n "$access" ]; then
                    _db_err "usage" "k2 db create --cluster takes --name, --id and --major only (no --test, no --db-access)"
                    exit 2
                fi
                if [ -n "$major" ]; then
                    case "$major" in [1-9][0-9]) ;; *) _db_err "usage" "--major takes a Postgres major, e.g. 16 or 17"; exit 2 ;; esac
                fi
                _db_named_post "$(_compute_json K2_CJ_op=create-db K2_CJ_project="$(_compute_ws "")" K2_CJ_cluster="$cluster" K2_CJ_name="$name" K2_CJ_id="$client_id" K2_CJ_major_n="$major")" "$as_json" create-db
                exit $?
            fi
            if [ -n "$major" ]; then
                case "$major" in [1-9][0-9]) ;; *) _db_err "usage" "--major takes a Postgres major, e.g. 16 or 17"; exit 2 ;; esac
            fi
            if [ -n "$access" ]; then
                case "$access" in off|read|write) ;; *) _db_err "usage" "--db-access must be off, read, or write"; exit 2 ;; esac
            fi
            if $test && [ -n "$name" ]; then
                _db_err "usage" "k2 db create --test does not take --name"
                exit 2
            fi
            D_VERB="create" D_JSON="$as_json" D_ID="$client_id" D_NAME="$name" D_ACCESS="$access" D_TEST="$test" D_MAJOR="$major" _db_py ;;
        upgrade)
            # `k2 db upgrade` (0.46.3): move a database to a newer Postgres
            # major by copy; the old copy is kept 14 days. 0.46.4: --live
            # (logical replication while it serves), --switch, --abort,
            # --prepare-live <major>, --status. POST/GET /cli/db/upgrade.
            # Exit 0 done, 3 refused, 1 failed.
            local u_db="" u_app="" u_all=false u_to="" u_check=false u_rb=false u_yes=false u_forget=false
            local u_force=false u_now=false u_adopted=false u_ack=false u_wait=true u_allws=false
            local u_live=false u_switch=false u_abort=false u_prep="" u_rif=false u_status=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --app) u_app="${2:-}"; shift 2 ;;
                    --all) u_all=true; shift ;;
                    --all-workspace-dbs) u_allws=true; shift ;;
                    --to) u_to="${2:-}"; shift 2 ;;
                    --to=*) u_to="${1#*=}"; shift ;;
                    --check) u_check=true; shift ;;
                    --rollback) u_rb=true; shift ;;
                    --yes) u_yes=true; shift ;;
                    --forget-old) u_forget=true; shift ;;
                    --force-now) u_force=true; shift ;;
                    --now) u_now=true; shift ;;
                    --allow-adopted) u_adopted=true; shift ;;
                    --acknowledge-failed) u_ack=true; shift ;;
                    --no-wait) u_wait=false; shift ;;
                    --live) u_live=true; shift ;;
                    --switch) u_switch=true; shift ;;
                    --abort) u_abort=true; shift ;;
                    --prepare-live) u_prep="${2:-}"; shift 2 ;;
                    --prepare-live=*) u_prep="${1#*=}"; shift ;;
                    --replica-identity-full) u_rif=true; shift ;;
                    --status) u_status=true; shift ;;
                    -*) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                    *) [ -z "$u_db" ] || { _db_err "usage" "one database"; exit 2; }; u_db="$1"; shift ;;
                esac
            done
            if [ -n "$u_to" ]; then
                case "$u_to" in [1-9][0-9]) ;; *) _db_err "usage" "--to takes a Postgres major, e.g. 17"; exit 2 ;; esac
            fi
            if [ -n "$u_prep" ]; then
                case "$u_prep" in [1-9][0-9]) ;; *) _db_err "usage" "--prepare-live takes a Postgres major, e.g. 16"; exit 2 ;; esac
            fi
            # --status: the database's live run and its latest moves (read-only).
            if $u_status; then
                [ -n "$u_db" ] || { _db_err "usage" "k2 db upgrade <db> --status"; exit 2; }
                _compute_split "$(_compute_send GET "/cli/db/upgrade?db=$(urlencode "$u_db")" "")"
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
                if $as_json; then printf '%s\n' "$_CP_BODY"; exit 0; fi
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
for r in v.get("live") or []:
    t=r.get("tables") or {}
    lag=r.get("lagBytes")
    print("live %s: Postgres %s → %s, %s; tables %s/%s; lag %s; WAL %s" % (r["db"], r["from"], r["to"], r["state"], t.get("ready"), t.get("total"),
          "-" if lag is None else "%d MiB" % (lag >> 20), r.get("walStatus") or "-"))
    if r.get("lastError"): print("  " + r["lastError"])
    if r["state"] == "streaming": print("  ready: k2 db upgrade %s --switch (or --abort)" % r["db"])
for m in (v.get("moves") or [])[:3]:
    print("%s %s %s → %s: %s (%s)%s" % (m.get("kind", "move"), m.get("method", "copy"), m.get("from"), m.get("to"), m.get("state"), m.get("step"),
          ("  " + m["lastError"]) if m.get("lastError") else ""))
if not (v.get("live") or v.get("moves")): print("no moves and no live move for this database")'
                exit 0
            fi
            local u_body
            u_body="$(_compute_json K2_CJ_db="$u_db" K2_CJ_app="$u_app" K2_CJ_all_b="$($u_all && echo 1)" \
                K2_CJ_to_n="$u_to" K2_CJ_check_b="$($u_check && echo 1)" K2_CJ_rollback_b="$($u_rb && echo 1)" \
                K2_CJ_yes_b="$($u_yes && echo 1)" K2_CJ_forgetOld_b="$($u_forget && echo 1)" K2_CJ_forceNow_b="$($u_force && echo 1)" \
                K2_CJ_now_b="$($u_now && echo 1)" K2_CJ_allowAdopted_b="$($u_adopted && echo 1)" \
                K2_CJ_acknowledgeFailed_b="$($u_ack && echo 1)" K2_CJ_allWorkspaceDbs_b="$($u_allws && echo 1)" \
                K2_CJ_live_b="$($u_live && echo 1)" K2_CJ_switch_b="$($u_switch && echo 1)" K2_CJ_abort_b="$($u_abort && echo 1)" \
                K2_CJ_prepareLive_n="$u_prep" K2_CJ_replicaIdentityFull_b="$($u_rif && echo 1)")"
            _compute_split "$(K2_CP_MAX_TIME=900 _compute_send POST /cli/db/upgrade "$u_body")"
            if [ "$_CP_STATUS" != "200" ]; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
raw=sys.stdin.read()
try: v=json.loads(raw)
except Exception: v={"error":"daemon_unreachable","hint":raw[:200] or "no answer from the K2 daemon"}
print(json.dumps({"error": v.get("error"), "hint": v.get("hint", "")}))' >&2
                case "$_CP_STATUS" in 400) exit 2 ;; *) exit 3 ;; esac
            fi
            if $as_json && { ! $u_wait || $u_check || $u_rb || $u_forget || $u_ack || $u_abort || [ -n "$u_prep" ] || { $u_live && ! $u_check; }; }; then printf '%s\n' "$_CP_BODY"; exit 0; fi
            # A live run started, aborted, or a cluster prepared: one answer.
            if { $u_live && ! $u_check; } || $u_abort || [ -n "$u_prep" ]; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
r=v.get("live")
if r:
    print("live move of %s: Postgres %s → %s, %s" % (r["db"], r["from"], r["to"], r["state"]))
    if r["state"] in ("syncing", "streaming"):
        print("copying while it serves; schema changes wait until the switch (at most 7 days)")
        print("watch: k2 db upgrade %s --status; then k2 db upgrade %s --switch (or --abort)" % (r["db"], r["db"]))
    elif r.get("lastError"):
        print("  " + r["lastError"])
else:
    print("Postgres %s: wal_level %s%s" % (v.get("major"), v.get("walLevel"), " (restarted once)" if v.get("restarted") else " (nothing restarted)"))'
                exit 0
            fi
            if $u_check || $u_rb || $u_forget || $u_ack; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
mark={"ok":"ok    ","warn":"warn  ","refuse":"REFUSE"}
refused=False
for r in v.get("check", []):
    if "error" in r:
        print("%s: %s" % (r.get("db"), r["error"])); refused=True; continue
    print("%s: Postgres %s → %s (about %s–%s s pause)" % (r["db"], r["from"], r["to"], r["pauseSecs"][0], r["pauseSecs"][1]))
    for l in r["lines"]:
        print("  %s %-10s %s%s" % (mark.get(l["level"], l["level"]), l["id"], l["text"], ("  — " + l["hint"]) if l.get("hint") else ""))
        refused = refused or l["level"] == "refuse"
if "check" not in v:
    print(json.dumps(v, indent=2))
sys.exit(3 if refused else 0)'
                exit $?
            fi
            # A move (or a live switch): watch it until it ends.
            local u_name="$u_db"
            [ -n "$u_name" ] || u_name="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v.get("switching") or v["started"][0])')"
            if ! $u_wait; then printf '%s\n' "$_CP_BODY"; exit 0; fi
            if $u_switch; then
                echo "switching $u_name (writes pause a few seconds; the old copy stays 14 days)" >&2
            else
                echo "moving $u_name (the old copy stays 14 days; k2 db upgrade $u_name --rollback --yes)" >&2
            fi
            local u_seen="" u_out u_rc
            while true; do
                sleep 3
                _compute_split "$(_compute_send GET "/cli/db/upgrade?db=$(urlencode "$u_name")" "")"
                [ "$_CP_STATUS" = "200" ] || continue
                u_out="$(printf '%s' "$_CP_BODY" | U_SEEN="$u_seen" U_JSON="$as_json" python3 -c 'import json,os,sys
v=json.load(sys.stdin)
m=(v.get("moves") or [None])[0]
if not m: print("wait"); sys.exit(0)
step="%s/%s" % (m["step"], m["state"])
if step != os.environ["U_SEEN"] and os.environ["U_JSON"] != "true":
    print("  " + step, file=sys.stderr)
end=m["state"] in ("succeeded","aborted","rolled_back","failed")
if end:
    if os.environ["U_JSON"] == "true": print(json.dumps(m), file=sys.stderr)
    else:
        if m.get("lastError"): print("  " + m["lastError"], file=sys.stderr)
        for w in m.get("warnings", []): print("  warn: " + w, file=sys.stderr)
        for d in m.get("diff", [])[:20]: print("  diff: " + d, file=sys.stderr)
    print("end:" + m["state"])
else:
    print("seen:" + step)')"
                case "$u_out" in
                    end:succeeded) exit 0 ;;
                    end:aborted|end:rolled_back) exit 3 ;;
                    end:*) exit 1 ;;
                    seen:*) u_seen="${u_out#seen:}" ;;
                esac
            done ;;
        clusters)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; --health) health=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="clusters" D_JSON="$as_json" D_HEALTH="$health" _db_py ;;
        cluster)
            # Named clusters (0.46.11, prd-named-pg-clusters-v1): POST
            # /cli/db/clusters over the cell socket (an agent's switch is
            # checked as local there) or the owner token.
            local c_op="${1:-}"; shift 2>/dev/null || true
            local c_major="" c_name="" c_port="" c_wal=""
            case "$c_op" in
                create)
                    c_major="${1:-}"; shift 2>/dev/null || true
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --json) as_json=true; shift ;;
                            --name) c_name="${2:-}"; shift 2 ;;
                            --name=*) c_name="${1#*=}"; shift ;;
                            --port) c_port="${2:-}"; shift 2 ;;
                            --port=*) c_port="${1#*=}"; shift ;;
                            --wal-archive) c_wal=1; shift ;;
                            *) _db_err "usage" "unexpected '$1' — k2 db cluster create <major> --name <name> [--port <port>] [--wal-archive]"; exit 2 ;;
                        esac
                    done
                    case "$c_major" in [1-9][0-9]) ;; *) _db_err "usage" "k2 db cluster create <major> --name <name> (e.g. k2 db cluster create 17 --name feedback)"; exit 2 ;; esac
                    [ -n "$c_name" ] || { _db_err "usage" "k2 db cluster create <major> --name <name>"; exit 2; }
                    if [ -n "$c_port" ]; then case "$c_port" in 54[3-4][0-9]) ;; *) _db_err "usage" "--port takes a port in 5433-5449"; exit 2 ;; esac; fi
                    _db_named_post "$(_compute_json K2_CJ_op=create K2_CJ_project="$(_compute_ws "")" K2_CJ_major_n="$c_major" K2_CJ_name="$c_name" K2_CJ_port_n="$c_port" K2_CJ_walArchive_b="$c_wal")" "$as_json" cluster ;;
                restart)
                    c_name="${1:-}"; shift 2>/dev/null || true
                    while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
                    [ -n "$c_name" ] || { _db_err "usage" "k2 db cluster restart <name>"; exit 2; }
                    _db_named_post "$(_compute_json K2_CJ_op=restart K2_CJ_project="$(_compute_ws "")" K2_CJ_name="$c_name")" "$as_json" restart ;;
                *) _db_err "usage" "k2 db cluster create <major> --name <name> [--port <port>] [--wal-archive] | k2 db cluster restart <name>"; exit 2 ;;
            esac ;;
        default-major)
            local major="${1:-}"; shift || true
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            case "$major" in [1-9][0-9]) ;; *) _db_err "usage" "k2 db default-major <major> (e.g. 17)"; exit 2 ;; esac
            D_VERB="default_major" D_JSON="$as_json" D_MAJOR="$major" _db_py ;;
        list)
            while [ $# -gt 0 ]; do case "$1" in --json) as_json=true; shift ;; *) _db_err "usage" "unexpected '$1'"; exit 2 ;; esac; done
            D_VERB="list" D_JSON="$as_json" _db_py ;;
        dsn|creds)
            # 0.46.11 (field report F1): no password unless --reveal.
            local test=false actor="" vref=false db_sel="" reveal=false ref_as=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --test) test=true; shift ;;
                    --ref) vref=true; shift ;;
                    --reveal) reveal=true; shift ;;
                    --as) ref_as="${2:-}"; shift 2 || { _db_err "usage" "--as needs a name"; exit 2; } ;;
                    --as=*) ref_as="${1#*=}"; shift ;;
                    --actor) actor="${2:-}"; shift 2 ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            if [ -n "$actor" ] && [ "$actor" != "migrator" ]; then
                _db_err "usage" "--actor must be migrator"
                exit 2
            fi
            if $reveal && $vref; then
                _db_err "usage" "--ref keeps the DSN in the vault and prints its reference; --reveal prints the DSN. Pick one"
                exit 2
            fi
            D_VERB="dsn" D_JSON="$as_json" D_TEST="$test" D_ACTOR="$actor" D_REF="$vref" D_DB="$db_sel" D_REVEAL="$reveal" D_AS="$ref_as" _db_py ;;
        run)
            # 0.46.11 (F1): `k2 db run [--db X] [--test [--actor migrator]]
            # [--as NAME] -- <cmd…>`: the command gets the DSN as $NAME
            # (DATABASE_URL), never on argv or stdout (as k2 vault run).
            local test=false actor="" db_sel="" env_as="DATABASE_URL"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --) shift; break ;;
                    --test) test=true; shift ;;
                    --actor) actor="${2:-}"; shift 2 ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    --as) env_as="${2:-}"; shift 2 ;;
                    *) _db_err "usage" "k2 db run [--db <name>] [--test [--actor migrator]] [--as NAME] -- <command…> (unexpected '$1')"; exit 2 ;;
                esac
            done
            [ $# -gt 0 ] || { _db_err "usage" "k2 db run … -- <command…>: name the command after --"; exit 2; }
            if [[ ! "$env_as" =~ ^[A-Z_][A-Z0-9_]*$ ]]; then _db_err "usage" "--as takes an env name like DATABASE_URL"; exit 2; fi
            if [ -n "$actor" ] && [ "$actor" != "migrator" ]; then _db_err "usage" "--actor must be migrator"; exit 2; fi
            local _run_out _run_url
            _run_out="$(D_VERB="dsn" D_JSON=true D_TEST="$test" D_ACTOR="$actor" D_DB="$db_sel" D_REVEAL=true _db_py)" || exit $?
            _run_url="$(printf '%s' "$_run_out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("dsn") or "")')"
            [ -n "$_run_url" ] || { _db_err "engine" "the daemon answered no DSN"; exit 1; }
            export "${env_as}=${_run_url}"
            unset _run_out _run_url
            exec "$@" ;;
        migrate)
            local db_sel=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --dir) dir="${2:-}"; shift 2 ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            D_VERB="migrate" D_JSON="$as_json" D_DIR="$dir" D_DB="$db_sel" _db_py ;;
        dump)
            local db_sel=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --out) out="${2:-}"; shift 2 ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            D_VERB="dump" D_JSON="$as_json" D_OUT="$out" D_DB="$db_sel" _db_py ;;
        restore)
            local db_sel=""
            file=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    -*) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                    *) [ -z "$file" ] || { _db_err "usage" "unexpected '$1'"; exit 2; }; file="$1"; shift ;;
                esac
            done
            [ -n "$file" ] || { _db_err "usage" "k2 db restore <file> [--db <name>]"; exit 2; }
            D_VERB="restore" D_JSON="$as_json" D_FILE="$file" D_DB="$db_sel" _db_py ;;
        drop)
            local test=false db_sel=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --yes) yes=true; shift ;;
                    --test) test=true; shift ;;
                    --db) _db_need_db_value "$@"; db_sel="$2"; shift 2 ;;
                    --db=*) db_sel="${1#*=}"; shift ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            D_VERB="drop" D_JSON="$as_json" D_YES="$yes" D_TEST="$test" D_DB="$db_sel" _db_py ;;
        cap)
            # `k2 db cap [<n>|default] [--workspace WS]` (0.46.10): how many
            # ACTIVE databases a workspace (= its agent) may hold. Reading:
            # anyone who can read the workspace. Setting: the owner only
            # (owner token, outside an agent's K2 terminal); an agent's
            # passport is refused (exit 3): an agent never raises its own cap.
            local cap_val=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    -h|--help|help) cmd_help_db; exit 0 ;;
                    -*) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                    *) [ -z "$cap_val" ] || { _db_err "usage" "k2 db cap [<n>|default] [--workspace WS]"; exit 2; }; cap_val="$1"; shift ;;
                esac
            done
            _db_cap "$cap_val" "$as_json" ;;
        grant)
            local grantee="" db_spec="" level="" manage=false
            local relations=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --db) db_spec="${2:-}"; shift 2 ;;
                    --level) level="${2:-}"; shift 2 ;;
                    --manage) manage=true; shift ;;
                    --relation)
                        if [ $# -lt 2 ] || [ -z "${2:-}" ] || [ "${2:0:1}" = "-" ]; then
                            _db_err "usage" "--relation requires schema.name or a bare name"
                            exit 2
                        fi
                        relations+=("$2")
                        shift 2 ;;
                    --relation=*)
                        local rel="${1#*=}"
                        if [ -z "$rel" ]; then
                            _db_err "usage" "--relation requires schema.name or a bare name"
                            exit 2
                        fi
                        relations+=("$rel")
                        shift ;;
                    -*) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                    *)
                        if [ -z "$grantee" ]; then grantee="$1"; shift
                        else _db_err "usage" "unexpected extra argument '$1'"; exit 2; fi ;;
                esac
            done
            [ -n "$grantee" ] || { _db_err "usage" "k2 db grant <workspace> --db … --level read|write [--relation schema.name …] [--manage]"; exit 2; }
            [ -n "$db_spec" ] || { _db_err "usage" "grant requires --db <id|name>"; exit 2; }
            [ -n "$level" ] || { _db_err "usage" "grant requires --level read|write"; exit 2; }
            case "$level" in read|write) ;; *) _db_err "usage" "--level must be read or write"; exit 2 ;; esac
            local relations_json
            relations_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' ${relations[@]+"${relations[@]}"})
            D_VERB="grant" D_JSON="$as_json" D_GRANTEE="$grantee" D_DB="$db_spec" D_LEVEL="$level" D_MANAGE="$manage" D_RELATIONS="$relations_json" _db_py ;;
        revoke)
            local grantee="" db_spec=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --db) db_spec="${2:-}"; shift 2 ;;
                    -*) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                    *)
                        if [ -z "$grantee" ]; then grantee="$1"; shift
                        else _db_err "usage" "unexpected extra argument '$1'"; exit 2; fi ;;
                esac
            done
            [ -n "$grantee" ] || { _db_err "usage" "k2 db revoke <workspace> --db …"; exit 2; }
            [ -n "$db_spec" ] || { _db_err "usage" "revoke requires --db <id|name>"; exit 2; }
            D_VERB="revoke" D_JSON="$as_json" D_GRANTEE="$grantee" D_DB="$db_spec" _db_py ;;
        bind)
            local role="" db_spec=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --role) role="${2:-}"; shift 2 ;;
                    --db) db_spec="${2:-}"; shift 2 ;;
                    *) _db_err "usage" "unexpected '$1'"; exit 2 ;;
                esac
            done
            [ -n "$role" ] || { _db_err "usage" "k2 db bind --role <pg_role>"; exit 2; }
            D_VERB="bind" D_JSON="$as_json" D_ROLE="$role" D_DB="$db_spec" _db_py ;;
        --help|-h) cmd_help_db; exit 0 ;;
        *) _db_err "usage" "unknown db command '$sub' — see k2 db --help"; exit 2 ;;
    esac
}

