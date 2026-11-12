# ── Retired by Garden access (0.46.3; k2 study garden-changes) ─────────
# A retired verb answers with a notice through 0.46.x and is removed in
# 0.47.0. Text mode: one line on stderr. --json: on stdout, either the
# answer of what it now does with a "retired" object added (exit as that
# answer), or, when it no longer does anything, only
# {"error":"retired","verb","replacement","removedIn","see"} (exit 1).
_ZEN_RETIRED_IN="0.47.0"
_ZEN_RETIRED_SEE="k2 study garden-changes"

# Is this daemon answer the 410 `retired` refusal?
_zen_is_retired() {
    ZEN_RAW="$1" python3 -c '
import json, os, sys
try:
    d = json.loads(os.environ.get("ZEN_RAW") or "")
except Exception:
    sys.exit(1)
e = d.get("error") if isinstance(d, dict) else None
code = e.get("code") if isinstance(e, dict) else e
sys.exit(0 if code == "retired" else 1)'
}

# _zen_retired_notice <verb> <replacement>: the text-mode line, on stderr.
_zen_retired_notice() {
    echo "Notice: \`$1\` is retired (Gardens use access lists since 0.46.3) and is removed in ${_ZEN_RETIRED_IN}. Use: $2. See \`${_ZEN_RETIRED_SEE}\`." >&2
}

# _zen_retired_tag <raw> <verb> <replacement>: the answer (a JSON object)
# with "retired": {verb, replacement, removedIn, see}; anything else as is.
_zen_retired_tag() {
    ZEN_RAW="$1" ZV="$2" ZR="$3" ZI="$_ZEN_RETIRED_IN" ZS="$_ZEN_RETIRED_SEE" python3 -c '
import json, os
raw = os.environ.get("ZEN_RAW") or ""
try:
    d = json.loads(raw)
except Exception:
    d = None
if isinstance(d, dict):
    d["retired"] = {"verb": os.environ["ZV"], "replacement": os.environ["ZR"], "removedIn": os.environ["ZI"], "see": os.environ["ZS"]}
    print(json.dumps(d))
else:
    print(raw)'
}

# _zen_retired_refuse <as_json> <verb> <replacement>: the verb no longer
# does anything. --json: the retired object alone on stdout; text: the
# notice on stderr. Exit 1.
_zen_retired_refuse() {
    if [ "$1" = true ]; then
        ZV="$2" ZR="$3" ZI="$_ZEN_RETIRED_IN" ZS="$_ZEN_RETIRED_SEE" python3 -c '
import json, os
print(json.dumps({"error": "retired", "verb": os.environ["ZV"], "replacement": os.environ["ZR"], "removedIn": os.environ["ZI"], "see": os.environ["ZS"]}, separators=(",", ":")))'
    else
        _zen_retired_notice "$2" "$3"
    fi
    exit 1
}

# Garden access (0.46.3): once per workspace, the first `k2 zen …` prints
# one line on stderr pointing at k2 study garden-changes. Text mode only
# (never with --json, never on stdout). The marker is that workspace's own
# .k2/.garden-changes-hint (like .thread-md-nudge), never ~/.k2. Only in
# the workspace K2 names (K2_PROJECT_PATH, set in every K2 terminal) when
# it has its .k2 folder: a plain shell, ~ and a test sandbox without one
# never see it. Best effort: no marker written, no line.
_zen_garden_changes_hint() {
    local a root dot
    for a in "$@"; do
        case "$a" in --json|--json=*) return 0 ;; esac
    done
    root="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-}}"
    [ -n "$root" ] && [ "$root" != "$HOME" ] || return 0
    dot="$(_ws_dot_dir "$root")"
    [ -d "$dot" ] || return 0
    if [ -d "$K2_HOME" ] && [ "$(cd "$dot" && pwd -P)" = "$(cd "$K2_HOME" && pwd -P)" ]; then
        return 0
    fi
    [ ! -e "$dot/.garden-changes-hint" ] || return 0
    printf '0.46.3\n' > "$dot/.garden-changes-hint" 2>/dev/null || return 0
    echo 'Gardens changed in 0.46.3: see `k2 study garden-changes` (update before 0.47.0)' >&2
}

# k2 zen proposals [--all] | show <id> | approve <id> | reject <id>
# Retired by Garden access (0.46.3): nothing new is proposed. On a K2 with
# access lists the pending rows drain through 0.46.x (list, show, the
# person's approve, anyone's reject of their own) with the notice; an
# agent's approve answers `retired` and does nothing. A K2 older than
# 0.46.3 keeps proposals as they were (no notice).
cmd_zen_proposals() {
    local as_json=false all=false
    local -a pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --all) all=true; shift ;;
            -h|--help) cmd_help_zen; exit 0 ;;
            -*) echo "Error: unexpected '$1' (see k2 zen --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local verb="${pos[0]:-list}" raw body view
    case "$verb" in
        list|show|approve|reject) ;;
        *) echo "Error: unknown 'k2 zen proposals' command '$verb' (show, approve, reject)" >&2; exit 2 ;;
    esac
    case "$verb" in
        list)
            view=list
            if [ "$all" = true ]; then raw=$(cli_request "/cli/zen/proposals" "all=1"); else raw=$(cli_request "/cli/zen/proposals"); fi ;;
        show)
            [ -n "${pos[1]:-}" ] || { echo "Error: k2 zen proposals show <id>" >&2; exit 2; }
            view=show
            raw=$(cli_request "/cli/zen/proposals/show" "id=$(urlencode "${pos[1]}")") ;;
        approve|reject)
            [ -n "${pos[1]:-}" ] || { echo "Error: k2 zen proposals $verb <id>" >&2; exit 2; }
            view=decide
            body=$(ZI="${pos[1]}" python3 -c 'import json, os; print(json.dumps({"id": os.environ["ZI"]}))')
            # Inside K2: the passport (an agent may withdraw its own with
            # reject; approve is the person's). Outside: the person.
            raw=$(_zen_session_post "/cli/zen/proposals/$verb" "$body") ;;
    esac
    local name="k2 zen proposals" repl="k2 zen garden access <garden> (edit when you're on its list, else ask)"
    [ "$verb" = list ] || name="k2 zen proposals $verb"
    if _zen_is_retired "$raw"; then
        _zen_retired_refuse "$as_json" "$name" "$repl"
    fi
    if _zen_access_on; then
        if [ "$as_json" = true ]; then
            raw=$(_zen_retired_tag "$raw" "$name" "$repl")
        else
            _zen_retired_notice "$name" "$repl"
        fi
    fi
    _zen_agent_print "$view" "$as_json" "$raw"
}

cmd_zen() {
    local sub="${1:-}"
    shift || true
    local raw=""
    case "$sub" in
        validate|history)
            _zen_args "$@"
            _zen_validate_only_flags "$sub"
            [ -z "$ZEN_TO" ] || { echo "Error: --to is for k2 zen reset" >&2; exit 2; }
            if [ "$ZEN_NO_SIGN" = true ] && { [ "$sub" != "validate" ] || [ -z "$ZEN_WIDGET_SEL" ]; }; then
                echo "Error: --no-sign is for k2 zen validate --widget" >&2
                exit 2
            fi
            if [ "$ZEN_SIGN" = true ] && [ -z "$ZEN_WIDGET_SEL" ]; then
                echo "Error: --sign is for k2 zen validate --widget <name>" >&2
                exit 2
            fi
            if [ "$ZEN_SIGN" = true ] && [ "$ZEN_NO_SIGN" = true ]; then
                echo "Error: pass --sign or --no-sign, not both" >&2
                exit 2
            fi
            if [ -n "$ZEN_TYPES" ] && { [ "$sub" != "validate" ] || [ -z "$ZEN_WIDGET_SEL" ]; }; then
                echo "Error: --types is for k2 zen validate --widget <name>" >&2
                exit 2
            fi
            if [ -n "$ZEN_AS" ] && [ -z "$ZEN_DIR_SEL" ]; then
                echo "Error: --as names a staged folder: k2 zen validate --dir <folder> --as <name>" >&2
                exit 2
            fi
            if [ -n "$ZEN_FROM" ] && [ -z "$ZEN_GARDEN_SEL" ]; then
                echo "Error: --from checks a staged Garden page: k2 zen validate --garden <name|id> --from <file>|-" >&2
                exit 2
            fi
            export ZEN_STRICT
            # A staged widget folder (0.46.1 S3): checked by the daemon,
            # nothing written, Zen needn't be set up. Never pending.
            if [ -n "$ZEN_DIR_SEL" ]; then
                local extra
                extra=$(ZA="$ZEN_AS" python3 -c 'import json, os; b = {}
if os.environ.get("ZA"): b["widget"] = os.environ["ZA"]
print(json.dumps(b))')
                raw=$(_zen_dir_call "/cli/zen/widget/folder/check" "$ZEN_DIR_SEL" "$extra" fallback) || exit $?
                _zen_print validate "$ZEN_AS_JSON" "$raw"
                return
            fi
            # A staged Garden page: checked against that Garden, never written.
            if [ -n "$ZEN_FROM" ]; then
                local body
                body="$(_zen_read_text "$ZEN_FROM" | ZG="$ZEN_GARDEN_SEL" python3 -c '
import json, os, sys
print(json.dumps({"garden": os.environ["ZG"], "text": sys.stdin.read()}))')"
                raw=$(_zen_session_post "/cli/zen/garden/check" "$body")
                _zen_print validate "$ZEN_AS_JSON" "$raw"
                return
            fi
            local -a q=()
            [ -n "$ZEN_GARDEN_SEL" ] && q+=("garden=$(urlencode "$ZEN_GARDEN_SEL")")
            [ -n "$ZEN_FILE_SEL" ] && q+=("file=$(urlencode "$ZEN_FILE_SEL")")
            [ -n "$ZEN_THEME_SEL" ] && q+=("theme=$(urlencode "$ZEN_THEME_SEL")")
            [ -n "$ZEN_WIDGET_SEL" ] && q+=("widget=$(urlencode "$ZEN_WIDGET_SEL")")
            [ -n "$ZEN_TYPES" ] && q+=("types=$ZEN_TYPES")
            raw=$(cli_request "/cli/zen/$sub" ${q[@]+"${q[@]}"})
            # Garden data (contract-garden-data-v1 §8): `validate --widget
            # --sign` then signs a clean version as the caller (a second call;
            # validate stays a read). 0.46.1: a check never signs by default;
            # --no-sign is kept as a no-op. k2 zen widget sign does it alone.
            local signed=""
            if [ "$sub" = "validate" ] && [ -n "$ZEN_WIDGET_SEL" ] && [ "$ZEN_SIGN" = true ] \
                && [ "$(ZEN_RAW="$raw" python3 -c 'import json, os
try:
    d = json.loads(os.environ.get("ZEN_RAW") or "")
except Exception:
    d = {}
print("yes" if isinstance(d, dict) and d.get("ok") is True and d.get("error") is None and not d.get("locked") else "no")')" = "yes" ]; then
                signed=$(_zen_session_post "/cli/zen/widget/sign" \
                    "$(ZW="$ZEN_WIDGET_SEL" python3 -c 'import json, os; print(json.dumps({"widget": os.environ["ZW"]}))')")
                if [ "$ZEN_AS_JSON" = true ]; then
                    raw=$(ZEN_RAW="$raw" ZEN_SIGNED="$signed" python3 -c 'import json, os
d = json.loads(os.environ["ZEN_RAW"])
try:
    d["signed"] = json.loads(os.environ.get("ZEN_SIGNED") or "null")
except Exception:
    d["signed"] = None
print(json.dumps(d))')
                fi
            fi
            _zen_print "$sub" "$ZEN_AS_JSON" "$raw"
            if [ -n "$signed" ] && [ "$ZEN_AS_JSON" != true ]; then
                _zen_sign_print sign false "$signed" || echo "(validate was clean; signing failed, so its data calls keep the version's old maker)" >&2
            fi
            ;;
        inspect)
            cmd_zen_inspect "$@"
            ;;
        reload)
            _zen_args "$@"; _zen_validate_only_flags "$sub"
            if [ -n "$ZEN_GARDEN_SEL$ZEN_FILE_SEL$ZEN_TO$ZEN_THEME_SEL$ZEN_WIDGET_SEL" ]; then
                echo "Error: k2 zen reload takes only --json" >&2
                exit 2
            fi
            raw=$(cli_post_json "/cli/zen/reload" '{}')
            _zen_print reload "$ZEN_AS_JSON" "$raw"
            ;;
        reset)
            _zen_args "$@"; _zen_validate_only_flags "$sub"
            local body
            body=$(ZG="$ZEN_GARDEN_SEL" ZF="$ZEN_FILE_SEL" ZT="$ZEN_TO" ZM="$ZEN_THEME_SEL" ZW="$ZEN_WIDGET_SEL" python3 -c '
import json, os
b = {}
for k, e in (("garden", "ZG"), ("file", "ZF"), ("to", "ZT"), ("theme", "ZM"), ("widget", "ZW")):
    if os.environ.get(e):
        b[k] = os.environ[e]
print(json.dumps(b))
')
            raw=$(_zen_session_post "/cli/zen/reset" "$body")
            _zen_print reset "$ZEN_AS_JSON" "$raw"
            ;;
        doctor|path)
            _zen_args "$@"; _zen_validate_only_flags "$sub"
            if [ -n "$ZEN_GARDEN_SEL$ZEN_FILE_SEL$ZEN_TO$ZEN_THEME_SEL$ZEN_WIDGET_SEL" ]; then
                echo "Error: k2 zen $sub takes only --json" >&2
                exit 2
            fi
            if [ "$sub" = "doctor" ]; then
                raw=$(cli_request "/cli/zen/doctor")
            else
                raw=$(cli_request "/cli/zen/status")
            fi
            _zen_print "$sub" "$ZEN_AS_JSON" "$raw"
            ;;
        garden|gardens)
            cmd_zen_garden "$@"
            ;;
        theme|themes)
            cmd_zen_theme "$@"
            ;;
        widget|widgets)
            cmd_zen_widget "$@"
            ;;
        sync)
            cmd_zen_sync "$@"
            ;;
        news)
            cmd_zen_news "$@"
            ;;
        links)
            cmd_zen_links "$@"
            ;;
        agents-order)
            cmd_zen_agents_order "$@"
            ;;
        proposals|proposal)
            cmd_zen_proposals "$@"
            ;;
        pages)
            echo "Error: there is no 'k2 zen pages' any more: Zen pages are Gardens now. Use k2 zen garden list." >&2
            exit 2
            ;;
        ""|help|--help|-h)
            cmd_help_zen
            exit 0
            ;;
        guide)
            cmd_zen_guide "$@"
            ;;
        # Garden data admin (contract-garden-data-v1 §8): the person only.
        data)
            cmd_zen_data "$@"
            ;;
        grant|grants|allow)
            echo "Error: there is no 'k2 zen $sub'. Widgets in your own Gardens need no permissions:" >&2
            echo "they can talk to your agents right away." >&2
            exit 2
            ;;
        *)
            echo "Unknown 'k2 zen' subcommand: $sub" >&2
            cmd_help_zen >&2
            exit 2
            ;;
    esac
}

