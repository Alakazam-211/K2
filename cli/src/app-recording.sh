# ── Recording in Apps (Sew 0.9.0, K2 0.46.11) ─────────────────────────────
# k2 app recording <app> [set …], k2 hostchat app-space recording[-lock],
# k2 hostchat recording policy, k2 voice settings. The daemon decides who
# may change what; a widening answers vault_confirm_required (an App's own
# settings) or owner_confirm_required (Chat Hosting's), and a terminal
# outside K2 then gets the person's confirm and sends the change again.

# Did the last answer ask for a person confirm this terminal can give?
_rec_needs_confirm() {
    [ "$_CP_STATUS" = "403" ] || return 1
    [[ "$_CP_BODY" == *'"vault_confirm_required"'* || "$_CP_BODY" == *'"owner_confirm_required"'* ]] || return 1
    ! _compute_in_session
}

# _rec_send_confirmed PATH BODY ACTION SECRET-ID LABEL: the person's confirm
# (Touch ID or the vault password), then the same body with confirmToken.
_rec_send_confirmed() {
    local path="$1" body="$2" tok raw
    tok="$(_vault_confirm_token "$3" "$4" "$5")" || exit 3
    body="$(printf '%s' "$body" | K2_RC_TOK="$tok" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_RC_TOK"]; print(json.dumps(b))')"
    tok=""
    raw="$(_compute_send POST "$path" "$body" tcp)"
    _compute_split "$raw"
}

# on|off|inherit → a JSON bool or null; empty stays empty (not named).
_rec_switch_json() {
    case "$1" in
        "") ;;
        on|true|yes) echo true ;;
        off|false|no) echo false ;;
        inherit) [ "${2:-}" = inherit ] && echo null || return 1 ;;
        *) return 1 ;;
    esac
}

_app_recording_usage() {
    cat <<'EOF'
usage: k2 app recording <app> [--json]
       k2 app recording <app> set [--start-level none|admin|moderator|member]
                                  [--audio-max-days N|org] [--required on|off]
                                  [--server-read on|off] [--json]
Recording in an App's calls, on this server's chat (Sew 0.9.0 or later).
No flags: what K2 wants for the App, whether Sew has it yet (delivered),
the Spaces the App is bound to, and the lock on each. The owner, an Admin
or the App's own agent may look. <app> is the App's id or name (k2 app list).
Each flag is optional; name one or more.

Every setting starts off: nobody may start a recording, nothing is
required, and the App's own server reads nothing. Each person in a call
still answers their own consent notice; a recording never records someone
who said no.
  --start-level L       the lowest chat level that may start a recording in
                        the App's Spaces: none (nobody, the default), admin,
                        moderator or member
  --audio-max-days N    keep recorded audio N days (1 to 3650; 30 or 30d),
                        never more than this server's own limit
                        (recording.audio_max_days, k2 hostchat recording
                        policy: above it is above_org_audio_max_days);
                        org = the server's limit
  --required on|off     on: every call with two or more people in the App's
                        Spaces is recorded, and nobody can stop it there. Each
                        person still consents for themselves.
  --server-read on|off  on: the App's own server may read its recordings'
                        details, transcripts and changes (the SDK's
                        recordings.list, get, transcript, events). Each
                        consent notice names the App as a reader; every read
                        is logged.
Who: turning a setting down or off: the owner, an Admin, or the App's own
agent with its App switch "…and manage them end to end" on (switch 2:
k2 app access <workspace> shows it; the owner turns it on with --manage on).
A higher start level or longer keep days: the owner or an Admin, in
person, with a confirm (Touch ID or the vault password) from a terminal
outside K2; an agent gets person_only. --required on and --server-read on:
the server's owner only, in person; anyone else gets owner_only. With
permission cards on for its workspace, the agent's refusal ends with the
line to ask the owner on a card (the card's key is server-read, with a dash):
  k2 permission request apps.recording app=<app> server-read=on --why "…"
  k2 permission request apps.recording app=<app> required=on server-read=on --why "…"
Not for the home client (the App that sees every Space): it takes no
recording settings (home_client_cannot_govern). A server whose chat can't
record in Apps yet answers sew_feature_missing (k2 hostchat upgrade).
A Space's own rule, and the owner's no-recording lock for CUI/FCI Spaces:
k2 hostchat app-space recording --help. Uninstalling an App never deletes
its recordings. More: k2 study app-recording.
Exit codes: 0 ok, 2 bad usage, 3 refused.
EOF
}

cmd_app_recording() {
    local app="" set_mode=0 level="" days="" required="" server_read="" json_out=0 days_named=0
    while [ $# -gt 0 ]; do
        case "$1" in
            set) set_mode=1; shift ;;
            --start-level) level="${2:-}"; shift 2 || { echo "usage: --start-level none|admin|moderator|member" >&2; exit 2; } ;;
            --start-level=*) level="${1#*=}"; shift ;;
            --audio-max-days) days="${2:-}"; days_named=1; shift 2 || { echo "usage: --audio-max-days N|org" >&2; exit 2; } ;;
            --audio-max-days=*) days="${1#*=}"; days_named=1; shift ;;
            --required) required="${2:-}"; shift 2 || { echo "usage: --required on|off" >&2; exit 2; } ;;
            --required=*) required="${1#*=}"; shift ;;
            --server-read) server_read="${2:-}"; shift 2 || { echo "usage: --server-read on|off" >&2; exit 2; } ;;
            --server-read=*) server_read="${1#*=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_recording_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app recording --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] && app="$1" || { echo "usage: one App only (k2 app recording --help)" >&2; exit 2; }; shift ;;
        esac
    done
    [ -n "$app" ] || { _app_recording_usage >&2; exit 2; }
    local any=0
    { [ -n "$level" ] || [ "$days_named" = 1 ] || [ -n "$required" ] || [ -n "$server_read" ]; } && any=1
    if [ "$set_mode" = 0 ] && [ "$any" = 1 ]; then
        echo "usage: to change a setting: k2 app recording $app set --start-level … (k2 app recording --help)" >&2; exit 2
    fi
    if [ "$set_mode" = 1 ] && [ "$any" = 0 ]; then
        echo "usage: name a change: k2 app recording $app set [--start-level L] [--audio-max-days N|org] [--required on|off] [--server-read on|off]" >&2; exit 2
    fi
    if [ "$set_mode" = 0 ]; then
        _compute_call GET /cli/app/recording "app=$(urlencode "$app")"
    else
        case "$level" in ""|none|admin|moderator|member) ;; *) echo "usage: --start-level takes none, admin, moderator or member" >&2; exit 2 ;; esac
        local days_j=""
        if [ "$days_named" = 1 ]; then
            days="${days%d}"
            if [ "$days" = org ]; then days_j=null
            elif [[ "$days" =~ ^[0-9]+$ ]] && [ "$((10#$days))" -ge 1 ] && [ "$((10#$days))" -le 3650 ]; then days_j="$((10#$days))"
            else echo "usage: --audio-max-days takes a number of days (1 to 3650) or org (the server's limit)" >&2; exit 2
            fi
        fi
        local req_j srv_j
        req_j="$(_rec_switch_json "$required")" || { echo "usage: --required takes on or off" >&2; exit 2; }
        srv_j="$(_rec_switch_json "$server_read")" || { echo "usage: --server-read takes on or off" >&2; exit 2; }
        local body
        body="$(_compute_json K2_CJ_app="$app" K2_CJ_startLevel="$level" K2_CJ_audioMaxDays_j="$days_j" K2_CJ_required_j="$req_j" K2_CJ_serverRead_j="$srv_j")"
        _app_rt_post /cli/app/recording/set "$body"
        if _rec_needs_confirm; then
            local action svc
            action="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("action") or "")')"
            svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
            [ -n "$action" ] && [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            _rec_send_confirmed /cli/app/recording/set "$body" "$action" "$svc" "Recording settings for $app"
        fi
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_RC_SET="$set_mode" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
a = v.get("app") or {}
f = v.get("flags") or {}
r = v.get("recording") or {}
yn = lambda b: "yes" if b else "no"
onoff = lambda b: "on" if b else "off"
name = a.get("name") or a.get("serviceId")
if os.environ.get("K2_RC_SET") == "1":
    print("%s: %s" % (name, "saved" if v.get("changed") else "nothing changed (already so)"))
print("App %s (%s): recording" % (name, a.get("serviceId")))
print("  Sew flags      record in Apps %s · voice:record cap %s · required %s · server reads %s" % (
    yn(f.get("appRecording")), yn(f.get("voiceRecordCap")), yn(f.get("recordingRequired")), yn(f.get("mgmtRecordingReads"))))
if not f.get("appRecording"):
    print("  This server'"'"'s chat can'"'"'t record in Apps yet (it needs Sew 0.9.0: k2 hostchat upgrade).")
if v.get("homeClient"):
    print("  Not for the home client: this App sees every Space a person is in, so it takes no recording settings.")
    sys.exit(0)
lvl = r.get("start_level") or "none"
print("  start level    %s%s" % (lvl, " (nobody starts a recording)" if lvl == "none" else " and above may start a recording"))
print("  required       %s" % onoff(r.get("required")))
print("  server reads   %s" % onoff(r.get("server_read")))
org = v.get("orgAudioMaxDays")
d = r.get("audio_max_days")
print("  audio kept     %s" % (("%s days" % d) if d is not None else ("the server'"'"'s limit (%s days)" % org if org is not None else "the server'"'"'s limit")))
print("  delivered      %s" % ("yes" if v.get("delivered") else "not yet: K2 is sending it to Sew"))
sp = v.get("spaces") or []
if not sp:
    print("  Spaces         none bound (k2 hostchat app-space bind <app> <space-id>)")
for s in sp:
    if s.get("locked"):
        rule = "LOCKED: no recording (CUI/FCI; the owner lifts it)"
    else:
        req = s.get("required")
        rule = "start level %s, required %s" % (s.get("startLevel") or "inherit", "inherit" if req is None else onoff(req))
    print("  Space %s: governed by %s; %s" % (s.get("spaceId"), s.get("governance") or "sew", rule))'
}

# k2 app account-deletion <app> | complete <app> <request|user> | cancel <app> <request|user>
_app_account_deletion_usage() {
    cat <<'EOF'
usage: k2 app account-deletion <app> [--json]                     pending requests and the settings
       k2 app account-deletion complete <app> <adr_…|username>    delete now (can't be undone)
       k2 app account-deletion cancel <app> <adr_…|username>      undo: their access comes back
A person deletes their own account from inside the App (profile.deleteAccount).
With the App's mode "request" (the default), K2 signs them out and turns their
access off, and the deletion waits here until you complete it or its grace period
ends. The owner or an Admin completes or cancels; so may the App's own agent with
"Let this agent manage its Apps' users" on. Settings: k2 app set <app> --help.
After a deletion, delete what the App keeps for that person in its own data.
More: k2 study app-account-deletion.
EOF
}

cmd_app_account_deletion() {
    local action="list" app="" who="" json_out=0
    case "${1:-}" in
        complete|cancel) action="$1"; shift ;;
        list) shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_account_deletion_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app account-deletion --help)" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; elif [ -z "$who" ]; then who="$1"; else echo "usage: too many arguments (k2 app account-deletion --help)" >&2; exit 2; fi; shift ;;
        esac
    done
    [ -z "$app" ] && { _app_account_deletion_usage >&2; exit 2; }
    if [ "$action" = list ]; then
        [ -n "$who" ] && { echo "usage: k2 app account-deletion <app> (to act on one: complete|cancel <app> <request>)" >&2; exit 2; }
        _compute_call GET /cli/app/account-deletion "app=$(urlencode "$app")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
d = v.get("accountDeletion") or {}
def t(x): return time.strftime("%Y-%m-%d %H:%M", time.localtime(x)) if x else "-"
print("App %s: account deletion %s, grace %s days" % (v.get("app"), d.get("mode"), d.get("graceDays")))
ps = v.get("pending") or []
if not ps:
    print("No pending deletion requests.")
for p in ps:
    print("  %-26s %-20s asked %s  completes %s  (%s)" % (p["id"], p["username"], t(p["requestedAt"]), t(p["dueAt"]), "the login goes" if p.get("scope") == "account" else "only this App access goes"))
if ps:
    print("Complete now: k2 app account-deletion complete %s <request|username>   Undo: ... cancel" % v.get("app"))'
        return 0
    fi
    [ -z "$who" ] && { echo "usage: k2 app account-deletion $action <app> <adr_…|username>" >&2; exit 2; }
    _compute_call POST "/cli/app/account-deletion/$action" "$(_compute_json K2_CJ_app="$app" K2_CJ_request="$who")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_AD_ACTION="$action" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
if os.environ["K2_AD_ACTION"] == "cancel":
    print("Cancelled %s (%s): their access to this App is back (%s grant(s) on). They sign in again." % (v.get("requestId"), v.get("username"), v.get("restored")))
    sys.exit(0)
print("Deleted %s (%s): %s." % (v.get("username"), "the login" if v.get("scope") == "account" else "only this App access", ", ".join(v.get("done") or [])))
for k in v.get("kept") or []:
    print("  kept %-14s %s" % (k["item"], k["why"]))
print("Now delete what the App keeps for them in its own data.")'
}

cmd_app_members() {
    local room="" app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) app="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_surfaces_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) room="$1"; shift ;;
        esac
    done
    if [ -z "$room" ] && ! _compute_in_session; then
        echo "usage: k2 app members <room> [--app <app-id>|host|all] [--json]" >&2
        exit 2
    fi
    local q=""
    [ -n "$room" ] && q="workspace=$(urlencode "$room")"
    [ -n "$app" ] && q="${q:+$q&}app=$(urlencode "$app")"
    _compute_call GET /cli/app/members "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
hidden = " (names hidden from Apps)" if v.get("namesHidden") else ""
print("%s: %d member(s)%s" % (v["workspace"], v["count"], hidden))
for m in v.get("members") or []:
    name = m.get("username") or "(hidden)"
    disp = m.get("displayName")
    label = name if not disp or disp == name else "%s (%s)" % (name, disp)
    print("  %-30s %-6s %-12s role %-12s %s" % (label, m["kind"], m["app"], m.get("role") or "-", ", ".join(m.get("caps") or [])))
print("  + %s" % v["admins"])'
}

cmd_app_room_names() {
    local room="" mode="" json_out=0 hide=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_surfaces_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$room" ]; then room="$1"; else mode="$1"; fi; shift ;;
        esac
    done
    case "$mode" in
        hide) hide=1 ;;
        show) hide=0 ;;
        *) echo "usage: k2 app room-names <room> hide|show   (the owner, outside K2)" >&2; exit 2 ;;
    esac
    [ -n "$room" ] || { echo "usage: k2 app room-names <room> hide|show   (the owner, outside K2)" >&2; exit 2; }
    if _compute_in_session; then
        echo '{"error":{"code":"owner_only","hint":"only the owner hides or shows guests names in a room: k2 app room-names <room> hide|show from a terminal outside K2"}}' >&2
        exit 3
    fi
    _compute_call POST /cli/app/room-names "$(_compute_json K2_CJ_workspace="$room" K2_CJ_hide_b="$hide")" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
state = "hidden from Apps" if v["namesHidden"] else "shown to Apps that hold room:members"
print("%s: guests names %s%s" % (v["workspace"], state, "" if v.get("changed") else " (already)"))'
}

# k2 app chat <app> [--json] (Sew 0.6.0, App-governed Spaces): read-only.
# The App's chat key, each Space bound to it (who governs it), its channels
# (from the manifest's chat block, or made by the App) and each person's
# private channels, level and last sync; box users leaving before the switch.
_app_chat_usage() {
    cat <<'EOF'
k2 app chat <app> [--json]
  What K2 keeps in step for an App's chat (Sew 0.6.0, App-governed Spaces). Read-only.
  - the chat key (k2 app grant <app> --kind chat-key) and its level ceiling;
  - each Space bound to the App and who governs it: sew (people run it in chat) or app
    (k2 hostchat app-space governance <app> <space> app|sew; the owner or an admin, in person);
  - its channels: the manifest's chat block (static channels; roles/rooms see the private ones;
    levels; agents) and channels the App made itself; each with Sew's id once made;
  - the steps K2 is taking toward Sew for each Space: the current one, Sew's last answer, and any
    blocked row (shown to a person and the App's own agent);
  - each App user's private channels and level, and their last sync: synced, pending (on its
    way; after their own chat account), queued (Sew runs a large change as a job), or held
    (banned in that Space, deactivated in chat, the Space at the App's member limit,
    an unknown channel: the reason says);
  - box users (the owner, admins) leaving the Space before it switches to app.
  The App decides who sees which channel: its manifest's chat block (change it and deploy), or,
  for channels it makes at run time, its own chat.channels / chat.setChannels calls. No k2 verb
  edits that; K2 syncs within seconds of any change (App users, roles, the manifest, the switch).
EOF
}

cmd_app_chat() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_chat_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app chat <app> [--json])" >&2; exit 2 ;;
            *) [ -z "$app" ] || { echo "usage: k2 app chat <app> [--json]" >&2; exit 2; }; app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app chat <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/sew/app-chat "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
c = v.get("chat") or {}
k = c.get("key")
print("%s (%s)" % (v.get("name"), c.get("appId")))
if not c.get("feature"):
    print("  this box'"'"'s Sew has no App-governed Spaces yet (it needs Sew 0.6.0: app_governed_spaces)")
if k:
    print("  chat key     %s, level ceiling %s, up to %s people" % ("on" if k.get("enabled") else "off", k.get("maxLevel"), k.get("maxMembers")))
else:
    print("  chat key     none (k2 app grant <app> --kind chat-key, the owner in person)")
print("  manifest     %s channel(s) in its chat block" % c.get("manifestChannels", 0))
spaces = c.get("spaces") or []
if not spaces:
    print("  no Space is bound to this App (k2 chat space create <slug> --name N --app <app>)")
for s in spaces:
    g = s.get("governance") or "sew"
    conf = s.get("governanceConfirmed")
    note = "" if (conf or "sew") == g else " (sending to Sew)"
    print("Space %s: governed by %s%s" % (s.get("spaceId"), g, note))
    for st in s.get("sync") or []:
        said = ""
        if st.get("lastStatus") is not None:
            said += " Sew answered %s" % st["lastStatus"]
        if st.get("lastError"):
            said += ": %s" % st["lastError"]
        print("  step: %s %s  %s, %s try(s).%s" % (st.get("op"), st.get("subject"), st.get("state"), st.get("attempts"), said))
        if st.get("state") == "blocked":
            print("    blocked: Sew refused this step, so K2 won'"'"'t try it again by itself. Fix what Sew said first (ask the owner if you can'"'"'t), then the owner retries it: k2 app grant hooks retry %s" % st.get("id"))
    for ch in s.get("channels") or []:
        why = (" (%s)" % ch["why"]) if ch.get("why") else ""
        print("  #%-24s %-5s %-7s %-8s %s%s" % (ch.get("slug"), ch.get("type"), "private" if ch.get("private") else "public", ch.get("state"), ch.get("channelId") or "-", why))
    w = s.get("waiting") or {}
    if w.get("channels"):
        print("  waiting: channels not made in Sew yet: %s" % ", ".join(w["channels"]))
    if w.get("agents"):
        print("  waiting: agents not in chat yet: %s" % ", ".join(w["agents"]))
    if w.get("refusedAgents"):
        print("  refused: these may not join the App'"'"'s channels (not its own workspace or a room on its roles): %s" % ", ".join(w["refusedAgents"]))
    for p in s.get("people") or []:
        why = (" (%s)" % p["why"]) if p.get("why") else ""
        who = p.get("username") or p.get("externalId")
        print("  %-24s %-9s %-8s %s%s" % (who, p.get("level"), p.get("state"), ", ".join(p.get("channels") or []) or "-", why))
    for l in s.get("leaving") or []:
        print("  leaving: box user %s (%s)" % (l.get("externalId"), "out" if l.get("state") == "gone" else "being removed"))'
}

cmd_app_self() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_surfaces_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app self <app-id> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/self "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
print("%s (workspace %s): %s" % (v["app"], v["workspace"], v["health"]))
print("  live Seed   %s" % (v.get("liveSeed") or "none yet"))
if v.get("deployment"):
    at = v.get("deployedAt")
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(at)) if at else "not live"
    print("  deployment  %s, live since %s" % (v["deployment"], when))'
}

cmd_app_usage() {
    local app="" hours="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --hours) hours="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_surfaces_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app usage <app-id> [--hours N] [--json]" >&2; exit 2; }
    case "$hours" in ''|*[!0-9]*) [ -z "$hours" ] || { echo "usage: --hours takes a whole number (1-720)" >&2; exit 2; } ;; esac
    local q="app=$(urlencode "$app")"
    [ -n "$hours" ] && q="${q}&hours=$hours"
    _compute_call GET /cli/app/usage "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
t, q, live = v["totals"], v["quotas"], v["live"]
print("%s, last %s h: %s calls, %s refused, %s B in, %s B out" % (v["app"], v["hours"], t["calls"], t["denied"], t["bytesIn"], t["bytesOut"]))
print("  quotas  %s calls/s (bursts of %s), %s sockets, %s subscriptions" % (q["rps"], q["burst"], q["sockets"], q["subscriptions"]))
print("  now     %s connections, %s subscriptions" % (live["connections"], live["subscriptions"]))
for x in v.get("verbs") or []:
    print("  %6s  %-24s %s%s" % (x["calls"], x.get("verb") or "-", x["route"], ("  (%s refused)" % x["denied"]) if x["denied"] else ""))'
}

# k2 app token rotate <app> — a remote App pass (AR10). Owner, outside K2.
cmd_app_token_rotate() {
    local app="" ttl="" keep=0 caps="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --ttl) ttl="${2:-}"; shift 2 ;;
            --keep-old) keep=1; shift ;;
            --caps) caps="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app token rotate <app-id> [--ttl 30d] [--keep-old] [--caps a,b]" >&2; exit 2; }
    if _compute_in_session; then
        echo '{"error":{"code":"owner_only","hint":"a remote App pass is a long-lived credential: the owner mints it from a terminal outside K2"}}' >&2
        exit 3
    fi
    local caps_j=""
    [ -n "$caps" ] && caps_j="$(K2_CAPS="$caps" python3 -c 'import json,os; print(json.dumps([c.strip() for c in os.environ["K2_CAPS"].split(",") if c.strip()]))')"
    _compute_call POST /cli/app/remote-pass/rotate "$(_compute_json K2_CJ_app="$app" K2_CJ_ttl="$ttl" \
        K2_CJ_keepOld_b="$([ "$keep" = 1 ] && echo 1)" K2_CJ_caps_j="$caps_j")" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
print(v["pass"])
sys.stderr.write("%s: remote pass %s, caps %s, rooms %s, expires %s; %s old revoked\n" % (
    v["app"], v.get("prefix"), ", ".join(v.get("caps") or []) or "none", ", ".join(v.get("rooms") or []),
    time.strftime("%Y-%m-%d %H:%M", time.localtime(v["expiresAt"])), v.get("revoked")))
if v.get("withheld"):
    sys.stderr.write("  withheld until the owner approves them: %s\n" % ", ".join(v["withheld"]))
sys.stderr.write("  %s\n" % v.get("note", ""))'
}

cmd_app_remote_pass() {
    local app="" revoke=0 id="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --revoke) revoke=1; shift; case "${1:-}" in ''|-*) ;; *) id="$1"; shift ;; esac ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_door_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app remote-pass <app-id> [--revoke [PASS-ID]] [--json]" >&2; exit 2; }
    if [ "$revoke" = 1 ]; then
        _compute_call POST /cli/app/remote-pass/revoke "$(_compute_json K2_CJ_app="$app" K2_CJ_id="$id")"
        if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s remote passes revoked" % (v["app"], v["revoked"]))'
        return 0
    fi
    _compute_call GET /cli/app/remote-pass "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
ps = v.get("passes") or []
if not ps:
    print("%s: no remote passes (k2 app token rotate %s, the owner)" % (v["app"], v["app"])); sys.exit(0)
now = time.time()
for p in ps:
    state = "revoked" if p.get("revokedAt") else ("expired" if p["expiresAt"] <= now else "live")
    print("%s  %-7s %s  expires %s  caps %s" % (p["id"][:8], state, p["prefix"], time.strftime("%Y-%m-%d %H:%M", time.localtime(p["expiresAt"])), ", ".join(p.get("caps") or []) or "none"))'
}

