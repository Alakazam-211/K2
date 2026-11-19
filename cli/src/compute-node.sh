cmd_compute_node() {
    local sub="${1:-}"
    shift 2>/dev/null || true
    local body
    case "$sub" in
        add)
            local name="" url="" add_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --url) url="${2:-}"; shift 2 ;;
                    --json) add_json=1; shift ;;
                    *) name="$1"; shift ;;
                esac
            done
            [ -n "$name" ] || { echo "usage: k2 compute node add <name> [--url URL] [--json]" >&2; exit 2; }
            body="$(_compute_json K2_CJ_name="$name" K2_CJ_url="$url")"
            _compute_call POST /cli/compute/nodes/enroll-code "$body" tcp
            if [ "$add_json" = "1" ]; then
                # One stable object for scripts (2026-10-28): the CODE.FP
                # string k2-node takes, its expiry, the node and controller.
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print(json.dumps({"code": v["enroll"], "expiresAt": v["expiresAt"], "name": v["name"], "controller": v["controllerUrl"]}))'
            else
                _compute_render enroll
            fi
            ;;
        confirm)
            [ $# -ge 2 ] || { echo "usage: k2 compute node confirm <name> <6-digit code>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_sas="${*:2}")"
            _compute_call POST /cli/compute/nodes/confirm "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s is active. %s" % (v["node"], v["next"]))'
            ;;
        remove|rm)
            [ $# -ge 1 ] || { echo "usage: k2 compute node remove <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1")"
            _compute_call POST /cli/compute/nodes/remove "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("removed %s (%d jobs cancelled)" % (v["node"], v["jobsCancelled"]))'
            ;;
        retire)
            [ $# -ge 1 ] || { echo "usage: k2 compute node retire <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1")"
            _compute_call POST /cli/compute/nodes/retire "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], "removed" if v["state"] == "revoked" else "draining; removed once its %d running job(s) end" % v.get("running", 0)))'
            ;;
        rename)
            [ $# -ge 2 ] || { echo "usage: k2 compute node rename <old> <new>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_name="$2")"
            _compute_call POST /cli/compute/nodes/rename "$body" tcp
            echo "renamed to $2"
            ;;
        pause|drain|resume)
            [ $# -ge 1 ] || { echo "usage: k2 compute node $sub <name>" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_action="$sub")"
            _compute_call POST /cli/compute/nodes/pause "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: server-side %s. %s" % (v["node"], v.get("controllerPause") or "active", v["note"]))'
            ;;
        pinned-only)
            # On: automatic placement never chooses the node; only a job
            # that names it runs there. The owner or an Admin only: the
            # daemon refuses an agent, a managing one too.
            local po_on=""
            case "${2:-}" in on) po_on=1 ;; off) po_on=0 ;; esac
            { [ $# -eq 2 ] && [ -n "$po_on" ]; } || { echo "usage: k2 compute node pinned-only <name> on|off" >&2; exit 2; }
            body="$(_compute_json K2_CJ_node="$1" K2_CJ_on_b="$po_on")"
            _compute_call POST /cli/compute/nodes/pinned-only "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: pinned-only %s%s. %s." % (v["node"], "on" if v["pinnedOnly"] else "off", "" if v.get("changed") else " (it already was)", v["note"]))'
            ;;
        route|routes)
            local op="${1:-}"
            [ $# -ge 3 ] || { echo "usage: k2 compute node route add|rm <name> <url>" >&2; exit 2; }
            case "$op" in
                add) body="$(_compute_json K2_CJ_node="$2" K2_CJ_add="$3")" ;;
                rm|remove) body="$(_compute_json K2_CJ_node="$2" K2_CJ_remove="$3")" ;;
                *) echo "usage: k2 compute node route add|rm <name> <url>" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/compute/nodes/routes "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("routes for %s: %s" % (v["node"], " ".join(v["routes"]) or "(none)"))'
            ;;
        admin) cmd_compute_node_admin "$@" ;;
        servers) cmd_compute_node_servers "$@" ;;
        packages|pkg) cmd_compute_node_packages "$@" ;;
        take-owner)
            # MT36 (0.46.4): a node that says this server owns it now (an
            # owner change on it) is taken over only by the owner, in person.
            # The node's word is only a claim: the person sees the full key,
            # the old owner and what moves here, and types the node's name.
            [ $# -eq 1 ] && [ "${1#-}" = "$1" ] || { echo "usage: k2 compute node take-owner <node>" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/take-owner "$(_compute_json K2_CJ_node="$1" K2_CJ_preview_j=true)" tcp
            local tname tfp
            tname="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["node"])')"
            tfp="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["fingerprint"])')"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("Take over %s?" % v["node"])
print("  node key (full): %s" % v["fingerprint"])
print("  owner until now: %s" % (v.get("from") or "another server"))
print("  The node only SAYS this server owns it. Go on only if you changed its owner yourself and that key is the node you know.")
print("  This server becomes its admin (root admin on that machine: updates, policy, servers, approvals).")
print("  Every compute:<workspace>/* secret grant starts sending its secrets to it:")
if not v.get("grantsKnown"):
    print("    " + v["secretGrantsText"])
elif not v.get("secretGrants"):
    print("    none today")
else:
    for g in v["secretGrants"]:
        print("    %s: %s" % (g["workspace"], g["secret"]))' >&2
            [ -r /dev/tty ] || { echo '{"error":"needs_a_person","hint":"take-owner asks you to type the node name: run it in a terminal"}' >&2; exit 3; }
            printf 'Type the node name (%s) to take it over: ' "$tname" >&2
            local typed=""
            IFS= read -r typed </dev/tty || typed=""
            [ "$typed" = "$tname" ] || { echo '{"error":"not_confirmed","hint":"the name did not match; nothing changed"}' >&2; exit 3; }
            local tid
            _compute_call GET /cli/compute/nodes/info "node=$(urlencode "$tname")"
            tid="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
            local tok
            tok="$(_vault_confirm_token node-admin "take-owner:$tid" "take over $tname ($tfp)")" || exit 3
            _compute_call POST /cli/compute/nodes/take-owner "$(_compute_json K2_CJ_node="$tid" K2_CJ_confirmToken="$tok")" tcp
            tok=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); print("%s is this server'"'"'s now (taken over from %s): its admin, updates, servers and approvals are here." % (v["node"], v.get("from") or "its owner"))'
            ;;
        policy)
            local node="${1:-}" act="${2:-show}"
            [ -n "$node" ] || { echo "usage: k2 compute node policy <node> show|set key=value..." >&2; exit 2; }
            shift 2 2>/dev/null || shift $#
            case "$act" in
                show)
                    _compute_call GET /cli/compute/nodes/policy "node=$(urlencode "$node")"
                    for a in "$@"; do [ "$a" = "--json" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }; done
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print("%s (%s, remote admin %s)" % (v["node"], "online" if v["online"] else "offline", v.get("remoteAdmin") or "unknown"))
p=v.get("policy")
if not p: print("  " + v.get("hint",""))
else:
    for k in sorted(p): print("  %-18s %s" % (k, json.dumps(p[k])))'
                    ;;
                set)
                    local reason="" pairs=()
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --reason) reason="${2:-}"; shift 2 ;;
                            *=*) pairs+=("$1"); shift ;;
                            *) echo "usage: k2 compute node policy <node> set key=value... [--reason …]" >&2; exit 2 ;;
                        esac
                    done
                    [ ${#pairs[@]} -gt 0 ] || { echo "usage: k2 compute node policy <node> set key=value..." >&2; exit 2; }
                    local set_json
                    set_json="$(_compute_kv_json "${pairs[@]}")" || exit 2
                    _compute_call POST /cli/compute/nodes/policy/set "$(_compute_json K2_CJ_node="$node" K2_CJ_set_j="$set_json" K2_CJ_reason="$reason")"
                    if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                        _compute_admin_held_or_approve
                    fi
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], v.get("note","ok")))'
                    ;;
                *) echo "usage: k2 compute node policy <node> show|set key=value..." >&2; exit 2 ;;
            esac
            ;;
        health)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node health <node> [--json]" >&2; exit 2; }
            _compute_call GET /cli/compute/nodes/health "node=$(urlencode "$1")"
            for a in "$@"; do [ "$a" = "--json" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }; done
            _compute_render_health
            ;;
        update|rollback)
            local node="" canary=0 now="" all="" batch=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --canary) canary=1; shift ;;
                    --now) now=1; shift ;;
                    --all) all=1; shift ;;
                    --batch)
                        batch="${2:-}"
                        case "$batch" in ''|*[!0-9]*) echo "usage: --batch N (a number of nodes; 0 = all at once)" >&2; exit 2 ;; esac
                        shift 2 ;;
                    -*) echo "usage: k2 compute node $sub <node> [--now]   (update also: --all [--batch N])" >&2; exit 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            if [ "$all" = "1" ]; then
                [ "$sub" = "update" ] && [ -z "$node" ] || { echo "usage: k2 compute node update --all [--batch N] [--now]   (no node name with --all)" >&2; exit 2; }
                _compute_call POST /cli/compute/nodes/update "$(_compute_json K2_CJ_all_b=1 K2_CJ_now_b="$now" K2_CJ_batch_j="${batch:-0}")"
                _compute_render_rollout 0
                return 0
            fi
            [ -n "$batch" ] && { echo "usage: --batch goes with --all" >&2; exit 2; }
            [ -n "$node" ] || { echo "usage: k2 compute node $sub <node> [--now]" >&2; exit 2; }
            _compute_call POST "/cli/compute/nodes/$sub" "$(_compute_json K2_CJ_node="$node" K2_CJ_canary_b="$canary" K2_CJ_now_b="$now")"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); u=v.get("update") or {}
print("%s: %s%s. %s" % (v["node"], u.get("state"), (" " + u["version"]) if u.get("version") else "", v.get("note","")))'
            ;;
        rollout)
            local verb="show" as_json=0 id=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    show|pause|resume|stop) verb="$1"; shift ;;
                    --json) as_json=1; shift ;;
                    --id) id="${2:-}"; shift 2 ;;
                    *) echo "usage: k2 compute node rollout [show|pause|resume|stop] [--id ID] [--json]" >&2; exit 2 ;;
                esac
            done
            if [ "$verb" = "show" ]; then
                _compute_call GET /cli/compute/nodes/rollout ""
            else
                _compute_call POST "/cli/compute/nodes/rollout/$verb" "$(_compute_json K2_CJ_id="$id")"
            fi
            _compute_render_rollout "$as_json"
            ;;
        wall)
            # Q5 (0.46.4): turn the Linux job wall on; operate on 0.46.4+ nodes.
            local node="" restart=1 reason=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --no-restart) restart=0; shift ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    -*) echo "usage: k2 compute node wall <node> [--no-restart] [--reason \"…\"]" >&2; exit 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] || { echo "usage: k2 compute node wall <node> [--no-restart] [--reason \"…\"]" >&2; exit 2; }
            local restart_j=true
            [ "$restart" = "0" ] && restart_j=false
            _compute_call POST /cli/compute/nodes/wall "$(_compute_json K2_CJ_node="$node" K2_CJ_restart_j="$restart_j" K2_CJ_reason="$reason")"
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["node"], v.get("note","ok")))'
            ;;
        alerts)
            local node="" after="" to="" have_to=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --after) after="${2:-}"; shift 2 ;;
                    --to) to="${2:-}"; have_to=1; shift 2 ;;
                    *) node="$1"; shift ;;
                esac
            done
            [ -n "$node" ] || [ "$have_to" = "1" ] || { echo "usage: k2 compute node alerts <node> [--after 10m|off] | --to <workspace>|none" >&2; exit 2; }
            local after_j=""
            if [ -n "$node" ]; then
                case "$after" in
                    "") after_j="" ;;
                    off|none) after_j='"off"' ;;
                    *) after_j="$(_compute_secs "$after")" ;;
                esac
            fi
            local to_val=""
            [ "$have_to" = "1" ] && to_val="${to:-none}"
            _compute_call POST /cli/compute/nodes/alerts "$(_compute_json K2_CJ_node="$node" K2_CJ_after_j="$after_j" K2_CJ_to="$to_val")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if "alertsTo" in v: print("alerts go to: %s" % (v["alertsTo"] or "nobody (unset)"))
if "node" in v:
    a=v.get("alertAfterSecs")
    print("%s: %s" % (v["node"], ("offline alert after %dm" % (a // 60)) if a else "no offline alert"))'
            ;;
        *)
            echo "usage: k2 compute node add|confirm|remove|rename|pause|drain|resume|pinned-only|route|health|policy|update|rollout|rollback|wall|alerts|admin|servers|packages …" >&2
            exit 2
            ;;
    esac
}

# A k2-node rollout (§5.10): one line, then each node's place.
_compute_render_rollout() {
    if [ "${1:-0}" = "1" ]; then
        printf '%s\n' "$_CP_BODY"
        return 0
    fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v.get("rollout")
if not r:
    print("no rollout yet (k2 compute node update --all starts one)%s" % ("" if v.get("follow", True) else "; auto-follow (compute.nodes.follow) is off"))
    sys.exit(0)
p=r.get("position") or {}
how="all at once" if r.get("now") else "a few per OS/arch at a time" + ((", at most %d in all" % r["batchSize"]) if r.get("batchSize") else "")
print("rollout %s: k2-node %s, %s, %s (%s of %s settled; %s)" % (r["id"][:8], r["toVersion"], r["state"], r["trigger"], p.get("settled"), p.get("total"), how))
if r.get("note"): print("  " + r["note"])
for n in r.get("nodes") or []:
    why=(" " + n["code"]) if n.get("code") else ""
    print("  %-14s %s%s%s" % (n["node"], n["state"], why, (": " + n["detail"]) if n.get("detail") and (n["state"] in ("skipped","failed","refused","timed_out") or (n.get("code") or "").startswith("waiting_")) else ""))
if v.get("note"): print(v["note"])'
}

# A package request held for this server's owner (`pkgHeld`): say so and
# exit 2, like every held change. With permission cards on, the daemon's
# Ask with: line (k2 permission request compute.packages request=<id> …)
# prints last, at exit, and brings the request to the owner as a card.
_compute_pkg_held_exit() {
    printf '%s' "$_CP_BODY" | grep -q '"pkgHeld": *true' || return 0
    local id
    id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("request") or {}).get("id") or "")')"
    echo "held for the owner: package request $id" >&2
    echo "  the owner approves it on the card, in Settings → Cores → Compute → Machine settings → Waiting for you, or outside K2: k2 compute node packages approve $id" >&2
    if _permission_in_body "$_CP_BODY"; then
        echo "  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card." >&2
    fi
    echo "  you get a [compute] message when it's decided; then run your job again" >&2
    exit 2
}

# `k2 compute node packages …` (0.46.9, prd-compute-self-serve-environments-v1
# §6.3): what a node can install for this server, and asking its owner for
# more. Thin: every rule lives in the daemon (compute_pkg.rs).
_compute_node_packages_usage() {
    cat <<'EOF'
k2 compute node packages — system packages on a compute node (0.46.9)

  k2 compute node packages <node> [--json]
        the node's host line, its package manager, the policy for this
        server there, the approved and installed names, and your recent
        requests
  k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why "…" [--job ID] [--json]
        ask the node's owner to install them: one request for every name
        on every node named. A name is a Debian name (libssl-dev; K2 maps
        it for each distro), a native one (pacman:openssl, dnf:openssl-devel)
        or a K2 profile (profile:tauri-linux). A job whose --needs aren't
        covered files the same request by itself. Held for this server's
        owner: exit 2, and the Ask with: line puts it on their card. You
        get a [compute] message when the owner decides; then run your job
        again
  k2 compute node packages requests [--node N] [--state S] [--limit N] [--json]
        your requests (the owner or an Admin outside K2: every request).
        S: pending, approved, installed, denied, failed, expired or all.
        installed = run your job again; denied, failed, expired = it won't
  k2 compute node packages approve|deny <request> [--add] [--note "…"]
        the owner or an Admin, from a terminal outside K2 (an agent never
        decides). <request> is the cpr_… id that requests shows (a start of
        it that fits one is enough). The card in chat and Settings → Cores →
        Compute → Machine settings → Waiting for you are the main path. approve --add also puts the
        names on that server's approved list, so the next ask for them
        needs no card
  k2 compute node packages policy <node> [--json]
        what this server may install there without asking
  k2 compute node packages policy <node> [<server>] [level=list|profiles|official]
        [profiles=a,b] [max_install_mb=N] [--reason "…"]
        change it for <server>: own (this server, the default), or another
        server's label as k2 compute node servers <node> shows it. The
        node's owner only. profiles= takes bare K2 profile names
        (build-base, tauri-linux, rust-gui-ci, k2-workspace, gpu-headless) and replaces
        the server's list. Narrowing applies at once; widening is held for
        the owner's approval (exit 2). Every server's policy, at the
        machine: k2-node packages list
EOF
}

cmd_compute_node_packages() {
    local first="${1:-}"
    case "$first" in
        ""|help|--help|-h) _compute_node_packages_usage; return 0 ;;
    esac
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$first" in
        request)
            shift
            local nodes="" why="" job="" ask_usage='usage: k2 compute node packages request <node>[,<node>…] <name>[,<name>…] --why "…" [--job ID]'
            local -a names=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) why="${2:-}"; shift 2 ;;
                    --job) job="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$ask_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *) if [ -z "$nodes" ]; then nodes="$1"; else names+=("$1"); fi; shift ;;
                esac
            done
            [ -n "$nodes" ] && [ "${#names[@]}" -gt 0 ] || { echo "$ask_usage" >&2; exit 2; }
            [ -n "$why" ] || { echo "$ask_usage (--why: one line the owner reads on the card, e.g. \"tauri build needs webkit2gtk\")" >&2; exit 2; }
            local nodes_json names_json
            nodes_json="$(python3 -c 'import json,sys; print(json.dumps([x.strip() for x in sys.argv[1].split(",") if x.strip()]))' "$nodes")"
            names_json="$(python3 -c '
import json, sys
out = []
for a in sys.argv[1:]:
    out += [x.strip() for x in a.split(",") if x.strip()]
print(json.dumps(out))' "${names[@]}")"
            _compute_call POST /cli/compute/nodes/packages/request "$(_compute_json K2_CJ_nodes_j="$nodes_json" K2_CJ_names_j="$names_json" K2_CJ_why="$why" K2_CJ_job="$job" K2_CJ_workspace="$(_compute_ws "")")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; _compute_pkg_held_exit; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v.get("request") or {}
print("request %s: %s" % (r.get("id"), r.get("summary") or ""))
for row in r.get("rows") or []:
    d = row.get("dry") or {}
    extra = ""
    if d.get("installs"):
        extra = " · installs %d package(s)%s" % (len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else "")
    if d.get("refusal"):
        extra += " · would be refused: " + d["refusal"]
    print("  %-14s %-9s %s%s" % (row.get("node"), row.get("state"), " ".join(row.get("names") or []), extra))
for c in v.get("cant") or []:
    print("  not asked: " + c)
if v.get("hint"):
    print(v["hint"])'
            # Held for this server's owner: exit 2 (sent to another
            # server's owner only: exit 0, you hear back by message).
            _compute_pkg_held_exit
            ;;
        requests|list|ls)
            shift
            local q=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node packages requests [--node N] [--state pending|approved|installed|denied|failed|expired|all] [--limit N] [--json]" >&2; exit 2 ;;
                esac
            done
            # The owner's view needs no passport: outside K2 it goes over TCP.
            if _compute_in_session; then
                _compute_call GET /cli/compute/nodes/packages/requests "${q#&}"
            else
                _compute_call GET /cli/compute/nodes/packages/requests "${q#&}" tcp
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("requests") or []
if not rows:
    print("no package requests")
def ago(t):
    d = max(int(time.time()) - int(t or 0), 0)
    return "now" if d < 60 else ("%dm ago" % (d // 60) if d < 3600 else ("%dh ago" % (d // 3600) if d < 86400 else "%dd ago" % (d // 86400)))
seen = set()
for r in rows:
    origin = {"own": "", "sent": "asked %s" % r.get("server"), "fwd": "from %s" % r.get("server")}.get(r.get("origin"), r.get("origin") or "")
    print("%-21s %-9s %-14s %-8s %s%s" % (r.get("batchId"), r.get("state"), r.get("node"), ago(r.get("createdAt")), " ".join(r.get("names") or []), ("  (" + origin + ")") if origin else ""))
    who = r.get("askedBy") or "?"
    print("%21s asked by %s%s%s" % ("", who, (" for job " + r["jobId"][:8]) if r.get("jobId") else "", (": " + r["why"]) if r.get("why") else ""))
    d = r.get("dry") or {}
    if d.get("installs"):
        print("%21s installs %d package(s)%s" % ("", len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else ""))
    if d.get("refusal"):
        print("%21s would be refused: %s" % ("", d["refusal"]))
    if r.get("note") or r.get("decidedBy"):
        print("%21s %s%s" % ("", ("by " + r["decidedBy"] + " ") if r.get("decidedBy") else "", r.get("note") or ""))
if v.get("canDecide") and any(r.get("state") == "pending" and r.get("origin") != "sent" for r in rows):
    print("decide: k2 compute node packages approve|deny <request> [--add] [--note \"…\"]")'
            ;;
        approve|deny)
            shift
            local id="" note="" add=0 dec_usage="usage: k2 compute node packages $first <request> [--note \"…\"]"
            [ "$first" = "approve" ] && dec_usage="usage: k2 compute node packages approve <request> [--add] [--note \"…\"]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --note|--why) note="${2:-}"; shift 2 ;;
                    --add) add=1; shift ;;
                    --json) shift ;;
                    -*) echo "$dec_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *) [ -z "$id" ] || { echo "$dec_usage (one request)" >&2; exit 2; }; id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "$dec_usage" >&2; exit 2; }
            [ "$first" = "deny" ] && [ "$add" = "1" ] && { echo "usage: --add goes with approve (it puts the names on the server's approved list)" >&2; exit 2; }
            if _compute_in_session; then
                echo '{"error":"owner_only","hint":"only the owner or an Admin decides package requests, in person: the card in chat, Settings → Cores → Compute → Machine settings → Waiting for you, or this command from a terminal outside K2. An agent can never approve."}' >&2
                exit 3
            fi
            # Show what is decided first: the names, nodes and install set.
            _compute_call GET /cli/compute/nodes/packages/requests "state=pending&limit=500" tcp
            local full
            full="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin).get("requests") or [] if r.get("origin") != "sent"]
ids = sorted({r["batchId"] for r in rows if r["batchId"] == want or r["id"] == want or r["batchId"].startswith(want)})
if len(ids) != 1:
    sys.exit(1)
b = ids[0]
print(b)
for r in rows:
    if r["batchId"] != b:
        continue
    d = r.get("dry") or {}
    sys.stderr.write("  %s on %s (%s), for %s%s\n" % (" ".join(r.get("names") or []), r.get("node"), r.get("host") or "?", r.get("askedBy") or "?", (" via " + r["server"]) if r.get("origin") == "fwd" else ""))
    if r.get("why"):
        sys.stderr.write("    why: %s\n" % r["why"])
    if d.get("installs"):
        sys.stderr.write("    installs %d package(s)%s: %s\n" % (len(d["installs"]), (", %s MB" % d["size_mb"]) if d.get("size_mb") is not None else "", " ".join(d["installs"])))
' "$id")" || full="$id"
            local approve_j=false
            [ "$first" = "approve" ] && approve_j=true
            local add_j=""
            [ "$add" = "1" ] && add_j=true
            _compute_call POST /cli/compute/nodes/packages/decide "$(_compute_json K2_CJ_request="$full" K2_CJ_approve_j="$approve_j" K2_CJ_add_j="$add_j" K2_CJ_note="$note")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("rows") or []:
    print("%s %s on %s%s" % (r.get("state"), r.get("request"), r.get("node"), (": " + r["note"]) if r.get("note") else ""))'
            ;;
        policy)
            shift
            local node="" server="" reason="" pol_usage='usage: k2 compute node packages policy <node> [<server>] [level=list|profiles|official] [profiles=a,b] [max_install_mb=N] [--reason "…"]'
            local -a pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$pol_usage (unknown flag '$1')" >&2; exit 2 ;;
                    *=*) pairs+=("$1"); shift ;;
                    *)
                        if [ -z "$node" ]; then node="$1"
                        elif [ -z "$server" ]; then server="$1"
                        else echo "$pol_usage" >&2; exit 2
                        fi
                        shift ;;
                esac
            done
            [ -n "$node" ] || { echo "$pol_usage" >&2; exit 2; }
            if [ "${#pairs[@]}" -eq 0 ]; then
                # Show: this server's policy there (the node reports only
                # the caller's; every server's is at the machine).
                _compute_call GET /cli/compute/nodes/packages "node=$(urlencode "$node")"
                [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
                printf '%s' "$_CP_BODY" | K2_CP_SERVER="$server" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
p = v.get("packages")
if not isinstance(p, dict):
    print("%s: %s" % (v.get("node"), v.get("note") or "no package status"))
    sys.exit(0)
print("%s: %s for this server" % (v.get("node"), p.get("policy")))
if os.environ.get("K2_CP_SERVER") not in ("", "own"):
    print("  (a node reports only the asking server'"'"'s policy; every server'"'"'s, at the machine: k2-node packages list)")'
                exit 0
            fi
            local body
            body="$(K2_PP_NODE="$node" K2_PP_SERVER="${server:-own}" K2_PP_REASON="$reason" python3 -c '
import json, os, sys
out = {"node": os.environ["K2_PP_NODE"], "server": os.environ["K2_PP_SERVER"]}
if os.environ.get("K2_PP_REASON"):
    out["reason"] = os.environ["K2_PP_REASON"]
for kv in sys.argv[1:]:
    k, _, v = kv.partition("=")
    if k == "level":
        if v not in ("list", "profiles", "official"):
            sys.stderr.write("usage: level=list|profiles|official\n"); sys.exit(2)
        out["level"] = v
    elif k == "profiles":
        out["profiles"] = [x.strip() for x in v.split(",") if x.strip()]
    elif k == "max_install_mb":
        if not v.isdigit():
            sys.stderr.write("usage: max_install_mb=N (megabytes per request, 1 to 65536)\n"); sys.exit(2)
        out["maxInstallMb"] = int(v)
    else:
        sys.stderr.write("usage: %s: the keys are level, profiles and max_install_mb\n" % k); sys.exit(2)
print(json.dumps(out))' "${pairs[@]}")" || exit 2
            _compute_call POST /cli/compute/nodes/packages/policy "$body"
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v.get("node"), v.get("note") or "ok"))'
            ;;
        -*)
            echo "usage: k2 compute node packages <node> | request | requests | approve | deny | policy … (k2 compute node packages --help)" >&2
            exit 2
            ;;
        *)
            # k2 compute node packages <node> [--json]
            local node="$first"
            shift
            for a in "$@"; do
                [ "$a" = "--json" ] || { echo "usage: k2 compute node packages <node> [--json]" >&2; exit 2; }
            done
            _compute_call GET /cli/compute/nodes/packages "node=$(urlencode "$node")"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
print("%s%s" % (v.get("node"), ("  host: " + v["hostLine"]) if v.get("hostLine") else ""))
p = v.get("packages")
if not isinstance(p, dict):
    print("  " + (v.get("note") or "no package status from this node"))
else:
    print("  manager    %s" % (p.get("manager") or "?"))
    print("  policy     %s (for this server; set by %s)" % (p.get("policy"), v.get("owner") or "its owner"))
    ap = p.get("approved") or []
    print("  approved   %s" % (" ".join(ap) if ap else "(none)"))
    ins = p.get("installed") or []
    print("  installed  %s" % (" ".join(ins) if ins else "(none of those reported)"))
    age = p.get("indexAgeSecs")
    if isinstance(age, (int, float)) and age > 86400:
        print("  index      %dd old (K2 refreshes apt/zypper before an install; an Arch node needs its owner to upgrade first)" % (age // 86400))
    if p.get("openRequests"):
        print("  open       %s request(s) wait for its owner" % p["openRequests"])
if not v.get("takesRequests"):
    print("  requests   this node takes none from K2 (older k2-node, or a Mac): its owner installs at the machine")
rows = v.get("requests") or []
for r in rows:
    print("  %-21s %-9s %s" % (r.get("batchId"), r.get("state"), " ".join(r.get("names") or [])))
print("ask for more: k2 compute node packages request %s <name>[,<name>…] --why \"…\"" % v.get("node"))'
            ;;
    esac
}

# `k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …]`
# (node sharing, 0.46.3: prd-compute-shared-queue-v1 §9; owner, 0.46.4:
# prd-macos-tenant-compute-v1 §6).
cmd_compute_node_servers() {
    local node="${1:-}"
    [ -n "$node" ] && [ "${node#-}" = "$node" ] || { echo "usage: k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …] (k2 compute --help)" >&2; exit 2; }
    shift
    local act="${1:-list}"
    [ "$act" = "--json" ] && act=list
    [ $# -gt 0 ] && [ "$act" != "list" ] && shift
    [ "$act" = "list" ] && [ "${1:-}" = "list" ] && shift
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$act" in
        list|ls)
            local q="node=$(urlencode "$node")"
            for a in "$@"; do [ "$a" = "--refresh" ] && q="$q&refresh=1"; done
            _compute_call GET /cli/compute/nodes/servers "$q" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            _compute_render servers
            ;;
        add)
            local url="" code="" trust="own" forv="" reason="" pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --url) url="${2:-}"; shift 2 ;;
                    --code) code="${2:-}"; shift 2 ;;
                    --trust) trust="${2:-}"; shift 2 ;;
                    --for) forv="${2:-}"; shift 2 ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    --weight) pairs+=("weight=${2:-}"); shift 2 ;;
                    --max-running) pairs+=("max_running=${2:-}"); shift 2 ;;
                    --max-queued) pairs+=("max_queued=${2:-}"); shift 2 ;;
                    --cpu-hours) pairs+=("cpu_hours_day=${2:-}"); shift 2 ;;
                    --max-time) pairs+=("max_job_secs=$(_compute_secs "${2:-}")"); shift 2 ;;
                    --cpus) pairs+=("max_cpus=${2:-}"); shift 2 ;;
                    --mem) local m="${2:-}"; pairs+=("max_mem_gb=${m%[Gg]}"); shift 2 ;;
                    --disk) local d="${2:-}"; pairs+=("disk_gb=${d%[Gg]}"); shift 2 ;;
                    --network) pairs+=("network=\"${2:-}\""); shift 2 ;;
                    --gate) pairs+=("gate=true" "top_class=\"gate\""); shift ;;
                    --exclusive) pairs+=("exclusive=true"); shift ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node servers <node> add --url URL --code -|CODE.FP [--trust own|team] [--for 7d|none] [limits…] (unknown '$1')" >&2; exit 2 ;;
                esac
            done
            # The code off argv (2026-10-28): --code - reads one line from
            # stdin; no --code at all takes $K2_NODE_ENROLL_CODE.
            if [ "$code" = "-" ]; then
                IFS= read -r code || true
                code="${code%$'\r'}"
            elif [ -z "$code" ] && _envn_any NODE_ENROLL_CODE; then
                _envn_into code NODE_ENROLL_CODE
            fi
            [ -n "$url" ] && [ -n "$code" ] || { echo "usage: k2 compute node servers <node> add --url URL --code - < code-file (or K2_NODE_ENROLL_CODE) [--trust own|team] [--for 7d|none]" >&2; exit 2; }
            local for_n=""
            case "$forv" in
                "") for_n="" ;;
                none|never|0) for_n=0 ;;
                *d) for_n=$(( ${forv%d} * 86400 )) ;;
                *) for_n="$(_compute_secs "$forv")" ;;
            esac
            local set_json
            set_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
            _compute_call POST /cli/compute/nodes/servers/add "$(_compute_json K2_CJ_node="$node" K2_CJ_url="$url" K2_CJ_code_s="$code" \
                K2_CJ_trust="$trust" K2_CJ_forSecs_n="$for_n" K2_CJ_set_j="$set_json" K2_CJ_reason="$reason")" tcp
            code=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if v.get("warning"): print("note: " + v["warning"], file=sys.stderr)'
            _compute_admin_held_or_approve
            ;;
        approve) [ -n "${1:-}" ] || { echo "usage: k2 compute node servers <node> approve <request>" >&2; exit 2; }; _compute_admin_approve "$1" ;;
        deny) cmd_compute_node_admin deny "$@" ;;
        limits)
            local server="${1:-}" reason="" forv="" prio="" pairs=()
            [ -n "$server" ] && [ "${server#-}" = "$server" ] || { echo "usage: k2 compute node servers <node> limits <server> [--weight N] [--reason TEXT] key=value…" >&2; exit 2; }
            shift
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    # Tiers (0.46.5): does this server's --tier priority count
                    # on the node? Its own body key (kept in the node's
                    # queue.toml, never in the share); a 0.46.5 node only.
                    priority=true|priority=yes|priority=1) prio=1; shift ;;
                    priority=false|priority=no|priority=0) prio=0; shift ;;
                    priority=*) echo "usage: priority=true|false" >&2; exit 2 ;;
                    --weight) pairs+=("weight=${2:-}"); shift 2 ;;
                    for=*) forv="${1#for=}"; shift ;;
                    --for) forv="${2:-}"; shift 2 ;;
                    max_job_secs=*h|max_job_secs=*m|max_job_secs=*s) pairs+=("max_job_secs=$(_compute_secs "${1#max_job_secs=}")"); shift ;;
                    network=*|top_class=*) pairs+=("${1%%=*}=\"${1#*=}\""); shift ;;
                    *=*) pairs+=("$1"); shift ;;
                    --json) shift ;;
                    *) echo "usage: k2 compute node servers <node> limits <server> key=value… (unknown '$1')" >&2; exit 2 ;;
                esac
            done
            local for_n=""
            case "$forv" in
                "") for_n="" ;;
                none|never|0) for_n=0 ;;
                *d) for_n=$(( ${forv%d} * 86400 )) ;;
                *) for_n="$(_compute_secs "$forv")" ;;
            esac
            [ ${#pairs[@]} -gt 0 ] || [ -n "$for_n" ] || [ -n "$prio" ] || { echo "usage: k2 compute node servers <node> limits <server> key=value… (at least one)" >&2; exit 2; }
            local set_json
            set_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
            _compute_call POST /cli/compute/nodes/servers/limits "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_set_j="$set_json" K2_CJ_forSecs_n="$for_n" K2_CJ_reason="$reason" K2_CJ_priority_b="$prio")" tcp
            if printf '%s' "$_CP_BODY" | grep -q '"held": *true'; then
                _compute_admin_held_or_approve
            fi
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s on %s: %s" % (v.get("server"), v["node"], v.get("note","ok")))'
            ;;
        pause|resume)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node servers <node> $act <server>" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/servers/pause "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$1" K2_CJ_action="$act")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s on %s: %s. %s" % (v["server"], v["node"], v["action"].split(".")[-1] + "d", v["note"]))'
            ;;
        revoke)
            local server="" now=0 wipe=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --now) now=1; shift ;;
                    --wipe) wipe=1; shift ;;
                    --json) shift ;;
                    -*) echo "usage: k2 compute node servers <node> revoke <server> [--now] [--wipe]" >&2; exit 2 ;;
                    *) server="$1"; shift ;;
                esac
            done
            [ -n "$server" ] || { echo "usage: k2 compute node servers <node> revoke <server> [--now] [--wipe]" >&2; exit 2; }
            _compute_call POST /cli/compute/nodes/servers/revoke "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_now_b="$now" K2_CJ_wipe_b="$wipe")" tcp
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("revoked %s on %s: %s" % (v["server"], v["node"], v["note"]))'
            ;;
        owner)
            local server="" old="keep" caches="move" now=0 reason=""
            local usage="usage: k2 compute node servers <node> owner <server> [--old keep|revoke] [--caches move|wipe] [--now] [--reason TEXT]"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --old) old="${2:-}"; shift 2 ;;
                    --caches) caches="${2:-}"; shift 2 ;;
                    --now) now=1; shift ;;
                    --reason) reason="${2:-}"; shift 2 ;;
                    --json) shift ;;
                    -*) echo "$usage" >&2; exit 2 ;;
                    *) server="$1"; shift ;;
                esac
            done
            [ -n "$server" ] || { echo "$usage" >&2; exit 2; }
            case "$old" in keep|revoke) ;; *) echo "$usage (--old is keep or revoke)" >&2; exit 2 ;; esac
            case "$caches" in move|wipe) ;; *) echo "$usage (--caches is move or wipe)" >&2; exit 2 ;; esac
            _compute_call POST /cli/compute/nodes/servers/owner "$(_compute_json K2_CJ_node="$node" K2_CJ_server="$server" K2_CJ_old="$old" \
                K2_CJ_caches="$caches" K2_CJ_now_b="$now" K2_CJ_reason="$reason")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
if v.get("note"): print("note: " + v["note"], file=sys.stderr)'
            _compute_admin_held_or_approve
            ;;
        *)
            echo "usage: k2 compute node servers <node> [add|approve|deny|limits|pause|resume|revoke|owner …]" >&2
            exit 2
            ;;
    esac
}

# key=value… → a JSON object. Values parse as JSON when they can (numbers,
# true/false, [lists]); a,b,c for names/labels becomes a list; else string.
_compute_kv_json() {
    python3 -c '
import json, sys
out = {}
for a in sys.argv[1:]:
    k, _, v = a.partition("=")
    k = k.strip()
    if not k:
        print("usage: key=value", file=sys.stderr); sys.exit(2)
    try:
        val = json.loads(v)
    except Exception:
        val = v
    if k in ("names", "labels") and isinstance(val, str):
        val = [x for x in val.split(",") if x]
    out[k] = val
print(json.dumps(out))' "$@"
}

_compute_render_health() {
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
f = v.get("facts") or {}
def gb(b): return "%d GB" % (b // (1 << 30)) if isinstance(b, int) else "?"
seen = v.get("lastSeen")
print("%s  %s%s" % (v["node"], "online" if v["online"] else "OFFLINE", "" if v["online"] or not seen else " since " + time.strftime("%H:%M", time.localtime(seen))))
print("  k2-node %s, remote admin %s, pending approvals %s" % (v.get("nodeVersion") or "?", v.get("remoteAdmin") or "unknown", v.get("pendingApprovals", 0)))
if v.get("needsPin"):
    print("  NEEDS ONE-TIME PIN: admin commands are refused until someone runs, at that machine:")
    print("      %s" % (v.get("pinCommand") or "sudo k2-node admin pin"))
if not f:
    print("  no health report yet (needs a k2-node with health)")
else:
    if f.get("uptime_secs") is not None: print("  uptime        %dd %dh" % (f["uptime_secs"] // 86400, f["uptime_secs"] % 86400 // 3600))
    for d in f.get("disks") or []: print("  disk %-8s %s free of %s" % (d.get("path") or d.get("name"), gb(d["free_bytes"]), gb(d["total_bytes"])))
    if f.get("mem_total_bytes"): print("  memory        %s available of %s, pressure %s" % (gb(f.get("mem_available_bytes")), gb(f["mem_total_bytes"]), f.get("memory_pressure") or "?"))
    if f.get("disk_encryption"): print("  FileVault     %s%s" % (f["disk_encryption"], " (a power cut needs someone at the machine)" if f["disk_encryption"] == "on" else ""))
    ar = (f.get("sleep") or {}).get("autorestart")
    if ar is not None: print("  autorestart   %s" % ("on" if ar == 1 else "off"))
    up = f.get("updates") or {}
    if up.get("pending") is not None: print("  updates       %s pending (%s security)" % (up["pending"], up.get("security") or 0))
    if up.get("reboot_required"): print("  reboot        required")
    ex = f.get("executor") or {}
    if ex and not ex.get("reachable"): print("  executor      not answering")
    if ex.get("wall_restart_pending"): print("  wall restart  owed: a restart onto the job wall is owed; run wall.enable again (or approve the held restart): k2 compute node wall %s" % v["node"])
    for s in f.get("stuck_jobs") or []: print("  stuck job     %s (quiet %dm)" % (s["job_id"][:8], s.get("quiet_secs", 0) // 60))
    lo = f.get("leftovers")
    if isinstance(lo, dict):
        # 0.46.16: job-user processes no live job owns (reported, never killed).
        print("  leftovers     %s" % ("none (no job-user process outlives its job)" if not lo.get("count") else "%d process(es) of the job user with no live job (not killed by the check)" % lo["count"]))
        for p in lo.get("procs") or []: print("  leftover      pid %s, %s s old%s: %s" % (p.get("pid"), p.get("age_secs", "?"), (", from job %s" % p["job_id"][:8]) if p.get("job_id") else "", (p.get("cmd") or "")[:100]))
    sec = f.get("security")
    if sec:
        print("  job wall      %s%s" % (sec.get("wall"), (" (jobs run as %s)" % sec["job_user"]) if sec.get("wall") == "job_user" and sec.get("job_user") else ""))
        print("  network none  %s" % ("enforced" if sec.get("network_none") else "not enforced: network = none takes no jobs here"))
        for h in sec.get("open_homes") or []: print("  open home     %s" % h)
        for n in sec.get("notes") or []: print("  WARNING       %s" % n)
    else:
        print("  job wall      none reported (k2-node before 0.46.3: jobs run as the node user and can read its key)")
u = v.get("update")
if u: print("  update        %s %s%s" % (u.get("state"), u.get("version") or "", (" (" + u["reason"] + ")") if u.get("reason") else ""))
a = v.get("alertAfterSecs")
print("  alerts        %s, to %s" % (("offline after %dm" % (a // 60)) if a else "no offline alert", v.get("alertsTo") or "nobody (k2 compute node alerts --to <ws>)"))'
}

# A held request: the owner (outside K2) confirms right away; anyone else
# gets the request id and exit 2.
_compute_admin_held_or_approve() {
    local id
    id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin)["request"]["id"])')"
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v["request"]
print("held for the owner: %s\n  %s" % (r["id"], r["summary"]))' >&2
    if ! _compute_in_session && [ -t 0 ] && [ "$(_envn NO_APPROVE)" != "1" ]; then
        _compute_admin_approve "$id"
        exit 0
    fi
    echo "  the owner approves it in Settings → Cores → Compute → Machine settings → Waiting for you (or: k2 compute node admin approve $id, outside K2)" >&2
    # 0.46.9: with permission cards on, the daemon's Ask with: line (printed
    # last, at exit) brings this same request to the owner's chat as a card.
    if _permission_in_body "$_CP_BODY"; then
        echo "  to ask the owner in chat now, run the Ask with: line below: it puts this same request on a card." >&2
        echo "  Allow there sends it to the node, as Settings would (don't run your command again); your [permission:] turn says how it went" >&2
    fi
    exit 2
}

# Owner confirm (Touch ID / login or vault password) bound to the request,
# then approve. Never works for an agent: the daemon refuses passports, and
# the confirm needs a person.
_compute_admin_approve() {
    local id="$1"
    if _compute_in_session; then
        echo '{"error":"owner_only","hint":"approve in Settings → Cores → Compute → Machine settings → Waiting for you, or from a terminal outside K2. An agent can never approve."}' >&2
        exit 3
    fi
    _compute_call GET /cli/compute/admin/requests "state=all&limit=500" tcp
    local found full label
    found="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin)["requests"] if r["id"] == want or r["id"].startswith(want)]
if len(rows) != 1:
    sys.exit(1)
r = rows[0]
print(r["id"]); print(r["state"]); print(r["summary"][:64])
print("---"); print(r["summary"])
if r.get("reason"): print("reason: " + r["reason"])
for s in (r.get("body") or {}).get("steps") or [r.get("body") or {}]:
    if s.get("verb") == "script":
        print("--- script (%s) ---" % (s.get("args") or {}).get("interpreter", "/bin/sh"))
        print((s.get("args") or {}).get("text", ""))
'  "$id")" || { echo "{\"error\":\"request_not_found\",\"hint\":\"no single request matches '$id' (k2 compute node admin list)\"}" >&2; exit 3; }
    local state
    full="$(printf '%s\n' "$found" | sed -n 1p)"
    state="$(printf '%s\n' "$found" | sed -n 2p)"
    label="$(printf '%s\n' "$found" | sed -n 3p)"
    if [ "$state" != "pending" ]; then
        echo "{\"error\":\"not_pending\",\"hint\":\"$full is $state\"}" >&2
        exit 3
    fi
    printf '%s\n' "$found" | sed -n '5,$p' >&2
    local tok
    tok="$(_vault_confirm_token node-admin "$full" "$label")" || exit 3
    _compute_call POST /cli/compute/admin/approve "$(_compute_json K2_CJ_request="$full" K2_CJ_confirmToken="$tok")" tcp
    tok=""
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin); r=v["request"]
print("approved %s: %s\n  %s" % (r["id"], r["summary"], v.get("note","")))'
}

cmd_compute_node_admin() {
    local sub="${1:-}"
    case "$sub" in
        propose|list|ls|approve|deny|withdraw) shift ;;
        ""|help|--help|-h) _compute_usage; return 0 ;;
        *) sub="propose" ;;  # PRD shape: k2 compute node admin <node> <verb> …
    esac
    case "$sub" in
        propose)
            local node="" verb="" reason="" file="" interp="" timeout="" plan="" pairs=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    --file) file="${2:-}"; shift 2 ;;
                    --interpreter) interp="${2:-}"; shift 2 ;;
                    --timeout) timeout="$(_compute_secs "${2:-}")"; shift 2 ;;
                    --plan) plan="${2:-}"; shift 2 ;;
                    *=*) pairs+=("$1"); shift ;;
                    *)
                        if [ -z "$node" ]; then node="$1"
                        elif [ -z "$verb" ]; then verb="$1"
                        else echo "usage: k2 compute node admin propose <node> <verb> [key=value…]" >&2; exit 2
                        fi
                        shift ;;
                esac
            done
            [ -n "$node" ] && { [ -n "$verb" ] || [ -n "$plan" ]; } || { echo "usage: k2 compute node admin propose <node> <verb> [key=value…] --reason \"…\" (or --plan plan.json)" >&2; exit 2; }
            local body
            if [ -n "$plan" ]; then
                [ -f "$plan" ] || { echo "usage: --plan needs a JSON file: {\"steps\": [{\"verb\": …, \"args\": {…}}]}" >&2; exit 2; }
                body="$(python3 -c '
import json, sys
p = json.load(open(sys.argv[1]))
steps = p["steps"] if isinstance(p, dict) else p
print(json.dumps({"node": sys.argv[2], "kind": "plan", "steps": steps, "reason": sys.argv[3] or None}))' "$plan" "$node" "$reason")" || exit 2
            else
                local args_json
                args_json="$(_compute_kv_json ${pairs[@]+"${pairs[@]}"})" || exit 2
                if [ "$verb" = "script" ]; then
                    [ -n "$file" ] && [ -f "$file" ] || { echo "usage: k2 compute node admin propose <node> script --file fix.sh --reason …" >&2; exit 2; }
                    args_json="$(python3 -c '
import json, sys
a = json.loads(sys.argv[1]); a["text"] = open(sys.argv[2]).read()
if sys.argv[3]: a["interpreter"] = sys.argv[3]
if sys.argv[4]: a["timeout_secs"] = int(sys.argv[4])
print(json.dumps(a))' "$args_json" "$file" "$interp" "$timeout")" || exit 2
                fi
                local kind=command
                case "$verb" in svc.*) kind=service ;; esac
                body="$(_compute_json K2_CJ_node="$node" K2_CJ_kind="$kind" K2_CJ_verb="$verb" K2_CJ_args_j="$args_json" K2_CJ_reason="$reason")"
            fi
            _compute_call POST /cli/compute/admin/propose "$body"
            _compute_admin_held_or_approve
            ;;
        list|ls)
            local q="" as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --node) q="$q&node=$(urlencode "${2:-}")"; shift 2 ;;
                    --state) q="$q&state=$(urlencode "${2:-}")"; shift 2 ;;
                    --limit) q="$q&limit=$(urlencode "${2:-}")"; shift 2 ;;
                    --json) as_json=1; shift ;;
                    *) echo "usage: k2 compute node admin list [--node N] [--state open|pending|all] [--limit N] [--json]" >&2; exit 2 ;;
                esac
            done
            _compute_call GET /cli/compute/admin/requests "${q#&}"
            [ "$as_json" = "1" ] && { printf '%s\n' "$_CP_BODY"; exit 0; }
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys,time
v=json.load(sys.stdin)
rows=v["requests"]
if not rows: print("no admin requests")
for r in rows:
    print("%-36s %-9s %-10s %s" % (r["id"], r["state"], r.get("node") or "?", r["summary"]))
    if r.get("reason"): print("%36s reason: %s" % ("", r["reason"]))
    if r.get("denyReason"): print("%36s denied: %s" % ("", r["denyReason"]))'
            ;;
        approve)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node admin approve <id>" >&2; exit 2; }
            _compute_admin_approve "$1"
            ;;
        deny)
            local id="" why=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) why="${2:-}"; shift 2 ;;
                    *) id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "usage: k2 compute node admin deny <id> [--why …]" >&2; exit 2; }
            _compute_call POST /cli/compute/admin/deny "$(_compute_json K2_CJ_request="$id" K2_CJ_why="$why")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin)["request"]; print("denied %s: %s" % (r["id"], r["summary"]))'
            ;;
        withdraw)
            [ -n "${1:-}" ] || { echo "usage: k2 compute node admin withdraw <id>" >&2; exit 2; }
            _compute_call POST /cli/compute/admin/withdraw "$(_compute_json K2_CJ_request="$1")"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; r=json.load(sys.stdin)["request"]; print("withdrew %s" % r["id"])'
            ;;
    esac
}

