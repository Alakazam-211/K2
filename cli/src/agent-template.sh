# ── k2 agent template — hireable agent types (0.46.12) ──────────────
# prd-agent-templates-v1 T1: list and show. Hiring from one is
# `k2 agent hire --template <id>`.
cmd_help_agent_template() {
    cat <<'EOF'
%include cli/text/help/agent/template.txt
EOF
}

cmd_agent_template() {
    local sub="${1:-}"
    case "$sub" in
        ""|--help|-h|help) cmd_help_agent_template; exit 0 ;;
        list)
            shift
            local all=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --all) all=true; shift ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    *) echo "Unknown argument: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                esac
            done
            if [ "$all" = true ]; then
                cli_request "/cli/agent-templates" "all=1" | _agent_template_print list "$as_json"
            else
                cli_request "/cli/agent-templates" | _agent_template_print list "$as_json"
            fi ;;
        show)
            shift
            local id="" files=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --files) files=true; shift ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    -*) echo "Unknown flag: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                    *) [ -z "$id" ] || { echo "Usage: k2 agent template show <id> (one id)" >&2; exit 2; }
                       id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "Usage: k2 agent template show <id>  (ids: k2 agent template list)" >&2; exit 2; }
            if [ "$files" = true ]; then
                cli_request "/cli/agent-templates/show" "id=$(urlencode "$id")" "files=1" | _agent_template_print show "$as_json" files
            else
                cli_request "/cli/agent-templates/show" "id=$(urlencode "$id")" | _agent_template_print show "$as_json"
            fi ;;
        save)
            shift
            local ws="" id="" name="" summary="" summary_set=false dry=false as_json=false
            local wiki=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --id) [ $# -ge 2 ] || { echo "--id needs a template id" >&2; exit 2; }; id="$2"; shift 2 ;;
                    --name) [ $# -ge 2 ] || { echo "--name needs a name" >&2; exit 2; }; name="$2"; shift 2 ;;
                    --summary) [ $# -ge 2 ] || { echo "--summary needs a line of text" >&2; exit 2; }; summary="$2"; summary_set=true; shift 2 ;;
                    --wiki) [ $# -ge 2 ] || { echo "--wiki needs a note name" >&2; exit 2; }; wiki+=("$2"); shift 2 ;;
                    --dry-run) dry=true; shift ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    -*) echo "Unknown flag: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                    *) [ -z "$ws" ] || { echo "Usage: k2 agent template save <ws> --id <id> (one workspace)" >&2; exit 2; }
                       ws="$1"; shift ;;
                esac
            done
            [ -n "$ws" ] && [ -n "$id" ] || { echo "Usage: k2 agent template save <ws> --id <id> [--name <n>] [--summary <s>] [--wiki <note>]... [--dry-run]  (<ws>: a folder or handle; . for this one)" >&2; exit 2; }
            ws="$(_agent_template_ws "$ws")"
            local body
            body="$(WS="$ws" TID="$id" TNAME="$name" TSUM="$summary" TSUMSET="$summary_set" TDRY="$dry" python3 -c '
import json, os, sys
b = {"workspace": os.environ["WS"], "id": os.environ["TID"], "wiki": sys.argv[1:], "dryRun": os.environ["TDRY"] == "true"}
if os.environ.get("TNAME"):
    b["name"] = os.environ["TNAME"]
if os.environ.get("TSUMSET") == "true":
    b["summary"] = os.environ.get("TSUM", "")
print(json.dumps(b))' ${wiki[@]+"${wiki[@]}"})"
            _agent_template_post "/cli/agent-templates/save" "$body" save "$as_json" ;;
        add)
            shift
            local dir="" dry=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --dry-run) dry=true; shift ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    -*) echo "Unknown flag: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                    *) [ -z "$dir" ] || { echo "Usage: k2 agent template add <dir> (one folder)" >&2; exit 2; }
                       dir="$1"; shift ;;
                esac
            done
            [ -n "$dir" ] || { echo "Usage: k2 agent template add <dir> [--dry-run]  (a folder with template.toml and ROLE.md)" >&2; exit 2; }
            [ -d "$dir" ] || { echo "Error: no_folder: $dir is not a folder" >&2; exit 4; }
            dir="$(cd "$dir" && pwd)"
            _agent_template_post "/cli/agent-templates/add" \
                "$(D="$dir" TDRY="$dry" python3 -c 'import json, os; print(json.dumps({"dir": os.environ["D"], "dryRun": os.environ["TDRY"] == "true"}))')" \
                add "$as_json" ;;
        remove|rm)
            shift
            local id="" version="" as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --version) [ $# -ge 2 ] || { echo "--version needs a number" >&2; exit 2; }; version="${2#v}"; shift 2 ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    -*) echo "Unknown flag: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                    *) [ -z "$id" ] || { echo "Usage: k2 agent template remove <id> (one id)" >&2; exit 2; }
                       id="$1"; shift ;;
                esac
            done
            [ -n "$id" ] || { echo "Usage: k2 agent template remove <id> [--version N]" >&2; exit 2; }
            case "$version" in ""|*[!0-9]*) [ -z "$version" ] || { echo "--version takes a number (1, 2, ...)" >&2; exit 2; } ;; esac
            _agent_template_post "/cli/agent-templates/remove" \
                "$(TID="$id" TV="$version" python3 -c 'import json, os; b = {"id": os.environ["TID"]}
v = os.environ.get("TV", "")
if v:
    b["version"] = int(v)
print(json.dumps(b))')" \
                remove "$as_json" ;;
        upgrade|diff)
            shift
            local args=() dry=false as_json=false
            while [ $# -gt 0 ]; do
                case "$1" in
                    --dry-run) [ "$sub" = upgrade ] || { echo "diff writes nothing; --dry-run is for upgrade" >&2; exit 2; }; dry=true; shift ;;
                    --json) as_json=true; shift ;;
                    --help|-h) cmd_help_agent_template; exit 0 ;;
                    -*) echo "Unknown flag: $1 (see: k2 agent template --help)" >&2; exit 2 ;;
                    *) args+=("$1"); shift ;;
                esac
            done
            local n=${#args[@]} body
            if [ "$n" -eq 1 ]; then
                local ws
                ws="$(_agent_template_ws "${args[0]}")"
                body="$(WS="$ws" TDRY="$dry" python3 -c 'import json, os; print(json.dumps({"workspace": os.environ["WS"], "dryRun": os.environ["TDRY"] == "true"}))')"
            elif [ "$n" -eq 3 ] && [ "$sub" = diff ]; then
                body="$(TID="${args[0]}" A="${args[1]#v}" B="${args[2]#v}" python3 -c '
import json, os, sys
try:
    a, b = int(os.environ["A"]), int(os.environ["B"])
except ValueError:
    print("versions are numbers: k2 agent template diff <id> 1 2", file=sys.stderr); sys.exit(2)
print(json.dumps({"id": os.environ["TID"], "from": a, "to": b}))')" || exit 2
            else
                if [ "$sub" = diff ]; then
                    echo "Usage: k2 agent template diff <ws> | <id> <vA> <vB>  (<ws>: a folder or handle; . for this one)" >&2
                else
                    echo "Usage: k2 agent template upgrade <ws> [--dry-run]  (<ws>: a folder or handle; . for this one)" >&2
                fi
                exit 2
            fi
            _agent_template_post "/cli/agent-templates/$sub" "$body" "$sub" "$as_json" ;;
        *)
            echo "Unknown: k2 agent template $sub (see: k2 agent template --help)" >&2; exit 2 ;;
    esac
}

# The workspace a save / upgrade / diff names. "." is the workspace you
# are in: in a K2 terminal the passport's own (from any subfolder or
# worktree), else the folder found from the current one (K2_PROJECT_PATH,
# or the nearest .k2 above it). Another folder is made absolute; a handle
# passes through for the daemon to resolve.
_agent_template_ws() {
    if [ "$1" = "." ]; then
        _cli_use_passport_project
        printf '%s' "$PROJECT"
    elif [ -d "$1" ]; then
        (cd "$1" && pwd)
    else
        printf '%s' "$1"
    fi
}

# POST a template-library route. Inside a K2 terminal it carries the
# session passport (AT14), so the daemon holds an agent to its own
# workspace; elsewhere the owner token. $1 route, $2 JSON body,
# $3 print mode, $4 "true" for raw JSON.
_agent_template_post() {
    local tok
    tok="$(_k2_project_token)"
    local TOKEN="$tok"
    cli_post_json "$1" "$2" | _agent_template_write_print "$3" "${4:-false}"
}

# Print a save / add / remove / upgrade / diff answer. Exit codes: usage
# 2, refused 3, not found 4, any other refusal 1.
_agent_template_write_print() {
    TV="$1" TJ="${2:-false}" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon answered: %s" % raw[:300], file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err:
    if not isinstance(err, dict):
        err = {"code": "", "hint": str(err)}
    code = err.get("code", "")
    hint = err.get("hint") or err.get("message") or ""
    print("Error: %s%s" % ((code + ": ") if code else "", hint), file=sys.stderr)
    if code in ("usage", "bad_id", "bad_template", "no_charter"):
        sys.exit(2)
    # Refusals of the template itself: exit 1, even when the hint names a
    # token (secret_found says "a GitHub token", not a sign-in problem).
    if code in ("id_reserved", "id_taken", "secret_found", "too_large"):
        sys.exit(1)
    if code in ("forbidden", "role_required", "own_workspace_only", "not_author", "owner_only", "template_conflict") or "token" in hint.lower():
        sys.exit(3)
    if code in ("not_found", "unknown_template", "no_workspace", "no_folder", "no_note", "no_template"):
        sys.exit(4)
    sys.exit(1)
if os.environ.get("TJ") == "true":
    print(json.dumps(d, indent=2)); sys.exit(0)
mode = os.environ.get("TV")
if mode == "remove":
    gone = ", ".join("v%s" % v for v in d.get("removed") or [])
    left = ", ".join("v%s" % v for v in d.get("remaining") or [])
    print("Removed %s: %s%s" % (d.get("id"), gone, ("; still saved: " + left) if left else ""))
    sys.exit(0)
if mode in ("save", "add"):
    dry = d.get("dryRun")
    print("%s %s v%s (author %s) %s %s" % ("Would save" if dry else "Saved", d.get("id"), d.get("version"), d.get("author"), "to" if dry else "at", d.get("path")))
    print("  Files:")
    for f in d.get("files") or []:
        print("    %s (%s bytes)" % (f.get("path"), f.get("bytes")))
    if d.get("catalog"):
        print("  Context layers: %s" % ", ".join(d["catalog"]))
    print("  Tool: %s" % (d.get("tool") or "default"))
    if d.get("heartbeats"):
        print("  Heartbeats: %s" % ", ".join(d["heartbeats"]))
    print("  Suggests: %s  (suggestions only: a hire never gets a switch from a template)" % (", ".join(d.get("suggest") or []) or "nothing"))
    for x in d.get("skipped") or []:
        print("  Not taken: %s %s (%s)" % (x.get("what"), x.get("name"), x.get("why")))
    if dry:
        print("Nothing written (dry run). Run it again without --dry-run to save.")
    else:
        print("Hire from it: k2 agent hire --template %s --name <name>" % d.get("id"))
    sys.exit(0)
if "rows" in d:
    a, b = d.get("from"), d.get("to")
    if a == b:
        print("%s template: already on v%s (%s)" % (d.get("name"), a, d.get("id")))
    else:
        print("%s template: v%s -> v%s (%s)%s" % (d.get("name"), a, b, d.get("id"), "  [dry run]" if d.get("dryRun") else ""))
    for r in d.get("rows") or []:
        if r.get("status") == "unchanged":
            continue
        print("  %-9s %-10s %s%s" % (r.get("status"), r.get("step"), r.get("target"), ("  (" + r["reason"] + ")") if r.get("reason") else ""))
        if r.get("proposed"):
            print("            new text: %s" % r["proposed"])
        if r.get("diff"):
            for line in r["diff"].splitlines():
                print("            " + line)
    if d.get("dryRun"):
        print("Nothing written.%s" % ("" if a == b else " Upgrade: k2 agent template upgrade <ws>"))
    elif d.get("changed"):
        print("Done. The agent has an inbox note saying what changed.")
    else:
        print("Nothing to change.")
    sys.exit(0)
if "files" in d and mode == "diff":
    print("%s: v%s -> v%s" % (d.get("id"), d.get("from"), d.get("to")))
    for f in d.get("files") or []:
        print("  %-8s %s" % (f.get("status"), f.get("path")))
        for line in (f.get("diff") or "").splitlines():
            print("           " + line)
    sys.exit(0)
print(json.dumps(d, indent=2))
'
}

# $1 = list|show, $2 = "true" for raw JSON, $3 = "files" to print files.
# Exit codes: unknown_template/not_found → 4, usage → 2, refused → 3, else 1.
_agent_template_print() {
    TV="$1" TJ="${2:-false}" TF="${3:-}" python3 -c '
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
    if code in ("forbidden", "role_required") or "token" in hint.lower():
        sys.exit(3)
    sys.exit({"not_found": 4, "unknown_template": 4, "usage": 2, "bad_template": 2}.get(code, 1))
if os.environ.get("TJ") == "true":
    print(json.dumps(d, indent=2)); sys.exit(0)
if os.environ.get("TV") == "list":
    ts = d.get("templates") or []
    def line(t):
        extra = []
        if t.get("aliases"):
            extra.append("old id: " + ", ".join(t["aliases"]))
        if t.get("kind") == "system":
            extra.append("system")
        if t.get("origin") == "library":
            extra.append("by " + str(t.get("author")) + ", v" + str(t.get("version")))
        tail = ("  (" + "; ".join(extra) + ")") if extra else ""
        return "  %-14s %-12s %s%s" % (t["id"], t.get("name", ""), t.get("summary", ""), tail)
    k2 = [t for t in ts if t.get("origin") != "library"]
    mine = [t for t in ts if t.get("origin") == "library"]
    print("Agent types (hire one: k2 agent hire --template <id> --name <name>):")
    for t in k2:
        print(line(t))
    if mine:
        print("")
        print("Yours (saved on this server):")
        for t in mine:
            print(line(t))
    for p in d.get("problems") or []:
        print("warning: %s: %s" % (p.get("dir"), p.get("error")), file=sys.stderr)
    sys.exit(0)
t = d.get("template") or {}
print("%s (%s) v%s, by %s" % (t.get("name"), t.get("id"), t.get("version"), t.get("author")))
print("  %s" % t.get("summary", ""))
if t.get("aliases"):
    print("  Old id: %s" % ", ".join(t["aliases"]))
ag = t.get("agent") or {}
print("  Role: %s   Tool: %s%s" % (ag.get("role"), ag.get("tool"), ("   Model: " + ag["model"]) if ag.get("model") else ""))
params = t.get("params") or {}
if params:
    print("  Params (--param key=value):")
    for k, p in params.items():
        print("    %s: %s (up to %s characters%s)" % (k, p.get("label"), p.get("max"), ", required" if p.get("required") else ""))
qs = t.get("questions") or []
if qs:
    print("  Questions (answer at hire with --answer id=text; the agent asks its person the rest):")
    for q in qs:
        f = q.get("fills") or ""
        where = "ROLE.md > What I know about my work" if f == "role" else (f[len("wiki:"):].replace("#", " > ") if f.startswith("wiki:") else f)
        print("    %s: %s  -> %s" % (q.get("id"), q.get("ask"), where))
for label, key in (("Wiki notes", "wiki"), ("Context layers", "catalog"), ("Own layers", "layers"), ("Skills", "skills")):
    if t.get(key):
        print("  %s: %s" % (label, ", ".join(t[key])))
if t.get("heartbeats"):
    print("  Heartbeats: %s" % ", ".join("%s (%s)" % (h["name"], h["frequency"]) for h in t["heartbeats"]))
perm = t.get("permissions") or {}
sugg = list(perm.get("expanded") or [])
if perm.get("cards"):
    sugg = ["permission cards"] + sugg
print("  Suggests: %s" % (", ".join(sugg) if sugg else "nothing"))
print("    (suggestions only: a person turns a switch on, or the agent asks on a card)")
if t.get("hello"):
    print("  Hello: %s" % t["hello"].replace("\n", "\n         "))
if t.get("versions"):
    print("  Versions: %s" % ", ".join("v%s" % v for v in t["versions"]))
print("  Hash: %s" % t.get("hash"))
if os.environ.get("TF") == "files":
    for path, body in sorted((t.get("files") or {}).items()):
        print("")
        print("── %s ──" % path)
        print(body.rstrip("\n"))
'
}

cmd_msg_who() {
    local id="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true ;;
            -h|--help) echo "Usage: k2 msg who <m_…> [--json]  (the id in [from … · m_…])"; exit 0 ;;
            -*) echo "Usage: k2 msg who <m_…> [--json]  (unknown flag $1)" >&2; exit 2 ;;
            *) [ -z "$id" ] || { echo "Usage: k2 msg who <m_…> [--json]  (one id only)" >&2; exit 2; }
               id="$1" ;;
        esac
        shift
    done
    [ -n "$id" ] || { echo "Usage: k2 msg who <m_…> [--json]  (the id in [from … · m_…])" >&2; exit 2; }
    cli_request "/cli/msg/who" "id=$(urlencode "$id")" | _hire_print who "$as_json"
}

