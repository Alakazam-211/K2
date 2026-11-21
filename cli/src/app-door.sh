# ── The 0.46.1 door (lane 2: S19, AR10) ──────────────────────────────
# k2 app suspend|resume|audit|requests|quota, k2 app token rotate, k2 app remote-pass.
# Exit codes: 0 done, 2 held for the owner (or usage), 3 refused or infra.

_app_door_usage() {
    cat <<'EOF'
%include cli/text/help/app/door.txt
EOF
}

cmd_app_suspend() {
    local app="" reason="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app suspend <app-id> [--reason TEXT]" >&2; exit 2; }
    _compute_call POST /cli/app/suspend "$(_compute_json K2_CJ_app="$app" K2_CJ_reason="$reason")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s suspended%s: %s passes revoked, deploys frozen" % (v["app"], " (already)" if v.get("already") else "", v.get("passesRevoked")))
st = v.get("stopped")
if isinstance(st, dict):
    print("  not stopped yet: %s" % st.get("hint"))
print("  %s" % v.get("hint", ""))'
}

cmd_app_resume() {
    local app="" start=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --start) start=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app resume <app-id> [--start]   (the owner, outside K2)" >&2; exit 2; }
    if _compute_in_session; then
        echo '{"error":{"code":"owner_only","hint":"only the owner resumes a suspended App: k2 app resume <app> from a terminal outside K2"}}' >&2
        exit 3
    fi
    _compute_call POST /cli/app/resume "$(_compute_json K2_CJ_app="$app" K2_CJ_start_b="$start")" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("already"):
    print("%s: %s" % (v["app"], v["already"])); sys.exit(0)
print("%s resumed" % v["app"])
if v.get("deployment"):
    print("  starting: deployment %s (k2 app deployment %s)" % (v["deployment"], v["app"]))
elif v.get("hint") or v.get("note"):
    print("  %s" % (v.get("hint") or v.get("note")))'
}

cmd_app_audit() {
    local app="" since="" verb="" denied=0 limit="" verify=0 json_out=0 oldest=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --oldest-first) oldest=1; shift ;;
            --newest-first) oldest=0; shift ;;
            --since) _need_value "$@"; since="${2:-}"; shift 2 ;;
            --verb) _need_value "$@"; verb="${2:-}"; shift 2 ;;
            --denied) denied=1; shift ;;
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --verify) verify=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app audit <app-id> [--since 1h] [--verb V] [--denied] [--limit N] [--oldest-first] [--verify] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ "$oldest" = 1 ] && q="${q}&oldest=1"
    [ -n "$since" ] && q="${q}&since=$(urlencode "$since")"
    [ -n "$verb" ] && q="${q}&verb=$(urlencode "$verb")"
    [ -n "$limit" ] && q="${q}&limit=$(urlencode "$limit")"
    [ "$denied" = 1 ] && q="${q}&denied=1"
    [ "$verify" = 1 ] && q="${q}&verify=1"
    _compute_call GET /cli/app/audit "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
t = v.get("totals") or {}
print("%s: %s calls, %s refused, %s B in, %s B out" % (v["app"], t.get("calls"), t.get("denied"), t.get("bytesIn"), t.get("bytesOut")))
calls = v.get("calls") or []
if calls:
    order = v.get("order") or "newest first"
    flip = "--newest-first" if order == "oldest first" else "--oldest-first"
    print("%d call%s, %s (%s to flip):" % (len(calls), "" if len(calls) == 1 else "s", order, flip))
for c in calls:
    at = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(c["atMs"] / 1000))
    code = (" " + c["code"]) if c.get("code") else ""
    print("%s  %3s%s  %s  %s  in %s out %s  %s ms" % (at, c.get("result"), code, c.get("actor"), c.get("verb"), c.get("bytesIn"), c.get("bytesOut"), c.get("latencyMs")))
for d in v.get("chain") or []:
    print("chain %s: %s" % (d["day"], ("ok, %s rows" % d["rows"]) if d.get("ok") else ("BROKEN at row %s" % d.get("brokenAt"))))'
    fi
    # --verify: a broken chain is a failed check (exit 1).
    if [ "$verify" = 1 ]; then
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if all(d.get("ok") for d in json.load(sys.stdin).get("chain") or []) else 1)'
    fi
}

# k2 app requests: visitors' requests to an App, one row each (the
# forwarder's per-request log; `GET /cli/app/requests`, apps/requests.rs).
cmd_app_requests() {
    local app="" since="" until="" status="" path="" method="" limit="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --since) _need_value "$@"; since="${2:-}"; shift 2 ;;
            --until) _need_value "$@"; until="${2:-}"; shift 2 ;;
            --status) _need_value "$@"; status="${2:-}"; shift 2 ;;
            --path) _need_value "$@"; path="${2:-}"; shift 2 ;;
            --method) _need_value "$@"; method="${2:-}"; shift 2 ;;
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app requests <app-id> [--since 1h] [--until 10m] [--status 4xx|5xx|N] [--path /prefix] [--method POST] [--limit N] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$since" ] && q="${q}&since=$(urlencode "$since")"
    [ -n "$until" ] && q="${q}&until=$(urlencode "$until")"
    [ -n "$status" ] && q="${q}&status=$(urlencode "$status")"
    [ -n "$path" ] && q="${q}&path_prefix=$(urlencode "$path")"
    [ -n "$method" ] && q="${q}&method=$(urlencode "$method")"
    [ -n "$limit" ] && q="${q}&limit=$(urlencode "$limit")"
    _compute_call GET /cli/app/requests "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
app = v["app"]
t = v.get("totals") or {}
rows = v.get("requests") or []
def size(n):
    n = n or 0
    if n < 1024:
        return str(n)
    for unit in ("K", "M", "G"):
        n = n / 1024.0
        if n < 1024 or unit == "G":
            return ("%.1f%s" % (n, unit)) if n < 10 else ("%d%s" % (n, unit))
print("%s: %s request%s (%s ok, %s 4xx, %s 5xx; %s answered by K2, %s closed early)" % (
    app, t.get("requests", 0), "" if t.get("requests") == 1 else "s", t.get("ok", 0), t.get("clientErrors", 0),
    t.get("serverErrors", 0), t.get("k2Answered", 0), t.get("closedEarly", 0)))
if not rows:
    print("No visitor request reached this App in that window. A request K2 never got (DNS, the relay, the")
    print("browser) has no row; widen it with --since 1d. Rows are kept %s days." % (v.get("keep") or {}).get("days", 7))
else:
    print("%-19s  %-7s  %-34s  %-28s  %6s  %6s  %6s" % ("TIME", "METHOD", "PATH", "STATUS", "IN", "OUT", "MS"))
    for r in rows:
        at = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(r["atMs"] / 1000))
        st = str(r["status"]) if r.get("status") else "-"
        if r.get("k2Code"):
            st += " K2 " + r["k2Code"]
        if r.get("ws"):
            st += " ws " + r["ws"]
        if r.get("closedEarly"):
            st += " (" + r["closedEarly"] + " closed)"
        path = r.get("path") or "-"
        if len(path) > 34:
            path = path[:33] + "~"
        print("%-19s  %-7s  %-34s  %-28s  %6s  %6s  %6s" % (at, r.get("method") or "-", path, st, size(r.get("bytesIn")), size(r.get("bytesOut")), r.get("totalMs")))
    print("newest first; status - means no answer came. Kept %s days, at most %s rows per App." % (
        (v.get("keep") or {}).get("days", 7), (v.get("keep") or {}).get("rowsPerApp")))
if v.get("dropped"):
    print("note: %s row(s) were dropped since K2 started (the log queue was full)" % v["dropped"])'
}

cmd_app_quota() {
    local app="" json_out=0 reset=0 reason="" rps="" burst="" sockets="" body="" upload="" subs="" any=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --rps) _need_value "$@"; rps="${2:-}"; any=1; shift 2 ;;
            --burst) _need_value "$@"; burst="${2:-}"; any=1; shift 2 ;;
            --sockets) _need_value "$@"; sockets="${2:-}"; any=1; shift 2 ;;
            --body) _need_value "$@"; body="${2:-}"; any=1; shift 2 ;;
            --upload) _need_value "$@"; upload="${2:-}"; any=1; shift 2 ;;
            --subscriptions) _need_value "$@"; subs="${2:-}"; any=1; shift 2 ;;
            --reset) reset=1; any=1; shift ;;
            --reason) _need_value "$@"; reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app quota <app-id> [--rps N] [--burst N] [--sockets N] [--body 8M] [--upload 100M] [--subscriptions N] [--reset]" >&2; exit 2; }
    local n
    for n in "$rps" "$burst" "$sockets" "$subs"; do
        case "$n" in ''|*[!0-9]*) [ -z "$n" ] || { echo "usage: --rps/--burst/--sockets/--subscriptions take whole numbers" >&2; exit 2; } ;; esac
    done
    if [ "$any" = 1 ]; then
        _app_rt_post /cli/app/quota/set "$(_compute_json K2_CJ_app="$app" K2_CJ_rps_n="$rps" K2_CJ_burst_n="$burst" \
            K2_CJ_sockets_n="$sockets" K2_CJ_bodyBytes="$body" K2_CJ_uploadBytes="$upload" K2_CJ_subscriptions_n="$subs" \
            K2_CJ_reset_b="$([ "$reset" = 1 ] && echo 1)" K2_CJ_reason="$reason")"
        _app_grace_done "$json_out"
    else
        _compute_call GET /cli/app/quota "app=$(urlencode "$app")"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
q = v.get("quotas") or {}
o = v.get("override") or {}
def mark(k):
    return " (override)" if k in o else ""
print("%s quotas:" % v["app"])
print("  calls/s        %s sustained, bursts of %s%s" % (q.get("rps"), q.get("burst"), mark("rps") or mark("burst")))
print("  sockets        %s%s" % (q.get("sockets"), mark("sockets")))
print("  body           %s bytes (uploads %s)%s" % (q.get("bodyBytes"), q.get("uploadBytes"), mark("bodyBytes") or mark("uploadBytes")))
print("  subscriptions  %s%s" % (q.get("subscriptions"), mark("subscriptions")))
live = v.get("live")
if live:
    print("  now            %s connections, %s subscriptions" % (live.get("connections"), live.get("subscriptions")))
if v.get("raised"):
    print("  raised: " + ", ".join(v["raised"]))'
}

# k2 app members|room-names|self|usage — room-scope R9/R16 (0.46.1). The same
# surfaces Apps reach through their gateway (room.members, app.self,
# app.usage) and Gardens through the bridge; here for agents and the owner.
_app_surfaces_usage() {
    cat <<'EOF'
%include cli/text/help/app/surfaces.txt
EOF
}

