# ── Feedback (F1) — ask a human for input, durably ───────────────────
#
# `k2 tickets` (alias: `k2 tickets`) files agent→human asks as durable
# items on this server's Tickets page (prd-agent-feedback-notifications, mockup:
# .k2/notes/feedback-cli-mockup.md — the help text below is that
# mockup VERBATIM; it passed the 36/36 comprehension gate).
#
# Contract highlights (pinned):
# - Success output human-readable; --json flips it to JSON.
# - ERRORS are ALWAYS JSON on stderr ({"error":{"code","hint"}})
#   regardless of --json.
# - Exit codes: 0 ok · 1 error · 2 bad usage · 4 not found (ambiguous
#   id prefix = 4 too, candidates in the hint).
# - `ask --wait` prints the BARE answer to stdout (exit 0);
#   resolved/dismissed WITHOUT an answer → empty stdout +
#   {"error":{"code":"closed_unanswered"}} on stderr + exit 1;
#   --timeout → {"error":{"code":"timeout"}} + exit 1, item STAYS
#   waiting.

# Note: `k2 tickets` remains a compatibility alias for `k2 tickets`.
# Product "Feedback" (K2 itself) is a future separate channel — use
# tickets for human contact on this server.

_feedback_err() {
    local code="$1" hint="$2"
    CODE="$code" HINT="$hint" python3 -c 'import json,os
print(json.dumps({"error": {"code": os.environ["CODE"], "hint": os.environ["HINT"]}}))' >&2 \
        || echo "Error ($code): $hint" >&2
}

# Session auto-detection (tickets ask): id + kind from the environment.
# First match wins (prd-ticket-answer-wakes-canonical D3):
#   1. K2_API_CELL=1 / K2SO_API_CELL=1 → id=K2_SESSION_ID, kind=api
#   2. K2_CELL=sidecar → kind=sidecar (id=K2_SESSION_ID when set)
#   3. K2_CELL=canonical, OR K2_SESSION_ID with no cell → kind=canonical
#   4. else CLAUDE_SESSION_ID / CLAUDE_CODE_SESSION_ID → kind=canonical
# Presence of K2_SESSION_ID is not a cell type. No legacy "sandbox" stamp.
# Nothing found → both stay empty (a session-less ask must still work).
_feedback_detect_session() {
    FB_SESSION_ID=""
    FB_SESSION_KIND=""
    if [ "${K5_API_CELL:-${K2_API_CELL:-${K2SO_API_CELL:-}}}" = "1" ]; then
        FB_SESSION_ID="${K5_SESSION_ID:-${K2_SESSION_ID:-${K2SO_SESSION_ID:-}}}"
        FB_SESSION_KIND="api"
    elif [ "${K5_CELL:-${K2_CELL:-${K2SO_CELL:-}}}" = "sidecar" ]; then
        FB_SESSION_ID="${K5_SESSION_ID:-${K2_SESSION_ID:-${K2SO_SESSION_ID:-}}}"
        FB_SESSION_KIND="sidecar"
    elif [ "${K5_CELL:-${K2_CELL:-${K2SO_CELL:-}}}" = "canonical" ]; then
        FB_SESSION_ID="${K5_SESSION_ID:-${K2_SESSION_ID:-${K2SO_SESSION_ID:-}}}"
        FB_SESSION_KIND="canonical"
    elif [ -n "${K5_SESSION_ID:-${K2_SESSION_ID:-${K2SO_SESSION_ID:-}}}" ] && [ -z "${K5_CELL:-${K2_CELL:-${K2SO_CELL:-}}}" ]; then
        FB_SESSION_ID="${K5_SESSION_ID:-${K2_SESSION_ID:-${K2SO_SESSION_ID:-}}}"
        FB_SESSION_KIND="canonical"
    elif [ -n "${CLAUDE_SESSION_ID:-}" ]; then
        FB_SESSION_ID="$CLAUDE_SESSION_ID"
        FB_SESSION_KIND="canonical"
    elif [ -n "${CLAUDE_CODE_SESSION_ID:-}" ]; then
        FB_SESSION_ID="$CLAUDE_CODE_SESSION_ID"
        FB_SESSION_KIND="canonical"
    fi
}

cmd_feedback_ask() {
    if _wants_help "$@"; then cmd_help_feedback_ask; exit 0; fi
    local title="" body="" body_file="" html_file="" kind="" options="" priority="" assign=""
    local wait=false timeout="" as_json=false options_set=false
    local -a files=() option_list=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --file)
                [ $# -ge 2 ] || { _feedback_err "usage" "--file requires a path (path[:line[-end]])"; exit 2; }
                files+=("$2"); shift 2 ;;
            --html)
                [ $# -ge 2 ] || { _feedback_err "usage" "--html requires a file path (or - for stdin). Start from \`k2 tickets template\`."; exit 2; }
                html_file="$2"; shift 2 ;;
            --body)
                [ $# -ge 2 ] || { _feedback_err "usage" "--body requires a value"; exit 2; }
                body="$2"; shift 2 ;;
            --body-file)
                [ $# -ge 2 ] || { _feedback_err "usage" "--body-file requires a file path"; exit 2; }
                body_file="$2"; shift 2 ;;
            --kind)
                [ $# -ge 2 ] || { _feedback_err "usage" "--kind requires a value (question | approval | fyi)"; exit 2; }
                kind="$2"; shift 2 ;;
            --options)
                [ $# -ge 2 ] || { _feedback_err "usage" "--options requires a list with ';' between options (e.g. \"Ship it; Hold\")"; exit 2; }
                [ ${#option_list[@]} -eq 0 ] || { _feedback_err "usage" "mix of --options and --option is not allowed"; exit 2; }
                options="$2"; options_set=true; shift 2 ;;
            --option)
                [ $# -ge 2 ] || { _feedback_err "usage" "--option requires a label"; exit 2; }
                $options_set && { _feedback_err "usage" "mix of --options and --option is not allowed"; exit 2; }
                option_list+=("$2"); shift 2 ;;
            --priority)
                [ $# -ge 2 ] || { _feedback_err "usage" "--priority requires a value (1-5)"; exit 2; }
                priority="$2"; shift 2 ;;
            --assign|--to)
                [ $# -ge 2 ] || { _feedback_err "usage" "--assign requires a comma-separated username list (e.g. owner,julie)"; exit 2; }
                assign="$2"; shift 2 ;;
            --wait) wait=true; shift ;;
            --timeout)
                [ $# -ge 2 ] || { _feedback_err "usage" "--timeout requires a value in seconds"; exit 2; }
                timeout="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets ask <title> [--html f|-] [--body|--body-file f] [--kind k] [--option \"…\"]… [--options \"a; b\"] [--priority 1-5] [--assign user[,user…]] [--file path[:line]]… [--wait [--timeout s]] [--json]"
                exit 2 ;;
            *)
                if [ -z "$title" ]; then title="$1"; else
                    _feedback_err "usage" "unexpected extra argument '$1' — quote the title if it contains spaces"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$title" ]; then
        _feedback_err "usage" "ask requires a <title>"
        exit 2
    fi
    case "${kind:-question}" in
        question|approval|fyi) ;;
        *)
            _feedback_err "usage" "invalid --kind '$kind' — valid: question, approval, fyi"
            exit 2 ;;
    esac
    if [ -n "$priority" ]; then
        case "$priority" in
            1|2|3|4|5) ;;
            *)
                _feedback_err "usage" "--priority must be 1-5"
                exit 2 ;;
        esac
    fi
    if [ -n "$body" ] && [ -n "$body_file" ]; then
        _feedback_err "usage" "--body and --body-file are mutually exclusive"
        exit 2
    fi
    # Files go to python by PATH, never through the environment: Linux caps
    # one env string at 128 KiB (MAX_ARG_STRLEN) and a brief may be 1 MiB
    # (prd-ticket-html-brief-v1 H31).
    if [ -n "$body_file" ]; then
        if [ ! -f "$body_file" ] || [ ! -r "$body_file" ]; then
            _feedback_err "usage" "--body-file: no such readable file '$body_file'"
            exit 2
        fi
    fi
    local html_path=""
    if [ -n "$html_file" ]; then
        if [ "$html_file" = "-" ]; then
            # python reads its program from a heredoc on stdin, so spool the
            # brief to a private temp file first and pass that path.
            html_path="$(mktemp -t k2-brief-XXXXXX)" || {
                _feedback_err "error" "--html -: cannot create a temp file"
                exit 1
            }
            # shellcheck disable=SC2064 # expand the path now, on purpose
            trap "rm -f '$html_path'" EXIT
            cat > "$html_path" || {
                _feedback_err "usage" "--html -: cannot read stdin"
                exit 2
            }
        elif [ ! -f "$html_file" ] || [ ! -r "$html_file" ]; then
            _feedback_err "usage" "--html: no such readable file '$html_file'"
            exit 2
        else
            html_path="$html_file"
        fi
    fi
    if [ -n "$timeout" ]; then
        if ! $wait; then
            _feedback_err "usage" "--timeout only makes sense with --wait"
            exit 2
        fi
        case "$timeout" in
            ''|*[!0-9]*|0)
                _feedback_err "usage" "--timeout must be a positive number of seconds"
                exit 2 ;;
        esac
    fi
    local files_json=""
    if [ ${#files[@]} -gt 10 ]; then
        _feedback_err "usage" "at most 10 files per message"
        exit 2
    fi
    if [ ${#files[@]} -gt 0 ]; then
        files_json="$(_thread_attachments_json "${files[@]}")" || { _feedback_err "usage" "--file: could not read the file list"; exit 2; }
    fi
    # --options splits on ';' only; --option labels go as a list, kept whole.
    local option_json=""
    if $options_set; then
        _k2_options_one_note "$options"
    elif [ ${#option_list[@]} -gt 0 ]; then
        option_json=$(printf '%s\n' "${option_list[@]}" | python3 -c 'import json,sys; print(json.dumps([ln.rstrip("\n") for ln in sys.stdin]))')
    fi
    _feedback_detect_session
    if [ "$(_envn TEST_FEEDBACK_DETECT)" = "1" ]; then
        printf '%s\n' "${FB_SESSION_KIND:-}"
        exit 0
    fi
    FB_TITLE="$title" FB_BODY="$body" FB_BODY_PATH="$body_file" FB_HTML_PATH="$html_path" \
    FB_KIND="$kind" FB_OPTIONS="$options" FB_OPTION_JSON="$option_json" \
    FB_PRIORITY="$priority" FB_ASSIGN="$assign" FB_WAIT="$wait" FB_TIMEOUT="$timeout" FB_JSON="$as_json" \
    FB_PROJECT="$PROJECT" FB_AGENT="$(_envn AGENT_NAME)" \
    FB_SESSION_ID="$FB_SESSION_ID" FB_SESSION_KIND="$FB_SESSION_KIND" FB_FILES="$files_json" \
    FB_FILES_PY="$_ticket_files_check_py" \
    ABASE="$BASE_URL" ATOKEN="$TOKEN" _K5_PY_HTTP="$_K5_PY_HTTP" _K5_PY_HEADERS="$(_k5_py_headers)" python3 - <<'PYEOF'
import json, os, re, sys, time
import urllib.error, urllib.parse, urllib.request
# Loopback and plain http skip the proxy; remote https may use it.
exec(os.environ["_K2_PY_PROXY_RULE"])
exec(os.environ.pop("_K5_PY_HTTP"))
exec(os.environ["FB_FILES_PY"])

base = os.environ["ABASE"]
token = os.environ["ATOKEN"]
as_json = os.environ.get("FB_JSON") == "true"
wait = os.environ.get("FB_WAIT") == "true"
timeout = os.environ.get("FB_TIMEOUT", "")

def die(err, exit_code=None):
    # Errors are ALWAYS JSON on stderr, regardless of --json.
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    if exit_code is None:
        code = str(err.get("code") or "")
        if code in ("not_found", "ambiguous_id"):
            exit_code = 4
        elif code == "usage" or code.startswith("brief_") or code.startswith("assignee_"):
            # brief_required / brief_empty / brief_too_large / brief_invalid
            # are bad usage: fix the brief and ask again (H3/H32). So are
            # assignee_required / assignee_unknown once the assignee policy
            # is Require (an owner setting: Settings → Tickets).
            exit_code = 2
        else:
            exit_code = 1
    sys.exit(exit_code)

def warn(code, hint):
    # Warnings are JSON on stderr too; they never change the exit code.
    sys.stderr.write(json.dumps({"warning": {"code": code, "hint": hint}}) + "\n")

def read_file(path, what):
    try:
        with open(path, "rb") as f:
            return f.read()
    except OSError as e:
        die({"code": "usage", "hint": "%s: cannot read '%s': %s" % (what, path, e.strerror)})

def request(method, route, params=None, body=None):
    qs = urllib.parse.urlencode(params or {})
    url = base + route + ("?" + qs if qs else "")
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
        headers={"Content-Type": "application/json"} if data else {})
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(k5_request(req), timeout=30) as r:
            return json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        try:
            return json.loads(e.read().decode("utf-8", "replace"))
        except Exception:
            return {"error": {"code": "error", "hint": "HTTP %s on %s" % (e.code, route)}}
    except Exception:
        return None

payload = {
    "project": os.environ["FB_PROJECT"],
    "title": os.environ["FB_TITLE"],
}
if os.environ.get("FB_BODY_PATH"):
    raw_body = read_file(os.environ["FB_BODY_PATH"], "--body-file")
    try:
        body_text = raw_body.decode("utf-8")
    except UnicodeDecodeError:
        die({"code": "usage", "hint": "--body-file: '%s' is not UTF-8 text" % os.environ["FB_BODY_PATH"]})
    body_text = body_text.rstrip("\n")
    if body_text:
        payload["body"] = body_text
elif os.environ.get("FB_BODY"):
    payload["body"] = os.environ["FB_BODY"]
sent_brief = False
if os.environ.get("FB_HTML_PATH"):
    raw_html = read_file(os.environ["FB_HTML_PATH"], "--html")
    try:
        payload["briefHtml"] = raw_html.decode("utf-8")
    except UnicodeDecodeError:
        # JSON can't carry raw non-UTF-8 bytes, so this one check happens
        # here, with the daemon's own code and exit (H3).
        die({"code": "brief_invalid",
             "hint": "brief_invalid: the --html file is not UTF-8. Save it as UTF-8 and ask again. See `k2 study ticket-brief`."})
    sent_brief = True
if os.environ.get("FB_KIND"):
    payload["kind"] = os.environ["FB_KIND"]
if os.environ.get("FB_PRIORITY"):
    payload["priority"] = int(os.environ["FB_PRIORITY"])
if os.environ.get("FB_OPTIONS"):
    # --options splits on ';' only; a comma is part of a label.
    opts = [o for o in (p.strip() for p in os.environ["FB_OPTIONS"].split(";")) if o]
    if opts:
        payload["options"] = opts
elif os.environ.get("FB_OPTION_JSON"):
    # --option, repeated: each label whole.
    opts = [o for o in (p.strip() for p in json.loads(os.environ["FB_OPTION_JSON"])) if o]
    if opts:
        payload["options"] = opts
if os.environ.get("FB_ASSIGN"):
    assignees = [a for a in (p.strip() for p in os.environ["FB_ASSIGN"].split(",")) if a]
    if assignees:
        payload["assignees"] = assignees
if os.environ.get("FB_AGENT"):
    payload["agentName"] = os.environ["FB_AGENT"]
# 0.46.6: files on the opening message, as a JSON array (never a string).
sent_files = False
if os.environ.get("FB_FILES"):
    payload["attachments"] = json.loads(os.environ["FB_FILES"])
    sent_files = bool(payload["attachments"])
# Session auto-detection result (may be absent — must not fail the ask).
if os.environ.get("FB_SESSION_ID"):
    payload["sessionId"] = os.environ["FB_SESSION_ID"]
    if os.environ.get("FB_SESSION_KIND"):
        payload["sessionKind"] = os.environ["FB_SESSION_KIND"]

d = request("POST", "/cli/feedback/create", body=payload)
if d is None:
    die({"code": "daemon_unreachable", "hint": "no response from the K2 daemon (is it running?)"})
if d.get("error"):
    die(d["error"])
if not d.get("ok"):
    die({"code": "error", "hint": "unexpected daemon response: %r" % json.dumps(d)[:200]})
ticket_files_check(sent_files, d)

for w in d.get("warnings") or []:
    if isinstance(w, dict):
        warn(str(w.get("code") or "warning"), str(w.get("hint") or ""))
if sent_brief and d.get("hasBrief") is not True:
    # H28: an older daemon ignores the unknown briefHtml field.
    warn("brief_not_stored", "this server is older than 0.43.2 and dropped the HTML brief")

fid = d["id"]

if not wait:
    if as_json:
        print(json.dumps(d))
    else:
        short = fid[:8]
        assignees = d.get("assignees") or []
        who = (" → " + ", ".join(assignees)) if assignees else ""
        if d.get("defaultAssignee"):
            # Settings → Tickets default assignee (you named nobody).
            who += " (this server's default assignee)"
        print('Filed feedback %s — "%s" (%s, priority %s, %s)%s'
              % (short, d.get("title", ""), d.get("kind", "?"),
                 d.get("priority", "?"), d.get("status", "?"), who))
        for line in ticket_file_lines(d.get("attachments"), None):
            print(line)
        print("A human will be notified. Check back with: k2 tickets show %s" % short)
    sys.exit(0)

# ── --wait: poll `show` (~2s) until answered / closed / timeout ──────
# Pinned contract: BARE answer to stdout + exit 0 on answered (also on
# resolved/dismissed WITH an answer); closed WITHOUT an answer →
# {"error":{"code":"closed_unanswered"}} on stderr + exit 1; timeout →
# {"error":{"code":"timeout"}} on stderr + exit 1 and the item STAYS
# waiting (we just stop polling). Transient poll failures are retried —
# a daemon blip must not fake a closed_unanswered.
deadline = time.time() + int(timeout) if timeout else None
while True:
    d = request("GET", "/cli/feedback/show", params={"id": fid})
    if isinstance(d, dict) and d.get("ok"):
        status = d.get("status")
        answer = d.get("answer")
        if status == "answered" and answer:
            # 0.46.6: stdout stays the BARE answer (pinned). The files on
            # the answering message get one stderr note; --json carries
            # them with absolute paths. The agent also got `Attached:`
            # lines in its session.
            msg = None
            for c in reversed(d.get("comments") or []):
                if isinstance(c, dict) and str(c.get("body", "")).strip() == str(answer).strip():
                    msg = c
                    break
            atts = [a for a in ((msg or {}).get("attachments") or []) if isinstance(a, dict)]
            root = d.get("projectPath")
            for a in atts:
                if root and a.get("path"):
                    a["absPath"] = os.path.join(root, a["path"])
            if as_json:
                print(json.dumps({"ok": True, "id": fid, "status": status, "answer": answer, "attachments": atts}))
                sys.exit(0)
            if atts:
                sys.stderr.write("%d file%s attached: k2 tickets show %s\n"
                                 % (len(atts), "" if len(atts) == 1 else "s", fid[:8]))
            print(answer)
            sys.exit(0)
        # Free text instead of an option pick: the person wants to talk.
        # Hand the agent their latest message (exit 5) so it can reply
        # with `comment`, settle it, then `resolve --answered`.
        if status == "needs_discussion":
            human = [c for c in (d.get("comments") or [])
                     if isinstance(c, dict) and c.get("author") != d.get("agentName")]
            if human:
                print(human[-1].get("body", ""))
            sys.stderr.write(json.dumps({"status": "needs_discussion", "hint": (
                "The person wrote a message instead of picking an option. "
                "Reply with: k2 tickets comment %s \"<reply>\". When settled: "
                "k2 tickets resolve %s --answered \"<outcome>\" (or resolve)."
                % (fid[:8], fid[:8]))}) + "\n")
            sys.exit(5)
        if status in ("resolved", "dismissed"):
            if answer:
                print(answer)
                sys.exit(0)
            sys.stderr.write('{"error":{"code":"closed_unanswered"}}\n')
            sys.exit(1)
    if deadline is not None and time.time() >= deadline:
        sys.stderr.write('{"error":{"code":"timeout"}}\n')
        sys.exit(1)
    nap = 2.0
    if deadline is not None:
        nap = max(0.1, min(nap, deadline - time.time()))
    time.sleep(nap)
PYEOF
}

cmd_feedback_list() {
    if _wants_help "$@"; then cmd_help_feedback_list; exit 0; fi
    local all=false status="" as_json=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --all) all=true; shift ;;
            --status)
                [ $# -ge 2 ] || { _feedback_err "usage" "--status requires a value (waiting | needs_discussion | answered | planned | resolved | dismissed)"; exit 2; }
                status="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets list [--all] [--status <s>] [--json]"
                exit 2 ;;
            *)
                _feedback_err "usage" "unexpected argument '$1' — usage: k2 tickets list [--all] [--status <s>] [--json]"
                exit 2 ;;
        esac
    done
    if [ -n "$status" ]; then
        case "$status" in
            waiting|needs_discussion|answered|planned|resolved|dismissed) ;;
            *)
                _feedback_err "usage" "invalid --status '$status' — valid: waiting, needs_discussion, answered, planned, resolved, dismissed"
                exit 2 ;;
        esac
    fi
    local resp
    # 0.46.3 A2: an agent's own workspace, from any folder.
    _cli_use_passport_project
    # bash 3.2 (macOS /bin/bash) + `set -u` rejects empty-array
    # expansion, so branch instead of building an args array.
    if [ -n "$status" ]; then
        resp=$(cli_request "/cli/feedback/list" "status=$(urlencode "$status")") || resp=""
    elif $all; then
        resp=$(cli_request "/cli/feedback/list" "all=1") || resp=""
    else
        resp=$(cli_request "/cli/feedback/list") || resp=""
    fi
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    printf '%s' "$resp" | AS_JSON="$as_json" python3 -c "$_feedback_list_py"
}

# `k2 tickets list`: print the list from the reply on stdin.
_feedback_list_py='
import json, os, sys
raw = sys.stdin.buffer.read().decode("utf-8", "replace")
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write(json.dumps({"error": {"code": "bad_response",
        "hint": "daemon returned an unparseable response: %r" % raw[:200]}}) + "\n")
    sys.exit(1)
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
    sys.exit(0)
print("ID        STATUS             PRI  KIND      TITLE")
for i in d.get("items", []):
    print("%-10s%-18s%2s   %-10s%s" % (
        str(i.get("id", ""))[:8], i.get("status", "?"),
        i.get("priority", "?"), i.get("kind", "?"), i.get("title", "")))
'

cmd_feedback_show() {
    if _wants_help "$@"; then cmd_help_feedback_show; exit 0; fi
    local id="" as_json=false want_html=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --html) want_html=true; shift ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets show <id> [--html] [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _feedback_err "usage" "unexpected extra argument '$1' — usage: k2 tickets show <id>"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        _feedback_err "usage" "show requires an <id> (from ask/list; a short unique prefix works)"
        exit 2
    fi
    # The reply can carry a 1 MiB brief, so it reaches python on stdin, not
    # in an env var, which Linux caps at 128 KiB per string
    # (prd-ticket-html-brief-v1 H20/H31). It is fetched like every other
    # verb (cli_request), so it gets the same retries and closing lines.
    local resp
    resp=$(cli_request "/cli/feedback/show" "id=$(urlencode "$id")" "brief=1") || resp=""
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    printf '%s' "$resp" | AS_JSON="$as_json" WANT_HTML="$want_html" FB_FILES_PY="$_ticket_files_check_py" \
        python3 -c "$_feedback_show_py"
}

# `k2 tickets show`: print one ticket from the reply on stdin.
_feedback_show_py='
import json, os, sys, time
exec(os.environ["FB_FILES_PY"])

as_json = os.environ.get("AS_JSON") == "true"
want_html = os.environ.get("WANT_HTML") == "true"

def die(err):
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)

raw = sys.stdin.buffer.read().decode("utf-8", "replace")
try:
    d = json.loads(raw)
except Exception:
    die({"code": "bad_response", "hint": "daemon returned an unparseable response: %r" % raw[:200]})
if not isinstance(d, dict):
    die({"code": "bad_response", "hint": "daemon returned an unexpected response: %r" % raw[:200]})
err = d.get("error")
if err is not None:
    die(err)

brief = d.get("brief") if isinstance(d.get("brief"), dict) else None

if want_html:
    # H41: stdout carries ONLY the cleaned HTML, so `> brief.html` is safe.
    if brief is None or not isinstance(brief.get("html"), str):
        die({"code": "no_brief", "hint": "ticket %s has no HTML brief" % str(d.get("id", ""))[:8]})
    if as_json:
        print(json.dumps(d))
    else:
        sys.stdout.write(brief["html"])
    sys.exit(0)

# Without --html the HTML never prints, --json included (H20).
d.pop("brief", None)
if as_json:
    print(json.dumps(d))
    sys.exit(0)

def ago(ts):
    s = int(time.time() - ts)
    if s < 60: return "just now"
    if s < 3600: return "%dm ago" % (s // 60)
    if s < 172800: return "%dh ago" % (s // 3600)
    return "%dd ago" % (s // 86400)

def size(n):
    if n < 1024: return "%d B" % n
    if n < 1024 * 1024: return "%.1f KB" % (n / 1024.0)
    return "%.1f MB" % (n / (1024.0 * 1024.0))

print("feedback : %s  (%s)" % (str(d.get("id", ""))[:8], d.get("status", "?")))
print("title    : %s" % d.get("title", ""))
print("kind     : %s · priority %s" % (d.get("kind", "?"), d.get("priority", "?")))
asked = "%s" % (d.get("agentName") or "?")
if d.get("sessionId"):
    asked += " · session %s" % str(d["sessionId"])[:8]
asked += " · %s" % ago(d.get("createdAt", 0))
print("asked by : %s" % asked)
assignees = d.get("assignees") or []
print("assigned : %s" % (", ".join(assignees) if assignees else "(none)"))
if d.get("options"):
    print("options  : %s" % " | ".join(d["options"]))
if d.get("answer"):
    print("answer   : %s" % d["answer"])
if d.get("body"):
    body_lines = str(d["body"]).splitlines() or [""]
    print("body     : %s" % body_lines[0])
    for extra in body_lines[1:]:
        print(" " * 11 + extra)
if d.get("hasBrief"):
    n = d.get("briefBytes")
    if n is None and brief is not None:
        n = brief.get("bytes")
    print("brief    : HTML, %s  (k2 tickets show %s --html > brief.html)"
          % (size(int(n)) if isinstance(n, (int, float)) else "?", str(d.get("id", ""))[:8]))
    text = str((brief or {}).get("text") or "")
    lines = text.splitlines()
    for line in lines[:40]:
        print("  " + line)
    if len(lines) > 40:
        print("  … %d more lines (open the ticket, or use --html)" % (len(lines) - 40))
print()
print("thread:")
for c in d.get("comments", []):
    lines = str(c.get("body", "")).splitlines() or [""]
    print("  %-9s%-10s%s" % (c.get("author", "?"), "(%s)" % ago(c.get("at", 0)), lines[0]))
    for extra in lines[1:]:
        print(" " * 21 + extra)
    # 0.46.6: the files of the message, absolute for the owner (projectPath).
    for line in ticket_file_lines(c.get("attachments"), d.get("projectPath"), " " * 21):
        print(line)
'

# `k2 tickets template` — the brief skeleton (prd-ticket-html-brief-v1 H15).
# Static and daemon-optional (H42). Only tags and classes the daemon keeps.
cmd_feedback_template() {
    if _wants_help "$@"; then cmd_help_feedback_template; exit 0; fi
    if [ $# -gt 0 ]; then
        _feedback_err "usage" "template takes no arguments — usage: k2 tickets template > brief.html"
        exit 2
    fi
    # No HTML comments in the skeleton: the daemon strips comments and would
    # warn brief_sanitized on every ticket made from it.
    cat <<'EOF'
<h2>Problem</h2>
<p>What is wrong, in plain words. One short paragraph.</p>

<h2>What I tried</h2>
<ul>
  <li>The command or change, and what happened.</li>
  <li>Why that did not solve it.</li>
</ul>

<section class="k2-need">
  <h2>What I need from you</h2>
  <p>The one decision or action you need, stated so it can be answered in a line.</p>
</section>

<h2>Options</h2>
<ol class="k2-options">
  <li><strong>Option A</strong> — what happens, and the cost.</li>
  <li><strong>Option B</strong> — what happens, and the cost.</li>
</ol>

<h2>Context</h2>
<p class="k2-callout">Anything the human must know before answering.</p>
<pre><code>Relevant log lines or command output.</code></pre>
<p>Screenshots go inline as &lt;img alt="what it shows" src="data:image/png;base64,…"&gt;. Delete this line if you have none.</p>
EOF
    # The next step goes to stderr, so `> brief.html` stays a clean brief.
    echo 'Next: k2 tickets ask "<title>" --html brief.html --assign <user>  (a user on this server: k2 connections list --users; the owner may require it: k2 tickets settings)' >&2
}

cmd_feedback_comment() {
    if _wants_help "$@"; then cmd_help_feedback_comment; exit 0; fi
    local id="" text="" as_json=false
    local -a files=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --file)
                [ $# -ge 2 ] || { _feedback_err "usage" "--file requires a path (path[:line[-end]])"; exit 2; }
                files+=("$2"); shift 2 ;;
            --)
                # The rest is the id and the text, as typed.
                shift
                while [ $# -gt 0 ]; do
                    if [ -z "$id" ]; then id="$1"
                    elif [ -z "$text" ]; then text="$1"
                    else
                        _feedback_err "usage" "unexpected extra argument '$1' — quote the comment text"
                        exit 2
                    fi
                    shift
                done ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets comment <id> [<text>] [--file path[:line]]… [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"
                elif [ -z "$text" ]; then text="$1"
                else
                    _feedback_err "usage" "unexpected extra argument '$1' — quote the comment text"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ] || { [ -z "$text" ] && [ ${#files[@]} -eq 0 ]; }; then
        _feedback_err "usage" "usage: k2 tickets comment <id> \"<text>\" (the text is optional with --file)"
        exit 2
    fi
    if [ ${#files[@]} -gt 10 ]; then
        _feedback_err "usage" "at most 10 files per message"
        exit 2
    fi
    local files_json=""
    if [ ${#files[@]} -gt 0 ]; then
        files_json="$(_thread_attachments_json "${files[@]}")" || { _feedback_err "usage" "--file: could not read the file list"; exit 2; }
    fi
    # Author attribution: the CLI is the AGENT interface, so default to
    # the agent identity (env override → workspace folder basename, the
    # product-default agent display name). The renderer's thread panel
    # posts author="owner" via the route directly.
    local author
    _envn_into author AGENT_NAME
    [ -z "$author" ] && author="$(basename "$PROJECT")"
    local body resp
    # Files: an array embedded in the body (TA22); relative paths resolve
    # against the TICKET's workspace on the daemon.
    body=$(FB_ID="$id" FB_TEXT="$text" FB_AUTHOR="$author" FB_FILES="$files_json" python3 -c 'import json,os
b = {"id": os.environ["FB_ID"], "body": os.environ["FB_TEXT"], "author": os.environ["FB_AUTHOR"]}
if os.environ.get("FB_FILES"):
    b["attachments"] = json.loads(os.environ["FB_FILES"])
print(json.dumps(b))')
    resp=$(cli_post_json "/cli/feedback/comment" "$body") || resp=""
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    RESP="$resp" AS_JSON="$as_json" SENT_FILES="${#files[@]}" FB_FILES_PY="$_ticket_files_check_py" python3 - <<'PYEOF'
import json, os, sys
exec(os.environ["FB_FILES_PY"])
d = json.loads(os.environ["RESP"])
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)
ticket_files_check(os.environ.get("SENT_FILES", "0") != "0", d)
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
else:
    print("Added comment to %s." % str(d.get("id", ""))[:8])
    for line in ticket_file_lines(d.get("attachments"), None):
        print(line)
PYEOF
}

cmd_feedback_resolve() {
    if _wants_help "$@"; then cmd_help_feedback_resolve; exit 0; fi
    local id="" as_json=false answered="" has_answered=false
    local -a files=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --file)
                [ $# -ge 2 ] || { _feedback_err "usage" "--file requires a path (path[:line[-end]])"; exit 2; }
                files+=("$2"); shift 2 ;;
            --answered)
                [ $# -ge 2 ] || { _feedback_err "usage" "--answered requires the agreed outcome — k2 tickets resolve <id> --answered \"<outcome>\""; exit 2; }
                answered="$2"; has_answered=true; shift 2 ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets resolve <id> [--answered \"<outcome>\" [--file path[:line]]…] [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"; else
                    _feedback_err "usage" "unexpected extra argument '$1' — usage: k2 tickets resolve <id>"
                    exit 2
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        _feedback_err "usage" "resolve requires an <id> (from ask/list; a short unique prefix works)"
        exit 2
    fi
    if [ "$has_answered" = true ] && [ -z "$(printf '%s' "$answered" | tr -d '[:space:]')" ]; then
        _feedback_err "usage" "--answered requires the agreed outcome — k2 tickets resolve <id> --answered \"<outcome>\""
        exit 2
    fi
    # 0.46.6: files ride a message; only --answered writes one.
    if [ ${#files[@]} -gt 0 ] && [ "$has_answered" != true ]; then
        _feedback_err "usage" "--file needs --answered (files ride a message; use k2 tickets comment <id> --file …)"
        exit 2
    fi
    if [ ${#files[@]} -gt 10 ]; then
        _feedback_err "usage" "at most 10 files per message"
        exit 2
    fi
    local files_json=""
    if [ ${#files[@]} -gt 0 ]; then
        files_json="$(_thread_attachments_json "${files[@]}")" || { _feedback_err "usage" "--file: could not read the file list"; exit 2; }
    fi
    # Settling a discussion records the outcome under the AGENT's name
    # (same identity rule as `comment`), never as a human answer.
    local author
    _envn_into author AGENT_NAME
    [ -z "$author" ] && author="$(basename "$PROJECT")"
    local body resp
    body=$(FB_ID="$id" FB_ANSWERED="$answered" FB_HAS_ANSWERED="$has_answered" FB_AUTHOR="$author" FB_FILES="$files_json" python3 -c 'import json,os
b = {"id": os.environ["FB_ID"]}
if os.environ.get("FB_HAS_ANSWERED") == "true":
    b["status"] = "answered"
    b["answer"] = os.environ["FB_ANSWERED"]
    b["author"] = os.environ["FB_AUTHOR"]
if os.environ.get("FB_FILES"):
    b["attachments"] = json.loads(os.environ["FB_FILES"])
print(json.dumps(b))')
    resp=$(cli_post_json "/cli/feedback/resolve" "$body") || resp=""
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    RESP="$resp" AS_JSON="$as_json" SENT_FILES="${#files[@]}" FB_FILES_PY="$_ticket_files_check_py" python3 - <<'PYEOF'
import json, os, sys
exec(os.environ["FB_FILES_PY"])
d = json.loads(os.environ["RESP"])
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)
ticket_files_check(os.environ.get("SENT_FILES", "0") != "0", d)
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
elif d.get("status") == "answered":
    print("Marked %s answered." % str(d.get("id", ""))[:8])
    for line in ticket_file_lines(d.get("attachments"), None):
        print(line)
else:
    print("Resolved %s." % str(d.get("id", ""))[:8])
PYEOF
}

# Replace assignees on an existing ticket. Usernames are snapshots
# (owner or connect-user names from `k2 users list`); empty --clear
# fans push back out to all devices.
cmd_feedback_assign() {
    if _wants_help "$@"; then cmd_help_feedback_assign; exit 0; fi
    local id="" as_json=false clear=false
    local -a users=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --clear) clear=true; shift ;;
            --to|--assign)
                [ $# -ge 2 ] || { _feedback_err "usage" "--to requires a comma-separated username list"; exit 2; }
                IFS=',' read -r -a _parts <<< "$2"
                for u in ${_parts[@]+"${_parts[@]}"}; do
                    u="${u#"${u%%[![:space:]]*}"}"
                    u="${u%"${u##*[![:space:]]}"}"
                    [ -n "$u" ] && users+=("$u")
                done
                shift 2 ;;
            -*)
                _feedback_err "usage" "unknown flag '$1' — usage: k2 tickets assign <id> <user> [user…] | --to u1,u2 | --clear [--json]"
                exit 2 ;;
            *)
                if [ -z "$id" ]; then id="$1"
                else users+=("$1")
                fi
                shift ;;
        esac
    done
    if [ -z "$id" ]; then
        _feedback_err "usage" "assign requires an <id> (from ask/list; a short unique prefix works)"
        exit 2
    fi
    if $clear; then
        users=()
    elif [ ${#users[@]} -eq 0 ]; then
        _feedback_err "usage" "assign requires at least one username (or --clear). Prefer: k2 tickets assign <id> owner  OR  --to owner,julie"
        exit 2
    fi
    local body resp
    body=$(FB_ID="$id" FB_USERS="$(printf '%s\n' ${users[@]+"${users[@]}"})" python3 -c 'import json,os
users=[u for u in os.environ.get("FB_USERS","").splitlines() if u.strip()]
print(json.dumps({"id": os.environ["FB_ID"], "usernames": users}))')
    resp=$(cli_post_json "/cli/feedback/assign" "$body") || resp=""
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    RESP="$resp" AS_JSON="$as_json" python3 - <<'PYEOF'
import json, os, sys
d = json.loads(os.environ["RESP"])
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    sys.exit(4 if code in ("not_found", "ambiguous_id") else 2 if code == "usage" else 1)
# Warnings (assignee_unknown: a name that is not a user on this server)
# print as JSON on stderr and never change the exit code.
for w in d.get("warnings") or []:
    if isinstance(w, dict):
        sys.stderr.write(json.dumps({"warning": {"code": str(w.get("code") or "warning"), "hint": str(w.get("hint") or "")}}) + "\n")
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
else:
    assignees = d.get("assignees") or []
    short = str(d.get("id", ""))[:8]
    if assignees:
        print("Assigned %s → %s." % (short, ", ".join(assignees)))
    else:
        print("Cleared assignees on %s." % short)
PYEOF
}

# Settings → Tickets (0.45.4): show or change the server's ticket rules
# (GET/POST /cli/tickets/settings). Changing them is owner/admin only.
# Inside a K2 terminal every tickets verb sends the SESSION PASSPORT
# (_cli_tickets_sends_passport): a read answers canEdit:false, and a write
# is refused (owner_only), so an agent can never loosen the rules it is
# held to.
cmd_feedback_settings() {
    if _wants_help "$@"; then cmd_help_feedback_settings; exit 0; fi
    local as_json=false brief="" assignee="" default="" set_default=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=true; shift ;;
            --brief|--assignee-required)
                [ $# -ge 2 ] || { _feedback_err "usage" "$1 requires off, warn or require"; exit 2; }
                case "$2" in
                    off|warn|require) ;;
                    *) _feedback_err "usage" "$1 must be off, warn or require (got '$2')"; exit 2 ;;
                esac
                if [ "$1" = "--brief" ]; then brief="$2"; else assignee="$2"; fi
                shift 2 ;;
            --default-assignee)
                [ $# -ge 2 ] || { _feedback_err "usage" "--default-assignee requires a user (owner or a Connect user) or none"; exit 2; }
                default="$2"; set_default=true
                shift 2 ;;
            *)
                _feedback_err "usage" "unknown argument '$1' — usage: k2 tickets settings [--brief off|warn|require] [--assignee-required off|warn|require] [--default-assignee <user|none>] [--json]"
                exit 2 ;;
        esac
    done
    local resp="" changed=false
    if [ -n "$brief" ] || [ -n "$assignee" ] || $set_default; then
        changed=true
        local body
        body=$(FB_BRIEF="$brief" FB_ASSIGNEE="$assignee" FB_SET_DEFAULT="$set_default" FB_DEFAULT="$default" python3 -c 'import json,os
b={}
if os.environ.get("FB_BRIEF"): b["briefPolicy"]=os.environ["FB_BRIEF"]
if os.environ.get("FB_ASSIGNEE"): b["assigneePolicy"]=os.environ["FB_ASSIGNEE"]
if os.environ.get("FB_SET_DEFAULT")=="true": b["defaultAssignee"]=os.environ.get("FB_DEFAULT","")
print(json.dumps(b))')
        resp=$(cli_post_json "/cli/tickets/settings" "$body") || resp=""
    else
        resp=$(cli_request "/cli/tickets/settings") || resp=""
    fi
    if [ -z "$resp" ]; then
        _feedback_err "daemon_unreachable" "no response from the K2 daemon (is it running?)"
        exit 1
    fi
    RESP="$resp" AS_JSON="$as_json" CHANGED="$changed" python3 - <<'PYEOF'
import json, os, sys
try:
    d = json.loads(os.environ["RESP"])
except ValueError:
    sys.stderr.write(json.dumps({"error": {"code": "bad_response", "hint": "the daemon did not answer with JSON (older daemon? Settings → Tickets needs K2 0.45.4)"}}) + "\n")
    sys.exit(1)
err = d.get("error")
if err is not None:
    if isinstance(err, str):
        err = {"code": "error", "hint": err}
    sys.stderr.write(json.dumps({"error": err}) + "\n")
    code = err.get("code")
    if code in ("owner_only", "role_required", "forbidden"):
        sys.exit(3)
    sys.exit(2 if code in ("usage", "bad_value", "bad_request", "unknown_user") else 1)
if os.environ.get("AS_JSON") == "true":
    print(json.dumps(d))
    sys.exit(0)
labels = {"off": "off", "warn": "warn (file it and say so)", "require": "require (refuse without)"}
who = d.get("defaultAssignee") or "none"
if d.get("defaultAssignee") and d.get("defaultAssigneeKnown") is False:
    who += " (not a user here any more: not applied)"
if os.environ.get("CHANGED") == "true":
    print("Saved.")
print("Ticket rules (Settings → Tickets)")
print("  Require an HTML brief:  %s" % labels.get(d.get("briefPolicy"), d.get("briefPolicy")))
print("  Require an assignee:    %s" % labels.get(d.get("assigneePolicy"), d.get("assigneePolicy")))
print("  Default assignee:       %s" % who)
if not d.get("canEdit"):
    print("Only the owner or an admin can change these.")
PYEOF
}

cmd_help_feedback_settings() {
    cat <<'EOF'
%include cli/text/help/feedback/settings.txt
EOF
}

cmd_help_feedback() {
    cat <<'EOF'
%include cli/text/help/feedback.txt
EOF
}

cmd_help_feedback_ask() {
    cat <<'EOF'
%include cli/text/help/feedback/ask.txt
EOF
}

cmd_help_feedback_assign() {
    cat <<'EOF'
%include cli/text/help/feedback/assign.txt
EOF
}

cmd_help_feedback_list() {
    cat <<'EOF'
%include cli/text/help/feedback/list.txt
EOF
}

cmd_help_feedback_show() {
    cat <<'EOF'
%include cli/text/help/feedback/show.txt
EOF
}

cmd_help_feedback_template() {
    cat <<'EOF'
%include cli/text/help/feedback/template.txt
EOF
}

cmd_help_feedback_comment() {
    cat <<'EOF'
%include cli/text/help/feedback/comment.txt
EOF
}

cmd_help_feedback_resolve() {
    cat <<'EOF'
%include cli/text/help/feedback/resolve.txt
EOF
}

