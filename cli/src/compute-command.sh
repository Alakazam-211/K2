cmd_compute() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _compute_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        # k2 compute node packages --help: its own page (0.46.9).
        if [ "$sub" = "node" ] && { [ "${1:-}" = "packages" ] || [ "${1:-}" = "pkg" ]; }; then
            _compute_node_packages_usage
            return 0
        fi
        # k2 compute git-access --help: its own page (0.46.9).
        if [ "$sub" = "git-access" ]; then
            _compute_git_access_usage
            return 0
        fi
        _compute_usage
        return 0
    fi
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$sub" in
        nodes|ls)
            # Outside a K2 tab, name this checkout's workspace so each row
            # shows its grant (a passport's comes from the passport).
            local nq="" nws
            nws="$(_compute_ws "")"
            [ -n "$nws" ] && nq="workspace=$(urlencode "$nws")"
            _compute_call GET /cli/compute/nodes "$nq"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render nodes
            ;;
        info)
            [ -n "${1:-}" ] || { echo "usage: k2 compute info <node>" >&2; exit 2; }
            _compute_call GET /cli/compute/nodes/info "node=$(urlencode "$1")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render info
            ;;
        run) cmd_compute_run "$@" ;;
        # CI seats on shared nodes (0.46.10 C1/C3a).
        roster|lanes|seats|trust|owners) cmd_compute_trust "$sub" "$@" ;;
        retier|tier) cmd_compute_retier "$@" ;;
        move) cmd_compute_move "$@" ;;
        allow-nodes) cmd_compute_allow_nodes "$@" ;;
        git-access) cmd_compute_git_access "$@" ;;
        cache) cmd_compute_cache "$@" ;;
        queue)
            [ -n "${1:-}" ] && [ "${1#-}" = "$1" ] || { echo "usage: k2 compute queue <node> [--json]" >&2; exit 2; }
            _compute_call GET /cli/compute/queue "node=$(urlencode "$1")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render queue
            ;;
        # `list` is the same verb (agents reach for it); `ls` stays `nodes`.
        jobs|list)
            local q="" n=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --workspace) q="$q&workspace=$(urlencode "${2:-}")"; shift 2 ;;
                    --mine) q="$q&mine=1"; shift ;;
                    # 0.46.7 (CE-9): the job's own --tag.
                    --tag) q="$q&tag=$(urlencode "${2:-}")"; shift 2 ;;
                    --tier)
                        case "${2:-}" in
                            ci|priority|normal|batch) q="$q&tier=$(urlencode "${2:-}")" ;;
                            standard) q="$q&tier=normal" ;;
                            *) echo "usage: --tier ci|priority|standard|batch" >&2; exit 2 ;;
                        esac
                        shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute jobs [--node N] [--mine] [--state S] [--tier T] [--tag T] [--limit N] [--workspace WS] [--json]" >&2; exit 2 ;;
                esac
            done
            n="${q#&}"
            _compute_call GET /cli/compute/jobs "$n"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render jobs
            ;;
        get|status)
            # `status` with no job: every node at a glance; `status <job>` = get.
            if [ "$sub" = "status" ] && { [ -z "${1:-}" ] || [ "${1#-}" != "$1" ]; }; then
                cmd_compute_status "$@"
            fi
            [ -n "${1:-}" ] || { echo "usage: k2 compute get <job> [--full] [--json]" >&2; exit 2; }
            local gfull=0 ga
            for ga in "$@"; do [ "$ga" = "--full" ] && gfull=1; done
            _compute_call_nf 4 GET /cli/compute/jobs/get "job=$(urlencode "$1")"
            # --json: the one result object (compute B), as run --json prints.
            [ "$as_json" = "1" ] && { printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result"), indent=2))'; exit 0; }
            K2_CP_FULL="$gfull" _compute_render get
            ;;
        fetch) cmd_compute_fetch "$@" ;;
        test) cmd_compute_test "$@" ;;
        prewarm) cmd_compute_prewarm "$@" ;;
        attach)
            local job="" since="" raw=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --since) since="${2:-}"; shift 2 ;;
                    --raw) raw=1; shift ;;
                    *) job="$1"; shift ;;
                esac
            done
            [ -n "$job" ] || { echo "usage: k2 compute attach <job> [--since SEQ] [--raw]" >&2; exit 2; }
            _compute_call GET /cli/compute/jobs/get "job=$(urlencode "$job")"
            job="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
            _compute_follow "$job" "$raw" "seq:${since:-0}"
            ;;
        logs) cmd_compute_logs "$@" ;;
        # prd-compute-blacksmith-class-v1 R1: --mem / --cpus from history.
        rightsize) cmd_compute_rightsize "$@" ;;
        stats) cmd_compute_stats "$@" ;;
        wait) cmd_compute_wait "$@" ;;
        cancel)
            # 0.46.1: a job that already left the queue is stopped only with
            # --force (the CLI sends ifState: queued without it; 409
            # job_started → exit 3). --force goes before or after the id.
            # 0.46.7 (CE-20): --all-mine cancels this session's jobs that
            # match (--workspace-all: every job of the workspace); queued
            # only unless --force.
            local cjob="" force=0 scope="" ctag="" cstate="" cnode="" cws="" ifq=0
            local cusage="usage: k2 compute cancel <job> [--force|--if-queued]  |  k2 compute cancel --all-mine|--workspace-all [--tag T] [--state queued|running|any] [--node N] [--workspace WS] [--force]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --force|-f) force=1; shift ;;
                    # 0.46.7: cancel only while queued; a started job is
                    # reported and left running, exit 0 (for scripts).
                    --if-queued) ifq=1; shift ;;
                    --all-mine) scope="session"; shift ;;
                    --workspace-all) scope="workspace"; shift ;;
                    --tag) ctag="${2:-}"; shift 2 ;;
                    --state) cstate="${2:-}"; shift 2 ;;
                    --node) cnode="${2:-}"; shift 2 ;;
                    --workspace) cws="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$cusage (unknown flag '$1')" >&2; exit 2 ;;
                    *)
                        [ -z "$cjob" ] || { echo "$cusage (one job)" >&2; exit 2; }
                        cjob="$1"; shift ;;
                esac
            done
            if [ -n "$scope" ]; then
                [ -z "$cjob" ] || { echo "$cusage (a job id or --all-mine, not both)" >&2; exit 2; }
                case "${cstate:-queued}" in queued|running|any) ;; *) echo "$cusage (--state is queued, running or any)" >&2; exit 2 ;; esac
                if [ "${cstate:-queued}" != "queued" ] && [ "$force" != "1" ]; then
                    echo "[k2 compute] --state ${cstate} stops started jobs only with --force; without it they are listed as skipped" >&2
                fi
                local filt
                filt="$(K2_CF_SCOPE="$scope" K2_CF_TAG="$ctag" K2_CF_STATE="$cstate" K2_CF_NODE="$cnode" K2_CF_WS="$(_compute_ws "$cws")" python3 -c '
import json, os
f = {"scope": os.environ["K2_CF_SCOPE"]}
for k, e in (("tag", "K2_CF_TAG"), ("state", "K2_CF_STATE"), ("node", "K2_CF_NODE"), ("workspace", "K2_CF_WS")):
    if os.environ.get(e):
        f[k] = os.environ[e]
print(json.dumps(f))')"
                local fb=0
                [ "$force" = "1" ] && fb=1
                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_filter_j="$filt" K2_CJ_force_b="$fb")"
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
c = v.get("cancelled") or []
print("cancelled %d of %d matching job(s)%s" % (len(c), v.get("matched", 0), (": " + " ".join(x["job"] for x in c)) if c else ""))
for s in v.get("skipped") or []:
    print("  skipped %s: %s (%s)" % (s.get("job"), s.get("code"), s.get("hint")))'
                exit 0
            fi
            [ -n "$cjob" ] || { echo "$cusage" >&2; exit 2; }
            [ "$ifq" = "1" ] && [ "$force" = "1" ] && { echo "$cusage (--if-queued or --force, not both)" >&2; exit 2; }
            local if_state="queued"
            [ "$force" = "1" ] && if_state=""
            if [ "$ifq" = "1" ]; then
                local craw
                craw="$(_compute_send POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState=queued)")"
                _compute_split "$craw"
                if [ "$_CP_STATUS" = "409" ] && printf '%s' "$_CP_BODY" | grep -Eq '"code":"(job_started|already_ended)"'; then
                    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                    printf '%s' "$_CP_BODY" | K2_CP_JOB="$cjob" python3 -c '
import json, os, sys
e = (json.load(sys.stdin).get("error") or {})
job = os.environ["K2_CP_JOB"][:8]
if e.get("code") == "already_ended":
    print("not cancelled: %s already ended (%s)" % (job, e.get("hint")))
else:
    print("not cancelled: %s already started; left running (k2 compute cancel %s --force stops it)" % (job, job))'
                    exit 0
                fi
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            else
                _compute_call POST /cli/compute/jobs/cancel "$(_compute_json K2_CJ_job="$cjob" K2_CJ_ifState="$if_state")"
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | K2_CP_JOB="$cjob" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
job = os.environ["K2_CP_JOB"][:8]
node = v.get("node") or "?"
def took(s):
    if s is None: return ""
    s = int(s)
    return " for %dm%02ds" % (s // 60, s % 60)
if v.get("state") == "cancelled" and v.get("was") == "queued":
    print("cancelled %s (was queued on %s)" % (job, node))
elif v.get("state") == "cancel_requested" and v.get("onNode"):
    print("cancel asked for %s (queued on %s; %s): %s" % (job, node, v.get("cancel"), v.get("note") or ""))
elif "cancel" in v:
    print("stopping %s (was %s on %s%s; cancel %s)" % (job, v.get("was") or v.get("state"), node, took(v.get("ranSecs")), v["cancel"]))
else:
    print(v.get("state", "ok"))'
            ;;
        retry)
            [ -n "${1:-}" ] || { echo "usage: k2 compute retry <job>" >&2; exit 2; }
            _compute_call POST /cli/compute/jobs/retry "$(_compute_json K2_CJ_job="$1")"
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "job" in v: print("retrying as attempt %s: k2 compute attach %s" % (v["job"]["attempt"], v["job"]["short"]))
else: print(v.get("state", "ok"))'
            ;;
        receipt|provenance|verify) cmd_compute_provenance "$sub" "$@" ;;
        node) cmd_compute_node "$@" ;;
        grant) cmd_compute_grant "$@" ;;
        revoke)
            local node="" workspace=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] && [ -n "$workspace" ] || { echo "usage: k2 compute revoke <node> --workspace WS" >&2; exit 2; }
            _compute_call POST /cli/compute/grants/revoke "$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$workspace")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(("revoked %s for %s. " % (v["node"], v["workspace"]) if v["removed"] else "no grant to revoke. ") + v["note"])'
            ;;
        grants)
            local q=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --workspace) q="$q&workspace=$(urlencode "${2:-}")"; shift 2 ;;
                    *) shift ;;
                esac
            done
            _compute_call GET /cli/compute/grants "${q#&}"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render grants
            ;;
        preview)
            # The owner's switch for the whole preview (CN30, `computePreview`).
            # Not a compute route, so it works while compute is off. Owner
            # only: run it from a plain terminal, not a K2 tab (a tab sends
            # its session passport and the daemon refuses).
            local action="${1:-status}"
            case "$action" in
                on|off)
                    _compute_call POST /cli/settings/update "{\"computePreview\":$([ "$action" = on ] && echo true || echo false)}" tcp
                    ;;
                status) _compute_call GET /cli/settings/get "" tcp ;;
                *) echo "usage: k2 compute preview on|off|status" >&2; exit 2 ;;
            esac
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("Compute nodes (preview): %s on this server" % ("on" if v.get("computePreview") else "off"))'
            ;;
        placement)
            # How auto-placement ranks nodes (0.46.6, `computePlacement`):
            # v2 (default) or v1 (the 0.46.5 order). The owner's switch,
            # like preview: a plain terminal, not a K2 tab.
            local action="${1:-status}"
            case "$action" in
                v1|v2) _compute_call POST /cli/settings/update "{\"computePlacement\":\"$action\"}" tcp ;;
                status) _compute_call GET /cli/settings/get "" tcp ;;
                *) echo "usage: k2 compute placement v1|v2|status" >&2; exit 2 ;;
            esac
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
p = json.load(sys.stdin).get("computePlacement") or "v2"
what = {"v2": "load bands, a warm slot worth one band, bursts spread", "v1": "the 0.46.5 order: warm slot first, then free CPU"}.get(p, p)
print("Compute placement: %s on this server (%s)" % (p, what))'
            ;;
        access)
            local action="${1:-status}" workspace="" all=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    --all-machines) all=1; shift ;;
                    *) echo "usage: k2 compute access on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off)
                    # The Agent tab's "Allow compute nodes" (the single
                    # control, Rosson 2026-10-11): on also grants every
                    # machine switched on (and ones added later); off removes
                    # the workspace's grants. --all-machines is accepted for
                    # old scripts; it's what plain on/off does now.
                    : "$all"
                    _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
on=v["agentsCanUseCompute"]
m=v.get("granted", []) + v.get("kept", [])
tail=((": " + ", ".join(m)) if m else ": no machines switched on yet (ones added later are granted)") if on else (" (%d grants removed)" % v.get("grantsRemoved", 0) if "grantsRemoved" in v else "")
print("Allow compute nodes: %s for %s%s" % ("on" if on else "off", v["workspace"], tail))'
                    ;;
                *) echo "usage: k2 compute access on|off --workspace WS" >&2; exit 2 ;;
            esac
            ;;
        operate)
            # Node admin (Q14): "Allow this agent to operate compute nodes".
            local action="${1:-}" workspace=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute operate on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off) ;;
                *) echo "usage: k2 compute operate on|off --workspace WS" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute_operate K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("Allow this agent to operate compute nodes: %s for %s%s" % ("on" if v["agentsCanOperateCompute"] else "off", v["workspace"], " (narrow, pause, update, restart; root actions still wait for you)" if v["agentsCanOperateCompute"] else ""))'
            ;;
        manage)
            # "Allow managing compute nodes" (0194): owner only. Inside a K2
            # terminal this sends the passport, which the daemon refuses
            # (an agent never turns it on, for itself or anyone).
            local action="${1:-}" workspace=""
            shift 2>/dev/null || true
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) workspace="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute manage on|off --workspace WS" >&2; exit 2 ;;
                esac
            done
            [ -z "$workspace" ] && workspace="$(_compute_ws "")"
            case "$action" in
                on|off) ;;
                *) echo "usage: k2 compute manage on|off --workspace WS" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$workspace" K2_CJ_toggle=compute_manage K2_CJ_value_b="$([ "$action" = on ] && echo 1 || echo 0)")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
on=v["agentsCanManageCompute"]
print("Allow managing compute nodes: %s for %s%s" % ("on" if on else "off", v["workspace"], " (node add/confirm/rename/pause/retire/route, grant/revoke; each act is in Access Audit; preview and node remove stay yours)" if on else ""))'
            ;;
        usage)
            local days=7
            [ "${1:-}" = "--days" ] && days="${2:-7}"
            _compute_call GET /cli/compute/usage "days=$(urlencode "$days")" tcp
            _compute_render usage
            ;;
        events)
            # 0.46.7 (compute feedback part 2, item 4): your jobs' events
            # (queued, started, finished with exit, requeued, moved), so an
            # agent follows them without polling logs. --json = NDJSON.
            # --audit: the owner's audit log (what `events` was before).
            local limit="" audit=0 efollow=0 emine=0 etag="" esince="" emax=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --audit) audit=1; shift ;;
                    --limit) limit="${2:-}"; shift 2 ;;
                    --follow|-f) efollow=1; shift ;;
                    --mine) emine=1; shift ;;
                    --tag) etag="${2:-}"; shift 2 ;;
                    --since) esince="${2:-}"; shift 2 ;;
                    --max-wait) emax="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute events [--follow] [--mine] [--tag T] [--since CURSOR] [--limit N] [--max-wait D] [--json]  |  k2 compute events --audit [--limit N]" >&2; exit 2 ;;
                esac
            done
            if [ "$audit" = "1" ]; then
                _compute_call GET /cli/compute/events "limit=$(urlencode "${limit:-50}")" tcp
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                _compute_render events
                exit 0
            fi
            local eq="events=1" ews edeadline=""
            [ "$emine" = "1" ] && eq="$eq&mine=1"
            [ -n "$etag" ] && eq="$eq&tag=$(urlencode "$etag")"
            ews="$(_compute_ws "")"
            [ -n "$ews" ] && eq="$eq&workspace=$(urlencode "$ews")"
            if [ -n "$emax" ]; then local emw; emw="$(_compute_secs "$emax")" || exit 2; edeadline=$(( $(date +%s) + emw )); fi
            local cursor="$esince" first=1 eout
            while :; do
                local q="$eq" w=0
                if [ -n "$cursor" ]; then
                    q="$q&after=$(urlencode "$cursor")"
                elif [ "$first" = "1" ] && [ "$efollow" = "0" ]; then
                    q="$q&last=$(urlencode "${limit:-20}")"
                fi
                if [ "$efollow" = "1" ]; then
                    w=20
                    if [ -n "$edeadline" ]; then
                        local left=$(( edeadline - $(date +%s) ))
                        [ "$left" -lt 0 ] && left=0
                        [ "$left" -lt "$w" ] && w="$left"
                    fi
                    q="$q&wait=$w"
                fi
                _compute_call GET /cli/compute/jobs "$q"
                # The events, then the next cursor as the last line.
                eout="$(printf '%s' "$_CP_BODY" | K2_CP_JSON="$as_json" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
for e in v.get("events") or []:
    if os.environ.get("K2_CP_JSON") == "1":
        print(json.dumps(e, separators=(",", ":")))
    else:
        tail = ""
        if e.get("event") == "finished":
            tail = ": %s" % e.get("state")
            if e.get("exit") is not None:
                tail += " exit %s" % e["exit"]
            if e.get("reason"):
                tail += " (%s)" % e["reason"]
        if e.get("detail"):
            # A move: "moved from linux-1 to linux-2: linux-2 can start it now".
            print("%s  %s  %s" % (time.strftime("%H:%M:%S", time.localtime(e.get("at") or 0)), e.get("short"), e["detail"]))
            continue
        print("%s  %s  %-9s on %s%s" % (time.strftime("%H:%M:%S", time.localtime(e.get("at") or 0)), e.get("short"), e.get("event"), e.get("node") or "?", tail))
print(v.get("cursor"))')"
                cursor="${eout##*$'\n'}"
                [ "$cursor" = "$eout" ] || printf '%s\n' "${eout%$'\n'*}"
                first=0
                [ "$efollow" = "1" ] || break
                if [ -n "$edeadline" ] && [ "$(date +%s)" -ge "$edeadline" ]; then
                    echo "[k2 compute] --max-wait passed; continue with: k2 compute events --follow --since $cursor" >&2
                    exit 0
                fi
            done
            echo "[k2 compute] next: k2 compute events --follow --since $cursor" >&2
            ;;
        local) cmd_compute_local "$@" ;;
        *)
            echo "usage: unknown compute verb '$sub' (k2 compute --help)" >&2
            exit 2
            ;;
    esac
}

