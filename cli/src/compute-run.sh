cmd_compute_run() {
    local node="" any=0 os="" arch="" detach=0 dirty=0 sha="" nosrc=0 cpus="" mem="" timeout="" exclusive=0
    local retry="" raw=0 workspace="" wait=0 wait_failures=0 wait_quiet=0 max_wait="" as_json=0 qtimeout="" retries="" stall="" nshards="" plan=0
    local gate="" tier="" supersede="" submitter="" repo="" cache="" why=""
    local preset="" now=0 cache_required=0 tag="" same_node="" distro="" receipt_v="" gpu="" gpu_mem="" mem_auto="" mem_auto_fb="" cpus_burst=""
    local network="" strict=""
    local -a labels=() envs=() needs=() artifacts=() secrets=() withs=() avoids=() onlys=() netns_dummies=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; break ;;
            # --avoid NODE (0.46.8, repeatable): K2 never picks that node for
            # this job (submit, moves, retries). Not with a named node.
            --avoid) _need_value "$@"; avoids+=("${2:-}"); shift 2
                [ -n "${avoids[${#avoids[@]}-1]}" ] || { echo "usage: --avoid <node> (a node K2 must not pick; repeatable)" >&2; exit 2; } ;;
            # --only NODE (0.46.9, repeatable): K2 picks only among these
            # nodes. Not with a named node.
            --only) _need_value "$@"; onlys+=("${2:-}"); shift 2
                [ -n "${onlys[${#onlys[@]}-1]}" ] || { echo "usage: --only <node> (K2 picks only among the nodes you list; repeatable)" >&2; exit 2; } ;;
            # --preset NAME: [presets.NAME] from .k2/compute.toml (0.46.7;
            # the daemon reads the file, flags still win).
            --preset) _need_value "$@"; preset="${2:-}"; shift 2
                if ! [[ "$preset" =~ ^[a-z0-9][a-z0-9._-]{0,39}$ ]]; then
                    echo "usage: --preset NAME (a [presets.NAME] in .k2/compute.toml: 1–40 of a-z 0-9 . _ -)" >&2; exit 2
                fi ;;
            # --now: start at once or fail (exit 3 not_now), nothing queued.
            --now) now=1; shift ;;
            # --tag NAME: your own label for the job (jobs --tag filters).
            --tag) _need_value "$@"; tag="${2:-}"; shift 2
                if ! [[ "$tag" =~ ^[A-Za-z0-9._:/-]{1,64}$ ]]; then
                    echo "usage: --tag NAME (1–64 of A-Z a-z 0-9 . _ : / -, e.g. fuzz:p3)" >&2; exit 2
                fi ;;
            # --same-node-as JOB: run on the node that ran one of your jobs.
            --same-node-as) _need_value "$@"; same_node="${2:-}"; shift 2
                [ -n "$same_node" ] || { echo "usage: --same-node-as <job> (one of this workspace's jobs)" >&2; exit 2; } ;;
            # --repo <folder>: a git repo (or a worktree of one) inside the
            # workspace, shipped instead of the workspace's own (0.46.6).
            --repo) _need_value "$@"; repo="${2:-}"; shift 2
                [ -n "$repo" ] || { echo "usage: --repo <folder> (a git repo inside the workspace, e.g. k2-multiplayer-base)" >&2; exit 2; } ;;
            # --with DIR[@SHA]: a sibling repo shipped beside the main one,
            # laid out as in the workspace (0.46.6). Repeatable.
            --with) _need_value "$@"; withs+=("${2:-}"); shift 2
                [ -n "${withs[${#withs[@]}-1]}" ] || { echo "usage: --with DIR[@SHA] (a git repo inside the workspace, or a worktree of one)" >&2; exit 2; } ;;
            # --cache NAME: a folder on the node that outlives the job
            # ($K2_JOB_CACHE), per workspace and name (0.46.6).
            --cache) _need_value "$@"; cache="${2:-}"; shift 2
                if ! [[ "$cache" =~ ^[a-z0-9][a-z0-9._-]{0,63}$ ]]; then
                    echo "usage: --cache NAME (1–64 of a-z 0-9 . _ -, starting with a letter or digit)" >&2; exit 2
                fi ;;
            # --cache-required: run only on the node that holds the --cache
            # (it waits there); without it the cache is only a preference.
            --cache-required) cache_required=1; shift ;;
            # --strict (0.46.17): the server runs a sh -c / bash -c script
            # under bash -euo pipefail.
            --strict) strict=1; shift ;;
            # --gate = --tier ci (0.46.5; an older daemon only knows gate,
            # so both go in the body).
            --gate) gate=1; tier="ci"; shift ;;
            # --priority (0.46.6; --critical kept) needs --why "<reason>".
            --priority|--critical) tier="priority"; shift ;;
            --why) _need_value "$@"; why="${2:-}"; shift 2 ;;
            --tier) _need_value "$@"; tier="${2:-}"; shift 2
                case "$tier" in
                    ci|priority|batch) ;;
                    normal|standard) tier="normal" ;;
                    *) echo "usage: --tier standard|priority|batch|ci (k2 study compute: TIERS)" >&2; exit 2 ;;
                esac
                [ "$tier" = "ci" ] && gate=1 ;;
            --supersede) _need_value "$@"; supersede="${2:-}"; shift 2
                if ! printf '%s' "$supersede" | python3 -c '
import sys
k = sys.stdin.read()
ok = 1 <= len(k) <= 120 and k == k.strip() and not any(ord(c) < 32 or ord(c) == 127 for c in k)
sys.exit(0 if ok else 1)'; then
                    echo "usage: --supersede KEY (1–120 characters, no control characters, no leading/trailing space)" >&2; exit 2
                fi ;;
            --submitter) _need_value "$@"; submitter="${2:-}"; shift 2
                if ! [[ "$submitter" =~ ^[a-z0-9._-]{1,40}$ ]]; then
                    echo "usage: --submitter NAME (1–40 of a-z 0-9 . _ -)" >&2; exit 2
                fi ;;
            --secret) _need_value "$@"; secrets+=("${2:-}"); shift 2 ;;
            --wait) wait=1; shift ;;
            # --wait --quiet: the old exit-only wait (no log text; 0.46.6).
            --quiet|-q) wait_quiet=1; shift ;;
            --failures) wait_failures=1; shift ;;
            --max-wait) _need_value "$@"; max_wait="${2:-}"; shift 2 ;;
            --any) any=1; shift ;;
            --os) _need_value "$@"; os="${2:-}"; shift 2 ;;
            # --distro ubuntu|ubuntu@24.04|debian (0.46.9): the node's
            # /etc/os-release ID or ID_LIKE, the version on a dot prefix.
            --distro) distro="$(_compute_distro_arg "${2:-}")" || exit 2; shift 2 ;;
            --arch) _need_value "$@"; arch="${2:-}"; shift 2 ;;
            --label) _need_value "$@"; labels+=("${2:-}"); shift 2 ;;
            --detach|-d) detach=1; shift ;;
            --dirty) dirty=1; shift ;;
            --sha) _need_value "$@"; sha="${2:-}"; shift 2 ;;
            --no-source) nosrc=1; shift ;;
            --env) _need_value "$@"; envs+=("${2:-}"); shift 2 ;;
            --needs|--need) _need_value "$@"; needs+=("${2:-}"); shift 2 ;;
            --cpus) _need_value "$@"; cpus="${2:-}"; cpus_burst=""; shift 2  # N+M (Blacksmith R2): cpus + cpusBurst
                if [[ "$cpus" == *+* ]]; then [[ "$cpus" =~ ^[0-9]+(\.[0-9]+)?\+[0-9]+(\.[0-9]+)?$ ]] || { echo "usage: --cpus N or --cpus N+M (4+8: 4 guaranteed, up to 8 on idle cores)" >&2; exit 2; }; cpus_burst="${cpus#*+}"; cpus="${cpus%%+*}"; fi ;;
            # --mem auto[:8G] (Blacksmith class R1): sized from this
            # command's finished runs; the size after : is the fallback.
            --mem)
                case "${2:-}" in
                    auto) mem_auto=1; mem=""; mem_auto_fb="" ;;
                    auto:*) mem_auto=1; mem=""; mem_auto_fb="$(_compute_bytes "${2#auto:}")" || exit 2 ;;
                    *) mem="$(_compute_bytes "${2:-}")" || exit 2; mem_auto=""; mem_auto_fb="" ;;
                esac
                shift 2 ;;
            --timeout) timeout="$(_compute_secs "${2:-}")"; shift 2 ;;
            --exclusive) exclusive=1; shift ;;
            # --gpu [any|amd|intel] (0.46.11): one GPU of the node, to this
            # job alone. Bare --gpu = any.
            --gpu=*) gpu="${1#--gpu=}"; shift
                _compute_gpu_arg "$gpu" || exit 2 ;;
            --gpu)
                case "${2:-}" in
                    ''|-*) gpu="any"; shift ;;
                    *) gpu="$2"; shift 2; _compute_gpu_arg "$gpu" || exit 2 ;;
                esac ;;
            # --gpu-mem SIZE (GPU jobs S6): the GPU job's GPU memory budget.
            --gpu-mem) gpu_mem="$(_compute_bytes "${2:-}")" || exit 2; shift 2 ;;
            # --network none (0.46.14): this job gets no network (its own
            # namespace with loopback only on Linux), whatever its node's
            # setting. A job can only narrow its network.
            --network)
                case "${2:-}" in
                    none) network="none" ;;
                    on|no-lan|full) echo "usage: --network none is the one value: a job can only narrow its network (its node's owner sets how much jobs get there)" >&2; exit 2 ;;
                    *) echo "usage: --network none (no network for this job: loopback only)" >&2; exit 2 ;;
                esac
                shift 2 ;;
            # --netns-dummy NAME=ADDR/PREFIX (0.46.14, repeatable, at most 4;
            # Linux nodes): a dummy interface in the job's own network
            # namespace, up before the job starts. Implies --network none.
            --netns-dummy) _need_value "$@"; netns_dummies+=("${2:-}"); shift 2
                [[ "${netns_dummies[${#netns_dummies[@]}-1]}" == ?*=?*/?* ]] || { echo "usage: --netns-dummy NAME=ADDRESS/PREFIX (e.g. dummy0=10.254.254.1/32; private, link-local or test ranges only)" >&2; exit 2; } ;;
            --receipt-v2) receipt_v=2; shift ;;
            --retry-interrupted) retry=1; shift ;;
            --no-retry) retry=0; retries=0; shift ;;
            --retries) _need_value "$@"; retries="${2:-}"; shift 2
                case "$retries" in ''|*[!0-9]*) echo "usage: --retries N (0–5)" >&2; exit 2 ;; esac ;;
            --queue-timeout)
                case "${2:-}" in
                    off|none|0) qtimeout=0 ;;
                    *) qtimeout="$(_compute_secs "${2:-}")" ;;
                esac
                shift 2 ;;
            --stall-timeout)
                case "${2:-}" in
                    off|none|0) stall=0 ;;
                    *) stall="$(_compute_secs "${2:-}")" ;;
                esac
                shift 2 ;;
            --raw) raw=1; shift ;;
            --artifact|--artifacts) _need_value "$@"; artifacts+=("${2:-}"); shift 2 ;;
            --shards) _need_value "$@"; nshards="${2:-}"; shift 2
                case "$nshards" in ''|*[!0-9]*) echo "usage: --shards N (2–16)" >&2; exit 2 ;; esac ;;
            --json) as_json=1; shift ;;
            --plan|--dry-run) plan=1; shift ;;
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            -*) echo "usage: unknown flag '$1' (k2 compute --help)" >&2; exit 2 ;;
            *)
                if [ -z "$node" ] && [ "$any" = "0" ]; then node="$1"; shift
                else echo "usage: put the command after --: k2 compute run [<node>] -- <command…>" >&2; exit 2
                fi ;;
        esac
    done
    if [ $# -eq 0 ]; then
        echo "usage: k2 compute run [<node>] [flags] -- <command…>  (no node: K2 picks one)" >&2
        exit 2
    fi
    # No node named: K2 picks a granted node (auto-placement, 0.46.6).
    # `any` rides in the body so an older daemon still accepts it; --any
    # is the same thing, kept for scripts.
    [ -z "$node" ] && any=1
    if [ "$wait" = "1" ] && [ "$detach" = "1" ]; then
        echo "usage: --wait and --detach don't mix (--wait blocks until the end; --detach returns at once)" >&2
        exit 2
    fi
    # Tiers v2 (0.46.6): Priority needs a one-line reason; Batch waits for
    # an idle node (hours), so it is always detached.
    if [ "$tier" = "priority" ] && [ -z "$why" ]; then
        echo "usage: --priority needs a one-line reason: --priority --why \"release gate for 0.46.6\" (it shows in the queue and k2 compute status; Priority is for the critical path only)" >&2
        exit 2
    fi
    # 0.46.7: --exclusive holds a whole node, so it needs a reason too.
    if [ "$exclusive" = "1" ] && [ -z "$why" ]; then
        echo "usage: --exclusive needs a one-line reason: --exclusive --why \"benchmark needs the whole node\" (it shows in the queue; it also needs a grant with --ci)" >&2
        exit 2
    fi
    if [ "$now" = "1" ] && { [ "$tier" = "batch" ] || [ -n "$nshards" ]; }; then
        echo "usage: --now doesn't mix with --tier batch (it waits for an idle node) or --shards (each shard is placed itself)" >&2
        exit 2
    fi
    if [ "$tier" = "batch" ]; then
        if [ "$wait" = "1" ]; then
            echo "usage: --tier batch doesn't mix with --wait: a Batch job starts only when the node is idle (maybe hours). It is detached; k2 compute wait <id> later" >&2
            exit 2
        fi
        detach=1
    fi
    if { [ "$wait_failures" = "1" ] || [ -n "$max_wait" ] || [ "$wait_quiet" = "1" ]; } && [ "$wait" = "0" ]; then
        echo "usage: --failures, --max-wait and --quiet go with --wait (k2 compute run <node> --wait --failures -- …)" >&2
        exit 2
    fi
    # A bad --max-wait is the usage error before the job is sent.
    [ -z "$max_wait" ] || _compute_secs "$max_wait" >/dev/null
    # --wait: attached on the daemon (detach:false), so an agent in a K2 tab
    # gets no second [compute] line while `wait` follows it; one automatic
    # retry after an interruption, like --detach.
    [ "$wait" = "1" ] && [ -z "$retry" ] && retry=1
    local cwd="" repo_dir=""
    if _k2_git_usable; then
        cwd="$(git rev-parse --show-prefix 2>/dev/null || true)"
        cwd="${cwd%/}"
        # The checkout we're in: from a git worktree inside the workspace the
        # daemon takes HEAD (and --dirty) from it, not the main checkout.
        repo_dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"
        # (--repo naming another repo than the one we're in: the daemon
        # starts the job at that repo's root; our folder means nothing there.)
    fi
    local client
    client="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
    local argv_json labels_json env_json
    argv_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$@")"
    labels_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --label key=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${labels[@]+"${labels[@]}"}")" || exit 2
    env_json="$(python3 -c '
import json, sys
out = {}
for l in sys.argv[1:]:
    if "=" not in l:
        sys.stderr.write("usage: --env NAME=value\n"); sys.exit(2)
    k, v = l.split("=", 1); out[k] = v
print(json.dumps(out))' "${envs[@]+"${envs[@]}"}")" || exit 2
    local needs_json=""
    if [ "${#needs[@]}" -gt 0 ]; then
        needs_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${needs[@]}")"
    fi
    local artifacts_json=""
    if [ "${#artifacts[@]}" -gt 0 ]; then
        artifacts_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${artifacts[@]}")"
    fi
    # --secret NAME[=ENVNAME]: names only (the value never leaves the vault
    # until the node asks for it at spawn, sealed).
    local secrets_json=""
    if [ "${#secrets[@]}" -gt 0 ]; then
        secrets_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    if not a or a.startswith("="):
        sys.stderr.write("usage: --secret NAME[=ENVNAME]\n"); sys.exit(2)
    n, _, e = a.partition("=")
    o = {"name": n}
    if e: o["env"] = e
    out.append(o)
print(json.dumps(out))' "${secrets[@]}")" || exit 2
    fi
    local with_json=""
    if [ "${#withs[@]}" -gt 0 ]; then
        with_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    repo, at, sha = a.rpartition("@")
    if not at:
        repo, sha = a, ""
    if not repo:
        sys.stderr.write("usage: --with DIR[@SHA]\n"); sys.exit(2)
    o = {"repo": repo}
    if sha: o["sha"] = sha
    out.append(o)
print(json.dumps(out))' "${withs[@]}")" || exit 2
    fi
    local avoid_json=""
    if [ "${#avoids[@]}" -gt 0 ]; then
        avoid_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${avoids[@]}")"
    fi
    local only_json=""
    if [ "${#onlys[@]}" -gt 0 ]; then
        only_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${onlys[@]}")"
    fi
    local ws_name
    ws_name="$(_compute_ws "$workspace")"
    # --gpu (0.46.11): one GPU of the vendor asked (count 1 in v1).
    local gpu_json=""
    [ -n "$gpu" ] && gpu_json="{\"vendor\": \"$gpu\", \"count\": 1}"
    # --gpu-mem (GPU jobs S6): only with --gpu.
    if [ -n "$gpu_mem" ] && [ -z "$gpu" ]; then
        echo "usage: --gpu-mem needs --gpu: it is the GPU memory budget of a GPU job (k2 compute run --gpu --gpu-mem 4G --mem 8G -- <command>)" >&2
        exit 2
    fi
    # --network none / --netns-dummy (0.46.14): the daemon checks the
    # names and addresses (exit 2 with the rule).
    local netns_json=""
    if [ "${#netns_dummies[@]}" -gt 0 ]; then
        if [ "${#netns_dummies[@]}" -gt 4 ]; then
            echo "usage: at most 4 --netns-dummy per job" >&2; exit 2
        fi
        netns_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${netns_dummies[@]}")"
        network="none"
    fi
    local body
    body="$(_compute_json K2_CJ_node="$node" K2_CJ_any_b="$any" K2_CJ_os="$os" K2_CJ_distro="$distro" K2_CJ_arch="$arch" K2_CJ_avoid_j="$avoid_json" K2_CJ_only_j="$only_json" \
        K2_CJ_labels_j="$labels_json" K2_CJ_env_j="$env_json" K2_CJ_argv_j="$argv_json" K2_CJ_needs_j="$needs_json" \
        K2_CJ_artifacts_j="$artifacts_json" K2_CJ_secrets_j="$secrets_json" K2_CJ_queueTimeoutSecs_n="$qtimeout" K2_CJ_retries_n="$retries" K2_CJ_stallSecs_n="$stall" K2_CJ_shards_n="$nshards" K2_CJ_plan_b="$plan" \
        K2_CJ_detach_b="$detach" K2_CJ_dirty_b="$dirty" K2_CJ_sha="$sha" K2_CJ_noSource_b="$nosrc" \
        K2_CJ_cpus_n="$cpus" K2_CJ_cpusBurst_n="$cpus_burst" K2_CJ_memBytes_n="$mem" K2_CJ_memAuto_b="$mem_auto" K2_CJ_memAutoFallbackBytes_n="$mem_auto_fb" K2_CJ_timeoutSecs_n="$timeout" \
        K2_CJ_exclusive_b="$exclusive" K2_CJ_retryInterrupted_b="$retry" K2_CJ_cwd="$cwd" \
        K2_CJ_repoDir="$repo_dir" K2_CJ_workspace="$ws_name" K2_CJ_clientJobId="$client" K2_CJ_gate_b="$gate" \
        K2_CJ_tier="$tier" K2_CJ_why="$why" K2_CJ_supersede="$supersede" K2_CJ_submitter="$submitter" \
        K2_CJ_repo="$repo" K2_CJ_cache="$cache" K2_CJ_with_j="$with_json" \
        K2_CJ_preset="$preset" K2_CJ_now_b="${now#0}" K2_CJ_cacheRequired_b="${cache_required#0}" K2_CJ_tag="$tag" K2_CJ_sameNodeAs="$same_node" K2_CJ_receiptV_n="$receipt_v" \
        K2_CJ_gpu_j="$gpu_json" K2_CJ_gpuMemBytes_n="$gpu_mem" K2_CJ_network="$network" K2_CJ_netnsDummies_j="$netns_json" K2_CJ_strict_b="$strict")"
    [ "$any" = "0" ] && body="$(printf '%s' "$body" | python3 -c 'import json,sys; v=json.load(sys.stdin); v.pop("any", None); print(json.dumps(v))')"
    # The same clientJobId makes a retried POST idempotent.
    local raw_resp attempt=0
    while :; do
        raw_resp="$(_compute_send POST /cli/compute/run "$body")"
        _compute_split "$raw_resp"
        [ "$_CP_STATUS" != "000" ] && break
        attempt=$((attempt + 1))
        [ "$attempt" -ge 3 ] && break
        sleep 2
    done
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$plan" = "1" ]; then
        # Dry run: one JSON object on stdout, a one-line summary on stderr.
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
e = v.get("estimate")
def d(x):
    return "%dm%02ds" % (x // 60, x % 60)
line = "[k2 compute] plan: %s on %s" % ("starts now" if v.get("startsNow") else "waits (%s ahead%s)" % (v["queue"]["ahead"], (": " + v["queue"]["blockedBy"]["code"]) if v["queue"].get("blockedBy") else ""), v["node"])
if not v.get("online"): line += " (offline now)"
if v.get("warmSlot"): line += " · " + v["warmSlot"].split(" ")[0]
line += (" · ~%s (p90 %s, %d runs: %s)" % (d(e["medianSecs"]), d(e["p90Secs"]), e["samples"], e["basis"])) if e else " · no estimate yet"
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % w)
for n in v.get("notices") or []:
    sys.stderr.write("[k2 compute] note: %s\n" % (n.get("message") or n.get("code")))
sys.stderr.write(line + "\n")
p = v.get("placement") or {}
if p.get("mode") == "auto":
    sys.stderr.write("[k2 compute] would place on %s (auto): %s%s\n" % (p.get("node"), p.get("why"), (" · " + p["fits"]) if p.get("fits") else ""))
elif p.get("why") and p.get("why") != "you named this node":
    sys.stderr.write("[k2 compute] pinned to %s: %s\n" % (p.get("node"), p.get("why")))
# 0.46.7: tools on that node, the limits that can end it, the settings
# .k2/compute.toml / --preset filled in, and the runners-up.
def dur(s):
    if s is None: return None
    s = int(s)
    return "%dh" % (s // 3600) if s >= 3600 and s % 3600 == 0 else ("%dm" % (s // 60) if s >= 60 and s % 60 == 0 else "%ds" % s)
src = v.get("source")
if isinstance(src, dict):
    where = src.get("checkout") or "workspace"
    if src.get("repo") and src.get("checkout") != "worktree": where = "repo"
    if src.get("path"): where += " " + src["path"]
    s = "[k2 compute] source: %s @ %s" % (where, (src.get("commit") or "")[:8])
    if isinstance(src.get("dirty"), dict):
        s += " +dirty (%s changed, %s untracked)" % (src["dirty"].get("changed"), src["dirty"].get("untracked"))
    sys.stderr.write(s + "\n")
if v.get("toolsLine"):
    sys.stderr.write("[k2 compute] tools on %s: %s\n" % (v["node"], v["toolsLine"]))
lim = v.get("limits") or {}
ends = []
if lim.get("stallSecs"):
    ends.append("ends after %s with no output (--stall-timeout)" % dur(lim["stallSecs"]))
if lim.get("queueTimeoutSecs"):
    ends.append("fails if not started within %s (--queue-timeout)" % dur(lim["queueTimeoutSecs"]))
if lim.get("timeoutSecs"):
    ends.append("stops after %s running (--timeout)" % dur(lim["timeoutSecs"]))
g = v.get("gpu")
if isinstance(g, dict):
    gs = g.get("gpus") or []
    if gs:
        sys.stderr.write("[k2 compute] gpu: %s, %s free now (one GPU, this job alone)\n" % (" · ".join(x.get("model") or x.get("pci") for x in gs), g.get("free")))
    else:
        sys.stderr.write("[k2 compute] gpu: none on %s for this server (it would fail there)\n" % v["node"])
    m = g.get("mem")
    if isinstance(m, dict):
        # GPU jobs S6: the GPU memory budget there and what it counts.
        gib = lambda b: "%.1fG" % (b / (1 << 30))
        if m.get("budgetBytes") is None:
            s = "gpu memory: not watched (a discrete GPU; --gpu-mem sets a budget)"
        else:
            s = "gpu memory: budget %s%s" % (gib(m["budgetBytes"]), "" if m.get("watched") else " (NOT watched: an older k2-node there)")
            if m.get("countedBytes"):
                s += ", counted with --mem (unified GPU: its memory is system RAM)"
        sys.stderr.write("[k2 compute] %s\n" % s)
nw = v.get("network")
if isinstance(nw, dict):
    # 0.46.14: the job asked for --network none (and dummy interfaces).
    sys.stderr.write("[k2 compute] network: %s (this job only: loopback%s, no internet, no LAN)\n" % (nw.get("line"), " and its dummy interfaces" if nw.get("dummies") else ""))
if ends:
    sys.stderr.write("[k2 compute] limits: %s\n" % " · ".join(ends))
eff = v.get("effective") or {}
parts = []
for k in ("os", "arch", "mem", "cpus", "needs", "env", "timeout", "stall", "tier", "cache"):
    e = eff.get(k)
    if not isinstance(e, dict) or e.get("from") in (None, "flag"):
        continue
    # --mem auto (Blacksmith class R1): its own note line says what and why.
    if k == "mem" and str(e.get("from")).startswith("auto"):
        continue
    val = e.get("value")
    if isinstance(val, list): val = ", ".join(str(x) for x in val)
    parts.append("%s %s (%s)" % (k, val, e["from"]))
if parts:
    src = eff.get("source") or ""
    if eff.get("preset"): src += (", " if src else "") + "preset " + eff["preset"]
    sys.stderr.write("[k2 compute] from %s: %s\n" % (src, "; ".join(parts)))
hints = ((eff.get("env") or {}).get("cpuHints")) or {}
if hints:
    sys.stderr.write("[k2 compute] CPU hints: %s (the job'"'"'s CPUs; --env NAME=… overrides)\n" % " ".join("%s=%s" % kv for kv in hints.items()))
rk = v.get("ranking") or []
if len(rk) > 1:
    def one(r):
        s = r["node"] + " (" + r["state"]
        if r.get("etaSecs") is not None: s += ", ~%s %s" % (dur(r["etaSecs"]), r.get("etaBasis"))
        return s + ")"
    sys.stderr.write("[k2 compute] ranking: %s\n" % " · ".join(one(r) for r in rk))
print(json.dumps(v, indent=2))'
        exit 0
    fi
    local info
    # A reader that raises prints one line, {"error": "bad_response", ...},
    # and exits 1, never a traceback (the hook on its first line).
    info="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys; sys.excepthook = lambda t, e, tb: None if issubclass(t, KeyboardInterrupt) else sys.stderr.write(json.dumps({"error": "bad_response", "hint": "k2 compute can\x27t read the daemon\x27s answer (a daemon on another K2 version?)"}) + "\n")
v = json.load(sys.stdin)
j = v["job"]
c = (j.get("srcCommit") or "")[:8]
q = v.get("queue") or {}
if v.get("shards"):
    line = "[k2 compute] job %s: %d shards → %s" % (j["short"], len(v["shards"]), ", ".join("%s:%s" % (x["index"], x.get("node") or "?") for x in v["shards"]))
else:
    line = "[k2 compute] job %s on %s" % (j["short"], j.get("node"))
    if c: line += " · commit " + c
    line += " · " + j["state"]
    if q: line += " (position %s: %s)" % (q.get("position"), q.get("reason"))
# 0.46.8: the pinned-node hint is its own line ("[k2 compute] hint: …").
for w in v.get("warnings") or []:
    sys.stderr.write("[k2 compute] %s\n" % (w if w.startswith("hint: ") else "note: " + w))
# Tiers (0.46.5): courtesy notes (the job is submitted) and --supersede.
for n in v.get("notices") or []:
    sys.stderr.write("note: %s\n" % (n.get("message") or n.get("code")))
rep = v.get("replaced") or []
if rep:
    sys.stderr.write("[k2 compute] replaced %d queued job(s) with the same --supersede key: %s\n" % (len(rep), ", ".join(str(r)[:8] for r in rep)))
if j.get("tier") and j.get("tier") != "normal":
    line += " · tier " + j["tier"]
if j.get("tierAsked") and j.get("tierAsked") != j.get("tier"):
    line += " (asked %s)" % j["tierAsked"]
src = v.get("source")
if isinstance(src, dict):
    where = src.get("checkout") or "workspace"
    if src.get("repo") and src.get("checkout") != "worktree": where = "repo"
    if src.get("path"): where += " " + src["path"]
    s = "[k2 compute] source: %s @ %s" % (where, (src.get("commit") or "")[:8])
    d = src.get("dirty")
    if isinstance(d, dict):
        s += " +dirty (%s changed, %s untracked)" % (d.get("changed"), d.get("untracked"))
    sys.stderr.write(s + "\n")
# Auto-placement (0.46.6): where K2 put it and why, one line. A pinned job
# (a node named) prints none.
p = v.get("placement") or j.get("placement") or {}
placed = ""
if p.get("mode") == "auto":
    placed = "[k2 compute] placed on %s (auto): %s" % (p.get("node"), p.get("why"))
    if p.get("fits"): placed += " · " + p["fits"]
clean = lambda x: x.replace("\t", " ").replace("\n", " ")
print(j["id"] + "\t" + ("set" if v.get("shards") else "one") + "\t" + clean(line) + "\t" + clean(placed))')"
    local job_id="${info%%$'\t'*}" rest="${info#*$'\t'}"
    local kind="${rest%%$'\t'*}" rest2="${rest#*$'\t'}"
    local line="${rest2%%$'\t'*}" placed="${rest2#*$'\t'}"
    echo "$line" >&2
    if [ -n "$placed" ]; then echo "$placed" >&2; fi
    if [ "$detach" = "1" ]; then
        if [ "$as_json" = "1" ]; then
            _compute_print_result "$job_id"
        else
            echo "${job_id:0:8}"
        fi
        echo "[k2 compute] detached. k2 compute wait ${job_id:0:8} blocks until it ends (agents in a K2 tab also get a [compute] message)" >&2
        exit 0
    fi
    if [ "$wait" = "1" ]; then
        # The id first, so an agent whose shell tool times out can resume
        # with `k2 compute wait <id> --follow`. Then (0.46.6, L8) the wait
        # lines and the job's output, streamed like an attached run, ending
        # with the job's exit code; --max-wait still bounds it. --quiet
        # (and --json, whose stdout is the wait JSON) keep the exit-only
        # wait. A split run's id waits like one job (no log stream: its
        # shards each have one).
        if [ "$as_json" = "1" ]; then echo "${job_id:0:8}" >&2; else echo "${job_id:0:8}"; fi
        if [ "$wait_quiet" = "0" ] && [ "$as_json" = "0" ] && [ "$kind" != "set" ]; then
            if [ -n "$max_wait" ]; then local mw; mw="$(_compute_secs "$max_wait")" || exit 2; _CP_DEADLINE=$(( $(date +%s) + mw )); fi
            _CP_FAILURES="$wait_failures"
            _compute_follow "$job_id" "$raw" 0 0
        fi
        local -a wargs=("$job_id")
        [ "$wait_failures" = "1" ] && wargs+=(--failures)
        [ -n "$max_wait" ] && wargs+=(--max-wait "$max_wait")
        [ "$as_json" = "1" ] && wargs+=(--json)
        cmd_compute_wait "${wargs[@]}"
    fi
    if [ "$kind" = "set" ]; then
        _compute_follow_set "$job_id" "$as_json"
    fi
    _compute_follow "$job_id" "$raw" 0 "$as_json"
}

# k2 compute cache [list|clear] — this server's job caches on its nodes
# (`--cache NAME`, 0.46.6).
cmd_compute_cache() {
    local verb="list" node="" ws="" name="" all=0 as_json=0 lineage=""
    case "${1:-}" in
        list|ls) shift ;;
        clear|rm) verb="clear"; shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            # Blacksmith class B2: cache generations, per lineage.
            --lineage)
                case "${2:-}" in trusted|scratch|release|all) lineage="$2" ;; *) echo "usage: --lineage trusted|scratch|release|all" >&2; exit 2 ;; esac
                shift 2 ;;
            --node) _need_value "$@"; node="${2:-}"; shift 2 ;;
            --workspace) _need_value "$@"; ws="${2:-}"; shift 2 ;;
            --name) _need_value "$@"; name="${2:-}"; shift 2 ;;
            --all) all=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 compute cache [list] [--node N] [--workspace WS] [--name NAME] [--lineage trusted|scratch|release|all] [--json] | clear (--workspace WS | --all) [--name NAME] [--node N]" >&2; exit 2 ;;
        esac
    done
    if [ -n "$lineage" ]; then
        [ "$verb" = "list" ] || { echo "usage: --lineage goes with k2 compute cache list" >&2; exit 2; }
        local q=""
        [ -n "$node" ] && q="$q&node=$(urlencode "$node")"
        [ -n "$ws" ] && q="$q&workspace=$(urlencode "$ws")"
        [ "$lineage" != "all" ] && q="$q&lineage=$lineage"
        _compute_call GET /cli/compute/cache/lineages "${q#&}"
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
nodes = v.get("nodes") or []
if not nodes:
    print("no cache generations of %s on any connected node (a node keeps them once its owner turns them on: sudo k2-node admin cache-gens on)" % v.get("workspace"))
for n in nodes:
    print("%s:" % n["node"])
    for l in n["lineages"]:
        age = l.get("ageSecs") or 0
        ago = "%dm" % (age // 60) if age < 7200 else ("%dh" % (age // 3600) if age < 172800 else "%dd" % (age // 86400))
        print("  %-28s #%-4s %s  %6.1f GB  used %s ago%s" % (l["lineage"], l["headSeq"], l["headCommit"][:10], l["bytes"] / 1073741824.0, ago, ("  (%d forks running)" % l["forks"]) if l.get("forks") else ""))
    if n.get("truncated"):
        print("  (more lineages than the node reports)")'
        exit 0
    fi
    if [ "$verb" = "clear" ]; then
        if [ -z "$ws" ] && [ "$all" = "0" ] && ! _compute_in_session; then
            echo "usage: k2 compute cache clear --workspace WS [--name NAME] (or --all for every workspace's caches)" >&2; exit 2
        fi
        local body
        body="$(_compute_json K2_CJ_node="$node" K2_CJ_workspace="$ws" K2_CJ_name="$name" K2_CJ_all_b="$all")"
        _compute_call POST /cli/compute/cache/clear "$body" tcp
    else
        local q=""
        [ -n "$node" ] && q="$q&node=$(urlencode "$node")"
        [ -n "$ws" ] && q="$q&workspace=$(urlencode "$ws")"
        [ -n "$name" ] && q="$q&name=$(urlencode "$name")"
        _compute_call GET /cli/compute/cache "${q#&}" tcp
    fi
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
    printf '%s' "$_CP_BODY" | K2_CP_VERB="$verb" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
clear = os.environ.get("K2_CP_VERB") == "clear"
def size(b):
    for u in ("B", "KB", "MB", "GB", "TB"):
        if b < 1024 or u == "TB":
            return ("%d %s" % (b, u)) if u == "B" else ("%.1f %s" % (b, u))
        b /= 1024.0
def ago(t):
    if not t: return "-"
    d = max(0, int(time.time()) - int(t))
    return "%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400))
rows = []
for n in v.get("nodes") or []:
    if n.get("error") or n.get("note") and not n.get("caches"):
        sys.stderr.write("[k2 compute] %s: %s\n" % (n["node"], n.get("note") or n.get("error")))
    for e in (n.get("cleared") or []) if clear else []:
        print("cleared  %-14s %-20s %-24s %10s" % (n["node"], e.get("workspace") or "(another: %s)" % e["wsKey"][:8], e["name"], size(e["bytes"])))
    for e in n.get("caches") or []:
        rows.append((n["node"], e.get("workspace") or "(unknown %s)" % e["wsKey"][:8], e["name"], size(e["bytes"]), ago(e.get("usedAt")), "busy" if e.get("busy") else ""))
if rows and not clear:
    print("%-14s %-20s %-24s %10s  %-9s %s" % ("NODE", "WORKSPACE", "CACHE", "SIZE", "USED", ""))
    for r in rows:
        print("%-14s %-20s %-24s %10s  %-9s %s" % r)
elif clear:
    for r in rows:
        if r[5] == "busy":
            sys.stderr.write("[k2 compute] %s: %s/%s is held by a running job; not cleared\n" % (r[0], r[1], r[2]))
elif not rows:
    sys.stderr.write("[k2 compute] no job caches\n")
'
}

# Follow a split run (--shards): one line per shard change on stderr,
# then the combined result; exit like a single job (the first failing
# shard's exit code, 3 for infra).
_compute_follow_set() {
    local job="$1" as_json="${2:-0}" seen fails=0
    seen="$(mktemp -t k2-compute-set-XXXXXX)"
    echo "{}" > "$seen"
    # shellcheck disable=SC2064
    trap "rm -f '$seen' '$seen.err'; echo '[k2 compute] detached; the shards keep running. k2 compute get $job | k2 compute cancel $job' >&2; exit 130" INT
    while :; do
        local resp
        resp="$(_compute_send GET /cli/compute/jobs/get "job=$(urlencode "$job")&wait=20")"
        _compute_split "$resp"
        if [ "$_CP_STATUS" = "000" ]; then
            fails=$((fails + 1))
            [ "$fails" -eq 1 ] && echo "[k2 compute] lost the K2 server; retrying (the shards keep running)" >&2
            if [ "$fails" -gt 60 ]; then
                rm -f "$seen"
                echo "{\"error\":\"lost_contact\",\"hint\":\"no answer from the K2 server for 10 minutes: k2 compute get $job\"}" >&2
                exit 3
            fi
            sleep 10
            continue
        fi
        [ "$_CP_STATUS" = "200" ] || { rm -f "$seen"; _compute_fail "$_CP_STATUS" "$_CP_BODY"; }
        fails=0
        local st
        st="$(printf '%s' "$_CP_BODY" | K2_CP_SEEN="$seen" python3 -c '
import json, os, re, sys
v = json.load(sys.stdin)
r = v.get("result") or {}
seen_p = os.environ["K2_CP_SEEN"]
seen = json.load(open(seen_p))
def tl(t):
    return (" · %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else ""
for x in r.get("shards") or []:
    key = "%s|%s|%s|%s" % (x["state"], x.get("attempt"), x.get("node"), x.get("exit"))
    if seen.get(str(x["index"])) != key:
        seen[str(x["index"])] = key
        what = x["state"]
        if x["state"] == "done":
            what = "exit %s" % x.get("exit")
        elif x.get("reason") and x["state"] != "running":
            what += " (%s)" % x["reason"]
        att = (" attempt %s" % x["attempt"]) if (x.get("attempt") or 1) > 1 else ""
        sys.stderr.write("[k2 compute] shard %s/%d on %s%s: %s%s\n" % (x["index"], len(r["shards"]), x.get("node") or "?", att, what, tl(x.get("tests"))))
json.dump(seen, open(seen_p, "w"))
term = v.get("state") in ("done", "failed", "cancelled", "timeout", "interrupted", "unknown")
if term:
    t = r.get("tests")
    took = (" after %dm%02ds" % (r["durationSecs"] // 60, r["durationSecs"] % 60)) if r.get("durationSecs") is not None else ""
    if v["state"] == "done":
        sys.stderr.write("[k2 compute] %d shards: exit %s%s%s · k2 compute logs %s --failures\n" % (len(r.get("shards") or []), r.get("exit"), took, (" · tests %d passed, %d failed" % (t["passed"], t["failed"] + t["errors"])) if t else "", v["short"]))
    else:
        err = {"error": v["state"], "reason": v.get("reason"), "detail": v.get("detail"), "hint": "k2 compute get %s --json; k2 compute retry %s" % (v["short"], v["short"])}
        open(seen_p + ".err", "w").write(json.dumps(err) + "\n")
def s(x):
    return re.sub(r"[^A-Za-z0-9_.:-]", "", str(x if x is not None else ""))
print("|".join(["1" if term else "0", s(v.get("state")), s(r.get("exit")), s(r.get("signal"))]))')"
        local end state code sig
        IFS='|' read -r end state code sig <<<"$st"
        if [ "$end" = "1" ]; then
            trap - INT
            [ "$as_json" = "1" ] && _compute_print_result "$job"
            if [ "$state" = "done" ]; then
                rm -f "$seen" "$seen.err"
                [ -n "$code" ] && exit "$code"
                # The signal comes from the daemon: digits only reach $(( )).
                [[ "$sig" =~ ^[0-9]+$ ]] || sig=""
                exit $((128 + 10#${sig:-0}))
            fi
            cat "$seen.err" >&2
            rm -f "$seen" "$seen.err"
            exit 3
        fi
    done
}

# k2 compute test [<node>] [--shards N] [--partition hash|count]
#   [--profile P] [--runner "cargo nextest run"] [run flags] [-- nextest args]
# One command for a cargo-nextest suite: a nextest partition per shard,
# JUnit kept and parsed, one combined pass/fail.
cmd_compute_test() {
    local node="" any=0 nshards=1 partition="hash" profile="" runner="cargo nextest run" has_nextest_need=0
    local -a pass=() targs=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --) shift; targs=("$@"); break ;;
            --any) any=1; shift ;;
            --shards) _need_value "$@"; nshards="${2:-}"; shift 2 ;;
            --partition) _need_value "$@"; partition="${2:-}"; shift 2 ;;
            --profile) _need_value "$@"; profile="${2:-}"; shift 2 ;;
            --runner) _need_value "$@"; runner="${2:-}"; shift 2 ;;
            --needs|--need)
                _need_value "$@"
                case "${2:-}" in *cargo-nextest*) has_nextest_need=1 ;; esac
                pass+=("$1" "${2:-}"); shift 2 ;;
            --os|--distro|--arch|--label|--env|--secret|--cpus|--mem|--timeout|--sha|--workspace|--artifact|--artifacts|--queue-timeout|--retries|--stall-timeout|--tier|--why|--supersede|--submitter|--repo|--cache|--with|--preset|--tag|--same-node-as|--avoid|--only|--network|--netns-dummy)
                _need_value "$@"
                pass+=("$1" "${2:-}"); shift 2 ;;
            --detach|-d|--dirty|--exclusive|--receipt-v2|--raw|--json|--no-retry|--retry-interrupted|--no-source|--gate|--critical|--now|--cache-required|--plan) pass+=("$1"); shift ;;
            --gpu=*) pass+=("$1"); shift ;;
            --gpu)
                case "${2:-}" in
                    ''|-*) pass+=("$1"); shift ;;
                    *) pass+=("$1" "$2"); shift 2 ;;
                esac ;;
            --gpu-mem) _need_value "$@"; pass+=("$1" "${2:-}"); shift 2 ;;
            -*) echo "usage: unknown flag '$1' (k2 compute test --help)" >&2; exit 2 ;;
            *) if [ -z "$node" ]; then node="$1"; shift; else echo "usage: put nextest's own arguments after --" >&2; exit 2; fi ;;
        esac
    done
    case "$nshards" in ''|*[!0-9]*) echo "usage: --shards N (1–16)" >&2; exit 2 ;; esac
    case "$partition" in hash|count) ;; *) echo "usage: --partition hash|count" >&2; exit 2 ;; esac
    if [ "$nshards" -gt 1 ] && [ -n "$node" ]; then
        echo "usage: --shards picks a node for each shard itself (no node: K2 picks); drop the node name" >&2
        exit 2
    fi
    [ -z "$node" ] && any=1
    local script
    script="$(K2_CT_RUNNER="$runner" K2_CT_PROFILE="$profile" K2_CT_SHARDS="$nshards" K2_CT_PART="$partition" python3 -c '
import os, shlex, sys
runner = shlex.split(os.environ["K2_CT_RUNNER"])
profile = os.environ["K2_CT_PROFILE"]
shards = int(os.environ["K2_CT_SHARDS"])
lines = [
    "set -u",
    # No K2 session variable reaches a test (the prod-isolation guard).
    "for v in $(env | sed -n \"s/^\\(K2[A-Z0-9_]*\\)=.*/\\1/p\"); do unset \"$v\"; done",
    "export XDG_RUNTIME_DIR=\"${TMPDIR:-/tmp}/k2-xdg\"; mkdir -p \"$XDG_RUNTIME_DIR\"; chmod 700 \"$XDG_RUNTIME_DIR\"",
]
cmd = [shlex.quote(w) for w in runner]
if profile:
    cmd += ["--profile", shlex.quote(profile)]
else:
    lines.append("cfg=\"${TMPDIR:-/tmp}/k2-nextest.toml\"")
    lines.append("printf \"%s\\n\" \"[profile.k2]\" \"fail-fast = false\" \"[profile.k2.junit]\" \"path = \\\"junit.xml\\\"\" > \"$cfg\"")
    cmd += ["--config-file", "\"$cfg\"", "--profile", "k2"]
if shards > 1:
    cmd += ["--partition", "%s:{shard}/{shards}" % os.environ["K2_CT_PART"], "--no-tests=pass"]
cmd += [shlex.quote(a) for a in sys.argv[1:]]
lines.append("exec " + " ".join(cmd))
print("\n".join(lines))' "${targs[@]+"${targs[@]}"}")" || exit 2
    local -a run_args=()
    if [ "$any" = "1" ]; then run_args+=(--any); else run_args+=("$node"); fi
    [ "$nshards" -gt 1 ] && run_args+=(--shards "$nshards")
    if [ "$has_nextest_need" = "0" ] && [ "$runner" = "cargo nextest run" ]; then
        run_args+=(--needs cargo-nextest)
    fi
    run_args+=(--artifact 'target/nextest/**/*.xml')
    cmd_compute_run "${run_args[@]}" "${pass[@]+"${pass[@]}"}" -- sh -c "$script"
}

# k2 compute prewarm [--preset P | --needs …] [--os …] [--arch …] [--label k=v]
# [--node N]... [--tag T] [--plan] [--wait] [--json] (0.46.7, CE-18): the
# daemon submits one Batch `sh -c true` job with the needs to each granted
# matching node where a need isn't ready, and reports the ready ones.
cmd_compute_prewarm() {
    local preset="" os="" arch="" tag="" workspace="" plan=0 wait=0 as_json=0 distro=""
    local -a needs=() nodes=() labels=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --preset) _need_value "$@"; preset="${2:-}"; shift 2 ;;
            --needs|--need) _need_value "$@"; needs+=("${2:-}"); shift 2 ;;
            --os) _need_value "$@"; os="${2:-}"; shift 2 ;;
            --distro) distro="$(_compute_distro_arg "${2:-}")" || exit 2; shift 2 ;;
            --arch) _need_value "$@"; arch="${2:-}"; shift 2 ;;
            --label) _need_value "$@"; labels+=("${2:-}"); shift 2 ;;
            --node) _need_value "$@"; nodes+=("${2:-}"); shift 2 ;;
            --tag) _need_value "$@"; tag="${2:-}"; shift 2 ;;
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            --plan|--dry-run) plan=1; shift ;;
            --wait) wait=1; shift ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 compute prewarm [--preset NAME | --needs rust@stable,cmake@3] [--os linux|macos] [--distro ubuntu@24.04] [--arch A] [--label k=v] [--node N]... [--tag T] [--plan] [--wait] [--workspace WS] [--json] (unknown '$1')" >&2; exit 2 ;;
        esac
    done
    # No --preset or --needs: the needs come from .k2/compute.toml's
    # [defaults] (the daemon says so when there are none).
    if [ "$plan" = "1" ] && [ "$wait" = "1" ]; then
        echo "usage: --plan submits nothing, so there is nothing to --wait for" >&2; exit 2
    fi
    local needs_json="" nodes_json="" labels_json="" repo_dir=""
    if [ "${#needs[@]}" -gt 0 ]; then
        needs_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${needs[@]}")"
    fi
    [ "${#nodes[@]}" -gt 0 ] && nodes_json="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${nodes[@]}")"
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
    # The daemon reads .k2/compute.toml from this checkout (or the workspace).
    _k2_git_usable && repo_dir="$(git rev-parse --show-toplevel 2>/dev/null || true)"
    local body
    body="$(_compute_json K2_CJ_preset="$preset" K2_CJ_needs_j="$needs_json" K2_CJ_os="$os" K2_CJ_distro="$distro" K2_CJ_arch="$arch" \
        K2_CJ_labels_j="$labels_json" K2_CJ_nodes_j="$nodes_json" K2_CJ_tag="$tag" K2_CJ_plan_b="${plan#0}" \
        K2_CJ_repoDir="$repo_dir" K2_CJ_workspace="$(_compute_ws "$workspace")")"
    _compute_call POST /cli/compute/prewarm "$body"
    if [ "$as_json" = "1" ] && [ "$wait" = "0" ]; then
        printf '%s\n' "$_CP_BODY"; exit 0
    fi
    local ids
    ids="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("nodes") or []:
    st = r.get("state")
    if st == "ready":
        s = "ready already"
    elif st == "installing":
        s = "installing %s (job %s)" % (", ".join(r.get("installs") or []), r.get("short"))
    elif st == "would_install":
        s = "would install %s" % ", ".join(r.get("installs") or [])
    elif st in ("asked", "would_ask"):
        # 0.46.9: system packages its owner must approve first.
        s = "%s its owner for %s" % ("asked" if st == "asked" else "would ask", ", ".join(r.get("ask") or []))
    elif st == "cant_install":
        s = "cannot install there"
    else:
        e = r.get("error") or {}
        s = "refused: %s: %s" % (e.get("code"), e.get("hint"))
    if r.get("note"): s += " · " + r["note"]
    sys.stderr.write("[k2 compute] %s: %s\n" % (r.get("node"), s))
if v.get("fits"): sys.stderr.write("[k2 compute] %s\n" % v["fits"])
rq = v.get("request")
if isinstance(rq, dict) and rq.get("id"):
    sys.stderr.write("[k2 compute] package request %s: %s. Prewarm again once it is installed (k2 compute node packages requests)\n" % (rq["id"], rq.get("summary") or ""))
elif isinstance(rq, dict) and isinstance(rq.get("error"), dict):
    sys.stderr.write("[k2 compute] package request not filed: %s: %s\n" % (rq["error"].get("code"), rq["error"].get("hint")))
print(" ".join(v.get("jobs") or []))')"
    if [ "$wait" = "1" ] && [ -n "$ids" ]; then
        local -a wargs=()
        read -r -a wargs <<< "$ids"
        [ "$as_json" = "1" ] && wargs+=(--json)
        cmd_compute_wait "${wargs[@]}"
    fi
    [ "$as_json" = "1" ] && printf '%s\n' "$_CP_BODY"
    # 0.46.9: a package request it filed for this server's owner is a hold.
    _compute_pkg_held_exit
    exit 0
}

