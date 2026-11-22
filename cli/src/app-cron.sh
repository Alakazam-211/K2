# ── k2 app cron (prd-app-supabase-compat-v1 §11.2) ───────────────────
# The daemon calls an App's own route on a schedule, headless, with
# `Authorization: Bearer <CRON_SECRET>` (a vault secret the App also gets
# as env): /cli/app/cron (list), /cli/app/cron/add|rm|run-now (POST),
# /cli/app/cron/logs. Exit codes: 0 done, 1 usage or a failed run-now, 3
# refused.

_app_cron_usage() {
    cat <<'EOF'
%include cli/text/help/app/cron.txt
EOF
}

_app_cron_bad() {
    printf '{"error":"usage","hint":%s}\n' "$(printf '%s' "$1 (k2 app cron --help)" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" >&2
    exit 1
}

cmd_app_cron() {
    local sub="${1:-}"
    case "$sub" in
        add|list|ls|rm|remove|run-now|logs) shift ;;
        ""|-h|--help|help) _app_cron_usage; return 0 ;;
        *) _app_cron_bad "k2 app cron add|list|rm|run-now|logs <app-id>" ;;
    esac
    case "$sub" in ls) sub=list ;; remove) sub=rm ;; esac
    local app="" json_out=0 schedule="" path="" method="" missed="" timeout="" id="" limit=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --path) _need_value "$@"; path="${2:-}"; shift 2 ;;
            --path=*) path="${1#*=}"; shift ;;
            --method) _need_value "$@"; method="${2:-}"; shift 2 ;;
            --method=*) method="${1#*=}"; shift ;;
            --missed) _need_value "$@"; missed="${2:-}"; shift 2 ;;
            --missed=*) missed="${1#*=}"; shift ;;
            --timeout) _need_value "$@"; timeout="${2:-}"; shift 2 ;;
            --timeout=*) timeout="${1#*=}"; shift ;;
            --limit) _need_value "$@"; limit="${2:-}"; shift 2 ;;
            --limit=*) limit="${1#*=}"; shift ;;
            -h|--help) _app_cron_usage; return 0 ;;
            -*) _app_cron_bad "unknown flag $1" ;;
            *)
                if [ -z "$app" ]; then app="$1"
                elif [ "$sub" = add ] && [ -z "$schedule" ]; then schedule="$1"
                elif [ "$sub" != add ] && [ -z "$id" ]; then id="$1"
                else _app_cron_bad "unexpected '$1'"
                fi
                shift ;;
        esac
    done
    [ -n "$app" ] || _app_cron_bad "name the App: k2 app cron $sub <app-id>"
    case "$sub" in
        add)
            [ -n "$schedule" ] && [ -n "$path" ] || _app_cron_bad "k2 app cron add <app-id> \"<min hour day month weekday>\" --path /api/cron/x"
            _app_rt_post /cli/app/cron/add "$(_compute_json K2_CJ_app="$app" K2_CJ_schedule="$schedule" K2_CJ_path="$path" K2_CJ_method="$method" K2_CJ_missed="$missed" K2_CJ_timeout="$timeout")" ;;
        rm|run-now)
            [ -n "$id" ] || _app_cron_bad "k2 app cron $sub <app-id> <cron-id> (k2 app cron list <app-id>)"
            if [ "$sub" = run-now ]; then
                K2_CP_MAX_TIME=3700 _app_rt_post /cli/app/cron/run-now "$(_compute_json K2_CJ_app="$app" K2_CJ_id="$id")"
            else
                _app_rt_post /cli/app/cron/rm "$(_compute_json K2_CJ_app="$app" K2_CJ_id="$id")"
            fi ;;
        list)
            _compute_split "$(_compute_send GET /cli/app/cron "app=$(urlencode "$app")")" ;;
        logs)
            local q="app=$(urlencode "$app")"
            [ -z "$id" ] || q="$q&id=$(urlencode "$id")"
            [ -z "$limit" ] || q="$q&limit=$(urlencode "$limit")"
            _compute_split "$(_compute_send GET /cli/app/cron/logs "$q")" ;;
    esac
    [ "$_CP_STATUS" = 200 ] || _app_db_fail
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("ok", True) else 1)'
        return
    fi
    printf '%s' "$_CP_BODY" | K2_CRON_SUB="$sub" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
sub = os.environ["K2_CRON_SUB"]
def line(c):
    last = c.get("last") or {}
    tail = ""
    if last:
        tail = "  last %s %s%s" % (last.get("at"), last.get("status"), " (HTTP %s)" % last["http"] if last.get("http") else "")
    print("%s  %-13s %-4s %s  next %s%s%s" % (c.get("id"), c.get("schedule"), c.get("method"), c.get("path"), c.get("next") or "-",
        "  RUNNING" if c.get("running") else "", tail))
    if c.get("failuresInARow"):
        print("    %s failures in a row" % c["failuresInARow"])
if sub == "add":
    line(v)
    s = v.get("secret") or {}
    rh = (s.get("rehost") or {}).get("result")
    if s.get("made"):
        print("CRON_SECRET made and granted to the App (not shown)%s" % ({"done": "; the App was re-hosted to get it", "not_running": "; it gets it when it starts"}.get(rh, "; re-host: %s" % rh)))
    else:
        print("CRON_SECRET: the one the App already gets")
    print("each call sends Authorization: Bearer $CRON_SECRET; the App checks it")
elif sub == "list":
    cs = v.get("crons") or []
    if not cs:
        print("no crons on %s (k2 app cron add %s \"*/15 * * * *\" --path /api/cron/x)" % (v.get("app"), v.get("app")))
    for c in cs:
        line(c)
elif sub == "rm":
    print("removed %s from %s (%s)" % (v.get("removed"), v.get("app"), v.get("note")))
elif sub == "run-now":
    print("%s %s: %s%s%s" % (v.get("app"), v.get("id"), v.get("status"), " (HTTP %s)" % v["http"] if v.get("http") else "", "  %s" % v["detail"] if v.get("detail") else ""))
    if not v.get("ok"):
        sys.exit(1)
elif sub == "logs":
    for e in v.get("logs") or []:
        print("%s %s" % (e.get("id"), e.get("path")))
        for r in e.get("runs") or []:
            print("  %s  %-15s %-9s %s%s%s" % (r.get("at"), r.get("status"), r.get("trigger"), "HTTP %s " % r["http"] if r.get("http") else "",
                "%s ms " % r["ms"] if r.get("ms") is not None else "", r.get("detail") or ""))
'
}

# k2 study app-cron.
_study_page_app_cron() {
    cat <<'EOF'
%include cli/text/study/app-cron.txt
EOF
}
