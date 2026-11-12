# ── Workspace Knowledge Base (brain map) — k2 wiki ────────────────────
# Thin wrappers over daemon /cli/wiki/* (indexer + optional localhost serve).
# Publish reuses k2 publish subdomain create/point/rm (K4 in PRD).
# See .k2/prds/prd-workspace-kb-brain-map-and-publish.md.

_wiki_usage() {
    cat <<'EOF' >&2
k2 wiki — per-workspace knowledge base (brain map)

Vault lives at <workspace>/.k2/wiki/ (Markdown + [[wikilinks]]).

USAGE
  k2 wiki status [--json] [project]
  k2 wiki index [--json] [project]
  k2 wiki note <id> [--json] [project]
  k2 wiki seed [project]
  k2 wiki serve on|off [--port N] [project]
  k2 wiki chat off [project]
  k2 wiki publish [--label LABEL] [project]
  k2 wiki unpublish [project]

COMMANDS
  status     Serve + publish state (port, url); public chat is always off
  index      Graph index (nodes + links)
  note       One note body by id (e.g. Home.md)
  seed       Create Home.md + _Index.md if missing
  serve      Start/stop read-only localhost site (127.0.0.1 only)
  chat       Public chat is unavailable (coming back with the K2 runtime); `on` exits 3
  publish    Serve on + nest a subdomain (or print create instructions)
  unpublish  Serve off + hint to remove the nested subdomain

HOSTING
  Wiki hosting is being rebuilt on the K2 runtime. serve/publish keep
  working from the CLI; the site answers only its own loopback host:port
  or its published subdomain, sends no CORS headers, and shows every note
  outside dot-folders. Public chat is off everywhere.

FLAGS
  --json                 Machine-readable stdout (status/index/note)
  --port N               Preferred port for serve on (daemon may reassign)
  --label LABEL          Nested subdomain label for publish (default: workspace basename slug)
  --workspace|--project  Alias for trailing [project] (path, name, or uuid)

EXIT CODES
  0 ok
  1 error (daemon / not found / control plane)
  2 bad usage
  3 public_chat_unavailable (k2 wiki chat on)
EOF
}

_wiki_err() {
    local code="$1" hint="$2"
    if [ "${AS_JSON:-false}" = "true" ] || [ "${as_json:-false}" = "true" ]; then
        # stdout → stderr first, then silence python's own stderr: the JSON
        # used to go to stderr and was swallowed by the 2>/dev/null.
        python3 -c 'import json,sys; print(json.dumps({"error":{"code":sys.argv[1],"hint":sys.argv[2]}}))' \
            "$code" "$hint" >&2 2>/dev/null || echo "{\"error\":{\"code\":\"$code\",\"hint\":\"$hint\"}}" >&2
    else
        echo "Error: $hint" >&2
    fi
}

# Resolve optional workspace selector into global PROJECT (path preferred).
# Accepts absolute/relative dir, workspace name, or uuid (via daemon resolve).
_wiki_apply_project() {
    local sel="${1:-}"
    [ -z "$sel" ] && return 0
    if [ -d "$sel" ]; then
        PROJECT="$(cd "$sel" && pwd)"
        return 0
    fi
    local resolved=""
    if [ -n "${PORT:-}" ] && [ -n "${TOKEN:-}" ]; then
        resolved=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$sel")" 2>/dev/null \
            | python3 -c '
import json,sys
try:
    d=json.loads(sys.stdin.read() or "{}")
except Exception:
    d={}
if isinstance(d, dict):
    print(d.get("path") or d.get("projectPath") or d.get("project_path") or "")
else:
    print("")
' 2>/dev/null || true)
    fi
    if [ -n "$resolved" ]; then
        PROJECT="$resolved"
        return 0
    fi
    # Pass through; daemon /cli/wiki/* also resolves project= tokens.
    PROJECT="$sel"
}

# Default nested label for wiki publish: workspace folder basename, slugified.
_wiki_default_label() {
    local base
    base="$(basename "$PROJECT" 2>/dev/null || echo brain)"
    printf '%s' "$base" | python3 -c '
import re,sys
s=sys.stdin.read().strip().lower()
s=re.sub(r"[^a-z0-9]+","-",s).strip("-")
print((s or "brain")[:48])
' 2>/dev/null || echo "brain"
}

# Parse shared wiki flags. Sets: as_json, project_sel, port, label.
# Remaining positionals go into wiki_pos array.
# Usage: _wiki_parse_args [--need-pos] "$@" then inspect wiki_pos.
_wiki_parse_args() {
    as_json=false
    project_sel=""
    port=""
    label=""
    wiki_pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --port)
                port="${2:-}"
                if [ -z "$port" ]; then
                    _wiki_err "usage" "--port requires a value"
                    exit 2
                fi
                shift 2
                ;;
            --port=*) port="${1#--port=}"; shift ;;
            --label)
                label="${2:-}"
                if [ -z "$label" ]; then
                    _wiki_err "usage" "--label requires a value"
                    exit 2
                fi
                shift 2
                ;;
            --label=*) label="${1#--label=}"; shift ;;
            --workspace|--project)
                project_sel="${2:-}"
                if [ -z "$project_sel" ]; then
                    _wiki_err "usage" "$1 requires a workspace path/name"
                    exit 2
                fi
                shift 2
                ;;
            --workspace=*|--project=*)
                project_sel="${1#*=}"; shift
                ;;
            --help|-h)
                return 99
                ;;
            --)
                shift
                while [ $# -gt 0 ]; do wiki_pos+=("$1"); shift; done
                break
                ;;
            -*)
                _wiki_err "usage" "unknown flag '$1'"
                exit 2
                ;;
            *)
                wiki_pos+=("$1"); shift
                ;;
        esac
    done
    # Trailing bare project when no --workspace given: last positional that
    # is not consumed as a required arg is applied by the caller.
    return 0
}

cmd_wiki() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        status)     cmd_wiki_status "$@" ;;
        index)      cmd_wiki_index "$@" ;;
        note)       cmd_wiki_note "$@" ;;
        seed)       cmd_wiki_seed "$@" ;;
        serve)      cmd_wiki_serve "$@" ;;
        chat)       cmd_wiki_chat "$@" ;;
        publish)    cmd_wiki_publish "$@" ;;
        unpublish)  cmd_wiki_unpublish "$@" ;;
        ""|help|--help|-h)
            cmd_help_wiki
            exit 0
            ;;
        *)
            echo "Unknown 'k2 wiki' subcommand: $sub" >&2
            _wiki_usage
            exit 1
            ;;
    esac
}

cmd_help_wiki() {
    cat <<'EOF'
k2 wiki — per-workspace knowledge base (brain map)

Vault: <workspace>/.k2/wiki/  (Markdown + YAML frontmatter + [[wikilinks]])
Index + optional localhost site live in the daemon (headless-safe).
Publish reuses k2 publish subdomain (nested label.user.k2.dev → 127.0.0.1:port).

USAGE
  k2 wiki status [--json] [project]
  k2 wiki index [--json] [project]
  k2 wiki note <id> [--json] [project]
  k2 wiki seed [project]
  k2 wiki serve on|off [--port N] [project]
  k2 wiki chat off [project]
  k2 wiki publish [--label LABEL] [project]
  k2 wiki unpublish [project]

COMMANDS
  status     Local serve state + published label (if any); public chat is always off
  index      Full graph index (nodes, links, noteCount)
  note       Fetch one note body by stable id (wiki-relative path, e.g. Home.md)
  seed       Create seed notes Home.md + _Index.md when missing
  serve      Toggle read-only localhost SPA (binds 127.0.0.1 only)
  chat       Public chat is unavailable — it is coming back with the K2 runtime.
             `chat on` exits 3 (public_chat_unavailable); `chat off` still works.
  publish    Turn serve ON, then create/point a nested subdomain (or print the command)
  unpublish  Turn serve OFF and print how to remove the nested subdomain

HOSTING
  Hosting is being rebuilt on the K2 runtime (hidden in the desktop app).
  serve/publish/unpublish keep working here. The site answers only its own
  loopback host:port or its published subdomain (no CORS), never shows
  absolute paths or dot-folder notes, and publishes every other note in
  .k2/wiki/ — unpublish anything you don't want public.

PROJECT
  Optional [project] / --workspace / --project: workspace path, name, or uuid.
  Default: current workspace (K2_PROJECT_PATH / walk-up from CWD).

EXAMPLES
  k2 wiki seed
  k2 wiki index --json
  k2 wiki note Home.md
  k2 wiki serve on --port 7420
  k2 wiki publish --label brain
  k2 wiki status --json
  k2 wiki unpublish

SEE ALSO
  k2 publish subdomain create <label> --target 127.0.0.1:<port>
  k2 publish subdomain rm <label>
  k2 publish --help
EOF
}

cmd_wiki_status() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    # Optional trailing project
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    local raw
    raw=$(cli_request "/cli/wiki/serve/status")
    # Enrich with index noteCount when human or when status alone is thin
    local idx_raw=""
    idx_raw=$(cli_request "/cli/wiki/index" 2>/dev/null || true)

    AS_JSON="$as_json" STATUS_RAW="$raw" INDEX_RAW="$idx_raw" PROJECT_PATH="$PROJECT" python3 -c '
import json, os, sys
as_json = os.environ.get("AS_JSON") == "true"
project = os.environ.get("PROJECT_PATH") or ""
raw = os.environ.get("STATUS_RAW") or ""
idx_raw = os.environ.get("INDEX_RAW") or ""

def load(s):
    try:
        return json.loads(s) if s.strip() else {}
    except Exception:
        return {}

d = load(raw)
if not raw.strip():
    print("Error: no response from daemon (wiki routes missing?)", file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err = d["error"]
    if as_json:
        print(json.dumps({"error": err if isinstance(err, dict) else {"code": "daemon", "hint": str(err)}}, separators=(",", ":")))
    else:
        print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)

idx = load(idx_raw)
if isinstance(idx, dict) and idx.get("error") is not None:
    idx = {}

note_count = idx.get("noteCount")
if note_count is None and isinstance(idx.get("nodes"), list):
    note_count = sum(1 for n in idx["nodes"] if n.get("exists", True))
wiki_rel = idx.get("wikiRel") or ".k2/wiki"
ws_path = idx.get("workspacePath") or project
enabled = bool(d.get("enabled") or d.get("serving"))
port = d.get("port")
url = d.get("url") or (("http://127.0.0.1:%s" % port) if port else None)
pub = d.get("publishedLabel") or d.get("published_label") or d.get("label")
generated = idx.get("generatedAt") or d.get("generatedAt")

out = {
    "workspacePath": ws_path,
    "wikiRel": wiki_rel,
    "enabled": enabled,
    "serving": enabled,
    "port": port,
    "url": url,
    "publishedLabel": pub,
    "noteCount": note_count,
    "generatedAt": generated,
}
# Preserve extra daemon fields
for k, v in (d.items() if isinstance(d, dict) else []):
    if k not in out:
        out[k] = v

if as_json:
    print(json.dumps(out, separators=(",", ":")))
    sys.exit(0)

print("Wiki  %s/%s" % (ws_path or "?", wiki_rel))
print("  Notes:      %s" % (note_count if note_count is not None else "?"))
if enabled:
    print("  Serving:    yes  %s" % (url or ("port %s" % (port or "?"))))
else:
    print("  Serving:    no")
if d.get("publicChatEnabled"):
    print("  Public chat: yes (older daemon — update it; chat is disabled from 0.46.0)")
else:
    print("  Public chat: no  (unavailable — coming back with the K2 runtime)")
if pub:
    print("  Published:  %s  (nested subdomain label)" % pub)
else:
    print("  Published:  —")
if generated:
    print("  Indexed:    %s" % generated)
'
}

cmd_wiki_index() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    local raw
    raw=$(cli_request "/cli/wiki/index")
    if [ "$as_json" = "true" ]; then
        AS_JSON=true python3 -c '
import json,sys,os
raw=sys.stdin.read()
if not raw.strip():
    print(json.dumps({"error":{"code":"no_response","hint":"no response from daemon"}}), file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception:
    print(json.dumps({"error":{"code":"bad_json","hint":raw[:200]}}), file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print(json.dumps({"error": err if isinstance(err, dict) else {"code":"daemon","hint":str(err)}})); sys.exit(1)
print(json.dumps(d, separators=(",", ":")))
' <<<"$raw"
        return 0
    fi
    printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print("Error: no response from daemon", file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)
nodes = d.get("nodes") or []
links = d.get("links") or []
exists = [n for n in nodes if n.get("exists", True)]
missing = [n for n in nodes if not n.get("exists", True)]
print("Wiki index  %s" % (d.get("workspacePath") or d.get("wikiRel") or ""))
print("  Generated:  %s" % (d.get("generatedAt") or "—"))
print("  Notes:      %s  (+ %s missing stubs)" % (len(exists), len(missing)))
print("  Links:      %s" % len(links))
if not exists:
    print("")
    print("  Empty vault. Seed with: k2 wiki seed")
    print("  Or drop Markdown under .k2/wiki/")
    sys.exit(0)
print("")
print("  %-28s  %s" % ("ID", "TITLE"))
for n in sorted(exists, key=lambda x: (x.get("title") or x.get("id") or "").lower())[:50]:
    print("  %-28s  %s" % ((n.get("id") or "")[:28], n.get("title") or ""))
if len(exists) > 50:
    print("  … %d more (use --json)" % (len(exists) - 50))
'
}

cmd_wiki_note() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    local note_id=""
    # First positional is note id; optional second is project.
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --workspace|--project)
                project_sel="${2:-}"; [ -z "$project_sel" ] && { _wiki_err "usage" "$1 requires a value"; exit 2; }
                shift 2 ;;
            --workspace=*|--project=*) project_sel="${1#*=}"; shift ;;
            --help|-h) cmd_help_wiki; exit 0 ;;
            -*)
                _wiki_err "usage" "unknown flag '$1' — usage: k2 wiki note <id> [--json] [project]"
                exit 2 ;;
            *)
                if [ -z "$note_id" ]; then note_id="$1"
                elif [ -z "$project_sel" ]; then project_sel="$1"
                else
                    _wiki_err "usage" "unexpected argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$note_id" ]; then
        _wiki_err "usage" "usage: k2 wiki note <id> [--json] [project]"
        exit 2
    fi
    _wiki_apply_project "$project_sel"

    local raw
    raw=$(cli_request "/cli/wiki/note" "id=$(urlencode "$note_id")")
    if [ "$as_json" = "true" ]; then
        printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print(json.dumps({"error":{"code":"no_response","hint":"no response from daemon"}}), file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception:
    print(json.dumps({"error":{"code":"bad_json","hint":raw[:200]}}), file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print(json.dumps({"error": err if isinstance(err, dict) else {"code":"daemon","hint":str(err)}})); sys.exit(1)
print(json.dumps(d, separators=(",", ":")))
'
        return 0
    fi
    printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print("Error: no response from daemon", file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)
title = d.get("title") or d.get("id") or "?"
print("# %s" % title)
tags = d.get("tags") or []
if tags:
    print("tags: %s" % ", ".join(tags))
print("")
body = d.get("body")
if body is None:
    body = d.get("content") or d.get("markdown") or ""
sys.stdout.write(body if body.endswith("\n") else body + "\n")
'
}

cmd_wiki_seed() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    # POST form (daemon: project on query via cli_post_form; body optional).
    local raw=""
    raw=$(cli_post_form "/cli/wiki/seed")
    printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print("Error: no response from daemon", file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)
created = d.get("created") or d.get("files") or []
if isinstance(created, str):
    created = [created]
if created:
    print("Seeded wiki:")
    for f in created:
        print("  + %s" % f)
else:
    print("Wiki already seeded (Home.md / _Index.md present).")
wiki = d.get("wikiRel") or d.get("path") or ".k2/wiki"
print("  Vault: %s" % wiki)
print("  View:  k2 wiki index   or open View Wiki in the app")
'
}

cmd_wiki_serve() {
    local mode="${1:-}"
    shift || true
    case "$mode" in
        on|off|start|stop|true|false|1|0) ;;
        --help|-h|help|"")
            cat <<'EOF'
Usage: k2 wiki serve on|off [--port N] [project]

  on|start    Bind read-only localhost site (127.0.0.1)
  off|stop    Drop the listener (in-app wiki still works)

  --port N    Preferred port when turning on (optional)
EOF
            [ -z "$mode" ] && exit 2 || exit 0
            ;;
        *)
            _wiki_err "usage" "usage: k2 wiki serve on|off [--port N] [project]"
            exit 2
            ;;
    esac

    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    local enabled="false"
    case "$mode" in
        on|start|true|1) enabled="true" ;;
        off|stop|false|0) enabled="false" ;;
    esac

    # Daemon accepts form: enabled=true|false&port=N (project on query).
    local raw
    if [ -n "$port" ]; then
        raw=$(cli_post_form "/cli/wiki/serve" "enabled=${enabled}" "port=$(urlencode "$port")")
    else
        raw=$(cli_post_form "/cli/wiki/serve" "enabled=${enabled}")
    fi
    printf '%s' "$raw" | ENABLED="$enabled" python3 -c '
import json,sys,os
raw=sys.stdin.read()
want = os.environ.get("ENABLED") == "true"
if not raw.strip():
    print("Error: no response from daemon", file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)
enabled = bool(d.get("enabled") if "enabled" in d else d.get("serving", want))
port = d.get("port")
url = d.get("url") or (("http://127.0.0.1:%s" % port) if port else None)
if enabled:
    print("Wiki serve ON  %s" % (url or ("port %s" % (port or "?"))))
    print("  Bind is 127.0.0.1 only (read-only). Publish with: k2 wiki publish [--label LABEL]")
    print("  Public chat: OFF (unavailable — coming back with the K2 runtime)")
else:
    print("Wiki serve OFF")
    if d.get("publishedLabel") or d.get("published_label"):
        print("  Note: nested subdomain may still point here — run k2 wiki unpublish or")
        print("  k2 publish subdomain rm %s" % (d.get("publishedLabel") or d.get("published_label")))
'
}

# W0 (0.46.0) — public wiki chat is disabled until it comes back on the K2
# runtime (it let anonymous visitors drive this workspace's real agent).
# `on` is refused here with exit 3 and never reaches the daemon (an older
# remote daemon would still accept it); the daemon refuses it too
# (410 public_chat_unavailable). `off` still works.
cmd_wiki_chat() {
    local mode="${1:-}"
    shift || true
    case "$mode" in
        on|enable|true|1)
            local as_json=false project_sel="" port="" label=""
            local -a wiki_pos=()
            _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
            _wiki_err "public_chat_unavailable" "Public wiki chat is coming back with the K2 runtime. It is off on every workspace for now; hosting is being rebuilt on the runtime."
            exit 3
            ;;
        off|disable|false|0) ;;
        --help|-h|help|"")
            cat <<'EOF'
Usage: k2 wiki chat off [project]

  off|disable Turn public chat off (serve can stay on)

Public chat is unavailable: it is coming back with the K2 runtime.
`k2 wiki chat on` exits 3 (public_chat_unavailable).
EOF
            [ -z "$mode" ] && exit 2 || exit 0
            ;;
        *)
            _wiki_err "usage" "usage: k2 wiki chat off [project]"
            exit 2
            ;;
    esac

    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    local raw
    raw=$(cli_post_form "/cli/wiki/chat" "enabled=false")
    printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print("Error: no response from daemon", file=sys.stderr); sys.exit(1)
try: d=json.loads(raw)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Error: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err.get("message") or err)), file=sys.stderr)
    sys.exit(1)
if d.get("publicChatEnabled"):
    print("Error: the daemon still reports public chat on (older daemon?) — update it", file=sys.stderr)
    sys.exit(1)
print("Wiki public chat OFF")
print("  Public chat is coming back with the K2 runtime.")
'
}

cmd_wiki_publish() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    _wiki_apply_project "$project_sel"

    if [ -z "$label" ]; then
        label="$(_wiki_default_label)"
    fi
    # skin/meet/chat/mail are box doors (a workspace named "chat" defaults here).
    _refuse_reserved_nested_label "$label"

    # 1) Ensure local serve is on (and capture port/url).
    local serve_raw status_raw
    if [ -n "$port" ]; then
        serve_raw=$(cli_post_form "/cli/wiki/serve" "enabled=true" "port=$(urlencode "$port")" 2>/dev/null || true)
    else
        serve_raw=$(cli_post_form "/cli/wiki/serve" "enabled=true" 2>/dev/null || true)
    fi
    # No dedicated /cli/wiki/publish on daemon V1 — serve + subdomain is enough.
    local pub_raw=""

    status_raw=$(cli_request "/cli/wiki/serve/status" 2>/dev/null || true)
    # Prefer serve response, then publish convenience, then status.
    local port_num url
    port_num=$(STATUS="$status_raw" SERVE="$serve_raw" PUB="$pub_raw" python3 -c '
import json,os,sys
def load(s):
    try: return json.loads(s) if (s or "").strip() else {}
    except Exception: return {}
for s in (os.environ.get("PUB"), os.environ.get("SERVE"), os.environ.get("STATUS")):
    d=load(s)
    if isinstance(d, dict) and d.get("error"): continue
    p=d.get("port")
    if p is not None:
        print(p); sys.exit(0)
print("")
')
    url=$(STATUS="$status_raw" SERVE="$serve_raw" PUB="$pub_raw" python3 -c '
import json,os,sys
def load(s):
    try: return json.loads(s) if (s or "").strip() else {}
    except Exception: return {}
for s in (os.environ.get("PUB"), os.environ.get("SERVE"), os.environ.get("STATUS")):
    d=load(s)
    if isinstance(d, dict) and d.get("error"): continue
    u=d.get("url")
    if u:
        print(u); sys.exit(0)
    p=d.get("port")
    if p is not None:
        print("http://127.0.0.1:%s" % p); sys.exit(0)
print("")
')

    if [ -z "$port_num" ]; then
        _wiki_err "serve_failed" "could not start wiki serve or read port from daemon status"
        echo "  Check: k2 wiki serve on && k2 wiki status" >&2
        exit 1
    fi

    local target="127.0.0.1:${port_num}"
    echo "Wiki local site: ${url:-http://127.0.0.1:${port_num}}"
    echo "  target for publish: $target"
    echo "  label:              $label"
    echo ""

    # 2) Try automated subdomain create. The daemon holds the Connect token
    # (0.45.7: in the K2 vault on desktops); with no token the create
    # fails quietly and we fall through to the manual instructions.
    local created=0
    # One owner per label: never create/point a name another workspace
    # on this server owns (falls through to the manual instructions).
    if _subdomain_label_preflight "$label" 0; then
        local cp_body cp_raw
        cp_body="$(python3 -c 'import json,sys; print(json.dumps({"label": sys.argv[1], "target": sys.argv[2]}))' "$label" "$target")"
        # Prefer create; on "already exists" style errors, try point.
        if cp_raw=$(_connect_curl POST "/subdomains" "$cp_body" 2>/dev/null); then
            local http_code
            http_code=$(printf '%s' "$cp_raw" | head -1 | tr -d '[:space:]')
            if [ "$http_code" = "200" ] || [ "$http_code" = "201" ]; then
                printf '%s' "$cp_raw" | _connect_subdomain_render '
host = d.get("host") or (str(d.get("label","?")) + ".k2.dev")
print("Published %s" % host)
print("  target:    %s" % d.get("target","?"))
print("  status:    %s" % d.get("status","?"))
print("  effective: %s" % d.get("effective","?"))
' 2>/dev/null && created=1 || true
                if [ "$created" = "1" ]; then
                    _stamp_subdomain_attrib claim "$label" || true
                    _nudge_subdomain_refresh || true
                fi
            fi
        fi
        if [ "$created" != "1" ]; then
            # Point (repoint existing label).
            local point_body
            point_body="$(python3 -c 'import json,sys; print(json.dumps({"target": sys.argv[1]}))' "$target")"
            if cp_raw=$(_connect_curl PUT "/subdomains/$(urlencode "$label")" "$point_body" 2>/dev/null); then
                http_code=$(printf '%s' "$cp_raw" | head -1 | tr -d '[:space:]')
                if [ "$http_code" = "200" ] || [ "$http_code" = "201" ]; then
                    printf '%s' "$cp_raw" | _connect_subdomain_render '
host = d.get("host") or (str(d.get("label","?")) + ".k2.dev")
print("Repointed %s -> %s" % (host, d.get("target","?")))
' 2>/dev/null && created=1 || true
                    if [ "$created" = "1" ]; then
                        _stamp_subdomain_attrib claim "$label" || true
                        _nudge_subdomain_refresh || true
                    fi
                fi
            fi
        fi
    fi

    if [ "$created" != "1" ]; then
        echo "To expose this brain on your K2 Connect host (Pro nested subdomain):"
        echo ""
        echo "  k2 publish subdomain create $label --target $target"
        echo ""
        echo "If the label already exists:"
        echo "  k2 publish subdomain point $label --target $target"
        echo ""
        echo "Requires: tunnel up (k2 tunnel start) + Pro plan for nested subdomains."
        echo "Unpublish later: k2 wiki unpublish"
    else
        echo ""
        echo "Live map is on the tunnel target above. Unpublish: k2 wiki unpublish"
    fi

    if [ "$as_json" = "true" ]; then
        python3 -c '
import json,sys
print(json.dumps({
  "ok": True,
  "label": sys.argv[1],
  "target": sys.argv[2],
  "url": sys.argv[3] or None,
  "port": int(sys.argv[4]) if sys.argv[4].isdigit() else sys.argv[4],
  "subdomainCreated": sys.argv[5] == "1",
}, separators=(",", ":")))
' "$label" "$target" "${url:-}" "$port_num" "$created"
    fi
}

cmd_wiki_unpublish() {
    local as_json=false project_sel="" port="" label=""
    local -a wiki_pos=()
    _wiki_parse_args "$@" || { cmd_help_wiki; exit 0; }
    if [ -z "$project_sel" ] && [ ${#wiki_pos[@]} -ge 1 ]; then
        project_sel="${wiki_pos[0]}"
    fi
    # Optional bare label? Spec is unpublish [project] only — if user
    # passed --label, use it for the rm hint.
    _wiki_apply_project "$project_sel"

    local status_raw pub_label
    status_raw=$(cli_request "/cli/wiki/serve/status" 2>/dev/null || true)
    pub_label=$(printf '%s' "$status_raw" | python3 -c '
import json,sys
try: d=json.loads(sys.stdin.read() or "{}")
except Exception: d={}
print(d.get("publishedLabel") or d.get("published_label") or d.get("label") or "")
' 2>/dev/null || true)
    if [ -n "$label" ]; then
        pub_label="$label"
    fi

    local raw
    raw=$(cli_post_form "/cli/wiki/serve" "enabled=false")
    printf '%s' "$raw" | python3 -c '
import json,sys
raw=sys.stdin.read()
if not raw.strip():
    print("Wiki serve: (no daemon response — continuing)")
    sys.exit(0)
try: d=json.loads(raw)
except Exception:
    print("Wiki serve: (unparseable daemon response — continuing)")
    sys.exit(0)
if isinstance(d, dict) and d.get("error") is not None:
    err=d["error"]
    print("Warning: serve off failed: %s" % (err if not isinstance(err, dict) else (err.get("hint") or err)), file=sys.stderr)
    sys.exit(0)
print("Wiki serve OFF")
' || true

    echo ""
    if [ -n "$pub_label" ]; then
        echo "To remove the nested subdomain advertisement:"
        echo "  k2 publish subdomain rm $pub_label"
        echo "  k2 publish subdomain unclaim $pub_label   # drop workspace attribution"
    else
        echo "If you created a nested subdomain for this brain, remove it with:"
        echo "  k2 publish subdomain list"
        echo "  k2 publish subdomain rm <label>"
    fi
    echo ""
    echo "Notes under .k2/wiki/ are kept (unpublish does not delete the vault)."

    if [ "$as_json" = "true" ]; then
        python3 -c '
import json,sys
print(json.dumps({
  "ok": True,
  "serving": False,
  "publishedLabel": sys.argv[1] or None,
  "hint": ("k2 publish subdomain rm %s" % sys.argv[1]) if sys.argv[1] else "k2 publish subdomain list",
}, separators=(",", ":")))
' "${pub_label:-}"
    fi
}

cmd_skills() {
    local subcmd="${1:-list}"
    shift || true
    case "$subcmd" in
        list)
            # Skills live at `.k2so/skills/<name>/SKILL.md` post-Phase-2.5b.
            # The daemon's `/cli/agents/list` route enumerates the same
            # folder (the legacy `.k2so/agents/` tree is consolidated
            # into `.k2so/skills/` at first boot — see
            # `consolidate_skills_v1`). Rename of the route to
            # `/cli/skills/list` is deferred until the daemon-side route
            # split lands; for now the renderer is one HTTP call away.
            cli_request "/cli/agents/list" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if isinstance(d, dict) and 'error' in d:
        sys.stderr.write('Error: %s\n' % d['error']); sys.exit(1)
    items = d if isinstance(d, list) else d.get('agents', [])
    if not items:
        print('No skills in this workspace.'); sys.exit(0)
    name_w = max(len(s.get('name','')) for s in items)
    for s in items:
        nm = s.get('name','')
        role = s.get('role','')
        atype = s.get('agentType') or s.get('agent_type') or 'skill'
        print(f'  {nm:<{name_w}}  [{atype}]  {role}')
except Exception as e:
    sys.stderr.write('Error: %s\n' % e); sys.exit(1)
" 2>/dev/null
            ;;
        create)
            local name="" source_skill="" role=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --template|--from) source_skill="$2"; shift 2 ;;
                    --role) role="$2"; shift 2 ;;
                    --*) shift ;;
                    *) [ -z "$name" ] && name="$1"; shift ;;
                esac
            done
            if [ -z "$name" ]; then
                echo "Usage: k2 skills create <name> [--template <existing-skill>] [--role \"...\"]" >&2
                echo "  --template <name>   Copy an existing skill as the starting point. Any" >&2
                echo "                      skill in this workspace can seed any other; templates" >&2
                echo "                      aren't a separate kind post-Phase-2.5b." >&2
                echo "  --role \"...\"        Frontmatter role/description for the new skill." >&2
                exit 1
            fi
            local params="agent=$(urlencode "$name")"
            [ -n "$role" ] && params="$params&role=$(urlencode "$role")"
            [ -n "$source_skill" ] && params="$params&template=$(urlencode "$source_skill")"
            cli_request_post "/cli/agents/create" "$params" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if isinstance(d, dict) and 'error' in d:
        sys.stderr.write('Error: %s\n' % d['error']); sys.exit(1)
    nm = d.get('name','?')
    print(f'Created skill: {nm}')
    print(f'  Edit: .k2/skills/{nm}/SKILL.md')
except Exception as e:
    sys.stderr.write('Error: %s\n' % e); sys.exit(1)
" 2>/dev/null
            ;;
        profile)
            local name="${1:-}"
            if [ -z "$name" ]; then
                echo "Usage: k2 skills profile <name>" >&2
                exit 1
            fi
            cli_request "/cli/agents/profile" "agent=$(urlencode "$name")" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if isinstance(d, dict) and 'error' in d:
        sys.stderr.write('Error: %s\n' % d['error']); sys.exit(1)
    body = d.get('profile') if isinstance(d, dict) else d
    print(body if isinstance(body, str) else json.dumps(d, indent=2))
except Exception as e:
    sys.stderr.write('Error: %s\n' % e); sys.exit(1)
" 2>/dev/null
            ;;
        remove|delete)
            local name="${1:-}"
            if [ -z "$name" ]; then
                echo "Usage: k2 skills remove <name>" >&2
                exit 1
            fi
            cli_request_post "/cli/agents/delete" "agent=$(urlencode "$name")" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if isinstance(d, dict) and 'error' in d:
        sys.stderr.write('Error: %s\n' % d['error']); sys.exit(1)
    print('Removed skill (sent to recycle bin).')
except Exception as e:
    sys.stderr.write('Error: %s\n' % e); sys.exit(1)
" 2>/dev/null
            ;;
        regenerate)
            cli_request_post "/cli/skills/regenerate" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if 'error' in d:
        sys.stderr.write('Error: %s\n' % d['error'])
        sys.exit(1)
    count = d.get('updated', 0)
    print('Regenerated SKILL.md for %d skill(s).' % count)
except Exception as e:
    sys.stderr.write('Error: %s\n' % e)
" 2>/dev/null
            ;;
        *)
            echo "Usage: k2 skills <list|create|profile|remove|regenerate> [options]" >&2
            echo "  list                              Skill profiles in this workspace" >&2
            echo "  create <name> [--template <src>]  Create a new skill profile" >&2
            echo "                                    (optionally seeded from another skill)" >&2
            echo "  profile <name>                    Print the skill's SKILL.md body" >&2
            echo "  remove <name>                     Trash the skill profile" >&2
            echo "  regenerate                        Refresh every skill's SKILL.md" >&2
            exit 1
            ;;
    esac
}

cmd_feed() {
    # 0.46.1: flags are parsed (--help and --json used to become the limit),
    # daemon errors are shown (not "No activity."), and timestamps carry
    # the date.
    local limit="20" agent="" as_json=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_activity; return 0 ;;
            --json) as_json=1; shift ;;
            --agent)
                [[ $# -ge 2 ]] || { echo "Error: --agent needs a name (k2 activity --help)" >&2; exit 2; }
                agent="$2"; shift 2 ;;
            --limit)
                [[ $# -ge 2 ]] || { echo "Error: --limit needs a number (k2 activity --help)" >&2; exit 2; }
                limit="$2"; shift 2 ;;
            -*) echo "Error: unknown flag '$1' (k2 activity --help)" >&2; exit 2 ;;
            *) limit="$1"; shift ;;
        esac
    done
    case "$limit" in ''|*[!0-9]*) echo "Error: the limit is a number, got '$limit' (k2 activity --help)" >&2; exit 2 ;; esac
    local params="limit=$limit"
    [[ -n "$agent" ]] && params="$params&agent=$(urlencode "$agent")"
    # 0.46.3 A2: an agent's own workspace, from any folder.
    _cli_use_passport_project
    cli_request "/cli/feed" "$params" | FEED_JSON="$as_json" python3 -c "
import json, os, sys, time
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print('Error: the K2 daemon gave no usable answer: %r' % raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get('error') if isinstance(d, dict) else 'unexpected answer'
if err is not None:
    hint = (err.get('hint') or err.get('code') or str(err)) if isinstance(err, dict) else str(err)
    print('Error: %s' % hint, file=sys.stderr)
    sys.exit(1)
if os.environ.get('FEED_JSON') == '1':
    print(json.dumps(d, indent=2))
    sys.exit(0)
items = d.get('feed', [])
if not items:
    print('No activity.')
for e in items:
    ts = time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(e.get('at', 0)))
    agent = e.get('agent') or '-'
    etype = e.get('type', '?')
    summary = e.get('summary') or ''
    frm = e.get('from') or ''
    to = e.get('to') or ''
    route = f' ({frm}→{to})' if frm and to else ''
    print(f'  [{ts}] {agent}: {etype}{route} {summary}')
"
}

# Unwrap daemon {"error": ...} (string or {hint,code}) to stderr and exit 1.
# Success bodies print as-is. curl is not --fail, so HTTP 400 still returns
# JSON with curl exit 0 (prd-workspace-already-exists-copy-v1 C6).
cli_print_or_fail() {
    local resp="$1"
    local err
    err=$(printf '%s' "$resp" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
e = d.get("error")
if not e:
    sys.exit(0)
if isinstance(e, dict):
    print(e.get("hint") or e.get("code") or "daemon refused")
elif e in ("folder_trust_pending", "folder_trust_denied") and d.get("message"):
    print("%s: %s" % (e, d["message"]))
else:
    print(e)' 2>/dev/null)
    if [ -n "$err" ]; then
        echo "Error: $err" >&2
        # Permission cards (vs-live C4): a refusal that teaches is a
        # teaching refusal, exit 3. Every other body keeps exit 1.
        if _permission_in_body "$resp"; then
            exit 3
        fi
        exit 1
    fi
    printf '%s\n' "$resp"
}

cmd_workspace_create() {
    local target_path="" no_wiki=false no_agents_md=false fanout=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --no-wiki) no_wiki=true; shift ;;
            --no-agents-md) no_agents_md=true; shift ;;
            --fanout) fanout=true; shift ;;
            -*) echo "Unknown flag: $1" >&2; echo "Usage: k2 workspace create <path> [--no-wiki] [--no-agents-md] [--fanout]" >&2; exit 1 ;;
            *) if [ -z "$target_path" ]; then target_path="$1"; else echo "Unexpected extra argument: $1" >&2; exit 1; fi; shift ;;
        esac
    done
    if [ -z "$target_path" ]; then
        echo "Usage: k2 workspace create <path> [--no-wiki] [--no-agents-md] [--fanout]" >&2
        exit 1
    fi
    # Resolve to absolute path
    target_path="$(cli_abs_path "$target_path")"
    local extra=()
    [ "$no_wiki" = true ] && extra+=("no_wiki=1")
    [ "$no_agents_md" = true ] && extra+=("no_agents_md=1")
    [ "$fanout" = true ] && extra+=("fanout=1")
    local resp
    if [ ${#extra[@]} -gt 0 ]; then
        resp=$(cli_request_post "/cli/workspace/create" "path=$(urlencode "$target_path")" "${extra[@]}")
    else
        resp=$(cli_request_post "/cli/workspace/create" "path=$(urlencode "$target_path")")
    fi
    cli_print_or_fail "$resp"
}

# P13: re-assert Claude/Codex/Grok/Gemini folder trust for every registered
# workspace. Since 0.46.4 (SS9) only folders a decision covers (or clean ones)
# are re-asserted; a risky folder with no decision is never granted.
cmd_trust_folders() {
    cli_request_post "/cli/trust-folders"
}

# Folder trust (prd-security-sweep-0464-v1 SS10). K2 looks inside a folder
# before it tells Claude, Codex, Grok, Gemini or Cursor to trust it. A folder
# with settings that run commands needs the owner's decision: the Thread card,
# or this command from the owner's own terminal. Agents ask (`request`, which
# only presents the card); people decide.
cmd_help_workspace_trust() {
    cat <<'EOF'
k2 workspace trust — folder trust (who may start AI tools in a folder)

K2 looks inside a folder before it tells Claude, Codex, Grok, Gemini or
Cursor to trust it. A clean folder is trusted as before. A folder with
settings that run commands or send your logins elsewhere waits for the
owner: a card on Thread (Trust it / Always trust this folder / Don't trust).

  k2 workspace trust status [<folder>] [--json]
                                    One folder (or every workspace): clean,
                                    trusted, always, pending, undecided,
                                    changed or denied, with the findings
  k2 workspace trust request <folder> --why "…"
                                    Agents: present the card (only asks)
  k2 workspace trust <folder> --yes|--always|--no [--fingerprint <fp>]
                                    Owner: Trust it (this version) / Always
                                    trust this folder / Don't trust, for the
                                    settings shown. A risky folder needs the
                                    fingerprint from status or the ticket,
                                    or a yes at the prompt in a terminal;
                                    refused if the settings changed
  k2 workspace trust reset <folder> Owner: back to undecided (asks again)
  k2 workspace trust forget <folder>
                                    Owner: remove every trust entry for the
                                    folder in every tool's file (yours and
                                    parent folders' too); status lists them

Deciding is the owner's: an agent or a K2 terminal gets exit 3 (the card is
the request). More: k2 study folder-trust
EOF
}

_workspace_trust_print() {
    # $1 = response body, $2 = mode (status|request|decide)
    printf '%s' "$1" | python3 -c '
import json, sys
mode = sys.argv[1]
try:
    d = json.load(sys.stdin)
except Exception:
    print("Error: the daemon answered something that is not JSON", file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err:
    msg = d.get("message") or d.get("hint") or ""
    if isinstance(err, dict):
        msg = err.get("hint") or err.get("message") or msg
        err = err.get("code") or "refused"
    print("Error: %s%s" % (err, (": " + msg) if msg else ""), file=sys.stderr)
    if err == "action_card_person_only":
        print("It can'"'"'t be requested.", file=sys.stderr)
        sys.exit(3)
    sys.exit(1)
def show(v):
    state = v.get("state", "?")
    line = "%s: %s" % (v.get("path", "?"), state)
    by = v.get("decided_by")
    if by and state in ("trusted", "always", "denied", "changed"):
        line += " (by %s)" % by
    print(line)
    for f in v.get("findings") or []:
        print("  • %s: %s" % (f.get("file"), f.get("detail")))
    if v.get("more"):
        print("  (and %d more)" % v["more"])
    if v.get("card"):
        print("  Card waiting on Thread: %s" % v["card"])
    if v.get("ticket"):
        print("  Owner ticket: %s" % v["ticket"])
    if v.get("scope") and v.get("scope") != v.get("path"):
        print("  Decision covers: %s" % v["scope"])
    if v.get("fingerprint"):
        print("  Fingerprint: %s" % v["fingerprint"])
    for t in v.get("trusted_by") or []:
        print("  Trusted by: %s" % t)
    if v.get("forget"):
        print("  Remove those entries (owner): %s" % v["forget"])
    if state in ("pending", "undecided", "changed", "denied"):
        print("  Decide (owner): %s" % v.get("decide", ""))
if mode == "status":
    if "folders" in d:
        for v in d["folders"]:
            print("%-10s %s" % (v.get("state", "?"), v.get("path", "?")))
    else:
        show(d)
elif mode == "request":
    st = d.get("state")
    if st == "clean":
        print("Clean: no risky settings, so the folder is trusted as before.")
    elif st == "trusted":
        print("Already trusted (a decision covers these settings).")
    elif st == "held":
        where = ("card " + d["card"]) if d.get("card") else (("owner ticket " + d["ticket"]) if d.get("ticket") else "no Thread or ticket")
        print("Asked the owner (%s). K2 starts no program there (but a plain shell tab) until they answer." % where)
        for f in (d.get("findings") or [])[:5]:
            print("  • %s: %s" % (f.get("file"), f.get("detail")))
        for t in d.get("leftover") or []:
            print("  Still trusted outside K2: %s" % t)
    elif st == "denied":
        print("Not trusted: the owner chose \"Don'"'"'t trust\". Only the owner can change it (Settings → Workspaces → Folder trust → Reset).")
    else:
        print("Not gated: %s" % (d.get("reason") or st))
elif mode == "forget":
    removed = d.get("removed") or []
    if removed:
        print("Removed:")
        for t in removed:
            print("  %s" % t)
    else:
        print("Nothing to remove: no tool trusts this folder outside K2'"'"'s own entries.")
    st = d.get("status") or {}
    for t in st.get("trusted_by") or []:
        print("  Still trusted: %s" % t)
else:
    print(d.get("message") or "Done.")
    for t in d.get("leftover") or []:
        print("  Still trusted elsewhere: %s" % t)
' "$2"
}

cmd_workspace_trust() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) cmd_help_workspace_trust; return 0 ;;
        status)
            shift
            local folder="" json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) json=1; shift ;;
                    -*) echo "Unknown flag: $1" >&2; echo "Usage: k2 workspace trust status [<folder>] [--json]" >&2; exit 2 ;;
                    *) [ -z "$folder" ] || { echo "Usage: k2 workspace trust status [<folder>] [--json]" >&2; exit 2; }; folder="$1"; shift ;;
                esac
            done
            local resp
            if [ -n "$folder" ]; then
                resp=$(cli_send cli_request "/cli/folder-trust/status" "path=$(urlencode "$(cli_abs_path "$folder")")")
            else
                resp=$(cli_send cli_request "/cli/folder-trust/status")
            fi
            if [ "$json" -eq 1 ]; then
                cli_print_or_fail "$resp"
                return
            fi
            _workspace_trust_print "$resp" status
            return
            ;;
        request)
            shift
            local folder="" why=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) [ $# -ge 2 ] || { echo "Usage: k2 workspace trust request <folder> --why \"…\"" >&2; exit 2; }; why="$2"; shift 2 ;;
                    -*) echo "Unknown flag: $1" >&2; echo "Usage: k2 workspace trust request <folder> --why \"…\"" >&2; exit 2 ;;
                    *) [ -z "$folder" ] || { echo "Usage: k2 workspace trust request <folder> --why \"…\"" >&2; exit 2; }; folder="$1"; shift ;;
                esac
            done
            if [ -z "$folder" ]; then
                echo "Usage: k2 workspace trust request <folder> --why \"…\"" >&2
                exit 2
            fi
            local resp
            resp=$(cli_send cli_request_post "/cli/folder-trust/request" "path=$(urlencode "$(cli_abs_path "$folder")")" "why=$(urlencode "$why")")
            _workspace_trust_print "$resp" request
            return
            ;;
    esac
    # Owner verbs: <folder> --yes|--always|--no, reset <folder>, forget <folder>.
    local endpoint="/cli/folder-trust/decide" folder="" decision=""
    if [ "$sub" = "reset" ]; then
        endpoint="/cli/folder-trust/reset"
        decision="reset"
        shift
    elif [ "$sub" = "forget" ]; then
        endpoint="/cli/folder-trust/forget"
        decision="forget"
        shift
    fi
    local usage="Usage: k2 workspace trust <folder> --yes|--always|--no [--fingerprint <fp>]   (or: reset <folder>, forget <folder>)"
    local fingerprint=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --yes)    case "$decision" in reset|forget) echo "$usage" >&2; exit 2 ;; esac; decision="once"; shift ;;
            --always) case "$decision" in reset|forget) echo "$usage" >&2; exit 2 ;; esac; decision="always"; shift ;;
            --no)     case "$decision" in reset|forget) echo "$usage" >&2; exit 2 ;; esac; decision="deny"; shift ;;
            --fingerprint) case "$decision" in reset|forget) echo "$usage" >&2; exit 2 ;; esac; [ $# -ge 2 ] || { echo "$usage" >&2; exit 2; }; fingerprint="$2"; shift 2 ;;
            -*) echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *) [ -z "$folder" ] || { echo "$usage" >&2; exit 2; }; folder="$1"; shift ;;
        esac
    done
    if [ -z "$folder" ] || [ -z "$decision" ]; then
        echo "$usage" >&2
        exit 2
    fi
    folder="$(cli_abs_path "$folder")"
    # A session passport (an agent, a K2 terminal) never decides: the card
    # is the request.
    if [ -n "$SCOPED_TOKEN" ]; then
        echo "Error: only the owner decides folder trust; an agent or a K2 terminal can't. The card is the request: k2 workspace trust request $(printf '%q' "$folder") --why \"…\"" >&2
        echo "It can't be requested." >&2
        exit 3
    fi
    local resp
    if [ "$decision" = "reset" ]; then
        resp=$(cli_request_post "$endpoint" "path=$(urlencode "$folder")")
        _workspace_trust_print "$resp" status
    elif [ "$decision" = "forget" ]; then
        resp=$(cli_request_post "$endpoint" "path=$(urlencode "$folder")")
        _workspace_trust_print "$resp" forget
    else
        # The daemon decides only for the settings the owner saw (reviews
        # B#3, round 2 M3): the --fingerprint from status or the ticket, or
        # a yes at a terminal prompt that shows the findings first. A risky
        # folder is never decided from a fingerprint fetched and sent back
        # unseen.
        local scope_arg=""
        if [ -z "$fingerprint" ]; then
            local st fp_scope
            st=$(cli_request "/cli/folder-trust/status" "path=$(urlencode "$folder")")
            _workspace_trust_print "$st" status
            fp_scope=$(printf '%s' "$st" | python3 -c 'import json,sys
d = json.load(sys.stdin)
print(d.get("fingerprint") or "none")
print(d.get("scope") or "")')
            fingerprint=$(printf '%s\n' "$fp_scope" | sed -n 1p)
            local scope
            scope=$(printf '%s\n' "$fp_scope" | sed -n 2p)
            if [ "$fingerprint" != "none" ]; then
                if [ -t 0 ] && [ -t 1 ]; then
                    local answer=""
                    printf 'Decide "%s" for the settings above (fingerprint %s)? [y/N] ' "$decision" "$fingerprint"
                    read -r answer || answer=""
                    case "$answer" in
                        y|Y|yes|YES|Yes) ;;
                        *) echo "Nothing decided." >&2; exit 1 ;;
                    esac
                else
                    echo "Error: this folder has risky settings; decide only for the ones you read. Run: k2 workspace trust status $(printf '%q' "$folder"), read the findings, then pass the --fingerprint it shows (or run this in a terminal to confirm at a prompt)." >&2
                    exit 2
                fi
            fi
            [ -z "$scope" ] || scope_arg="scope=$(urlencode "$scope")"
        fi
        if [ -n "$scope_arg" ]; then
            resp=$(cli_request_post "$endpoint" "path=$(urlencode "$folder")" "decision=$decision" "fingerprint=$(urlencode "$fingerprint")" "$scope_arg")
        else
            resp=$(cli_request_post "$endpoint" "path=$(urlencode "$folder")" "decision=$decision" "fingerprint=$(urlencode "$fingerprint")")
        fi
        _workspace_trust_print "$resp" decide
    fi
}

cmd_workspace_preview() {
    local target_path="${1:-}"
    if [ -z "$target_path" ]; then
        target_path="$PWD"
    fi
    if [ -d "$target_path" ]; then
        target_path="$(cd "$target_path" && pwd)"
    fi
    echo "Preview: what K2 will do if you add $target_path"
    echo ""
    echo "Collision-prone files K2 takes over (archive → import body into SKILL.md → symlink):"
    local any_found=false
    for rel in "CLAUDE.md" "GEMINI.md" "AGENT.md" ".goosehints" ".cursor/rules/k2so.mdc"; do
        if [ -f "$target_path/$rel" ] && [ ! -L "$target_path/$rel" ]; then
            # If this is K2's own generated file, don't report it as user content.
            if grep -q "k2so_generated: true" "$target_path/$rel" 2>/dev/null; then
                printf "  %-30s  K2-generated (will refresh in place)\n" "$rel"
            else
                local size
                size=$(wc -c < "$target_path/$rel" 2>/dev/null | tr -d ' ')
                printf "  %-30s  %s bytes → will archive + import\n" "$rel" "$size"
                any_found=true
            fi
        fi
    done
    if [ -f "$target_path/.aider.conf.yml" ] && [ ! -L "$target_path/.aider.conf.yml" ]; then
        if grep -q "SKILL.md" "$target_path/.aider.conf.yml" 2>/dev/null; then
            echo "  .aider.conf.yml               already references SKILL.md (no change)"
        else
            echo "  .aider.conf.yml               will archive + merge SKILL.md into read: list"
        fi
        any_found=true
    fi
    if [ "$any_found" = "false" ]; then
        echo "  (none — workspace is clean for first-time setup)"
    fi
    echo ""
    echo "Files K2 will create fresh (no pre-existing file):"
    for rel in "SKILL.md" "CLAUDE.md" "GEMINI.md" "AGENT.md" ".goosehints" ".cursor/rules/k2so.mdc" ".aider.conf.yml" ".opencode/agent/k2so.md" ".pi/skills/k2so/SKILL.md"; do
        if [ ! -e "$target_path/$rel" ]; then
            printf "  %s\n" "$rel"
        fi
    done
    echo ""
    echo "Marker-injected (preserves your existing content, K2 adds its own block):"
    for rel in "AGENTS.md" ".github/copilot-instructions.md"; do
        if [ -f "$target_path/$rel" ]; then
            echo "  $rel  (exists — K2 block inserted between markers)"
        else
            echo "  $rel  (will create)"
        fi
    done
    echo ""
    echo "Nothing is destroyed — archives land in .k2/migration/ and can be restored via"
    echo "  k2 workspace remove $target_path --mode restore-original"
}

cmd_workspace_remove() {
    local target_path="" mode="" apps_way="" drop=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --mode) mode="$2"; shift 2 ;;
            --mode=*) mode="${1#--mode=}"; shift ;;
            # Q8: a workspace that maintains Apps goes only after they do:
            # handover:<ws> (owner) | uninstall | park (k2 app release).
            --apps) apps_way="${2:-}"; shift 2 ;;
            --apps=*) apps_way="${1#--apps=}"; shift ;;
            --drop-access) drop=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 1 ;;
            *) if [ -z "$target_path" ]; then target_path="$1"; fi; shift ;;
        esac
    done
    local target_token="$target_path"
    if [ -z "$target_path" ]; then
        echo "Usage: k2 workspace remove <path> [--mode keep-current|restore-original]" >&2
        echo "                           [--apps handover:<ws>|uninstall|park [--drop-access]]" >&2
        echo "" >&2
        echo "  --mode keep-current      Freeze the current SKILL.md body into each harness file so" >&2
        echo "                           every CLI LLM keeps working after K2 disconnects." >&2
        echo "  --mode restore-original  Restore pre-K2 files from .k2/migration/; files K2" >&2
        echo "                           created fresh are removed. .k2/ stays intact for reconnect." >&2
        echo "  (no mode)                DB-only deregister; filesystem untouched (symlinks remain)." >&2
        echo "  --apps handover:<ws>|uninstall|park" >&2
        echo "                           First release the Apps this workspace maintains:" >&2
        echo "                           hand them to another workspace, uninstall them, or" >&2
        echo "                           park them (stopped, held for the owner). Without" >&2
        echo "                           --apps, a workspace with Apps is refused (exit 1)." >&2
        echo "                           All or nothing: if the workspace can't be removed, or" >&2
        echo "                           an App would lose people's access (uninstall), no App" >&2
        echo "                           is released and the reasons are listed, each App with" >&2
        echo "                           its counts (exit 3)." >&2
        echo "  --drop-access            With --apps uninstall: uninstall even Apps whose users" >&2
        echo "                           would lose their logins, grants, shared secrets or" >&2
        echo "                           chat Spaces. With any other --apps: exit 2." >&2
        exit 1
    fi
    # Resolve to absolute path
    if [ -d "$target_path" ]; then
        target_path="$(cd "$target_path" && pwd)"
    fi
    # 0.40.24 (B3): accept a NAME as well as a path — every other verb
    # (`connections`, `msg`) already resolves name-or-path; remove was
    # path-only. Route the token through the daemon's canonical resolver
    # (name | absolute path | UUID) before the path-keyed removal.
    local resolved
    resolved=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target_path")" \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path", ""))
except Exception:
    pass' 2>/dev/null) || resolved=""
    if [ -z "$resolved" ]; then
        echo "Error: workspace '$target_token' not found (by name or path)." >&2
        echo "Run 'k2 workspace list' for registered workspaces." >&2
        exit 4
    fi
    target_path="$resolved"
    if [ "$drop" = 1 ] && [ "$apps_way" != "uninstall" ]; then
        echo "usage: --drop-access goes with --apps uninstall" >&2
        exit 2
    fi
    if [ -n "$apps_way" ]; then
        # forRemoval: the daemon refuses before releasing anything when the
        # workspace itself can't go (all or nothing).
        _app_post_confirming /cli/app/release "Give this workspace's Apps to ${apps_way#handover:}" K2_CJ_workspace="$target_path" K2_CJ_apps="$apps_way" K2_CJ_dropAccess_b="$drop" K2_CJ_forRemoval_b=1
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    fi
    local resp
    if [ -n "$mode" ]; then
        resp=$(cli_request_post "/cli/workspace/remove" \
            "path=$(urlencode "$target_path")" \
            "mode=$(urlencode "$mode")")
    else
        resp=$(cli_request_post "/cli/workspace/remove" "path=$(urlencode "$target_path")")
    fi
    # Surface daemon refusals cleanly (exit 1) instead of echoing raw
    # error JSON with exit 0 — notably the Projects V1 §4.5 PoC guard
    # ({"error":{"code":"poc_of_projects","hint":"X is the Point of
    # Contact for: A, B. Reassign the PoC first."}}).
    cli_print_or_fail "$resp"
}

cmd_workspace_open() {
    local target_path="" no_wiki=false no_agents_md=false fanout=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --no-wiki) no_wiki=true; shift ;;
            --no-agents-md) no_agents_md=true; shift ;;
            --fanout) fanout=true; shift ;;
            -*) echo "Unknown flag: $1" >&2; echo "Usage: k2 workspace open <path> [--no-wiki] [--no-agents-md] [--fanout]" >&2; exit 1 ;;
            *) if [ -z "$target_path" ]; then target_path="$1"; else echo "Unexpected extra argument: $1" >&2; exit 1; fi; shift ;;
        esac
    done
    if [ -z "$target_path" ]; then
        echo "Usage: k2 workspace open <path> [--no-wiki] [--no-agents-md] [--fanout]" >&2
        exit 1
    fi
    # Resolve to absolute path
    if [ -d "$target_path" ]; then
        target_path="$(cd "$target_path" && pwd)"
    fi
    local extra=()
    [ "$no_wiki" = true ] && extra+=("no_wiki=1")
    [ "$no_agents_md" = true ] && extra+=("no_agents_md=1")
    [ "$fanout" = true ] && extra+=("fanout=1")
    local resp
    if [ ${#extra[@]} -gt 0 ]; then
        resp=$(cli_request_post "/cli/workspace/open" "path=$(urlencode "$target_path")" "${extra[@]}")
    else
        resp=$(cli_request_post "/cli/workspace/open" "path=$(urlencode "$target_path")")
    fi
    cli_print_or_fail "$resp"
}

cmd_heartbeat_toggle() {
    # 0.40.41 (GH#10/#22/#23/#24): this used to be the legacy control
    # surface — a workspace-level on/off POST to /cli/heartbeat (route
    # removed in 0.40.31, so it 404'd), a single unnamed Lane-A schedule
    # writer that accepted ANY token as a frequency (how `--help` junk got
    # written), and a bare-verb "triage" fallback that exited 0 on error.
    # All of that is gone. Named heartbeats (`k2 heartbeat schedule ...`)
    # are the only heartbeat control surface; this function only maps
    # retired verbs to their replacements and rejects unknowns loudly.
    local action="${1:-}"
    case "$action" in
        on|enable|1|true|off|disable|0|false)
            fail_deprecated "heartbeat $action" "k2 heartbeat schedule enable|disable <name>   (per-heartbeat pause/resume)"
            ;;
        noop)
            fail_deprecated "heartbeat noop" "nothing — per-agent adaptive backoff was removed; heartbeat cadence is user-owned via 'k2 heartbeat schedule'"
            ;;
        action)
            fail_deprecated "heartbeat action" "nothing — per-agent adaptive backoff was removed; heartbeat cadence is user-owned via 'k2 heartbeat schedule'"
            ;;
        force)
            fail_deprecated "heartbeat force" "k2 heartbeat signal fire <name>  (fires a named heartbeat now)"
            ;;
        set)
            fail_deprecated "heartbeat set" "k2 heartbeat schedule add/edit --name <n> <spec>  (see 'k2 help heartbeat')"
            ;;
        get)
            fail_deprecated "heartbeat get" "k2 heartbeat show <name> [--json]"
            ;;
        *)
            echo "Unknown 'heartbeat' subcommand: ${action}" >&2
            echo "Run 'k2 help heartbeat' for the schedule/signal/inspection surface." >&2
            exit 2
            ;;
    esac
}

cmd_settings() {
    # 0.39.45 (#26c): `k2 settings --mode <value>` actually SETS the
    # workspace agent mode (it used to print the settings and ignore
    # the flag).
    # 0.46.9: `k2 settings set <key> <value>`, the server switches the
    # daemon's hints name (voice rooms, voice tunnel, compute, board files).
    if [ "${1:-}" = "set" ]; then
        shift
        _settings_set "$@"
        return
    fi
    local set_mode=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_settings; return 0 ;;
            --mode)
                set_mode="${2:-}"
                shift
                [[ $# -gt 0 ]] && shift
                ;;
            -*)
                echo "Error: unknown flag '$1' (supported: --mode <off|custom|manager|k2>)" >&2
                exit 2
                ;;
            *) shift ;;
        esac
    done
    if [[ -n "$set_mode" ]]; then
        case "$set_mode" in
            # `k2so` accepted for back-compat (B2); the daemon stores one
            # canonical spelling either way (settings::stored_agent_mode_value).
            off|custom|manager|k2|k2so) ;;
            *)
                echo "Error: invalid mode '$set_mode' (valid: off, custom, manager, k2)" >&2
                exit 2
                ;;
        esac
        cli_request_post "/cli/mode" "set=$(urlencode "$set_mode")" | _settings_print set
        return
    fi
    cli_request "/cli/settings" | _settings_print show
}

# `k2 settings set <key> <true|false>` (0.46.9): the short list of server
# switches K2's own hints tell the owner to flip. Each is an owner-only key
# on POST /cli/settings/update (settings_routes OWNER_ONLY_KEYS); the daemon
# refuses agents and every other login, so this CLI decides nothing but the
# allowlist and the value. Order = the order help prints them.
_K2_SETTINGS_SET_KEYS=(voiceRoomsPreview voiceTunnelPreview computePreview ciPreview boardFilePreview videoRoomsPreview usageSingleRecord)

_settings_set_keys_line() {
    local IFS=","
    printf '%s' "${_K2_SETTINGS_SET_KEYS[*]}" | sed 's/,/, /g'
}

_settings_set() {
    case "${1:-}" in
        -h|--help) cmd_help_settings; return 0 ;;
    esac
    if [ $# -ne 2 ]; then
        echo "Error: usage: k2 settings set <key> <true|false>  (keys: $(_settings_set_keys_line))" >&2
        exit 2
    fi
    local key="$1" raw="$2" value="" k known=0
    for k in "${_K2_SETTINGS_SET_KEYS[@]}"; do
        [ "$k" = "$key" ] && known=1
    done
    if [ "$known" != 1 ]; then
        echo "Error: k2 settings set doesn't change '$key'. It takes: $(_settings_set_keys_line). Other settings are in the K2 app's Settings." >&2
        exit 2
    fi
    case "$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')" in
        true|on) value=true ;;
        false|off) value=false ;;
        *)
            echo "Error: $key takes true or false (on and off work too), not '$raw'." >&2
            exit 2
            ;;
    esac
    local raw_out status out
    raw_out="$(cli_send_json POST /cli/settings/update "{\"$key\":$value}")"
    status="${raw_out##*$'\n'}"
    out="${raw_out%$'\n'*}"
    [ "$status" = "$raw_out" ] && out=""
    [ -n "$status" ] || status="000"
    printf '%s' "$out" | SS_KEY="$key" SS_VALUE="$value" SS_STATUS="$status" python3 -c '
import json, os, sys
key, want, status = os.environ["SS_KEY"], os.environ["SS_VALUE"] == "true", os.environ["SS_STATUS"]
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    d = None
if status != "200" or not isinstance(d, dict) or d.get("error") is not None:
    err = d.get("error") if isinstance(d, dict) else None
    code = err.get("code", "") if isinstance(err, dict) else ""
    hint = (err.get("hint") or err.get("message") or code) if isinstance(err, dict) else (err if isinstance(err, str) else "")
    if not hint:
        hint = "no answer from the K2 daemon" if status == "000" else "the daemon answered HTTP %s: %r" % (status, raw[:200])
    print("Error: %s%s" % ((code + ": ") if code and code not in hint else "", hint), file=sys.stderr)
    low = (code + " " + hint).lower()
    if status in ("401", "403") or any(w in low for w in ("owner", "admin", "forbidden", "not allowed", "refused", "passport")):
        print("Server settings are the owner'"'"'s: ask your human, or run it from a terminal outside a K2 agent session.", file=sys.stderr)
        sys.exit(3)
    sys.exit(2 if status == "400" else 1)
if d.get(key) is not want:
    print("Error: the daemon answered, but %s is %s, not %s. Run k2 settings set again, or check the daemon log." % (key, json.dumps(d.get(key)), json.dumps(want)), file=sys.stderr)
    sys.exit(1)
print("%s is now %s on this server." % (key, "true" if want else "false"))
nxt = {
    "voiceRoomsPreview": "k2 voice status says whether this server can host rooms, and why not.",
    "voiceTunnelPreview": "k2 hostchat media tier tunnel moves voice onto K2'"'"'s tunnel.",
    "computePreview": "k2 compute nodes lists the machines.",
    "ciPreview": "k2 study ci (CI on K2 Compute: k2 ci, node rosters, lanes, seats, signer pairing; needs computePreview on too).",
    "boardFilePreview": "Board files open in a tab in the K2 app.",
    "videoRoomsPreview": "Video in voice rooms shows in the K2 desktop app'"'"'s call panel (preview).",
    "usageSingleRecord": "One usage percent per token: the top bar, Settings and the phone read one record. Off goes straight back.",
}
if want:
    print("Next: " + nxt[key])
'
}

# 0.46.1: a daemon refusal (an agent passport, a member) used to print
# "Name: ?, Mode: off" with exit 0. Errors go to stderr: exit 3 for a
# refusal (with the owner/admin hint), 1 for anything else.
_settings_print() {
    SP_VIEW="$1" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: the K2 daemon gave no usable answer: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else "unexpected answer"
if err is not None:
    code = err.get("code", "") if isinstance(err, dict) else ""
    hint = (err.get("hint") or err.get("message") or code) if isinstance(err, dict) else str(err)
    print("Error: %s%s" % ((code + ": ") if code and code not in hint else "", hint), file=sys.stderr)
    low = (code + " " + hint).lower()
    if any(w in low for w in ("owner", "admin", "forbidden", "token", "not allowed", "refused", "passport")):
        print("Workspace settings are the owner'"'"'s or an admin'"'"'s: ask your human, or run it from a terminal outside a K2 agent session.", file=sys.stderr)
        sys.exit(3)
    sys.exit(1)
if os.environ.get("SP_VIEW") == "set":
    print("Mode set to %s" % d.get("mode", "?"))
    sys.exit(0)
print("Workspace Settings:")
print("  Name:      %s" % d.get("name", "?"))
print("  Mode:      %s" % d.get("mode", "off"))
print("  Heartbeat: %s" % ("on" if d.get("heartbeatEnabled") else "off"))
print("  Pinned:    %s" % ("yes" if d.get("pinned") else "no"))
'
}

cmd_help() {
    # Phase 2.1: 3-tier help.
    #   k2 help                  → daily verbs (14)
    #   k2 help --advanced       → power-user verbs (heartbeat, daemon, etc.)
    #   k2 help --internal       → orchestrator-RPC verbs (rare)
    #   k2 help <verb>           → per-verb help (msg, inbox, workspace, ...)
    case "${1:-}" in
        -a|--advanced|advanced)   cmd_help_v2_advanced ;;
        --internal|internal)      cmd_help_internal ;;
        msg)                      cmd_help_msg ;;
        thread)                   cmd_help_thread ;;
        browser)                  cmd_help_browser ;;
        whoami)                   cmd_help_whoami ;;
        setup)                    _setup_usage ;;
        talk)                     cmd_help_talk ;;
        read)                     cmd_help_read ;;
        inbox)                    cmd_help_inbox ;;
        agent)                    cmd_help_agent ;;
        preset)                   cmd_help_preset ;;
        sidecar)                  _sidecar_usage ;;
        compute)                  _compute_usage ;;
        ci)                       _ci_usage ;;
        changes)                  _changes_usage ;;
        sim)                      _sim_usage ;;
        hostchat)                 _hostchat_usage ;;
        chat)                     _chat_usage ;;
        sources|source)           _sources_usage ;;
        llm)                      _llm_accounts_usage ;;
        usage)                    _usage_cli_usage ;;
        vault)                    _vault_usage ;;
        job|jobs)                 _job_usage ;;
        feedback|tickets)         cmd_help_feedback ;;
        mail)                     cmd_help_mail ;;
        calendar|cal)             cmd_help_calendar ;;
        db)                       cmd_help_db ;;
        store)                    cmd_help_store ;;
        board)                    cmd_help_board ;;
        dns)                      cmd_help_dns ;;
        domain)                   cmd_help_domain ;;
        cert)                     cmd_help_cert ;;
        project)                  cmd_help_project ;;
        workspace)                cmd_help_workspace ;;
        skills)                   cmd_help_skills ;;
        settings)                 cmd_help_settings ;;
        heartbeat)                cmd_help_heartbeat ;;
        wiki)                     cmd_help_wiki ;;
        zen)                      cmd_help_zen ;;
        publish)                  _publish_usage ;;
        connect)                  _connect_usage ;;
        users)                    _users_usage ;;
        remote-session)           cmd_help_remote_session ;;
        study)                    _study_usage ;;
        explain)                  _explain_usage ;;
        capabilities)             _capabilities_usage ;;
        deprecated|--deprecated)  cmd_help_deprecated ;;
        '')                       cmd_help_v2_daily ;;
        *)                        cmd_help_v2_daily ;;
    esac
}

cmd_help_whoami() {
    cat <<'EOF'
k2 whoami [--json]

Print this cell's identity (same fields a launch snapshot pastes).
Sidecar means this chat is an extra session of the workspace, not the
primary. Owner messages here stay here unless you need the primary.

  workspace: sales
  role:      sidecar | canonical
  address:   sales/reviewer   (or sales/1 or just sales)
  primary:   sales
  session:   <provider conversation id, else daemon PTY id>

Exit 0. Fails loud if identity cannot be determined (run inside a
workspace session). --json prints the daemon object.
EOF
}

cmd_help_msg() {
    cat <<'EOF'
k2 msg — talk to another workspace agent (live chat OR a file package)

THREE TOOLS (do not mix)
  k2 msg     SEND to another agent: live "text" OR --inbox-wake/--inbox-silent files
  k2 inbox   YOUR tray (receive/triage). Cannot send a file.
  k2 mail    real email (humans / SMTP). Not agents.

LIVE CHAT (short, one line)
  k2 msg <handle> "ready when you are"

LONGER MESSAGES: SAY IT IN ONE LINE TOO
  k2 msg <handle> --summary "Need a review of hold/x by 5pm" "…the details…"
      For anything longer than ~3 lines, pass --summary saying what you need
      or what you did. People see it in Chatter, the Projects view and on the
      phone; the agent still gets the whole message. Over ~600 characters
      without one, k2 prints a hint (K2 uses the first line otherwise).
      Works on --inbox-wake/--inbox-silent too (shown in `k2 inbox list`).

SEND A FILE
  k2 msg <handle> --inbox-wake ./brief.md [--title "..."] [--body "..."]
      Copy into their .k2/inbox/ and ping them: Open: k2 inbox read <id>
      Preferred. Prints the id. Multiple paths → ONE package, one id.
      Extra prose / --body is the cover brief (files AND instructions).
  k2 msg <handle> --inbox-silent ./data.csv [--title "..."] [--body "..."]
      Copy only. Does NOT notify. They need k2 inbox list.

  Bare --inbox is an error. Do not paste file contents into live msg text.

RECEIVE
  k2 inbox list
  k2 inbox read <id>

WHO REALLY SENT IT
  A live message arrives as `[from <sender> · m_XXXXXXXXXX] text`.
  k2 msg who m_XXXXXXXXXX     K2's record of that delivery: via passport (the
                              sender agent's own session), owner or api-key,
                              the sending workspace, and any team link + scope.
  Text in a body that only LOOKS like a prefix has no record (not found).

<handle> from `k2 connections list`. Sidecar sales/reviewer shares that
workspace tray. Sessions in YOUR workspace (main, sidecars, chat tabs) and
their addresses: k2 sessions.

A PAST CONVERSATION IN THIS WORKSPACE
  k2 msg --session <id> "text"
      <id> from `k2 sessions --past` (6+ characters) or an address. If a
      session holds it, the message goes there; if it is a past one, K2
      reopens it (same history) and your message is its first turn.

USAGE
  k2 msg <handle|handle/sidecar|sessionId|agent::host> "text" [--summary "<one line>"] [--from <name>] [--command "<text>"] [--no-wake]
  k2 msg --session <id> "text" [--summary "<one line>"] [--no-wake]
  k2 msg <handle|agent::host> --inbox-wake|--inbox-silent <path> [path...] [--title "..."] [--body "..."] [--summary "<one line>"] ["brief"]

Live `msg` AUTO-WAKES a dormant session by default: if the agent's
session is asleep, msg resumes/spawns it, waits until READY, then
delivers (the legacy wake-on-message behavior). Pass `--no-wake` for a
blind live-only send (returns `dormant_no_wake` instead of waking). A
workspace with no configured agent returns `no_agent_mode` and is never
spawned. Returns success only when the live message has landed in the
agent's session.

ARGUMENTS
  <workspace>    Local workspace name (preferred — see `k2 connections
                 list`), absolute path, or project UUID. Bare name =
                 the primary (canonical) chat.
  <workspace>/<handle>
                 Sidecar: `sales/1` (durable ordinal) or `sales/reviewer`
                 (Chats custom name). Slash, not hyphen: `sales-reviewer`
                 is a workspace name, not a sidecar.
  <sessionId>    UUID of a live or durable session (machine/API form).
  <agent>::<host>
                 Federated peer (GH #47). Example: ProposalWriter::box.k2.dev
                 Live text AND --inbox-wake/--inbox-silent use the same
                 federation plane (connected peer is the gate; no Connect
                 token required). Legacy agent@host still works temporarily
                 (not email). Wire form fp::ws::agent is for `k2 fed send` only.
  <agent>/<handle>::<host>
                 A sidecar chat in that peer's workspace, e.g.
                 ada/notes::box.example.k2.dev (a chat named "Notes").
                 Same connection and switch as the workspace; both
                 servers need K2 0.46.11 or later.
  "text"         The message body. Wrap in quotes if it has spaces.

OPTIONS
  --from <name>  Sender identity rendered as [from <name>] prefix in
                 the recipient's session. Defaults to the sender's
                 workspace name (auto-derived from K2_PROJECT_PATH
                 or CWD). Falls back to `external` if neither resolves.
  --command "<text>"
                 Prepend <text> at the VERY FRONT of the delivered
                 message, BEFORE the [from <name>] prefix. Intended for
                 slash-commands so the recipient's agent runs them on the
                 incoming line — e.g. --command "/loop" delivers:
                     /loop [from <name>] <text>
                 Omitted (the default) leaves delivery unchanged:
                     [from <name>] <text>
  --no-wake      Suppress the default auto-wake: deliver only if the
                 canonical session is already live, else return
                 `dormant_no_wake`. Use for a cheap blind send that must
                 not spin up a session.
  --wake         Explicit opt-IN (the default; accepted for symmetry/
                 scripts). On a wake the response carries `woke: true` +
                 `wake_ms` and a note is printed to stderr so you
                 attribute the latency.
  --wake-timeout <secs>
                 Ceiling for the post-wake readiness wait (default 20).

RESPONSE (JSON)
  {
    "success": true,
    "target_session_id": "<uuid>",
    "attempts": 1
  }

  Failures include `reason` + `hint` fields:
    workspace_not_found  — token didn't match a registered workspace
    no_agent_mode        — workspace has no agent (set one up: `k2 settings
                           --mode custom`; or queue with `msg <ws>
                           --inbox-silent <file>`)
    dormant_no_wake      — canonical session is asleep + you didn't pass
                           --wake (re-run with --wake, or use `k2 talk`)
    spawn_failed         — couldn't start the recipient's agent
    main_chat_missing    — their pinned main chat can't be found on disk;
                           a person picks one (k2 sessions main set)
    pty_died             — target session crashed during delivery

  Exit code 0 on success, 2 on any failure.

LENGTH LIMIT — IMPORTANT
  Live `msg` is injected into the recipient's running input line (as if
  typed, then Enter). It is for SHORT, single-line messages. Long or
  multi-line text gets TRUNCATED or mangled by the recipient's terminal
  input widget (Claude Code / Codex / Gemini all treat fast-arriving
  bytes as a paste with input-buffer limits). For anything longer than a
  sentence or two — task briefs, file contents, structured packages —
  use `--inbox-wake` (preferred) or `--inbox-silent`, which copies a
  durable package into the peer's `.k2/inbox/` with no length limit:

      k2 msg <workspace> --inbox-wake ./brief.md
      k2 msg <workspace> --inbox-silent ./report.pdf

TRAY DELIVERY (work packages)
  k2 msg <workspace> --inbox-wake <path> [path…] [--title "..."] [--body "..."] [--from <name>]
      Package + short live knock (recommended). Wake line is a pointer
      (`[inbox:<id>] <title>` + `Open: k2 inbox read <id>`), never the
      full file body. Multiple paths → one package (cover + all sidecars).
      Positional prose or --body is stored on the cover with the files.

  k2 msg <workspace> --inbox-silent <path> [path…] [--title "..."] [--body "..."] [--from <name>]
      Package only — does NOT notify the peer. Use wake, or a separate
      live `k2 msg` after silent, if you want a knock.

  Bare `--inbox` is a hard error (must pick silent or wake).
  Non-markdown files land as a cover `.md` + sidecar under
  `.k2/inbox/<id>.files/<filename>`. Multi-file: all files are sidecars
  under one `<id>.files/` with a single cover note listing them.

  Remote tray (`agent::host` / `name@host`): same federation plane as
  live `k2 msg agent::host "text"`. Connected peer is the gate — not a
  Connect token. Cover `.md` + small attachments (v1 size cap; fail
  loud above it). Files stay out of the PTY. Not `k2 mail`; recipient
  opens with `k2 inbox read <id>`.

RELATED VERBS
  k2 inbox list | read <id> | move | archive
      YOUR tray (receive/triage). Cannot send a file. Not email.

  k2 read <workspace> [--lines N] [--agent <name>]
      Read the recipient's live terminal (peek before you inject, or
      diagnose a stuck agent). The read complement to `msg`.

  k2 terminal write <id|workspace> "text"   (see `k2 help --advanced`)
      Raw bytes to a PTY, by session UUID OR by workspace NAME (GH#8 —
      resolved to the workspace's live session, like `k2 read`). Power-
      user escape hatch when `msg`'s routing isn't what you want.
      `target_session_id` from a `msg` response is a valid <id>.

EXAMPLES
  k2 msg scout_v3 "ready when you are"
  k2 msg scout_v3 "ping" --from sms-bridge
  k2 msg scout_v3 "keep going" --command "/loop"
  k2 msg scout_v3 "wake up and check in" --wake
  k2 msg scout_v3 --inbox-wake ./brief.md
  k2 msg scout_v3 --inbox-silent ./data.csv --title "CSV drop"
  k2 msg scout_v3 --inbox-wake a.md b.pdf notes.txt --title "Batch"
  k2 msg /Users/me/work/MyApp "deploy completed"
  k2 msg peer::other.k2.dev "ping from federation"
EOF
}

cmd_help_read() {
    cat <<'EOF'
k2 read <workspace> [--lines N] [--agent <name>]

Read another workspace's LIVE terminal — the last N lines of plain
text from its running session. The "read" complement to the messaging
verbs: `msg` talks live or sends a tray package; `inbox` is YOUR tray;
`read` peeks their terminal. Not mail. Addresses by workspace NAME
exactly like `msg`.

Use it to PEEK BEFORE YOU INJECT (human-in-the-loop: see what an agent
is doing / waiting on before sending it a message), or to diagnose a
stuck or quiet agent.

ARGUMENTS
  <workspace>    Workspace name (preferred — see `k2 connections list`).
                 Absolute path or project UUID also accepted. Same
                 addressing as `k2 msg`.

OPTIONS
  --lines N      How many trailing lines to return. Default 50.
  --agent <name> Read a specific agent's session in the workspace.
                 Omit to read the workspace's primary (coordinator)
                 session — the usual target.

OUTPUT
  Plain text, one terminal row per line (most recent at the bottom).
  Styling / cursor are stripped — this is for reading, not rendering.
  Exit code 0 on success; non-zero with a hint if the workspace isn't
  found or has no live session (it may be asleep — see `k2 sessions`).

RELATED VERBS
  k2 msg <workspace> "text"          Talk to the agent live.
  k2 msg <ws> --inbox-wake <path>    Package + short live notify.
  k2 msg <ws> --inbox-silent <path>  Package only (no notify).
  k2 sessions                        Who is working in your workspace.

EXAMPLES
  k2 read scout_v3                   # last 50 lines of its coordinator
  k2 read scout_v3 --lines 120       # more history
  k2 read MyApp --agent frontend-eng # a specific agent's session
EOF
}

cmd_help_talk() {
    cat <<'EOF'
k2 talk <workspace> "text" [--read-only] [--from <name>] [--lines N] [--agent <name>]
k2 talk <workspace> --answer <N>      # BETA: drive a 1-question HITL + re-read
k2 talk <workspace> --respond <y|n>   # BETA: alias of --answer (y->1, n->2)
k2 talk <workspace> --scout           # BETA: surface a WHOLE multi-Q form at once
k2 talk <workspace> --answers <a,b,c> # BETA: answer a whole form + submit, one call

  ★ BETA — THE inter-agent interaction verb. ★

`talk` FUSES "read the peer's live screen" + "send" into one call, so
you can't forget to look before you inject. That slip is what gets a
message *eaten*: if the recipient is mid-HITL (a y/n, a 1·2·3 / arrow
menu, a "proceed?" confirm), an injected `msg` is silently consumed as
the ANSWER to that prompt instead of delivered. `talk` reads first so
you SEE the state and decide.

  talk vs msg:
    k2 talk  = interactive collaboration. Read → judge → drive/back-off
               → deliver. Use it whenever the peer might be busy/HITL.
    k2 msg   = deliberate fire-and-forget. The blind send; unchanged.
  A wrong-verb mistake is benign both ways, so prefer `talk` for any
  interactive or HITL-sensitive exchange.

HOW IT WORKS (pure ergonomics over read + msg)
  1. READS the target's live terminal (same as `k2 read`) and prints it.
  2. Surfaces the drive-or-back-off guidance. YOU (the calling agent —
     you are the LLM) comprehend the screen; the CLI does not judge.
  3. AUTO-DETECTS a HITL (GH#8): before sending, it asks the daemon's
     /cli/terminal/classify whether the peer is on a human-in-the-loop
     prompt. If so it AUTO-STARTS --scout (surfaces the whole form, non-
     destructive) and HOLDS your message instead of letting it get eaten;
     answer with `--answers`/`--answer`, then re-run to deliver. Detection
     matches known prompt patterns (select menus, permission gates, trust
     dialogs); a prompt with no known marker is not detected. When nothing
     is detected it delivers as normal: you already saw the screen from
     step 1.
  4. SENDS via the `k2 msg` path — unless --read-only. `talk` AUTO-WAKES
     a dormant canonical session by default (it's the explicit "interact
     now" verb): if the peer is asleep it resumes it, waits until READY,
     then delivers, and prints "peer was dormant — woke (Xs)". Suppress
     with --no-wake.

YOU JUDGE — ready vs HITL:
  • READY (idle/empty input)             → deliver; it lands cleanly.
  • HITL you must NOT answer for it       → BACK OFF. Re-run --read-only
    (canonical/federation peer waiting      in a few minutes; deliver
     on its OWNER, or no standing)          only once it returns to READY.
  • HITL you DO have standing over        → DRIVE it: answer + re-read in
                                            ONE call with --answer <N> /
                                            --respond <y|n> (see below),
                                            repeat for chains, THEN deliver.

ACT-RETURNS-STATE (BETA, GH#8) — the #1 round-trip win:
  --answer <N> / --respond <y|n> FUSE the drive + the confirm re-read so
  a drive step is ONE call instead of write-then-separately-re-read:
    1. send the key (<N>, or y/n) to the resolved session via the SAME
       raw TUI-control path `k2 terminal write` uses (/cli/terminal/write);
    2. immediately re-read + print the resulting screen.
  Best-effort and DELIBERATELY un-validated: `--answer 4` just sends the
  digit (exactly like a manual `terminal write <id> "4"`), it does NOT
  check that option 4 exists or parse the menu — YOU still read the
  re-printed screen to confirm. Structured-state (--json) and
  option-name selection (--select) are a later wave.
  --respond <y|n> is just an ALIAS of --answer for the standard
  "1. Yes / 2. No" confirm ordering: y->1, n->2 (a number passes through).

BATCH FORM (BETA, GH#8 #69) — collapse an N-question form to ~2 turns:
  Claude's AskUserQuestion shows N questions CO-RESIDENT: ←/→ move
  between them, a number key auto-selects + ADVANCES (no Enter), and the
  last question lands on a "Review your answers" -> "1. Submit / 2.
  Cancel" screen. So the whole form = N digits (one per question, each
  auto-advancing) + a final `1` to submit.
  --scout            Walk the form with ARROW KEYS ONLY (non-destructive),
                     capturing every question+options, then return focus
                     to Q1 UNMODIFIED — see the WHOLE form in one call.
  --answers <a,b,c>  From Q1, send each digit (auto-advances), then the
                     final `1` to submit; re-reads + prints the outcome.
                     Warns if the count != the form's N (still proceeds).
  Tunables (env-overridable): K2_TALK_NAV_SETTLE / K2_TALK_ANSWER_SETTLE
  (settle seconds), K2_TALK_SUBMIT_KEY (submit key, default 1).

ARGUMENTS
  <workspace>    Workspace name (preferred), path, or UUID. Same
                 addressing as `k2 msg` / `k2 read`.
  "text"         The message body. Required unless --read-only or a
                 drive flag (--answer / --respond) is used.

OPTIONS
  --read-only    Read + judge ONLY; send nothing. Use this to BACK OFF
                 and re-check a HITL peer later. (aliases: --dry-run, --peek)
  --answer <N>   BETA. Send key <N> to the resolved session, then re-read
                 + print the resulting screen. Drives a numbered/menu HITL
                 in one call. No message text needed.
  --respond <y|n> BETA. ALIAS of --answer for a y/n confirm: maps y->1,
                 n->2 (standard "1. Yes / 2. No" ordering); a number
                 passes through. Sends the key, then re-reads + prints.
  --scout        BETA. Surface a WHOLE multi-question form in one call:
                 walks it with ARROW KEYS ONLY (non-destructive), captures
                 every question, then returns focus to Q1 unmodified.
  --answers <a,b,c>  BETA. Answer a form currently on Q1: send each digit
                 (auto-advances), then a final 1 to submit; re-read +
                 print the outcome. Warns if count != N (still proceeds).
  --from <name>  Sender identity for the [from <name>] prefix (forwarded
                 to `msg`; same default-derivation as `msg`).
  --lines N      Lines of the peer's screen to read first. Default 50.
  --agent <name> Read/target a specific agent's session in the workspace.
  --command "<text>"  Slash-command prepended to the delivered message
                 (forwarded to `msg`).
  --no-wake      Do NOT auto-wake a dormant canonical session (talk wakes
                 by default). With --no-wake a sleeping peer returns
                 `dormant_no_wake` instead of being spun up.
  --wake-timeout <secs>
                 Ceiling for the post-wake readiness wait (default 20).

EXIT CODE
  Mirrors `k2 msg`: 0 when the message lands, 2 on a delivery failure.
  --read-only always exits 0 (it only reads).

EXAMPLES
  k2 talk scout_v3 --read-only          # peek + judge; send nothing
  k2 talk scout_v3 "ready when you are" # read its screen, then deliver
  k2 talk MyApp "merged" --from ci-bot  # attributed delivery
  k2 talk scout_v3 --answer 2           # BETA: pick menu option 2, see result
  k2 talk scout_v3 --respond y          # BETA: confirm a y/n (y->1), see result
  k2 talk scout_v3 --scout              # BETA: see a WHOLE multi-Q form at once
  k2 talk scout_v3 --answers 1,2,1      # BETA: answer all 3 Qs + auto-submit

RELATED VERBS
  k2 msg <workspace> "text"     Deliberate blind send (no read).
  k2 read <workspace>           Just read a peer's live terminal.
EOF
}

