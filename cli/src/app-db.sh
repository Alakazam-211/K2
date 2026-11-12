# ── App egress (runtime S15) and held widenings ──────────────────────
# A guarded App reaches only the hosts on its egress allowlist, through
# its own proxy. New hosts are held for the owner (every caller, the owner
# token included); narrowing is immediate.

# ── k2 app db (0.46.11, prd-app-database-integrations-v1 S2) ─────────
# Connect an App to a box Postgres database end to end: POST
# /cli/app/db/connect (and status/check/disconnect). Exit codes (R7): 0
# done (PASS, or the check skipped: the App isn't running), 1 failed (FAIL,
# or a usage error, code usage), 2 held (one card for the owner, nothing
# changed yet), 3 refused. Only ids and names are printed, never a URL.

_app_db_usage() {
    cat <<'EOF'
k2 app db — connect an App to a Postgres database on this box

  k2 app db connect <app-id> --new [--major N] [--cluster NAME] [--name DB]
  k2 app db connect <app-id> --db <name|id>
        [--as NAME_URL] [--no-migrate-url] [--wait [--timeout 10m]] [--reason TEXT] [--json]
      --new         a new database for this App (connecting --new again is the
                    same one). --major: a Postgres major this box runs
                    (k2 app db status <app-id> lists them). --cluster: a named
                    cluster of this workspace (k2 db clusters). Connect never
                    installs Postgres: a major or cluster this box doesn't run
                    is refused (app_db_cluster_missing, exit 3) with the
                    command that makes it.
      --db          a database the App's workspace owns or was granted
                    (k2 db list).
      --as          the env name, ending in _URL (default DATABASE_URL). Each
                    name holds one database: a second database needs another
                    name (--as REPORTS_URL) or a disconnect first
                    (app_db_already_connected, exit 3).
      What it does: makes the App its own login (never your agent's), grants
      the App DATABASE_URL and, only on a database its workspace owns, the
      migrator's DATABASE_MIGRATE_URL (--no-migrate-url: not that one; with
      --as REPORTS_URL they are REPORTS_URL and REPORTS_MIGRATE_URL), opens
      the App's database door, re-hosts the App (it starts again with the new
      env; people signed in to an App that keeps sessions in memory sign in
      again), then checks it from inside the App: select 1 as the App. In the
      App, read process.env.DATABASE_URL. No URL or password is ever printed.
      Who waits: the owner and Admins connect at once. An agent (a K2 tab)
      connects at once only when every switch it needs is on and the
      database is its own workspace's; otherwise connect files ONE card for
      the owner naming each switch that is off by its Settings name and
      exits 2 with nothing changed. Run the "Ask with:" line it prints, then
      wait: --wait blocks (default 10m; exit 2 if still waiting), or
      k2 app db status <app-id>. Running connect again doesn't nudge: it is
      the same card.
  k2 app db status <app-id> [--json]   its databases, last checks, waiting
                                       cards, and the majors this box runs
  k2 app db check <app-id> [--as NAME_URL] [--json]
                                       the live check again (no card needed)
  k2 app db disconnect <app-id> [--as NAME_URL] [--drop-roles] [--json]
                                       its login stops, open sessions end, the
                                       door shuts, its secrets go, it is
                                       re-hosted. The database is never dropped.

What connect prints (PASS):
  connected feedback → ws_…_app_feedback (Postgres 17, cluster 17/main)
  secrets  DATABASE_URL, DATABASE_MIGRATE_URL (granted to the App; not shown)
  check    PASS · PostgreSQL 17.6 · 3 ms · from inside the App
A FAIL ends with "check    FAIL · <step>: <error>", a "fix" line and the App's
last output lines (steps: k2 study app-databases, THE CHECK).

Exit codes: 0 done (PASS) · 1 FAIL (a "check FAIL" line), or a usage error
({"error":"usage",…} on stderr) · 2 held for the owner (one card) · 3 refused
({"error":"<code>",…} and a fix line; k2 explain <code>).
More: k2 study app-databases (which App kinds reach what, outside databases).
EOF
}

# Print a connect/check answer (stdin); exit 1 on FAIL.
_app_db_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
app = v.get("app", "")
pre = "app-db.%s." % app
def env(n):
    return n[len(pre):] if n.startswith(pre) else n
c = v.get("check") or {}
if "created" in v or "rehost" in v:
    print("connected %s → %s (Postgres %s, cluster %s)%s" % (app, v.get("db"), v.get("major"), v.get("cluster"), "  [new]" if v.get("created") else ""))
else:
    print("%s → %s (Postgres %s, cluster %s)" % (app, v.get("db"), v.get("major"), v.get("cluster")))
print("roles    %s" % ", ".join(v.get("roles") or []))
print("secrets  %s (granted to the App; not shown)" % ", ".join(env(s) for s in v.get("secrets") or []))
kind = v.get("kind")
door = "open" if v.get("doorOpen") else "SHUT"
where = {"container": "container door, in-wall 127.0.0.1:5432", "process-exception": "its nft rule, the cluster port", "process": "the daemon network"}.get(kind, kind)
print("door     %s (%s)" % (door, where))
for r in v.get("replaced") or []:
    print("replaced %s (it was granted under the same name)" % env(r))
if v.get("note"):
    print("note     %s" % v["note"])
rh = v.get("rehost") or {}
if rh.get("result") == "done":
    print("rehost   done (%s)" % rh.get("deployment"))
elif rh.get("result") == "not_running":
    print("rehost   not needed: it is not running")
elif rh.get("result") == "failed":
    print("rehost   FAILED: %s" % rh.get("error"))
for s in v.get("switchesTurnedOn") or []:
    print("switch   %s turned on" % s)
if c.get("ok") is True:
    print("check    PASS · %s · %s ms · from %s" % (c.get("version") or "?", c.get("latencyMs"), "inside the App" if c.get("from") == "app" else "the daemon"))
    sys.exit(0)
if c.get("ok") is None:
    print("check    skipped: %s" % (c.get("note") or c.get("skipped") or "not run"))
    sys.exit(0)
print("check    FAIL · %s: %s" % (c.get("step"), c.get("error")))
if c.get("hint"):
    print("fix      %s" % c["hint"])
out = c.get("appOutput") or []
if out:
    print("App output (last %d lines, untrusted):" % len(out))
    for l in out:
        print("  | " + l)
sys.exit(1)'
}

# A held connect (stdin): print it.
_app_db_held_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("request") or {}
print("held for the owner: %s (one card)" % r.get("id"))
print("  %s" % (r.get("summary") or v.get("diff", {}).get("lines", [""])[0]))
for s in v.get("switches") or []:
    if not s.get("on"):
        print("  switch off: \"%s\" (%s)" % (s.get("name"), s.get("where")))
for l in (v.get("diff") or {}).get("lines") or []:
    if "belongs to workspace" in l:
        print("  " + l)
if v.get("reused"):
    print("  this is your earlier card %s, still waiting; nothing new was filed" % r.get("id"))
print("  " + (v.get("hint") or ""))'
}

cmd_app_db() {
    local sub="${1:-}"
    case "$sub" in
        connect|status|check|disconnect) shift ;;
        ""|-h|--help|help) _app_db_usage; return 0 ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"k2 app db connect|status|check|disconnect <app-id> (k2 app db --help)\"}" >&2; exit 1 ;;
    esac
    local app="" json_out=0 new=0 db="" major="" cluster="" name="" as="" nomig=0 wait=0 timeout=600 drop=0 reason=""
    local bad=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --new) new=1; shift ;;
            --db|--major|--cluster|--name|--as|--timeout|--reason)
                if [ $# -lt 2 ] || [ -z "${2:-}" ]; then bad="$1 needs a value"; break; fi
                case "$1" in
                    --db) db="$2" ;; --major) major="$2" ;; --cluster) cluster="$2" ;; --name) name="$2" ;;
                    --as) as="$2" ;; --reason) reason="$2" ;; --timeout) timeout="$(_compute_secs "$2")" ;;
                esac
                shift 2 ;;
            --db=*) db="${1#*=}"; shift ;;
            --major=*) major="${1#*=}"; shift ;;
            --cluster=*) cluster="${1#*=}"; shift ;;
            --as=*) as="${1#*=}"; shift ;;
            --no-migrate-url) nomig=1; shift ;;
            --wait) wait=1; shift ;;
            --drop-roles) drop=1; shift ;;
            -h|--help) _app_db_usage; return 0 ;;
            -*) bad="unknown flag $1"; break ;;
            *) if [ -z "$app" ]; then app="$1"; else bad="one App at a time (got $1 too)"; break; fi; shift ;;
        esac
    done
    [ -n "$bad" ] || [ -n "$app" ] || bad="name the App: k2 app db $sub <app-id>"
    if [ -z "$bad" ] && [ -n "$major" ]; then
        case "$major" in [1-9][0-9]) ;; *) bad="--major takes a Postgres major, like 17" ;; esac
    fi
    if [ -n "$bad" ]; then
        printf '{"error":"usage","hint":%s}\n' "$(printf '%s' "$bad (k2 app db --help)" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" >&2
        exit 1
    fi
    local body
    case "$sub" in
        status)
            _compute_split "$(_compute_send GET /cli/app/db/status "app=$(urlencode "$app")")"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s  %s · %s" % (v["app"], v.get("kind"), "running" if v.get("running") else "not running"))
if not v.get("postgres"):
    print("  this box runs no Postgres for Apps (Linux only): use SQLite in $K2_APP_DATA")
cs = v.get("connections") or []
if not cs:
    print("  no database connected (k2 app db connect %s --new | --db <name>)" % v["app"])
for c in cs:
    k = c.get("check") or {}
    res = "PASS %s ms" % k.get("latencyMs") if k.get("ok") is True else ("FAIL %s" % k.get("step") if k.get("ok") is False else "not checked")
    print("  %-22s → %s (Postgres %s, %s)  login %s  door %s  check %s" % (c["as"], c["db"], c.get("major"), c.get("cluster"),
          (c.get("roles") or ["?"])[0], "open" if c.get("doorOpen") else "SHUT", res))
for p in v.get("pending") or []:
    print("  waiting for the owner: card %s (%s)" % (p["id"], p["summary"]))
print("  majors K2 runs here: %s" % ", ".join(str(m) for m in v.get("majors") or []))'
            ;;
        check)
            body="$(_compute_json K2_CJ_app="$app" K2_CJ_as="$as")"
            K2_CP_MAX_TIME=300 _app_rt_post /cli/app/db/check "$body"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$_CP_BODY"
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(1 if (json.load(sys.stdin).get("check") or {}).get("ok") is False else 0)'
                return $?
            fi
            printf '%s' "$_CP_BODY" | _app_db_print
            ;;
        disconnect)
            body="$(_compute_json K2_CJ_app="$app" K2_CJ_as="$as" K2_CJ_dropRoles_b="$drop")"
            K2_CP_MAX_TIME=300 _app_rt_post /cli/app/db/disconnect "$body"
            [ "$_CP_STATUS" = 200 ] || _app_db_fail
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$_CP_BODY"
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("ok") else 1)'
                return $?
            fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("disconnected %s from %s (%s)" % (v["app"], v["db"], v["as"]))
print("login    %s stopped; %s open session(s) cut%s" % (", ".join(v.get("roles") or []), v.get("sessionsCut", 0), "; roles dropped" if v.get("rolesDropped") else ""))
print("door     %s" % ("SHUT" if not v.get("doorOpen") else "still open (another connection uses it)"))
print("secrets  removed: %s" % (", ".join(s.split(".")[-1] for s in v.get("secrets") or []) or "none"))
print("note     %s" % v.get("note", ""))
e = v.get("error")
if e:
    print("error    %s" % e.get("hint"), file=sys.stderr)
    sys.exit(1)'
            ;;
        connect)
            if [ "$new" = 0 ] && [ -z "$db" ]; then
                echo '{"error":"usage","hint":"say which database: --new or --db <name> (k2 app db --help)"}' >&2; exit 1
            fi
            body="$(_compute_json K2_CJ_app="$app" K2_CJ_new_b="$new" K2_CJ_db="$db" K2_CJ_major_n="$major" K2_CJ_cluster="$cluster" \
                K2_CJ_name="$name" K2_CJ_as="$as" K2_CJ_noMigrateUrl_b="$nomig" K2_CJ_reason="$reason")"
            K2_CP_MAX_TIME=900 _app_rt_post /cli/app/db/connect "$body"
            case "$_CP_STATUS" in
                200)
                    if [ "$json_out" = 1 ]; then
                        printf '%s\n' "$_CP_BODY"
                        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(1 if (json.load(sys.stdin).get("check") or {}).get("ok") is False else 0)'
                        return $?
                    fi
                    printf '%s' "$_CP_BODY" | _app_db_print
                    return $?
                    ;;
                202) ;;
                *) _app_db_fail ;;
            esac
            local id
            id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
            if [ "$wait" = 0 ]; then
                if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | _app_db_held_print; fi
                return 2
            fi
            [ "$json_out" = 1 ] || printf '%s' "$_CP_BODY" | _app_db_held_print >&2
            echo "waiting for the owner (card $id, up to $((timeout / 60)) min)…" >&2
            local waited=0 st
            while [ "$waited" -lt "$timeout" ]; do
                sleep 5; waited=$((waited + 5))
                _compute_split "$(_compute_send GET /cli/app/db/status "app=$(urlencode "$app")&request=$(urlencode "$id")")"
                [ "$_CP_STATUS" = 200 ] || continue
                st="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("request") or {}).get("status") or "")')"
                case "$st" in
                    pending|"") continue ;;
                    approved)
                        local res
                        res="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["request"]["result"]))')"
                        if [ "$json_out" = 1 ]; then
                            printf '%s\n' "$res"
                            printf '%s' "$res" | python3 -c 'import json,sys; sys.exit(1 if (json.load(sys.stdin).get("check") or {}).get("ok") is False else 0)'
                            return $?
                        fi
                        printf '%s' "$res" | _app_db_print
                        return $? ;;
                    failed)
                        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin)["request"]; e=r.get("result") or {}; print(json.dumps({"error": e.get("code","failed"), "hint": "approved, but connecting failed: %s" % e.get("hint","")}))' >&2
                        return 1 ;;
                    *)
                        echo "{\"error\":\"card_$st\",\"hint\":\"the owner's card $id ended $st: nothing changed\"}" >&2
                        return 3 ;;
                esac
            done
            echo "{\"error\":\"wait_timeout\",\"hint\":\"card $id is still waiting for the owner (k2 app db status $app)\"}" >&2
            return 2
            ;;
    esac
}

# A refused k2 app db call: usage → exit 1 (R7), else _compute_fail (3).
_app_db_fail() {
    if [ "$_CP_STATUS" = 400 ]; then
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    e = json.load(sys.stdin).get("error") or {}
except Exception:
    e = {}
print(json.dumps({"error": e.get("code", "usage"), "hint": e.get("hint", "")}))' >&2
        exit 1
    fi
    _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

_app_egress_usage() {
    cat <<'EOF'
k2 app egress — what a guarded App may reach on the internet

  k2 app egress <app-id> [--json]      Mode, hosts (allowed / pending / seen,
                                       with counts), pending requests, proxy.
  k2 app egress add <app-id> <host[:port]>... [--reason TEXT] [--json]
                                       Ask for hosts (443 by default; *.suffix
                                       for any name under a suffix). Always
                                       held for the owner: exit 2 with the
                                       request id until they approve.
  k2 app egress rm <app-id> <host[:port]> [--json]
                                       Take a host off at once (open tunnels to
                                       it are cut). No approval needed.
  k2 app egress mode <app-id> allowlist|broad [--reason TEXT]
                                       allowlist applies at once; broad is the
                                       open path's (held for the owner).
  k2 app egress log <app-id> [--tail N] [--json]
                                       The last connections: host, port, result,
                                       bytes (never paths or queries).

  k2 app approvals [--app A] [--all] [--json]
                                       Held requests (an agent sees its own
                                       workspace's).
  k2 app approve <request-id>          Owner, in person: Touch ID or your
                                       password, outside K2. Agents never can.
  k2 app deny <request-id> [--reason TEXT]
                                       Owner/admin denies; the requester
                                       withdraws its own.

Always refused, even if listed: private, local, link-local and metadata
addresses, this box's own names (reach K2 through K2_GATEWAY_URL), and a
TLS name that isn't the CONNECT name. A manifest's egress.hosts are asked
for at deploy. Keys never go through this proxy (use the vault).

Exit codes: 0 ok · 2 usage or held (pending the owner) · 3 refused/infra.
EOF
}

_app_held_exit() {
    # stdin: a held answer. Print it; exit 2 (loopable) unless it is allowed
    # (now, under the agent's own switch, or from before); 3 if applying failed.
    python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("status") == "failed":
    r = v.get("result") if isinstance(v.get("result"), dict) else {}
    print("not applied: %s" % (r.get("hint") or r.get("code") or "applying it failed"), file=sys.stderr)
    sys.exit(3)
if v.get("status") != "held":
    new = v.get("added") or [x.get("name") for x in (v.get("services") or []) if isinstance(x, dict) and x.get("name")]
    already = v.get("alreadyAllowed") or []
    if new:
        print("allowed: " + ", ".join(new))
    if already:
        print("already allowed: " + ", ".join(already))
    r = v.get("result") if isinstance(v.get("result"), dict) else {}
    note = r.get("note") or (v.get("hint") if new or not already else None)
    if note:
        print("  " + note)
    sys.exit(0)
r = v["request"]
print("held for the owner: %s" % r["id"])
print("  %s" % r["summary"])
if v.get("alreadyAllowed"):
    print("  already allowed: " + ", ".join(v["alreadyAllowed"]))
if v.get("reused"):
    print("  this is your earlier request %s (asked %s ago), still pending; nothing new was filed" % (r["id"], v.get("askedAgo") or "a while"))
c = v.get("card") if isinstance(v.get("card"), dict) else {}
if c.get("where"):
    print("  owner: %s" % c["where"])
print("  " + v.get("hint", ""))
sys.exit(2)'
}

cmd_app_egress() {
    local sub="${1:-}"
    case "$sub" in
        add|rm|remove|mode|log) shift ;;
        ""|-h|--help|help) _app_egress_usage; return 0 ;;
        *) sub="show" ;;
    esac
    local app="" json_out=0 reason="" tail="" hosts=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --reason) reason="${2:-}"; shift 2 ;;
            --tail) tail="${2:-}"; shift 2 ;;
            -h|--help) _app_egress_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app egress --help)" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else hosts+=("$1"); fi; shift ;;
        esac
    done
    [ -z "$app" ] && { _app_egress_usage >&2; exit 2; }
    case "$sub" in
        show)
            [ "${#hosts[@]}" -eq 0 ] || { echo "usage: k2 app egress <app-id> (to ask for a host: k2 app egress add <app-id> <host>)" >&2; exit 2; }
            _compute_call GET /cli/app/egress "app=$(urlencode "$app")"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s  egress %s%s" % (v["app"], v["mode"], "" if v.get("enforced") else "  (NOT enforced: process engine)"))
print("  " + v.get("note", ""))
hosts = v.get("hosts") or []
if not hosts:
    print("  no hosts: this App reaches nothing outside K2")
for h in hosts:
    hp = ("[%s]:%s" % (h["host"], h["port"])) if ":" in h["host"] else ("%s:%s" % (h["host"], h["port"]))
    extra = "  request " + h["requestId"] if h.get("requestId") else ""
    print("  %-8s %-40s %-10s hits %-6s refused %s%s" % (h["status"], hp, h["source"], h["hits"], h["refused"], extra))
for r in v.get("pending") or []:
    print("  pending  %s  %s (by %s)" % (r["id"], r["summary"], r["requestedBy"]))
p = v.get("proxy") or {}
if p.get("listening"):
    print("  proxy    127.0.0.1:%s  %s open, %s total, %s refused" % (p["port"], p["active"], p["connections"], p["refused"]))
else:
    print("  proxy    not listening (it opens when the guarded App starts)")'
            ;;
        add)
            [ "${#hosts[@]}" -gt 0 ] || { echo "usage: k2 app egress add <app-id> <host[:port]>... [--reason TEXT]" >&2; exit 2; }
            local hj
            hj="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${hosts[@]}")"
            _app_rt_post /cli/app/egress/add "$(_compute_json K2_CJ_app="$app" K2_CJ_hosts_j="$hj" K2_CJ_reason="$reason")"
            # 202 = held for the owner (the body says status: held).
            case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then
                printf '%s\n' "$_CP_BODY"
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(2 if json.load(sys.stdin).get("status") == "held" else 0)'
                return $?
            fi
            local rc=0
            printf '%s' "$_CP_BODY" | _app_held_exit || rc=$?
            if [ "$rc" = 2 ] && ! _compute_in_session && [ -t 0 ] && [ "${K2_NO_APPROVE:-}" != "1" ]; then
                # The owner at a terminal outside K2: approve right here.
                local id
                id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
                _app_approve "$id"
                return 0
            fi
            return "$rc"
            ;;
        rm|remove)
            [ "${#hosts[@]}" -eq 1 ] || { echo "usage: k2 app egress rm <app-id> <host[:port]>" >&2; exit 2; }
            _app_rt_post /cli/app/egress/rm "$(_compute_json K2_CJ_app="$app" K2_CJ_host="${hosts[0]}")"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(("removed %s (at once)" % v["host"]) if v.get("removed") else v.get("hint", "not on the list"))'
            ;;
        mode)
            [ "${#hosts[@]}" -eq 1 ] || { echo "usage: k2 app egress mode <app-id> allowlist|broad" >&2; exit 2; }
            _app_rt_post /cli/app/egress/mode "$(_compute_json K2_CJ_app="$app" K2_CJ_mode="${hosts[0]}" K2_CJ_reason="$reason")"
            case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; [ "$_CP_STATUS" = 202 ] && return 2; return 0; fi
            if printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("status") == "held" else 1)'; then
                printf '%s' "$_CP_BODY" | _app_held_exit
                return $?
            fi
            echo "$app: egress mode $(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["mode"])')"
            ;;
        log)
            local q="app=$(urlencode "$app")"
            [ -n "$tail" ] && q="${q}&tail=$(urlencode "$tail")"
            _compute_call GET /cli/app/egress/log "$q"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
for l in json.load(sys.stdin).get("lines") or []:
    at = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(l.get("at", 0)))
    hp = "%s:%s" % (l.get("host", "-"), l.get("port", "-"))
    if l.get("result") == "ok":
        print("%s  ok       %-6s %-40s up %s down %s  %sms (%s)" % (at, l.get("kind", ""), hp, l.get("up"), l.get("down"), l.get("ms"), l.get("end")))
    else:
        print("%s  %-8s %-6s %-40s %s" % (at, l.get("result"), l.get("kind", ""), hp, l.get("code")))'
            ;;
    esac
}

