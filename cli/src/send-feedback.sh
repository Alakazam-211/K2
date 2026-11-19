# ── k2 send-feedback (prd-k2-product-feedback-v1, 0.46.11) ──────────────
# Feedback about K2 itself to Alakazam Labs, K2's makers: a bug, an idea,
# praise. NOT `k2 feedback` / `k2 tickets` (asking your own human on this
# server). The daemon keeps it in an outbox and sends it when it can;
# `/cli/product-feedback/*`. Exit 0 saved or sent; 2 a usage or input
# mistake; 3 refused (turned off, agents off, the agent limit, no K2
# Connect pairing, air-gap).
#
# 0.46.17, the agent opt-in: agents report only after a person said yes
# (`settings --agents off|cli|all`, `--attach`), each report says its
# scope (`--scope cli --command "<what it ran>"` or `--scope platform`),
# and while agents are off an agent's `k2 help` leaves this command out
# and its study page is one line (_sf_agents_off_here).

# The product's name in the lines this part writes: brand/k2/brand.toml
# [names] product (tests/cli/send_feedback_optin.sh checks they agree).
_SF_PRODUCT="K2"
_send_feedback_usage() {
    cat <<'EOF'
k2 send-feedback "<text>" [--category bug|idea|praise|other] [--scope cli|platform] [--command "<cmd>"]
                 [--diagnostics | --diagnostics-preview] [--yes] [--json]
k2 send-feedback - [flags]                 the text from stdin
k2 send-feedback list [--state queued|sending|sent|failed|refused|cancelled] [--json]
k2 send-feedback show <id> [--json]
k2 send-feedback retry <id>                a failed one goes back in the queue now
k2 send-feedback cancel <id>               a queued one is not sent
k2 send-feedback settings [--enabled on|off] [--agents off|cli|all] [--attach <parts>|none|all]
                          [--agent-diagnostics on|off] [--json]

Tell Alakazam Labs, the people who make K2, about K2 itself: a bug, an
idea, praise. Not for your human: to ask the people on this server, use
k2 tickets ask "<question>" (k2 feedback ask is the same command). Write
what you did, what you expected and what happened; the versions are added
for you. At most 20,000 bytes as typed.

What is sent: the category (default other); your text, with values from
the vault replaced by [k2v:NAME] (anything else you type is sent as
written, so leave out paths, names and output you wouldn't hand to
Alakazam); whether a person or an agent sent it; the client (cli); the K2,
daemon and OS versions, install kind, architecture and locale; and a
5-minute K2 Connect token naming this server's label (such as
<name>.k2.dev, which can point to who runs it), the proof of which server
it came from. Never sent: your name, login or workspace, files, terminal
output, chats, mail, tickets, environment variables, your own tokens, IP
addresses.

  --category C           bug, idea, praise or other (default other)
  --scope S              an agent's report says what it is about: cli (a
                         problem with a K2 command) or platform (anything
                         else about K2). A person's report has no scope.
  --command "<cmd>"      with --scope cli: the one command you ran, as you
                         ran it (one line, at most 1000 bytes; required)
  --diagnostics          also include details about K2 on this computer:
                         versions, settings that are on, counts, recent
                         errors and the end of K2's log, with passwords,
                         keys, names and paths taken out. They are printed
                         first; a person at a terminal is asked "Send these
                         details with your feedback? [y/N]" (N still sends
                         the feedback, without them). The CLI never
                         remembers the answer.
  --diagnostics-preview  print those details, send nothing (exit 0)
  --yes                  send the details without asking. Agents always
                         pass it with --diagnostics (there is no one to
                         ask); without a terminal and without --yes,
                         --diagnostics is a usage error (exit 2)
  --json                 the saved row as JSON

The plain output prints the id (pf_…) that show, retry and cancel take.
This server keeps what you send in its outbox and sends it to Alakazam
when it can. Offline, or while K2's feedback service isn't reachable yet,
it waits as queued with the reason, and nothing is lost: don't send it
again, check it with show. After 7 days of trying it is failed. One that
failed because the service couldn't be reached goes again by itself: this
server checks about once an hour whether the service is taking feedback
again (it sends nothing to check), and once it is, all of them saved in
the last 90 days go back in the queue and leave oldest first. Retry puts any failed one back at once, including one
that failed for another reason (sending turned off, air-gap, no K2
Connect pairing). A refused one (the service said no; the reason names why)
is not retried: fix what it says and send a new one. Rows are deleted 90
days after they are sent, refused, cancelled or failed.

settings with no flags shows this server's switches and "can send now":
what agents may report (nothing, CLI problems, or anything about K2), the
parts of the K2 details they may attach, each on or off, and who answered
and when (or "not asked yet"); --json is the daemon's answer. Anyone may
read it. Changing them is for a person, the owner or an admin:
  --agents off|cli|all   off: agents report nothing (and attach nothing);
                         cli: problems with K2 commands; all: anything
                         about K2 too. on is the same as cli.
  --attach <parts>       the parts agents may attach, joined by commas
                         (settings lists them), or none or all. The
                         versions always go.
  --agent-diagnostics on|off
                         the older switch: on is every part, off none. Not
                         with --attach.
Until a person answers, agents report nothing. A server older than K2
0.46.17 takes only --agents on|off and --agent-diagnostics; there,
--agents cli|all and --attach say it needs the update (exit 1) and change
nothing. k2 setup agent-reports takes the same --agents and --attach.

Agents: allowed only while a person on this server said yes, at most 1 in any 60
seconds and 3 in any 24 hours per workspace, and 10 in any 24 hours for
all agents on this server. Every saved send counts (cancelled ones don't;
refusals and previews save nothing and don't count). There is no contact
email from the CLI: only a person, in the desktop dialog, can ask
Alakazam to write back. An agent's report says its scope: --scope cli
--command "<the command you ran>" for a problem with a K2 command, or
--scope platform for anything else (only where a person allowed that
too: otherwise scope_off). --diagnostics and --diagnostics-preview from an
agent attach only the parts a person allowed (and the versions); with
none allowed nothing is saved or sent (diagnostics_need_owner): send it
again without --diagnostics. The details are printed for you too and the
owner sees them in the Sent tab. When this server can't send at all,
tell your human: they can email hello@alakazamlabs.com.

Text that is exactly list, show, retry, cancel, settings or help, or
starts with a dash: send it from stdin (echo "settings" | k2 send-feedback -).

Exit codes: 0 saved, sent or previewed; 1 this server needs an update
for the flag; 2 a usage or input mistake (bad_category, text_required,
text_too_long, command_required, bad_command, bad_scope, bad_attach_part,
platform_needs_agents); 3 refused, nothing saved or changed: disabled,
agents_off, scope_off, answer_agents_first, agent_limit (with retryAfterSecs),
diagnostics_need_owner, needs_connect (this server isn't paired with K2
Connect), airgap, settings_people_only, admin_only, not_found, wrong_state
(retry on a row that isn't failed, cancel on one that isn't queued). A
refusal prints one JSON line on stderr: {"error": <code>, "hint": <why>}.
A later refusal by the service shows as refused in list. More:
k2 study send-feedback.
EOF
}

# Hidden from agents while off (0.46.17, K2's ruling): in an agent's
# session `k2 help` leaves send-feedback out and `k2 study send-feedback`
# is one line, while this server's settings answer agents:false (the
# switch itself, not canSend or why: a drain hold wins over agents_off
# there) and senderKind:agent (the daemon's word for the caller: a person
# typing in a K2 terminal tab is a person and sees everything). One read
# per process, cached, only with a session passport. Any failure (no
# daemon, a refusal, an answer without those fields) shows everything as
# before. A terminal outside K2 (the owner token) never asks.
_K2_SF_AGENTS_OFF=""
_sf_agents_off_here() {
    if [ -z "$_K2_SF_AGENTS_OFF" ]; then
        _K2_SF_AGENTS_OFF=0
        if [ -n "${SCOPED_TOKEN:-}" ] && _sf_agents_off_probe; then
            _K2_SF_AGENTS_OFF=1
        fi
    fi
    [ "$_K2_SF_AGENTS_OFF" = 1 ]
}

# One settings read with the session passport, the cell socket first (as
# _permission_probe). Succeeds only on 200 with "agents": false and
# "senderKind": "agent".
_sf_agents_off_probe() {
    local sock out="" _K2_CFG=""
    sock="$(_hook_sock)"
    if [ -n "$sock" ] && [ -S "$sock" ] && ! _cli_skip_uds; then
        _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
        out="$(_k2_curl "$_K2_CFG" -s -G --unix-socket "$sock" "http://localhost/cli/product-feedback/settings" \
            --connect-timeout 1 --max-time 2 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    elif [ -n "${PORT:-}" ]; then
        _k2_cfg_auth "$SCOPED_TOKEN"
        out="$(_k2_curl "$_K2_CFG" -s -G "http://${HOST}:${PORT}/cli/product-feedback/settings" \
            --connect-timeout 1 --max-time 2 -w $'\n%{http_code}' 2>/dev/null)" || out=""
    fi
    [[ "$out" == *$'\n200' ]] || return 1
    out="${out%$'\n200'}"
    printf '%s' "$out" | python3 -c '
import json, sys
try:
    v = json.load(sys.stdin)
except Exception:
    sys.exit(1)
sys.exit(0 if isinstance(v, dict) and v.get("agents") is False and v.get("senderKind") == "agent" else 1)
' 2>/dev/null
}

# The study page's one line while agents are off.
_sf_off_line() {
    echo "Agents don't report $_SF_PRODUCT problems on this server: a person turns it on in Settings, then Feedback."
}

_sf_onoff() {
    case "$1" in
        on|true|yes|1) echo true ;;
        off|false|no|0) echo false ;;
        *) echo "usage: $2 takes on or off" >&2; exit 2 ;;
    esac
}

# Print a preview / show item's sections.
_sf_print_sections() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
secs = v.get("sections") or (v.get("item") or {}).get("diagnostics") or []
for s in secs:
    print("── %s (%s) ──" % (s.get("title", ""), s.get("id", "")))
    print(s.get("text", ""))
    print()
'
}

_sf_send() {
    local text="" have_text=0 category="" diag=0 preview=0 yes=0 json_out=0 scope="" have_scope=0 command="" have_command=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --category) [ $# -ge 2 ] || { echo "usage: --category bug|idea|praise|other" >&2; exit 2; }; category="$2"; shift 2 ;;
            --category=*) category="${1#--category=}"; shift ;;
            --scope) [ $# -ge 2 ] || { echo "usage: --scope cli|platform" >&2; exit 2; }; scope="$2"; have_scope=1; shift 2 ;;
            --scope=*) scope="${1#--scope=}"; have_scope=1; shift ;;
            --command) [ $# -ge 2 ] || { echo "usage: --command \"<the command you ran>\"" >&2; exit 2; }; command="$2"; have_command=1; shift 2 ;;
            --command=*) command="${1#--command=}"; have_command=1; shift ;;
            --diagnostics) diag=1; shift ;;
            --diagnostics-preview) preview=1; shift ;;
            --yes|-y) yes=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _send_feedback_usage; return 0 ;;
            --) shift; [ $# -ge 1 ] || break; [ "$have_text" = 0 ] || { echo "usage: one text (quote it)" >&2; exit 2; }; text="$1"; have_text=1; shift ;;
            -) [ "$have_text" = 0 ] || { echo "usage: one text (quote it)" >&2; exit 2; }; text="$(cat)"; have_text=1; shift ;;
            -*) echo "Unknown flag: $1 (k2 send-feedback --help)" >&2; exit 2 ;;
            *) [ "$have_text" = 0 ] || { echo "usage: one text (quote it): k2 send-feedback \"<text>\"" >&2; exit 2; }; text="$1"; have_text=1; shift ;;
        esac
    done
    if [ "$diag" = 1 ] && [ "$preview" = 1 ]; then
        echo "usage: --diagnostics or --diagnostics-preview, not both" >&2; exit 2
    fi
    if [ -n "$category" ]; then
        case "$category" in bug|idea|praise|other) ;; *) echo "usage: --category is bug, idea, praise or other" >&2; exit 2 ;; esac
    fi
    # The scope and the command, checked before anything is asked.
    case "$scope" in
        cli|platform) ;;
        "") [ "$have_scope" = 0 ] || { echo "usage: --scope cli|platform" >&2; exit 2; } ;;
        *) echo "usage: --scope is cli (a problem with a $_SF_PRODUCT command) or platform (anything else about $_SF_PRODUCT)" >&2; exit 2 ;;
    esac
    case "$command" in
        *$'\n'*|*$'\r'*) echo "usage: --command is the one line you ran, without a new line" >&2; exit 2 ;;
    esac
    if [ "$have_command" = 1 ] && [ -z "${command//[[:space:]]/}" ]; then
        echo "usage: --command needs the command you ran" >&2; exit 2
    fi
    if [ "$scope" = cli ] && [ "$have_command" = 0 ]; then
        echo "usage: --scope cli names the command you ran: --command \"<the command>\"" >&2; exit 2
    fi
    if [ "$preview" = 0 ] && [ -z "${text//[[:space:]]/}" ]; then
        echo "usage: k2 send-feedback \"<text>\" (or - for stdin). k2 send-feedback --help" >&2; exit 2
    fi
    local draft=""
    if [ "$diag" = 1 ] || [ "$preview" = 1 ]; then
        local dpayload
        dpayload="$(_compute_json K2_CJ_client=cli K2_CJ_page=cli "K2_CJ_cliVersion=${K2_CLI_VERSION:-}")"
        _compute_call POST /cli/product-feedback/diagnostics "$dpayload"
        local preview_body="$_CP_BODY"
        if [ "$preview" = 1 ] && [ "$json_out" = 1 ]; then
            printf '%s\n' "$preview_body"; return 0
        fi
        echo "These details about K2 on this computer would be sent, exactly as below:" >&2
        echo >&2
        printf '%s' "$preview_body" | _sf_print_sections
        if [ "$preview" = 1 ]; then
            echo "Nothing was sent." >&2
            return 0
        fi
        local answer=""
        if [ "$yes" = 1 ]; then
            answer=y
        elif [ -t 0 ] && [ -t 2 ] && { : < /dev/tty; } 2>/dev/null; then
            printf 'Send these details with your feedback? [y/N] ' > /dev/tty
            IFS= read -r answer < /dev/tty || answer=""
        else
            echo "usage: --diagnostics asks before sending, and there is no terminal to ask on; add --yes to send the details" >&2
            exit 2
        fi
        case "$answer" in
            y|Y|yes|YES|Yes) draft="$(printf '%s' "$preview_body" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("draftId",""))')" ;;
            *) echo "Sending your feedback without the K2 details." >&2 ;;
        esac
    fi
    local locale="${LC_ALL:-${LANG:-}}"
    locale="${locale%%.*}"
    local payload
    payload="$(K2_SF_TEXT="$text" K2_SF_CAT="$category" K2_SF_DRAFT="$draft" K2_SF_LOCALE="$locale" \
        K2_SF_SCOPE="$scope" K2_SF_COMMAND="$command" python3 -c '
import json, os
b = {"text": os.environ["K2_SF_TEXT"], "client": "cli"}
if os.environ.get("K2_SF_CAT"):
    b["category"] = os.environ["K2_SF_CAT"]
if os.environ.get("K2_SF_SCOPE"):
    b["scope"] = os.environ["K2_SF_SCOPE"]
if os.environ.get("K2_SF_COMMAND"):
    b["command"] = os.environ["K2_SF_COMMAND"]
if os.environ.get("K2_SF_LOCALE"):
    b["locale"] = os.environ["K2_SF_LOCALE"]
if os.environ.get("K2_SF_DRAFT"):
    b["diagnostics"] = {"draftId": os.environ["K2_SF_DRAFT"]}
print(json.dumps(b))
')"
    _compute_call POST /cli/product-feedback/send "$payload"
    local id
    id="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
    [ -n "$id" ] || { echo '{"error":"daemon_error","hint":"the daemon answered without an id"}' >&2; exit 3; }
    # The daemon sends right away; wait a moment to say what happened.
    local i=0 st="queued" show=""
    while [ "$i" -lt 10 ]; do
        _compute_call GET /cli/product-feedback/show "id=$(urlencode "$id")"
        show="$_CP_BODY"
        st="$(printf '%s' "$show" | python3 -c 'import json,sys; print(json.load(sys.stdin)["item"]["state"])')"
        case "$st" in queued|sending) ;; *) break ;; esac
        # Still queued with a reason: the attempt is over for now.
        if [ "$st" = queued ] && printf '%s' "$show" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin)["item"].get("error") else 1)'; then
            break
        fi
        sleep 0.5
        i=$((i + 1))
    done
    if [ "$json_out" = 1 ]; then
        printf '%s' "$show" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["item"]))'
        return 0
    fi
    printf '%s' "$show" | python3 -c '
import json, sys
it = json.load(sys.stdin)["item"]
st = it["state"]
if st == "sent":
    print("Sent to Alakazam Labs: %s (%s)." % (it["id"], it["category"]))
elif st == "refused":
    print("Refused by the feedback service: %s (%s). It will not be retried." % (it["id"], it.get("errorMessage") or it.get("error") or ""))
else:
    print("Saved on this server: %s (%s). It sends when it can." % (it["id"], it["category"]))
    if it.get("errorMessage"):
        print("  " + it["errorMessage"])
    print("  Check it with: k2 send-feedback show %s" % it["id"])
if it.get("hasDiagnostics"):
    print("  The K2 details you approved are included.")
'
}

_sf_list() {
    local state="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --state) [ $# -ge 2 ] || { echo "usage: --state <state>" >&2; exit 2; }; state="$2"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _send_feedback_usage; return 0 ;;
            *) echo "usage: k2 send-feedback list [--state S] [--json]" >&2; exit 2 ;;
        esac
    done
    local q=""
    [ -n "$state" ] && q="state=$(urlencode "$state")"
    _compute_call GET /cli/product-feedback/list "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
items = v.get("items") or []
if not items:
    print("No feedback to K2 sent from here yet." if not v.get("all") else "No feedback to K2 on this server yet.")
    sys.exit(0)
for it in items:
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(it["createdAt"] / 1000))
    text = " ".join(it.get("text", "").split())
    if len(text) > 60:
        text = text[:57] + "..."
    who = "" if it.get("mine") else "  [" + it.get("sender", "") + "]"
    scope = "  (scope %s)" % it["scope"] if it.get("scope") else ""
    print("%s  %-9s %-6s %s  %s%s%s" % (it["id"], it["state"], it["category"], when, text, who, scope))
    if it.get("errorMessage") and it["state"] in ("queued", "failed", "refused"):
        print("    " + it["errorMessage"])
'
}

_sf_show() {
    local id="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _send_feedback_usage; return 0 ;;
            -*) echo "usage: k2 send-feedback show <id> [--json]" >&2; exit 2 ;;
            *) [ -z "$id" ] || { echo "usage: k2 send-feedback show <id> [--json]" >&2; exit 2; }; id="$1"; shift ;;
        esac
    done
    [ -n "$id" ] || { echo "usage: k2 send-feedback show <id> [--json]" >&2; exit 2; }
    _compute_call GET /cli/product-feedback/show "id=$(urlencode "$id")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
it = json.load(sys.stdin)["item"]
def t(ms):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ms / 1000)) if ms else "-"
print("%s  %s  %s" % (it["id"], it["state"], it["category"]))
print("  from:     %s (%s, %s)" % (it.get("sender", ""), it.get("senderKind", ""), it.get("client", "")))
if it.get("scope"):
    print("  scope:    %s" % it["scope"])
print("  created:  %s" % t(it.get("createdAt")))
if it.get("sentAt"):
    print("  sent:     %s (Alakazam id %s)" % (t(it["sentAt"]), it.get("remoteId") or "-"))
elif it["state"] == "queued":
    print("  next try: %s (attempts so far: %s)" % (t(it.get("nextAttemptAt")), it.get("attempts", 0)))
if it.get("errorMessage"):
    print("  reason:   %s" % it["errorMessage"])
if it.get("contactEmail"):
    print("  contact:  %s" % it["contactEmail"])
print()
print(it.get("text", ""))
d = it.get("diagnostics")
if d:
    print()
    print("K2 details included:")
    for s in d:
        print("── %s ──" % s.get("title", ""))
        print(s.get("text", ""))
'
}

_sf_transition() {
    local verb="$1" id="${2:-}"
    [ -n "$id" ] && [ $# -eq 2 ] || { echo "usage: k2 send-feedback $verb <id>" >&2; exit 2; }
    case "$id" in -h|--help) _send_feedback_usage; return 0 ;; esac
    _compute_call POST "/cli/product-feedback/$verb" "$(_compute_json "K2_CJ_id=$id")"
    case "$verb" in
        retry) echo "Queued again: $id. It sends when it can (k2 send-feedback show $id)." ;;
        cancel) echo "Cancelled: $id. It will not be sent." ;;
    esac
}

# `--agents` as one word: off, on (the 0.46.11 word, kept as cli), cli or
# all. Anything else is a usage mistake.
_sf_agents_word() {
    case "$1" in
        off|false|no|0) echo off ;;
        on|true|yes|1) echo on ;;
        cli|all) echo "$1" ;;
        *) echo "usage: --agents takes off, cli or all" >&2; exit 2 ;;
    esac
}

# The settings read and the agent opt-in write, one function for
# `k2 send-feedback settings` and `k2 setup agent-reports` (the same POST to
# /cli/product-feedback/settings).
#   _sf_settings_run settings|setup [flags]
# setup takes only --agents, --attach and --json.
_sf_settings_run() {
    local who="$1"; shift
    local enabled="" agents="" attach="" have_attach=0 agent_diag="" json_out=0 usage
    if [ "$who" = setup ]; then
        usage="usage: k2 setup agent-reports [--agents off|cli|all] [--attach <parts>|none|all] [--json]"
    else
        usage="usage: k2 send-feedback settings [--enabled on|off] [--agents off|cli|all] [--attach <parts>|none|all] [--agent-diagnostics on|off] [--json]"
    fi
    while [ $# -gt 0 ]; do
        case "$1" in
            --enabled|--agent-diagnostics)
                [ "$who" = settings ] || { echo "$usage" >&2; exit 2; }
                [ $# -ge 2 ] || { echo "usage: $1 on|off" >&2; exit 2; }
                if [ "$1" = --enabled ]; then enabled="$(_sf_onoff "$2" --enabled)"; else agent_diag="$(_sf_onoff "$2" --agent-diagnostics)"; fi
                shift 2 ;;
            --agents) [ $# -ge 2 ] || { echo "usage: --agents off|cli|all" >&2; exit 2; }; agents="$(_sf_agents_word "$2")"; shift 2 ;;
            --attach)
                [ $# -ge 2 ] && [ -n "${2//[[:space:]]/}" ] || { echo "usage: --attach <parts joined by commas>|none|all (k2 send-feedback settings lists the parts)" >&2; exit 2; }
                attach="$2"; have_attach=1; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) if [ "$who" = setup ]; then _setup_usage; else _send_feedback_usage; fi; return 0 ;;
            *) echo "$usage" >&2; exit 2 ;;
        esac
    done
    if [ "$have_attach" = 1 ] && [ -n "$agent_diag" ]; then
        echo "usage: --attach (the parts) or --agent-diagnostics (all or none), not both" >&2; exit 2
    fi
    if [ -n "$enabled$agents$agent_diag" ] || [ "$have_attach" = 1 ]; then
        # cli, all and --attach need a 0.46.17 server; on reads as cli there.
        # Which server this is: its GET names agentsAsked and attachParts.
        local new=1
        if [ "$have_attach" = 1 ] || [ "$agents" = on ] || [ "$agents" = cli ] || [ "$agents" = all ]; then
            _compute_call GET /cli/product-feedback/settings ""
            case "$_CP_BODY" in *'"agentsAsked"'*|*'"attachParts"'*) new=1 ;; *) new=0 ;; esac
        fi
        if [ "$new" = 0 ] && { [ "$have_attach" = 1 ] || [ "$agents" = cli ] || [ "$agents" = all ]; }; then
            echo "This server's $_SF_PRODUCT is older: --agents cli|all and --attach need its update (k2 update). Nothing was changed." >&2
            exit 1
        fi
        local payload="" sep=""
        if [ -n "$enabled" ]; then payload="${payload}${sep}\"enabled\": $enabled"; sep=", "; fi
        case "$agents" in
            off) payload="${payload}${sep}\"agents\": false"; sep=", " ;;
            on)
                if [ "$new" = 1 ]; then payload="${payload}${sep}\"agents\": true, \"agentPlatform\": false"; else payload="${payload}${sep}\"agents\": true"; fi
                sep=", " ;;
            cli) payload="${payload}${sep}\"agents\": true, \"agentPlatform\": false"; sep=", " ;;
            all) payload="${payload}${sep}\"agents\": true, \"agentPlatform\": true"; sep=", " ;;
        esac
        if [ "$have_attach" = 1 ]; then
            _setup_jstr "$attach"
            payload="${payload}${sep}\"agentAttach\": ${_SETUP_J}"; sep=", "
        fi
        if [ -n "$agent_diag" ]; then payload="${payload}${sep}\"agentDiagnostics\": $agent_diag"; sep=", "; fi
        _compute_call POST /cli/product-feedback/settings "{${payload}}"
    else
        _compute_call GET /cli/product-feedback/settings ""
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_SF_PRODUCT="$_SF_PRODUCT" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
product = os.environ["K2_SF_PRODUCT"]
oo = lambda b: "on" if b else "off"
print("Send feedback to %s on this server" % product)
print("  enabled:            %s" % oo(v.get("enabled")))
if "agentsAsked" in v or "attachParts" in v:
    if not v.get("agents"):
        what = "nothing (off)"
    elif v.get("agentPlatform"):
        what = "anything about %s" % product
    else:
        what = "problems with %s commands" % product
    print("  agents report:      %s" % what)
    if v.get("agentsAsked"):
        by = v.get("agentsAskedBy") or "-"
        by = "the owner" if by == "owner" else by
        at = v.get("agentsAskedAt")
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(at / 1000)) if isinstance(at, (int, float)) else "-"
        print("  answered:           by %s, %s" % (by, when))
    else:
        print("  answered:           not asked yet")
    allowed = set(v.get("agentAttach") or [])
    print("  agents may attach:  (the versions always go)")
    for p in v.get("attachParts") or []:
        print("    %-3s  %-13s  %s" % (oo(p["id"] in allowed), p["id"], p.get("label", "")))
else:
    print("  agents:             %s" % oo(v.get("agents")))
    print("  agent diagnostics:  %s" % oo(v.get("agentDiagnostics")))
print("  can send now:       %s" % ("yes" if v.get("canSend") else "no"))
if v.get("message"):
    print("    " + v["message"])
svc = v.get("service") or {}
if svc.get("message"):
    print("  service:            " + svc["message"])
if v.get("pending"):
    print("  waiting to send:    %s" % v["pending"])
print("  sends to:           %s" % v.get("endpoint", ""))
'
}

_sf_settings() { _sf_settings_run settings "$@"; }

cmd_send_feedback() {
    _K2_REFUSAL_AREA=""
    case "${1:-}" in
        "") _send_feedback_usage >&2; exit 2 ;;
        -h|--help|help) _send_feedback_usage; return 0 ;;
        list) shift; _sf_list "$@" ;;
        show) shift; _sf_show "$@" ;;
        retry) shift; _sf_transition retry "$@" ;;
        cancel) shift; _sf_transition cancel "$@" ;;
        settings) shift; _sf_settings "$@" ;;
        *) _sf_send "$@" ;;
    esac
}

# `k2 app status <app-id> [--json] [--no-checks] [--public]` (TC-15).
_app_status_usage() {
    cat <<'EOF'
k2 app status <app-id> [--json] [--no-checks] [--public]

How an App is doing, in one answer: everything you would otherwise check
one command at a time after a deploy.

  deploy     the live Seed (version, commit, who, when) and the last deploy's result
  health     a health check now (status and time), uptime, restarts
  checks     the App's own host.checks from its manifest, run now
  errors     failures in the last hour, by kind (crash, health, build, deploy)
  resources  the App's disk and memory, and the box's disk level
  door       outside hosts refused in the last hour; requests waiting for the owner
  hosts      its label, URL and custom domains
  phone      phone pairing, push keys (set or not) and devices
  updates    the App's desktop update feed (on servers that have one)
  next       one exact command for each red or yellow item

  --no-checks  don't run host.checks; show the run recorded when the live
               deploy switched in (it may be old)
  --public     run host.checks through the App's public URL (proves TLS
               and the relay), not on this machine. Not with --no-checks.
  --json       {schema: 1, verdict, deploy, health, checks, errors,
                resources, door, hosts, phone, updates, next: [{level,
                part, what, run}]}

Exit codes: 0 green (all good); 1 red (the health check failing, a
host.check failed, crash looping, nothing live after a deploy); 2 yellow
and nothing red (a deploy in progress, nothing deployed yet, the last
deploy failed while the previous version still serves, requests waiting
for the owner, failures in the last hour); 3 refused (no such App, or not
yours: k2 explain <id>). A usage mistake also exits 2, with a usage: line.
Text the App itself produced is never printed: checks say only whether
the status, header and body matched.
EOF
}

cmd_app_status() {
    local app="" json_out=0 nochecks=0 public=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --no-checks) nochecks=1; shift ;;
            --public) public=1; shift ;;
            -h|--help) _app_status_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; echo "usage: k2 app status <app-id> [--json] [--no-checks] [--public]" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app status <app-id> [--json] [--no-checks] [--public]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app status <app-id> [--json] [--no-checks] [--public]" >&2; exit 2; }
    [ "$nochecks" = 1 ] && [ "$public" = 1 ] && { echo "k2 app status: --public runs the checks; it can't go with --no-checks" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ "$nochecks" = 1 ] && q="$q&noChecks=1"
    [ "$public" = 1 ] && q="$q&public=1"
    _compute_call GET /cli/app/status "$q"
    local verdict
    verdict="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("verdict",""))')"
    if [ "$json_out" = 1 ]; then
        printf '%s\n' "$_CP_BODY"
    else
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d, h, c = v.get("deploy") or {}, v.get("health") or {}, v.get("checks") or {}
print("%s  %s  (%s)" % (v["app"], v.get("verdict", "").upper(), d.get("state", "")))
if d.get("stateReason"):
    print("  why        %s" % d["stateReason"])
seed, live, last = d.get("seed") or {}, d.get("live") or {}, d.get("last") or {}
if seed:
    print("  deploy     Seed %s%s, live since %s by %s, commit %s" % ((seed.get("hash") or "")[:12], (" v" + seed["version"]) if seed.get("version") else "",
          live.get("liveAt") or "-", live.get("actor") or "-", (live.get("commit") or "-")[:12]))
else:
    print("  deploy     nothing deployed yet")
if last:
    err = (": " + last["error"]) if last.get("error") else ""
    print("             last: %s %s %s%s" % (last.get("kind", ""), last.get("id", ""), last.get("result", ""), err[:160]))
p = h.get("probe")
if p:
    st = p.get("status") if p.get("status") is not None else "no answer"
    print("  health     GET %s: %s in %s ms; up %ss; restarts %s" % (h.get("path"), st, p.get("ms"), h.get("uptimeSecs"), h.get("recentRestarts")))
else:
    print("  health     nothing live to probe")
if c.get("results"):
    print("  checks     %s of %s failed (%s)" % (c.get("failed", 0), c.get("declared"), c.get("source", "")))
    for r in c["results"]:
        got = r.get("status") if r.get("status") is not None else "no answer"
        extra = "" if r.get("ok") else ("  ✗ %s" % r.get("why"))
        print("             %-16s GET %s: expect %s, got %s%s" % (r["name"], r["path"], r["expect"], got, extra))
elif c.get("recorded"):
    rec = c["recorded"].get("results") or []
    print("  checks     recorded at the switch: %s of %s failed" % (c.get("failed"), len(rec)))
elif c.get("error"):
    print("  checks     %s" % c["error"])
else:
    print("  checks     none declared (host.checks in the manifest)")
e = v.get("errors") or {}
if "unavailable" in e:
    print("  errors     unavailable (%s)" % e["unavailable"])
else:
    by = ", ".join("%s %s" % (k, n) for k, n in (e.get("byKind") or {}).items()) or "none"
    print("  errors     last hour: %s" % by)
r = v.get("resources") or {}
if "unavailable" in r:
    print("  resources  unavailable (%s)" % r["unavailable"])
else:
    a = r.get("app") or {}
    disk = a.get("diskBytes")
    print("  resources  disk %s; box disk level %s" % ("%.1f MB" % (disk / 1e6) if isinstance(disk, (int, float)) else "not measured yet", r.get("level")))
dr = v.get("door") or {}
print("  door       egress refused last hour: %s; waiting for the owner: %s" % (dr.get("egressDenied"), dr.get("heldApprovals")))
ho = v.get("hosts") or {}
doms = ho.get("domains") or {}
nd = len(doms.get("domains") or []) if isinstance(doms, dict) else 0
print("  hosts      %s%s" % (ho.get("url") or ho.get("label") or "-", ("; %s custom domain(s)" % nd) if nd else ""))
ph = v.get("phone") or {}
pu, pa = ph.get("push") or {}, ph.get("pairing") or {}
keys = pu.get("keys") or {}
print("  phone      pairing %s; push %s (APNs key %s, FCM key %s, %s device(s))" % ("on" if pa.get("on") else "off", "on" if pu.get("on") else "off",
      "set" if keys.get("apns") else "not set", "set" if keys.get("fcm") else "not set", pu.get("devices", 0)))
u = v.get("updates") or {}
print("  updates    feed %s" % (u.get("feed") or "-"))
nx = v.get("next") or []
if nx:
    print("next")
    for n in nx:
        print("  [%s] %s: %s" % (n["level"], n["what"], n["run"]))
'
    fi
    case "$verdict" in
        green) return 0 ;;
        yellow) exit 2 ;;
        red) exit 1 ;;
        *) echo "k2 app status: the daemon gave no verdict" >&2; exit 1 ;;
    esac
}

