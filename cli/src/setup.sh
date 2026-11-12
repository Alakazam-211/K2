# ── k2 setup (prd-setup-checklist-v1 C2) ────────────────────────────────
# The setup checklist on this server: the same list the app shows, read
# from `GET /cli/first-run/status` (each step's done state is the server's
# real state). Per person: the owner token and the logins the owner linked
# share one list. Exit 0 ok; 2 a usage mistake; 3 refused (an agent, an app
# pass, a Voice & Chat only login); 4 no such Focus Group; 1 anything else.
_setup_usage() {
    cat <<'EOF'
k2 setup [--json]       your setup list on this server: [x] done, [ ] to do, [-] skipped
k2 setup hide           hide the list in the app (k2 setup still prints it)
k2 setup show           bring the list back, skipped steps included, even after it finished
k2 setup skip <step>    skip one step: dev_tools, llm_sign_in, first_agent (or chief_of_staff), tunnel, phone, own_sign_in

Kessel, K2's own agent (when the owner has turned on kesselFirstRun):
k2 setup kessel [--tool claude|codex|grok] [--name N] [--handle H] [--focus-group G]
                        meet Kessel, the agent that runs K2: its setup,
                        settings and agents. It helps you hire
                        your Chief of Staff. --tool defaults to the
                        server default (it must be signed in: k2 llm tokens);
                        --name Kessel and --handle kessel by default (its folder
                        is <hire root>/<handle>); --focus-group puts it in a
                        Focus Group (id or name; none by default).
                        Owner, Owner or Admin login.
k2 setup chief-of-staff --name N --about "what they do" --tool claude|codex|grok [--focus-group G]
                        hire this person's Chief of Staff, their lead agent.
                        It joins Kessel's Focus Group unless --focus-group names
                        another (id or name).
                        For Kessel only: it works from Kessel's own pinned chat, never a
                        sidecar, another agent or a person (exit 3).
k2 setup dismiss-offer  "Not now" on the Meet Kessel offer (yours only)

Each step is done by doing it: the list reads what is set up on this
server, never a tick. dev_tools shows only while git does not run here:
on a Mac, k2 git-tools install opens the Apple installer on its screen. It is yours: the owner and the logins the owner
linked share one list, and every other login has its own. When every
step is done or skipped the list finishes and the app stops showing it.
Agents have no setup list (exit 3), except Kessel running k2 setup chief-of-staff.
EOF
}

cmd_setup() {
    local sub="" as_json=0 row="" kessel_tool="" kessel_name="" kessel_handle="" kessel_about="" have_about=0 kessel_group=""
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help|help) _setup_usage; return 0 ;;
            --json) as_json=1; shift ;;
            hide|show|kessel|chief-of-staff|dismiss-offer)
                [ -z "$sub" ] || { _setup_usage >&2; exit 2; }
                sub="$1"; shift ;;
            --tool|--name|--handle|--about|--focus-group)
                case "$sub" in
                    kessel|chief-of-staff) ;;
                    *) echo "k2 setup: $1 goes with k2 setup kessel or k2 setup chief-of-staff" >&2; exit 2 ;;
                esac
                [ $# -ge 2 ] || { echo "k2 setup: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --tool) kessel_tool="$2" ;;
                    --name) kessel_name="$2" ;;
                    --focus-group) kessel_group="$2" ;;
                    --handle)
                        [ "$sub" = "kessel" ] || { echo "k2 setup chief-of-staff: no --handle (its folder comes from --name)" >&2; exit 2; }
                        kessel_handle="$2" ;;
                    --about)
                        [ "$sub" = "chief-of-staff" ] || { echo "k2 setup kessel: no --about" >&2; exit 2; }
                        kessel_about="$2"; have_about=1 ;;
                esac
                shift 2 ;;
            skip)
                [ -z "$sub" ] || { _setup_usage >&2; exit 2; }
                sub=skip; shift
                row="${1:-}"
                [ -n "$row" ] || { echo "usage: k2 setup skip <step> (k2 setup lists them)" >&2; exit 2; }
                shift ;;
            *) echo "k2 setup: unknown argument: $1" >&2; _setup_usage >&2; exit 2 ;;
        esac
    done
    if [ "$sub" = "chief-of-staff" ]; then
        if [ -z "$kessel_name" ] || [ "$have_about" = "0" ] || [ -z "$kessel_tool" ]; then
            echo "usage: k2 setup chief-of-staff --name N --about \"what they do\" --tool claude|codex|grok [--focus-group G] (ask your person all three first)" >&2
            exit 2
        fi
    fi
    local body="" raw
    case "$sub" in
        kessel|chief-of-staff)
            # The fields given, in this order; chief-of-staff always sends about.
            local sep=""
            body="{"
            if [ -n "$kessel_tool" ]; then _setup_jstr "$kessel_tool"; body="${body}${sep}\"tool\": ${_SETUP_J}"; sep=", "; fi
            if [ -n "$kessel_name" ]; then _setup_jstr "$kessel_name"; body="${body}${sep}\"name\": ${_SETUP_J}"; sep=", "; fi
            if [ -n "$kessel_handle" ]; then _setup_jstr "$kessel_handle"; body="${body}${sep}\"handle\": ${_SETUP_J}"; sep=", "; fi
            if [ -n "$kessel_about" ] || [ "$sub" = "chief-of-staff" ]; then
                _setup_jstr "$kessel_about"; body="${body}${sep}\"about\": ${_SETUP_J}"; sep=", "
            fi
            if [ -n "$kessel_group" ]; then _setup_jstr "$kessel_group"; body="${body}${sep}\"focusGroup\": ${_SETUP_J}"; sep=", "; fi
            body="${body}}" ;;
        skip) _setup_jstr "$row"; body="{\"row\": ${_SETUP_J}}" ;;
        hide|show|dismiss-offer) body='{}' ;;
    esac
    case "$sub" in
        "") raw="$(_setup_send GET /cli/first-run/status "$as_json")" ;;
        hide|show) raw="$(_setup_send POST "/cli/first-run/$sub" "$as_json" "$body")" ;;
        dismiss-offer) raw="$(_setup_send POST /cli/first-run/offer/dismiss "$as_json" "$body")" ;;
        kessel) raw="$(_setup_send POST /cli/first-run/kessel "$as_json" "$body")" ;;
        chief-of-staff) raw="$(_setup_send POST /cli/first-run/chief-of-staff "$as_json" "$body")" ;;
        skip) raw="$(_setup_send POST /cli/first-run/skip "$as_json" "$body")" ;;
    esac
    _setup_render "$sub" "$as_json" "$raw"
}

# k2 setup is pure bash + curl, like k2 git-tools: on a Mac without
# Apple's Command Line Tools /usr/bin/python3 is one of Apple's shims (it
# opens the installer instead of running), and that is the Mac the
# dev_tools step is for. The daemon renders the lines (`?format=text`);
# refusals stay JSON and are read here.

# _setup_jstr VALUE: VALUE as a JSON string, quotes included, in _SETUP_J.
_setup_jstr() {
    local LC_ALL=C s="$1" out="" c i
    if [[ "$s" != *[[:cntrl:]\\\"]* ]]; then
        _SETUP_J="\"${s}\""
        return 0
    fi
    for ((i = 0; i < ${#s}; i++)); do
        c="${s:i:1}"
        case "$c" in
            \\) out="${out}\\\\" ;;
            \") out="${out}\\\"" ;;
            $'\n') out="${out}\\n" ;;
            $'\t') out="${out}\\t" ;;
            $'\r') out="${out}\\r" ;;
            $'\177') out="${out}${c}" ;;
            [[:cntrl:]]) printf -v c '\\u%04x' "'$c"; out="${out}${c}" ;;
            *) out="${out}${c}" ;;
        esac
    done
    _SETUP_J="\"${out}\""
}

# _setup_junesc TEXT: a JSON string body (between its quotes) decoded, in
# _SETUP_J.
_setup_junesc() {
    local LC_ALL=C s="$1" out="" c i hex cp
    if [[ "$s" != *\\* ]]; then
        _SETUP_J="$s"
        return 0
    fi
    for ((i = 0; i < ${#s}; i++)); do
        c="${s:i:1}"
        if [ "$c" != "\\" ]; then
            out="${out}${c}"
            continue
        fi
        i=$((i + 1))
        c="${s:i:1}"
        case "$c" in
            n) out="${out}"$'\n' ;;
            t) out="${out}"$'\t' ;;
            r) out="${out}"$'\r' ;;
            b) out="${out}"$'\b' ;;
            f) out="${out}"$'\f' ;;
            u)
                hex="${s:i+1:4}"
                i=$((i + 4))
                [[ "$hex" =~ ^[0-9A-Fa-f]{4}$ ]] || continue
                cp=$((16#$hex))
                if [ "$cp" -lt 128 ]; then
                    printf -v c "\\x$(printf '%02x' "$cp")"
                elif [ "$cp" -lt 2048 ]; then
                    printf -v c "\\x$(printf '%02x' $((192 | (cp >> 6))))\\x$(printf '%02x' $((128 | (cp & 63))))"
                else
                    printf -v c "\\x$(printf '%02x' $((224 | (cp >> 12))))\\x$(printf '%02x' $((128 | ((cp >> 6) & 63))))\\x$(printf '%02x' $((128 | (cp & 63))))"
                fi
                out="${out}${c}" ;;
            *) out="${out}${c}" ;;
        esac
    done
    _SETUP_J="$out"
}

# _setup_jfield KEY JSON: true with the first "KEY": "<string>" in JSON,
# decoded, in _SETUP_J.
_setup_jfield() {
    local re='"'"$1"'"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)"'
    [[ "$2" =~ $re ]] || return 1
    _setup_junesc "${BASH_REMATCH[1]}"
}

# _setup_send <GET|POST> <path> <as_json> [<body>]: the answer, then a
# newline and the HTTP code. The token rides curl's config, never argv.
_setup_send() {
    local method="$1" path="$2" as_json="$3" body="${4:-}" q out try=1 code _K2_CFG
    q="token=$(_git_tools_q "$TOKEN")"
    [ "$as_json" = "1" ] || q="${q}&format=text"
    while :; do
        _K2_CFG=""
        _k2_cfg_add url "${BASE_URL}${path}?${q}"
        if [ "$method" = "GET" ]; then
            out="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 30 -w $'\n%{http_code}' 2>/dev/null; printf x)"
        else
            _k2_cfg_add data-raw "$body"
            out="$(_k2_curl "$_K2_CFG" -s -X POST --connect-timeout 5 --max-time 30 \
                -H "Content-Type: application/json" -w $'\n%{http_code}' 2>/dev/null; printf x)"
        fi
        out="${out%x}"
        code="${out##*$'\n'}"
        # The daemon before it is ready (no handler ran): ask again.
        if [ "$method" = "GET" ] && [ "$code" = "503" ] && [[ "$out" == *'"state":"migrating"'* ]] && [ "$try" -lt "$_K2_CURL_TRIES" ]; then
            try=$((try + 1)); sleep 1; continue
        fi
        # A passport the daemon refused while restarting: once with the new one.
        if _cli_retry_with_healed_passport "${out%$'\n'*}"; then
            q="token=$(_git_tools_q "$TOKEN")"
            [ "$as_json" = "1" ] || q="${q}&format=text"
            continue
        fi
        break
    done
    printf '%s' "$out"
}

# _setup_render <sub> <as_json> <raw "body\ncode">: print the answer.
# Exit 0 ok; 2 a usage mistake (usage, unknown_row, tool_not_signed_in);
# 3 refused (person_only, session_only, forbidden, role_required, a token
# or passport refusal); 4 no such Focus Group (unknown_focus_group,
# ambiguous_focus_group); 1 anything else, as k2 setup always has.
_setup_render() {
    local sub="$1" as_json="$2" raw="$3" code body err_code="" hint="" low
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ] || [ -z "$body" ]; then
        echo "Error: could not reach the K2 daemon (k2 setup)" >&2
        exit 1
    fi
    _permission_note "$body"
    local trimmed="${body#"${body%%[![:space:]]*}"}"
    if [ "$code" = "200" ]; then
        if [ "$as_json" = "1" ]; then
            printf '%s\n' "$body"
            return 0
        fi
        case "$trimmed" in
            "{"*)
                # A daemon from before ?format=text answers JSON.
                printf '%s' "$body" | _setup_render_json_py "$sub" "$as_json"
                return $? ;;
        esac
        printf '%s' "$body"
        return 0
    fi
    case "$trimmed" in
        "{"*) ;;
        *)
            echo "Error: the daemon sent something k2 setup does not understand: ${body:0:200}" >&2
            exit 1 ;;
    esac
    local re_obj='"error"[[:space:]]*:[[:space:]]*\{(.*)$'
    local re_str='"error"[[:space:]]*:[[:space:]]*"'
    if [[ "$body" =~ $re_obj ]]; then
        local rest="${BASH_REMATCH[1]}"
        _setup_jfield code "$rest" && err_code="$_SETUP_J"
        if _setup_jfield hint "$rest" && [ -n "$_SETUP_J" ]; then
            hint="$_SETUP_J"
        elif _setup_jfield message "$rest" && [ -n "$_SETUP_J" ]; then
            hint="$_SETUP_J"
        else
            hint="$err_code"
        fi
    elif [[ "$body" =~ $re_str ]] && _setup_jfield error "$body"; then
        hint="$_SETUP_J"
    else
        hint="${body:0:200}"
    fi
    if [ -n "$err_code" ] && [[ "$hint" != *"$err_code"* ]]; then
        echo "Error: ${err_code}: ${hint}" >&2
    else
        echo "Error: ${hint}" >&2
    fi
    case "$err_code" in
        usage|unknown_row|tool_not_signed_in) exit 2 ;;
        unknown_focus_group|ambiguous_focus_group) exit 4 ;;
    esac
    low="$(printf '%s' "$err_code $hint" | tr '[:upper:]' '[:lower:]')"
    case "$low" in
        *person_only*|*session_only*|*forbidden*|*role_required*|*token*|*passport*) exit 3 ;;
    esac
    exit 1
}

# The list from a daemon before ?format=text (it answers JSON): the
# renderer k2 setup used before the daemon drew the lines. Only an older
# daemon (a --host one) gets here.
_setup_render_json_py() {
    K2_SETUP_SUB="$1" K2_SETUP_JSON="$2" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon sent something k2 setup does not understand: " + raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
sub = os.environ.get("K2_SETUP_SUB") or ""
made = sub in ("kessel", "chief-of-staff")
if err is not None or not isinstance(d, dict) or ("rows" not in d and not (made and "workspaceId" in d)):
    if isinstance(err, dict):
        code = err.get("code") or ""
        hint = err.get("hint") or err.get("message") or code
    else:
        code, hint = "", str(err if err is not None else raw[:200])
    print("Error: %s%s" % ((code + ": ") if code and code not in hint else "", hint), file=sys.stderr)
    low = (code + " " + hint).lower()
    if code in ("usage", "unknown_row", "tool_not_signed_in"):
        sys.exit(2)
    if code in ("unknown_focus_group", "ambiguous_focus_group"):
        sys.exit(4)
    if any(w in low for w in ("person_only", "session_only", "forbidden", "role_required", "token", "passport")):
        sys.exit(3)
    sys.exit(1)
if os.environ.get("K2_SETUP_JSON") == "1":
    print(json.dumps(d, indent=2))
    sys.exit(0)
if made:
    perms = d.get("permissions") or {}
    what = "Kessel" if sub == "kessel" else "your Chief of Staff"
    print("Made %s: %s (%s) at %s" % (what, d.get("name"), d.get("handle"), d.get("path")))
    granted = perms.get("granted") or []
    left = perms.get("left") or []
    if granted:
        print("Allowed from the start: " + ", ".join(granted))
    if left:
        print("It asks on a permission card for: " + ", ".join("%s (%s)" % (l.get("action"), l.get("reason")) for l in left))
    if sub == "kessel":
        print("Kessel is starting and will say hello on its Thread: k2 thread %s" % d.get("handle"))
    else:
        print("Tell your person on your Thread, and point them to %s (k2 msg %s)." % (d.get("name"), d.get("handle")))
    sys.exit(0)
LABELS = {
    "dev_tools": "Install developer tools",
    "llm_sign_in": "Sign in to an AI tool",
    "first_agent": "Make your first agent",
    "chief_of_staff": "Hire your Chief of Staff",
    "tunnel": "Link your subdomain",
    "phone": "Connect your phone",
    "own_sign_in": "Make your own sign-in",
}
NOTES = {
    "needs_tunnel": "Away from home, your phone needs your subdomain first.",
    "needs_kessel": "You will meet Kessel first.",
    "installing": "The Apple installer is open. Follow it, and this step finishes by itself.",
}
MARK = {"done": "[x]", "todo": "[ ]", "skipped": "[-]"}
rows = d.get("rows") or []
done = sum(1 for r in rows if r.get("state") == "done")
print("Setup: %d of %d done" % (done, len(rows)))
for r in rows:
    rid = r.get("id") or "?"
    line = "  %s %s" % (MARK.get(r.get("state"), "[?]"), LABELS.get(rid, rid))
    note = NOTES.get(r.get("note") or "")
    if rid == "dev_tools" and r.get("opens") is None and r.get("note"):
        # Linux: the note is the install line to run.
        note = "run: " + str(r.get("note"))
    elif rid == "dev_tools" and r.get("opens") == "dev_tools" and not note:
        note = "The Apple tools include git, which workspaces need: k2 git-tools install"
    if note and r.get("state") != "done":
        line += " (" + note + ")"
    print(line)
if d.get("finished"):
    print("Finished: the app no longer shows this list (k2 setup show brings it back).")
elif d.get("hidden"):
    print("Hidden in the app (k2 setup show brings it back).")
if not d.get("enabled"):
    print("The app does not show setup lists on this server yet.")
kessel = d.get("kessel")
if kessel:
    print("Kessel: %s at %s (k2 thread <its handle> to talk to it)" % (kessel.get("name"), kessel.get("path")))
if d.get("offer") == "meet_kessel":
    print("Next: meet Kessel, the agent that runs K2: k2 setup kessel")
'
}

# After a sign-in: the Meet Kessel hint, when the offer would show (quiet when
# it would not, or the daemon cannot say). No python3 (k2 setup above).
_setup_kessel_hint() {
    local raw
    raw="$(_setup_send GET /cli/first-run/status 1 2>/dev/null)" || return 0
    [ "${raw##*$'\n'}" = "200" ] || return 0
    case "${raw%$'\n'*}" in
        *'"offer":"meet_kessel"'*) echo "Next: meet Kessel, the agent that runs K2: k2 setup kessel" ;;
    esac
    return 0
}

