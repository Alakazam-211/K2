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
  k2 agent template save <ws> --id <id> [--name <n>] [--summary <s>] [--wiki <note>]... [--dry-run] [--json]
  k2 agent template add <dir> [--dry-run] [--json]
  k2 agent template remove <id> [--version N] [--json]
  k2 agent template diff <ws> | <id> <vA> <vB> [--json]
  k2 agent template upgrade <ws> [--dry-run] [--json]

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
  save <ws>       Save an agent's setup as a new template you can hire from.
                  <ws> is the agent's folder or handle (an agent: its own
                  only; "." works). It takes the charter (ROLE.md), its
                  context layers, the skills it made, its heartbeats, its
                  tool, and the permission switches now on (as suggestions).
                  Wiki notes only with --wiki. Never Threads, transcripts,
                  vault secrets, mail, sign-ins, .env files or connections.
                  A file holding a secret-shaped string (a token, an API
                  key, a private key) is refused, naming the file and line.
                  Saving the same --id again writes its next version (v2,
                  v3, ...); an id someone else saved, or a K2 id, is refused.
                  Try --dry-run first: it lists every file it would write.
  add <dir>       Install a template folder (template.toml, ROLE.md,
                  wiki/, context/, skills/) into this server's library, as
                  its next version, with you as its author. Same checks
                  as save. An agent adds only from inside its own folder.
  remove <id>     Remove a saved template (every version, or --version N).
                  An agent removes only templates it saved. K2's built-ins
                  can't be removed.
  diff <ws>       What an upgrade would do to an agent made from a template,
                  file by file, with the lines that change. Writes nothing.
  diff <id> <vA> <vB>
                  Two versions of a saved template, file by file.
  upgrade <ws>    Move an agent to its template's newest version. A file the
                  agent never changed is replaced; one it changed is kept,
                  and the new text lands beside it as <file>.proposed to read
                  and merge. A file it deleted stays deleted. In ROLE.md only
                  the K2:MANAGED block is replaced; the agent's own part is
                  never touched. New notes, layers, skills and context are
                  added; new heartbeats and suggested permissions are only
                  listed. Never changes permissions, tool, model, name or
                  connections. The agent gets one inbox note saying what
                  changed. K2's built-ins upgrade by themselves when K2
                  updates; a saved template only with this command. An agent
                  upgrades its own folder only.

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
  --id <id>       save: the new template id (a-z, 0-9, -; at most 40)
  --name <n>      save: its name in the list (default: the agent's name)
  --summary <s>   save: one line on what it is for (default: none)
  --wiki <note>   save: take this wiki note too (repeatable; none by default)
  --version N     remove: only this version
  --dry-run       save, add, upgrade: show what would be written; write nothing
  --json          JSON output

EXAMPLES
  k2 agent template list
  k2 agent template show writer
  k2 agent template save . --id my-writer --summary "Writes our posts" --dry-run
  k2 agent template save . --id my-writer --wiki "Voice and style"
  k2 agent hire --template my-writer --name "Ada"
  k2 agent template remove my-writer --version 1
  k2 agent template diff .
  k2 agent template diff my-writer 1 2
  k2 agent template upgrade . --dry-run
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
            if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
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
                local ws="${args[0]}"
                if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
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
    if code in ("forbidden", "role_required", "own_workspace_only", "not_author", "owner_only") or "token" in hint.lower():
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

