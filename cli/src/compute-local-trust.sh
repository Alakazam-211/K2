cmd_compute_local() {
    local sub="${1:-status}"
    shift 2>/dev/null || true
    local body
    case "$sub" in
        status)
            _compute_call GET /cli/compute/local/status ""
            if [ "${1:-}" = "--json" ]; then printf '%s\n' "$_CP_BODY"; exit 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v.get("binary") or {}
hc = v.get("homeCheck") or {}
if not v.get("installed"):
    print("This computer is not a compute node.")
    print("  k2-node: %s" % (("%s (%s, %s)" % (b.get("path"), b.get("source"), b.get("version") or "?")) if b else "not on this machine (Linux: downloaded and signature-checked on enable)"))
    print("  root: %s" % {"helper": "the root helper installs it (no password)", "prompt": "an admin prompt or one sudo command", "command": "one sudo command (or passwordless sudo, tried on enable)"}.get(v.get("elevation"), "one sudo command"))
    if hc:
        print("  home: %s is %s (mode %s)" % (hc.get("home"), "private" if hc.get("private") else "readable by other users", hc.get("mode")))
    print("Enable: k2 compute local enable [--controller URL --code-stdin --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]")
    sys.exit(0)
n = v.get("node") or {}
c = v.get("control") or {}
print("node %s: %s%s" % (n.get("name", "?"), n.get("state", "unknown"), (" via " + n["route"]) if n.get("route") else ""))
print("control: %s%s" % (c.get("state", "active"), (" by %s" % c["by"]) if c.get("by") else ""))
if n.get("running"): print("running: " + " ".join(n["running"]))
if n.get("last_error"): print("last error: " + str(n["last_error"]))
if n.get("sas"): print("waiting for confirmation, code " + str(n["sas"]))
r = v.get("enrollResult") or {}
if r and not r.get("ok"): print("last enroll request failed: " + str(r.get("message")))
p = v.get("packages") or {}
if p.get("approved"): print("approved packages: " + " ".join(p["approved"]))
if hc and not hc.get("private"): print("note: %s is readable by other users (mode %s); jobs can read files there that are readable by everyone" % (hc.get("home"), hc.get("mode")))
ra = v.get("remoteAdmin")
print("remote admin: %s" % (ra or "not set up"))
if v.get("remoteAdminCommand"): print("  K2 can\x27t update this node. Turn it on: k2 compute local remote-admin policy (or: %s)" % v["remoteAdminCommand"])
'
            ;;
        pause)
            local now=0
            [ "${1:-}" = "--now" ] && now=1
            body="$(_compute_json K2_CJ_now_b="$now")"
            _compute_call POST /cli/compute/local/pause "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["control"], v["note"]))'
            ;;
        drain|resume)
            _compute_call POST "/cli/compute/local/$sub" "{}" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s" % (v["control"], v["note"]))'
            ;;
        policy)
            if [ $# -eq 0 ]; then
                _compute_call GET /cli/compute/local/status ""
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("policy"), indent=2))'
                exit 0
            fi
            body="$(python3 -c '
import json, sys
s = {}
for a in sys.argv[1:]:
    if "=" not in a:
        sys.stderr.write("usage: k2 compute local policy key=value...\n"); sys.exit(2)
    k, v = a.split("=", 1); s[k.replace("-", "_")] = v
print(json.dumps({"set": s}))' "$@")" || exit 2
            _compute_call POST /cli/compute/local/policy "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["policy"], indent=2))'
            ;;
        enable)
            local controller="" enroll="" enroll_flag="" code_stdin=0 ticket="" ticket_flag="" ticket_stdin=0 name="" private="" remote_admin=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --controller) controller="${2:-}"; shift 2 ;;
                    --code-stdin) code_stdin=1; shift ;;
                    --enroll|--code) enroll="${2:-}"; enroll_flag="$1"; shift 2 ;;
                    --ticket-stdin) ticket_stdin=1; shift ;;
                    --ticket) ticket="${2:-}"; ticket_flag="$1"; shift 2 ;;
                    --name) name="${2:-}"; shift 2 ;;
                    --private-home) private=1; shift ;;
                    --no-private-home) private=0; shift ;;
                    --remote-admin)
                        remote_admin="${2:-}"; shift 2
                        case "$remote_admin" in off|policy|full) ;; *) echo "--remote-admin is off, policy or full" >&2; exit 2 ;; esac
                        ;;
                    *) echo "usage: k2 compute local enable [--controller URL --code-stdin --name NAME] [--private-home|--no-private-home] [--remote-admin off|policy|full]" >&2; exit 2 ;;
                esac
            done
            # The enroll code and the ticket off argv: --code-stdin and
            # --ticket-stdin read one line from stdin. --enroll/--code
            # CODE.FP and --ticket T still work for now, with a note: a
            # command line is visible to every user on this computer while
            # the command runs.
            [ "$code_stdin" = 0 ] || [ "$ticket_stdin" = 0 ] || { echo "k2 compute local enable: --code-stdin or --ticket-stdin, not both (stdin holds one)" >&2; exit 2; }
            if [ "$ticket_stdin" = 1 ]; then
                [ -z "$ticket" ] || { echo "k2 compute local enable: --ticket-stdin or --ticket, not both" >&2; exit 2; }
                _cli_read_secret_stdin "Ticket: "
                ticket="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
                [ -n "$ticket" ] || { echo "k2 compute local enable: --ticket-stdin: no ticket on stdin" >&2; exit 2; }
            elif [ -n "$ticket_flag" ]; then
                echo "note: --ticket is visible to every user on this computer while the command runs; use --ticket-stdin" >&2
            fi
            if [ "$code_stdin" = 1 ]; then
                [ -z "$enroll" ] || { echo "k2 compute local enable: --code-stdin or $enroll_flag, not both" >&2; exit 2; }
                _cli_read_secret_stdin "Enroll code: "
                enroll="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
                [ -n "$enroll" ] || { echo "k2 compute local enable: --code-stdin: no code on stdin" >&2; exit 2; }
            elif [ -n "$enroll_flag" ]; then
                echo "note: $enroll_flag is visible to every user on this computer while the command runs; use --code-stdin" >&2
            fi
            body="$(_compute_json K2_CJ_controller="$controller" K2_CJ_enroll_s="$enroll" K2_CJ_ticket_s="$ticket" K2_CJ_name="$name" K2_CJ_privateHome_b="$private" K2_CJ_remoteAdmin="$remote_admin")"
            _compute_call POST /cli/compute/local/enable "$body" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
if v.get("sas"):
    print("Code: %s — confirm it on the controller: k2 compute node confirm <name> <code>" % v["sas"])
'
            ;;
        remote-admin)
            case "${1:-}" in
                off|policy|full) ;;
                *) echo "usage: k2 compute local remote-admin off|policy|full" >&2; exit 2 ;;
            esac
            _compute_call POST /cli/compute/local/remote-admin "$(_compute_json K2_CJ_level="$1")" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
'
            ;;
        disable)
            # Q2 (Rosson 2026-10-24): an owner stop stops the jobs running
            # here; name them first, and ask (or need --yes) when any run.
            local yes=0
            [ "${1:-}" = "--yes" ] && yes=1
            _compute_call GET /cli/compute/local/status ""
            local running
            running="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(" ".join((v.get("node") or {}).get("running") or []))')"
            if [ -n "$running" ] && [ "$yes" = "0" ]; then
                echo "These jobs are running here and stop now: $running" >&2
                if [ -t 0 ]; then
                    printf "Stop being a compute node? [y/N] " >&2
                    local answer=""
                    read -r answer || answer=""
                    case "$answer" in y|Y|yes) ;; *) echo "not stopped" >&2; exit 2 ;; esac
                else
                    echo "pass --yes to stop them (k2 compute local disable --yes)" >&2
                    exit 2
                fi
            fi
            _compute_call POST /cli/compute/local/disable "{}" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("message", ""))
if v.get("command"):
    print("  " + v["command"])
'
            ;;
        *)
            echo "usage: k2 compute local status|pause [--now]|drain|resume|policy|enable|remote-admin|disable" >&2
            exit 2
            ;;
    esac
}

# --gpu VALUE (0.46.11): any, amd or intel. NVIDIA isn't supported yet.
_compute_gpu_arg() {
    case "${1:-}" in
        any|amd|intel) return 0 ;;
        nvidia) echo "usage: --gpu nvidia: NVIDIA GPU jobs aren't supported yet; use --gpu any, amd or intel" >&2; return 2 ;;
        *) echo "usage: --gpu [any|amd|intel] (one GPU of the node, to this job alone; bare --gpu = any)" >&2; return 2 ;;
    esac
}

# --distro VALUE (0.46.9): ubuntu, ubuntu@24.04, debian, arch, fedora…
# (the node's ID or ID_LIKE; the version matches on a dot prefix).
_compute_distro_arg() {
    if [[ "${1:-}" =~ ^[a-z0-9][a-z0-9._-]{0,39}(@[0-9A-Za-z][0-9A-Za-z._-]{0,19})?$ ]]; then
        printf '%s' "$1"
    else
        echo "usage: --distro ID[@VERSION] (ubuntu, ubuntu@24.04, debian, arch, fedora: the ID or ID_LIKE in the node's /etc/os-release; k2 compute nodes shows each node's)" >&2
        return 2
    fi
}

# Pretty JSON of the last answer (the trust verbs print what the daemon said).
_compute_trust_print() {
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), indent=2))'
}

# A person's confirm for a trust request (bound to its id), then approve.
_compute_trust_approve() {
    local id="${1:-}"
    [ -n "$id" ] || { echo "usage: k2 compute trust approve <ctr_…>" >&2; exit 2; }
    if _compute_in_session; then
        echo '{"error":"person_only","hint":"a person approves this, from a terminal outside K2 (Touch ID / vault password). An agent never approves."}' >&2
        exit 3
    fi
    _compute_call GET /cli/compute/trust/requests "state=all" tcp
    local found full state label digest
    found="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin)["requests"] if r["id"] == want or r["id"].startswith(want)]
if len(rows) != 1:
    sys.exit(1)
r = rows[0]
print(r["id"]); print(r["state"]); print(r["summary"][:64]); print(r["digest"]); print("---"); print(r["summary"])
if r.get("reason"): print("reason: " + r["reason"])
' "$id")" || { echo "{\"error\":\"request_not_found\",\"hint\":\"no single request matches '$id' (k2 compute trust)\"}" >&2; exit 3; }
    full="$(printf '%s\n' "$found" | sed -n 1p)"
    state="$(printf '%s\n' "$found" | sed -n 2p)"
    label="$(printf '%s\n' "$found" | sed -n 3p)"
    # The digest of what is printed here: the daemon refuses the approval
    # when the request changed since (request_changed). The Touch ID
    # sheet's words are the daemon's own.
    digest="$(printf '%s\n' "$found" | sed -n 4p)"
    if [ "$state" != "pending" ]; then
        echo "{\"error\":\"not_pending\",\"hint\":\"$full is $state\"}" >&2
        exit 3
    fi
    printf '%s\n' "$found" | sed -n '6,$p' >&2
    local tok
    tok="$(_vault_confirm_token compute-trust "$full" "$label")" || exit 3
    K2_CP_MAX_TIME=300 _compute_call POST /cli/compute/trust/approve "$(_compute_json K2_CJ_request="$full" K2_CJ_expectedDigest="$digest" K2_CJ_confirmToken="$tok")" tcp
    tok=""
    _compute_trust_print
    # A seat that asked for secrets: the seat's approval didn't give them.
    # The consent is its own yes, with its own words and confirm.
    local next
    next="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(((json.load(sys.stdin).get("result") or {}).get("consentNext")) or "")')"
    if [ -n "$next" ]; then
        echo "The seat asked to send vault secrets to that owner's nodes. That is a separate yes: $next" >&2
    fi
}

# k2 compute roster|lanes|seats|trust|owners (0.46.10 C1/C3a).
cmd_compute_trust() {
    local noun="$1"; shift
    local as_json=0 a
    for a in "$@"; do [ "$a" = "--json" ] && as_json=1; done
    case "$noun" in
        roster)
            local sub="${1:-show}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                show|--json)
                    _compute_call GET /cli/compute/roster ""
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
l = v.get("latest")
if l:
    print("node roster seq %s (key %s…), issued %s%s" % (l["seq"], l["keyFingerprint"][:16], l["issuedAt"], ", STALE: k2 compute roster publish --resign" if v.get("stale") else ""))
    for e in l["entries"]:
        print("  %-16s %-8s %-15s host %s  key %s…" % (e["name"], e["status"], e.get("taintBasis") or "-", e["host"], e["nodeFp"][:12]))
else:
    print("no node roster yet: k2 compute roster set <node> …, then a person approves it")
for n in v.get("nodes", []):
    st = n.get("statement") or {}
    print("node %-16s statement %s, taint %s, on roster: %s" % (n["name"], "yes" if st else "none (k2-node older than protocol 9)", st.get("taint", "-"), n.get("roster") or "no"))
for r in v.get("pending", []):
    print("waiting: %s  %s" % (r["id"], r["summary"]))
'
                    ;;
                key)
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"the roster key is minted and shown to a person: run it from a terminal outside K2"}' >&2
                        exit 3
                    fi
                    _compute_call POST /cli/compute/roster/key "{}" tcp
                    _compute_trust_print
                    ;;
                set|attest)
                    local node="${1:-}"; [ -n "$node" ] && shift
                    [ -n "$node" ] || { echo "usage: k2 compute roster set <node> [--host H] [--provider P] [--site S] [--admin-reach a,b] [--attest TEXT]" >&2; exit 2; }
                    local host="" prov="" site="" reach="" att="" why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --host) host="${2:-}"; shift 2 ;;
                            --provider) prov="${2:-}"; shift 2 ;;
                            --site) site="${2:-}"; shift 2 ;;
                            --admin-reach) reach="${2:-}"; shift 2 ;;
                            --attest) att="${2:-}"; shift 2 ;;
                            --why|--reason) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute roster set <node> [--host H] [--provider P] [--site S] [--admin-reach a,b] [--attest TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/roster/propose "$(_compute_json K2_CJ_op=set K2_CJ_node="$node" K2_CJ_host="$host" K2_CJ_provider="$prov" K2_CJ_site="$site" K2_CJ_adminReach="$reach" K2_CJ_attest="$att" K2_CJ_reason="$why")"
                    _compute_trust_print
                    ;;
                retire|revoke)
                    local node="${1:-}"; [ -n "$node" ] && shift
                    [ -n "$node" ] || { echo "usage: k2 compute roster $sub <node>" >&2; exit 2; }
                    local reason=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --reason) reason="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute roster $sub <node> [--reason compromised|key_lost|other]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/roster/propose "$(_compute_json K2_CJ_op="$sub" K2_CJ_node="$node" K2_CJ_reason="$reason")"
                    _compute_trust_print
                    ;;
                publish)
                    local resign=""
                    for a in "$@"; do [ "$a" = "--resign" ] && resign=1; done
                    K2_CP_MAX_TIME=300 _compute_call POST /cli/compute/roster/publish "$(_compute_json K2_CJ_resign_b="$resign")"
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute roster [show|key|set|retire|revoke|publish] (k2 compute --help)" >&2; exit 2 ;;
            esac
            ;;
        lanes)
            if [ "${1:-}" = "set" ]; then
                shift
                local name="${1:-}"; [ -n "$name" ] && shift
                [ -n "$name" ] || { echo "usage: k2 compute lanes set <name> [--from ci-standard] [key=value …]" >&2; exit 2; }
                local from="" sets="{}" why=""
                while [ $# -gt 0 ]; do
                    case "$1" in
                        --from) from="${2:-}"; shift 2 ;;
                        --why|--reason) why="${2:-}"; shift 2 ;;
                        --json) shift ;;
                        *=*)
                            sets="$(printf '%s' "$sets" | K2_LS_KV="$1" python3 -c '
import json, os, sys
d = json.load(sys.stdin)
k, v = os.environ["K2_LS_KV"].split("=", 1)
try:
    d[k] = json.loads(v)
except Exception:
    d[k] = v.split(",") if k in ("isolation", "profiles") else v
print(json.dumps(d))')"
                            shift ;;
                        *) echo "usage: k2 compute lanes set <name> [--from ci-standard] [key=value …]" >&2; exit 2 ;;
                    esac
                done
                _compute_call POST /cli/compute/lanes/set "$(_compute_json K2_CJ_name="$name" K2_CJ_from="$from" K2_CJ_set_j="$sets" K2_CJ_reason="$why")"
                _compute_trust_print
                return 0
            fi
            _compute_call GET /cli/compute/lanes ""
            if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v["lanes"]:
    print("no approved lanes: k2 compute lanes set ci-standard --from ci-standard")
for l in v["lanes"]:
    print("%s: %s (approved by %s)" % (l["name"], l["summary"], l["approvedBy"]))
'
            ;;
        seats)
            local sub="${1:-list}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                list|--json)
                    _compute_call GET /cli/compute/seats ""
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for s in v["offered"]:
    print("offered %-24s to %-24s lane %-12s %-9s nodes %s" % (s["id"], s["clientLabel"], s["lane"], s["state"], ",".join(s["nodes"])))
for s in v["held"]:
    print("held    %-24s on %-24s lane %-12s %-9s nodes %s%s" % (s["id"], s["ownerId"], s["lane"], s["state"], ",".join(s["nodes"]) or "-", ("  (" + s["note"] + ")") if s.get("note") else ""))
if not v["offered"] and not v["held"]:
    print("no seats")
'
                    ;;
                request)
                    local owner="${1:-}"; [ -n "$owner" ] && shift
                    [ -n "$owner" ] || { echo "usage: k2 compute seats request <owner-host> [--lane L] [--seat ID] [--secrets] [--why TEXT]" >&2; exit 2; }
                    # Secrets are off unless asked for (--secrets); even then
                    # the seat's approval never gives them: a person agrees
                    # on their own confirm (k2 compute owners consent … on).
                    local lane="" seat="" secrets="0" why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --lane) lane="${2:-}"; shift 2 ;;
                            --seat) seat="${2:-}"; shift 2 ;;
                            --secrets) secrets="1"; shift ;;
                            --no-secrets) secrets="0"; shift ;;
                            --why) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats request <owner-host> [--lane L] [--seat ID] [--secrets] [--why TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    _compute_call POST /cli/compute/seats/request "$(_compute_json K2_CJ_owner="$owner" K2_CJ_lane="$lane" K2_CJ_seat="$seat" K2_CJ_secrets_b="$secrets" K2_CJ_why="$why")"
                    _compute_trust_print
                    ;;
                offer)
                    local req="${1:-}"; [ -n "$req" ] && shift
                    local nodes=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --nodes) nodes="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats offer <request> --nodes a,b,c" >&2; exit 2 ;;
                        esac
                    done
                    { [ -n "$req" ] && [ -n "$nodes" ]; } || { echo "usage: k2 compute seats offer <request> --nodes a,b,c" >&2; exit 2; }
                    _compute_call POST /cli/compute/seats/offer "$(_compute_json K2_CJ_request="$req" K2_CJ_nodes="$nodes")"
                    _compute_trust_print
                    ;;
                end|suspend|resume)
                    # (owner) The owner, in person: the seat's statement is
                    # signed again and its nodes' plans use the owner key.
                    local seat="${1:-}"; [ -n "$seat" ] && shift
                    local why=""
                    while [ $# -gt 0 ]; do
                        case "$1" in
                            --why) why="${2:-}"; shift 2 ;;
                            --json) shift ;;
                            *) echo "usage: k2 compute seats $sub <seat> [--why TEXT]" >&2; exit 2 ;;
                        esac
                    done
                    [ -n "$seat" ] || { echo "usage: k2 compute seats $sub <seat> [--why TEXT]" >&2; exit 2; }
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"the node owner ends, suspends or resumes a seat in person, from a terminal outside K2 (Touch ID / vault password)"}' >&2
                        exit 3
                    fi
                    local tok
                    tok="$(_vault_confirm_token compute-trust "seat-$sub:$seat" "")" || exit 3
                    K2_CP_MAX_TIME=300 _compute_call POST "/cli/compute/seats/$sub" "$(_compute_json K2_CJ_seat="$seat" K2_CJ_why="$why" K2_CJ_confirmToken="$tok")" tcp
                    tok=""
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute seats [list|request|offer|end|suspend|resume] (k2 compute --help)" >&2; exit 2 ;;
            esac
            ;;
        trust)
            local sub="${1:-requests}"
            [ $# -gt 0 ] && shift
            case "$sub" in
                requests|list|ls|--json|--all)
                    local st="open"
                    for a in "$sub" "$@"; do [ "$a" = "--all" ] && st="all"; done
                    _compute_call GET /cli/compute/trust/requests "state=$st"
                    if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
                    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v["requests"]:
    print("%s  %-9s %-12s %s" % (r["id"], r["state"], r["kind"], r["summary"]))
if not v["requests"]:
    print("nothing waits for a person")
'
                    ;;
                approve) _compute_trust_approve "${1:-}" ;;
                deny)
                    local id="${1:-}"; [ -n "$id" ] && shift
                    local why=""
                    [ "${1:-}" = "--why" ] && why="${2:-}"
                    [ -n "$id" ] || { echo "usage: k2 compute trust deny <id> [--why TEXT]" >&2; exit 2; }
                    _compute_call POST /cli/compute/trust/deny "$(_compute_json K2_CJ_request="$id" K2_CJ_why="$why")"
                    _compute_trust_print
                    ;;
                withdraw)
                    [ -n "${1:-}" ] || { echo "usage: k2 compute trust withdraw <id>" >&2; exit 2; }
                    _compute_call POST /cli/compute/trust/withdraw "$(_compute_json K2_CJ_request="$1")"
                    _compute_trust_print
                    ;;
                *) echo "usage: k2 compute trust [requests|approve|deny|withdraw]" >&2; exit 2 ;;
            esac
            ;;
        owners)
            if [ "${1:-}" = "consent" ]; then
                local owner="${2:-}" onoff="${3:-}"
                case "$onoff" in on|off) ;; *) echo "usage: k2 compute owners consent <owner> on|off" >&2; exit 2 ;; esac
                if [ "$onoff" = "on" ]; then
                    if _compute_in_session; then
                        echo '{"error":"person_only","hint":"sending vault secrets to another owner'"'"'s nodes is a person'"'"'s choice: a terminal outside K2"}' >&2
                        exit 3
                    fi
                    _compute_call GET /cli/compute/owners "" tcp
                    local kfp
                    kfp="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
o=sys.argv[1]
r=[t for t in json.load(sys.stdin)["trusted"] if t["ownerId"]==o or t["keyFp"]==o]
print(r[0]["keyFp"] if len(r)==1 else "")' "$owner")"
                    [ -n "$kfp" ] || { echo "{\"error\":\"node_owner_not_trusted\",\"hint\":\"no pinned node owner $owner (k2 compute owners)\"}" >&2; exit 3; }
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=1 K2_CJ_preview_j=true)" tcp
                    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys
v=json.load(sys.stdin)
print(v["text"])
print("  %s'"'"'s node-roster key: %s" % (v["owner"], v["keyFp"]))' >&2
                    local tok
                    tok="$(_vault_confirm_token compute-trust "consent:$kfp" "Send vault secrets to $owner's nodes")" || exit 3
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=1 K2_CJ_confirmToken="$tok")" tcp
                else
                    _compute_call POST /cli/compute/owners/consent "$(_compute_json K2_CJ_owner="$owner" K2_CJ_on_b=0)"
                fi
                _compute_trust_print
                return 0
            fi
            _compute_call GET /cli/compute/owners ""
            if [ "$as_json" = "1" ]; then _compute_trust_print; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("self")
if s:
    print("this server (%s): node-roster key %s" % (s["ownerId"], s["key"]["fingerprint"]))
for t in v["trusted"]:
    l = t.get("latest") or {}
    print("%s: key %s, roster seq %s, secrets %s" % (t["ownerId"], t["keyFp"], l.get("seq", "-"), "on" if t["secretsConsent"] else "off"))
if not s and not v["trusted"]:
    print("no node owners pinned, and this server has no node-roster key yet")
'
            ;;
    esac
}

