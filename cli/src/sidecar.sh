# ── k2 sidecar v1 (prd-k2-sidecar-cli-v1) ────────────────────────────
# The daemon owns the job (SC2): POST /cli/sidecar/new|stop, GET
# /cli/sidecar/list, POST /cli/agent-access/set. `sidecar` is an `id`
# tool (SC35): inside a K2 terminal the request carries this session's
# passport (per-cell socket first, else TCP with the scoped token), never
# the owner token.

_sidecar_usage() {
    cat <<'EOF'
%include cli/text/help/sidecar.txt
EOF
}

# _sidecar_send <GET|POST> <path> <json-body|query> — prints the body, then
# a last line with the HTTP status. Per-cell socket first (scoped Bearer),
# else TCP ($TOKEN, already the scoped token inside a cell for id tools).
#
# 0.46.15: the send-once rule of _compute_send. A POST goes again over TCP
# after the cell socket only when the socket took none of it: the connect
# failed (curl 7) or the daemon answered "not served on cell socket". Any
# other failure may have run it, so it is not sent again:
# _cli_send_unanswered says so and ends the command with 1. GETs keep the
# fallback.
_sidecar_send() {
    local method="$1" path="$2" payload="${3:-}"
    local sock out="" _K2_CFG="" rc=0 t0="$SECONDS"
    # The payload (an API key for `k2 llm tokens add-key`) and the token
    # both ride the curl config on stdin — never curl's argv.
    sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds && [ -n "${SCOPED_TOKEN:-}" ]; then
        _K2_CFG=""
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            if [ -n "$payload" ]; then
                _k2_cfg_add data "$payload"
            fi
            out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 120 \
                -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        else
            _k2_cfg_add data-raw "$payload"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 120 \
                -H "Content-Type: application/json" \
                -w $'\n%{http_code}' 2>/dev/null)" || rc=$?
        fi
        if [ "$rc" -eq 0 ] && [ -n "$out" ] && [[ "$out" != *"not served on cell socket"* ]] && [[ "$out" != *$'\n000' ]]; then
            printf '%s' "$out"
            return 0
        fi
        # Soft callers (_K2_NOANSWER_SOFT=1) get status 000 back instead.
        if [ "$method" != "GET" ] && [ "$rc" -ne 7 ] && [[ "$out" != *"not served on cell socket"* ]]; then
            _cli_send_unanswered "$path" "$t0" 0 $'\n000'
            return
        fi
    fi
    # TCP: in a session whose cell socket failed, the passport, never the
    # owner token from disk (_k2_sock_fallback_token).
    local _K2_TCP_TOKEN="${TOKEN:-}"
    _k2_sock_fallback_token "$sock" "$out"
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        if [ -n "$payload" ]; then
            _k2_cfg_add data "$payload"
        fi
        _k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time 120 \
            -w $'\n%{http_code}' 2>/dev/null || true
    else
        _k2_cfg_add url "${BASE_URL}${path}"
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        _k2_cfg_add data-raw "$payload"
        _k2_curl "$_K2_CFG" -s -X POST \
            --connect-timeout 5 --max-time 120 \
            -H "Content-Type: application/json" \
            -w $'\n%{http_code}' 2>/dev/null || true
    fi
}

# Exit code for an HTTP status (k2 study errors; PRD §5.6).
_sidecar_exit_for() {
    case "$1" in
        200) echo 0 ;;
        400|413) echo 2 ;;
        403) echo 3 ;;
        404) echo 4 ;;
        *) echo 1 ;;
    esac
}

# `<body>\n<status>` → _SC_BODY / _SC_STATUS.
_sidecar_split() {
    local raw="${1:-}"
    _SC_STATUS="${raw##*$'\n'}"
    _SC_BODY="${raw%$'\n'*}"
    if [ "$_SC_STATUS" = "$raw" ]; then
        _SC_BODY=""
    fi
    [ -z "$_SC_STATUS" ] && _SC_STATUS="000"
    return 0
}

# Print a refusal (`code: hint`) to stderr and exit with its code.
_sidecar_fail() {
    local status="$1" body="$2"
    local line=""
    if [ -n "$body" ]; then
        line="$(printf '%s' "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    print("daemon_error: " + raw[:200]); sys.exit(0)
e = v.get("error") if isinstance(v, dict) else None
if isinstance(e, dict):
    print("%s: %s" % (e.get("code", "error"), e.get("hint", "")))
elif e:
    print("error: %s" % e)
else:
    print("error: unexpected response")
' 2>/dev/null)"
    fi
    [ -z "$line" ] && line="daemon_unreachable: no answer from the K2 daemon (status ${status:-none})"
    echo "$line" >&2
    # Permission cards on: the refusal's closing "Ask with: …" line.
    _permission_note "$body"
    exit "$(_sidecar_exit_for "$status")"
}

# Render one verb's `<body>\n<status>` answer, then exit.
_sidecar_render() {
    local verb="$1" as_json="${2:-0}"
    _sidecar_split "${3:-}"
    if [ "$_SC_STATUS" != "200" ]; then
        _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$_SC_BODY"
        exit 0
    fi
    case "$verb" in
        new)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v["address"])
what = "resumed" if v.get("resumed") else "opened"
brief = v.get("brief") or {}
extra = (" with brief %s (%d bytes)" % (brief.get("path"), brief.get("bytes", 0))) if brief else ""
wt = v.get("worktree") or {}
if wt:
    extra += " in %s on branch %s" % (wt.get("dir"), wt.get("branch"))
    if wt.get("base"):
        extra += " (from %s %s)" % (wt.get("base"), (wt.get("baseSha") or "")[:10])
sys.stderr.write("%s %s (%s)%s. Message it: k2 msg %s \"...\"\n" % (what, v["address"], v.get("harness"), extra, v["address"]))
'
            ;;
        stop)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(("stopped %s" % v["address"]) if v.get("stopped") else ("%s was already asleep" % v["address"]))
'
            ;;
        archive)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("address"):
    print("archived %s. Its conversation stays: k2 sidecar new %s --harness <the same harness> resumes it." % (v["address"], v["address"].split("/")[-1]))
    sys.exit(0)
n = v.get("count", 0)
print("archived %d stopped sidecar%s%s" % (n, "" if n == 1 else "s", (": " + ", ".join(v.get("archived") or [])) if n else ""))
for s in v.get("skipped") or []:
    print("not archived %s: %s" % (s.get("address"), s.get("hint")))
'
            ;;
        list)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("sidecars", [])
print("%s: %d live of %d (cap %d); its agent may open sidecars: %s" % (v.get("workspace"), v.get("live", 0), len(rows), v.get("cap", 0), "on" if v.get("agentsCanManage") else "off"))
if not rows:
    print("(no sidecars)"); sys.exit(0)
fmt = "%-28s %-20s %-8s %-7s %-14s %-16s %s"
print(fmt % ("ADDRESS", "NAME", "HARNESS", "STATE", "BY", "SINCE", "TASK"))
for r in rows:
    since = r.get("lastSeenAt") or 0
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(since)) if since else "-"
    wt = r.get("worktree") or {}
    task = "-"
    if wt:
        task = "%s %s" % (r.get("taskState") or "-", wt.get("branch") or "")
        if wt.get("ahead"):
            task += " +%d" % wt.get("ahead")
        if wt.get("dirty"):
            task += " (uncommitted)"
    print(fmt % (r.get("address") or "-", (r.get("name") or "-")[:20], r.get("harness") or "-",
                 r.get("state") or "-", (r.get("createdBy") or "app")[:14], when, task))
w = v.get("worktrees") or {}
if w.get("open"):
    print("worktree sidecars: %d of %d (retire one to free a slot)" % (w.get("open", 0), w.get("cap", 0)))
'
            ;;
        access)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Let this agent manage its sidecars: %s for %s" % ("on" if v.get("agentsCanManage") else "off", v.get("workspace")))
'
            ;;
        interrupt)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("interrupted"):
    print("interrupted %s (sent %s); it keeps running" % (v["address"], v.get("key")))
else:
    print("%s is asleep; nothing to interrupt" % v["address"])
'
            ;;
        review)
            _sidecar_print_review "$_SC_BODY"
            ;;
        land)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
n = v.get("count", 0)
print("landed %d commit%s from %s on %s (%s..%s)" % (n, "" if n == 1 else "s", v["address"], v.get("onto"), (v.get("from") or "")[:10], (v.get("to") or "")[:10]))
r = v.get("retired")
e = v.get("retireError")
if isinstance(r, dict):
    print("retired %s: worktree in the Trash%s" % (v["address"], ", branch deleted" if r.get("branchDeleted") else ""))
elif isinstance(e, dict):
    print("not retired: %s: %s" % (e.get("code"), e.get("hint")))
    print("retire it later: k2 sidecar retire %s" % v["address"])
'
            ;;
        retire)
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("retired %s (%s): worktree in the Trash%s" % (v["address"], v.get("how"), ", branch deleted" if v.get("branchDeleted") else ", branch kept"))
dc = v.get("discardedCommits") or []
if dc:
    print("discarded %d commit(s); until git cleans up, git branch <name> %s brings them back" % (len(dc), dc[-1]["sha"][:10]))
'
            ;;
    esac
    exit 0
}

# Print a review answer (commits, files, uncommitted files, land or why not).
_sidecar_print_review() {
    printf '%s' "$1" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s  %s in %s  (%s)" % (v["address"], v.get("branch"), v.get("dir"), v.get("taskState")))
if v.get("retiredAt"):
    print("retired (%s); nothing to land" % v.get("retiredHow")); sys.exit(0)
commits = v.get("commits") or []
print("%d commit%s to land on %s (at %s):" % (len(commits), "" if len(commits) == 1 else "s", v.get("onto"), (v.get("ontoHead") or "-")[:10]))
for c in commits:
    print("  %s %s%s" % (c["sha"][:10], c.get("subject", ""), "  (merge)" if c.get("merge") else ""))
files = v.get("files") or []
if files:
    print("files:")
    for f in files:
        print("  %s %s" % (f.get("status"), f.get("path")))
dirty = v.get("dirty") or []
if dirty:
    print("uncommitted in the worktree (never landed): %s" % ", ".join(dirty))
why = {
    "wrong_branch": "the main checkout is on %s, not %s" % (v.get("mainBranch"), v.get("onto")),
    "main_has_uncommitted_changes": "the main checkout has uncommitted changes: %s" % ", ".join(v.get("mainDirty") or []),
    "operation_in_progress": "a merge, rebase or cherry-pick is in progress in the main checkout",
    "has_merge": "a merge commit is in range; rebase the branch first",
    "already_landed": "every commit is already on %s" % v.get("onto"),
    "stale": "the base or the branch is gone",
}
b = v.get("blocked")
print("land: ready (tip %s)" % (v.get("tip") or "")[:10] if not b else "land: not now (%s): %s" % (b, why.get(b, b)))
'
}

cmd_sidecar_new() {
    local name="" harness="" brief_file="" brief_text="" have_text=0 workspace="" model="" as_json=0
    local worktree=0 base=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --worktree)
                worktree=1; shift
                # `--worktree <name>` names the sidecar too.
                if [ -z "$name" ] && [ $# -gt 0 ] && [[ "$1" != -* ]]; then
                    name="$1"; shift
                fi
                ;;
            --base) _need_value "$@"; base="${2:-}"; shift 2 ;;
            --harness) _need_value "$@"; harness="${2:-}"; shift 2 ;;
            --brief) _need_value "$@"; brief_file="${2:-}"; shift 2 ;;
            --brief-text) _need_value "$@"; brief_text="${2:-}"; have_text=1; shift 2 ;;
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            --model) _need_value "$@"; model="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            --) shift; break ;;
            -*) echo "usage: unknown flag '$1' (k2 sidecar --help)" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"
                else
                    echo "usage: one name only; quote names with spaces (k2 sidecar --help)" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ] || [ -z "$harness" ]; then
        echo "usage: k2 sidecar new <name> --harness <preset> [--brief <file>|-] [--brief-text \"…\"] [--worktree [--base <ref>]]" >&2
        exit 2
    fi
    if [ -n "$base" ] && [ "$worktree" != "1" ]; then
        echo "usage: --base goes with --worktree (k2 sidecar --help)" >&2
        exit 2
    fi
    if [ -n "$brief_file" ] && [ "$have_text" = "1" ]; then
        echo "usage: pass --brief or --brief-text, not both" >&2
        exit 2
    fi
    local brief_src=""
    if [ -n "$brief_file" ]; then
        if [ "$brief_file" = "-" ]; then
            brief_src="$(mktemp -t k2-sidecar-brief-XXXXXX)"
            cat > "$brief_src"
        elif [ -f "$brief_file" ]; then
            brief_src="$brief_file"
        else
            echo "usage: brief file not found: $brief_file" >&2
            exit 2
        fi
    fi
    [ -z "$workspace" ] && workspace="$PROJECT"
    local body rc=0
    body="$(K2_SC_NAME="$name" K2_SC_HARNESS="$harness" K2_SC_WS="$workspace" K2_SC_MODEL="$model" \
        K2_SC_WORKTREE="$worktree" K2_SC_BASE="$base" \
        K2_SC_BRIEF_FILE="$brief_src" K2_SC_BRIEF_TEXT="$brief_text" K2_SC_HAVE_TEXT="$have_text" python3 -c '
import json, os, sys
b = {"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"],
     "harness": os.environ["K2_SC_HARNESS"]}
if os.environ.get("K2_SC_MODEL"):
    b["model"] = os.environ["K2_SC_MODEL"]
if os.environ.get("K2_SC_WORKTREE") == "1":
    b["worktree"] = True
    if os.environ.get("K2_SC_BASE"):
        b["base"] = os.environ["K2_SC_BASE"]
path = os.environ.get("K2_SC_BRIEF_FILE")
if path:
    raw = open(path, "rb").read()
    try:
        b["brief"] = raw.decode("utf-8")
    except UnicodeDecodeError:
        sys.stderr.write("brief_not_text: the brief is not UTF-8 text\n"); sys.exit(2)
elif os.environ.get("K2_SC_HAVE_TEXT") == "1":
    b["brief"] = os.environ.get("K2_SC_BRIEF_TEXT", "")
print(json.dumps(b))
')" || rc=$?
    [ "$brief_file" = "-" ] && rm -f "$brief_src"
    [ "$rc" -ne 0 ] && exit 2
    local raw
    raw="$(_sidecar_send POST /cli/sidecar/new "$body")" || true
    _sidecar_render new "$as_json" "$raw"
}

cmd_sidecar_list() {
    local workspace="" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            --json) as_json=1; shift ;;
            *) echo "usage: k2 sidecar list [--workspace <ws>] [--json]" >&2; exit 2 ;;
        esac
    done
    [ -z "$workspace" ] && workspace="$PROJECT"
    local raw
    raw="$(_sidecar_send GET /cli/sidecar/list "workspace=$(urlencode "$workspace")")" || true
    _sidecar_render list "$as_json" "$raw"
}

cmd_sidecar_archive() {
    local usage="usage: k2 sidecar archive <name>|--all-stopped [--workspace <ws>] [--json]"
    local name="" workspace="" all=0 as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            --all-stopped) all=1; shift ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sidecar --help)" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"
                else
                    echo "$usage" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    if { [ -z "$name" ] && [ "$all" = 0 ]; } || { [ -n "$name" ] && [ "$all" = 1 ]; }; then
        echo "$usage" >&2
        exit 2
    fi
    [ -z "$workspace" ] && workspace="$PROJECT"
    local body
    body="$(K2_SC_NAME="$name" K2_SC_WS="$workspace" K2_SC_ALL="$all" python3 -c '
import json, os
print(json.dumps({"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"], "allStopped": os.environ["K2_SC_ALL"] == "1"}))
')"
    local raw
    raw="$(_sidecar_send POST /cli/sidecar/archive "$body")" || true
    _sidecar_render archive "$as_json" "$raw"
}

cmd_sidecar_stop() {
    local name="" workspace="" force=0 as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            --force) force=1; shift ;;
            --json) as_json=1; shift ;;
            -*) echo "usage: unknown flag '$1' (k2 sidecar --help)" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then
                    name="$1"
                else
                    echo "usage: k2 sidecar stop <name> [--workspace <ws>] [--force] [--json]" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$name" ]; then
        echo "usage: k2 sidecar stop <name> [--workspace <ws>] [--force] [--json]" >&2
        exit 2
    fi
    [ -z "$workspace" ] && workspace="$PROJECT"
    local body
    body="$(K2_SC_NAME="$name" K2_SC_WS="$workspace" K2_SC_FORCE="$force" python3 -c '
import json, os
print(json.dumps({"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"],
                  "force": os.environ["K2_SC_FORCE"] == "1"}))
')"
    local raw
    raw="$(_sidecar_send POST /cli/sidecar/stop "$body")" || true
    _sidecar_render stop "$as_json" "$raw"
}

# One `<name>` plus flags for the Workbench verbs. Sets _SCW_NAME,
# _SCW_WS, _SCW_JSON, _SCW_YES and the verb's own flags in _SCW_FLAGS.
_sidecar_name_args() {
    local usage="$1"; shift
    _SCW_NAME="" _SCW_WS="" _SCW_JSON=0 _SCW_YES=0 _SCW_FLAGS=" "
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; _SCW_WS="${2:-}"; shift 2 ;;
            --json) _SCW_JSON=1; shift ;;
            --yes|-y) _SCW_YES=1; shift ;;
            --keep|--discard|--keep-branch) _SCW_FLAGS="${_SCW_FLAGS}$1 "; shift ;;
            -*) echo "usage: unknown flag '$1' ($usage)" >&2; exit 2 ;;
            *)
                if [ -z "$_SCW_NAME" ]; then
                    _SCW_NAME="$1"
                else
                    echo "usage: $usage" >&2
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$_SCW_NAME" ]; then
        echo "usage: $usage" >&2
        exit 2
    fi
    [ -z "$_SCW_WS" ] && _SCW_WS="$PROJECT"
    return 0
}

_sidecar_flag() {
    case "$_SCW_FLAGS" in *" $1 "*) return 0 ;; esac
    return 1
}

# Ask a person y/N on the terminal (land, retire --discard). --yes skips it;
# without a terminal and without --yes, nothing happens.
_sidecar_confirm() {
    local question="$1"
    [ "$_SCW_YES" = "1" ] && return 0
    if ! { [ -t 0 ] && [ -t 2 ]; }; then
        echo "not done: no terminal to ask on; read the above, then run it again with --yes" >&2
        exit 2
    fi
    local ans=""
    printf '%s [y/N] ' "$question" >&2
    read -r ans || ans=""
    case "$ans" in y|Y|yes|YES) return 0 ;; esac
    echo "not done" >&2
    exit 1
}

cmd_sidecar_interrupt() {
    _sidecar_name_args "k2 sidecar interrupt <name> [--workspace <ws>] [--json]" "$@"
    local body raw
    body="$(K2_SC_NAME="$_SCW_NAME" K2_SC_WS="$_SCW_WS" python3 -c '
import json, os
print(json.dumps({"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"]}))
')"
    raw="$(_sidecar_send POST /cli/sidecar/interrupt "$body")" || true
    _sidecar_render interrupt "$_SCW_JSON" "$raw"
}

cmd_sidecar_review() {
    _sidecar_name_args "k2 sidecar review <name> [--workspace <ws>] [--json]" "$@"
    local raw
    raw="$(_sidecar_send GET /cli/sidecar/review "workspace=$(urlencode "$_SCW_WS")&name=$(urlencode "$_SCW_NAME")")" || true
    _sidecar_render review "$_SCW_JSON" "$raw"
}

cmd_sidecar_land() {
    _sidecar_name_args "k2 sidecar land <name> [--keep] [--yes] [--workspace <ws>] [--json]" "$@"
    local raw tip onto blocked body
    raw="$(_sidecar_send GET /cli/sidecar/review "workspace=$(urlencode "$_SCW_WS")&name=$(urlencode "$_SCW_NAME")")" || true
    _sidecar_split "$raw"
    [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
    # What lands, shown before anything changes (stderr with --json).
    if [ "$_SCW_JSON" = "1" ]; then
        _sidecar_print_review "$_SC_BODY" >&2
    else
        _sidecar_print_review "$_SC_BODY"
    fi
    read -r tip onto blocked < <(printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print(v.get("tip") or "-", v.get("ontoHead") or "-", v.get("blocked") or "-")
')
    if [ "$blocked" = "already_landed" ] || [ "$blocked" = "sidecar_retired" ]; then
        echo "nothing to land" >&2
        exit 1
    fi
    _sidecar_confirm "Land these on the base now?"
    body="$(K2_SC_NAME="$_SCW_NAME" K2_SC_WS="$_SCW_WS" K2_SC_TIP="$tip" K2_SC_ONTO="$onto" \
        K2_SC_KEEP="$(_sidecar_flag --keep && echo 1 || echo 0)" python3 -c '
import json, os
b = {"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"], "expectTip": os.environ["K2_SC_TIP"],
     "retire": os.environ["K2_SC_KEEP"] != "1"}
if os.environ["K2_SC_ONTO"] != "-":
    b["expectOnto"] = os.environ["K2_SC_ONTO"]
print(json.dumps(b))
')"
    raw="$(_sidecar_send POST /cli/sidecar/land "$body")" || true
    _sidecar_render land "$_SCW_JSON" "$raw"
}

cmd_sidecar_retire() {
    _sidecar_name_args "k2 sidecar retire <name> [--discard] [--keep-branch] [--yes] [--workspace <ws>] [--json]" "$@"
    local discard=0 keep=0 body raw
    _sidecar_flag --discard && discard=1
    _sidecar_flag --keep-branch && keep=1
    if [ "$discard" = "1" ]; then
        raw="$(_sidecar_send GET /cli/sidecar/review "workspace=$(urlencode "$_SCW_WS")&name=$(urlencode "$_SCW_NAME")")" || true
        _sidecar_split "$raw"
        [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
        _sidecar_print_review "$_SC_BODY" >&2
        _sidecar_confirm "Discard this work (commits deleted with the branch, files to the Trash)?"
    fi
    body="$(K2_SC_NAME="$_SCW_NAME" K2_SC_WS="$_SCW_WS" K2_SC_DISCARD="$discard" K2_SC_KEEP="$keep" python3 -c '
import json, os
print(json.dumps({"workspace": os.environ["K2_SC_WS"], "name": os.environ["K2_SC_NAME"],
                  "discard": os.environ["K2_SC_DISCARD"] == "1", "keepBranch": os.environ["K2_SC_KEEP"] == "1"}))
')"
    raw="$(_sidecar_send POST /cli/sidecar/retire "$body")" || true
    _sidecar_render retire "$_SCW_JSON" "$raw"
}

cmd_sidecar_access() {
    local action="${1:-}" workspace=""
    shift 2>/dev/null || true
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) _need_value "$@"; workspace="${2:-}"; shift 2 ;;
            *) echo "usage: k2 sidecar access on|off|status [--workspace <ws>]" >&2; exit 2 ;;
        esac
    done
    [ -z "$workspace" ] && workspace="$PROJECT"
    local raw
    case "$action" in
        on|off)
            local body
            body="$(K2_SC_WS="$workspace" K2_SC_ON="$action" python3 -c '
import json, os
print(json.dumps({"workspace": os.environ["K2_SC_WS"], "toggle": "agents_manage",
                  "value": os.environ["K2_SC_ON"] == "on"}))
')"
            raw="$(_sidecar_send POST /cli/agent-access/set "$body")" || true
            _sidecar_render access 0 "$raw"
            ;;
        status)
            raw="$(_sidecar_send GET /cli/sidecar/list "workspace=$(urlencode "$workspace")")" || true
            _sidecar_render access 0 "$raw"
            ;;
        *)
            echo "usage: k2 sidecar access on|off|status [--workspace <ws>]" >&2
            exit 2
            ;;
    esac
}

cmd_sidecar() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _sidecar_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        _sidecar_usage
        return 0
    fi
    case "$sub" in
        new) cmd_sidecar_new "$@" ;;
        list|ls) cmd_sidecar_list "$@" ;;
        stop) cmd_sidecar_stop "$@" ;;
        archive) cmd_sidecar_archive "$@" ;;
        access) cmd_sidecar_access "$@" ;;
        interrupt) cmd_sidecar_interrupt "$@" ;;
        review) cmd_sidecar_review "$@" ;;
        land) cmd_sidecar_land "$@" ;;
        retire) cmd_sidecar_retire "$@" ;;
        *)
            echo "usage: unknown sidecar verb '$sub' (new|list|stop|archive|interrupt|review|land|retire|access — k2 sidecar --help)" >&2
            exit 2
            ;;
    esac
}

