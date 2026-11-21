cmd_compute_grant() {
    local node="" workspace="" maxt="" maxd="" maxp="" maxq="" allow_ci="" maxsub="" allow_gpu=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            # Per-submitter running cap on this node (0.46.6; 0 = off,
            # absent keeps what the grant had).
            --submitter-max-running) _need_value "$@"; maxsub="${2:-}"; shift 2
                case "$maxsub" in ''|*[!0-9]*) echo "usage: --submitter-max-running N (0 = off)" >&2; exit 2 ;; esac ;;
            # CI permission (0.46.5): owner or an Admin login only; absent
            # keeps what the grant had.
            --ci) allow_ci=1; shift ;;
            --no-ci) allow_ci=0; shift ;;
            # GPU jobs (0.46.11): owner or an Admin login only; absent keeps
            # what the grant had.
            --gpu) allow_gpu=1; shift ;;
            --no-gpu) allow_gpu=0; shift ;;
            --max-time) maxt="$(_compute_secs "${2:-}")"; shift 2 ;;
            --max-disk) _need_value "$@"; maxd="${2:-}"; maxd="${maxd%[Gg]}"; shift 2 ;;
            --max-parallel) _need_value "$@"; maxp="${2:-}"; shift 2 ;;
            --max-queued) _need_value "$@"; maxq="${2:-}"; shift 2 ;;
            -*) echo "usage: unknown flag '$1'" >&2; exit 2 ;;
            *) node="$1"; shift ;;
        esac
    done
    # An agent's session may leave out --workspace: its passport names its
    # own (0.46.17). A person's never may.
    { [ -n "$node" ] && { [ -n "$workspace" ] || _compute_in_session; }; } || { echo "usage: k2 compute grant <node> --workspace WS [--max-time 2h] [--max-disk 60G] [--max-parallel 1] [--max-queued 10] [--submitter-max-running N] [--ci|--no-ci] [--gpu|--no-gpu]" >&2; exit 2; }
    local body
    body="$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$workspace" K2_CJ_maxJobSecs_n="$maxt" \
        K2_CJ_maxDiskGb_n="$maxd" K2_CJ_maxParallel_n="$maxp" K2_CJ_maxQueued_n="$maxq" K2_CJ_submitterMaxRunning_n="$maxsub")"
    if [ -n "$allow_ci" ]; then
        body="$(printf '%s' "$body" | K2_CG_CI="$allow_ci" python3 -c 'import json,os,sys; v=json.load(sys.stdin); v["allowCi"] = os.environ["K2_CG_CI"] == "1"; print(json.dumps(v))')"
    fi
    if [ -n "$allow_gpu" ]; then
        body="$(printf '%s' "$body" | K2_CG_GPU="$allow_gpu" python3 -c 'import json,os,sys; v=json.load(sys.stdin); v["allowGpu"] = os.environ["K2_CG_GPU"] == "1"; print(json.dumps(v))')"
    fi
    _compute_call POST /cli/compute/grants/set "$body" tcp
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
g = v["grant"]
sub = (", %s running per submitter" % g["submitterMaxRunning"]) if g.get("submitterMaxRunning") else ""
print("granted %s to %s (max %ss/job, %s parallel%s, %s queued, %s GB disk%s%s). %s" % (v["node"], v["workspace"], g["maxJobSecs"], g["maxParallel"], sub, g["maxQueued"], g["maxDiskGb"], ", CI allowed" if g.get("allowCi") else "", ", GPU jobs allowed" if g.get("allowGpu") else "", v["note"]))
if v.get("resent"):
    print("re-sent %d queued job(s) of %s on %s with the new caps (they keep their order)" % (v["resent"], v["workspace"], v["node"]))'
}

# k2 compute allow-nodes (0.46.8): this workspace's compute node list. No
# argument shows it; nodes (or --add/--remove/--clear) change it. The
# daemon decides who may: the owner or an Admin any way; an agent only
# narrows its own (adding or --clear: allow_nodes_needs_person, exit 3).
cmd_compute_allow_nodes() {
    local nodes="" add="" remove="" clear="" workspace="" as_json=0
    local usage="usage: k2 compute allow-nodes [<node>[,<node>…] | --add NODE[,NODE…] | --remove NODE[,NODE…] | --clear] [--workspace WS] [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --add|--remove|--workspace)
                [ $# -ge 2 ] && [ -n "$2" ] || { echo "$usage ($1 needs a value)" >&2; exit 2; }
                case "$1" in
                    --add) add="${add:+$add,}$2" ;;
                    --remove) remove="${remove:+$remove,}$2" ;;
                    --workspace) workspace="$2" ;;
                esac
                shift 2 ;;
            --clear) clear=1; shift ;;
            --json) as_json=1; shift ;;
            -*) echo "$usage (unknown flag '$1')" >&2; exit 2 ;;
            *) nodes="${nodes:+$nodes,}$1"; shift ;;
        esac
    done
    local ws
    ws="$(_compute_ws "$workspace")"
    if [ -z "$nodes$add$remove$clear" ]; then
        local q=""
        [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
        _compute_call GET /cli/compute/allow-nodes "$q"
    else
        _compute_call POST /cli/compute/allow-nodes/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_nodes="$nodes" \
            K2_CJ_add="$add" K2_CJ_remove="$remove" K2_CJ_clear_b="$clear")"
    fi
    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("change") == "same":
    print("no change")
if v.get("list"):
    names = []
    for n in v.get("nodes") or []:
        if not n.get("name"):
            names.append("%s (removed)" % n.get("id"))
            continue
        name = n["name"]
        if n.get("state") not in (None, "active"):
            name += " (%s)" % n["state"]
        if not n.get("granted"):
            name += " (no grant)"
        names.append(name)
    print("%s: jobs go only to %s" % (v.get("workspace"), ", ".join(names)))
    print("  a node off the list is refused (node_not_allowed); --any, moves and retries stay on the list")
else:
    print("%s: no node list (its jobs may go to any node granted to it)" % v.get("workspace"))
if v.get("failedQueued"):
    print("  ended %d queued job(s) on nodes that left the list (failed/node_not_allowed): %s" % (len(v["failedQueued"]), " ".join(j[:8] for j in v["failedQueued"])))
if v.get("withdrawn"):
    print("  asked %d job(s) back from a shared node'"'"'s queue (they never start there)" % v["withdrawn"])'
}

# k2 compute git-access (0.46.9, GH#78): short-lived, read-only git tokens
# for a workspace's compute jobs, for the GitHub repos a person granted.
# The daemon decides who may: setup is the owner's or an Admin's (outside
# K2); grant is a person's (an agent gets git_access_needs_person, exit 3,
# and asks on a card); an agent may list and revoke its own workspace's.
_compute_git_access_usage() {
    cat <<'EOF'
%include cli/text/help/compute/git-access.txt
EOF
}

cmd_compute_git_access() {
    local verb="list"
    case "${1:-}" in
        setup|grant|revoke|list) verb="$1"; shift ;;
    esac
    local github="" nodes="" workspace="" shared="" all="" clear="" app_id="" key_file="" token_file="" token_stdin="" as_json=0
    local usage="usage: k2 compute git-access [setup|grant|revoke] … (k2 compute git-access --help)"
    while [ $# -gt 0 ]; do
        case "$1" in
            --github|--nodes|--workspace|--github-app|--key-file|--github-token-file)
                [ $# -ge 2 ] && [ -n "$2" ] || { echo "$usage ($1 needs a value)" >&2; exit 2; }
                case "$1" in
                    --github) github="${github:+$github,}$2" ;;
                    --nodes) nodes="${nodes:+$nodes,}$2" ;;
                    --workspace) workspace="$2" ;;
                    --github-app) app_id="$2" ;;
                    --key-file) key_file="$2" ;;
                    --github-token-file) token_file="$2" ;;
                esac
                shift 2 ;;
            --github-token-stdin) token_stdin=1; shift ;;
            --shared) shared=1; shift ;;
            --read-only) shift ;;
            --all) all=1; shift ;;
            --clear) clear=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "$usage (unknown argument '$1')" >&2; exit 2 ;;
        esac
    done
    local ws
    ws="$(_compute_ws "$workspace")"
    case "$verb" in
        list)
            local q=""
            [ -n "$ws" ] && q="workspace=$(urlencode "$ws")"
            _compute_call GET /cli/compute/git-access "$q"
            ;;
        setup)
            local body
            if [ -n "$clear" ]; then
                body='{"clear":true}'
            elif [ -n "$app_id" ]; then
                [ -n "$key_file" ] || { echo "usage: k2 compute git-access setup --github-app <app id> --key-file <app.pem>" >&2; exit 2; }
                [ -r "$key_file" ] || { echo "k2 compute git-access: can't read $key_file" >&2; exit 2; }
                body="$(K2_GA_ID="$app_id" K2_GA_FILE="$key_file" python3 -c '
import json, os
print(json.dumps({"app_id": os.environ["K2_GA_ID"], "key_pem": open(os.environ["K2_GA_FILE"]).read()}))')"
            elif [ -n "$token_file" ] || [ -n "$token_stdin" ]; then
                if [ -n "$token_file" ]; then
                    [ -r "$token_file" ] || { echo "k2 compute git-access: can't read $token_file" >&2; exit 2; }
                fi
                body="$(K2_GA_FILE="$token_file" python3 -c '
import json, os, sys
f = os.environ.get("K2_GA_FILE")
t = (open(f).read() if f else sys.stdin.read()).strip()
print(json.dumps({"token": t}))')"
            else
                echo "usage: k2 compute git-access setup --github-app <app id> --key-file <app.pem> | --github-token-file <file> | --github-token-stdin | --clear" >&2
                echo "  (a person's, from a terminal outside K2; an agent asks: k2 permission request compute.git-access-setup --why \"…\")" >&2
                exit 2
            fi
            _compute_call POST /cli/compute/git-access/setup "$body"
            ;;
        grant)
            [ -n "$github" ] || { echo "usage: k2 compute git-access grant --github <owner>/<repo>[,…] [--nodes all|<node>,…] [--shared] [--workspace WS]" >&2; exit 2; }
            _compute_call POST /cli/compute/git-access/grant "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_github="$github" K2_CJ_nodes="$nodes" K2_CJ_shared_b="$shared")"
            ;;
        revoke)
            [ -n "$github$all" ] || { echo "usage: k2 compute git-access revoke --github <owner>/<repo>[,…] [--nodes …] | --all [--workspace WS]" >&2; exit 2; }
            _compute_call POST /cli/compute/git-access/revoke "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_github="$github" K2_CJ_nodes="$nodes" K2_CJ_all_b="$all")"
            ;;
    esac
    [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if "workspace" not in v:
    c = v.get("credential")
    if c is None:
        print("GitHub credential cleared: every later ask for a git token is refused; grants stay")
    else:
        print("GitHub credential set: %s%s" % ("App " + str(c.get("appId")) + (" (" + c["app"] + ")" if c.get("app") else "") if c.get("kind") == "app" else "fine-grained token", ""))
    if v.get("note"):
        print("  " + v["note"])
    sys.exit(0)
c = v.get("credential")
if c:
    print("credential: %s" % ("GitHub App " + str(c.get("appId")) if c.get("kind") == "app" else "a fine-grained token (nodes get the token itself)"))
else:
    print("credential: none (a person runs: k2 compute git-access setup --github-app <id> --key-file <app.pem>;")
    print("  an agent asks: k2 permission request compute.git-access-setup --why \"<what you need it for>\")")
for w in v.get("warnings") or []:
    print("warning: " + w)
if v.get("granted"):
    print("granted: %s" % ", ".join(v["granted"]))
if v.get("revoked"):
    print("revoked: %s (%d grant%s removed)" % (", ".join(v["revoked"]), v.get("removed", 0), "" if v.get("removed") == 1 else "s"))
    if v.get("tokensDeleted"):
        print("  %d token(s) already handed out were deleted at GitHub" % v["tokensDeleted"])
    for j in v.get("runningJobs") or []:
        print("  running job %s had a token for %s: it keeps running; its next git call for it fails" % (j["job"], j["repo"]))
    if v.get("note"):
        print("  " + v["note"])
gs = v.get("grants") or []
if gs:
    print("%s: jobs may read" % v["workspace"])
    for g in gs:
        print("  %-40s nodes: %s%s" % (g["repo"], g["nodes"], " (shared: another server owns it)" if g.get("shared") else ""))
else:
    print("%s: no git access (k2 compute git-access grant --github <owner>/<repo>)" % v["workspace"])
if v.get("tokenNote"):
    print("note: " + v["tokenNote"])'
}

# k2 compute retier <job> --tier T [--why "…"] [--json] (0.46.6, prd
# job tiers v2 L2): a queued job's tier. The job's own workspace, or the
# owner / an Admin for any job.
cmd_compute_retier() {
    local job="" tier="" why="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --tier) _need_value "$@"; tier="${2:-}"; shift 2 ;;
            --priority|--critical) tier="priority"; shift ;;
            --standard|--normal) tier="normal"; shift ;;
            --batch) tier="batch"; shift ;;
            --why) _need_value "$@"; why="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"])" >&2; exit 2 ;;
            *) job="$1"; shift ;;
        esac
    done
    case "$tier" in
        ci|priority|batch) ;;
        normal|standard) tier="normal" ;;
        *) echo "usage: k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"]   (--priority needs --why)" >&2; exit 2 ;;
    esac
    [ -n "$job" ] || { echo "usage: k2 compute retier <job> --tier standard|priority|batch|ci [--why \"…\"]" >&2; exit 2; }
    if [ "$tier" = "priority" ] && [ -z "$why" ]; then
        echo "usage: --priority needs a one-line reason: k2 compute retier $job --priority --why \"release gate for 0.46.6\"" >&2
        exit 2
    fi
    local body
    body="$(_compute_json K2_CJ_job="$job" K2_CJ_tier="$tier" K2_CJ_why="$why")"
    _compute_call POST /cli/compute/jobs/tier "$body"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
w = {"ci": "CI", "priority": "Priority", "normal": "Standard", "batch": "Batch"}
line = "job %s: %s → %s on %s" % (v["job"], w.get(v["from"], v["from"]), w.get(v["tier"], v["tier"]), v["node"])
if v.get("asked") and v["asked"] != v["tier"]:
    line += " (asked %s: this server'"'"'s Priority doesn'"'"'t count there)" % w.get(v["asked"], v["asked"])
print(line)
print("  " + v["note"])'
}

# k2 compute move <job>... --auto [--avoid N]... [--os O|any] [--arch A|any]
#   [--label k=v]... | --to <node>   (0.46.8): a QUEUED job to auto-placement
# (K2 moves it to a node that can start it) or to one named node, without a
# cancel. --all-mine / --workspace-all [--tag T] [--node N] pick the jobs as
# cancel does. The daemon checks everything; this only builds the body.
cmd_compute_move() {
    local to="" auto=0 os="" arch="" scope="" mtag="" mnode="" mws="" as_json=0
    local -a jobs=() avoids=() labels=()
    local musage="usage: k2 compute move <job>... --auto [--avoid <node>]... [--os O|any] [--arch A|any] [--label k=v]  |  k2 compute move <job>... --to <node>  |  k2 compute move --all-mine|--workspace-all [--tag T] [--node N] [--workspace WS] (--auto … | --to <node>)"
    while [ $# -gt 0 ]; do
        case "$1" in
            --auto) auto=1; shift ;;
            --to) _need_value "$@"; to="${2:-}"; shift 2
                [ -n "$to" ] || { echo "$musage (--to needs a node)" >&2; exit 2; } ;;
            --avoid) _need_value "$@"; avoids+=("${2:-}"); shift 2
                [ -n "${avoids[${#avoids[@]}-1]}" ] || { echo "$musage (--avoid needs a node)" >&2; exit 2; } ;;
            --os) _need_value "$@"; os="${2:-}"; shift 2 ;;
            --arch) _need_value "$@"; arch="${2:-}"; shift 2 ;;
            --label) _need_value "$@"; labels+=("${2:-}"); shift 2 ;;
            --all-mine) scope="session"; shift ;;
            --workspace-all) scope="workspace"; shift ;;
            --tag) _need_value "$@"; mtag="${2:-}"; shift 2 ;;
            --node) _need_value "$@"; mnode="${2:-}"; shift 2 ;;
            --workspace) _need_value "$@"; mws="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            -*) echo "$musage (unknown flag '$1')" >&2; exit 2 ;;
            *) jobs+=("$1"); shift ;;
        esac
    done
    if [ "$auto" = "1" ] && [ -n "$to" ]; then echo "$musage (--auto or --to, not both)" >&2; exit 2; fi
    if [ "$auto" = "0" ] && [ -z "$to" ]; then echo "$musage (say where: --auto or --to <node>)" >&2; exit 2; fi
    if [ -n "$scope" ] && [ "${#jobs[@]}" -gt 0 ]; then echo "$musage (job ids or --all-mine, not both)" >&2; exit 2; fi
    if [ -z "$scope" ] && [ "${#jobs[@]}" -eq 0 ]; then echo "$musage (name a job, or --all-mine)" >&2; exit 2; fi
    if [ -z "$scope" ] && { [ -n "$mtag" ] || [ -n "$mnode" ]; }; then echo "$musage (--tag and --node pick jobs with --all-mine or --workspace-all)" >&2; exit 2; fi
    local avoid_json="" labels_json=""
    if [ "${#avoids[@]}" -gt 0 ]; then
        avoid_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${avoids[@]}")"
    fi
    if [ "${#labels[@]}" -gt 0 ]; then
        labels_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --label key=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${labels[@]}")" || exit 2
    fi
    local ab=""
    [ "$auto" = "1" ] && ab=1
    local render='
import json, sys
v = json.load(sys.stdin)
def one(r):
    f = r.get("filter") or {}
    extra = []
    if f.get("os") or f.get("arch"):
        extra.append("%s-%s" % (f.get("os") or "any", f.get("arch") or "any"))
    if f.get("avoid"):
        extra.append("avoids " + ", ".join(f["avoid"]))
    tail = (" (" + "; ".join(extra) + ")") if extra else ""
    if r.get("moved"):
        return "job %s: moved %s -> %s (%s)%s. %s" % (r["job"], r["from"], r["node"], r["mode"], tail, r.get("note") or "")
    if r.get("pulling"):
        # 0.46.17: it waited in the own queue of a shared node; that node is
        # asked to give it back, then K2 places it again (or pins it).
        return "job %s: pulling back from %s (%s)%s. %s" % (r["job"], r["from"], r["mode"], tail, r.get("note") or "")
    if r.get("mode") == "auto":
        return "job %s: auto-placed now%s. %s" % (r["job"], tail, r.get("note") or "")
    return "job %s: %s" % (r["job"], r.get("note") or "pinned to " + str(r.get("node")))
# 0.46.12: an --avoid name that is not one of your nodes is a note.
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % w)
if "matched" in v:
    m = v.get("moved") or []
    print("moved %d of %d matching queued job(s)" % (len(m), v.get("matched", 0)))
    for r in m:
        print("  " + one(r))
    for s in v.get("skipped") or []:
        print("  skipped %s: %s (%s)" % (s.get("job"), s.get("code"), s.get("hint")))
else:
    print(one(v))'
    if [ -n "$scope" ]; then
        local filt
        filt="$(K2_CF_SCOPE="$scope" K2_CF_TAG="$mtag" K2_CF_NODE="$mnode" K2_CF_WS="$(_compute_ws "$mws")" python3 -c '
import json, os
f = {"scope": os.environ["K2_CF_SCOPE"]}
for k, e in (("tag", "K2_CF_TAG"), ("node", "K2_CF_NODE"), ("workspace", "K2_CF_WS")):
    if os.environ.get(e):
        f[k] = os.environ[e]
print(json.dumps(f))')"
        _compute_call POST /cli/compute/jobs/move "$(_compute_json K2_CJ_filter_j="$filt" K2_CJ_auto_b="$ab" K2_CJ_to="$to" \
            K2_CJ_avoid_j="$avoid_json" K2_CJ_os="$os" K2_CJ_arch="$arch" K2_CJ_labels_j="$labels_json")"
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
        printf '%s' "$_CP_BODY" | python3 -c "$render"
        exit 0
    fi
    # One job: refusals exit as for every compute verb (2 usage, 3 refused).
    # Several: each is tried; the worst exit wins, every refusal is printed.
    local j rc worst=0
    for j in "${jobs[@]}"; do
        (
            _compute_call POST /cli/compute/jobs/move "$(_compute_json K2_CJ_job="$j" K2_CJ_auto_b="$ab" K2_CJ_to="$to" \
                K2_CJ_avoid_j="$avoid_json" K2_CJ_os="$os" K2_CJ_arch="$arch" K2_CJ_labels_j="$labels_json")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; else printf '%s' "$_CP_BODY" | python3 -c "$render"; fi
        )
        rc=$?
        [ "$rc" -gt "$worst" ] && worst="$rc"
    done
    exit "$worst"
}

