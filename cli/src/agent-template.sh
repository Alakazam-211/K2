# ── k2 agent template — hireable agent types (0.46.12) ──────────────
# prd-agent-templates-v1 T1: list and show. Hiring from one is
# `k2 agent hire --template <id>`.
cmd_help_agent_template() {
    cat <<'EOF'
Agent templates: the types of agent you can hire. A template is a charter
(the agent's ROLE.md), starter wiki notes and context layers. It gives the
agent no permissions: it only suggests some, for a person to turn on.

USAGE
  k2 agent template list [--all] [--json]
  k2 agent template show <id> [--files] [--json]
  k2 agent hire --template <id> --name <name> [--dry-run]   (make an agent from one)

COMMANDS
  list            Every type you can hire: K2's ten built-ins (general, lead,
                  developer, app-owner, designer, researcher, reviewer, writer,
                  support, ops), then any saved on this server, under "Yours".
                  --all also shows K2's system templates (hired by id only).
  show <id>       What one gives: its charter, params (--param on hire; marked
                  when required), its first-conversation questions, starter
                  notes, context layers, suggested permissions and the hello
                  it sends on its first launch.
                  An old id works too: worker = general, manager = lead,
                  qa = reviewer. An unknown id exits 4.

QUESTIONS
  Each built-in asks its person a few short questions on its first start,
  so it can fit its ROLE.md to their work (a writer: who they write for,
  samples, what to avoid, where drafts go). `show` lists each question's
  id, the question, and where the answer goes (ROLE.md, or a starter note).
  If you are hiring for someone and know an answer, pass it at hire (--name
  is the new agent's name):
    k2 agent hire --template writer --name "Max" --answer audience="Founders"
  Answer only what you know; don't guess. The agent asks its person the
  rest, one or two at a time, on the Thread, and its hello names the open
  ones. An answer is written once; to correct one later, tell the agent.

FLAGS
  --all           list: include system templates
  --files         show: print every file of the template
  --json          JSON output

EXAMPLES
  k2 agent template list
  k2 agent template show writer
  k2 agent hire --template writer --name "Max" --agent codex --dry-run
  k2 agent hire --template writer --name "Max" --answer audience="Founders" --answer samples="https://example.k2.dev/blog"
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
        *)
            echo "Unknown: k2 agent template $sub (see: k2 agent template --help)" >&2; exit 2 ;;
    esac
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

