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
cmd_agent_isolation() {
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
