# ── `k2 permission` — permission cards (Beta) ────────────────────────
# PRD prd-agent-settings-cards-v1 rev 3.1 §11. The family exists only while
# the server's "Permission cards (Beta)" switch is on (the default from
# 0.46.11) and, for an agent, its own "Can ask for permission cards" switch
# too (off until a person turns it on). Otherwise every
# surface prints today's bytes: `k2 permission …` is an unknown command,
# `k2 help` has no line for it, `k2 study permissions` is an unknown
# topic. It is not in `k2 --schema` or the CLI explorer during Beta.
#
# The probe (§11.2): `GET /cli/permission/enabled`, --max-time 1. Inside a
# K2 cell it asks with the session passport (both switches); outside one,
# `k2 permission …` asks with the owner credential (the server switch).
# `k2 help` / `k2 study` probe only inside a cell. Any failure = off.
# `k2 permission beta …` outside a cell skips the probe: it is how the
# owner turns the switch on from a terminal.

_K2_PERMISSION_ON=""

# One probe; prints nothing. Succeeds only on 200 {"enabled":true}.
_permission_probe() {
    local tok="$1" sock out="" _K2_CFG=""
    [ -n "$tok" ] || return 1
    sock="$(_hook_sock)"
    if [ -n "${SCOPED_TOKEN:-}" ] && [ "$tok" = "$SCOPED_TOKEN" ] && [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds; then
        _k2_cfg_add header "Authorization: Bearer ${tok}"
        out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/permission/enabled" \
            --connect-timeout 1 --max-time 1 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    elif [ -n "${PORT:-}" ]; then
        _k2_cfg_add data-urlencode "token=${tok}"
        out="$(_k2_curl "$_K2_CFG" -s -G "http://${HOST}:${PORT}/cli/permission/enabled" \
            --connect-timeout 1 --max-time 1 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    fi
    [[ "$out" == *$'\n200' ]] || return 1
    out="${out%$'\n200'}"
    printf '%s' "$out" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except Exception:
    sys.exit(1)
sys.exit(0 if isinstance(v, dict) and v.get("enabled") is True else 1)
' 2>/dev/null
}

# For `k2 help` / `k2 study`: inside a K2 cell only, cached per process.
_permission_cli_enabled() {
    if [ -z "$_K2_PERMISSION_ON" ]; then
        _K2_PERMISSION_ON=0
        if [ -n "${SCOPED_TOKEN:-}" ] && _permission_probe "$SCOPED_TOKEN"; then
            _K2_PERMISSION_ON=1
        fi
    fi
    [ "$_K2_PERMISSION_ON" = 1 ]
}

# For `k2 permission <sub>`: does the family exist for this caller?
_permission_family_available() {
    local sub="${1:-}"
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        _permission_cli_enabled
        return
    fi
    [ "$sub" = "beta" ] && return 0
    _permission_probe "${TOKEN:-}"
}

_permission_usage() {
    cat <<'EOF'
Usage: k2 permission <command> (Beta)

Ask a person to allow what K2 refused you. Every refusal ends with the
exact command to run ("Ask with: …"), or "It can't be requested."

Agents (in a K2 terminal):
  k2 permission request <action> [key=value …] --why "<one line>" [--dry-run]
  k2 permission update <id> --why "<new facts>"   once, on a denied request
  k2 permission status [<id>]                      check once; never loop
  k2 permission cancel <id>
  k2 permission list [--area <area>] [--search <word>]

People (a terminal outside K2):
  k2 permission pending                            what waits on you
  k2 permission approve <id>
  k2 permission deny <id> [--reason "…"]
  k2 permission beta on|off|status                 the server switch (owner; on by default)
  k2 permission agent <workspace> on|off           one agent's "Let this agent ask for
                                                   permission" (owner or admin; off until
                                                   someone turns it on)

Rules: k2 study permissions
EOF
}

# Print a family answer ("<body>\n<status>" from cli_send_json) and exit
# with the documented code: 2xx 0, 400 2, 404 4, 403/409/429 3, else 1.
_permission_print() {
    local verb="$1" raw="$2" json="${3:-0}"
    local status="${raw##*$'\n'}" body="${raw%$'\n'*}"
    if [ "$status" = "000" ] || [ -z "$status" ]; then
        echo "Error: no answer from the K2 daemon" >&2
        exit 1
    fi
    if [ "$json" = 1 ] && [ "${status:0:1}" = 2 ]; then
        printf '%s\n' "$body"
        exit 0
    fi
    printf '%s' "$body" | K2_PV="$verb" K2_PS="$status" python3 -c '
import json, os, sys, time
verb, status = os.environ["K2_PV"], int(os.environ["K2_PS"])
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    v = None
def when(ts):
    try:
        return time.strftime("%Y-%m-%d %H:%M", time.localtime(int(ts)))
    except Exception:
        return str(ts)
def expires_in(ts):
    try:
        s = int(ts) - int(time.time())
    except Exception:
        return ""
    if s <= 0:
        return "expired"
    d, h = s // 86400, (s % 86400) // 3600
    if d >= 1:
        return "expires in %d day%s" % (d, "" if d == 1 else "s")
    return "expires in %d hour%s" % (max(h, 1), "" if h == 1 else "s")
def people(r):
    out = []
    for p in r or []:
        if isinstance(p, dict):
            n = p.get("login") or p.get("name") or "?"
            role = p.get("role")
            out.append("%s (%s)" % (n, role) if role else n)
        else:
            out.append(str(p))
    return ", ".join(out) or "nobody yet"
if status >= 300 or not isinstance(v, dict):
    e = v.get("error") if isinstance(v, dict) else None
    code = e.get("code") if isinstance(e, dict) else (e if isinstance(e, str) else "error")
    hint = (e.get("hint") or e.get("message")) if isinstance(e, dict) else (v.get("hint") or v.get("message") if isinstance(v, dict) else None)
    if code == "recently_denied" and isinstance(v, dict):
        d = v.get("denied") or (e if isinstance(e, dict) else {})
        msg = "Denied by %s at %s" % (d.get("by", "?"), when(d.get("at"))) if d.get("by") else "Denied"
        if d.get("reason"):
            msg += ": \"%s\"" % d["reason"]
        print(msg + ".", file=sys.stderr)
        until = d.get("askAgainAt") or d.get("until")
        rid = d.get("id") or v.get("id") or "<id>"
        print("Ask again after %s, or add facts: k2 permission update %s --why \"…\"" % (when(until) if until else "24 hours", rid), file=sys.stderr)
    else:
        print("Error [%s]: %s" % (code, hint or raw.strip()[:300] or "status %d" % status), file=sys.stderr)
    sys.exit(0)
st = v.get("status")
if verb == "request":
    if v.get("dryRun") or ("text" in v and "id" not in v):
        print("Dry run: nothing was sent.")
        print("Card: %s" % (v.get("text") or ""))
        if v.get("consequences"):
            print("What it means: %s" % v["consequences"])
        print("Shown to: %s" % people(v.get("routedTo") or v.get("approvers")))
        if v.get("expiresIn"):
            print("Expires: %s" % v["expiresIn"])
    elif st == "already_pending" or v.get("code") == "already_pending":
        print("Already asked: %s (pending). Nothing new sent." % v.get("id", "?"))
    elif st == "already_done" or v.get("code") == "already_done":
        print("Already on: nothing to ask. Run your command again.")
    else:
        exp = expires_in(v.get("expiresAt"))
        print("Asked: %s (%s%s). Shown to: %s." % (v.get("id", "?"), st or "pending", "; " + exp if exp else "", people(v.get("routedTo"))))
        print("Card raised on your Thread%s: they answer it there (desktop, phone or web); you get one [permission:%s] turn with the answer." % (" (" + v["addr"] + ")" if v.get("addr") else "", v.get("id", "?")))
        if v.get("text"):
            print("Card: %s" % v["text"] + ("" if not v.get("duration") else " Lasts: %s." % v["duration"]))
elif verb in ("update", "cancel", "approve", "deny", "beta", "agent"):
    print(v.get("message") or v.get("hint") or "OK.")
else:
    print(json.dumps(v, indent=2))
' || true
    case "$status" in
        2??) exit 0 ;;
        400|413) exit 2 ;;
        404) exit 4 ;;
        403|409|429) exit 3 ;;
        *) exit 1 ;;
    esac
}

# People verbs are refused inside a K2 terminal (§11.1): a person answers
# from a terminal outside K2, the app or the phone.
_permission_people_only() {
    if [ -n "${SCOPED_TOKEN:-}" ]; then
        echo "Error [person_only]: k2 permission $1 is for people, from a terminal outside K2 (or the K2 app). An agent asks with k2 permission request." >&2
        exit 3
    fi
}

_permission_request() {
    local action="" why="" dry=0 json=0 kv=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --why) [ $# -ge 2 ] || { echo "Error: --why needs a value" >&2; exit 2; }; why="$2"; shift 2 ;;
            --why=*) why="${1#--why=}"; shift ;;
            --dry-run) dry=1; shift ;;
            --json) json=1; shift ;;
            -h|--help) _permission_usage; exit 0 ;;
            --*) echo "Error: unknown flag '$1'" >&2; _permission_usage >&2; exit 2 ;;
            *=*) kv+=("$1"); shift ;;
            *)
                if [ -z "$action" ]; then action="$1"; shift
                else echo "Error: unexpected argument '$1' (values are key=value)" >&2; exit 2; fi ;;
        esac
    done
    [ -n "$action" ] || { echo "Usage: k2 permission request <action> [key=value …] --why \"<one line>\"" >&2; exit 2; }
    [ -n "$why" ] || { echo "Error [bad_why]: --why is required: one line, at most 200 characters, saying what you need it for" >&2; exit 2; }
    local body
    body="$(K2_PA="$action" K2_PW="$why" K2_PD="$dry" python3 -c '
import json, os, sys
params = {}
for a in sys.argv[1:]:
    k, _, v = a.partition("=")
    params[k] = v
out = {"action": os.environ["K2_PA"], "params": params, "why": os.environ["K2_PW"]}
if os.environ["K2_PD"] == "1":
    out["dryRun"] = True
print(json.dumps(out))
' "${kv[@]+"${kv[@]}"}")"
    _permission_print request "$(cli_send_json POST /cli/permission/request "$body")" "$json"
}

_permission_cmd() {
    local sub="${1:-}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        ""|help|-h|--help) _permission_usage ;;
        request|ask) _permission_request "$@" ;;
        update)
            local id="${1:-}" why=""
            [ -n "$id" ] && shift
            while [ $# -gt 0 ]; do
                case "$1" in
                    --why) why="${2:-}"; shift 2 ;;
                    *) echo "Error: unexpected argument '$1'" >&2; exit 2 ;;
                esac
            done
            [ -n "$id" ] && [ -n "$why" ] || { echo "Usage: k2 permission update <id> --why \"<new facts>\"" >&2; exit 2; }
            _permission_print update "$(cli_send_json POST /cli/permission/update "$(K2_PI="$id" K2_PW="$why" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_PI"], "why": os.environ["K2_PW"]}))')")"
            ;;
        status)
            local q=""
            [ -n "${1:-}" ] && q="id=$(urlencode "$1")"
            _permission_print status "$(cli_send_json GET /cli/permission/status "$q")"
            ;;
        cancel)
            [ -n "${1:-}" ] || { echo "Usage: k2 permission cancel <id>" >&2; exit 2; }
            _permission_print cancel "$(cli_send_json POST /cli/permission/cancel "$(K2_PI="$1" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_PI"]}))')")"
            ;;
        list)
            local q="" area="" search=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --area) area="${2:-}"; shift 2 ;;
                    --search) search="${2:-}"; shift 2 ;;
                    *) echo "Error: unexpected argument '$1'" >&2; exit 2 ;;
                esac
            done
            [ -n "$area" ] && q="area=$(urlencode "$area")"
            [ -n "$search" ] && q="${q}${q:+&}search=$(urlencode "$search")"
            _permission_print list "$(cli_send_json GET /cli/permission/list "$q")"
            ;;
        pending)
            _permission_people_only pending
            _permission_print pending "$(cli_send_json GET /cli/permission/pending)"
            ;;
        approve|deny)
            _permission_people_only "$sub"
            local id="${1:-}" reason=""
            [ -n "$id" ] && shift
            while [ $# -gt 0 ]; do
                case "$1" in
                    --reason) reason="${2:-}"; shift 2 ;;
                    *) echo "Error: unexpected argument '$1'" >&2; exit 2 ;;
                esac
            done
            [ -n "$id" ] || { echo "Usage: k2 permission $sub <id>" >&2; exit 2; }
            local pending answer body
            pending="$(cli_send_json GET /cli/permission/pending)"
            [ "$sub" = approve ] && answer="Allow" || answer="Deny"
            body="$(printf '%s' "${pending%$'\n'*}" | K2_PI="$id" K2_PA="$answer" K2_PR="$reason" python3 -c '
import json, os, sys
try:
    v = json.load(sys.stdin)
except Exception:
    sys.exit(1)
rid = os.environ["K2_PI"]
for r in v.get("requests") or []:
    if r.get("requestId") == rid:
        if os.environ["K2_PA"] == "Allow" and (r.get("proof") or {}).get("kind") == "confirm":
            print("CONFIRM")
            sys.exit(0)
        out = {"addr": r.get("addr"), "id": r.get("cardId"), "answer": os.environ["K2_PA"]}
        if os.environ["K2_PR"]:
            out["reason"] = os.environ["K2_PR"]
        print(json.dumps(out))
        sys.exit(0)
sys.exit(4)
' 2>/dev/null)" || {
                echo "Error [not_found]: no request $id waits on you (k2 permission pending)" >&2
                exit 4
            }
            if [ "$body" = "CONFIRM" ]; then
                echo "Error [vault_confirm_required]: $id needs the same confirm Settings takes. Approve it in the K2 app, on your phone, or in Settings." >&2
                exit 3
            fi
            _permission_print "$sub" "$(cli_send_json POST /cli/thread/answer "$body")"
            ;;
        beta)
            _permission_people_only beta
            case "${1:-status}" in
                on|off)
                    _permission_print beta "$(cli_send_json POST /cli/settings/update "{\"permissionCards\":$([ "$1" = on ] && echo true || echo false)}")"
                    ;;
                status)
                    local raw
                    raw="$(cli_send_json GET /cli/settings/get)"
                    printf '%s' "${raw%$'\n'*}" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except Exception:
    print("Error: no answer from the K2 daemon", file=sys.stderr); sys.exit(1)
s = v.get("settings", v) if isinstance(v, dict) else {}
print("Permission cards (Beta): %s" % ("on" if s.get("permissionCards") is True else "off"))
'
                    ;;
                *) echo "Usage: k2 permission beta on|off|status" >&2; exit 2 ;;
            esac
            ;;
        agent)
            _permission_people_only agent
            local ws="${1:-}" onoff="${2:-}"
            case "$onoff" in on|off) ;; *) echo "Usage: k2 permission agent <workspace> on|off" >&2; exit 2 ;; esac
            _permission_print agent "$(cli_send_json POST /cli/agent-access/set "$(K2_PW="$ws" K2_PV="$onoff" python3 -c 'import json,os; print(json.dumps({"workspace": os.environ["K2_PW"], "toggle": "permission_cards", "value": os.environ["K2_PV"] == "on"}))')")"
            ;;
        *)
            echo "Error: unknown k2 permission command: $sub" >&2
            _permission_usage >&2
            exit 2
            ;;
    esac
}

# `k2 permission …` from the router: today's unknown command unless the
# family exists for this caller.
cmd_permission() {
    if ! _permission_family_available "${1:-}"; then
        echo "Unknown command: permission" >&2
        cmd_help
        exit 1
    fi
    # An `id` tool: inside a K2 cell the daemon sees the session passport,
    # never the owner token (outside one, the owner credential as usual).
    if [ -n "${SCOPED_TOKEN:-}" ] && [ -n "$(_hook_sock)" ]; then
        TOKEN="$SCOPED_TOKEN"
    fi
    _permission_cmd "$@"
}
# END_CLI_PERMISSION

# BEGIN_CLI_ORGANIZATION
# `k2 organization` — the Org Chart (prd-org-chart-v1, 0.46.11): orgs of
# agents, their heads, the lines between them and the daily server
# bulletin. One tool, no short form. The daemon owns everything
# (/cli/organization/*); this is the thin front-end. An `id` tool: inside
# a K2 terminal the daemon sees the session passport, never the owner token.
cmd_help_organization() {
    cat <<'EOF'
k2 organization — orgs on this server's Org Chart, and the daily bulletin

The Org Chart groups this server's agents into orgs (departments). Each org
has one head, who keeps its bulletin (at most 5 lines). The server bulletin
is every org's bulletin together: read it once a day, and use it to pick the
right head before you route a question to another agent.

READ (any agent; any person on the server)
  k2 organization bulletin                today's server bulletin; marks it read for you
  k2 organization bulletin --peek         show it without marking it read
  k2 organization bulletin read           mark today's as read (no output)
  k2 organization status                  "<day>: unread" or "<day>: read" (a day starts at
                                          06:00 in the owner's timezone, so before 06:00
                                          it is still yesterday)
  k2 organization list                    every org: name, head, members, purpose
  k2 organization show <org>              one org: head, members, lines, its bulletin
  k2 organization mine                    your org, your head, and whether you're connected to it
                                          (in no org: says so)
  k2 organization night                   tonight's night run (by day, last night's): each org's
                                          time, and queued / running / done / skipped

REACH A DEPARTMENT
  k2 organization reach <org|head> --why "<one line>"     (an org's name, or its head's)
      Connected to that head already: prints the k2 msg line (exit 0).
      Not connected: asks an Admin to connect you (a permission card, or a
      ticket) and exits 2. K2 then types one line into your session: you're
      connected (use the k2 msg line), or it was declined (ask your person;
      don't reach again). Don't ask twice, and don't k2 msg them before that.

HEADS
  k2 organization bulletin set "<line 1>" "<line 2>" …   replaces your org's whole bulletin:
                                                           at most 5 lines, 160 characters each
  k2 organization bulletin set --file notes.txt           one bulletin line per line of the file
      An Owner or Admin writes any org's: add --org <org>.
  k2 organization ask "<question>" [--member <agent>]      ask your members (or one) for news; K2
                                                           types it to one member at a time, about
                                                           2 minutes apart; they answer with
                                                           k2 organization update
  k2 organization updates                                  what your members sent you (since last night)

MEMBERS
  k2 organization update "<text>"          news to your org's head (at most 500 characters). Stored
                                           at once (the head sees it in updates); at night it is
                                           also typed into the head's session

MANAGE (Owner and Admin people; an agent with "Manage the org chart" on)
  k2 organization create <name> [--purpose "<short paragraph>"]
  k2 organization rename <org> <new name>
  k2 organization purpose <org> "<short paragraph, at most 400 characters>"
  k2 organization delete <org>                       members leave; agents stay
  k2 organization add <org> <agent>                  an agent is in one org at most
  k2 organization move <agent> <org>                 out of its org, into this one
  k2 organization remove <org> <agent>
  k2 organization head <org> <agent|none>            the head must be a member
  k2 organization link <a> above <b>                 a sits above b (b reports to a)
  k2 organization link <a> below <b>                 a sits below b (a reports to b)
  k2 organization link <a> beside <b>                side by side
  k2 organization unlink <a> <b>
      Lines are drawn on the chart between cards (agents or orgs; write
      org:<name> / agent:<name> when a name is both). They show who sits
      over whom; they aren't membership (add/move put an agent in an org).
      A line joins one card's bottom to another's top, or two sides: never
      top to top. No loops. A card may sit under several cards.
  There is no position verb: the chart lays itself out (a person drags on
  the Org Chart tab).

Add --json to any verb for the daemon's JSON. Exit codes: 0 ok; 2 refused
for a reason you can fix, held, or the Org Chart is off (the reason is
printed as "Error [code]: …"); 3 not allowed for you; 1 no daemon or an error.

WHEN IT'S OFF
  Every verb answers "The Org Chart is off on this server" (exit 2) until an
  Owner or Admin turns it on in Settings → Org Chart.

MORE: k2 study organization
EOF
}

# The renderer: the answer's body on stdin, K2_OV verb, K2_OS status, K2_OJ
# json (1/0). Prints, then exits with the documented code.
_ORG_PY='
import json, os, sys
verb, status, as_json = os.environ["K2_OV"], int(os.environ["K2_OS"]), os.environ["K2_OJ"] == "1"
raw = sys.stdin.read()
try:
    v = json.loads(raw)
except Exception:
    v = None
def code_for(st):
    if 200 <= st < 300:
        return 0
    if st in (401, 403):
        return 3
    if 400 <= st < 500:
        return 2
    return 1
rc = code_for(status)
if not isinstance(v, dict):
    sys.stderr.write("Error: unexpected answer from the K2 daemon (HTTP %d): %s\n" % (status, raw[:300]))
    sys.exit(rc or 1)
b = v.get("bulletin")
if isinstance(b, dict) and "unread" in b:
    v.pop("bulletin")
if rc != 0:
    e = v.get("error")
    if isinstance(e, dict):
        sys.stderr.write("Error [%s]: %s\n" % (e.get("code", "error"), e.get("hint") or e.get("message") or ""))
    else:
        sys.stderr.write("Error: %s\n" % (v.get("message") or e or raw[:300]))
    sys.exit(rc)
held = verb == "reach" and v.get("status") in ("card", "ticket", "open")
if as_json:
    print(json.dumps(v))
    sys.exit(2 if held else 0)
w = sys.stdout.write
def handle(r):
    return r.get("handle") if isinstance(r, dict) else "none yet"
def org_lines(o):
    head = o.get("head")
    w("%s · head: %s\n" % (o.get("name"), handle(head) if head else "none yet"))
    if o.get("purpose"):
        w("  Purpose: %s\n" % o["purpose"])
    ms = o.get("members") or []
    if ms:
        w("  Members: %s\n" % ", ".join(("%s (head)" % m["handle"]) if m.get("isHead") else m["handle"] for m in ms))
        for m in ms:
            if m.get("role"):
                w("    %s: %s\n" % (m["handle"], m["role"]))
    else:
        w("  Members: none yet\n")
    bl = o.get("bulletin")
    if bl and bl.get("lines"):
        w("  Bulletin:\n")
        for l in bl["lines"]:
            w("    %s\n" % l)
    else:
        w("  Bulletin: none yet\n")
if verb == "bulletin":
    w(v.get("text", ""))
elif verb == "bulletin-read":
    pass
elif verb == "status":
    w("%s: %s\n" % (v.get("day"), "unread (read it: k2 organization bulletin)" if v.get("unread") else "read"))
elif verb == "list":
    orgs = v.get("orgs") or []
    if not orgs:
        w("No orgs yet. An Owner or Admin makes one: k2 organization create <name>\n")
    for o in orgs:
        org_lines(o)
elif verb == "show":
    o = v.get("org") or {}
    org_lines(o)
    for l in o.get("lines") or []:
        word = "above" if l.get("kind") == "below" else "beside"
        w("  Line: %s %s %s\n" % (l["a"]["name"], word, l["b"]["name"]))
elif verb == "mine":
    o = v.get("org")
    if not o:
        w(v.get("next", "You are in no org.") + "\n")
    else:
        head = v.get("head")
        if v.get("isHead"):
            w("You are %s head.\n" % (o["name"] + chr(39) + "s"))
        elif not head:
            w("You are in %s. It has no head yet.\n" % o["name"])
        elif v.get("connectedToHead"):
            w("You are in %s. Head: %s (connected: k2 msg %s \"...\").\n" % (o["name"], head["handle"], head["handle"]))
        else:
            w("You are in %s. Head: %s (not connected: k2 organization reach %s --why \"...\").\n" % (o["name"], head["handle"], o["name"]))
        org_lines(o)
elif verb == "reach":
    w((v.get("text") or "") + "\n")
elif verb == "night":
    g = v.get("governor") or {}
    mc = g.get("machine") or {}
    c = v.get("counts") or {}
    w("%s (night of %s, %s): runs spread %s-%s, last call %s, new day %s.\n" % (
        "Tonight" if v.get("tonight") else "Last night", v.get("night"), v.get("zone"),
        v.get("runAt"), v.get("spreadEndsAt"), v.get("lastCallAt"), v.get("dayStartsAt")))
    free = mc.get("availableBytes")
    load = mc.get("load1")
    w("This computer: %s at a time (%s cores%s%s).\n" % (
        g.get("slots"), mc.get("cores"),
        ", %.1f GiB free" % (free / 1073741824.0) if isinstance(free, (int, float)) else "",
        ", load %.1f" % load if isinstance(load, (int, float)) else ""))
    if g.get("holding"):
        w("Holding new starts: %s.\n" % g["holding"])
    w("Queued %s · running %s · done %s · skipped %s%s\n" % (
        c.get("queued", 0), c.get("running", 0), c.get("done", 0), c.get("skipped", 0),
        (" · failed %s" % c["failed"]) if c.get("failed") else ""))
    for o in v.get("orgs") or []:
        state = o.get("state")
        if state == "done" and o.get("wrote"):
            state = "done (wrote its bulletin)"
        elif state == "skipped":
            state = "skipped (the bulletin stays as it was)"
        w("  %s  %s · head: %s · %s\n" % (o.get("due"), o["org"]["name"], o["head"]["handle"], state))
elif verb == "updates":
    us = v.get("updates") or []
    if not us:
        w("No updates for %s since last night.\n" % (v.get("org") or {}).get("name", "your org"))
    for u in us:
        w("%s: %s\n" % (u["from"]["handle"], u["body"]))
elif verb == "ask":
    q = v.get("queued") or []
    w("Asked %s now.\n" % (", ".join(v.get("asked") or []) or "nobody"))
    if q:
        w("Queued: %s (K2 asks %s).\n" % (", ".join(q), v.get("spacing") or "one at a time"))
    w("Their answers: k2 organization updates\n")
    for n in v.get("notDelivered") or []:
        w("Not delivered to %s (%s); the ask is stored.\n" % (n.get("member"), n.get("why")))
elif verb == "update":
    h = (v.get("head") or {}).get("handle", "the head")
    if v.get("deliveredNow"):
        w("Sent to %s.\n" % h)
    else:
        w("Stored for %s (heads read updates at night: k2 organization updates).\n" % h)
elif verb == "bulletin-set":
    n = len(v.get("lines") or [])
    w("%s bulletin now has %d line%s; every agent sees it at once.\n" % (v["org"]["name"] + chr(39) + "s", n, "" if n == 1 else "s"))
elif verb == "link":
    l = v.get("line") or {}
    word = "above" if l.get("kind") == "below" else "beside"
    w("Linked: %s %s %s.\n" % (l.get("a", {}).get("name"), word, l.get("b", {}).get("name")))
elif verb == "unlink":
    w("Unlinked.\n")
elif verb == "delete":
    w("Deleted %s; its members left (the agents stay).\n" % v["deleted"]["name"])
elif verb in ("add", "remove") and v.get("changed") is False:
    w("No change.\n")
    org_lines(v.get("org") or {})
elif verb in ("create", "rename", "purpose", "add", "move", "remove", "head"):
    org_lines(v.get("org") or {})
else:
    print(json.dumps(v))
sys.exit(2 if held else 0)
'

# Send METHOD PATH PAYLOAD, print the answer for VERB, exit with its code.
_org_call() {
    local verb="$1" method="$2" path="$3" payload="${4:-}" json="${_ORG_JSON:-0}"
    local raw status body
    raw="$(cli_send_json "$method" "$path" "$payload")"
    status="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$status" = "000" ] || [ "$status" = "$raw" ]; then
        echo "Error: no answer from the K2 daemon (is K2 running? k2 daemon status)" >&2
        exit 1
    fi
    local rc=0
    printf '%s' "$body" | K2_OV="$verb" K2_OS="$status" K2_OJ="$json" python3 -c "$_ORG_PY" || rc=$?
    exit "$rc"
}

# JSON object from key=value pairs (strings), via python (no quoting bugs).
_org_json() {
    python3 -c '
import json, sys
out = {}
args = sys.argv[1:]
for a in args:
    k, _, v = a.partition("=")
    if k.endswith("[]"):
        out.setdefault(k[:-2], []).append(v)
    else:
        out[k] = v
print(json.dumps(out))
' "$@"
}

_org_usage_err() {
    echo "Error: $1" >&2
    echo "See: k2 organization --help" >&2
    exit 2
}

cmd_organization() {
    local verb="${1:-}"
    [ $# -gt 0 ] && shift
    case "$verb" in
        ''|--help|-h|help) cmd_help_organization; exit 0 ;;
    esac
    if _wants_help "$@"; then cmd_help_organization; exit 0; fi
    # An `id` tool: inside a K2 cell the daemon sees the session passport.
    if [ -n "${SCOPED_TOKEN:-}" ] && [ -n "$(_hook_sock)" ]; then
        TOKEN="$SCOPED_TOKEN"
    fi
    _ORG_JSON=0
    local args=() a peek=0 file="" why="" purpose="" member="" org="" since=""
    while [ $# -gt 0 ]; do
        a="$1"
        case "$a" in
            --json) _ORG_JSON=1; shift ;;
            --peek) peek=1; shift ;;
            --file) [ $# -ge 2 ] || _org_usage_err "--file needs a path"; file="$2"; shift 2 ;;
            --file=*) file="${a#--file=}"; shift ;;
            --why) [ $# -ge 2 ] || _org_usage_err "--why needs one line"; why="$2"; shift 2 ;;
            --why=*) why="${a#--why=}"; shift ;;
            --purpose) [ $# -ge 2 ] || _org_usage_err "--purpose needs text"; purpose="$2"; shift 2 ;;
            --purpose=*) purpose="${a#--purpose=}"; shift ;;
            --member) [ $# -ge 2 ] || _org_usage_err "--member needs an agent"; member="$2"; shift 2 ;;
            --member=*) member="${a#--member=}"; shift ;;
            --org) [ $# -ge 2 ] || _org_usage_err "--org needs an org"; org="$2"; shift 2 ;;
            --org=*) org="${a#--org=}"; shift ;;
            --since) [ $# -ge 2 ] || _org_usage_err "--since needs a unix time"; since="$2"; shift 2 ;;
            --) shift; while [ $# -gt 0 ]; do args+=("$1"); shift; done ;;
            --*) _org_usage_err "unknown flag '$a'" ;;
            *) args+=("$a"); shift ;;
        esac
    done
    local n=${#args[@]}
    local P="/cli/organization"
    case "$verb" in
        bulletin)
            local sub="${args[0]:-}"
            case "$sub" in
                '')
                    if [ "$peek" = 1 ]; then _org_call bulletin GET "$P/bulletin" "peek=1"; fi
                    _org_call bulletin GET "$P/bulletin" ""
                    ;;
                read) _org_call bulletin-read POST "$P/bulletin/read" "{}" ;;
                show)
                    local o="${args[1]:-$org}"
                    [ -n "$o" ] || _org_usage_err "name the org: k2 organization bulletin show <org>"
                    _org_call show GET "$P/show" "org=$(urlencode "$o")"
                    ;;
                set)
                    local lines=() i
                    if [ -n "$file" ]; then
                        [ -f "$file" ] || _org_usage_err "no file '$file'"
                        lines+=("$(cat "$file")")
                    fi
                    for ((i = 1; i < n; i++)); do lines+=("${args[$i]}"); done
                    [ ${#lines[@]} -gt 0 ] || _org_usage_err "give the lines: k2 organization bulletin set \"<line 1>\" … (or --file <path>)"
                    local kv=() l
                    for l in "${lines[@]}"; do kv+=("lines[]=$l"); done
                    [ -n "$org" ] && kv+=("org=$org")
                    _org_call bulletin-set POST "$P/bulletin/set" "$(_org_json "${kv[@]}")"
                    ;;
                *) _org_usage_err "bulletin takes no argument, or read, set or show" ;;
            esac
            ;;
        status) _org_call status GET "$P/bulletin/status" "" ;;
        list) _org_call list GET "$P/list" "" ;;
        show)
            [ "$n" -ge 1 ] || _org_usage_err "name the org: k2 organization show <org>"
            _org_call show GET "$P/show" "org=$(urlencode "${args[0]}")"
            ;;
        mine) _org_call mine GET "$P/mine" "" ;;
        night) _org_call night GET "$P/night" "" ;;
        chart) _ORG_JSON=1; _org_call chart GET "$P/chart" "" ;;
        reach)
            [ "$n" -ge 1 ] || _org_usage_err "name the org (or its head): k2 organization reach <org> --why \"<one line>\""
            [ -n "$why" ] || _org_usage_err "say why in one line: k2 organization reach ${args[0]} --why \"<one line>\""
            _org_call reach POST "$P/reach" "$(_org_json "org=${args[0]}" "why=$why")"
            ;;
        updates)
            local q=""
            [ -n "$org" ] && q="org=$(urlencode "$org")"
            [ -n "$since" ] && q="${q:+$q&}since=$(urlencode "$since")"
            _org_call updates GET "$P/updates" "$q"
            ;;
        ask)
            [ "$n" -ge 1 ] || _org_usage_err "give the question: k2 organization ask \"<question>\" [--member <agent>]"
            local kv=("question=${args[*]}")
            [ -n "$member" ] && kv+=("member=$member")
            _org_call ask POST "$P/ask" "$(_org_json "${kv[@]}")"
            ;;
        update)
            [ "$n" -ge 1 ] || _org_usage_err "give the news: k2 organization update \"<text>\""
            local kv=("text=${args[*]}")
            [ -n "$org" ] && kv+=("org=$org")
            _org_call update POST "$P/update" "$(_org_json "${kv[@]}")"
            ;;
        create)
            [ "$n" -ge 1 ] || _org_usage_err "name the org: k2 organization create <name> [--purpose \"…\"]"
            _org_call create POST "$P/create" "$(_org_json "name=${args[*]}" "purpose=$purpose")"
            ;;
        rename)
            [ "$n" -ge 2 ] || _org_usage_err "k2 organization rename <org> <new name>"
            _org_call rename POST "$P/rename" "$(_org_json "org=${args[0]}" "name=${args[*]:1}")"
            ;;
        purpose)
            [ "$n" -ge 1 ] || _org_usage_err "k2 organization purpose <org> \"<short paragraph>\""
            local p="${args[*]:1}"
            [ -n "$purpose" ] && p="$purpose"
            _org_call purpose POST "$P/purpose" "$(_org_json "org=${args[0]}" "purpose=$p")"
            ;;
        delete)
            [ "$n" -eq 1 ] || _org_usage_err "k2 organization delete <org>"
            _org_call delete POST "$P/delete" "$(_org_json "org=${args[0]}")"
            ;;
        add)
            [ "$n" -eq 2 ] || _org_usage_err "k2 organization add <org> <agent>"
            _org_call add POST "$P/member/add" "$(_org_json "org=${args[0]}" "agent=${args[1]}")"
            ;;
        move)
            [ "$n" -eq 2 ] || _org_usage_err "k2 organization move <agent> <org>"
            _org_call move POST "$P/member/move" "$(_org_json "agent=${args[0]}" "org=${args[1]}")"
            ;;
        remove)
            [ "$n" -eq 2 ] || _org_usage_err "k2 organization remove <org> <agent>"
            _org_call remove POST "$P/member/remove" "$(_org_json "org=${args[0]}" "agent=${args[1]}")"
            ;;
        head)
            [ "$n" -eq 2 ] || _org_usage_err "k2 organization head <org> <agent|none>"
            _org_call head POST "$P/head" "$(_org_json "org=${args[0]}" "agent=${args[1]}")"
            ;;
        link)
            [ "$n" -eq 3 ] || _org_usage_err "k2 organization link <a> above|below|beside <b>"
            case "${args[1]}" in
                above|below|beside) ;;
                *) _org_usage_err "say how they sit: above (a over b), below (a under b) or beside. Never top to top." ;;
            esac
            # The route's kinds: below (a over b), under (a under b), beside.
            local lk="${args[1]}"
            case "$lk" in above) lk=below ;; below) lk=under ;; esac
            _org_call link POST "$P/line/add" "$(_org_json "a=${args[0]}" "kind=$lk" "b=${args[2]}")"
            ;;
        unlink)
            [ "$n" -eq 2 ] || _org_usage_err "k2 organization unlink <a> <b>"
            _org_call unlink POST "$P/line/remove" "$(_org_json "a=${args[0]}" "b=${args[1]}")"
            ;;
        position|place|layout|arrange)
            _org_usage_err "there is no position verb: the chart lays itself out (a person drags cards on the Org Chart tab)"
            ;;
        *)
            echo "Unknown organization command: $verb" >&2
            cmd_help_organization >&2
            exit 2
            ;;
    esac
}
# END_CLI_ORGANIZATION

