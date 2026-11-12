# ── W6 (0.40.30): `k2 preset` — agent-preset management ──────────────
#
# Thin client over the daemon's `/cli/presets/*` routes (list/get are
# GETs; create/update/delete are owner-or-admin POSTs). A preset row is
# what `hire --agent` / Settings resolve into a launchable agent:
# command string + the migration-0070 metadata (danger_flags = the
# agent's OWN auto-approve flags, env = spawn-time environment,
# readiness = injection timing class). Selector tolerance mirrors
# `workspace::agent_resolve`: id exact → label (case-insensitive) →
# command first-token; ambiguity is a loud exit 2.
#
# Contract (same as `k2 project`): success output human-readable,
# --json flips to JSON; errors ALWAYS JSON on stderr
# ({"error":{"code","hint"}}); exit 0 ok · 1 refused/failed · 2 bad
# usage · 4 selector unresolvable.

_preset_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}},
                 ensure_ascii=False, separators=(",", ":")))' >&2 \
        || echo "Error ($code): $hint" >&2
}

# Shared HTTP driver for every preset verb. Inputs ride PR_* variables
# (bash dynamic scope from the calling cmd_preset_*), re-exported onto
# the python3 invocation.
_preset_py() {
    ABASE="$BASE_URL" ATOKEN="$TOKEN" \
    PR_VERB="${PR_VERB:-}" PR_JSON="${PR_JSON:-false}" \
    PR_SELECTOR="${PR_SELECTOR:-}" PR_ID="${PR_ID:-}" \
    PR_LABEL="${PR_LABEL:-}" PR_COMMAND="${PR_COMMAND:-}" \
    PR_ICON="${PR_ICON:-}" PR_READINESS="${PR_READINESS:-}" \
    PR_DANGER="${PR_DANGER:-}" PR_ENV_KV="${PR_ENV_KV:-}" \
    PR_ENABLE="${PR_ENABLE:-}" PR_CLEAR_DANGER="${PR_CLEAR_DANGER:-false}" \
    PR_CLEAR_ENV="${PR_CLEAR_ENV:-false}" PR_CLEAR_READY="${PR_CLEAR_READY:-false}" \
    PR_INJECT_FLOW="${PR_INJECT_FLOW:-}" PR_CLEAR_INJECT="${PR_CLEAR_INJECT:-false}" \
    python3 - <<'PYEOF'
import json, os, sys
import urllib.error, urllib.parse, urllib.request

for _s in (sys.stdout, sys.stderr):
    try:
        _s.reconfigure(encoding="utf-8")
    except Exception:
        pass

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
verb = os.environ["PR_VERB"]
as_json = os.environ.get("PR_JSON") == "true"

def dumps(obj):
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))

def die(code, hint, exit_code=None):
    sys.stderr.write(dumps({"error": {"code": code, "hint": hint}}) + "\n")
    if exit_code is None:
        exit_code = 4 if code in ("not_found", "ambiguous") else 2 if code == "usage" else 1
    sys.exit(exit_code)

def request(method, route, params=None, body=None):
    qs = urllib.parse.urlencode(dict(params or {}, token=token))
    url = base + route + "?" + qs
    data = dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return e.code, {"error": "HTTP %s on %s" % (e.code, route)}
    except Exception:
        return None, None

def call(method, route, params=None, body=None):
    status, d = request(method, route, params, body)
    if d is None:
        die("daemon_unreachable", "no response from the K2 daemon (is it running?)")
    if isinstance(d, dict) and d.get("error"):
        err = d["error"]
        hint = err if isinstance(err, str) else err.get("hint", dumps(err))
        if status == 403:
            die("forbidden", "this verb needs the owner token or an Owner/Admin session")
        die("daemon_error", hint)
    return d

def fetch_presets():
    d = call("GET", "/cli/presets/list")
    if not isinstance(d, list):
        die("daemon_error", "unexpected /cli/presets/list response: %s" % dumps(d)[:200])
    return d

def first_token(p):
    parts = (p.get("command") or "").split()
    return parts[0] if parts else ""

def resolve(selector):
    presets = fetch_presets()
    want = selector.strip()
    by_id = [p for p in presets if p.get("id") == want]
    if by_id:
        return by_id[0]
    by_label = [p for p in presets if (p.get("label") or "").lower() == want.lower()]
    if len(by_label) == 1:
        return by_label[0]
    if len(by_label) > 1:
        die("ambiguous", "label '%s' matches %d presets: %s — pass the preset id" %
            (want, len(by_label), ", ".join(p.get("id") or "?" for p in by_label)))
    by_tok = [p for p in presets if first_token(p) == want]
    if len(by_tok) == 1:
        return by_tok[0]
    if len(by_tok) > 1:
        die("ambiguous", "command token '%s' matches %d presets: %s — pass the preset id" %
            (want, len(by_tok), ", ".join(p.get("id") or "?" for p in by_tok)))
    die("not_found", "no preset matches '%s' — run `k2 preset list`" % want)

def parse_env_pairs(raw_list):
    env = {}
    for pair in raw_list:
        if "=" not in pair:
            die("usage", "--env expects K=V, got %r" % pair)
        k, _, v = pair.partition("=")
        if not k:
            die("usage", "--env expects a non-empty variable name, got %r" % pair)
        env[k] = v
    return env

def meta_summary(p):
    # Human-readable one-liners for the metadata columns; raw column
    # text is JSON, malformed rows shown as-is (the daemon fail-closes
    # on them at spawn time).
    def parse(raw, default):
        if raw is None:
            return default
        try:
            return json.loads(raw)
        except Exception:
            return "MALFORMED: %s" % raw
    return {
        "dangerFlags": parse(p.get("danger_flags"), []),
        "env": parse(p.get("env"), {}),
        "readiness": p.get("readiness"),
    }

# ── list ─────────────────────────────────────────────────────────
if verb == "list":
    presets = fetch_presets()
    if as_json:
        print(dumps({"ok": True, "presets": presets}))
        sys.exit(0)
    rows = []
    for p in presets:
        cmd = p.get("command") or ""
        if len(cmd) > 44:
            cmd = cmd[:41] + "..."
        meta = []
        if p.get("danger_flags"): meta.append("flags")
        if p.get("env"): meta.append("env")
        if p.get("readiness"): meta.append(p["readiness"])
        rows.append((p.get("id") or "?", p.get("label") or "?", cmd,
                     "yes" if p.get("enabled") else "no",
                     "built-in" if p.get("is_built_in") else "custom",
                     ",".join(meta) or "-"))
    widths = [max(len(r[i]) for r in rows + [("ID", "LABEL", "COMMAND", "ENABLED", "SOURCE", "METADATA")])
              for i in range(6)]
    fmt = "  ".join("%%-%ds" % w for w in widths)
    print(fmt % ("ID", "LABEL", "COMMAND", "ENABLED", "SOURCE", "METADATA"))
    for r in rows:
        print(fmt % r)
    sys.exit(0)

# ── show ─────────────────────────────────────────────────────────
if verb == "show":
    p = resolve(os.environ["PR_SELECTOR"])
    if as_json:
        print(dumps({"ok": True, "preset": p}))
        sys.exit(0)
    meta = meta_summary(p)
    print("id:        %s" % p.get("id"))
    print("label:     %s" % p.get("label"))
    print("command:   %s" % p.get("command"))
    print("enabled:   %s" % ("yes" if p.get("enabled") else "no"))
    print("source:    %s" % ("built-in" if p.get("is_built_in") else "custom"))
    if p.get("icon"):
        print("icon:      %s" % p.get("icon"))
    print("readiness: %s" % (meta["readiness"] or "(unknown — default injection profile)"))
    flags = meta["dangerFlags"]
    print("danger-flags: %s" % (" ".join(flags) if isinstance(flags, list) and flags
                                else flags if isinstance(flags, str)
                                else "(none declared — API host sessions strip only the known floor)"))
    env = meta["env"]
    if isinstance(env, dict) and env:
        print("env:")
        for k in sorted(env):
            print("  %s=%s" % (k, env[k]))
    elif isinstance(env, str):
        print("env:       %s" % env)
    flow = p.get("inject_flow")
    print("inject_flow: %s" % (flow if flow else "(default — paste / return / return)"))
    sys.exit(0)

# ── add / set ────────────────────────────────────────────────────
if verb in ("add", "set"):
    body = {}
    if os.environ.get("PR_LABEL"):
        body["label"] = os.environ["PR_LABEL"]
    if os.environ.get("PR_COMMAND"):
        body["command"] = os.environ["PR_COMMAND"]
    if os.environ.get("PR_ICON"):
        body["icon"] = os.environ["PR_ICON"]
    if os.environ.get("PR_DANGER"):
        body["dangerFlags"] = dumps(json.loads(os.environ["PR_DANGER"]))
    if os.environ.get("PR_ENV_KV"):
        body["env"] = dumps(parse_env_pairs(json.loads(os.environ["PR_ENV_KV"])))
    if os.environ.get("PR_READINESS"):
        body["readiness"] = os.environ["PR_READINESS"]
    if os.environ.get("PR_INJECT_FLOW"):
        try:
            parsed_flow = json.loads(os.environ["PR_INJECT_FLOW"])
        except Exception:
            die("usage", "--inject-flow must be a JSON array of {key, waitMs} steps")
        body["injectFlow"] = dumps(parsed_flow)

    if verb == "add":
        body["id"] = os.environ["PR_ID"]
        body.setdefault("label", body["id"])
        d = call("POST", "/cli/presets/create", body=body)
        if as_json:
            print(dumps({"ok": True, "preset": d}))
        else:
            print("Created preset '%s' (%s)." % (d.get("id"), d.get("command")))
            print("Use it: k2 agent hire <dir> --agent %s   (or Settings → Editors & Agents)" % d.get("id"))
        sys.exit(0)

    # set — resolve first, then apply only what was passed.
    p = resolve(os.environ["PR_SELECTOR"])
    body["id"] = p.get("id")
    if os.environ.get("PR_CLEAR_DANGER") == "true":
        body["dangerFlags"] = ""
    if os.environ.get("PR_CLEAR_ENV") == "true":
        body["env"] = ""
    if os.environ.get("PR_CLEAR_READY") == "true":
        body["readiness"] = ""
    if os.environ.get("PR_CLEAR_INJECT") == "true":
        body["injectFlow"] = ""
    if os.environ.get("PR_ENABLE"):
        body["enabled"] = 1 if os.environ["PR_ENABLE"] == "1" else 0
    if len(body) == 1:
        die("usage", "nothing to set — pass at least one flag (see: k2 preset set --help)")
    d = call("POST", "/cli/presets/update", body=body)
    if as_json:
        print(dumps({"ok": True, "preset": d}))
    else:
        print("Updated preset '%s'." % d.get("id"))
    sys.exit(0)

# ── remove ───────────────────────────────────────────────────────
if verb == "remove":
    p = resolve(os.environ["PR_SELECTOR"])
    if p.get("is_built_in"):
        # Fail client-side with the daemon's own policy wording so the
        # exit code is the refused tier (1), not a generic daemon_error.
        die("refused", "'%s' is a built-in preset — built-ins can't be deleted (disable it: "
            "k2 preset set %s --disable; reset all built-ins via Settings)" %
            (p.get("label"), p.get("id")))
    call("POST", "/cli/presets/delete", body={"id": p.get("id")})
    if as_json:
        print(dumps({"ok": True, "removed": p.get("id")}))
    else:
        print("Removed preset '%s'." % p.get("id"))
    sys.exit(0)

die("usage", "unknown preset verb %r" % verb)
PYEOF
}

cmd_help_preset() {
    cat <<'EOF'
Manage agent PRESETS — the roster of launchable agent programs this
server knows (Claude, Codex, Gemini, ... plus your own). A preset is a
command string plus metadata the daemon uses at spawn time:

  danger-flags   the agent's OWN auto-approve flags (e.g. --yes-always).
                 On API host-session spawns they are KEPT by default
                 (api_skip_permissions ON). Opt out per workspace to strip
                 them (fail-closed).
  env            environment variables staged into the agent's process
                 (e.g. OPENAI_BASE_URL for a local-LLM harness).
  readiness      when prompt injection may fire: 'bracketed-paste'
                 (TUI readiness signal is trustworthy) or 'settle:<ms>'.
  inject-flow    keystroke sequence used to deliver a message into this
                 LLM's terminal (paste|esc|space|return). NULL = default
                 paste / return / return.

USAGE
  k2 preset list [--json]
  k2 preset show <preset> [--json]
  k2 preset add --id <slug> --command '<argv string>' [flags]
  k2 preset set <preset> [flags]
  k2 preset remove <preset> [--json]        (custom presets only)

<preset> = preset id, label (case-insensitive), or command first-token.
Mutations need the owner token or an Owner/Admin session. Built-ins:
editable (incl. metadata), never deletable — disable instead.

Point a workspace at a preset: k2 agent hire <dir> --agent <preset>

EXAMPLES
  k2 preset list
  k2 preset show codex
  k2 preset add --id aider-ollama --command 'aider --model ollama_chat/qwen3' \
      --env OLLAMA_API_BASE=http://localhost:11434 \
      --danger-flag --yes-always --readiness settle:2000
  k2 preset set aider-ollama --readiness settle:3000
  k2 preset remove aider-ollama

See `k2 preset add --help` / `k2 preset set --help` for the full flags,
and docs/agent-contract.md for what a custom agent must do to be a
first-class K2 citizen.
EOF
}

cmd_help_preset_add() {
    cat <<'EOF'
Add a CUSTOM agent preset.

USAGE
  k2 preset add --id <slug> --command '<argv string>' [flags]

FLAGS
  --id <slug>             Preset id (1-64 chars: letters, digits, . _ -).
                          This is the stable handle `hire --agent` and
                          projects.default_agent reference.
  --command '<argv>'      Full launch command string, quote-aware
                          (e.g. 'aider --model ollama_chat/qwen3').
  --label <label>         Display label (default: the id)
  --icon <emoji>          Icon shown in Settings/pickers
  --danger-flag <flag>    Declare ONE of the agent's own auto-approve
                          flags (repeatable). Kept on API host-session
                          spawns by default; stripped if the workspace
                          sets api_skip_permissions off.
  --env K=V               Environment variable staged at spawn
                          (repeatable). Values may hold secrets — they
                          are never logged by the daemon.
  --readiness <v>         'bracketed-paste' | 'settle:<ms>' (1..600000)
  --json                  JSON output

EXAMPLE — local-LLM harness (aider against Ollama):
  k2 preset add --id aider-ollama \
      --command 'aider --model ollama_chat/qwen3 --no-auto-commits' \
      --env OLLAMA_API_BASE=http://localhost:11434 \
      --danger-flag --yes-always \
      --readiness settle:2000
EOF
}

cmd_help_preset_set() {
    cat <<'EOF'
Edit a preset (custom or built-in). Only the flags you pass change.
--danger-flag / --env REPLACE the stored set wholesale (they don't
append) — pass the full desired set.

USAGE
  k2 preset set <preset> [flags]

FLAGS
  --command '<argv>'      Replace the launch command string
  --label <label>         Rename
  --icon <emoji>          Change the icon
  --enable | --disable    Toggle availability (disabled presets are
                          skipped by the default-agent resolver)
  --danger-flag <flag>    Replace the declared auto-approve flags
                          (repeatable)
  --env K=V               Replace the spawn env (repeatable)
  --readiness <v>         'bracketed-paste' | 'settle:<ms>'
  --clear-danger-flags    Clear back to undeclared (fail-closed floor)
  --clear-env             Clear the spawn env
  --clear-readiness       Clear back to the default injection profile
  --inject-flow '<json>'  Replace the submit-keys flow (paste|esc|space|return)
  --clear-inject-flow     Reset submit keys to the default paste/return/return
  --json                  JSON output

EXAMPLES
  k2 preset set aider-ollama --readiness settle:3000
  k2 preset set Gemini --danger-flag --yolo
  k2 preset set my-agent --disable
  k2 preset set grok --inject-flow '[{"key":"esc","waitMs":0},{"key":"space","waitMs":50},{"key":"paste","waitMs":150},{"key":"return","waitMs":250},{"key":"return","waitMs":120}]'
EOF
}

cmd_preset_list() {
    if _wants_help "$@"; then cmd_help_preset; exit 0; fi
    local as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            *)
                _preset_err "usage" "unexpected argument '$1' — usage: k2 preset list [--json]"
                exit 2 ;;
        esac
    done
    local PR_VERB="list" PR_JSON="$as_json"
    _preset_py
}

cmd_preset_show() {
    if _wants_help "$@"; then cmd_help_preset; exit 0; fi
    local selector="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _preset_err "usage" "unknown flag '$1' — usage: k2 preset show <preset> [--json]"
                exit 2 ;;
            *)
                if [ -z "$selector" ]; then selector="$1"; else
                    _preset_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$selector" ]; then
        _preset_err "usage" "show requires a <preset> (id, label, or command token)"
        exit 2
    fi
    local PR_VERB="show" PR_JSON="$as_json" PR_SELECTOR="$selector"
    _preset_py
}

cmd_preset_add() {
    if _wants_help "$@"; then cmd_help_preset_add; exit 0; fi
    local id="" command_str="" label="" icon="" readiness="" as_json=false
    local danger_flags=() env_pairs=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --id)
                [ $# -ge 2 ] || { _preset_err "usage" "--id requires a value"; exit 2; }
                id="$2"; shift 2 ;;
            --command)
                [ $# -ge 2 ] || { _preset_err "usage" "--command requires a value (quote it)"; exit 2; }
                command_str="$2"; shift 2 ;;
            --label)
                [ $# -ge 2 ] || { _preset_err "usage" "--label requires a value"; exit 2; }
                label="$2"; shift 2 ;;
            --icon)
                [ $# -ge 2 ] || { _preset_err "usage" "--icon requires a value"; exit 2; }
                icon="$2"; shift 2 ;;
            --danger-flag)
                [ $# -ge 2 ] || { _preset_err "usage" "--danger-flag requires a flag value (e.g. --danger-flag --yes-always)"; exit 2; }
                danger_flags+=("$2"); shift 2 ;;
            --env)
                [ $# -ge 2 ] || { _preset_err "usage" "--env requires K=V"; exit 2; }
                env_pairs+=("$2"); shift 2 ;;
            --readiness)
                [ $# -ge 2 ] || { _preset_err "usage" "--readiness requires 'bracketed-paste' or 'settle:<ms>'"; exit 2; }
                readiness="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            *)
                _preset_err "usage" "unknown argument '$1' — see: k2 preset add --help"
                exit 2 ;;
        esac
    done
    if [ -z "$id" ] || [ -z "$command_str" ]; then
        _preset_err "usage" "add requires --id <slug> and --command '<argv string>' (see: k2 preset add --help)"
        exit 2
    fi
    local danger_json env_json
    danger_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${danger_flags[@]+"${danger_flags[@]}"})
    env_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${env_pairs[@]+"${env_pairs[@]}"})
    [ "$danger_json" = "[]" ] && danger_json=""
    [ "$env_json" = "[]" ] && env_json=""
    local PR_VERB="add" PR_JSON="$as_json" PR_ID="$id" PR_LABEL="$label" \
          PR_COMMAND="$command_str" PR_ICON="$icon" PR_READINESS="$readiness" \
          PR_DANGER="$danger_json" PR_ENV_KV="$env_json"
    _preset_py
}

cmd_preset_set() {
    if _wants_help "$@"; then cmd_help_preset_set; exit 0; fi
    local selector="" command_str="" label="" icon="" readiness="" enable="" as_json=false
    local clear_danger=false clear_env=false clear_ready=false
    local inject_flow="" clear_inject=false
    local danger_flags=() env_pairs=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --command)
                [ $# -ge 2 ] || { _preset_err "usage" "--command requires a value (quote it)"; exit 2; }
                command_str="$2"; shift 2 ;;
            --label)
                [ $# -ge 2 ] || { _preset_err "usage" "--label requires a value"; exit 2; }
                label="$2"; shift 2 ;;
            --icon)
                [ $# -ge 2 ] || { _preset_err "usage" "--icon requires a value"; exit 2; }
                icon="$2"; shift 2 ;;
            --danger-flag)
                [ $# -ge 2 ] || { _preset_err "usage" "--danger-flag requires a flag value"; exit 2; }
                danger_flags+=("$2"); shift 2 ;;
            --env)
                [ $# -ge 2 ] || { _preset_err "usage" "--env requires K=V"; exit 2; }
                env_pairs+=("$2"); shift 2 ;;
            --readiness)
                [ $# -ge 2 ] || { _preset_err "usage" "--readiness requires 'bracketed-paste' or 'settle:<ms>'"; exit 2; }
                readiness="$2"; shift 2 ;;
            --inject-flow)
                [ $# -ge 2 ] || { _preset_err "usage" "--inject-flow requires a JSON array"; exit 2; }
                inject_flow="$2"; shift 2 ;;
            --enable)  enable="1"; shift ;;
            --disable) enable="0"; shift ;;
            --clear-danger-flags) clear_danger=true; shift ;;
            --clear-env)          clear_env=true; shift ;;
            --clear-readiness)    clear_ready=true; shift ;;
            --clear-inject-flow)  clear_inject=true; shift ;;
            --json) as_json=true; shift ;;
            -*)
                _preset_err "usage" "unknown flag '$1' — see: k2 preset set --help"
                exit 2 ;;
            *)
                if [ -z "$selector" ]; then selector="$1"; else
                    _preset_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$selector" ]; then
        _preset_err "usage" "set requires a <preset> (id, label, or command token)"
        exit 2
    fi
    if { [ ${#danger_flags[@]} -gt 0 ] && $clear_danger; } \
        || { [ ${#env_pairs[@]} -gt 0 ] && $clear_env; } \
        || { [ -n "$readiness" ] && $clear_ready; } \
        || { [ -n "$inject_flow" ] && $clear_inject; }; then
        _preset_err "usage" "a --clear-* flag and its value flag are mutually exclusive"
        exit 2
    fi
    local danger_json env_json
    danger_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${danger_flags[@]+"${danger_flags[@]}"})
    env_json=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' \
        ${env_pairs[@]+"${env_pairs[@]}"})
    [ "$danger_json" = "[]" ] && danger_json=""
    [ "$env_json" = "[]" ] && env_json=""
    local PR_VERB="set" PR_JSON="$as_json" PR_SELECTOR="$selector" \
          PR_LABEL="$label" PR_COMMAND="$command_str" PR_ICON="$icon" \
          PR_READINESS="$readiness" PR_DANGER="$danger_json" PR_ENV_KV="$env_json" \
          PR_ENABLE="$enable" PR_CLEAR_DANGER="$clear_danger" \
          PR_CLEAR_ENV="$clear_env" PR_CLEAR_READY="$clear_ready" \
          PR_INJECT_FLOW="$inject_flow" PR_CLEAR_INJECT="$clear_inject"
    _preset_py
}

cmd_preset_remove() {
    if _wants_help "$@"; then cmd_help_preset; exit 0; fi
    local selector="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            -*)
                _preset_err "usage" "unknown flag '$1' — usage: k2 preset remove <preset> [--json]"
                exit 2 ;;
            *)
                if [ -z "$selector" ]; then selector="$1"; else
                    _preset_err "usage" "unexpected extra argument '$1'"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$selector" ]; then
        _preset_err "usage" "remove requires a <preset> (id, label, or command token)"
        exit 2
    fi
    local PR_VERB="remove" PR_JSON="$as_json" PR_SELECTOR="$selector"
    _preset_py
}

