# Agent Isolation M1 (plan-agent-isolation-build-v1): `k2 agent isolation`
# (an agent's folder fence, owner-controlled) and `k2 agent whois` (which
# agent owns a process; owner only). The identity itself (each agent's
# terminals in their own cgroup) has no switch here: it is on wherever the
# box has it; `k2 app doctor` and /boot-status say whether it does.

cmd_help_agent_isolation() {
    cat <<'EOF'
%include cli/text/help/agent/isolation.txt
EOF
}

cmd_help_agent_whois() {
    cat <<'EOF'
%include cli/text/help/agent/whois.txt
EOF
}

_study_page_isolation() {
    cat <<'EOF'
%include cli/text/study/isolation.txt
EOF
}

# k2 agent isolation <workspace> fence|off [--json]
# k2 agent isolation layout on|off|status [--json]
cmd_agent_isolation() {
    if [ "${1:-}" = "layout" ]; then
        shift
        cmd_agent_isolation_layout "$@"
        return
    fi
    local ws="" mode="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) cmd_help_agent_isolation; return 0 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; cmd_help_agent_isolation >&2; exit 2 ;;
            *) if [ -z "$ws" ]; then ws="$1"; elif [ -z "$mode" ]; then mode="$1"; else cmd_help_agent_isolation >&2; exit 2; fi; shift ;;
        esac
    done
    if [ -z "$ws" ] || [ -z "$mode" ]; then
        cmd_help_agent_isolation >&2
        exit 2
    fi
    local on
    case "$mode" in
        fence|on) on=1 ;;
        off) on=0 ;;
        *) echo "Error: the mode is fence or off (got '$mode')" >&2; exit 2 ;;
    esac
    _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle=fence K2_CJ_value_b="$on")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
state = "on" if v.get("fence") else "off"
print("Folder fence is %s for %s: %s." % (state, v.get("workspace"), v.get("appliesTo", "its next terminals")))'
}

# k2 agent isolation layout on|off|status [--json] (Agent Isolation M2 B4):
# a root-owned scope for every agent session. on is the owner's, with this
# computer's owner check: on a Mac or Windows PC the daemon asks at once;
# otherwise it answers vault_presence_required with the check to get, which
# is asked here (a terminal outside K2) and `on` is sent again with its
# token. off is the owner's; status the owner's or an admin's.
cmd_agent_isolation_layout() {
    local mode="" json_out=0 raw sid tok
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) cmd_help_agent_isolation; return 0 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; cmd_help_agent_isolation >&2; exit 2 ;;
            *) if [ -z "$mode" ]; then mode="$1"; else cmd_help_agent_isolation >&2; exit 2; fi; shift ;;
        esac
    done
    case "$mode" in
        on|off|status) ;;
        '') cmd_help_agent_isolation >&2; exit 2 ;;
        *) echo "Error: the layout is on, off or status (got '$mode')" >&2; exit 2 ;;
    esac
    raw="$(_compute_send POST /cli/agent-isolation/layout "$(_compute_json K2_CJ_mode="$mode")")"
    _compute_split "$raw"
    if [ "$mode" = on ] && [ "$_CP_STATUS" != "200" ]; then
        sid="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read() or "{}")
except Exception:
    d = {}
e = d.get("error") if isinstance(d, dict) else None
c = d.get("confirm") if isinstance(d, dict) else None
ok = isinstance(e, dict) and e.get("code") == "vault_presence_required" and isinstance(c, dict) and c.get("action") == "isolation-layout"
print((c.get("secretId") or "") if ok else "")')"
        if [ -n "$sid" ] && ! _envn_any HOOK_SOCK HOOK_TOKEN && [ -S "$(_vault_sock)" ]; then
            tok="$(_vault_confirm_token isolation-layout "$sid" "")" || exit 3
            raw="$(_compute_send POST /cli/agent-isolation/layout "$(_compute_json K2_CJ_mode=on K2_CJ_confirmToken="$tok")")"
            tok=""
            _compute_split "$raw"
        fi
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_AIL_MODE="$mode" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
mode = os.environ["K2_AIL_MODE"]
if mode == "status":
    print("Agent scopes: %s (the owner'"'"'s setting: %s)" % ("on" if v.get("placing") else "off", v.get("setting")))
    if v.get("whyOff"):
        print("  why off: %s" % v["whyOff"])
    l = v.get("layout")
    if isinstance(l, dict):
        scopes = l.get("scopes") or []
        fenced = sum(1 for s in scopes if s.get("kind") == "fenced")
        print("  slices ready: %s; P1: %s; kernel.pid_max: %s" % ("yes" if l.get("ready") else "no", l.get("p1"), l.get("pidMax")))
        print("  live scopes: %d (%d fenced)" % (len(scopes), fenced))
        if l.get("survivors"):
            print("  agents in the old layout: %d" % len(l["survivors"]))
    elif v.get("layoutError"):
        print("  layout: %s" % v["layoutError"])
    for o in v.get("oldLayout") or []:
        print("  old layout: %s (%s)" % (o.get("agent"), o.get("why")))
elif mode == "on":
    if v.get("changed"):
        print("Agent scopes are on: %s." % v.get("appliesTo"))
    else:
        print("Agent scopes were already on.")
else:
    print("Agent scopes are off: %s." % v.get("appliesTo"))
    if v.get("message"):
        print("  %s." % v["message"].rstrip("."))'
}

# k2 agent whois <pid> [--json]
cmd_agent_whois() {
    local pid="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) cmd_help_agent_whois; return 0 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; cmd_help_agent_whois >&2; exit 2 ;;
            *) if [ -z "$pid" ]; then pid="$1"; else cmd_help_agent_whois >&2; exit 2; fi; shift ;;
        esac
    done
    case "$pid" in
        ''|*[!0-9]*) cmd_help_agent_whois >&2; exit 2 ;;
    esac
    _compute_call GET /cli/agent-whois "pid=$pid"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("pid %s: %s" % (v.get("pid"), v.get("who")))
if v.get("workspace"):
    print("  workspace: %s (folder fence %s)" % (v["workspace"], "on" if v.get("fence") else "off"))
print("  cgroup: %s" % (v.get("cgroup") or "unknown"))
print("  identity on this box: %s" % v.get("identity"))'
}
