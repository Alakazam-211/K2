# ── k2 postal (Beta, prd-postal-in-k2-v1 S0, S3, S4, S5) ─────────────
# Postal lets an agent here be friends with agents on other machines
# (addresses like scout::acme.postal.bot), one agent with one agent,
# behind the owner's Postal switch (off by default). The server talks to
# the Postal network with its K2 Connect account. Every step an agent
# takes to befriend another is a card for its person (exit 2, "Asked: …"):
# turning Postal on, its address, an invite, accepting a request, the p5
# link. Removing, blocking, rejecting and quieter routing are its own.
# Waits on others (never pretended): live delivery to this server (mail
# comes by the network's hold) and adopting p5's handles.

_postal_usage() {
    cat <<EOF
Usage: k2 postal <command> [--workspace <ws>] [--json]

Postal (Beta): friends for an agent here among agents on other machines.
Addresses look like handle::label.postal.bot: the handle is the agent, the
label is its server's name on Postal (ask your person which label is
yours). One address per workspace; sidecars and subagents share it. A
friendship needs both sides: a person on each machine accepts, typing
the 6-digit code shown on the other machine.

Getting started (an agent; each step is a card your person answers):
  1. k2 postal enable [--why "…"]     ask the server's owner to turn Postal on
                                      (off by default; until then the other
                                      commands answer postal_off, exit 3, so
                                      wait for this answer before step 2)
  2. k2 postal claim <address> [--why "…"]
                                      ask for this workspace's Postal address
                                      (an Owner or Admin allows it)
  3. k2 postal invite <address> --why "what you want to do together"
                                      ask to be friends with that agent (an Owner
                                      or Admin allows it)
  Each prints "Asked: …" and exits 2: the card is on your person's Thread,
  and the answer comes back to you as a [permission:] turn. Asking again
  while it waits finds the same card. An "Ask with: k2 permission request
  postal.…" line under it names that same card: you don't need to run it.

Every day:
  k2 postal status                   your address; counts of friends and requests
  k2 postal friends                  friends and requests (in, out, history)
  k2 postal accept <request id>      ask your person to accept a request: puts
                                      its card up again (you can't accept one; the
                                      Owner does, or an Admin if the Owner allows it)
  k2 postal reject <request id>      turn an incoming request down yourself
  k2 postal remove <address>         end a friendship or withdraw a request
  k2 postal block <address>          refuse that address from now on
  k2 msg <address> "text" [--no-wake] [--json]
                                      Postal mail to a friend, from this
                                      workspace's address (exit 0 queued, held or
                                      delivered; 3 not a friend or the text holds a
                                      vault secret's value)
  k2 postal route [--friend <address>] terminal|inbox|inbox+pointer|thread:<person>… [--no-wake]
                                      where Postal mail lands (default: terminal,
                                      waking you); you may only make yours quieter
  k2 postal route --clear [--friend <address>]   back to the default

People (an Owner or Admin) do the same directly, or in the workspace's Postal tab:
  k2 postal claim <address> --workspace <ws>
  k2 postal code <request id>        this server's 6-digit code: read it to the
                                      other side
  k2 postal accept <request id> --code <their code> [--workspace <ws>]
                                      accept, typing the code shown on the OTHER
                                      side's screen (this server checks it, then
                                      signs; a code that doesn't match fails the
                                      request on both sides)
  k2 postal owner-link [--token-stdin]
                                      the Owner, once: link Postal approvals for
                                      this server (opens a page to approve; or hand
                                      over a k2c_pd_ token approved elsewhere)
  k2 postal enable | disable         the server's switch (the Owner)
  k2 postal admins-may-accept on|off let Admins answer incoming requests too (the
                                      Owner); claim and invite are already an
                                      Owner's or Admin's
  k2 postal settings                 show both

p5, Postal's own client on this machine (works while Postal is off):
  k2 postal link-p5                  a card: the owner's Allow makes a key that can
                                      only message this workspace and hands it to p5

Not available yet:
  live delivery to this server        mail comes by the Postal network's hold
                                      (within about a minute) until the network
                                      routes this server's name to it
  k2 postal adopt                    taking over p5's handles

Incoming requests never wake you: K2 puts a card on your person's Thread,
and its answer doesn't wake you either (k2 postal friends shows it).
Postal mail arrives as [postal <address> · p_<code>] with every line quoted
(long or busy: in your inbox with an [inbox:<id>] pointer); check one with
k2 msg verify <code> --text-file f.
Exit codes: 0 done; 2 asked (stdout starts "Asked:") or a usage error (an
"Error:" or "Usage:" line on stderr); 3 refused (the lines under the error
say what to do); 4 not found; 1 anything else.
k2 study postal says more.
EOF
}

# _postal_body <key=value>...: a JSON object of the pairs (empty values dropped).
_postal_body() {
    python3 -c '
import json, sys
out = {}
for a in sys.argv[1:]:
    k, _, v = a.partition("=")
    if v != "":
        out[k] = True if v == "__true" else False if v == "__false" else v
print(json.dumps(out))
' "$@"
}

# Where a person takes the steps a session may not take as the owner.
_POSTAL_PERSON_LINE="A person does this in the workspace's Postal tab or in Settings → General → Postal, or with k2 postal from a terminal outside K2."

# _postal_session_token: in a session (the HOOK_SOCK marker), Postal acts
# as the session: every request carries the session's passport, never the
# owner token k2 read from disk. A session with no passport: one line, exit
# 3, nothing sent. Outside a session (and on a remote host) $TOKEN stays.
_postal_session_token() {
    [ -n "$(_hook_sock)" ] || return 0
    _cli_skip_uds && return 0
    if [ -z "${SCOPED_TOKEN:-}" ]; then
        echo "Error: in a session, k2 postal doesn't act as this server's owner, and this session has no passport, so nothing was sent." >&2
        echo "$_POSTAL_PERSON_LINE" >&2
        exit 3
    fi
    TOKEN="$SCOPED_TOKEN"
}

# _postal_print <view> <json?>: the daemon's answer, for people and agents.
# In a session, a step that is a person's ends with where a person takes it
# (unless the daemon's own line already says).
_postal_print() {
    local in_session=""
    [ -n "$(_hook_sock)" ] && in_session=1
    PV="$1" PJ="${2:-false}" PS="$in_session" PL="$_POSTAL_PERSON_LINE" python3 -c '
import json, os, sys, time
raw = sys.stdin.read()
if raw == "\x1ek2:not-answered\x1e":
    sys.exit(1)  # the send said why and is ending the command
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon answered: %s" % raw[:300], file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err:
    code = err.get("code", "") if isinstance(err, dict) else ""
    hint = err.get("hint", "") if isinstance(err, dict) else str(err)
    print("Error: %s%s" % ((code + ": ") if code else "", hint), file=sys.stderr)
    if os.environ.get("PS") and code in ("owner_only", "postal_people_only") and "Postal tab" not in hint and "Settings" not in hint:
        print(os.environ["PL"], file=sys.stderr)
    perm = ("owner_only", "forbidden", "postal_off", "postal_people_only", "postal_people_decide", "postal_owner_decides", "postal_own_workspace_only", "role_required", "postal_not_friends", "postal_secret_in_text", "postal_route_louder", "postal_owner_link_missing", "postal_rekey_pending")
    sys.exit(3 if code in perm else {"not_found": 4, "postal_no_address": 4, "postal_no_code": 4, "usage": 2, "bad_postal_address": 2, "postal_code_needed": 2, "too_large": 2}.get(code, 1))
if os.environ.get("PJ") == "true":
    print(json.dumps(d, indent=2)); sys.exit(0)
v = os.environ.get("PV")
def when(t):
    return time.strftime("%Y-%m-%d %H:%M", time.localtime(t)) if t else "-"
if d.get("asked"):
    print("Asked: a card is on your person'"'"'s Thread (%s)%s." % (d.get("request") or d.get("card"), ", waiting already" if d.get("waiting") else ""))
    hint = d.get("hint") or "a person answers the card; the answer comes back to you as a [permission:] turn"
    print(hint[0].upper() + hint[1:] + ".")
    sys.exit(2)
if v == "settings":
    print("Postal: %s" % ("on" if d.get("enabled") else "off"))
    print("Admins may accept friend requests: %s" % ("yes" if d.get("adminsMayAccept") else "no (the Owner only)"))
    sys.exit(0)
h = d.get("handle")
ws = (d.get("workspace") or {}).get("handle") or "this workspace"
if v == "send":
    st = d.get("status")
    print("%s to %s: %s%s" % ("Postal mail" if st != "failed" else "Not sent", d.get("to"), st, (" (" + d["reason"] + ")") if d.get("reason") and st == "failed" else ""))
    if d.get("badge"): print("Badge:    %s" % d["badge"])
    if d.get("deliveryId"): print("Record:   %s (k2 msg verify %s)" % (d["deliveryId"], d["deliveryId"]))
    if st == "queued": print(d.get("hint") or "Queued on this server; it goes out at the next try.")
    if st == "held": print("Held on the Postal network until their server picks it up (up to 7 days).")
    if st == "delivered": print("Their server stored it.")
    sys.exit(1 if st == "failed" else 0)
if v == "code":
    print("Your code: %s" % d.get("code"))
    print("Read it to the other side, and type the code shown on their screen:")
    print("  k2 postal accept %s --code <their code>" % d.get("id"))
    sys.exit(0)
if v == "route":
    r = d.get("route")
    who = d.get("friend") or "this workspace"
    print("Postal mail from %s lands: %s" % (who, ("default (terminal, waking)" if not r else " + ".join(r.get("targets", [])) + ("" if r.get("wake", True) else ", no wake"))))
    sys.exit(0)
if v == "owner-link":
    if d.get("linked"):
        print("Postal approvals linked for %s." % d.get("label")); sys.exit(0)
    print("Open this page and approve (as the owner of %s):" % d.get("label"))
    print("  %s" % d.get("url"))
    print("Code on the page: %s. This server picks the approval up by itself." % d.get("userCode"))
    sys.exit(0)
if v in ("status", "claim"):
    if d.get("already"):
        print("This workspace has that address already.")
    if h:
        print("Address:  %s (%s)" % (h.get("address"), "kept here; it goes on the Postal network once this server is connected" if h.get("status") == "local" else "on the Postal network" if h.get("status") == "claimed" else h.get("status")))
    else:
        print("Address:  none yet for %s (a person claims one: k2 postal claim <handle::label.postal.bot>)" % ws)
    c = d.get("counts")
    if c:
        print("Friends:  %d   requests in: %d   out: %d" % (c.get("accepted", 0), c.get("pendingIn", 0), c.get("pendingOut", 0)))
    if d.get("ownerLinked") is False:
        print("Approvals: not linked yet; the owner runs k2 postal owner-link once")
    note = (d.get("plane") or {}).get("note")
    if note: print(note)
    sys.exit(0)
if v == "friends":
    rows = d.get("friends") or []
    if not h:
        print("%s has no Postal address yet." % ws); sys.exit(0)
    print("Address: %s" % h.get("address"))
    groups = [("Friends", ("accepted", "rekey_pending", "blocked")), ("Requests in (a person answers)", ("pending_in",)), ("Requests out (waiting)", ("pending_out",)), ("History", ("rejected", "revoked"))]
    for title, sts in groups:
        g = [r for r in rows if r.get("status") in sts]
        if not g: continue
        print("%s:" % title)
        for r in g:
            extra = []
            if r.get("status") == "blocked": extra.append("blocked")
            if r.get("status") == "rekey_pending": extra.append("key changed: waits for a person")
            if r.get("ownerName"): extra.append("owner " + r["ownerName"])
            if r.get("status") in ("pending_in", "pending_out", "rekey_pending"): extra.append("id " + r["id"])
            if r.get("codeWaiting"): extra.append("pairing code waiting: a person types theirs")
            if r.get("status") == "pending_out" and r.get("decidedBy"): extra.append("you accepted; waiting for their side")
            print("  %s  %s%s" % (r.get("peer"), when(r.get("updatedAt")), ("  (" + ", ".join(extra) + ")") if extra else ""))
    if not rows: print("No friends or requests yet.")
    sys.exit(0)
f = d.get("friend")
if f:
    print("%s: %s" % (f.get("peer"), f.get("status")))
    if d.get("friends"):
        print("Friends now.")
    elif d.get("hint"):
        print(d["hint"][0].upper() + d["hint"][1:] + ".")
    elif f.get("status") == "pending_out" and v == "invite":
        print("Sent over the Postal network." if d.get("sent") else "Recorded; it goes out as soon as this server is connected to the Postal network.")
    sys.exit(0)
print(json.dumps(d))
'
}

cmd_postal() {
    local sub="${1:-}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        ""|-h|--help|help) _postal_usage; return 0 ;;
    esac
    local ws="" as_json=false why="" code="" friend="" nowake=false clear=false token_stdin=false args=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --workspace) [ $# -ge 2 ] || { echo "k2 postal: --workspace needs a name" >&2; exit 2; }; ws="$2"; shift 2 ;;
            --why) [ $# -ge 2 ] || { echo "k2 postal: --why needs the reason" >&2; exit 2; }; why="$2"; shift 2 ;;
            --json) as_json=true; shift ;;
            --code) [ $# -ge 2 ] || { echo "k2 postal: --code needs the 6 digits shown on the other side" >&2; exit 2; }; code="$2"; shift 2 ;;
            --friend) [ $# -ge 2 ] || { echo "k2 postal: --friend needs an address" >&2; exit 2; }; friend="$2"; shift 2 ;;
            --no-wake) nowake=true; shift ;;
            --clear) clear=true; shift ;;
            --token-stdin) token_stdin=true; shift ;;
            -h|--help) _postal_usage; return 0 ;;
            -*) echo "k2 postal: unknown flag '$1' (k2 postal --help)" >&2; exit 2 ;;
            *) args+=("$1"); shift ;;
        esac
    done
    [ -n "$ws" ] || ws="${PROJECT:-$PWD}"
    local one="${args[0]:-}"
    _postal_need() {
        [ -n "$one" ] || { echo "Usage: k2 postal $sub $1  (k2 postal --help)" >&2; exit 2; }
    }
    # Every verb that reaches the daemon acts as the session in a session.
    case "$sub" in
        status|friends|claim|invite|accept|reject|code|route|owner-link|send|remove|block|enable|disable|admins-may-accept|settings|link-p5)
            _postal_session_token ;;
    esac
    case "$sub" in
        status|friends)
            _K2_PIPE_MARK=1 cli_send cli_request "/cli/postal/$sub" "workspace=$(urlencode "$ws")" | _postal_print "$sub" "$as_json" ;;
        claim)
            _postal_need "<handle::label.postal.bot> [--why \"…\"]"
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/claim" "$(_postal_body "workspace=$ws" "address=$one" "why=$why")" | _postal_print claim "$as_json" ;;
        invite)
            _postal_need "<handle::label.postal.bot> --why \"what you want to do together\""
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/invite" "$(_postal_body "workspace=$ws" "to=$one" "why=$why")" | _postal_print invite "$as_json" ;;
        accept|reject)
            _postal_need "<request id>  (k2 postal friends shows the ids)"
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/decide" "$(_postal_body "workspace=$ws" "id=$one" "decision=$sub" "code=$code")" | _postal_print decide "$as_json" ;;
        code)
            _postal_need "<request id>  (k2 postal friends shows the ids)"
            cli_request "/cli/postal/code" "workspace=$(urlencode "$ws")&id=$(urlencode "$one")" | _postal_print code "$as_json" ;;
        route)
            local targets="${args[*]:-}" wakev="__true"
            $nowake && wakev="__false"
            if $clear; then
                _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/route" "$(_postal_body "workspace=$ws" "friend=$friend" "clear=__true")" | _postal_print route "$as_json"
            else
                [ -n "$targets" ] || { echo "Usage: k2 postal route [--friend <address>] terminal|inbox|inbox+pointer|thread:<person>… [--no-wake] | --clear" >&2; exit 2; }
                _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/route" "$(_postal_body "workspace=$ws" "friend=$friend" "targets=$targets" "wake=$wakev")" | _postal_print route "$as_json"
            fi ;;
        owner-link)
            if $token_stdin; then
                local tok
                IFS= read -r tok || true
                [ -n "$tok" ] || { echo "k2 postal owner-link: no token on stdin" >&2; exit 2; }
                cli_post_json "/cli/postal/owner-link" "$(_postal_body "token=$tok")" | _postal_print owner-link "$as_json"
            else
                cli_post_json "/cli/postal/owner-link" "{}" | _postal_print owner-link "$as_json"
            fi ;;
        send)
            _postal_need "<handle::label.postal.bot> \"text\""
            local extra=()
            $nowake && extra+=(--no-wake)
            $as_json && extra+=(--json)
            _is_postal_address "$one" || { echo "k2 postal send: '$one' isn't a Postal address (handle::label.postal.bot)" >&2; exit 2; }
            _postal_send_teach "${args[@]}" ${extra[@]+"${extra[@]}"} ;;
        remove|block)
            _postal_need "<handle::label.postal.bot>"
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/$sub" "$(_postal_body "workspace=$ws" "peer=$one")" | _postal_print "$sub" "$as_json" ;;
        enable)
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/enable" "$(_postal_body "why=$why")" | _postal_print settings "$as_json" ;;
        disable)
            cli_post_json "/cli/postal/settings" "$(_postal_body "enabled=__false")" | _postal_print settings "$as_json" ;;
        admins-may-accept)
            case "$one" in
                on) cli_post_json "/cli/postal/settings" "$(_postal_body "adminsMayAccept=__true")" | _postal_print settings "$as_json" ;;
                off) cli_post_json "/cli/postal/settings" "$(_postal_body "adminsMayAccept=__false")" | _postal_print settings "$as_json" ;;
                *) echo "Usage: k2 postal admins-may-accept on|off" >&2; exit 2 ;;
            esac ;;
        settings)
            cli_request "/cli/postal/settings" | _postal_print settings "$as_json" ;;
        link-p5)
            _K2_PIPE_MARK=1 cli_send cli_post_json "/cli/postal/link-p5" "$(_postal_body "workspace=$ws")" | _postal_print link "$as_json" ;;
        adopt)
            # S7 waits on Postal Bot (p5 standing down). Never pretend.
            echo "k2 postal adopt: not available yet: p5 keeps its own handles; k2 postal link-p5 lets it deliver to this workspace." >&2
            exit 1 ;;
        *) echo "k2 postal: unknown command '$sub' (k2 postal --help)" >&2; exit 2 ;;
    esac
}

_study_page_postal() {
    cat <<'EOF'
%include cli/text/study/postal.txt
EOF
}
