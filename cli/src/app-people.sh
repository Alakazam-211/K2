# ── app-mobile-access-v1 (0.46.3): People grid, discovery, stay signed in ──

# k2 app people <app> [<user>] [--json]: who reaches which agents through
# one App, and what they may do there (the owner's person × agent grid).
cmd_app_people() {
    local app="" user="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) echo "usage: k2 app people <app> [<user>] [--json]   (app: service id, <workspace>/<name> or host)"; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else user="$1"; fi; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app people <app> [<user>] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$user" ] && q="$q&user=$(urlencode "$user")"
    _compute_call GET /cli/skin/access "$q" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("settings") or {}
stay = s.get("staySignedInDays")
print("App %s: at most %s devices a person; stay signed in %s; listed for sign-in %s" % (v.get("app"), s.get("maxDevices"), ("%s days" % stay) if stay else "off (7-day login)", "on" if s.get("listed") else "off"))
people = v.get("people") or []
if not people:
    print("  nobody has this App yet")
for p in people:
    who = p.get("fullName") or p.get("username")
    state = (p.get("role") or "rooms only (Thread)") if p.get("enabled") else "off"
    print("  %s (%s) · %s" % (who, p.get("username"), state))
    cells = p.get("cells") or []
    if not cells and p.get("enabled"):
        print("    no agents")
    for c in cells:
        f = c.get("folders") or {}
        folders = ", ".join(["read " + x for x in f.get("read") or []] + ["write " + x for x in f.get("write") or []])
        print("    %-20s %s%s" % (c.get("handle"), ", ".join(c.get("caps") or []), ("  [" + folders + "]") if folders else ""))
for d in v.get("devices") or []:
    print("  device %s: %s · %s%s" % (d.get("id"), d.get("username"), d.get("name"), " · paired by QR" if d.get("via") == "pairing" else ""))'
}

# k2 app discovery <app> on|off: list one App for sign-in discovery (only
# its name and how to sign in; off = the server answers as if nothing were
# there). The owner, or the App's own agent with "…manage them end to end".
cmd_app_discovery() {
    local app="${1:-}" mode="${2:-}"
    case "$mode" in
        on) mode=true ;;
        off) mode=false ;;
        *) echo "usage: k2 app discovery <app> on|off" >&2; exit 2 ;;
    esac
    [ -z "$app" ] && { echo "usage: k2 app discovery <app> on|off" >&2; exit 2; }
    local body
    body=$(_compute_json K2_CJ_app="$app" K2_CJ_listed_j="$mode")
    _compute_call POST /cli/skin/access/listed "$body"
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("settings") or {}
print("Listed for sign-in: %s%s" % ("on" if s.get("listed") else "off", "" if v.get("changed") else " (unchanged)"))'
}

# k2 app stay-signed-in <app> <days>: how long a phone that signs in with a
# device stays signed in without being used (0 = off: today's 7-day login).
cmd_app_stay() {
    local app="${1:-}" days="${2:-}"
    case "$days" in
        ''|*[!0-9]*) echo "usage: k2 app stay-signed-in <app> <days>   (0 = off, at most 90)" >&2; exit 2 ;;
    esac
    local body
    body=$(_compute_json K2_CJ_app="$app" K2_CJ_staySignedInDays_n="$days")
    _compute_call POST /cli/skin/access/settings "$body" tcp
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
s = json.load(sys.stdin).get("settings") or {}
d = s.get("staySignedInDays")
print("Stay signed in: %s" % (("%s days" % d) if d else "off (7-day login)"))'
}

# k2 app login settings <app> [flags] (also: k2 app invite-settings): how long
# an App's invite and reset links live, its own set-password page, and its
# own invite / reset subject and body (K2 always adds the link itself). The
# owner, or the App's own agent with "…manage them end to end".
cmd_app_invite_settings() {
    local app="" json_out=0 invite_ttl="" reset_ttl="" url="" isub="" ibody="" rsub="" rbody="" defaults="" max_devices=""
    _read_body_file() { [ -f "$1" ] || { echo "no such file: $1" >&2; exit 2; }; cat "$1"; }
    while [ $# -gt 0 ]; do
        case "$1" in
            --invite-ttl) invite_ttl="${2:-}"; shift 2 ;;
            --reset-ttl) reset_ttl="${2:-}"; shift 2 ;;
            --set-password-url) url="${2:-}"; shift 2 ;;
            --invite-subject) isub="${2:-}"; shift 2 ;;
            --invite-body-file) ibody="$(_read_body_file "${2:-}")"; shift 2 ;;
            --reset-subject) rsub="${2:-}"; shift 2 ;;
            --reset-body-file) rbody="$(_read_body_file "${2:-}")"; shift 2 ;;
            --max-devices) max_devices="${2:-}"; shift 2 ;;
            --default) defaults="$defaults ${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help)
                cat <<'EOF'
usage: k2 app login settings <app> [flags]        (also: k2 app invite-settings)
  --invite-ttl 72h          invite links live this long (15m to 30d; default 72h)
  --reset-ttl 24h           reset links live this long (15m to 30d; default 24h)
  --set-password-url <url>  the App's own set-password page (https, no #; K2 adds #k2inv_…)
  --invite-subject <text>   the App's invite subject ({app} {name} {username} {sender} {expires})
  --invite-body-file <f>    the App's invite body (plain text; K2 adds the link after it)
  --reset-subject <text>    the App's reset subject
  --reset-body-file <f>     the App's reset body
  --max-devices <n>         devices one person may keep signed in to this App (1 to 20; default 20).
                            Past it, a new device's sign-in ends their least recently used ones
                            until it fits. Lowering it signs nobody out until their next new device.
                            (Sign-ins per person. k2 app push set --max-devices is another number:
                            phones that can get this App's notifications, everyone together.)
  --default <field>         back to K2's: inviteTtl resetTtl setPasswordUrl inviteSubject inviteBody resetSubject resetBody maxDevices
No flags: show the settings. The owner, or the App's own agent with the switch
"…and manage them end to end" (k2 app access).
EOF
                return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app login settings <app> [flags]   (k2 app login settings --help)" >&2; exit 2; }
    case "$max_devices" in
        ""|[0-9]|[0-9][0-9]) ;;
        *) echo "k2 app login settings: --max-devices is a whole number from 1 to 20 (not '$max_devices')" >&2; exit 2 ;;
    esac
    local body
    body=$(K2_IS_MD="$max_devices" K2_IS_APP="$app" K2_IS_IT="$invite_ttl" K2_IS_RT="$reset_ttl" K2_IS_URL="$url" K2_IS_ISUB="$isub" K2_IS_IBODY="$ibody" K2_IS_RSUB="$rsub" K2_IS_RBODY="$rbody" K2_IS_DEF="$defaults" python3 -c '
import json, os
b = {"app": os.environ["K2_IS_APP"]}
for env, key in [("K2_IS_IT","inviteTtl"),("K2_IS_RT","resetTtl"),("K2_IS_URL","setPasswordUrl"),("K2_IS_ISUB","inviteSubject"),("K2_IS_IBODY","inviteBody"),("K2_IS_RSUB","resetSubject"),("K2_IS_RBODY","resetBody")]:
    v = os.environ.get(env, "")
    if v:
        b[key] = v
if os.environ.get("K2_IS_MD", ""):
    b["maxDevices"] = int(os.environ["K2_IS_MD"])
for f in os.environ.get("K2_IS_DEF", "").split():
    b[f] = None
print(json.dumps(b))')
    _compute_call POST /cli/skin/access/invites "$body"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("settings") or {}
def life(n, own):
    n = int(n or 0)
    t = ("%d days" % (n // 86400)) if n % 86400 == 0 and n >= 4 * 86400 else ("%d hours" % (n // 3600)) if n % 3600 == 0 else ("%d minutes" % (n // 60))
    return t + ("" if own else " (default)")
print("App %s" % v.get("app"))
print("  invite links live %s" % life(s.get("inviteTtlSecs"), s.get("inviteTtlSet")))
print("  reset links live  %s" % life(s.get("resetTtlSecs"), s.get("resetTtlSet")))
print("  set-password page %s" % (s.get("setPasswordUrl") or "the box front door /reset (default)"))
for k, label in [("invite", "invite"), ("reset", "reset")]:
    t = s.get(k) or {}
    print("  %s mail: %s" % (label, "the App template" if (t.get("subject") or t.get("body")) else "K2 template"))
print("  devices per person: %s%s" % (v.get("maxDevices"), " (default)" if v.get("maxDevices") == 20 else ""))'
}

# k2 app profile settings <app> [--names on|off] [--image on|off] (0.46.5,
# app-user-profile): may the App's people change their own names and
# picture. The owner or an Admin; the App's own agent with "manage its Apps'
# users" may only turn them off (never on, never the box login).
cmd_app_profile_settings() {
    local app="" names="" image="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --names) names="${2:-}"; shift 2 ;;
            --image|--picture) image="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help)
                cat <<'EOF'
usage: k2 app profile settings <app> [--names on|off] [--image on|off] [--json]
  --names on|off   people may change their own full and display names (default on)
  --image on|off   people may set and clear their own picture (default on)
No flags: show the settings. The owner or an Admin; the App's own agent with
"manage its Apps' users" may only turn a setting off.
EOF
                return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -z "$app" ] && { echo "usage: k2 app profile settings <app> [--names on|off] [--image on|off]   (k2 app profile settings --help)" >&2; exit 2; }
    for v in "$names" "$image"; do
        case "$v" in ""|on|off) ;; *) echo "usage: --names and --image take on or off" >&2; exit 2 ;; esac
    done
    if [ -z "$names" ] && [ -z "$image" ]; then
        _compute_call GET /cli/app/profile-settings "app=$(urlencode "$app")"
    else
        local nb="" ib=""
        [ -n "$names" ] && nb=$([ "$names" = on ] && echo 1 || echo 0)
        [ -n "$image" ] && ib=$([ "$image" = on ] && echo 1 || echo 0)
        _compute_call POST /cli/app/profile-settings "$(_compute_json K2_CJ_app="$app" K2_CJ_editNames_b="$nb" K2_CJ_editImage_b="$ib")"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
s = json.load(sys.stdin).get("settings") or {}
print("App %s: people may change their names %s, their picture %s" % (s.get("app"), "on" if s.get("editNames") else "off", "on" if s.get("editImage") else "off"))'
}

# k2 app set <app> [--account-deletion request|immediate] [--deletion-grace <days>]
#                  [--client-logs-days <days>] [--json]   (0.46.9)
_app_set_usage() {
    cat <<'EOF'
usage: k2 app set <app> [--account-deletion request|immediate] [--deletion-grace <days>]
                        [--client-logs-days <days>] [--json]
  --account-deletion request    (default) a person's "Delete my account" signs them out of
                                the App everywhere and turns their access off at once; the
                                deletion completes when you run k2 app account-deletion
                                complete, or by itself when the grace period ends
  --account-deletion immediate  the deletion runs at once
  --deletion-grace <days>       how long a request waits before K2 completes it
                                (1 to 90 days, default 30; 30d also works)
  --client-logs-days <days>     how long K2 keeps the App's client log bundles
                                (1 to 90 days, default 14; k2 app logs <app> --client)
No flags: show the settings. The owner or an Admin changes them; so may the App's
own agent with "Let this agent manage its Apps' users" on. Every change is a line in
k2 app history <app> --users. More: k2 study app-account-deletion.
EOF
}

cmd_app_set() {
    local app="" mode="" grace="" logs="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --account-deletion) mode="${2:-}"; shift 2 ;;
            --account-deletion=*) mode="${1#*=}"; shift ;;
            --deletion-grace) grace="${2:-}"; shift 2 ;;
            --deletion-grace=*) grace="${1#*=}"; shift ;;
            --client-logs-days) logs="${2:-}"; shift 2 ;;
            --client-logs-days=*) logs="${1#*=}"; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_set_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app set --help)" >&2; exit 2 ;;
            *) [ -z "$app" ] && app="$1" || { echo "usage: one App only (k2 app set --help)" >&2; exit 2; }; shift ;;
        esac
    done
    [ -z "$app" ] && { _app_set_usage >&2; exit 2; }
    case "$mode" in ""|request|immediate) ;; *) echo "usage: --account-deletion takes request or immediate" >&2; exit 2 ;; esac
    grace="${grace%d}"; logs="${logs%d}"
    for v in "$grace" "$logs"; do
        [[ -z "$v" || "$v" =~ ^[0-9]+$ ]] || { echo "usage: --deletion-grace and --client-logs-days take a number of days (1 to 90)" >&2; exit 2; }
    done
    if [ -z "$mode" ] && [ -z "$grace" ] && [ -z "$logs" ]; then
        _compute_call GET /cli/app/settings "app=$(urlencode "$app")"
    else
        _compute_call POST /cli/app/settings "$(_compute_json K2_CJ_app="$app" K2_CJ_accountDeletion="$mode" K2_CJ_deletionGraceDays_n="$grace" K2_CJ_clientLogsDays_n="$logs")"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
d = v.get("accountDeletion") or {}
c = v.get("clientLogs") or {}
print("App %s" % v.get("app"))
print("  account deletion   %s, grace %s days%s" % (d.get("mode"), d.get("graceDays"), "" if d.get("set") else " (the defaults)"))
print("  client logs kept   %s days%s" % (c.get("keepDays"), "" if c.get("set") else " (the default)"))'
}

