# ── Agent hire: teams, instances, hire root, sender check (0.45.6) ────
# prd-agent-hire-templates-v1 "build A": the v1 API hires from an inline
# recipe (`POST /v1/agents/hire`, an API key with the hire scope). These
# verbs are the people's side on this box:
#   k2 agent team [--workspace <ws>] [--json]      who serves me / my team / who I serve
#   k2 agent team served [--workspace <ws>]        the workspaces this team serves (+ scope)
#   k2 agent team link <team> <workspace> [--kind serves|helper_of] [--scope JSON] [--mesh]
#   k2 agent team unlink <link-id>
#   k2 agent instances [--key <api-key-id>] [--json]
#   k2 agent instances show <instance>
#   k2 agent instances hire <recipe.json|-> [--key <idempotency-key>] [--dry-run] [--json]
#   k2 agent instances retire <instance> [--force]
#   k2 agent hire-root [set <dir>]
#   k2 msg who <m_…>                               who really sent a delivery
# Inside a K2 terminal `team` and `instances` carry the session passport:
# an agent reads its own links; it never hires teams, links or retires.

# Print a hire-route JSON answer. $1 = view, $2 = "true" for raw JSON.
# Exit codes: owner_only/forbidden/exceeds_key/vault_* → 3, not_found → 4,
# usage → 2, anything else → 1.
_hire_print() {
    HIRE_VIEW="$1" HIRE_JSON="${2:-false}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon answered: %s" % raw[:300], file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err:
    code = err.get("code", "") if isinstance(err, dict) else ""
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    print("Error: %s%s" % ((code + ": ") if code else "", hint), file=sys.stderr)
    if code in ("owner_only", "forbidden", "exceeds_key", "role_required") or "token" in hint.lower():
        sys.exit(3)
    sys.exit({"not_found": 4, "usage": 2}.get(code, 1))
if os.environ.get("HIRE_JSON") == "true":
    print(json.dumps(d, indent=2)); sys.exit(0)
view = os.environ.get("HIRE_VIEW", "")
def link_line(l, side):
    other = l.get("teamLeadHandle") if side == "out" else l.get("consumerHandle")
    team = l.get("teamName") or l.get("teamLeadHandle") or "?"
    scope = json.dumps(l["scope"]) if l.get("scope") is not None else "-"
    if side == "out":
        return "  %s  %s %s (lead %s)  scope %s" % (l["id"], "served by" if l["kind"] == "serves" else "helped by", team, other or "?", scope)
    return "  %s  %s %s  scope %s" % (l["id"], "serves" if l["kind"] == "serves" else "helps", other or "?", scope)
if view in ("team", "served"):
    ws = d.get("workspace") or {}
    print("Workspace: %s" % (ws.get("handle") or ws.get("id")))
    t = d.get("team")
    if view == "team":
        if t:
            print("Team: %s (%s)" % (t.get("name") or "(single hire)", t.get("instance")))
            for m in t.get("members") or []:
                print("  %s%s  %s" % (m["handle"], " (lead)" if m.get("lead") else "", m.get("role", "")))
        out = d.get("servedBy") or []
        print("Served by:" if out else "Served by: nobody")
        for l in out: print(link_line(l, "out"))
    inn = d.get("serves") or []
    print("Serves:" if inn else "Serves: nobody")
    for l in inn: print(link_line(l, "in"))
elif view == "instances":
    rows = d.get("instances") or []
    if not rows:
        print("No hires yet."); sys.exit(0)
    for i in rows:
        members = ",".join(m["handle"] for m in i.get("members") or [])
        who = i.get("apiKeyId") and ("key " + i["apiKeyId"][:8]) or i.get("actor")
        print("%s  [%s]  %s  %s  by %s%s" % (i["id"], i["status"], i.get("teamName") or "-", members or "-", who,
              ("  for %s (app-asserted)" % i["onBehalfOf"]) if i.get("onBehalfOf") else ""))
elif view == "instance":
    i = d.get("instance") or {}
    print("%s  [%s]  %s" % (i.get("id"), i.get("status"), i.get("teamName") or "(single hire)"))
    if i.get("reason"): print("  reason: %s" % i["reason"])
    for m in i.get("members") or []:
        print("  %s%s  %s  [%s]  %s" % (m["handle"], " (lead)" if m.get("lead") else "", m.get("role"), m.get("status"), m.get("path")))
    for l in d.get("links") or []:
        print("  link %s  %s → %s" % (l["id"], l.get("teamLeadHandle"), l.get("consumerHandle")))
    for e in d.get("events") or []:
        print("  event %s %s %s%s" % (e["at"], e["action"], e["result"], (" — " + e["reason"]) if e.get("reason") else ""))
elif view == "hire":
    if d.get("dryRun"):
        p = d.get("plan") or {}
        print("Plan (nothing written):")
        for m in p.get("members") or []:
            print("  %s%s  %s" % (m["handle"], " (lead)" if m.get("lead") else "", m["path"]))
        print(p.get("note", "")); sys.exit(0)
    print("%s %s" % ("Hired" if d.get("changed") else "Already hired (same key):", d.get("instance")))
    for m in d.get("members") or []:
        print("  %s%s  %s" % (m["handle"], " (lead)" if m.get("lead") else "", m["path"]))
    for l in d.get("links") or []:
        print("  linked %s" % l["id"])
elif view == "retire":
    print("%s %s" % (d.get("instance"), d.get("status")))
    for m in d.get("members") or []:
        print("  %s %s%s" % (m["handle"], m["status"], (" — " + m["reason"]) if m.get("reason") else ""))
    if d.get("hint"): print(d["hint"])
    if d.get("status") == "retire_blocked": sys.exit(3)
elif view == "link":
    l = d.get("link") or {}
    print("%s %s" % ("Linked" if d.get("changed") else "Already linked:", l.get("id")))
elif view == "unlink":
    print("Unlinked %s" % d.get("link") if d.get("changed") else "Link %s was already removed" % d.get("link"))
elif view == "root":
    print("%s%s" % (d.get("root"), "  (default)" if d.get("default") else ""))
elif view == "who":
    s = d.get("sender") or {}
    print("Delivery %s" % d.get("id"))
    print("  via:     %s%s" % (d.get("via"), "  (the sender agent'"'"'s own session)" if d.get("via") == "passport" else ""))
    print("  sender:  %s" % (s.get("handle") or s.get("label") or "-"))
    print("  actor:   %s" % d.get("actor"))
    if d.get("onBehalfOf"): print("  for:     %s (app-asserted, not verified)" % d["onBehalfOf"]["value"])
    print("  linked:  %s" % ("yes " + ", ".join("%s %s" % (l["id"], json.dumps(l.get("scope"))) for l in d.get("links") or []) if d.get("linked") else "no"))
else:
    print(json.dumps(d, indent=2))
'
}

cmd_agent_team() {
    local sub="${1:-}"
    case "$sub" in
        --help|-h|help)
            sed -n '/^# ── Agent hire: teams, instances/,/^# Inside a K2 terminal/p' "$0" | sed 's/^# \{0,1\}//'
            return 0 ;;
        link)
            shift
            local team="" ws="" kind="" scope="" topo="" as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --kind) kind="${2:-}"; shift 2 ;;
                    --scope) scope="${2:-}"; shift 2 ;;
                    --mesh) topo="mesh"; shift ;;
                    --json) as_json=true; shift ;;
                    -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) if [ -z "$team" ]; then team="$1"; elif [ -z "$ws" ]; then ws="$1"; else echo "Unexpected: $1" >&2; exit 2; fi; shift ;;
                esac
            done
            [ -n "$team" ] && [ -n "$ws" ] || { echo "Usage: k2 agent team link <team instance|workspace> <workspace it serves> [--kind serves|helper_of] [--scope JSON] [--mesh]" >&2; exit 2; }
            local body
            body=$(T="$team" W="$ws" KIND="$kind" SCOPE="$scope" TOPO="$topo" python3 -c '
import json, os, sys
b = {"team": os.environ["T"], "workspace": os.environ["W"]}
if os.environ.get("KIND"): b["kind"] = os.environ["KIND"]
if os.environ.get("TOPO"): b["topology"] = os.environ["TOPO"]
s = os.environ.get("SCOPE", "")
if s:
    try: b["scope"] = json.loads(s)
    except ValueError: print("--scope must be JSON", file=sys.stderr); sys.exit(2)
print(json.dumps(b))') || exit 2
            cli_post_json "/cli/agent-teams/link" "$body" | _hire_print link "$as_json" ;;
        unlink)
            shift
            local id="${1:-}"
            [ -n "$id" ] || { echo "Usage: k2 agent team unlink <link-id>" >&2; exit 2; }
            cli_post_json "/cli/agent-teams/unlink" "$(L="$id" python3 -c 'import json,os; print(json.dumps({"link": os.environ["L"]}))')" \
                | _hire_print unlink false ;;
        served|""|--json|--workspace)
            local view="team" ws="" as_json=false
            [ "$sub" = "served" ] && { view="served"; shift; }
            while [ $# -gt 0 ]; do
                case "$1" in
                    --workspace) ws="${2:-}"; shift 2 ;;
                    --json) as_json=true; shift ;;
                    *) echo "Unknown argument: $1 (see k2 agent team --help)" >&2; exit 2 ;;
                esac
            done
            [ -n "$ws" ] || ws="$PROJECT"
            cli_request "/cli/agent-teams/links" "workspace=$(urlencode "$ws")" | _hire_print "$view" "$as_json" ;;
        *)
            echo "Unknown: k2 agent team $sub (see k2 agent team --help)" >&2; exit 2 ;;
    esac
}

cmd_agent_instances() {
    local sub="${1:-}"
    case "$sub" in
        --help|-h|help)
            sed -n '/^# ── Agent hire: teams, instances/,/^# Inside a K2 terminal/p' "$0" | sed 's/^# \{0,1\}//'
            return 0 ;;
        show)
            local id="${2:-}" as_json=false
            [ "${3:-}" = "--json" ] && as_json=true
            [ -n "$id" ] || { echo "Usage: k2 agent instances show <instance>" >&2; exit 2; }
            cli_request "/cli/agent-instances" "instance=$(urlencode "$id")" | _hire_print instance "$as_json" ;;
        retire)
            shift
            local id="" force=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --force) force=true; shift ;;
                    --json) as_json=true; shift ;;
                    -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "Usage: k2 agent instances retire <instance> [--force]" >&2; exit 2; }
            cli_post_json "/cli/agent-instances/retire" "$(I="$id" F="$force" python3 -c 'import json,os; print(json.dumps({"instance": os.environ["I"], "force": os.environ["F"] == "true"}))')" \
                | _hire_print retire "$as_json" ;;
        hire)
            shift
            local file="" key="" dry=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --key) key="${2:-}"; shift 2 ;;
                    --dry-run) dry=true; shift ;;
                    --json) as_json=true; shift ;;
                    -*) [ "$1" = "-" ] && { file="-"; shift; continue; }; echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) file="$1"; shift ;;
                esac
            done
            [ -n "$file" ] || { echo "Usage: k2 agent instances hire <recipe.json|-> [--key <idempotency-key>] [--dry-run]" >&2; exit 2; }
            local recipe
            if [ "$file" = "-" ]; then recipe="$(cat)"; else
                [ -r "$file" ] || { echo "Error: cannot read $file" >&2; exit 2; }
                recipe="$(cat "$file")"
            fi
            local body
            body=$(printf '%s' "$recipe" | KEY="$key" DRY="$dry" python3 -c '
import json, os, sys
try: b = json.load(sys.stdin)
except ValueError as e: print("Error: the recipe is not JSON: %s" % e, file=sys.stderr); sys.exit(2)
if os.environ.get("KEY"): b["key"] = os.environ["KEY"]
if os.environ.get("DRY") == "true": b["dryRun"] = True
print(json.dumps(b))') || exit 2
            printf '%s' "$body" | cli_post_json_stdin "/cli/agent-instances/hire" | _hire_print hire "$as_json" ;;
        ""|--json|--key)
            local key="" as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --key) key="${2:-}"; shift 2 ;;
                    --json) as_json=true; shift ;;
                    *) echo "Unknown argument: $1" >&2; exit 2 ;;
                esac
            done
            if [ -n "$key" ]; then
                cli_request "/cli/agent-instances" "key=$(urlencode "$key")" | _hire_print instances "$as_json"
            else
                cli_request "/cli/agent-instances" | _hire_print instances "$as_json"
            fi ;;
        *)
            echo "Unknown: k2 agent instances $sub (see k2 agent instances --help)" >&2; exit 2 ;;
    esac
}

cmd_agent_hire_root() {
    case "${1:-}" in
        set)
            [ -n "${2:-}" ] || { echo "Usage: k2 agent hire-root set <dir>" >&2; exit 2; }
            cli_post_json "/cli/agents/hire-root" "$(R="$2" python3 -c 'import json,os; print(json.dumps({"root": os.environ["R"]}))')" \
                | _hire_print root false ;;
        ""|--json)
            cli_request "/cli/agents/hire-root" | _hire_print root "$([ "${1:-}" = "--json" ] && echo true || echo false)" ;;
        *) echo "Usage: k2 agent hire-root [set <dir>]  — the folder API keys hire agents into (owner sets it)" >&2; exit 2 ;;
    esac
}

