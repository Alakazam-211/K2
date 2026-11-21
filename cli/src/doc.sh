# ── `k2 doc` — live Markdown docs (Markdown multiplayer v1, K3) ─────
# Every verb is one daemon call on `/cli/docs/*`: people and agents edit
# the same .md file at once, with presence, attribution and their own
# undo; the daemon writes the file back. Inside a K2 terminal the session
# passport is sent (an `id` + locked tool): an agent reaches its own
# workspace, and another workspace only when connected AND shared with it.
# Text verbs (cat head tail wc grep sed diff view, sed -i patch append
# insert replace delete) run on the multiplayer kit's tools in the daemon:
# GNU semantics for a subset, one atomic change per edit verb.

_DOC_JSON=0
_DOC_WS=""
_DOC_OB=""
_DOC_FENCE=""
_DOC_TEXT=""
_DOC_TEXT_SET=0
_DOC_FILE=""

_doc_usage() {
    cat <<'EOF'
%include cli/text/help/doc.txt
EOF
}

_doc_die() {
    local code="$1" hint="$2" rc="${3:-2}"
    [ "${_DOC_JSON:-0}" = "1" ] || echo "Error: $hint" >&2
    python3 -c 'import json,sys; print(json.dumps({"error": {"code": sys.argv[1], "hint": sys.argv[2]}}), file=sys.stderr)' "$code" "$hint"
    exit "$rc"
}

_doc_in_cell() {
    ! _cli_skip_uds && [ -n "$(_hook_sock)" ] && [ -n "${SCOPED_TOKEN:-}" ]
}

# _doc_path P → the path to send: absolute when the file is here and no
# --workspace / --host names where it lives; else as given.
_doc_path() {
    local p="$1"
    if [ -z "$_DOC_WS" ] && [ "${_K2_FORCE_TCP:-0}" != "1" ] && [ -e "$p" ]; then
        python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$p"
    else
        printf '%s\n' "$p"
    fi
}

# _doc_exit STATUS BODY → print the refusal and exit with the doc exit code.
_doc_fail() {
    local status="$1" body="$2"
    printf '%s' "$body" | python3 -c '
import json, sys
status, as_json = sys.argv[1], sys.argv[2] == "1"
raw = sys.stdin.read()
code, hint = "", ""
try:
    v = json.loads(raw)
except Exception:
    v = None
if isinstance(v, dict):
    e = v.get("error")
    if isinstance(e, dict):
        code = e.get("code") or ""
        hint = e.get("hint") or e.get("message") or ""
    elif e is not None:
        hint = str(e)
elif raw.strip():
    hint = raw.strip()[:300]
if status == "000":
    code, hint = "daemon_unreachable", "no answer from the K2 daemon"
# The daemon names the exit for every kit error (error.exit, the kit'"'"'s
# K-EXIT-1 table); K2'"'"'s own refusals go by status.
rc = {"400": 2, "401": 3, "403": 3, "404": 4, "405": 2, "413": 2, "422": 4, "429": 2}.get(status, 1)
if status == "409":
    rc = {"not_joined": 2, "bad_approval": 2, "pack_mismatch": 4, "held": 2, "revert_too_old": 8, "base_expired": 8}.get(code, 1)
if isinstance(v, dict) and isinstance(v.get("error"), dict) and isinstance(v["error"].get("exit"), int):
    rc = v["error"]["exit"]
if not hint:
    hint = "the daemon refused (HTTP %s)" % status
if not as_json:
    print("Error: %s" % hint, file=sys.stderr)
print(json.dumps({"error": {"code": code or "error", "hint": hint, "status": int(status) if status.isdigit() else 0}}), file=sys.stderr)
if isinstance(v, dict) and isinstance(v.get("conflicts"), list) and not as_json:
    for c in v["conflicts"]:
        ln = c.get("lines") or ["?", "?"]
        yl, cl = c.get("yourLines"), c.get("currentLines")
        if isinstance(yl, list) and isinstance(cl, list) and len(yl) == 2 and len(cl) == 2:
            # write --base: where the region is in your text and in the current text.
            print("conflict at your lines %s-%s (now lines %s-%s):\n  yours: %r\n  now:   %r" % (yl[0], yl[1], cl[0], cl[1], c.get("ours"), c.get("theirs")), file=sys.stderr)
        else:
            print("conflict at lines %s-%s:\n  yours:  %r\n  theirs: %r" % (ln[0], ln[1], c.get("ours"), c.get("theirs")), file=sys.stderr)
sys.exit(rc)
' "$status" "${_DOC_JSON:-0}"
    exit $?
}

# _doc_call METHOD ROUTE PAYLOAD → _DOC_STATUS/_DOC_BODY; exits on non-200.
# A POST's PAYLOAD is _doc_body's output, never empty: empty means the body
# builder refused (it said why on stderr), so nothing is sent.
_doc_call() {
    if [ "$1" != "GET" ] && [ -z "${3:-}" ]; then
        exit 2
    fi
    local raw
    raw="$(cli_send_json "$1" "$2" "${3:-}")"
    _DOC_STATUS="${raw##*$'\n'}"
    _DOC_BODY="${raw%$'\n'*}"
    [ "$_DOC_STATUS" = "$raw" ] && _DOC_BODY=""
    [ -n "$_DOC_STATUS" ] || _DOC_STATUS="000"
    if [ "$_DOC_STATUS" != "200" ]; then
        _doc_fail "$_DOC_STATUS" "$_DOC_BODY"
    fi
}

# A temp folder for whole-file bodies and answers (removed on exit; the
# permission note still prints).
_doc_tmp() {
    [ -n "${_DOC_TMPD:-}" ] && return 0
    _DOC_TMPD="$(mktemp -d "${TMPDIR:-/tmp}/k2doc.XXXXXX")" || _doc_die tmp "can't make a temp folder"
    trap '_k2_exit_rc=$?; rm -rf "$_DOC_TMPD"; _permission_note_flush "$_k2_exit_rc"' EXIT
}

# _doc_xfer METHOD ROUTE QUERY BODYFILE OUTFILE → _DOC_STATUS; the answer
# in OUTFILE. For whole files (a tier-0 write, show --version): the body
# goes from a file and the answer to one, never through argv or a shell
# variable. Same transports as cli_send_json (the cell socket with the
# passport, else loopback with the token).
#
# 0.46.15: the send-once rule of cli_send_json. A POST goes again over TCP
# after the cell socket only when the socket took none of it: the connect
# failed (curl 7) or the daemon answered "not served on cell socket". Any
# other failure may have applied it, so it is not sent again:
# _cli_send_unanswered says so and ends the command with 1. GETs keep the
# fallback.
_doc_xfer() {
    local method="$1" path="$2" query="$3" bodyf="$4" outf="$5"
    local sock code="" _K2_CFG="" rc=0 t0="$SECONDS"
    : >"$outf"
    sock="$(_hook_sock)"
    if ! _cli_skip_uds && [ -n "$sock" ] && [ -S "$sock" ] && [ -n "${SCOPED_TOKEN:-}" ]; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        if [ "$method" = "GET" ]; then
            [ -n "$query" ] && _k2_cfg_add data "$query"
            code="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 300 -o "$outf" -w '%{http_code}' 2>/dev/null)" || rc=$?
        else
            _k2_cfg_add data-binary "@$bodyf"
            code="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" "http://localhost${path}" \
                --connect-timeout 5 --max-time 300 -H "Content-Type: application/json" \
                -o "$outf" -w '%{http_code}' 2>/dev/null)" || rc=$?
        fi
        if [ "$rc" -eq 0 ] && [ -n "$code" ] && [ "$code" != "000" ] && ! grep -q "not served on cell socket" "$outf" 2>/dev/null; then
            _DOC_STATUS="$code"
            [ "$code" = "200" ] || _permission_note "$(cat "$outf")"$'\n'"$code"
            return 0
        fi
        if [ "$method" != "GET" ] && [ "$rc" -ne 7 ] && ! grep -q "not served on cell socket" "$outf" 2>/dev/null; then
            _cli_send_unanswered "$path" "$t0" 0 ""
            # Soft callers (_K2_NOANSWER_SOFT=1) get status 000 instead.
            _DOC_STATUS="000"
            : >"$outf"
            return 0
        fi
    fi
    # TCP: in a session whose cell socket failed, the passport, never the
    # owner token from disk (_k2_sock_fallback_token). The socket's "not
    # served" answer, if any, is in OUTFILE.
    local _K2_TCP_TOKEN="${TOKEN:-}" sock_said=""
    if grep -q "not served on cell socket" "$outf" 2>/dev/null; then
        sock_said="not served on cell socket"
    fi
    _k2_sock_fallback_token "$sock" "$sock_said"
    _K2_CFG=""
    if [ "$method" = "GET" ]; then
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        [ -n "$query" ] && _k2_cfg_add data "$query"
        code="$(_k2_curl "$_K2_CFG" -s -G "${BASE_URL}${path}" \
            --connect-timeout 5 --max-time 300 -o "$outf" -w '%{http_code}' 2>/dev/null)" || code="000"
    else
        _k2_cfg_add url "${BASE_URL}${path}"
        _k2_cfg_auth "$_K2_TCP_TOKEN"
        _k2_cfg_add data-binary "@$bodyf"
        code="$(_k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time 300 \
            -H "Content-Type: application/json" -o "$outf" -w '%{http_code}' 2>/dev/null)" || code="000"
    fi
    [ -n "$code" ] || code="000"
    _DOC_STATUS="$code"
    [ "$code" = "200" ] || _permission_note "$(cat "$outf")"$'\n'"$code"
}

# _doc_call_file METHOD ROUTE QUERY BODYFILE → _DOC_OUT (the answer's file);
# exits on non-200 like _doc_call.
_doc_call_file() {
    _doc_tmp
    _DOC_OUT="$_DOC_TMPD/answer.json"
    _doc_xfer "$1" "$2" "$3" "$4" "$_DOC_OUT"
    if [ "$_DOC_STATUS" != "200" ]; then
        _doc_fail "$_DOC_STATUS" "$(cat "$_DOC_OUT")"
    fi
}

# _doc_body PATH [key value]... → JSON body with workspace/path/onBehalfOf.
# Every value is sent as the plain text it is: a value the user typed is
# never read as a marker, so `--name @int:5` names the version "@int:5".
# A key the CLI writes with a leading "@" carries a marker the CLI built
# itself, and its value must be one of:
#   "@text"  --text, --file or stdin as UTF-8 text (byte-exact)
#   "@b64"   the same bytes as base64, for any file
#   "@true" / "@false"   a JSON boolean
#   "@int:N" a JSON number (N is digits)
# Anything else there is refused with one usage error and exit 2, and the
# body is empty, so _doc_call sends nothing. The key "argv" is a JSON array
# the CLI built.
# The script rides `-c`, not a heredoc: a heredoc would BE python's stdin,
# and the doc's text comes from stdin.
_DOC_BODY_PY='import base64, json, re, sys
as_json, ws, ob, text_set, text, file_, path, rest = sys.argv[1] == "1", sys.argv[2], sys.argv[3], sys.argv[4] == "1", sys.argv[5], sys.argv[6], sys.argv[7], sys.argv[8:]
def refuse(hint):
    if not as_json:
        print("Error: " + hint, file=sys.stderr)
    print(json.dumps({"error": {"code": "usage", "hint": hint}}), file=sys.stderr)
    sys.exit(2)
d = {}
if ws:
    d["workspace"] = ws
if path:
    d["path"] = path
if ob:
    d["onBehalfOf"] = ob
def body_bytes():
    if text_set:
        return text.encode("utf-8", "surrogateescape")
    if file_:
        with open(file_, "rb") as f:
            return f.read()
    return sys.stdin.buffer.read()
def body_text():
    try:
        return body_bytes().decode("utf-8")
    except UnicodeDecodeError:
        refuse("the text is not valid UTF-8")
for i in range(0, len(rest), 2):
    k, v = rest[i], rest[i + 1]
    if k == "argv":
        v = json.loads(v)
    elif k.startswith("@"):
        k = k[1:]
        if v == "@text":
            v = body_text()
        elif v == "@b64":
            v = base64.b64encode(body_bytes()).decode("ascii")
        elif v in ("@true", "@false"):
            v = v == "@true"
        elif re.fullmatch(r"@int:[0-9]+", v):
            v = int(v[5:])
        elif v.startswith("@int:"):
            refuse("%s takes a whole number, not %r" % (k, v[5:]))
        else:
            refuse("%s: %r is not a value marker" % (k, v))
    d[k] = v
print(json.dumps(d))'
_doc_body() {
    python3 -c "$_DOC_BODY_PY" "${_DOC_JSON:-0}" "$_DOC_WS" "$_DOC_OB" "$_DOC_TEXT_SET" "$_DOC_TEXT" "$_DOC_FILE" "$@"
}

# _doc_query PATH [key value]... → urlencoded query for the GET routes.
_doc_query() {
    python3 - "$_DOC_WS" "$_DOC_OB" "$@" <<'PY'
import sys, urllib.parse
ws, ob, path, rest = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
q = []
if ws:
    q.append(("workspace", ws))
if path:
    q.append(("path", path))
if ob:
    q.append(("onBehalfOf", ob))
q += [(k, v) for k, v in zip(rest[0::2], rest[1::2]) if v != ""]
print(urllib.parse.urlencode(q))
PY
}

_doc_fence_on() {
    case "$_DOC_FENCE" in
        1) return 0 ;;
        0) return 1 ;;
    esac
    _doc_in_cell
}

# _doc_print_tool: a tool answer → stdout bytes, fence + summary on stderr,
# exit with the tool's code.
# The answer is in $_DOC_OUT (a file: `cat` of a whole any-file doc can be
# big). Heads-up notes (who else is on those lines) go to stderr; binary
# bytes never go onto a terminal.
# --json: a text doc's tool call gives its result as data (the kit's
# mpc.result/1, one JSON line on stdout, its exit the call's; K2's own lines
# on stderr); any other answer is printed whole.
_doc_print_tool() {
    if [ "${_DOC_JSON:-0}" = "1" ]; then
        python3 -c '
import json, sys
raw = open(sys.argv[1], encoding="utf-8").read()
try:
    v = json.loads(raw)
except ValueError:
    sys.stdout.write(raw + "\n")
    sys.exit(1)
r = v.get("result") if isinstance(v, dict) else None
if not isinstance(r, dict):
    sys.stdout.write(raw + "\n")
    sys.exit(int(v.get("exit", 1)) if isinstance(v, dict) else 1)
sys.stdout.write(v["stdout"].rstrip("\n") + "\n")
if v.get("ifVersion") is not None and v.get("changed"):
    sys.stderr.write("(checked against v%s, the version you last read)\n" % v["ifVersion"])
a = v.get("approval")
if isinstance(a, dict):
    sys.stderr.write("on behalf of %s: %s\n" % (a.get("onBehalfOf"), a.get("state")))
sys.exit(int(r["exit"]))
' "$_DOC_OUT"
        exit $?
    fi
    local fence=0
    _doc_fence_on && fence=1
    python3 -c '
import base64, json, sys
v = json.load(open(sys.argv[1], encoding="utf-8"))
fence, tty = sys.argv[2] == "1", sys.argv[3] == "1"
f = v.get("fence") if fence else None
out = v.get("stdout")
data = out.encode("utf-8") if isinstance(out, str) else base64.b64decode(v.get("stdoutB64") or "")
if tty and (not isinstance(out, str) or b"\0" in data):
    hint = "%s is binary (%d bytes); pipe it, or write it to a file: k2 doc show --version V --out FILE" % (v["doc"]["path"], len(data))
    print("Error: " + hint, file=sys.stderr)
    print(json.dumps({"error": {"code": "binary_output", "hint": hint}}), file=sys.stderr)
    sys.exit(2)
if f:
    sys.stderr.write(f.get("begin", ""))
    sys.stderr.flush()
sys.stdout.buffer.write(data)
sys.stdout.flush()
if f:
    if data and not data.endswith(b"\n"):
        sys.stderr.write("\n")
    sys.stderr.write(f.get("end", ""))
err = v.get("stderr")
if err:
    sys.stderr.write(err.rstrip("\n") + "\n")
for k in ("forEdit", "notes"):
    if v.get(k):
        sys.stderr.write(v[k].rstrip("\n") + "\n")
if v.get("ifVersion") is not None and v.get("changed"):
    sys.stderr.write("(checked against v%s, the version you last read)\n" % v["ifVersion"])
a = v.get("approval")
if isinstance(a, dict):
    sys.stderr.write("on behalf of %s: %s\n" % (a.get("onBehalfOf"), a.get("state")))
sys.exit(int(v.get("exit", 1)))
' "$_DOC_OUT" "$fence" "$([ -t 1 ] && echo 1 || echo 0)"
    exit $?
}

# A tool call's body, built in one place (argv, the doc, stdin when the
# verb reads it): `--old-file P` / `--new-file P` become `--old` / `--new`
# with the file's text (the kit does no I/O). Written to stdout as JSON.
_DOC_TOOL_PY='import json, os, sys
ws, ob, text_set, text, path, argv = sys.argv[1], sys.argv[2], sys.argv[3] == "1", sys.argv[4], sys.argv[5], sys.argv[6:]
def die(msg, rc=2):
    print("Error: " + msg, file=sys.stderr)
    print(json.dumps({"error": {"code": "usage", "hint": msg}}), file=sys.stderr)
    sys.exit(rc)
out, i = [], 0
while i < len(argv):
    a = argv[i]
    if a == "--":
        out += argv[i:]
        break
    flag = fp = None
    if a in ("--old-file", "--new-file"):
        if i + 1 >= len(argv):
            die(a + " needs a file")
        flag, fp, i = a[:-5], argv[i + 1], i + 2
    elif a.startswith("--old-file=") or a.startswith("--new-file="):
        k, fp = a.split("=", 1)
        flag, i = k[:-5], i + 1
    if flag:
        try:
            with open(fp, "rb") as f:
                raw = f.read()
        except OSError as e:
            die("%s-file %s: %s" % (flag, fp, e.strerror), 4)
        try:
            out += [flag, raw.decode("utf-8")]
        except UnicodeDecodeError:
            die("%s-file %s is not UTF-8 text" % (flag, fp))
        continue
    out.append(a)
    i += 1
verb, rest = out[0], out[1:]
opts = rest[: rest.index("--")] if "--" in rest else rest
exact = any(a in ("--old", "--new") or a.startswith("--old=") or a.startswith("--new=") for a in opts)
reads = verb in ("diff", "patch", "apply", "append", "insert", "replace-section") or (verb == "replace" and not exact) or "-" in opts
d = {"argv": out}
if os.environ.get("_DOC_JSON") == "1":
    d["json"] = True
if ws:
    d["workspace"] = ws
if path:
    d["path"] = path
if ob:
    d["onBehalfOf"] = ob
if reads:
    if text_set:
        d["stdin"] = text
    elif sys.stdin.isatty():
        die("k2 doc %s reads its text from stdin (pipe it in) or --text \"...\"" % verb)
    else:
        try:
            d["stdin"] = sys.stdin.buffer.read().decode("utf-8")
        except UnicodeDecodeError:
            die("k2 doc %s: stdin is not UTF-8 text" % verb)
print(json.dumps(d))'


# The --host server's token off argv: --token-stdin reads one line from
# stdin into _DOC_TOKEN. --token T still works for now, with a note: a
# command line is visible to every account on this computer while the
# command runs. _doc_token_stdin <token_stdin 0|1> <--token flag if given>
_doc_token_stdin() {
    _DOC_TOKEN=""
    if [ "$1" = 1 ]; then
        [ -z "$2" ] || _doc_die usage "--token-stdin or --token, not both"
        _cli_read_secret_stdin "Token: "
        _DOC_TOKEN="${_K2_SECRET%$'\r'}"; _K2_SECRET=""
        [ -n "$_DOC_TOKEN" ] || _doc_die usage "--token-stdin: no token on stdin"
    elif [ -n "$2" ]; then
        echo "note: --token is visible to every account on this computer while the command runs; use --token-stdin" >&2
    fi
}

# Parse the common flags out of "$@" into _DOC_* and _DOC_ARGS (the rest).
_doc_parse() {
    _DOC_ARGS=()
    local host="" token="" token_stdin=0 token_flag=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _DOC_JSON=1; shift ;;
            -w|--workspace) [ $# -ge 2 ] || _doc_die usage "$1 needs a workspace"; _DOC_WS="$2"; shift 2 ;;
            --workspace=*) _DOC_WS="${1#*=}"; shift ;;
            --on-behalf-of) [ $# -ge 2 ] || _doc_die usage "--on-behalf-of needs a person"; _DOC_OB="$2"; shift 2 ;;
            --on-behalf-of=*) _DOC_OB="${1#*=}"; shift ;;
            --fence) _DOC_FENCE=1; shift ;;
            --no-fence) _DOC_FENCE=0; shift ;;
            --text) [ $# -ge 2 ] || _doc_die usage "--text needs a value"; _DOC_TEXT="$2"; _DOC_TEXT_SET=1; shift 2 ;;
            --text=*) _DOC_TEXT="${1#*=}"; _DOC_TEXT_SET=1; shift ;;
            --host) [ $# -ge 2 ] || _doc_die usage "--host needs a server"; host="$2"; shift 2 ;;
            --host=*) host="${1#*=}"; shift ;;
            --token-stdin) token_stdin=1; shift ;;
            --token) [ $# -ge 2 ] || _doc_die usage "--token needs a value"; token="$2"; token_flag="$1"; shift 2 ;;
            --) shift; while [ $# -gt 0 ]; do _DOC_ARGS+=("$1"); shift; done ;;
            *) _DOC_ARGS+=("$1"); shift ;;
        esac
    done
    _doc_token_stdin "$token_stdin" "$token_flag"
    [ "$token_stdin" = 1 ] && token="$_DOC_TOKEN"
    _DOC_TOKEN=""
    if [ -n "$host" ]; then
        _apply_remote_host "$host" "$token" || exit 1
    fi
}

# The last argument is the doc; the rest go to the tool as given.
_doc_tool() {
    local verb="$1"; shift
    _doc_parse "$@"
    local n=${#_DOC_ARGS[@]}
    [ "$n" -ge 1 ] || _doc_die usage "name the doc: k2 doc $verb [flags] <file.md>"
    local path="${_DOC_ARGS[$((n - 1))]}"
    case "$path" in -*) _doc_die usage "the doc's path comes last: k2 doc $verb [flags] <file.md>" ;; esac
    local -a argv=("$verb")
    local i
    for ((i = 0; i < n - 1; i++)); do argv+=("${_DOC_ARGS[$i]}"); done
    path="$(_doc_path "$path")"
    _doc_tmp
    _DOC_JSON="${_DOC_JSON:-0}" python3 -c "$_DOC_TOOL_PY" "$_DOC_WS" "$_DOC_OB" "$_DOC_TEXT_SET" "$_DOC_TEXT" "$path" ${argv[@]+"${argv[@]}"} >"$_DOC_TMPD/tool.json" || exit $?
    _doc_call_file POST /cli/docs/tool "" "$_DOC_TMPD/tool.json"
    _doc_print_tool
}

# Any-file doc verbs the docs kit runs itself (BlobSession::run_tool):
# comments on any file; on PSD/PSB files the layer index and claims by
# layer. The doc is the last argument that isn't a flag's value; K2's own
# flags (-w, --on-behalf-of, --host, --token, --fence, --no-fence,
# --read-budget) are K2's, and everything else goes to the kit as typed,
# in order (--json and --text included: the kit prints the JSON page).
_DOC_KIT_VALUE_FLAGS=" --body --note --hint --mention --proposal --cursor --kind --under --version --depth --ttl --id --node --on --since --after --limit --n "
_doc_blob_tool() {
    local verb="$1"; shift
    local -a k2args=() toks=()
    local last=-1 budget=""
    while [ $# -gt 0 ]; do
        case "$1" in
            -w|--workspace|--on-behalf-of|--host|--token)
                [ $# -ge 2 ] || _doc_die usage "$1 needs a value"
                k2args+=("$1" "$2"); shift 2 ;;
            --token-stdin) k2args+=("$1"); shift ;;
            --workspace=*|--on-behalf-of=*|--host=*|--fence|--no-fence) k2args+=("$1"); shift ;;
            --read-budget)
                [ $# -ge 2 ] || _doc_die usage "--read-budget needs a number of bytes"
                budget="$2"; shift 2 ;;
            --read-budget=*) budget="${1#*=}"; shift ;;
            --) shift
                while [ $# -gt 0 ]; do toks+=("$1"); last=$((${#toks[@]} - 1)); shift; done ;;
            -*)
                if [[ "$_DOC_KIT_VALUE_FLAGS" == *" $1 "* ]]; then
                    [ $# -ge 2 ] || _doc_die usage "$1 needs a value (k2 doc --help)"
                    toks+=("$1" "$2"); shift 2
                else
                    toks+=("$1"); shift
                fi ;;
            *) toks+=("$1"); last=$((${#toks[@]} - 1)); shift ;;
        esac
    done
    [ "$last" -ge 0 ] || _doc_die usage "name the doc last: k2 doc $verb [flags] <file>"
    case "$budget" in *[!0-9]*) _doc_die usage "--read-budget takes a number of bytes (at least 16384)" ;; esac
    local path="${toks[$last]}"
    unset 'toks[last]'
    _doc_parse ${k2args[@]+"${k2args[@]}"}
    path="$(_doc_path "$path")"
    local argv
    argv="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "$verb" ${toks[@]+"${toks[@]}"})"
    _doc_tmp
    if [ -n "$budget" ]; then
        _doc_body "$path" argv "$argv" @readBudget "@int:$budget" >"$_DOC_TMPD/tool.json" </dev/null
    else
        _doc_body "$path" argv "$argv" >"$_DOC_TMPD/tool.json" </dev/null
    fi
    _doc_call_file POST /cli/docs/tool "" "$_DOC_TMPD/tool.json"
    _doc_print_tool
}

# `claim` on a .psd/.psb file, or with --node: a claim by layer through the
# kit (`--ttl` as a duration, sent as milliseconds).
_doc_node_claim() {
    local -a args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --ttl|--ttl=*)
                local ttl
                if [ "$1" = --ttl ]; then _need_value "$@"; ttl="${2:-}"; shift 2; else ttl="${1#*=}"; shift; fi
                ttl="$(python3 -c '
import re, sys
m = re.fullmatch(r"(\d+)(ms|s|m)?", sys.argv[1].strip())
if not m or int(m.group(1)) == 0:
    sys.exit(1)
print(int(m.group(1)) * {None: 1, "ms": 1, "s": 1000, "m": 60000}[m.group(2)])
' "$ttl")" || _doc_die usage "--ttl takes a duration: 30s, 5m, or milliseconds"
                args+=(--ttl "$ttl") ;;
            *) args+=("$1"); shift ;;
        esac
    done
    _doc_blob_tool claim ${args[@]+"${args[@]}"}
}

_doc_cad_usage() {
    cat <<'USAGE'
k2 doc cad <read verb> [args] <drawing>   a .dxf or .dwg drawing's reads (the CAD core)

Each read is of the drawing's current version (a save from your CAD app is
taken in first). It never opens the drawing for anyone: no presence.

  drawing info                 version, units, extents, counts, handle health
  layer list [--unused]        each layer with its switches and its entities
  block list [--unused] [--anon]   each block with its base point and inserts
  entity find [--layer L] [--kind K] [--in model|paper|@h:B]
              [--window x0,y0,x1,y1 | --crossing x0,y0,x1,y1] [--after @h:H] [--limit N]
  text find --contains S [--layer L] [--case] [--after @h:H] [--limit N]
  attrib find ...  block show @h:B  entity props @h:H
  draw header                  everything a viewer needs before drawing
  draw page --space model|@h:B [--cursor C]   one page of a space, ready to draw
  header get $VAR...           (a .dxf) header variables as written
  check [--writeback]          (a .dxf) structure and reference findings

A read that has more prints the next call on stderr ("more: ..."). --json
prints the whole answer: path, version, verb, shape, stdout, data, next.
A .dxf changes through its edit verbs (k2 doc --help); whole-file saves of
a drawing, and a .dwg, are refused (cad_writer_disabled, exit 3).
Exit: 0 ok · 2 bad usage, a drawing that doesn't read, or a stale cursor
(the drawing changed: start again) · 3 refused (core_off: the CAD core is
off) · 4 an address that names nothing.
USAGE
}

# `k2 doc cad <read verb> [args] <drawing>`: GET /cli/docs/cad/read. The
# drawing is the last argument; the rest go to the pack as typed (K2's own
# flags aside).
_doc_cad() {
    local -a k2args=() toks=()
    local last=-1
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) _doc_cad_usage; return 0 ;;
            -w|--workspace|--on-behalf-of|--host|--token)
                [ $# -ge 2 ] || _doc_die usage "$1 needs a value"
                k2args+=("$1" "$2"); shift 2 ;;
            --token-stdin) k2args+=("$1"); shift ;;
            --workspace=*|--on-behalf-of=*|--host=*|--json) k2args+=("$1"); shift ;;
            --) shift
                while [ $# -gt 0 ]; do toks+=("$1"); last=$((${#toks[@]} - 1)); shift; done ;;
            -*) toks+=("$1"); shift ;;
            *) toks+=("$1"); last=$((${#toks[@]} - 1)); shift ;;
        esac
    done
    [ "$last" -ge 1 ] || _doc_die usage "k2 doc cad <read verb> [args] <drawing>  (k2 doc cad --help)"
    local path="${toks[$last]}"
    unset 'toks[last]'
    _doc_parse ${k2args[@]+"${k2args[@]}"}
    path="$(_doc_path "$path")"
    local argv q
    argv="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' ${toks[@]+"${toks[@]}"})"
    q="$(_doc_query "$path" argv "$argv")"
    _doc_call GET /cli/docs/cad/read "$q"
    _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
out = v.get("stdout") or ""
if out:
    sys.stdout.write(out if out.endswith("\n") else out + "\n")
nxt = list(v.get("next") or [])
while "--version" in nxt:
    i = nxt.index("--version")
    del nxt[i:i + 2]
if nxt:
    print("more: k2 doc cad %s %s" % (" ".join(nxt), v.get("path", "")), file=sys.stderr)
'
}

_doc_av_usage() {
    cat <<'USAGE'
k2 doc av read|summary --config <file> <drawing|folder>...   the AV reading of a set (the CAD core)

Reads a set of AV system drawings (.dxf, .dwg) as a technician does: devices
across sheets, cables and the boundaries they are pulled through, off-sheet
flag pairs, title blocks and the sheet index, with findings for what doesn't
hold together. A folder adds every .dxf and .dwg under it. At most 200
drawings; each needs the read right, as does the configuration.

  --config <file>   the set's conventions, one key = value per line (attribute
                    tags, layers, linetypes, block names); nothing about a
                    project is built in
  read              every row (identifiers, labels and sheet numbers included)
  summary           counts and finding codes only, never a value: what you can
                    pass on about a private set

--json prints the whole answer (read: sheets_read, shape av.set/1, data).
Exit: 0 ok · 2 bad usage, a bad configuration or a drawing that doesn't read
· 3 refused (core_off: the CAD core is off) · 4 a drawing or the
configuration isn't there.
USAGE
}

# `k2 doc av read|summary --config <file> <drawing|folder>...`:
# GET /cli/docs/av/read.
_doc_av() {
    local what="${1:-}"
    case "$what" in
        read|summary) shift ;;
        -h|--help) _doc_av_usage; return 0 ;;
        *) _doc_die usage "k2 doc av read|summary --config <file> <drawing|folder>...  (k2 doc av --help)" ;;
    esac
    local config=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) _doc_av_usage; return 0 ;;
            --config) [ $# -ge 2 ] || _doc_die usage "--config needs a file"; config="$2"; shift 2 ;;
            --config=*) config="${1#*=}"; shift ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    [ -n "$config" ] || _doc_die usage "k2 doc av $what needs --config <file>"
    _doc_parse ${rest[@]+"${rest[@]}"}
    [ ${#_DOC_ARGS[@]} -ge 1 ] || _doc_die usage "name the drawings (or a folder of them): k2 doc av $what --config <file> <drawing|folder>..."
    local -a paths=()
    local a f
    for a in "${_DOC_ARGS[@]}"; do
        if [ -d "$a" ]; then
            while IFS= read -r f; do
                paths+=("$(_doc_path "$f")")
            done < <(find "$a" -type f \( -iname '*.dxf' -o -iname '*.dwg' \) | LC_ALL=C sort)
        else
            paths+=("$(_doc_path "$a")")
        fi
    done
    [ ${#paths[@]} -ge 1 ] || _doc_die usage "no .dxf or .dwg drawing there"
    config="$(_doc_path "$config")"
    local pj q
    pj="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${paths[@]}")"
    q="$(_doc_query "" config "$config" paths "$pj" what "$what")"
    _doc_call GET /cli/docs/av/read "$q"
    _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
out = (v.get("summary") if v.get("what") == "summary" else v.get("stdout")) or ""
if out:
    sys.stdout.write(out if out.endswith("\n") else out + "\n")
'
}

# The owner's comment purge and participant erasure on an any-file doc.
_doc_owner_purge() {
    local verb="$1"; shift
    local thread="" n="" text="" all=0 who="" pseudo=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --thread) _need_value "$@"; thread="${2:-}"; shift 2 ;;
            --n) _need_value "$@"; n="${2:-}"; shift 2 ;;
            --text) _need_value "$@"; text="${2:-}"; shift 2 ;;
            --all) all=1; shift ;;
            --participant) _need_value "$@"; who="${2:-}"; shift 2 ;;
            --pseudonym) _need_value "$@"; pseudo="${2:-}"; shift 2 ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    case "$n" in *[!0-9]*) _doc_die usage "--n takes a comment number" ;; esac
    _doc_parse ${rest[@]+"${rest[@]}"}
    _doc_one_path "$verb"; local path="$_DOC_PATH"
    local -a kv=()
    if [ "$verb" = purge-comments ]; then
        [ -n "$thread" ] && kv+=(thread "$thread")
        [ -n "$n" ] && kv+=(@n "@int:$n")
        [ -n "$text" ] && kv+=(text "$text")
        [ "$all" = 1 ] && kv+=(@all "@true")
        [ ${#kv[@]} -gt 0 ] || _doc_die usage "k2 doc purge-comments --thread ann-N [--n N] | --text \"...\" | --all <file>"
        _doc_call POST /cli/docs/comments/purge "$(_doc_body "$path" ${kv[@]+"${kv[@]}"} </dev/null)"
    else
        [ -n "$who" ] && [ -n "$pseudo" ] || _doc_die usage "k2 doc erase-participant --participant P --pseudonym NAME <file>"
        _doc_call POST /cli/docs/erase "$(_doc_body "$path" participant "$who" pseudonym "$pseudo" </dev/null)"
    fi
    _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
c = v.get("counts") or {}
if v.get("erased"):
    print("erased %s as %s from %s (%s attributions, %s comments cleared)" % (v["erased"], v.get("pseudonym"), v["doc"]["path"], c.get("attributions", 0), c.get("comments", 0)))
else:
    print("purged %s comment(s) and %s pin copies from %s" % (c.get("comments", 0), c.get("anchors", 0), v["doc"]["path"]))
print("store file rewritten" if v.get("storeRewritten") else "the store file was NOT rewritten: purged bytes stay in freed pages until the next rewrite", file=sys.stderr)
for n in v.get("notReached") or []:
    print("not reached: %s%s" % (n.get("describe"), (" (" + n["k2"] + ")") if n.get("k2") else ""), file=sys.stderr)
'
}

# One positional (the doc) for the plain verbs → _DOC_PATH (called in the
# main shell, so a usage error exits the CLI).
_doc_one_path() {
    local verb="$1"
    [ ${#_DOC_ARGS[@]} -eq 1 ] || _doc_die usage "k2 doc $verb takes one doc: k2 doc $verb [flags] <file.md>"
    _DOC_PATH="$(_doc_path "${_DOC_ARGS[0]}")"
}

_doc_print_json_or() {
    if [ "${_DOC_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_DOC_BODY"
        return 0
    fi
    printf '%s' "$_DOC_BODY" | python3 -c "$1" "${@:2}"
}

_doc_who() {
    local follow="${1:-}"; shift || true
    local path q
    if [ -n "$follow" ]; then
        [ ${#_DOC_ARGS[@]} -eq 2 ] || _doc_die usage "k2 doc follow <name> <file.md>"
        path="$(_doc_path "${_DOC_ARGS[1]}")"
    else
        _doc_one_path who; path="$_DOC_PATH"
    fi
    q="$(_doc_query "$path")"
    _doc_call GET /cli/docs/who "$q"
    _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
follow = sys.argv[1]
rows = v.get("roster") or []
if follow:
    f = follow.lower()
    hit = [r for r in rows if f in (r.get("name") or "").lower() or r.get("pid") == follow]
    if not hit:
        print("Error: %s is not in this doc" % follow, file=sys.stderr)
        sys.exit(4)
    r = hit[0]
    c = r.get("cursor") or {}
    vw = r.get("view") or {}
    print("%s %s  caret %s  viewing %s%s" % (r.get("name"), r.get("role"), c.get("head") or "-",
          ("%s-%s" % (vw["first"], vw["last"])) if vw else (r.get("region") or "-"),
          "  holds the lease" if r.get("holdsLease") else ""))
    sys.exit(0)
if not rows:
    print("Nobody is here.")
for r in rows:
    c = r.get("cursor") or {}
    vw = r.get("view") or {}
    tag = "%s (%s)" % (r.get("name"), r.get("kind"))
    extra = []
    if c.get("head"):
        extra.append("caret %s" % c["head"])
    if vw:
        extra.append("viewing %s-%s" % (vw["first"], vw["last"]))
    if r.get("region"):
        extra.append("viewing %s" % r["region"])
    for cl in r.get("claims") or []:
        extra.append("claimed %s-%s%s (%ds left)" % (cl["lines"][0], cl["lines"][1],
                     (" \"%s\"" % cl["note"]) if cl.get("note") else "", int(cl["remainingMs"] / 1000)))
    if r.get("lastEdit"):
        extra.append("last edit at lines %s-%s" % tuple(r["lastEdit"]))
    if r.get("holdsLease"):
        extra.append("holds the lease")
    if r.get("away"):
        extra.append("away")
    if r.get("connections", 1) > 1:
        extra.append("%d connections" % r["connections"])
    print("%-32s %-7s %s" % (tag, r.get("role"), "  ".join(extra)))
' "$follow"
}

# watch / tail -f: stay present, and print each new version's changes as a
# unified diff with who made it. Polls once a second; a refusal (rights
# revoked) ends it with exit 3 within the next poll (K-PERM-12).
_doc_watch() {
    _doc_one_path watch; local path="$_DOC_PATH"
    _doc_call POST /cli/docs/open "$(_doc_body "$path")"
    trap 'cli_send_json POST /cli/docs/leave "$(_doc_body "'"$path"'")" >/dev/null 2>&1; exit 0' INT TERM
    local last="" body
    if [ "$(printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("tier", 1))')" = "0" ]; then
        _doc_watch_any "$path"
        return
    fi
    _doc_call POST /cli/docs/tool "$(_doc_body "$path" argv '["cat"]')"
    last="$_DOC_BODY"
    printf '%s' "$last" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("watching %s at v%s (Ctrl-C to stop)" % (v["doc"]["path"], v.get("version")), file=sys.stderr)'
    while :; do
        sleep 1
        _doc_call GET /cli/docs/status "$(_doc_query "$path")"
        if printf '%s\n%s' "$last" "$_DOC_BODY" | python3 -c '
import json, sys
a, b = sys.stdin.read().split("\n", 1)
sys.exit(0 if json.loads(a).get("version") == json.loads(b).get("version") else 1)
'; then
            continue
        fi
        _doc_call POST /cli/docs/tool "$(_doc_body "$path" argv '["cat"]')"
        body="$_DOC_BODY"
        printf '%s\n%s' "$last" "$body" | python3 -c '
import difflib, json, sys
a, b = sys.stdin.read().split("\n", 1)
a, b = json.loads(a), json.loads(b)
old, new = a.get("stdout") or "", b.get("stdout") or ""
f = b.get("fence") or {}
who = ""
begin = f.get("begin") or ""
for line in begin.splitlines():
    if "contributors since you last read:" in line:
        who = line.split("contributors since you last read:", 1)[1].strip(" ┄")
print("── v%s → v%s%s" % (a.get("version"), b.get("version"), (" by " + who) if who else ""))
for l in difflib.unified_diff(old.splitlines(True), new.splitlines(True), "v%s" % a.get("version"), "v%s" % b.get("version")):
    sys.stdout.write(l if l.endswith("\n") else l + "\n")
sys.stdout.flush()
'
        last="$body"
    done
}

# watch on an any-file doc (tier 0): each new version (why, by whom, its
# size) and each change of the lease, from `status` once a second.
_doc_watch_any() {
    local path="$1" last
    _doc_call GET /cli/docs/status "$(_doc_query "$path")"
    last="$_DOC_BODY"
    printf '%s' "$last" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("watching %s at v%s (Ctrl-C to stop)" % (v["doc"]["path"], v.get("version")), file=sys.stderr)'
    while :; do
        sleep 1
        _doc_call GET /cli/docs/status "$(_doc_query "$path")"
        printf '%s\n%s' "$last" "$_DOC_BODY" | python3 -c '
import json, sys
a, b = sys.stdin.read().split("\n", 1)
a, b = json.loads(a), json.loads(b)
if a.get("version") != b.get("version"):
    c = b.get("current") or {}
    extra = (" (kept the replaced one as \"%s\")" % c["conflictCopy"]) if c.get("conflictCopy") else ""
    print("── v%s → v%s: %s by %s, %s bytes%s" % (a.get("version"), b.get("version"), c.get("reason"),
          c.get("authorLabel") or c.get("author"), c.get("size"), extra))
la, lb = (a.get("lease") or {}).get("pid"), (b.get("lease") or {}).get("pid")
if la != lb:
    print("── lease: %s" % (("held by " + b["lease"]["label"]) if lb else "free"))
sys.stdout.flush()
'
        last="$_DOC_BODY"
    done
}

# An undo / redo / revert answer: what applied, what was skipped (exit 1).
_doc_print_undo() {
    local verb="$1"
    if [ "${_DOC_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_DOC_BODY"
        python3 -c 'import json,sys; sys.exit(int(json.loads(sys.stdin.read()).get("exit", 0)))' <<<"$_DOC_BODY"
        exit $?
    fi
    printf '%s' "$_DOC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
verb = sys.argv[1]
if v.get("nothing"):
    print("nothing to %s" % verb, file=sys.stderr)
elif v.get("historyLost") or v.get("clearedByPurge"):
    print(v.get("hint", "no undo history"), file=sys.stderr)
elif v.get("dropped") is not None:
    print("dropped the top step (change %s); nothing applied" % v["dropped"], file=sys.stderr)
else:
    r = v.get("result") or {}
    print("%s %s step(s) (+%s -%s) · v%s" % (verb, v.get("steps", 1), r.get("linesAdded", 0), r.get("linesRemoved", 0), r.get("version")), file=sys.stderr)
    for c in v.get("skipped") or []:
        ln = c.get("lines") or ["?", "?"]
        print("skipped lines %s-%s (%s): changed by someone since" % (ln[0], ln[1], c.get("kind")), file=sys.stderr)
    if v.get("blocked"):
        print("stopped: the next step overlaps later edits", file=sys.stderr)
sys.exit(int(v.get("exit", 0)))
' "$verb"
    exit $?
}

# History reads and writes: versions, show, restore, version --name, revert.
_doc_history() {
    local verb="$1"; shift
    local ver="" name="" partial=@false out=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --version) _need_value "$@"; ver="${2:-}"; shift 2 ;;
            --version=*) ver="${1#*=}"; shift ;;
            --name) _need_value "$@"; name="${2:-}"; shift 2 ;;
            --name=*) name="${1#*=}"; shift ;;
            --partial) partial=@true; shift ;;
            --out|-o) [ "$verb" = show ] || _doc_die usage "--out is for k2 doc show"; out="${2:-}"; [ -n "$out" ] || _doc_die usage "--out needs a file"; shift 2 ;;
            --out=*) [ "$verb" = show ] || _doc_die usage "--out is for k2 doc show"; out="${1#*=}"; shift ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    case "$ver" in *[!0-9]*) _doc_die usage "--version takes a version id (k2 doc versions)" ;; esac
    _doc_parse ${rest[@]+"${rest[@]}"}
    local path change=""
    if [ "$verb" = revert ]; then
        [ ${#_DOC_ARGS[@]} -eq 2 ] || _doc_die usage "k2 doc revert <change> [--partial] <file.md>"
        change="${_DOC_ARGS[0]}"
        case "$change" in ''|*[!0-9]*) _doc_die usage "the change is a number (k2 doc log)" ;; esac
        path="$(_doc_path "${_DOC_ARGS[1]}")"
    else
        _doc_one_path "$verb"; path="$_DOC_PATH"
    fi
    case "$verb" in
        versions)
            _doc_call GET /cli/docs/versions "$(_doc_query "$path")"
            _doc_print_json_or '
import json, sys, time
v = json.load(sys.stdin)
def size(n):
    n = n or 0
    return "%d B" % n if n < 1024 else ("%.1f KiB" % (n / 1024) if n < 1048576 else "%.1f MiB" % (n / 1048576))
for x in v.get("versions") or []:
    t = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime((x.get("atMs") or 0) / 1000))
    name = (" \"%s\"" % x["name"]) if x.get("name") else ""
    if v.get("tier") == 0:
        extra = ""
        if x.get("restoredFrom") is not None:
            extra += " (from version %s)" % x["restoredFrom"]
        if x.get("conflictCopy"):
            extra += " (kept the replaced one as \"%s\")" % x["conflictCopy"]
        if x.get("redacted"):
            extra += " [purged]"
        print("%-6s %s  %-8s %10s  %s via %s%s%s%s" % (x.get("id"), t, x.get("reason"), size(x.get("size")),
              x.get("authorLabel") or x.get("author"), x.get("via"), name, extra, "  (current)" if x.get("current") else ""))
    else:
        print("%-6s %s  at change %-6s %-16s%s  %s" % (x.get("id"), t, x.get("version"), x.get("reason"), name, ", ".join(x.get("authors") or [])))
' ;;
        show)
            [ -n "$ver" ] || _doc_die usage "k2 doc show --version V [--out FILE] <file>"
            _doc_call_file GET /cli/docs/show "$(_doc_query "$path" version "$ver")" ""
            if [ "${_DOC_JSON:-0}" = "1" ]; then cat "$_DOC_OUT"; echo; return 0; fi
            # Any file's bytes: to --out, or stdout; never binary onto a
            # terminal (pipe it, or name a file).
            python3 -c '
import base64, json, sys
v = json.load(open(sys.argv[1], encoding="utf-8"))
out, tty = sys.argv[2], sys.argv[3] == "1"
data = base64.b64decode(v["bytesB64"]) if "bytesB64" in v else (v.get("text") or "").encode("utf-8")
if out:
    with open(out, "wb") as f:
        f.write(data)
    print("wrote version %s of %s (%d bytes) to %s" % (v.get("version"), v["doc"]["path"], len(data), out), file=sys.stderr)
    sys.exit(0)
if tty:
    binary = b"\0" in data
    if not binary:
        try:
            data.decode("utf-8")
        except UnicodeDecodeError:
            binary = True
    if binary:
        hint = "version %s of %s is binary (%d bytes); write it to a file with --out FILE, or pipe it" % (v.get("version"), v["doc"]["path"], len(data))
        print("Error: " + hint, file=sys.stderr)
        print(json.dumps({"error": {"code": "binary_output", "hint": hint}}), file=sys.stderr)
        sys.exit(2)
sys.stdout.buffer.write(data)
sys.stdout.flush()
' "$_DOC_OUT" "$out" "$([ -t 1 ] && echo 1 || echo 0)"
            exit $? ;;
        restore)
            [ -n "$ver" ] || _doc_die usage "k2 doc restore --version V <file>"
            _doc_call POST /cli/docs/restore "$(_doc_body "$path" @version "@int:$ver")"
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
r = v.get("result") or {}
if v.get("tier") == 0:
    if r.get("changed"):
        print("restored version %s as version %s (%s bytes)%s" % (v.get("restored"), r.get("version"), r.get("size"),
              "" if r.get("written") else "; the file is written back shortly"), file=sys.stderr)
    else:
        print("version %s already holds those bytes; nothing new recorded" % r.get("version"), file=sys.stderr)
else:
    print("restored version %s as a new change (+%s -%s) · v%s; k2 doc undo reverses it" % (v.get("restored"), r.get("linesAdded", 0), r.get("linesRemoved", 0), r.get("version")), file=sys.stderr)
' ;;
        version)
            [ -n "$name" ] || _doc_die usage "k2 doc version --name \"<name>\" [--version V] <file.md>"
            if [ -n "$ver" ]; then
                _doc_call POST /cli/docs/name "$(_doc_body "$path" name "$name" @version "@int:$ver")"
            else
                _doc_call POST /cli/docs/name "$(_doc_body "$path" name "$name")"
            fi
            _doc_print_json_or 'import json,sys; v=json.load(sys.stdin); print("version %s named \"%s\"" % (v.get("version"), v.get("name")))' ;;
        revert)
            _doc_call POST /cli/docs/revert "$(_doc_body "$path" @change "@int:$change" @partial "$partial")"
            _doc_print_undo revert ;;
    esac
}

# `diff --version V [--to W] [-U N]`: a unified diff between versions (or a
# version and the live doc). Exit 1 when they differ.
_doc_history_diff() {
    local from="" to="" ctx=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --version) _need_value "$@"; from="${2:-}"; shift 2 ;;
            --version=*) from="${1#*=}"; shift ;;
            --to) _need_value "$@"; to="${2:-}"; shift 2 ;;
            -U) _need_value "$@"; ctx="${2:-}"; shift 2 ;;
            -u) shift ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    case "$from$to$ctx" in *[!0-9]*) _doc_die usage "--version, --to and -U take numbers" ;; esac
    _doc_parse ${rest[@]+"${rest[@]}"}
    _doc_one_path diff; local path="$_DOC_PATH"
    _doc_call GET /cli/docs/diff "$(_doc_query "$path" fromVersion "$from" toVersion "$to" context "$ctx")"
    if [ "${_DOC_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_DOC_BODY"
    else
        printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin).get("text", ""))'
    fi
    python3 -c 'import json,sys; sys.exit(int(json.loads(sys.stdin.read()).get("exit", 0)))' <<<"$_DOC_BODY"
    exit $?
}

# `k2 doc export` (prd-k2-core-exports-v1 §4.3, vs-live E3): a doc's
# version as a file, through the kit's export verb (P24). The daemon runs
# the job; this polls it (progress on a TTY), cancels it on Ctrl-C, and for
# an --out outside every workspace (or any --host call) writes the bytes
# itself from `export/fetch`, never over an existing file.
_doc_export() {
    local to="" ver="" out="" list=0 node=""
    local -a opts=() rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --list) list=1; shift ;;
            --node) [ $# -ge 2 ] || _doc_die usage "--node needs a layer address (k2 doc outline <file>)"; node="$2"; shift 2 ;;
            --node=*) node="${1#*=}"; shift ;;
            --to) [ $# -ge 2 ] || _doc_die usage "--to needs a target (k2 doc export --list <file> shows them)"; to="$2"; shift 2 ;;
            --to=*) to="${1#*=}"; shift ;;
            --version) [ $# -ge 2 ] || _doc_die usage "--version needs a version id (k2 doc versions <file>)"; ver="$2"; shift 2 ;;
            --version=*) ver="${1#*=}"; shift ;;
            --option|-O) [ $# -ge 2 ] || _doc_die usage "--option needs NAME=VALUE"; opts+=("$2"); shift 2 ;;
            --option=*) opts+=("${1#*=}"); shift ;;
            --out|-o) [ $# -ge 2 ] || _doc_die usage "--out needs a file"; out="$2"; shift 2 ;;
            --out=*) out="${1#*=}"; shift ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    case "$ver" in *[!0-9]*) _doc_die usage "--version takes a version id (the first column of k2 doc versions <file>)" ;; esac
    _doc_parse ${rest[@]+"${rest[@]}"}
    _doc_one_path export; local path="$_DOC_PATH"
    if [ "$list" = 1 ]; then
        [ -z "$to$ver$out" ] && [ ${#opts[@]} -eq 0 ] || _doc_die usage "k2 doc export --list <file> takes no other export flags"
        _doc_call GET /cli/docs/export/targets "$(_doc_query "$path")"
        _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
rows = v.get("targets") or []
print("%-14s %-9s %-7s %-7s %s" % ("TARGET", "FIDELITY", "EXT", "RUNS", "OPTIONS"))
for t in rows:
    opts = []
    for o in t.get("options") or []:
        k = o.get("kind") or {}
        kind = k.get("type", "")
        if kind == "int":
            kind = "%s..%s" % (k.get("min"), k.get("max"))
        elif kind == "choice":
            kind = "|".join(k.get("values") or [])
        d = (" (default %s)" % o["default"]) if o.get("default") is not None else ""
        opts.append("%s=%s%s" % (o.get("name"), kind, d))
    print("%-14s %-9s %-7s %-7s %s" % (t.get("id"), t.get("fidelity"), t.get("extension") or "-", t.get("runs"), ", ".join(opts) or "-"))
w = v.get("worker") or {}
if not w.get("available"):
    print("note: the export worker is not available here (%s); only targets run by the daemon are listed" % w.get("reason"), file=sys.stderr)
'
        return 0
    fi
    [ -n "$to" ] || _doc_die usage "k2 doc export --to <target> [--version V] [--option NAME=VALUE]... [--out FILE] <file>  (k2 doc export --list <file> shows the targets)"
    # --out: from where you stand (absolute), unless -w names the workspace
    # it is relative to. On --host it is a path on THIS machine: the CLI
    # writes it (fetch).
    local remote=0 local_out="" send_out=""
    [ "${_K2_FORCE_TCP:-0}" = "1" ] && remote=1
    if [ -n "$out" ]; then
        if [ "$remote" = 1 ]; then
            local_out="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$out")"
        elif [ -n "$_DOC_WS" ] && [ "${out#/}" = "$out" ]; then
            send_out="$out"
        else
            local_out="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$out")"
            send_out="$local_out"
        fi
        if [ -n "$local_out" ] && { [ -e "$local_out" ] || [ -L "$local_out" ]; }; then
            _doc_die exists "$out already exists; an export never overwrites a file (name another one)" 2
        fi
    fi
    _doc_tmp
    python3 - "$_DOC_WS" "$_DOC_OB" "$path" "$to" "$ver" "$send_out" "$([ "$remote" = 1 ] && [ -n "$out" ] && echo 1 || echo 0)" "$node" ${opts[@]+"${opts[@]}"} >"$_DOC_TMPD/export.json" <<'PY' || _doc_die usage "couldn't build the request"
import json, sys
ws, ob, path, to, ver, out, fetch, node, opts = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5], sys.argv[6], sys.argv[7] == "1", sys.argv[8], sys.argv[9:]
# A short first wait: the poll loop (where Ctrl-C / SIGTERM cancel) takes over.
d = {"path": path, "target": to, "options": opts, "waitMs": 2000}
if ws:
    d["workspace"] = ws
if ob:
    d["onBehalfOf"] = ob
if ver:
    d["version"] = int(ver)
if out:
    d["out"] = out
if fetch:
    d["fetch"] = True
if node:
    d["node"] = node
print(json.dumps(d))
PY
    _doc_call_file POST /cli/docs/export "" "$_DOC_TMPD/export.json"
    _DOC_BODY="$(cat "$_DOC_OUT")"
    local job state
    job="$(printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("job",""))')"
    state="$(printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state",""))')"
    # Still running after the daemon's wait: poll; Ctrl-C cancels the job.
    if [ "$state" != "done" ]; then
        local _doc_export_cancel=0 tty=0
        [ -t 2 ] && tty=1
        trap '_doc_export_cancel=1' INT TERM
        while [ "$state" != "done" ]; do
            if [ "$_doc_export_cancel" = 1 ]; then
                _doc_call POST /cli/docs/export/cancel "$(_doc_body "" job "$job")" >/dev/null
                _doc_export_cancel=2
            fi
            sleep 0.5
            _doc_call GET /cli/docs/export/status "$(_doc_query "" job "$job")"
            state="$(printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state",""))')"
            if [ "$tty" = 1 ] && [ "$state" != "done" ]; then
                printf '%s' "$_DOC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d, t = v.get("done") or 0, v.get("total") or 0
sys.stderr.write("\rexporting… %d%%  " % (100 * d // t) if t else "\rexporting…  ")
sys.stderr.flush()
'
            fi
        done
        trap - INT TERM
        [ "$tty" = 1 ] && printf '\r\033[K' >&2
    fi
    # Outside every workspace (or --host): the CLI writes the bytes itself,
    # to a temp name beside the file, linked into place only when complete.
    local fetch
    fetch="$(printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; print(1 if json.load(sys.stdin).get("fetch") else 0)')"
    if [ "$fetch" = 1 ]; then
        [ -n "$local_out" ] || _doc_die fetch_without_out "the daemon kept the export for you to fetch, but no --out names where (pass --out FILE)" 2
        local part offset=0 eof=0
        part="$(dirname "$local_out")/.$(basename "$local_out").k2x-$job.part"
        python3 -c 'import sys; open(sys.argv[1], "xb").close()' "$part" 2>/dev/null \
            || _doc_die io "can't write $part (no such folder, or no permission)" 1
        local done_body="$_DOC_BODY"
        while [ "$eof" != 1 ]; do
            _doc_xfer GET /cli/docs/export/fetch "$(_doc_query "" job "$job" offset "$offset")" "" "$_DOC_TMPD/chunk.json"
            if [ "$_DOC_STATUS" != "200" ]; then
                rm -f "$part"
                _doc_fail "$_DOC_STATUS" "$(cat "$_DOC_TMPD/chunk.json")"
            fi
            read -r offset eof < <(python3 -c '
import base64, json, sys
v = json.load(open(sys.argv[1], encoding="utf-8"))
data = base64.b64decode(v["bytesB64"])
with open(sys.argv[2], "ab") as f:
    f.write(data)
print(v["offset"] + len(data), 1 if v.get("eof") else 0)
' "$_DOC_TMPD/chunk.json" "$part") || { rm -f "$part"; _doc_die io "writing $local_out failed" 1; }
        done
        if ! python3 -c '
import os, sys
part, out = sys.argv[1], sys.argv[2]
with open(part, "rb") as f:
    os.fsync(f.fileno())
os.link(part, out)
os.unlink(part)
' "$part" "$local_out" 2>/dev/null; then
            rm -f "$part"
            _doc_die exists "$out appeared while exporting; an export never overwrites a file (name another one)" 2
        fi
        _DOC_BODY="$done_body"
    fi
    if [ "${_DOC_JSON:-0}" = "1" ]; then
        printf '%s' "$_DOC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if sys.argv[1]:
    v["wrote"] = sys.argv[1]
print(json.dumps(v))
' "$local_out"
        return 0
    fi
    printf '%s' "$_DOC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
local = sys.argv[1]
placed = v.get("placed")
where = local if v.get("fetch") else (placed or {}).get("path")
full = local if v.get("fetch") else ((placed or {}).get("abs") if sys.argv[2] == "0" else None) or where
ver = ("version %s" % v["version"]) if v.get("version") is not None else "now"
print("exported %s as %s to %s (%s, doc version %s, %s bytes)" % (v["doc"]["path"], v.get("target"), where, ver, v.get("docVersion"), v.get("size")), file=sys.stderr)
if v.get("codec") not in (None, "exact"):
    print("note: rendered with today'"'"'s line endings and BOM, which an outside save may have changed since that version", file=sys.stderr)
r = v.get("report") or {}
for l in r.get("losses") or []:
    print("note: lost %s: %s" % (l.get("kind"), l.get("detail")), file=sys.stderr)
for n in r.get("notes") or []:
    print("note: %s" % n, file=sys.stderr)
print(full)
' "$local_out" "$remote"
}

_doc_cores_usage() {
    cat <<'USAGE'
%include cli/text/help/doc/cores.txt
USAGE
}

# `k2 doc cores`: Settings → Cores → Multiplayer from the CLI. Listing is
# GET /cli/docs/cores (agents too); on|off writes the box key through
# /cli/settings/update (never a docs route), people only.
_doc_cores() {
    local action="" core=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) _doc_cores_usage; return 0 ;;
            --host|--token) rest+=("$1" "${2:-}"); shift; [ $# -gt 0 ] && shift ;;
            --json|--host=*|--token-stdin) rest+=("$1"); shift ;;
            -*) _doc_die usage "k2 doc cores doesn't take $1 (k2 doc cores --help)" ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    _doc_parse "${rest[@]+"${rest[@]}"}"
    case "${#_DOC_ARGS[@]}" in
        0) ;;
        2) action="${_DOC_ARGS[0]}"; core="${_DOC_ARGS[1]}" ;;
        *) _doc_die usage "k2 doc cores [on|off <core>]  (<core>: markdown | text | any-file | board | slides | timeline | fab | blend | cad)" ;;
    esac
    if [ -z "$action" ]; then
        _doc_call GET /cli/docs/cores ""
        _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
print("Multiplayer cores on this server (the owner or an Admin turns them on or off):")
for c in v.get("cores") or []:
    pill = " (Beta)" if c.get("status") == "beta" else ""
    print("  %-10s %-4s  %s%s: %s" % (c["id"], "on" if c.get("on") else "off", c["name"], pill, c.get("short") or c.get("description") or ""))
    o = c.get("open") or {}
    if c.get("on") and o.get("files"):
        n = o["files"]
        shown = ", ".join(o.get("paths") or [])
        print("  %-10s       open live now: %d file%s%s" % ("", n, "" if n == 1 else "s", " (%s)" % shown if shown else ""))
print("While a core is off, k2 doc refuses its files (core_off, exit 3) and people open them the usual way.")
'
        return 0
    fi
    case "$action" in on|off) ;; *) _doc_die usage "k2 doc cores on|off <core>  (<core>: markdown | text | any-file | board | slides | timeline | fab | blend | cad)" ;; esac
    case "$core" in markdown|text|any-file|board|slides|timeline|fab|blend|cad) ;; *) _doc_die usage "no such core: $core (markdown, text, any-file, board, slides, timeline, fab, blend, cad)" ;; esac
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        [ "${_DOC_JSON:-0}" = "1" ] || echo "Error [person_only]: k2 doc cores on|off is for people, from a terminal outside K2 or Settings → Cores → Multiplayer. An agent can list the cores (k2 doc cores) and asks its human (k2 thread) to turn one on." >&2
        python3 -c 'import json; print(json.dumps({"error": {"code": "person_only", "hint": "k2 doc cores on|off is for people"}}))' >&2
        exit 3
    fi
    local body raw status out
    body="$(python3 -c 'import json,sys; print(json.dumps({"multiplayerCores": {sys.argv[1]: sys.argv[2] == "on"}}))' "$core" "$action")"
    raw="$(cli_send_json POST /cli/settings/update "$body")"
    status="${raw##*$'\n'}"
    out="${raw%$'\n'*}"
    [ "$status" = "$raw" ] && out=""
    [ -n "$status" ] || status="000"
    if [ "$status" != "200" ]; then
        printf '%s' "$out" | python3 -c '
import json, sys
status, as_json = sys.argv[1], sys.argv[2] == "1"
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    v = None
hint = ""
if isinstance(v, dict):
    e = v.get("error")
    if isinstance(e, str):
        hint = e
    elif isinstance(e, dict):
        hint = e.get("hint") or e.get("message") or ""
if not hint:
    hint = raw.strip()[:300] or ("no answer from the K2 daemon" if status == "000" else "the daemon refused (HTTP %s)" % status)
code = {"400": "bad_request", "401": "refused", "403": "refused", "000": "daemon_unreachable"}.get(status, "error")
rc = {"400": 2, "401": 3, "403": 3}.get(status, 1)
if not as_json:
    print("Error: %s" % hint, file=sys.stderr)
print(json.dumps({"error": {"code": code, "hint": hint, "status": int(status) if status.isdigit() else 0}}), file=sys.stderr)
sys.exit(rc)
' "$status" "${_DOC_JSON:-0}"
        exit $?
    fi
    printf '%s' "$out" | python3 -c '
import json, sys
core, action, as_json = sys.argv[1], sys.argv[2], sys.argv[3] == "1"
v = json.load(sys.stdin)
r = v.get("multiplayerCoresReport") or {}
failed = r.get("failed") or []
if as_json:
    print(json.dumps({"multiplayerCores": v.get("multiplayerCores") or {}, "multiplayerCoresReport": r}))
    sys.exit(1 if failed else 0)
# A core with no name here (a newer one) is named by its id: the daemon
# has already made the change, so this line must not fail on it.
name = {"markdown": "The Markdown core", "text": "The text and code core", "any-file": "The any-file core", "board": "The Board core"}.get(core) or "The %s core" % core
changed = any(c.get("core") == core for c in r.get("changed") or [])
if not changed:
    print("%s is already %s." % (name, action))
elif core == "board":
    print("The Board core is on. Board Gardens show their boards and k2 board works." if action == "on" else
          "The Board core is off. Board Gardens show a placeholder and k2 board answers core_off; the boards are kept.")
elif action == "on":
    print("%s is on. Its files are live docs on this server from now on." % name)
else:
    bad = [f.get("path") for f in failed]
    n = int(r.get("closed") or 0) - len(failed)
    paths = [p for p in r.get("paths") or [] if p not in bad]
    if n <= 0 and not failed:
        line = "%s is off. No files were open live." % name
    elif n <= 0:
        line = "%s is off." % name
    else:
        line = "%s is off. %d live file%s written to disk and closed%s." % (
            name, n, " was" if n == 1 else "s were", " (%s)" % ", ".join(paths) if paths else "")
    if failed:
        line += " %d file%s couldn%st be written: %s; %s changes are kept in K2%ss history and are written when the core is turned on again." % (
            len(failed), "" if len(failed) == 1 else "s", chr(39),
            ", ".join("%s (%s)" % (f.get("path"), f.get("error")) for f in failed),
            "its" if len(failed) == 1 else "their", chr(39))
    print(line)
sys.exit(1 if failed else 0)
' "$core" "$action" "${_DOC_JSON:-0}"
    exit $?
}

# A .k5board through k2 doc: the board verbs (the pack's, and K2's new, merge,
# explain and capabilities) go to k2 board's own route, so both commands give
# the same answers, exits and bytes. Every other verb on a .k5board still goes
# to the docs routes, which refuse it (board_file): write --base and restore
# would write bytes past the format's codec.
_DOC_BOARD_VERBS=" new list read search fields templates references add update delete put-back connect react vote comment reply resolve meta lock check-add check-set check-remove table-cell table-add-row table-remove-row table-add-column table-remove-column move order frame-order group ungroup align distribute tidy copy-style duplicate paste connected-copy mind-child template clear-blob apply inverse merge explain capabilities "

# True when `k2 doc <verb> ...` is a board verb on a .k5board.
_doc_board_call() {
    case "$_DOC_BOARD_VERBS" in *" $1 "*) ;; *) return 1 ;; esac
    shift
    local a
    for a in "$@"; do
        case "$(printf '%s' "$a" | tr 'A-Z' 'a-z')" in
            --*) ;;
            *.k5board) return 0 ;;
        esac
    done
    return 1
}

# k2 doc's flags that mean the same for a board: -w, --host, --token-stdin
# and --token.
_doc_board() {
    local verb="$1"; shift
    local host="" token="" token_stdin=0 token_flag=""
    local -a rest=()
    while [ $# -gt 0 ]; do
        case "$1" in
            -w|--workspace) [ $# -ge 2 ] || _doc_die usage "$1 needs a workspace"; rest+=(--workspace "$2"); shift 2 ;;
            --workspace=*) rest+=(--workspace "${1#*=}"); shift ;;
            --host) [ $# -ge 2 ] || _doc_die usage "--host needs a server"; host="$2"; shift 2 ;;
            --host=*) host="${1#*=}"; shift ;;
            --token-stdin) token_stdin=1; shift ;;
            --token) [ $# -ge 2 ] || _doc_die usage "--token needs a value"; token="$2"; token_flag="$1"; shift 2 ;;
            *) rest+=("$1"); shift ;;
        esac
    done
    _doc_token_stdin "$token_stdin" "$token_flag"
    [ "$token_stdin" = 1 ] && token="$_DOC_TOKEN"
    _DOC_TOKEN=""
    if [ -n "$host" ]; then
        _apply_remote_host "$host" "$token" || exit 1
    fi
    cmd_board "$verb" ${rest[@]+"${rest[@]}"}
}

cmd_doc() {
    local verb="${1:-}"
    # `k2 doc cores --help` is its own page.
    if [ "$verb" = cores ]; then
        shift
        _doc_cores "$@"
        return $?
    fi
    if [ -z "$verb" ] || _wants_help "$@"; then
        _doc_usage
        return 0
    fi
    shift
    if _doc_board_call "$verb" "$@"; then
        _doc_board "$verb" "$@"
        return $?
    fi
    case "$verb" in
        cores)
            # Settings → Cores → Multiplayer: which kinds of file are live
            # docs on this server (k2 doc cores --help).
            _doc_cores "$@" ;;
        diff)
            # `diff --version V`: the doc's history; otherwise the text tool
            # (your copy on stdin vs the doc).
            local a hist=0
            for a in "$@"; do case "$a" in --version|--version=*) hist=1 ;; esac; done
            if [ "$hist" = 1 ]; then
                _doc_history_diff "$@"
            else
                _doc_tool diff "$@"
            fi ;;
        cat|head|wc|grep|sed|patch|apply|append|insert|replace|delete)
            _doc_tool "$verb" "$@" ;;
        layer|outline|node|comment|reply|resolve|comments|reanchor|clip|track|marker|media|timeline|feature|hole|object|scene|collection|entity|attrib|block|text)
            # Any-file docs, through the docs kit: comments (any file);
            # PSD/PSB files' layer index (k2 doc --help, "PSD/PSB files");
            # timeline, fab, BLEND and DXF files' index verbs, and a live
            # timeline's or scene's own verbs (the kit routes them by the
            # doc's tier). Every argument goes to the kit as typed.
            _doc_blob_tool "$verb" "$@" ;;
        cad)
            _doc_cad "$@" ;;
        av)
            _doc_av "$@" ;;
        purge-comments|erase-participant)
            _doc_owner_purge "$verb" "$@" ;;
        view)
            # `view --region "page 3"`: what you look at in an any-file doc
            # (tier 0); otherwise the text tool (`view --lines a,b`).
            local a reg="" has_reg=0
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --region) _need_value "$@"; has_reg=1; reg="${2:-}"; shift 2 ;;
                    --region=*) has_reg=1; reg="${1#*=}"; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            if [ "$has_reg" = 1 ]; then
                _doc_parse ${rest[@]+"${rest[@]}"}
                _doc_one_path view; local path="$_DOC_PATH"
                _doc_call POST /cli/docs/view "$(_doc_body "$path" region "$reg")"
                _doc_print_json_or 'import json,sys; v=json.load(sys.stdin); print("viewing %s in %s" % (v.get("region") or "nothing in particular", v["doc"]["path"]))'
            else
                _doc_tool view ${rest[@]+"${rest[@]}"}
            fi ;;
        lease|lock|unlock)
            # Any-file docs (tier 0): one holder at a time; the others'
            # writes exit 5 until it is released or expires.
            local action="acquire" ttl=""
            [ "$verb" = unlock ] && action="release"
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --ttl) _need_value "$@"; ttl="${2:-}"; shift 2 ;;
                    --ttl=*) ttl="${1#*=}"; shift ;;
                    --renew) action="renew"; shift ;;
                    --release) action="release"; shift ;;
                    --break) action="break"; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            local secs=""
            if [ -n "$ttl" ]; then
                secs="$(python3 -c '
import re, sys
m = re.fullmatch(r"(\d+)([smh]?)", sys.argv[1].strip())
if not m or int(m.group(1)) == 0:
    sys.exit(1)
print(int(m.group(1)) * {"": 1, "s": 1, "m": 60, "h": 3600}[m.group(2)])
' "$ttl")" || _doc_die usage "--ttl takes a duration: 90, 90s, 5m, 1h (at most 1h)"
            fi
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path "$verb"; local path="$_DOC_PATH"
            if [ -n "$secs" ]; then
                _doc_call POST /cli/docs/lease "$(_doc_body "$path" action "$action" @ttlSecs "@int:$secs")"
            else
                _doc_call POST /cli/docs/lease "$(_doc_body "$path" action "$action")"
            fi
            _doc_print_json_or '
import json, sys, time
v = json.load(sys.stdin)
a, p = v.get("action"), v["doc"]["path"]
def until(l):
    left = max(0, int((l["expiresMs"] - time.time() * 1000) / 1000))
    return "until %s (%dm %ds)" % (time.strftime("%H:%M:%S", time.localtime(l["expiresMs"] / 1000)), left // 60, left % 60)
if a in ("acquire", "renew"):
    print("%s %s %s; others can read, their writes wait (exit 5)" % ("leased" if a == "acquire" else "renewed the lease on", p, until(v["lease"])))
elif a == "release":
    print(("released the lease on %s" % p) if v.get("released") else ("you held no lease on %s" % p))
else:
    b = v.get("broken")
    print(("broke %s'"'"'s lease on %s" % (b.get("label"), p)) if b else ("nobody held a lease on %s" % p))
sys.exit(int(v.get("exit", 0)))
'
            exit $? ;;
        claim)
            # A .psd/.psb file (or --node): a claim by layer, through the kit.
            local _a _node=0
            for _a in "$@"; do case "$_a" in --node|--node=*) _node=1 ;; esac; done
            case "${@: -1}" in *.[pP][sS][dDbB]) _node=1 ;; esac
            if [ "$_node" = 1 ]; then
                _doc_node_claim "$@"
                return $?
            fi
            # Agent presence: "I'm working on lines a-b" (expires; your own
            # edit there ends it unless --hold). Others' agents' edits into
            # it bounce (exit 5) and wait in line; people are never held.
            local action="take" lines="" ttl="" note="" hold="" id=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --lines) _need_value "$@"; lines="${2:-}"; shift 2 ;;
                    --lines=*) lines="${1#*=}"; shift ;;
                    --ttl) _need_value "$@"; ttl="${2:-}"; shift 2 ;;
                    --ttl=*) ttl="${1#*=}"; shift ;;
                    --note) _need_value "$@"; note="${2:-}"; shift 2 ;;
                    --note=*) note="${1#*=}"; shift ;;
                    --hold) hold=@true; shift ;;
                    --no-hold) hold=@false; shift ;;
                    --id) _need_value "$@"; id="${2:-}"; shift 2 ;;
                    --id=*) id="${1#*=}"; shift ;;
                    --renew) action="renew"; shift ;;
                    --release) action="release"; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            local ms=""
            if [ -n "$ttl" ]; then
                ms="$(python3 -c '
import re, sys
m = re.fullmatch(r"(\d+)(ms|s|m)?", sys.argv[1].strip())
if not m or int(m.group(1)) == 0:
    sys.exit(1)
print(int(m.group(1)) * {None: 1, "ms": 1, "s": 1000, "m": 60000}[m.group(2)])
' "$ttl")" || _doc_die usage "--ttl takes a duration: 30s, 5m, or milliseconds (at most 10m)"
            fi
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path claim; local path="$_DOC_PATH"
            if [ "$action" = take ] && [ -z "$lines" ]; then
                _doc_die usage "k2 doc claim --lines a,b [--ttl 2m] [--note \"what for\"] [--hold] <file.md>"
            fi
            local -a kv=(action "$action")
            [ -n "$lines" ] && kv+=(lines "$lines")
            [ -n "$ms" ] && kv+=(@ttlMs "@int:$ms")
            [ -n "$note" ] && kv+=(note "$note")
            [ -n "$hold" ] && kv+=(@hold "$hold")
            [ -n "$id" ] && kv+=(id "$id")
            _doc_call POST /cli/docs/claim "$(_doc_body "$path" ${kv[@]+"${kv[@]}"})"
            _doc_print_json_or '
import json, sys, time
v = json.load(sys.stdin)
a, p = v.get("action"), v["doc"]["path"]
if a == "release":
    r = v.get("released") or []
    print(("released %s on %s" % (", ".join(r), p)) if r else ("you held no claim on %s" % p))
    sys.exit(int(v.get("exit", 0)))
c = v["claim"]
left = max(0, int(c["remainingMs"] / 1000))
print("%s lines %s-%s of %s as %s for %ds%s%s" % ("claimed" if a == "take" else "renewed", c["lines"][0], c["lines"][1], p, c["id"],
      left, " (held through your edits)" if c.get("hold") else " (your next edit there ends it)",
      (" · %d waiting" % len(c["waiters"])) if c.get("waiters") else ""))
'
            exit $? ;;
        activity)
            local n=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    -n) _need_value "$@"; n="${2:-}"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            case "$n" in *[!0-9]*) _doc_die usage "-n needs a number" ;; esac
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path activity; local path="$_DOC_PATH"
            _doc_call GET /cli/docs/activity "$(_doc_query "$path" n "$n")"
            if [ "${_DOC_JSON:-0}" = "1" ]; then printf '%s\n' "$_DOC_BODY"; return 0; fi
            printf '%s' "$_DOC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
sys.stdout.write(v.get("rendered") or "No activity.\n")
' ;;
        versions|show|restore|version|revert)
            _doc_history "$verb" "$@" ;;
        tail)
            local a f=0
            for a in "$@"; do case "$a" in -f|--follow) f=1 ;; esac; done
            if [ "$f" = 1 ]; then
                local -a rest=()
                for a in "$@"; do case "$a" in -f|--follow) ;; *) rest+=("$a") ;; esac; done
                _doc_parse ${rest[@]+"${rest[@]}"}
                _doc_watch
            else
                _doc_tool tail "$@"
            fi ;;
        watch) _doc_parse "$@"; _doc_watch ;;
        who) _doc_parse "$@"; _doc_who "" ;;
        say)
            local to="" at="" time="" reply=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --to) [ $# -ge 2 ] || _doc_die usage "--to needs a person or agent"; to="$2"; shift 2 ;;
                    --at) [ $# -ge 2 ] || _doc_die usage "--at needs an address"; at="$2"; shift 2 ;;
                    --time) [ $# -ge 2 ] || _doc_die usage "--time needs a time"; time="$2"; shift 2 ;;
                    --reply-to) [ $# -ge 2 ] || _doc_die usage "--reply-to needs a message id"; reply="$2"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            _doc_parse ${rest[@]+"${rest[@]}"}
            [ ${#_DOC_ARGS[@]} -ge 1 ] || _doc_die usage "k2 doc say <file> [--to <who>] [--at <address>] \"text\""
            local path msg
            path="$(_doc_path "${_DOC_ARGS[0]}")"
            if [ ${#_DOC_ARGS[@]} -ge 2 ]; then
                msg="${_DOC_ARGS[*]:1}"
            elif [ "${_DOC_TEXT_SET:-0}" = "1" ]; then
                msg="$_DOC_TEXT"
            else
                _doc_die usage "k2 doc say <file> [--to <who>] \"text\": say what?"
            fi
            # The text rides as --text does (never read as a body flag).
            _DOC_TEXT="$msg"; _DOC_TEXT_SET=1
            _doc_call POST /cli/docs/say "$(_doc_body "$path" @text @text to "$to" at "$at" time "$time" reply_to "$reply")"
            _doc_print_json_or 'import json,sys; v=json.load(sys.stdin)["say"]; print("said %s%s" % (v["id"], " to " + v["to"] if v.get("to") else " to the room"))' ;;
        follow)
            _doc_parse "$@"
            [ ${#_DOC_ARGS[@]} -ge 1 ] || _doc_die usage "k2 doc follow <name> <file.md>"
            _doc_who "${_DOC_ARGS[0]}" ;;
        cursor)
            local at="" to=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --at) _need_value "$@"; at="${2:-}"; shift 2 ;;
                    --to|--select) _need_value "$@"; to="${2:-}"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            [ -n "$at" ] || _doc_die usage "k2 doc cursor --at L:C [--to L:C] <file.md>"
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path cursor; local path="$_DOC_PATH"
            if [ -n "$to" ]; then
                _doc_call POST /cli/docs/cursor "$(_doc_body "$path" at "$at" to "$to")"
            else
                _doc_call POST /cli/docs/cursor "$(_doc_body "$path" at "$at")"
            fi
            _doc_print_json_or 'import json,sys; v=json.load(sys.stdin); c=v["cursor"]; print("caret at bytes %s-%s" % (c["anchor"], c["head"]))' ;;
        write)
            local base=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --base) _need_value "$@"; base="${2:-}"; shift 2 ;;
                    --base=*) base="${1#*=}"; shift ;;
                    --file) _need_value "$@"; _DOC_FILE="${2:-}"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            [ -n "$base" ] || _doc_die usage "k2 doc write --base <version>|live <file.md> < text (read the doc first: k2 doc cat)"
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path write; local path="$_DOC_PATH"
            if [ "$_DOC_TEXT_SET" != 1 ] && [ -z "$_DOC_FILE" ] && [ -t 0 ]; then
                _doc_die usage "k2 doc write reads the whole new content from stdin, --file or --text"
            fi
            if [ -n "$_DOC_FILE" ] && [ ! -f "$_DOC_FILE" ]; then
                _doc_die usage "--file $_DOC_FILE: no such file" 4
            fi
            # The bytes ride as base64 from a temp file (any file, any size
            # up to the cap; a Markdown doc's must be UTF-8).
            _doc_tmp
            _doc_body "$path" base "$base" @bytesB64 @b64 >"$_DOC_TMPD/body.json" \
                || _doc_die usage "couldn't read the new content"
            _doc_call_file POST /cli/docs/write "" "$_DOC_TMPD/body.json"
            _DOC_BODY="$(cat "$_DOC_OUT")"
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
r = v.get("result") or {}
if v.get("tier") == 0:
    if r.get("changed"):
        print("wrote %s (%s bytes) · v%s" % (v["doc"]["path"], r.get("size"), r.get("version")), file=sys.stderr)
    else:
        print("%s already holds those bytes · v%s (nothing new recorded)" % (v["doc"]["path"], r.get("version")), file=sys.stderr)
    sys.exit(0)
print("wrote %s (+%s -%s) · v%s" % (v["doc"]["path"], r.get("linesAdded", 0), r.get("linesRemoved", 0), r.get("version")), file=sys.stderr)
if v.get("warning"):
    print("Warning: " + v["warning"], file=sys.stderr)
if v.get("notes"):
    print(v["notes"].rstrip("\n"), file=sys.stderr)
a = v.get("approval")
if a:
    print("on behalf of %s: %s" % (a.get("onBehalfOf"), a.get("state")), file=sys.stderr)
' ;;
        undo|redo)
            local steps=1 mode="all" for_p=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --steps) _need_value "$@"; steps="${2:-}"; shift 2 ;;
                    --steps=*) steps="${1#*=}"; shift ;;
                    --partial) mode="partial"; shift ;;
                    --drop) mode="drop"; shift ;;
                    --for) _need_value "$@"; for_p="${2:-}"; shift 2 ;;
                    --for=*) for_p="${1#*=}"; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            case "$steps" in ''|*[!0-9]*) _doc_die usage "--steps needs a number" ;; esac
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path "$verb"; local path="$_DOC_PATH"
            _doc_call POST "/cli/docs/$verb" "$(_doc_body "$path" @steps "@int:$steps" mode "$mode" forPerson "$for_p")"
            _doc_print_undo "$verb" ;;
        log)
            local by="" since="" n="" pend=@false for_p=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --by) _need_value "$@"; by="${2:-}"; shift 2 ;;
                    --since) _need_value "$@"; since="${2:-}"; shift 2 ;;
                    -n) _need_value "$@"; n="${2:-}"; shift 2 ;;
                    --pending) pend=@true; shift ;;
                    --for|--on-behalf-of) _need_value "$@"; for_p="${2:-}"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            case "$n" in *[!0-9]*) _doc_die usage "-n needs a number" ;; esac
            local since_ms=""
            if [ -n "$since" ]; then
                since_ms="$(python3 -c '
import re, sys, time
s = sys.argv[1]
m = re.fullmatch(r"(\d+)([smhdw])", s)
if not m:
    sys.exit(1)
mult = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}[m.group(2)]
print(int((time.time() - int(m.group(1)) * mult) * 1000))
' "$since")" || _doc_die usage "--since takes a duration: 30m, 2h, 7d"
            fi
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path log; local path="$_DOC_PATH"
            _doc_call GET /cli/docs/log "$(_doc_query "$path" by "$by" sinceMs "$since_ms" n "$n" pending "$([ "$pend" = @true ] && echo 1 || echo 0)" forPerson "$for_p")"
            _doc_print_json_or '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("log") or []
if not rows:
    print("No history.")
for x in rows:
    t = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime((x.get("atMs") or 0) / 1000))
    if x.get("type") == "version":
        name = (" \"%s\"" % x["name"]) if x.get("name") else ""
        by = (" by %s via %s, %s bytes" % (x["authorLabel"], x.get("via"), x.get("size"))) if x.get("authorLabel") else ""
        if x.get("onBehalfOf"):
            by += " for %s (%s)" % (x["onBehalfOf"], x.get("approval") or "pending")
        print("  version %-5s %s  %s%s%s" % (x.get("id"), t, x.get("reason"), name, by))
    else:
        who = x.get("author") or "?"
        if x.get("onBehalfOf"):
            who += " for %s (%s)" % (x["onBehalfOf"], x.get("approval") or "pending")
        of = (" of %s" % x["of"]) if x.get("of") is not None else ""
        print("change %-6s %s  %-8s%s +%s -%s  %s via %s" % (x.get("change"), t, x.get("kind"), of,
              x.get("linesAdded", 0), x.get("linesRemoved", 0), who, x.get("via")))
' ;;
        status)
            _doc_parse "$@"
            if [ ${#_DOC_ARGS[@]} -eq 0 ]; then
                _doc_call GET /cli/docs/status ""
                _doc_print_json_or '
import json, sys
p = json.load(sys.stdin).get("pool") or {}
if not p.get("running"):
    print("live docs: off (%s)" % p.get("error", "?"))
    sys.exit(0)
mib = lambda b: "%.1f MiB" % (b / 1048576)
print("live docs: %s loaded · %s of %s · %s unloaded since start · disk sync %s" % (
    p.get("loaded"), mib(p.get("estimatedBytes", 0)), mib(p.get("budgetBytes", 0)), p.get("unloadedTotal"),
    "on" if p.get("diskSync") else "off"))
a = p.get("anyFile") or {}
if a.get("on"):
    print("any-file docs: %s loaded · %s blobs, %s (%s unreferenced) · files up to %s" % (
        a.get("loaded"), a.get("blobs"), mib(a.get("bytes", 0)), mib(a.get("unreferencedBytes", 0)), mib(a.get("maxFileBytes", 0))))
elif a:
    print("any-file docs: off (see the daemon log)")
s = p.get("saves") or {}
if s:
    how = "grouped (%s changes or %s ms per wait for the disk)" % (s.get("groupMaxChanges"), s.get("groupMaxMs")) if s.get("commits") == "grouped" else "every change at once"
    print("saves: %s · %s of %s on disk%s" % (how, s.get("commitsOnDisk"), s.get("commitsMade"),
          " · restart K2 for Save every change immediately to take effect" if s.get("restartNeeded") else ""))
'
                return 0
            fi
            _doc_one_path status; local path="$_DOC_PATH"
            _doc_call GET /cli/docs/status "$(_doc_query "$path")"
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
d = v.get("disk") or {}
r = v.get("rights") or {}
state = d.get("state", "-")
if d.get("reason"):
    state += " (%s)" % d["reason"]
kind = " · any file (tier 0)" if v.get("tier") == 0 else ""
print("%s%s · v%s · %s here (%s connections) · disk %s" % (v["doc"]["path"], kind, v.get("version"), v.get("participants"), v.get("connections"), state))
if v.get("tier") == 0:
    import time
    c = v.get("current") or {}
    print("current: %s bytes, %s by %s via %s at %s" % (c.get("size"), c.get("reason"), c.get("authorLabel") or c.get("author"),
          c.get("via"), time.strftime("%Y-%m-%d %H:%M:%S", time.localtime((c.get("atMs") or 0) / 1000))))
    l = v.get("lease")
    if l:
        print("lease: %s until %s%s" % (l.get("label"), time.strftime("%H:%M:%S", time.localtime(l["expiresMs"] / 1000)),
              " (yours)" if l.get("mine") else ""))
    else:
        print("lease: free")
    for cp in v.get("conflictCopies") or []:
        print("conflict copy: %s (version %s, kept for 30 days)" % (cp.get("path"), cp.get("kept")))
print("your rights: " + ", ".join(k for k in ("read", "edit", "history") if r.get(k)))
if v.get("declined"):
    print("live editing is off: %s" % v["declined"])
' ;;
        flush|open|leave)
            _doc_parse "$@"
            _doc_one_path "$verb"; local path="$_DOC_PATH"
            _doc_call POST "/cli/docs/$verb" "$(_doc_body "$path")"
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
verb = sys.argv[1]
if verb == "flush":
    print("%s · disk %s%s" % (v["doc"]["path"], (v.get("disk") or {}).get("state", "-"), " (written)" if v.get("written") else ""))
elif verb == "open":
    print("in %s · v%s (stay present until k2 doc leave)" % (v["doc"]["path"], v.get("version")))
else:
    print("left %s" % v["doc"]["path"])
' "$verb" ;;
        approvals)
            local pend="" mine="" for_p=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --pending) pend=1; shift ;;
                    --mine|--on-behalf-of-me) mine=1; shift ;;
                    --for|--on-behalf-of) [ $# -ge 2 ] || _doc_die usage "$1 needs a person (or me)"; for_p="$2"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path approvals; local path="$_DOC_PATH"
            _doc_call GET /cli/docs/approvals "$(_doc_query "$path" pending "${pend:-0}" mine "${mine:-0}" forPerson "$for_p")"
            _doc_print_json_or '
import json, sys
rows = json.load(sys.stdin).get("approvals") or []
if not rows:
    print("No approvals.")
for r in rows:
    a = r.get("approval") or {}
    print("change %-6s %-9s by %s for %s%s" % (r.get("change"), a.get("state"), a.get("author"), a.get("onBehalfOf"),
          (" (settled by %s)" % a["by"]) if a.get("by") else ""))
' ;;
        approve|decline)
            _doc_parse "$@"
            [ ${#_DOC_ARGS[@]} -eq 2 ] || _doc_die usage "k2 doc $verb <change> <file.md>"
            local change="${_DOC_ARGS[0]}"
            case "$change" in ''|*[!0-9]*) _doc_die usage "the change is a number (k2 doc approvals)" ;; esac
            local path; path="$(_doc_path "${_DOC_ARGS[1]}")"
            local state="approved"; [ "$verb" = decline ] && state="declined"
            _doc_call POST /cli/docs/approve "$(_doc_body "$path" @change "@int:$change" state "$state")"
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
print("%s %s %s" % ("version" if v.get("tier") == 0 else "change", v.get("change"), v["approval"]["state"]))
if v.get("hint"):
    print(v["hint"], file=sys.stderr)
' ;;
        share|unshare)
            local with="" write=@false history=@false comment=@false
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --with) _need_value "$@"; with="${2:-}"; shift 2 ;;
                    --with=*) with="${1#*=}"; shift ;;
                    --write) write=@true; shift ;;
                    --history) history=@true; shift ;;
                    --comment) comment=@true; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            _doc_parse ${rest[@]+"${rest[@]}"}
            [ -n "$with" ] || _doc_die usage "k2 doc $verb <file-or-folder> --with <workspace> -w <its workspace>"
            [ -n "$_DOC_WS" ] || _doc_die usage "name the workspace the file is in: -w <workspace>"
            [ ${#_DOC_ARGS[@]} -eq 1 ] || _doc_die usage "k2 doc $verb <file-or-folder> --with <workspace> -w <ws>"
            local rel="${_DOC_ARGS[0]}"
            if [ "$verb" = share ]; then
                _doc_call POST /cli/docs/share "$(_doc_body "$rel" with "$with" @write "$write" @history "$history" @comment "$comment")"
            else
                _doc_call POST /cli/docs/unshare "$(_doc_body "$rel" with "$with")"
            fi
            _doc_print_json_or '
import json, sys
v = json.load(sys.stdin)
s = v.get("share") or v.get("removed") or {}
grants = ["read"] + [g for g in ("write", "history", "comment") if s.get(g)]
what = "shared" if v.get("share") else "unshared"
print("%s %s (%s) · %s" % (what, s.get("path") or ".", "folder" if s.get("folder") else "file", ", ".join(grants) if what == "shared" else ""))
' ;;
        shares)
            local with=""
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --with) _need_value "$@"; with="${2:-}"; shift 2 ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_call GET /cli/docs/shares "$(_doc_query "" with "$with")"
            _doc_print_json_or '
import json, sys
rows = json.load(sys.stdin).get("shares") or []
if not rows:
    print("No shares.")
for s in rows:
    grants = ["read"] + [g for g in ("write", "history", "comment") if s.get(g)]
    print("%s:%s%s -> %s  (%s)" % (s.get("owner"), s.get("path") or ".", "/" if s.get("folder") else "", s.get("sharee"), ", ".join(grants)))
' ;;
        blame)
            local range="" form="rendered"
            local -a rest=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    -L) _need_value "$@"; range="${2:-}"; shift 2 ;;
                    -L*) range="${1#-L}"; shift ;;
                    --porcelain|--line-porcelain) form="porcelain"; shift ;;
                    *) rest+=("$1"); shift ;;
                esac
            done
            case "$range" in ''|[0-9]*,[0-9]*|[0-9]*) ;; *) _doc_die usage "-L takes a,b (line numbers)" ;; esac
            case "$range" in *[!0-9,]*) _doc_die usage "-L takes a,b (line numbers)" ;; esac
            _doc_parse ${rest[@]+"${rest[@]}"}
            _doc_one_path blame; local path="$_DOC_PATH"
            _doc_call GET /cli/docs/blame "$(_doc_query "$path" lines "$range")"
            if [ "${_DOC_JSON:-0}" = "1" ]; then printf '%s\n' "$_DOC_BODY"; return 0; fi
            printf '%s' "$_DOC_BODY" | python3 -c 'import json,sys; v = json.load(sys.stdin); sys.stdout.write(v.get(sys.argv[1]) or v.get("rendered") or "")' "$form" ;;
        mv)
            _doc_parse "$@"
            [ ${#_DOC_ARGS[@]} -eq 2 ] || _doc_die usage "k2 doc mv <file.md> <new-name.md>"
            local path to
            path="$(_doc_path "${_DOC_ARGS[0]}")"
            to="${_DOC_ARGS[1]}"
            # A new name from where you stand (its folder exists here) goes
            # absolute; with -w it stays relative to that workspace.
            if [ -z "$_DOC_WS" ] && [ "${_K2_FORCE_TCP:-0}" != "1" ] && [ -d "$(dirname "$to")" ]; then
                to="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$to")"
            fi
            _doc_call POST /cli/docs/mv "$(_doc_body "$path" to "$to")"
            _doc_print_json_or 'import json,sys; v=json.load(sys.stdin); print("moved %s -> %s" % (v.get("from"), v["doc"]["path"]))' ;;
        export)
            _doc_export "$@" ;;
        help) _doc_usage ;;
        *)
            # A verb of the doc's own pack (Markdown: replace-section), or a
            # kit tool this CLI doesn't list: the daemon's tools decide
            # (an unknown verb exits 2, unsupported).
            case "$verb" in
                -*|*[!a-z-]*) _doc_die usage "unknown k2 doc verb: $verb (k2 doc --help)" ;;
            esac
            _doc_tool "$verb" "$@" ;;
    esac
}
# END_CLI_DOC

# BEGIN_CLI_PERMISSION
