# ── k2 app auth (0.46.9, prd-app-hosted-sign-in-v1) ─────────────────
_app_auth_usage() {
    cat <<'EOF'
k2 app auth — the sign-in door: your App's own sign-in page posts the
password straight to K2 at /.k2/auth/* on the App's own host, never to your
server. K2 answers that path before your App sees it and gives back a
one-time code; your server swaps the code for the person's pass
(k2.auth.exchange). How to build the page: k2 study auth-pages.

  k2 app auth [<app>] [--json]          Is the door on for this server? With
                                        <app>: that App's door, its extra page
                                        origins and its hosts.
  k2 app auth door on|off [<app>] [--json]
                                        No <app>: turn the door on or off for
                                        every App on this server (the owner;
                                        it starts off). With <app>: that App
                                        only. off hands /.k2/auth/* back to the
                                        App (one that serves the path itself);
                                        on undoes that. An App has a door only
                                        while the server's switch is on too.
  k2 app auth origins <app> [--origin O]… [--clear] [--json]
                                        The page origins, beyond the App's own
                                        host, whose posts the door takes and
                                        answers with CORS: a bundled Tauri
                                        page (tauri://localhost,
                                        http://tauri.localhost). Each call
                                        replaces the whole list; --clear
                                        empties it. The webview origins the
                                        App's phone pairing lists count too.

Your App's own server reads the same answer for itself on its link, with
no agent: k2.auth.doorStatus() (GET /cli/skin/auth/door with the App pass):
{on, box, appOff, hosts: [{host, door}]}, its own App only.

Who: door on|off for the server is the owner's. One App's door and origins:
the owner, an Admin, or the App's own agent with "Build and run Apps"
(Settings → Apps). Every change is recorded. Exit codes: 0 ok, 2 bad usage,
3 refused (JSON with a code on stderr).
EOF
}

cmd_app_auth() {
    local sub="${1:-}"
    case "$sub" in
        -h|--help|help) _app_auth_usage; return 0 ;;
        door|origins) shift ;;
        ""|--json) sub=status ;;
        -*) echo "Unknown flag: $sub (k2 app auth --help)" >&2; exit 2 ;;
        *) sub=status ;;
    esac
    local app="" json_out=0 onoff="" origins="" clear=0
    local nl='
'
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            --origin) [ "$sub" = origins ] || { echo "--origin goes with k2 app auth origins" >&2; exit 2; }; origins="$origins${2:-}$nl"; shift 2 ;;
            --clear) [ "$sub" = origins ] || { echo "--clear goes with k2 app auth origins" >&2; exit 2; }; clear=1; shift ;;
            -h|--help) _app_auth_usage; return 0 ;;
            -*) echo "Unknown flag: $1 (k2 app auth --help)" >&2; exit 2 ;;
            on|off)
                if [ "$sub" = door ] && [ -z "$onoff" ]; then onoff="$1"; shift
                elif [ -z "$app" ]; then app="$1"; shift
                else echo "Unexpected argument: $1 (k2 app auth --help)" >&2; exit 2; fi ;;
            *) if [ -z "$app" ]; then app="$1"; shift; else echo "Unexpected argument: $1 (k2 app auth --help)" >&2; exit 2; fi ;;
        esac
    done
    case "$sub" in
        status)
            if [ -n "$app" ]; then _compute_call GET /cli/app/auth "app=$(urlencode "$app")"; else _compute_call GET /cli/app/auth ""; fi
            ;;
        door)
            [ -n "$onoff" ] || { echo "usage: k2 app auth door on|off [<app>] (k2 app auth --help)" >&2; exit 2; }
            _app_rt_post /cli/app/auth/set "$(_compute_json K2_CJ_door="$onoff" K2_CJ_app="$app")"
            [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            ;;
        origins)
            [ -n "$app" ] || { echo "usage: k2 app auth origins <app> [--origin O]… [--clear] (k2 app auth --help)" >&2; exit 2; }
            if [ -z "$origins" ] && [ "$clear" = 0 ]; then
                _compute_call GET /cli/app/auth "app=$(urlencode "$app")"
            else
                [ -n "$origins" ] && [ "$clear" = 1 ] && { echo "--origin and --clear don't go together (k2 app auth --help)" >&2; exit 2; }
                local body
                body="$(K2_AA_APP="$app" K2_AA_ORIGINS="$origins" python3 -c '
import json, os
e = os.environ
print(json.dumps({"app": e["K2_AA_APP"], "origins": [x.strip() for x in e.get("K2_AA_ORIGINS", "").split("\n") if x.strip()]}))')" || exit 2
                _app_rt_post /cli/app/auth/set "$body"
                [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            fi
            ;;
    esac
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
b = v.get("box") or {}
line = "sign-in door on this server: " + ("on" if b.get("on") else "off")
if b.get("envOverride"):
    line += " (K2_APP_AUTH_DOOR=%s on the daemon wins over the stored switch)" % b.get("envOverride")
elif not b.get("on"):
    line += " (the owner turns it on: k2 app auth door on)"
print(line)
a = v.get("app")
if a:
    print("%s: door %s%s" % (a.get("name") or a.get("app"), a.get("door"), " (turned off for this App)" if a.get("appOff") else ""))
    o = a.get("origins") or []
    print("  extra page origins:    " + (", ".join(o) if o else "-"))
    po = a.get("pairingOrigins") or []
    if po:
        print("  from phone pairing:    " + ", ".join(po))
    print("  hosts:                 " + (", ".join(a.get("hosts") or []) or "- (no public host yet)"))
    if a.get("labelNote"):
        print("  note: " + a["labelNote"])
    if a.get("notRuntime"):
        print("  note: not a runtime App: K2 answers /.k2/auth/* only in front of a runtime App (k2 app deploy)")
for w in v.get("warnings") or []:
    print("note: " + w)'
}

_app_pairing_usage() {
    cat <<'EOF'
k2 app pairing — let people pair a custom App's own phone app by QR from
the App's web page (no password typed on the phone). K2 serves the App's
iOS and Android link files on its own k2.dev labels while pairing is on.

  k2 app pairing status <app> [--json]
                                       On or off, the phone apps, the hosts K2
                                       pairs on, paired phones, the last check.
  k2 app pairing setup <app> [--ios TEAMID.com.you.app]…
                 [--android com.you.app:AB:CD:…[,AB:CD:…]]…
                 [--host <app label host>]… [--native-origin tauri://localhost]…
                 [--link-path '/jobs/*']… [--app-store URL] [--play-store URL]
                                       Name the phone apps that open the App's
                                       sign-in codes and turn pairing on. The
                                       owner, in person: Touch ID, your login
                                       or the vault password confirms it (from
                                       a terminal outside K2). Every setup
                                       replaces the last one.
  k2 app pairing setup <app> --desktop-only [--native-origin tauri://localhost]…
                 [--host <app label host>]…
                                       An App with no phone app: turn pairing
                                       on so its desktop app pairs with the
                                       code the web App shows, typed in (K2
                                       serves no link files; nothing opens the
                                       QR). Same owner, same confirm, the same
                                       checks. Not with --ios, --android or
                                       --link-path. A Mac app that opens
                                       universal links (Associated Domains)
                                       can go under --ios TEAMID.bundle.id
                                       instead, like an iPhone app.
  k2 app pairing off <app>             Turn it off (the owner, an Admin or the
                                       App's agent). Codes stop at once; phones
                                       already paired stay signed in (end them
                                       in k2 app people).
  k2 app pairing check <app> [--json]  Fetch the link files from each host over
                                       the internet (and Apple's CDN copy) and
                                       compare them with what K2 serves.
  k2 app pairing log <app> [--json]    The App's paired phones: when, who, the
                                       device's name, the host, live or ended.

v1 pairs on the App's own k2.dev labels. A custom domain works only when you
front it to the label yourself. In your app, list each host exactly
(applinks:jobs.you.k2.dev), never a wildcard. While pairing is on, K2 answers
/.well-known/apple-app-site-association and /.well-known/assetlinks.json on
those hosts: list the App's other universal-link paths with --link-path.

Who: status and off: an Admin or the App's own agent ("Build and run Apps");
log: the App's agent only when it may manage the App's users; setup and
check: the owner in person (an agent asks with a permission card).

The web App calls pairing.start / pairing.status; the phone (or desktop) app
calls pairing.check, then pairing.redeem: k2 study app-pairing for the whole
guide. K2 Companion's own phones are k2 users pair-phone (k2 users --help).
EOF
}

# Print one pairing status (stdin: the status JSON).
_app_pairing_status_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
state = v.get("state")
line = "%s: phone pairing %s" % (v.get("name") or v.get("app"), state)
if state != "on":
    why = {"owner_off": "turned off", "not_set_up": "not set up yet (k2 app pairing setup)", "box_off": "K2_APP_PAIRING=off on this server",
           "no_app": "no such App"}.get(v.get("off"), v.get("off") or "")
    if not v.get("served") and v.get("notServed"):
        why = (why + "; " if why else "") + v["notServed"]
    line += " (" + why + ")"
print(line)
if v.get("desktopOnly"):
    print("  apps:     desktop only (no phone app; people type the code into your desktop app)")
for i in v.get("ios") or []:
    print("  iOS:      " + i)
for a in v.get("android") or []:
    print("  Android:  %s (%d fingerprint%s)" % (a.get("package"), len(a.get("sha256") or []), "" if len(a.get("sha256") or []) == 1 else "s"))
for h in v.get("hosts") or []:
    print("  host:     %s%s" % (h.get("host"), "" if h.get("pairs") else " (not pairing)"))
if not (v.get("hosts") or []):
    print("  host:     none (the App needs K2 Connect and its own label)")
for o in v.get("nativeOrigins") or []:
    print("  webview:  " + o)
for p in v.get("linkPaths") or []:
    print("  link:     " + p)
st = v.get("store") or {}
for k, n in (("ios", "App Store"), ("android", "Google Play")):
    if st.get(k):
        print("  %-9s %s" % (n + ":", st[k]))
print("  phones:   %s paired now, %s in the last day; stay signed in %s" % (v.get("pairedPhones", 0), v.get("pairingsLastDay", 0),
      ("%s days" % v["staySignedInDays"]) if v.get("staySignedInDays") else "off (no pairing)"))
lc = v.get("lastCheck")
if lc:
    for h in lc.get("hosts") or []:
        print("  checked:  %s aasa %s, assetlinks %s, Apple CDN %s" % (h.get("host"), h.get("aasa"), h.get("assetlinks"), h.get("appleCdn")))
for w in v.get("warnings") or []:
    print("  note: " + w)'
}

cmd_app_pairing() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _app_pairing_usage; return 0 ;;
    esac
    shift
    case "$sub" in
        status) cmd_app_pairing_status "$@" ;;
        setup|set) cmd_app_pairing_setup "$@" ;;
        off) cmd_app_pairing_off "$@" ;;
        check) cmd_app_pairing_check "$@" ;;
        log) cmd_app_pairing_log "$@" ;;
        *) echo "usage: k2 app pairing status|setup|off|check|log (k2 app pairing --help)" >&2; exit 2 ;;
    esac
}

cmd_app_pairing_status() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_pairing_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app pairing status <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/pairing "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_pairing_status_print
}

cmd_app_pairing_setup() {
    local app="" json_out=0 ios="" android="" hosts="" origins="" paths="" appstore="" playstore="" desktop=0
    local nl='
'
    while [ $# -gt 0 ]; do
        case "$1" in
            --desktop-only) desktop=1; shift ;;
            --ios) ios="$ios${2:-}$nl"; shift 2 ;;
            --android) android="$android${2:-}$nl"; shift 2 ;;
            --host) hosts="$hosts${2:-}$nl"; shift 2 ;;
            --native-origin) origins="$origins${2:-}$nl"; shift 2 ;;
            --link-path) paths="$paths${2:-}$nl"; shift 2 ;;
            --app-store) appstore="${2:-}"; shift 2 ;;
            --play-store) playstore="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_pairing_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local usage="usage: k2 app pairing setup <app> ([--ios TEAMID.com.you.app]… [--android com.you.app:AB:CD:…]… | --desktop-only) [--host h]… [--native-origin o]… [--link-path p]… [--app-store URL] [--play-store URL]"
    [ -n "$app" ] || { echo "$usage" >&2; exit 2; }
    if [ "$desktop" = 1 ]; then
        { [ -n "$ios" ] || [ -n "$android" ]; } && { echo "$usage" >&2; echo "--desktop-only names no phone app: leave out --ios and --android" >&2; exit 2; }
    else
        { [ -z "$ios" ] && [ -z "$android" ]; } && { echo "$usage" >&2; echo "name at least one phone app (--ios and/or --android), or --desktop-only for an App whose only native app is a desktop app" >&2; exit 2; }
    fi
    local body
    body="$(K2_AP_APP="$app" K2_AP_IOS="$ios" K2_AP_ANDROID="$android" K2_AP_HOSTS="$hosts" K2_AP_ORIGINS="$origins" K2_AP_PATHS="$paths" \
        K2_AP_APPSTORE="$appstore" K2_AP_PLAYSTORE="$playstore" K2_AP_DESKTOP="$desktop" python3 -c '
import json, os, sys
e = os.environ
lines = lambda k: [x.strip() for x in e.get(k, "").split("\n") if x.strip()]
b = {"app": e["K2_AP_APP"]}
if e.get("K2_AP_DESKTOP") == "1":
    b["desktopOnly"] = True
if lines("K2_AP_IOS"):
    b["ios"] = lines("K2_AP_IOS")
android = []
for spec in lines("K2_AP_ANDROID"):
    if ":" not in spec:
        sys.stderr.write("--android is <package>:<SHA-256 fingerprint>[,<fingerprint>…]\n")
        sys.exit(2)
    pkg, fprs = spec.split(":", 1)
    android.append({"package": pkg, "sha256": [f.strip() for f in fprs.split(",") if f.strip()]})
if android:
    b["android"] = android
if lines("K2_AP_HOSTS"):
    b["hosts"] = lines("K2_AP_HOSTS")
if lines("K2_AP_ORIGINS"):
    b["nativeOrigins"] = lines("K2_AP_ORIGINS")
if lines("K2_AP_PATHS"):
    b["linkPaths"] = lines("K2_AP_PATHS")
store = {}
if e.get("K2_AP_APPSTORE"):
    store["ios"] = e["K2_AP_APPSTORE"]
if e.get("K2_AP_PLAYSTORE"):
    store["android"] = e["K2_AP_PLAYSTORE"]
if store:
    b["store"] = store
print(json.dumps(b))')" || exit 2
    _app_rt_post /cli/app/pairing/set "$body"
    if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session; then
        local svc tok
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        tok="$(_vault_confirm_token app-pairing-apps "$svc" "")" || exit 3
        body="$(printf '%s' "$body" | K2_AP_TOK="$tok" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_AP_TOK"]; print(json.dumps(b))')"
        tok=""
        _app_rt_post /cli/app/pairing/set "$body"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_pairing_status_print
    echo "Next: k2 app pairing check $app (Apple's CDN can take up to a day to fetch a new host)"
}

cmd_app_pairing_off() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app pairing off <app>" >&2; exit 2; }
    _compute_call POST /cli/app/pairing/off "$(_compute_json K2_CJ_app="$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_pairing_status_print
}

cmd_app_pairing_check() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app pairing check <app> [--json]" >&2; exit 2; }
    _compute_call POST /cli/app/pairing/check "$(_compute_json K2_CJ_app="$app")" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
bad = 0
words = {"ok": "ok", "none": "not used", "stale": "another file is served there (DNS or a proxy elsewhere, or a cached copy)",
         "not_served": "not served", "tls_error": "TLS error"}
for h in v.get("hosts") or []:
    print(h.get("host"))
    for k, n in (("aasa", "iOS file"), ("assetlinks", "Android file"), ("appleCdn", "Apple CDN copy")):
        s = h.get(k)
        if s not in ("ok", "none") and k != "appleCdn":
            bad = 1
        print("  %-15s %s" % (n + ":", words.get(s, s)))
sys.exit(1 if bad else 0)'
}

cmd_app_pairing_log() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app pairing log <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/pairing/log "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
v = json.load(sys.stdin)
rows = v.get("pairings") or []
if not rows:
    print("%s: no phone has paired yet" % (v.get("name") or v.get("app")))
for r in rows:
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(r.get("pairedAt") or 0))
    state = "live" if r.get("live") else ("ended: " + (r.get("ended") or "expired"))
    print("%s  %-16s %-24s %s  %s" % (when, r.get("username"), (r.get("deviceName") or "")[:24], r.get("host"), state))'
}

# k2 app push … (prd-custom-app-push-v1, 0.46.4): push notifications for a
# custom App's own phone app. The daemon sends to Apple and Google with the
# App owner's own keys, held in the vault. Keys are read from files here and
# sent over the local socket; a path never leaves this CLI, and nothing
# prints a key.
_app_push_usage() {
    cat <<'EOF'
k2 app push — notifications to a custom App's own phone app (its bundle id,
its Apple team, its Firebase project). K2 keeps the App owner's Apple and
Google keys in the vault and sends to Apple and Google itself. The App, its
agent and other Apps can use the keys, never read them.

  k2 app push status <app> [--json]   On or off, the keys' ids and fingerprints
                                       (never a key), device counts.
  k2 app push setup <app> [--apns-key AuthKey_XXXXXXXXXX.p8 --key-id XXXXXXXXXX
                 --team-id YYYYYYYYYY --bundle-id com.you.app]
                 [--fcm-service-account sa.json]
                                       Store the App's Apple key and/or Firebase
                                       service account. The owner, in person:
                                       Touch ID, your login or the vault password
                                       confirms it (from a terminal outside K2).
                                       K2 checks each key before storing it.
  k2 app push creds rm <app> --apns|--fcm|--all
                                       Remove a key (the same confirm).
  k2 app push check <app>              One real call per key to a fake token:
                                       says whether Apple and Google accept it.
  k2 app push on|off <app>             On is the owner's approval (also given by
                                       approving push:send in a deploy); off is
                                       the owner's, an Admin's or the App agent's.
  k2 app push set <app> [--agent-replies on|off] [--previews on|off]
                 [--max-devices N] [--register server|any]
                                       Agent-reply pushes (off by default) and
                                       their previews; --max-devices: phones
                                       that can get this App's notifications,
                                       everyone together (owner; not sign-ins:
                                       k2 app login settings --max-devices).
                                       --register server: only your App's own
                                       server may call push.register and
                                       push.viewing, never a phone calling K2
                                       directly (push_server_only). Pairing
                                       still signs the phone in, its answer says
                                       push {registered: false}, and your server
                                       registers the token. Default any.
  k2 app push send <app> (--to <user> [--to <user>…] | --room <room>)
                 --title T --body B [--data key=value]… [--topic t]
                 [--collapse-id c] [--thread-id t] [--sound s] [--badge n]
                 [--priority high|normal] [--ttl secs] [--android-channel c]
                 [--silent] [--json]   Send to this App's own users only.
  k2 app push test <app> --to <user>  "Test notification from K2".
  k2 app push log <app> [--json]       The last hour's sends: counts only.
  k2 app push devices <app> [--guest <user>] [--json]
                                       Each registered phone: platform,
                                       environment, last seen. Never a token.
  k2 app push devices rm <app> <apd_…> Remove one.

Who: status and log: an Admin or the App's own agent. send, test and
agent replies: an Admin or the App's agent with "…and manage them end to end".
Keys: the owner in person only; an agent asks with a permission card.

The phone app registers with the guest's own pass (push.register); see
k2 study app-push for the whole mobile guide. K2 Companion's own phones are
`k2 push` (k2 push --help).
EOF
}

# Print one push status (stdin: the status JSON).
_app_push_status_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
state = v.get("state")
line = "%s: notifications %s" % (v.get("name") or v.get("app"), state)
if state != "on":
    why = {"owner_off": "the owner turned them off", "not_approved": "not approved yet (k2 app push on, or approve push:send)",
           "box_off": "K2_APP_PUSH=off on this server"}.get(v.get("off"), v.get("off") or "")
    line += " (" + why + ")"
print(line)
a = v.get("apns")
print("  Apple:    " + ("key %s, team %s, bundle %s, fingerprint %s" % (a["keyId"], a["teamId"], a["bundleId"], a["keyFingerprint"]) if a else "no key"))
f = v.get("fcm")
print("  Firebase: " + ("project %s, %s, fingerprint %s" % (f["projectId"], f["clientEmail"], f["keyFingerprint"]) if f else "no service account"))
if v.get("credError"):
    print("  last key refusal: %s" % v["credError"])
d = v.get("devices") or {}
print("  devices:  %s for %s people (apns %s, fcm %s; sandbox %s)" % (d.get("total", 0), d.get("guests", 0), d.get("apns", 0), d.get("fcm", 0), d.get("sandbox", 0)))
if v.get("serverOnly"):
    print("  devices register only through this App server (k2 app push set <app> --register any to change)")
print("  agent replies %s, previews %s, at most %s devices" % ("on" if v.get("agentReplies") else "off", "on" if v.get("previews") else "off", v.get("maxDevices")))
for s in v.get("set") or []:
    print("  set: " + s)
for r in v.get("removed") or []:
    print("  removed: " + r)'
}

cmd_app_push() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _app_push_usage; return 0 ;;
    esac
    shift
    case "$sub" in
        status) cmd_app_push_status "$@" ;;
        setup) cmd_app_push_setup "$@" ;;
        creds|credentials) cmd_app_push_creds "$@" ;;
        check) cmd_app_push_check "$@" ;;
        on|off) cmd_app_push_onoff "$sub" "$@" ;;
        set) cmd_app_push_set "$@" ;;
        send) cmd_app_push_send send "$@" ;;
        test) cmd_app_push_send test "$@" ;;
        log) cmd_app_push_log "$@" ;;
        devices) cmd_app_push_devices "$@" ;;
        *) echo "usage: k2 app push status|setup|creds|check|on|off|set|send|test|log|devices (k2 app push --help)" >&2; exit 2 ;;
    esac
}

cmd_app_push_status() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_push_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app push status <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/push "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_push_status_print
}

# POST a keys body; on vault_confirm_required (the owner, outside K2)
# confirm in person, bound to the App, and send it again.
_app_push_keys_post() {
    local path="$1" body="$2" app="$3" label="$4"
    _app_rt_post "$path" "$body"
    if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session; then
        local svc tok
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        tok="$(_vault_confirm_token app-push-keys "$svc" "$label")" || exit 3
        body="$(printf '%s' "$body" | K2_AP_TOK="$tok" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_AP_TOK"]; print(json.dumps(b))')"
        tok=""
        _app_rt_post "$path" "$body"
    fi
    body=""
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

# POST /cli/app/push/set; turning on (or raising the device ceiling) asks
# the owner, outside K2, for a fresh confirm bound to the App.
_app_push_on_post() {
    local body="$1"
    _app_rt_post /cli/app/push/set "$body"
    if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session; then
        local svc tok
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
        tok="$(_vault_confirm_token app-push-on "$svc" "")" || exit 3
        body="$(printf '%s' "$body" | K2_AP_TOK="$tok" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_AP_TOK"]; print(json.dumps(b))')"
        tok=""
        _app_rt_post /cli/app/push/set "$body"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

cmd_app_push_setup() {
    local app="" p8="" kid="" team="" bundle="" sa="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --apns-key) p8="${2:-}"; shift 2 ;;
            --key-id) kid="${2:-}"; shift 2 ;;
            --team-id) team="${2:-}"; shift 2 ;;
            --bundle-id) bundle="${2:-}"; shift 2 ;;
            --fcm-service-account|--fcm) sa="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_push_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local usage="usage: k2 app push setup <app> [--apns-key FILE.p8 --key-id ID --team-id ID --bundle-id com.you.app] [--fcm-service-account sa.json]"
    [ -n "$app" ] || { echo "$usage" >&2; exit 2; }
    { [ -z "$p8" ] && [ -z "$sa" ]; } && { echo "$usage" >&2; exit 2; }
    if [ -n "$p8" ] && { [ -z "$kid" ] || [ -z "$team" ] || [ -z "$bundle" ]; }; then
        echo "--apns-key needs --key-id, --team-id and --bundle-id" >&2; exit 2
    fi
    [ -z "$p8" ] || [ -r "$p8" ] || { echo "can't read $p8" >&2; exit 2; }
    [ -z "$sa" ] || [ -r "$sa" ] || { echo "can't read $sa" >&2; exit 2; }
    local body
    body="$(K2_AP_APP="$app" K2_AP_P8="$p8" K2_AP_KID="$kid" K2_AP_TEAM="$team" K2_AP_BUNDLE="$bundle" K2_AP_SA="$sa" python3 -c '
import json, os
e = os.environ
b = {"app": e["K2_AP_APP"]}
if e.get("K2_AP_P8"):
    with open(e["K2_AP_P8"], encoding="utf-8") as f:
        b["apns"] = {"p8": f.read(), "keyId": e["K2_AP_KID"], "teamId": e["K2_AP_TEAM"], "bundleId": e["K2_AP_BUNDLE"]}
if e.get("K2_AP_SA"):
    with open(e["K2_AP_SA"], encoding="utf-8") as f:
        b["fcm"] = {"serviceAccountJson": f.read()}
print(json.dumps(b))')" || { echo "couldn't read the key file(s)" >&2; exit 2; }
    _app_push_keys_post /cli/app/push/credentials "$body" "$app" "Push keys for $app"
    body=""
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_push_status_print
    echo "Next: k2 app push check $app"
}

cmd_app_push_creds() {
    local verb="${1:-}"
    case "$verb" in
        rm|remove) shift ;;
        -h|--help|"") _app_push_usage; return 0 ;;
        *) echo "usage: k2 app push creds rm <app> --apns|--fcm|--all" >&2; exit 2 ;;
    esac
    local app="" platform="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --apns) platform=apns; shift ;;
            --fcm) platform=fcm; shift ;;
            --all) platform=all; shift ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$platform" ]; } && { echo "usage: k2 app push creds rm <app> --apns|--fcm|--all" >&2; exit 2; }
    _app_push_keys_post /cli/app/push/credentials/remove "$(_compute_json K2_CJ_app="$app" K2_CJ_platform="$platform")" "$app" "Remove push keys for $app"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_push_status_print
}

cmd_app_push_check() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app push check <app> [--json]" >&2; exit 2; }
    _compute_call POST /cli/app/push/check "$(_compute_json K2_CJ_app="$app")" tcp
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
bad = 0
for p, name in (("apns", "Apple"), ("fcm", "Firebase")):
    r = (v.get("results") or {}).get(p) or {}
    if not r.get("configured"):
        print("%-9s no key" % name)
    elif r.get("ok"):
        print("%-9s key works" % name)
    else:
        bad = 1
        print("%-9s refused: %s" % (name, r.get("code")))
    if r.get("limits"):
        print("          " + r["limits"])
sys.exit(1 if bad else 0)'
}

cmd_app_push_onoff() {
    local mode="$1"; shift
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app push on|off <app>" >&2; exit 2; }
    local on=0
    [ "$mode" = on ] && on=1
    _app_push_on_post "$(_compute_json K2_CJ_app="$app" K2_CJ_enabled_b="$on")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_push_status_print
}

cmd_app_push_set() {
    local app="" replies="" previews="" maxd="" register="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --agent-replies) replies="${2:-}"; shift 2 ;;
            --previews) previews="${2:-}"; shift 2 ;;
            --max-devices) maxd="${2:-}"; shift 2 ;;
            --register) register="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local usage="usage: k2 app push set <app> [--agent-replies on|off] [--previews on|off] [--max-devices N] [--register server|any]"
    [ -n "$app" ] || { echo "$usage" >&2; exit 2; }
    local r_b="" p_b="" s_b=""
    case "$register" in server) s_b=1 ;; any) s_b=0 ;; "") ;; *) echo "$usage" >&2; exit 2 ;; esac
    case "$replies" in on) r_b=1 ;; off) r_b=0 ;; "") ;; *) echo "$usage" >&2; exit 2 ;; esac
    case "$previews" in on) p_b=1 ;; off) p_b=0 ;; "") ;; *) echo "$usage" >&2; exit 2 ;; esac
    case "$maxd" in ""|*[!0-9]*) [ -z "$maxd" ] || { echo "$usage" >&2; exit 2; } ;; esac
    { [ -z "$r_b" ] && [ -z "$p_b" ] && [ -z "$maxd" ] && [ -z "$s_b" ]; } && { echo "$usage" >&2; exit 2; }
    _app_push_on_post "$(_compute_json K2_CJ_app="$app" K2_CJ_agentReplies_b="$r_b" K2_CJ_previews_b="$p_b" K2_CJ_maxDevices_n="$maxd" K2_CJ_serverOnly_b="$s_b")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_push_status_print
}

cmd_app_push_send() {
    local mode="$1"; shift
    local app="" room="" title="" text="" topic="" collapse="" thread="" sound="" badge="" prio="" ttl="" chan="" silent=0 json_out=0
    local tos=() datas=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) tos+=("${2:-}"); shift 2 ;;
            --room) room="${2:-}"; shift 2 ;;
            --title) title="${2:-}"; shift 2 ;;
            --body) text="${2:-}"; shift 2 ;;
            --data) datas+=("${2:-}"); shift 2 ;;
            --topic) topic="${2:-}"; shift 2 ;;
            --collapse-id) collapse="${2:-}"; shift 2 ;;
            --thread-id) thread="${2:-}"; shift 2 ;;
            --sound) sound="${2:-}"; shift 2 ;;
            --badge) badge="${2:-}"; shift 2 ;;
            --priority) prio="${2:-}"; shift 2 ;;
            --ttl) ttl="${2:-}"; shift 2 ;;
            --android-channel) chan="${2:-}"; shift 2 ;;
            --silent) silent=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_push_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    if [ "$mode" = test ]; then
        { [ -z "$app" ] || [ "${#tos[@]}" -eq 0 ]; } && { echo "usage: k2 app push test <app> --to <user>" >&2; exit 2; }
    else
        [ -n "$app" ] || { echo "usage: k2 app push send <app> (--to <user>… | --room <room>) --title T --body B" >&2; exit 2; }
        { [ "${#tos[@]}" -eq 0 ] && [ -z "$room" ]; } && { echo "name --to <user> (one or more) or --room <room>" >&2; exit 2; }
        { [ "${#tos[@]}" -gt 0 ] && [ -n "$room" ]; } && { echo "--to or --room, not both" >&2; exit 2; }
    fi
    local body
    body="$(K2_AP_APP="$app" K2_AP_ROOM="$room" K2_AP_TITLE="$title" K2_AP_BODY="$text" K2_AP_TOPIC="$topic" K2_AP_COLLAPSE="$collapse" \
        K2_AP_THREAD="$thread" K2_AP_SOUND="$sound" K2_AP_BADGE="$badge" K2_AP_PRIO="$prio" K2_AP_TTL="$ttl" K2_AP_CHAN="$chan" \
        K2_AP_SILENT="$silent" K2_AP_MODE="$mode" python3 -c '
import json, os, sys
e = os.environ
args = sys.argv[1:]
sep = args.index("--")
tos, datas = args[:sep], args[sep + 1:]
b = {"app": e["K2_AP_APP"]}
if e.get("K2_AP_ROOM"):
    b["to"] = {"room": e["K2_AP_ROOM"]}
elif len(tos) == 1:
    b["to"] = {"guest": tos[0]}
else:
    b["to"] = {"guests": tos}
if e["K2_AP_MODE"] == "send":
    for k, name in (("K2_AP_TITLE", "title"), ("K2_AP_BODY", "body"), ("K2_AP_TOPIC", "topic"), ("K2_AP_COLLAPSE", "collapseId"),
                    ("K2_AP_THREAD", "threadId"), ("K2_AP_SOUND", "sound"), ("K2_AP_PRIO", "priority"), ("K2_AP_CHAN", "androidChannel")):
        if e.get(k):
            b[name] = e[k]
    for k, name in (("K2_AP_BADGE", "badge"), ("K2_AP_TTL", "ttlSecs")):
        if e.get(k):
            try:
                b[name] = int(e[k])
            except ValueError:
                print("--%s is a number" % name.lower(), file=sys.stderr); sys.exit(2)
    if e.get("K2_AP_SILENT") == "1":
        b["silent"] = True
    if datas:
        d = {}
        for kv in datas:
            if "=" not in kv:
                print("--data is key=value", file=sys.stderr); sys.exit(2)
            k, v = kv.split("=", 1)
            d[k] = v
        b["data"] = d
print(json.dumps(b))' "${tos[@]}" -- "${datas[@]}")" || exit 2
    _compute_send_call POST "/cli/app/push/$mode" "$body"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("skipped") or {}
skipped = ", ".join("%s %s" % (n, k.replace("_", " ")) for k, n in s.items() if n)
print("queued %s device send(s), send %s%s" % (v.get("queued"), v.get("sendId"), ("; skipped: " + skipped) if skipped else ""))
print("  counts: k2 app push log " + sys.argv[1])' "$app"
}

# POST that also takes 202 (a queued send) as success.
_compute_send_call() {
    local raw
    raw="$(_compute_send "$1" "$2" "$3")"
    _compute_split "$raw"
    case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
}

cmd_app_push_log() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app push log <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/push/log "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, datetime
v = json.load(sys.stdin)
rows = v.get("sends") or []
if not rows:
    print("No sends in the last hour."); sys.exit(0)
for r in rows:
    at = datetime.datetime.fromtimestamp(r.get("at") or 0).strftime("%H:%M:%S")
    sk = sum((r.get("skipped") or {}).values())
    print("%s  %s  queued %s  sent %s  dead %s  failed %s  wrong app %s  dropped %s  skipped %s" % (
        at, r.get("sendId"), r.get("queued"), r.get("sent"), r.get("dead"), r.get("failed"), r.get("wrongApp"), r.get("dropped"), sk))
if v.get("pending"):
    print("%s device send(s) still going out" % v["pending"])'
}

cmd_app_push_devices() {
    if [ "${1:-}" = rm ] || [ "${1:-}" = remove ]; then
        shift
        local app="${1:-}" id="${2:-}"
        { [ -z "$app" ] || [ -z "$id" ]; } && { echo "usage: k2 app push devices rm <app> <apd_…>" >&2; exit 2; }
        _compute_call POST /cli/app/push/devices/remove "$(_compute_json K2_CJ_app="$app" K2_CJ_id="$id")"
        echo "removed $id"
        return 0
    fi
    local app="" guest="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --guest|--user) guest="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app push devices <app> [--guest <user>] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$guest" ] && q="$q&guest=$(urlencode "$guest")"
    _compute_call GET /cli/app/push/devices "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, datetime
v = json.load(sys.stdin)
ds = v.get("devices") or []
if not ds:
    print("No registered phones."); sys.exit(0)
for d in ds:
    seen = datetime.datetime.fromtimestamp(d.get("lastSeenAt") or 0).strftime("%Y-%m-%d %H:%M")
    print("%s  %-16s %-5s %-10s %-24s seen %s" % (d["id"], (d.get("username") or "-")[:16], d.get("platform"), d.get("environment"), (d.get("deviceId") or "")[:24], seen))'
}

# k2 app email … and k2 notifications email … (prd-email-notifications-core-v1,
# 0.46.5): one-way email from SENDERS no agent can read. The owner adds a
# sender (k2 notifications email sender add) and grants it to an App (k2 app
# email on); the App sends with email.send on its link, an App key with scope
# email, or its agent here under its email sending setting.
_app_email_usage() {
    cat <<'EOF'
k2 app email — an App's own email (Email Notifications). The App sends from
a sender the owner granted it (noreply@your-domain), never from a mailbox; no
agent can read the sender's mail.

  k2 app email show <app> [--json]   On or off, the sender, the recipient scope.
  k2 app email on <app> --sender <address|id> [--scope users|any]
                                     The owner, in person, from a terminal
                                     outside K2 (Touch ID / login / vault
                                     password confirms; scope any asks twice).
  k2 app email off <app>             The owner, an Admin or the App's agent.
  k2 app email test <app>            K2's fixed text to the owner's own address.
  k2 app email send <app> --to <addr> [--to …] [--cc …] [--bcc …]
                 --subject S (--text T | --text-file F) [--html-file F]
                 [--reply-to <addr>] [--header X-Name=value]… [--tag t]…
                 [--idempotency-key K] [--json]
                                     The owner and Admins send as themselves;
                                     the App's agent needs "…and manage them
                                     end to end" and its email sending setting
                                     (approval holds the email for the owner).
  k2 app email log <app> [--limit N] [--status S] [--tag T] [--json]
  k2 app email status <app> <id> [--json]

Scope users (default): only this App's users whose address is confirmed
(they used a link K2 mailed them). Scope any: any address, lower limits.
Limits: 100 recipients an hour and 1,000 a day per App (any: 30 and 300;
a domain's first 14 days: 20 and 200). Senders and relays: k2 notifications
email --help. The whole guide: k2 study email-notifications.
EOF
}

# POST an Email Notifications body; on vault_confirm_required (the owner,
# outside K2) confirm in person and send it again. app-email-any needs a
# second confirm (confirmAnyToken).
_email_post() {
    local path="$1" body="$2" tries=0
    while :; do
        _app_rt_post "$path" "$body"
        if [ "$_CP_STATUS" = "403" ] && [[ "$_CP_BODY" == *'"vault_confirm_required"'* ]] && ! _compute_in_session && [ "$tries" -lt 3 ]; then
            local action sid field tok
            action="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("action") or "")')"
            sid="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; e=json.load(sys.stdin).get("error") or {}; print(e.get("secretId") or e.get("serviceId") or "")')"
            { [ -n "$action" ] && [ -n "$sid" ]; } || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            tok="$(_vault_confirm_token "$action" "$sid" "")" || exit 3
            field=confirmToken
            [ "$action" = "app-email-any" ] && field=confirmAnyToken
            body="$(printf '%s' "$body" | K2_EM_TOK="$tok" K2_EM_FIELD="$field" python3 -c 'import json,os,sys; b=json.load(sys.stdin); b[os.environ["K2_EM_FIELD"]]=os.environ["K2_EM_TOK"]; print(json.dumps(b))')"
            tok=""
            tries=$((tries + 1))
            continue
        fi
        break
    done
    case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
}

_app_email_state_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("sender") or {}
state = "on" if v.get("on") else "off"
print("%s: email %s%s" % (v.get("name") or v.get("app"), state, "" if v.get("boxSwitch") == "on" else " (K2_EMAIL_NOTIFICATIONS=off on this server)"))
if s:
    print("  sender:  %s (%s, %s, %s)" % (s.get("address"), s.get("fromName"), s.get("path"), s.get("state")))
else:
    print("  sender:  none (the owner grants one: k2 app email on <app> --sender <address>)")
print("  scope:   %s" % v.get("recipientScope"))
if v.get("offReason") and state == "off":
    print("  off:     %s" % v["offReason"])'
}

cmd_app_email() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _app_email_usage; return 0 ;;
    esac
    shift
    case "$sub" in
        show|status-of) cmd_app_email_show "$@" ;;
        on) cmd_app_email_on "$@" ;;
        off) cmd_app_email_onoff off "$@" ;;
        test) cmd_app_email_onoff test "$@" ;;
        send) cmd_app_email_send "$@" ;;
        log) cmd_app_email_log "$@" ;;
        status) cmd_app_email_status "$@" ;;
        *) echo "usage: k2 app email show|on|off|test|send|log|status (k2 app email --help)" >&2; exit 2 ;;
    esac
}

cmd_app_email_show() {
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_email_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app email show <app> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/email "app=$(urlencode "$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_email_state_print
}

cmd_app_email_on() {
    local app="" sender="" scope="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --sender|--from) sender="${2:-}"; shift 2 ;;
            --scope) scope="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_email_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$sender" ]; } && { echo "usage: k2 app email on <app> --sender <address|id> [--scope users|any]" >&2; exit 2; }
    case "$scope" in ""|users|any) ;; *) echo "--scope is users or any" >&2; exit 2 ;; esac
    _email_post /cli/app/email/on "$(_compute_json K2_CJ_app="$app" K2_CJ_sender="$sender" K2_CJ_scope="$scope")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _app_email_state_print
}

cmd_app_email_onoff() {
    local mode="$1"; shift
    local app="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -h|--help) _app_email_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app email $mode <app>" >&2; exit 2; }
    _email_post "/cli/app/email/$mode" "$(_compute_json K2_CJ_app="$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    if [ "$mode" = test ]; then
        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("test email %s: %s (to %s)" % (v.get("id"), v.get("result"), v.get("to")))'
    else
        printf '%s' "$_CP_BODY" | _app_email_state_print
    fi
}

cmd_app_email_send() {
    local app="" subject="" text="" text_file="" html_file="" key="" json_out=0
    local tos=() ccs=() bccs=() rts=() hdrs=() tags=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --to) tos+=("${2:-}"); shift 2 ;;
            --cc) ccs+=("${2:-}"); shift 2 ;;
            --bcc) bccs+=("${2:-}"); shift 2 ;;
            --reply-to) rts+=("${2:-}"); shift 2 ;;
            --header) hdrs+=("${2:-}"); shift 2 ;;
            --tag) tags+=("${2:-}"); shift 2 ;;
            --subject) subject="${2:-}"; shift 2 ;;
            --text) text="${2:-}"; shift 2 ;;
            --text-file) text_file="${2:-}"; shift 2 ;;
            --html-file) html_file="${2:-}"; shift 2 ;;
            --idempotency-key) key="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_email_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    local usage="usage: k2 app email send <app> --to <addr> --subject S (--text T | --text-file F) [--html-file F]"
    { [ -z "$app" ] || [ "${#tos[@]}" -eq 0 ] || [ -z "$subject" ]; } && { echo "$usage" >&2; exit 2; }
    { [ -z "$text" ] && [ -z "$text_file" ] && [ -z "$html_file" ]; } && { echo "$usage" >&2; exit 2; }
    [ -z "$text_file" ] || [ -r "$text_file" ] || { echo "can't read $text_file" >&2; exit 2; }
    [ -z "$html_file" ] || [ -r "$html_file" ] || { echo "can't read $html_file" >&2; exit 2; }
    local body
    body="$(K2_EM_APP="$app" K2_EM_SUBJECT="$subject" K2_EM_TEXT="$text" K2_EM_TEXTF="$text_file" K2_EM_HTMLF="$html_file" K2_EM_KEY="$key" python3 -c '
import json, os, sys
e = os.environ
args = sys.argv[1:]
lists = {"to": [], "cc": [], "bcc": [], "replyTo": [], "headers": [], "tags": []}
cur = None
for a in args:
    if a.startswith("--") and a[2:] in lists:
        cur = a[2:]
        continue
    lists[cur].append(a)
b = {"app": e["K2_EM_APP"], "subject": e["K2_EM_SUBJECT"], "to": lists["to"]}
for k in ("cc", "bcc", "replyTo", "tags"):
    if lists[k]:
        b[k] = lists[k]
if lists["headers"]:
    h = {}
    for kv in lists["headers"]:
        if "=" not in kv:
            print("--header is X-Name=value", file=sys.stderr); sys.exit(2)
        k, v = kv.split("=", 1)
        h[k] = v
    b["headers"] = h
if e.get("K2_EM_TEXTF"):
    with open(e["K2_EM_TEXTF"], encoding="utf-8") as f:
        b["text"] = f.read()
elif e.get("K2_EM_TEXT"):
    b["text"] = e["K2_EM_TEXT"]
if e.get("K2_EM_HTMLF"):
    with open(e["K2_EM_HTMLF"], encoding="utf-8") as f:
        b["html"] = f.read()
if e.get("K2_EM_KEY"):
    b["idempotencyKey"] = e["K2_EM_KEY"]
print(json.dumps(b))' --to "${tos[@]}" --cc "${ccs[@]}" --bcc "${bccs[@]}" --replyTo "${rts[@]}" --headers "${hdrs[@]}" --tags "${tags[@]}")" || exit 2
    _email_post /cli/app/email/send "$body"
    body=""
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
line = "email %s: %s (%s recipient%s)" % (v.get("id"), v.get("status"), v.get("recipients"), "" if v.get("recipients") == 1 else "s")
if v.get("status") == "held":
    line += " — held for the owner (k2 mail approvals)"
print(line)'
    # Held for the owner's approval: exit 2 (still pending), like k2 mail send.
    if printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("status") == "held" else 1)'; then
        exit 2
    fi
    return 0
}

cmd_app_email_log() {
    local app="" limit="" status="" tag="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --limit) limit="${2:-}"; shift 2 ;;
            --status) status="${2:-}"; shift 2 ;;
            --tag) tag="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_email_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) app="$1"; shift ;;
        esac
    done
    [ -n "$app" ] || { echo "usage: k2 app email log <app> [--limit N] [--status S] [--tag T] [--json]" >&2; exit 2; }
    local q="app=$(urlencode "$app")"
    [ -n "$limit" ] && q="$q&limit=$(urlencode "$limit")"
    [ -n "$status" ] && q="$q&status=$(urlencode "$status")"
    [ -n "$tag" ] && q="$q&tag=$(urlencode "$tag")"
    _compute_call GET /cli/app/email/log "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | _email_log_print
}

_email_log_print() {
    python3 -c '
import json, sys, datetime
v = json.load(sys.stdin)
rows = v.get("items") or []
if not rows:
    print("No email in the log (it keeps 30 days)."); sys.exit(0)
for r in rows:
    at = datetime.datetime.fromtimestamp(r.get("createdAt") or 0).strftime("%Y-%m-%d %H:%M")
    to = r.get("to") or []
    who = (to[0] + (" +%d" % (len(to) - 1) if len(to) > 1 else "")) if to else "-"
    extra = (" " + r["error"]) if r.get("error") else ""
    what = r.get("subject") or r.get("purpose") or ""
    print("%s  %s  %-9s%s  %s  %s" % (at, r.get("id"), r.get("status"), extra, who, what[:60]))'
}

cmd_app_email_status() {
    local app="" id="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) if [ -z "$app" ]; then app="$1"; else id="$1"; fi; shift ;;
        esac
    done
    { [ -z "$app" ] || [ -z "$id" ]; } && { echo "usage: k2 app email status <app> <id> [--json]" >&2; exit 2; }
    _compute_call GET /cli/app/email/status "app=$(urlencode "$app")&id=$(urlencode "$id")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("%s: %s%s (%s recipients)" % (v.get("id"), v.get("status"), (" " + v["error"]) if v.get("error") else "", v.get("recipients")))'
}

_notifications_usage() {
    cat <<'EOF'
k2 notifications email — the senders K2's notification mail leaves from
(Email Notifications). Owner and Admins only; adding a sender or a relay,
choosing the system sender and test sends need the owner in person (Touch
ID / login / vault password), from a terminal outside K2.

  k2 notifications email [status] [--json]
  k2 notifications email sender add <address> [--name N] [--lock-name]
                 [--reply-to <addr>]… [--relay <relay id> | --via <bounce subdomain>]
                                    A send-only account K2 creates (hosted
                                    mail, Linux) or an address on a relay. It
                                    is never a mailbox: no agent can read it.
                                    --via: send as noreply@brand.com when
                                    brand.com's mail lives elsewhere (Google,
                                    Microsoft…). Hosted mail only (Linux). Host
                                    a subdomain here first: k2 hostmail domain
                                    add bounce.brand.com prints the DNS records
                                    to publish (MX, SPF, DKIM, DMARC); then
                                    k2 hostmail domain check bounce.brand.com.
                                    Bounces and the signature use it, and K2
                                    never changes brand.com's DNS. brand.com
                                    needs a DMARC record that isn't strict on
                                    both checks: adkim=s with aspf=s is
                                    refused; one relaxed (r, the default) is
                                    enough.
  k2 notifications email sender list [--all] [--json]
  k2 notifications email sender check <sender>   re-check DMARC and the domain
  k2 notifications email sender set <sender> [--name N] [--lock-name|--unlock-name] [--reply-to <addr>]…
  k2 notifications email sender rm <sender>       its address stays reserved 30 days
  k2 notifications email sender resume <sender>   the owner, with a confirm: an App's sender
                                    that K2 paused (over 5% hard bounces or 0.1%
                                    spam complaints of at least 50 recipients in
                                    24 h) sends again, and the 24-hour count
                                    starts again from zero. `sender list` marks
                                    paused senders (and shows each App's). The
                                    system sender is never paused (you get a
                                    warning notice instead).
                                    <sender>, everywhere here: its address or
                                    its id from `sender list`.
  k2 notifications email suppressions [list] [<address>] [--limit N] [--json]
                                    Addresses K2 won't send to: a hard bounce
                                    (every sender), a spam complaint (that sender
                                    only), or added by hand. Kept hashed: the
                                    list shows the domain, reason and hash;
                                    give an address to see only its entries.
  k2 notifications email suppressions add <address> [--sender <sender>]
                                    Owner or Admin. Without --sender: every sender.
  k2 notifications email suppressions rm <address> [--sender <sender> | --all]
                                    The owner: mail may go to it again. No flag:
                                    removes its every-sender entry (a hard
                                    bounce, or one added for every sender);
                                    --sender: that sender's entry (a complaint);
                                    --all: every entry for the address.
  k2 notifications email system-sender [show] | set <sender> | clear | test
                                    K2's own mail (owner password resets with
                                    no --from, codes) comes from it.
  k2 notifications email relay add --host H [--port P] [--tls implicit|starttls]
                 --username U --from-domain D   (the password is read from stdin)
  k2 notifications email relay list | rm <relay>
  k2 notifications email relay test <relay> --from <sender>
                                    One test to your own address; the sender
                                    goes ready when the relay takes it.
  k2 notifications email log [--app <app>] [--limit N] [--json]
  k2 notifications email settings [--retention-days 7-90] [--owner-email <you@…>]
                                    Test emails only ever go to --owner-email.

Your notice address: where emails FROM this server's AI agents reach you.
Run these yourself from a terminal outside K2 (any person with a login on this
server, or the server's owner). An agent can't run them (refused, 403).
  k2 notifications email address [show] [--person <login>] [--json]
                                    Your confirmed address ("address") and any
                                    still waiting for a confirm ("pending", up
                                    to 3). --person: another person's (the
                                    server's owner or an Admin only).
  k2 notifications email address set <you@example.com>
                                    Emails that address a confirm link from the
                                    system sender. The link expires after 24
                                    hours. Notices go there only after you press
                                    Confirm on the page it opens. Your current
                                    address keeps working until the new one is
                                    confirmed, then it is replaced; if the link
                                    expires, nothing changes. Setting the same
                                    address again sends a fresh link.
  k2 notifications email address remove [<address>] [--person <login>]
                                    Stops notices to <address> now, confirmed or
                                    still waiting. With no address: all of them
                                    (confirmed and waiting). --person: another
                                    person's (the server's owner or an Admin).

Agents' email notices (an agent emails a person on this server; off by default):
  k2 notifications email agent [list] [--json]
  k2 notifications email agent on <workspace> --sender <address>
                                    The owner, with a confirm: the workspace's
                                    agent may email people here from <address>
                                    (a sender of its own: not the system
                                    sender, not an App's; add it first with
                                    sender add). Agents then send with
                                    k2 notify email (see k2 notify --help).
  k2 notifications email agent off <workspace>    the owner or an Admin

A hosted sender needs its domain Verified (k2 hostmail domain check) and a
DMARC record (any policy). On a Mac (no hosted mail), add a relay first.
Grant a sender to an App: k2 app email on. The guide: k2 study email-notifications.
EOF
}

cmd_notifications() {
    case "${1:-}" in
        email) shift; cmd_notifications_email "$@" ;;
        ""|-h|--help|help) _notifications_usage ;;
        *) echo "usage: k2 notifications email … (k2 notifications --help)" >&2; exit 2 ;;
    esac
}

# N10a: the owner's switch for a workspace's agent notices.
cmd_notifications_agent() {
    local verb="${1:-list}"
    [ $# -gt 0 ] && shift
    local ws="" sender="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --sender) sender="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) [ -z "$ws" ] || { echo "one workspace at a time" >&2; exit 2; }; ws="$1"; shift ;;
        esac
    done
    case "$verb" in
        list|ls|--json)
            _compute_call GET /cli/notifications/email/agents "" tcp
            if [ "$json_out" = 1 ] || [ "$verb" = --json ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("agents") or []
if not rows:
    print("No workspace has email notices (k2 notifications email agent on <workspace> --sender <address>).")
for r in rows:
    print("%-28s %-3s %s%s" % (r.get("workspace"), "on" if r.get("on") else "off", r.get("sender") or "(sender removed)", ("  (" + r["offReason"] + ")") if r.get("offReason") else ""))'
            ;;
        on)
            { [ -n "$ws" ] && [ -n "$sender" ]; } || { echo "usage: k2 notifications email agent on <workspace> --sender <address>" >&2; exit 2; }
            local body
            body="$(K2_EA_WS="$ws" K2_EA_S="$sender" python3 -c 'import json,os; print(json.dumps({"workspace": os.environ["K2_EA_WS"], "sender": os.environ["K2_EA_S"]}))')"
            _email_post /cli/notifications/email/agent/on "$body"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("email notices on for %s from %s" % (v.get("workspace"), v.get("sender")))'
            ;;
        off)
            [ -n "$ws" ] || { echo "usage: k2 notifications email agent off <workspace>" >&2; exit 2; }
            local body
            body="$(K2_EA_WS="$ws" python3 -c 'import json,os; print(json.dumps({"workspace": os.environ["K2_EA_WS"]}))')"
            _email_post /cli/notifications/email/agent/off "$body"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("email notices off for %s" % v.get("workspace"))'
            ;;
        *) echo "usage: k2 notifications email agent [list] | on <workspace> --sender <address> | off <workspace>" >&2; exit 2 ;;
    esac
}

_notify_usage() {
    cat <<'EOF'
k2 notify email: an agent emails a person on this server (text only).

  k2 notify email <login|owner> --subject "…" --text "…"
  k2 notify email <login|owner> --subject "…" --text-file <file>
                 One email to that person, from the workspace you run it in
                 (there is no --workspace) and its own sender. <login> is
                 their K2 login on this server; type `owner` literally for
                 the server's owner. Never an email address.
                 Text only: a subject up to 200 characters, a body up to
                 16 KB; no HTML, no attachments, no Reply-To. K2 adds a line
                 saying which agent sent it, and a link to stop them.
                 Refusals (the error code, then what to do):
                   agent_email_off    off for this workspace: only the owner
                                      turns it on. Ask them: run the
                                      "Ask with:" line the error ends with,
                                      if it has one, else a Thread message.
                   no_notice_address  the person hasn't confirmed an address
                                      (they run k2 notifications email
                                      address set themselves); tell them.
                   person_muted       they muted this workspace; don't retry.
                   rate_limited       10 an hour, 50 a day per workspace;
                                      retryAfterMs says when.
  k2 notify email off
                 Turn this workspace's notices off. Only the owner turns them
                 back on.

For people (not agents), from their own terminal:
  k2 notify email mute <workspace>      stop that workspace's notices to you
  k2 notify email unmute <workspace>
EOF
}

cmd_notify() {
    case "${1:-}" in
        email) shift; cmd_notify_email "$@" ;;
        ""|-h|--help|help) _notify_usage ;;
        *) echo "usage: k2 notify email … (k2 notify --help)" >&2; exit 2 ;;
    esac
}

cmd_notify_email() {
    case "${1:-}" in
        ""|-h|--help|help) _notify_usage; return 0 ;;
        off)
            [ $# -eq 1 ] || { echo "usage: k2 notify email off" >&2; exit 2; }
            _app_rt_post /cli/notify/email/off '{}'
            [ "$_CP_STATUS" = 200 ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            echo "email notices off for this workspace (the owner turns them back on)"
            return 0
            ;;
        mute|unmute)
            local verb="$1" ws="${2:-}"
            { [ -n "$ws" ] && [ $# -eq 2 ]; } || { echo "usage: k2 notify email $verb <workspace>" >&2; exit 2; }
            local body raw
            body="$(K2_EA_WS="$ws" K2_EA_M="$verb" python3 -c 'import json,os; print(json.dumps({"workspace": os.environ["K2_EA_WS"], "muted": os.environ["K2_EA_M"] == "mute"}))')"
            raw="$(_compute_send POST /cli/notify/email/mute "$body" tcp)"
            _compute_split "$raw"
            [ "$_CP_STATUS" = 200 ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(("muted" if v.get("muted") else "unmuted") + " email notices from " + str(v.get("workspace")))'
            return 0
            ;;
    esac
    local person="$1"
    shift
    local subject="" text="" text_file="" have_text=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --subject) subject="${2:-}"; shift 2 ;;
            --text) text="${2:-}"; have_text=1; shift 2 ;;
            --text-file) text_file="${2:-}"; shift 2 ;;
            *) echo "Unknown argument: $1 (k2 notify --help)" >&2; exit 2 ;;
        esac
    done
    if [ -n "$text_file" ]; then
        [ "$have_text" = 0 ] || { echo "--text or --text-file, not both" >&2; exit 2; }
        [ -f "$text_file" ] || { echo "no file $text_file" >&2; exit 2; }
        text="$(cat "$text_file")"
    fi
    { [ -n "$subject" ] && [ -n "$text" ]; } || { echo "usage: k2 notify email <login|owner> --subject \"…\" --text \"…\"" >&2; exit 2; }
    local body
    body="$(K2_NE_TO="$person" K2_NE_S="$subject" K2_NE_T="$text" python3 -c 'import json,os; e=os.environ; print(json.dumps({"to": e["K2_NE_TO"], "subject": e["K2_NE_S"], "text": e["K2_NE_T"]}))')"
    _app_rt_post /cli/notify/email "$body"
    case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("sent to %s (%s, %s)" % (v.get("to"), v.get("id"), v.get("status")))'
}

_notif_sender_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("sender") or v.get("systemSender")
if not s:
    print("no sender"); sys.exit(0)
owner = s.get("owner") or "unassigned"
print("%s  %s  %s  %s  %s (%s)" % (s.get("id"), s.get("address"), s.get("path"), s.get("state"), owner, s.get("fromName")))
if s.get("envelope"):
    print("  sends through %s (bounces and the signature use it; the From is %s)" % (s.get("envelope"), s.get("address")))
nr = s.get("notReady")
if nr:
    print("  not ready: %s: %s" % (nr.get("code"), nr.get("hint")))
if s.get("next"):
    print("  next: " + s["next"])'
}

cmd_notifications_email() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        -h|--help|help) _notifications_usage; return 0 ;;
        status|--json)
            local json_out=0
            [ "$sub" = "--json" ] && json_out=1
            [ "${1:-}" = "--json" ] && json_out=1
            _compute_call GET /cli/notifications/email "" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Email Notifications: %s; hosted mail %s%s" % (v.get("boxSwitch"), "here" if v.get("hostedMail") else "not on this server (use a relay)", "; AIR-GAP ON" if v.get("airgap") else ""))
sys_s = v.get("systemSender")
print("system sender: %s" % ((sys_s.get("address") + " (" + sys_s.get("state") + ")") if sys_s else "none"))
for s in v.get("senders") or []:
    print("  %s  %-34s %-6s %-11s %s" % (s.get("id"), s.get("address"), s.get("path"), s.get("state"), s.get("owner") or "unassigned"))
for r in v.get("relays") or []:
    print("  relay %s  %s:%s %s  user %s  for %s  test %s" % (r.get("id"), r.get("host"), r.get("port"), r.get("tls"), r.get("username"), r.get("fromDomain"), {True: "ok", False: "FAILED"}.get(r.get("lastTestOk"), "never")))
st = v.get("settings") or {}
print("log kept %s days; owner address %s" % (st.get("logRetentionDays"), st.get("ownerEmail") or "not set (k2 notifications email settings --owner-email)"))'
            ;;
        sender) cmd_notifications_sender "$@" ;;
        system-sender) cmd_notifications_system "$@" ;;
        relay|relays) cmd_notifications_relay "$@" ;;
        address) cmd_notifications_address "$@" ;;
        suppressions|suppression) cmd_notifications_suppressions "$@" ;;
        agent|agents) cmd_notifications_agent "$@" ;;
        log)
            local app="" limit="" json_out=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --app) app="${2:-}"; shift 2 ;;
                    --limit) limit="${2:-}"; shift 2 ;;
                    --json) json_out=1; shift ;;
                    *) echo "Unknown flag: $1" >&2; exit 2 ;;
                esac
            done
            local q=""
            [ -n "$app" ] && q="app=$(urlencode "$app")"
            [ -n "$limit" ] && q="${q:+$q&}limit=$(urlencode "$limit")"
            _compute_call GET /cli/notifications/email/log "$q" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | _email_log_print
            ;;
        settings)
            local days="" owner="" set_owner=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --retention-days) days="${2:-}"; shift 2 ;;
                    --owner-email) owner="${2:-}"; set_owner=1; shift 2 ;;
                    *) echo "Unknown flag: $1" >&2; exit 2 ;;
                esac
            done
            local body
            body="$(K2_EM_DAYS="$days" K2_EM_OWNER="$owner" K2_EM_SETO="$set_owner" python3 -c '
import json, os
e = os.environ
b = {}
if e.get("K2_EM_DAYS"):
    b["logRetentionDays"] = int(e["K2_EM_DAYS"])
if e.get("K2_EM_SETO") == "1":
    b["ownerEmail"] = e.get("K2_EM_OWNER") or ""
print(json.dumps(b))')" || { echo "--retention-days is a number" >&2; exit 2; }
            _email_post /cli/notifications/email/settings "$body"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; s=json.load(sys.stdin).get("settings") or {}; print("log kept %s days; owner address %s" % (s.get("logRetentionDays"), s.get("ownerEmail") or "not set"))'
            ;;
        *) echo "usage: k2 notifications email status|sender|system-sender|relay|suppressions|address|agent|log|settings (--help)" >&2; exit 2 ;;
    esac
}

# Email Notifications part 2, slice V: a person's own notice address
# (GET /cli/notify/email/address, POST …/set, POST …/remove). The daemon
# resolves the person from the token; an agent's passport is refused.
cmd_notifications_address() {
    local verb="${1:-show}"
    [ $# -gt 0 ] && shift
    local person="" json_out=0 addr=""
    case "$verb" in
        -h|--help|help) _notifications_usage; return 0 ;;
        --json) verb=show; json_out=1 ;;
        --person) verb=show; person="${1:-}"; [ $# -gt 0 ] && shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --person) person="${2:-}"; [ -n "$person" ] || { echo "--person needs a login" >&2; exit 2; }; shift 2 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) [ -z "$addr" ] || { echo "one address at a time" >&2; exit 2; }; addr="$1"; shift ;;
        esac
    done
    case "$verb" in
        show)
            [ -z "$addr" ] || { echo "usage: k2 notifications email address [show] [--person <login>] [--json]" >&2; exit 2; }
            local q=""
            [ -n "$person" ] && q="person=$(urlencode "$person")"
            _compute_call GET /cli/notify/email/address "$q" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
a = v.get("address")
print("notice address: %s" % ((a.get("address") + " (confirmed)") if a else "none (k2 notifications email address set <you@example.com>)"))
for p in v.get("pending") or []:
    import time
    until = time.strftime("%Y-%m-%d %H:%M", time.localtime(p.get("expiresAt") or 0))
    print("  waiting for a confirm: %s (link sent; works until %s)" % (p.get("address"), until))'
            ;;
        set)
            { [ -n "$addr" ] && [ -z "$person" ]; } || { echo "usage: k2 notifications email address set <you@example.com> (your own only)" >&2; exit 2; }
            local body
            body="$(K2_EA_ADDR="$addr" python3 -c 'import json,os; print(json.dumps({"address": os.environ["K2_EA_ADDR"]}))')"
            _app_rt_post /cli/notify/email/address/set "$body"
            case "$_CP_STATUS" in 200|202) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
a = (json.load(sys.stdin).get("address") or {}).get("address")
print("Sent a confirm link to %s. It works for 24 hours; nothing else goes there until it is confirmed." % a)'
            ;;
        remove|rm)
            local body
            body="$(K2_EA_ADDR="$addr" K2_EA_PERSON="$person" python3 -c '
import json, os
b = {}
if os.environ.get("K2_EA_ADDR"):
    b["address"] = os.environ["K2_EA_ADDR"]
if os.environ.get("K2_EA_PERSON"):
    b["person"] = os.environ["K2_EA_PERSON"]
print(json.dumps(b))')"
            _app_rt_post /cli/notify/email/address/remove "$body"
            case "$_CP_STATUS" in 200) ;; *) _compute_fail "$_CP_STATUS" "$_CP_BODY" ;; esac
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print("Removed %s notice address(es)." % json.load(sys.stdin).get("removed"))'
            ;;
        *) echo "usage: k2 notifications email address [show] | set <address> | remove [<address>] [--person <login>]" >&2; exit 2 ;;
    esac
}

cmd_notifications_sender() {
    local verb="${1:-list}"
    [ $# -gt 0 ] && shift
    case "$verb" in
        add)
            local addr="" name="" lock=0 relay="" via=""
            local rts=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --name) name="${2:-}"; shift 2 ;;
                    --lock-name) lock=1; shift ;;
                    --reply-to) rts+=("${2:-}"); shift 2 ;;
                    --relay) relay="${2:-}"; shift 2 ;;
                    --via) via="${2:-}"; shift 2 ;;
                    -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) addr="$1"; shift ;;
                esac
            done
            [ -n "$addr" ] || { echo "usage: k2 notifications email sender add <address> [--name N] [--lock-name] [--reply-to A]… [--relay ID | --via <bounce subdomain>]" >&2; exit 2; }
            local body
            body="$(K2_EM_ADDR="$addr" K2_EM_NAME="$name" K2_EM_LOCK="$lock" K2_EM_RELAY="$relay" K2_EM_VIA="$via" python3 -c '
import json, os, sys
e = os.environ
b = {"address": e["K2_EM_ADDR"]}
if e.get("K2_EM_NAME"):
    b["name"] = e["K2_EM_NAME"]
if e.get("K2_EM_LOCK") == "1":
    b["nameLocked"] = True
if e.get("K2_EM_RELAY"):
    b["relay"] = e["K2_EM_RELAY"]
if e.get("K2_EM_VIA"):
    b["via"] = e["K2_EM_VIA"]
if sys.argv[1:]:
    b["replyTo"] = sys.argv[1:]
print(json.dumps(b))' "${rts[@]}")"
            _email_post /cli/notifications/email/sender/add "$body"
            printf '%s' "$_CP_BODY" | _notif_sender_print
            ;;
        list|ls)
            local q="" json_out=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --all) q="all=1"; shift ;;
                    --json) json_out=1; shift ;;
                    *) echo "Unknown flag: $1" >&2; exit 2 ;;
                esac
            done
            _compute_call GET /cli/notifications/email/senders "$q" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("senders") or []
if not rows:
    print("No senders yet: k2 notifications email sender add noreply@<your domain>"); sys.exit(0)
for s in rows:
    p = s.get("paused")
    paused = ("  PAUSED (%s; resume: k2 notifications email sender resume %s)" % (p.get("reason"), s.get("address"))) if p else ""
    print("%s  %-34s %-6s %-11s %-18s %s%s" % (s.get("id"), s.get("address"), s.get("path"), s.get("state"), s.get("owner") or "unassigned", s.get("fromName"), paused))'
            ;;
        resume)
            local who="${1:-}"
            { [ -n "$who" ] && [ $# -eq 1 ]; } || { echo "usage: k2 notifications email sender resume <sender>" >&2; exit 2; }
            local body
            body="$(K2_EM_S="$who" python3 -c 'import json,os; print(json.dumps({"sender": os.environ["K2_EM_S"]}))')"
            _email_post /cli/notifications/email/sender/resume "$body"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("resumed %s: its sends go out again (a new 24-hour count starts now)" % v.get("sender"))'
            ;;
        check|set|rm|remove)
            local who="" name="" lockflag=""
            local rts=()
            while [ $# -gt 0 ]; do
                case "$1" in
                    --name) name="${2:-}"; shift 2 ;;
                    --lock-name) lockflag=1; shift ;;
                    --unlock-name) lockflag=0; shift ;;
                    --reply-to) rts+=("${2:-}"); shift 2 ;;
                    -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) who="$1"; shift ;;
                esac
            done
            [ -n "$who" ] || { echo "usage: k2 notifications email sender $verb <sender>" >&2; exit 2; }
            [ "$verb" = remove ] && verb=rm
            local body
            body="$(K2_EM_S="$who" K2_EM_NAME="$name" K2_EM_LOCK="$lockflag" python3 -c '
import json, os, sys
e = os.environ
b = {"sender": e["K2_EM_S"]}
if e.get("K2_EM_NAME"):
    b["name"] = e["K2_EM_NAME"]
if e.get("K2_EM_LOCK") in ("0", "1"):
    b["nameLocked"] = e["K2_EM_LOCK"] == "1"
if sys.argv[1:]:
    b["replyTo"] = sys.argv[1:]
print(json.dumps(b))' "${rts[@]}")"
            _email_post "/cli/notifications/email/sender/$verb" "$body"
            if [ "$verb" = rm ]; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print("removed %s (%s); its address stays reserved %s days" % (v.get("removed"), v.get("address"), v.get("reservedDays")))'
            else
                printf '%s' "$_CP_BODY" | _notif_sender_print
            fi
            ;;
        *) echo "usage: k2 notifications email sender add|list|check|set|resume|rm" >&2; exit 2 ;;
    esac
}

# N6: addresses K2 won't send to (kept hashed; a search names the address).
cmd_notifications_suppressions() {
    local verb="${1:-list}"
    [ $# -gt 0 ] && shift
    local addr="" sender="" all=0 json_out=0 limit=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --sender) sender="${2:-}"; shift 2 ;;
            --all) all=1; shift ;;
            --limit) limit="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) [ -z "$addr" ] || { echo "one address at a time" >&2; exit 2; }; addr="$1"; shift ;;
        esac
    done
    case "$verb" in
        list|ls)
            local q=""
            [ -n "$addr" ] && q="address=$(urlencode "$addr")"
            [ -n "$limit" ] && q="${q:+$q&}limit=$(urlencode "$limit")"
            _compute_call GET /cli/notifications/email/suppressions "$q" tcp
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys, time
rows = json.load(sys.stdin).get("suppressions") or []
if not rows:
    print("No suppressed addresses" + (" matching that one." if len(sys.argv) > 1 and sys.argv[1] else "."))
for x in rows:
    where = "every sender" if x.get("scope") == "box" else "sender " + (x.get("scopeRef") or "?")
    print("%s  @%-28s %-12s %-16s %s" % (time.strftime("%Y-%m-%d", time.localtime(x.get("createdAt") or 0)), x.get("domain"), x.get("reason"), where, x.get("addressSha256")[:12]))' "$addr"
            ;;
        add|rm|remove)
            [ -n "$addr" ] || { echo "usage: k2 notifications email suppressions $verb <address> [--sender <sender>]" >&2; exit 2; }
            [ "$verb" = remove ] && verb=rm
            local body
            body="$(K2_EM_A="$addr" K2_EM_S="$sender" K2_EM_ALL="$all" python3 -c '
import json, os
e = os.environ
b = {"address": e["K2_EM_A"]}
if e.get("K2_EM_S"):
    b["sender"] = e["K2_EM_S"]
if e.get("K2_EM_ALL") == "1":
    b["all"] = True
print(json.dumps(b))')"
            _app_rt_post "/cli/notifications/email/suppressions/$verb" "$body"
            [ "$_CP_STATUS" = 200 ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
            if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            if [ "$verb" = add ]; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(("suppressed (%s)" % v.get("scope")) if v.get("added") else "already suppressed")'
            else
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print("removed %s suppression(s)" % json.load(sys.stdin).get("removed"))'
            fi
            ;;
        *) echo "usage: k2 notifications email suppressions [list] [<address>] | add <address> [--sender S] | rm <address> [--sender S|--all]" >&2; exit 2 ;;
    esac
}

cmd_notifications_system() {
    local verb="${1:-show}"
    [ $# -gt 0 ] && shift
    case "$verb" in
        show)
            _compute_call GET /cli/notifications/email/system-sender "" tcp
            printf '%s' "$_CP_BODY" | _notif_sender_print
            ;;
        set)
            [ -n "${1:-}" ] || { echo "usage: k2 notifications email system-sender set <sender>" >&2; exit 2; }
            _email_post /cli/notifications/email/system-sender/set "$(_compute_json K2_CJ_sender="$1")"
            printf '%s' "$_CP_BODY" | _notif_sender_print
            ;;
        clear)
            _email_post /cli/notifications/email/system-sender/clear '{}'
            echo "no system sender"
            ;;
        test)
            _email_post /cli/notifications/email/system-sender/test '{}'
            printf '%s' "$_CP_BODY" | _notif_test_print
            ;;
        *) echo "usage: k2 notifications email system-sender show|set|clear|test" >&2; exit 2 ;;
    esac
}

_notif_test_print() {
    python3 -c '
import json, sys
v = json.load(sys.stdin)
print("test email %s: %s (to %s)" % (v.get("id") or v.get("sender"), v.get("result"), v.get("to")))
c = v.get("checks") or {}
d = c.get("dmarc") or {}
print("  DMARC: %s" % (("found at %s, p=%s" % (d.get("at"), d.get("policy"))) if d.get("ok") else ("missing: " + str(d.get("hint")))))
if c.get("dkimPublished") is not None:
    print("  DKIM key published: %s; SPF published: %s" % (c.get("dkimPublished"), c.get("spfPublished")))
if c.get("note"):
    print("  " + c["note"])'
}

cmd_notifications_relay() {
    local verb="${1:-list}"
    [ $# -gt 0 ] && shift
    case "$verb" in
        add)
            local host="" port="" tls="" user="" dom=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --host) host="${2:-}"; shift 2 ;;
                    --port) port="${2:-}"; shift 2 ;;
                    --tls) tls="${2:-}"; shift 2 ;;
                    --username) user="${2:-}"; shift 2 ;;
                    --from-domain) dom="${2:-}"; shift 2 ;;
                    *) echo "Unknown flag: $1" >&2; exit 2 ;;
                esac
            done
            { [ -z "$host" ] || [ -z "$user" ] || [ -z "$dom" ]; } && { echo "usage: k2 notifications email relay add --host H [--port P] [--tls implicit|starttls] --username U --from-domain D (password on stdin)" >&2; exit 2; }
            case "$port" in ""|*[!0-9]*) [ -z "$port" ] || { echo "--port is a number" >&2; exit 2; } ;; esac
            local pw=""
            if [ -t 0 ]; then
                printf 'Relay password (SMTP password or API key): ' >&2
                IFS= read -rs pw || pw=""
                echo >&2
            else
                IFS= read -r pw || true
            fi
            [ -n "$pw" ] || { echo "no password given" >&2; exit 2; }
            local body
            body="$(printf '%s' "$pw" | K2_EM_HOST="$host" K2_EM_PORT="$port" K2_EM_TLS="$tls" K2_EM_USER="$user" K2_EM_DOM="$dom" python3 -c '
import json, os, sys
e = os.environ
b = {"host": e["K2_EM_HOST"], "username": e["K2_EM_USER"], "fromDomain": e["K2_EM_DOM"], "password": sys.stdin.read()}
if e.get("K2_EM_PORT"):
    b["port"] = int(e["K2_EM_PORT"])
if e.get("K2_EM_TLS"):
    b["tls"] = e["K2_EM_TLS"]
print(json.dumps(b))')"
            pw=""
            _email_post /cli/notifications/email/relay/add "$body"
            body=""
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin); r=v.get("relay") or {}; print("relay %s: %s:%s %s for %s" % (r.get("id"), r.get("host"), r.get("port"), r.get("tls"), r.get("fromDomain"))); print("  next: " + v.get("next", ""))'
            ;;
        list|ls)
            _compute_call GET /cli/notifications/email/relays "" tcp
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("relays") or []
if not rows:
    print("No relays."); sys.exit(0)
for r in rows:
    print("%s  %s:%s %s  user %s  for %s  last test %s" % (r.get("id"), r.get("host"), r.get("port"), r.get("tls"), r.get("username"), r.get("fromDomain"), {True: "ok", False: "FAILED"}.get(r.get("lastTestOk"), "never")))'
            ;;
        rm|remove)
            [ -n "${1:-}" ] || { echo "usage: k2 notifications email relay rm <relay>" >&2; exit 2; }
            _email_post /cli/notifications/email/relay/rm "$(_compute_json K2_CJ_relay="$1")"
            echo "removed relay $1"
            ;;
        test)
            local relay="" from=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --from) from="${2:-}"; shift 2 ;;
                    -*) echo "Unknown flag: $1" >&2; exit 2 ;;
                    *) relay="$1"; shift ;;
                esac
            done
            { [ -z "$relay" ] || [ -z "$from" ]; } && { echo "usage: k2 notifications email relay test <relay> --from <sender>" >&2; exit 2; }
            _email_post /cli/notifications/email/relay/test "$(_compute_json K2_CJ_relay="$relay" K2_CJ_from="$from")"
            printf '%s' "$_CP_BODY" | _notif_test_print
            ;;
        *) echo "usage: k2 notifications email relay add|list|rm|test" >&2; exit 2 ;;
    esac
}

# Held App requests of every kind (an agent's widening, an App key, clean
# mode off, a new --cmd service, new egress hosts, broad egress): one list,
# one approve (the owner, in person), one deny.
cmd_app_approvals() {
    if [ "${1:-}" = "revoke" ]; then
        shift
        _app_approvals_revoke "$@"
        return
    fi
    local app="" all=0 json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) app="${2:-}"; shift 2 ;;
            --all) all=1; shift ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            *) echo "Unknown argument: $1 (k2 app approvals [--app A] [--all])" >&2; exit 2 ;;
        esac
    done
    local q="state=pending"
    [ "$all" = 1 ] && q="state=all"
    [ -n "$app" ] && q="${q}&app=$(urlencode "$app")"
    _compute_call GET /cli/app/approvals "$q"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rs = json.load(sys.stdin).get("requests") or []
if not rs:
    print("No held App requests."); sys.exit(0)
for r in rs:
    print("%s  %-9s %-11s %s (%s) by %s" % (r["id"], r["status"], r["kind"], r["appLabel"], r.get("workspaceHandle"), r["requestedBy"]))
    print("    %s" % r["summary"])
    if r.get("reason"):
        print("    why: %s" % r["reason"])
    for why in r.get("reasons") or []:
        print("    held: %s" % why)
    if r.get("decidedBy"):
        print("    %s by %s%s" % (r["status"], r["decidedBy"], (" via " + r["confirmVia"]) if r.get("confirmVia") else ""))'
}

# Owner confirm bound to the request, then approve. Never for an agent:
# the daemon refuses passports, and the confirm needs a person.
_app_approve() {
    local id="$1"
    if _compute_in_session; then
        echo '{"error":"owner_only","hint":"the owner approves in person: Settings → App Approvals, or k2 app approve <id> from a terminal outside K2. An agent can never approve."}' >&2
        exit 3
    fi
    _compute_call GET /cli/app/approvals "state=all" tcp
    local found
    found="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
rows = [r for r in json.load(sys.stdin).get("requests") or [] if r["id"] == want or r["id"].startswith(want)]
if len(rows) != 1:
    sys.exit(1)
r = rows[0]
print(r["id"]); print(r["status"]); print(r["summary"][:64])
print("---"); print(r["summary"])
print("asked by %s%s" % (r["requestedBy"], (": " + r["reason"]) if r.get("reason") else ""))
' "$id")" || { echo "{\"error\":\"request_not_found\",\"hint\":\"no single App request matches '$id' (k2 app approvals)\"}" >&2; exit 3; }
    local full state label
    full="$(printf '%s\n' "$found" | sed -n 1p)"
    state="$(printf '%s\n' "$found" | sed -n 2p)"
    label="$(printf '%s\n' "$found" | sed -n 3p)"
    if [ "$state" != "pending" ]; then
        echo "{\"error\":\"not_pending\",\"hint\":\"$full is $state\"}" >&2
        exit 3
    fi
    printf '%s\n' "$found" | sed -n '5,$p' >&2
    local tok
    tok="$(_vault_confirm_token app-approve "$full" "$label")" || exit 3
    _compute_call POST /cli/app/approve "$(_compute_json K2_CJ_request="$full" K2_CJ_confirmToken="$tok")" tcp
    tok=""
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
r = v["request"]
print("%s %s: %s" % (r["id"], v.get("status"), r["summary"]))
if v.get("note"):
    print("  %s" % v["note"])
box = (v.get("result") or {}).get("boxServices") if isinstance(v.get("result"), dict) else None
for x in (box or {}).get("refused") or []:
    print("  box service %s not allowed: %s" % (x.get("name"), x.get("hint")))
if v.get("error"):
    print("  %s" % v["error"].get("hint")); sys.exit(1)'
}

cmd_app_approve() {
    case "${1:-}" in
        ""|-h|--help) echo "usage: k2 app approve <request-id>   (the owner, in person, from a terminal outside K2)" >&2; exit 2 ;;
    esac
    _app_approve "$1"
}

# `k2 app approvals revoke <w_id> | --app <name>` (0.46.6, client papercut 13):
# the owner withdraws an approved `k2 publish run`'s standing, so the next
# identical re-run asks again. Owner only (the daemon refuses a passport).
_app_approvals_revoke() {
    local id="" app="" json_out=0
    local usage="usage: k2 app approvals revoke <request-id> | --app <name> [--json]   (the owner)"
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) [ $# -ge 2 ] || { echo "$usage" >&2; exit 2; }; app="$2"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) echo "$usage"; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *) [ -z "$id" ] || { echo "$usage" >&2; exit 2; }; id="$1"; shift ;;
        esac
    done
    if { [ -z "$id" ] && [ -z "$app" ]; } || { [ -n "$id" ] && [ -n "$app" ]; }; then
        echo "$usage" >&2; exit 2
    fi
    _compute_call POST /cli/app/approvals/revoke "$(_compute_json K2_CJ_request="$id" K2_CJ_app="$app")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for r in v.get("revoked") or []:
    print("%s revoked: %s" % (r["id"], r["summary"]))
print(v.get("note") or "")'
}

cmd_app_deny() {
    local id="" reason="" json_out=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --reason) reason="${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) echo "usage: k2 app deny <request-id> [--reason TEXT]   (an owner/admin denies; the requester withdraws its own)" >&2; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) id="$1"; shift ;;
        esac
    done
    [ -n "$id" ] || { echo "usage: k2 app deny <request-id> [--reason TEXT]" >&2; exit 2; }
    _compute_call POST /cli/app/deny "$(_compute_json K2_CJ_request="$id" K2_CJ_reason="$reason")"
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)["request"]
print("%s %s: %s" % (r["id"], r["status"], r["summary"]))'
}
_app_access_print() {
    # The switches and cores as `GET /cli/app/list?workspace=` answers them.
    python3 -c '
import json, sys
v = json.load(sys.stdin).get("access") or {}
print("%s: Build and run Apps %s, manage them end to end %s, manage their users %s" % (v.get("workspace"), "on" if v.get("buildAndRun") else "off", "on" if v.get("manageEndToEnd") else "off", "on" if v.get("manageUsers") else "off"))
for c in v.get("coreList") or []:
    later = c.get("later")
    print("  [%s] %-16s %s%s" % ("x" if c.get("ticked") else " ", c.get("label", c.get("core")), c.get("core"), "  (" + later + ")" if later else ""))'
}

cmd_app_access() {
    local ws="" build="" manage="" users="" open="" json_out=0 cores=""
    local _usage="usage: k2 app access <workspace> [--build on|off] [--manage on|off] [--users on|off] [--core db|files|thread|mail|sew=on|off[,…]]… [--json]"
    while [ $# -gt 0 ]; do
        case "$1" in
            --build) build="${2:-}"; shift 2 ;;
            --users) users="${2:-}"; shift 2 ;;
            --manage) manage="${2:-}"; shift 2 ;;
            --open) open="${2:-}"; shift 2 ;;
            --core|--cores) cores="${cores:+$cores,}${2:-}"; shift 2 ;;
            --json) json_out=1; shift ;;
            -h|--help) _app_grace_usage; return 0 ;;
            -*) echo "Unknown flag: $1" >&2; exit 2 ;;
            *) ws="$1"; shift ;;
        esac
    done
    [ -z "$ws" ] && { echo "$_usage" >&2; exit 2; }
    # --core db=on,files=off → {"db": true, "files": false} (checked here;
    # the daemon checks the core names and who may tick).
    local cores_j=""
    if [ -n "$cores" ]; then
        cores_j="$(K2_CORES="$cores" python3 -c '
import json, os, sys
out = {}
for part in os.environ["K2_CORES"].split(","):
    part = part.strip()
    if not part:
        continue
    k, sep, v = part.partition("=")
    if not sep or v not in ("on", "off") or not k:
        sys.exit(1)
    out[k.strip()] = v == "on"
print(json.dumps(out))')" || { echo "usage: --core <core>=on|off[,<core>=on|off…] (cores: db, files, thread, mail, sew)" >&2; exit 2; }
    fi
    if [ -z "$build" ] && [ -z "$manage" ] && [ -z "$users" ] && [ -z "$cores_j" ] && [ -z "$open" ]; then
        # No change asked: show the switches and cores.
        _compute_call GET /cli/app/list "workspace=$(urlencode "$ws")"
        if [ "$json_out" = 1 ]; then
            printf '%s' "$_CP_BODY" | python3 -c 'import json, sys; print(json.dumps(json.load(sys.stdin).get("access")))'
            return 0
        fi
        printf '%s' "$_CP_BODY" | _app_access_print
        return 0
    fi
    if [ -n "$open" ]; then
        # N1 (0.46.3): the standing approval for the open path is retired.
        echo "k2: --open is retired: each open-path move (a new --cmd service, --path open, extra ports, broad egress) is held" >&2
        echo "    for the owner, who approves it in person: k2 app approvals, then k2 app approve <id> (outside K2)." >&2
        exit 2
    fi
    local t v
    for t in build manage; do
        [ "$t" = build ] && v="$build" || v="$manage"
        [ -z "$v" ] && continue
        case "$v" in on|off) ;; *) echo "usage: --$t on|off" >&2; exit 2 ;; esac
    done
    if [ -n "$manage" ] && [ -z "$build" ]; then
        # Deprecation line: before 0.46.3 --manage was switch 1.
        echo "k2: note: --manage is now switch 2 (\"…and manage them end to end\"); \"Build and run Apps\" is --build." >&2
    fi
    # Switch 1 first when turning on; switch 2 first when turning off.
    local order="build manage"
    [ "$build" = off ] && order="manage build"
    for t in $order; do
        [ "$t" = build ] && v="$build" || v="$manage"
        [ -z "$v" ] && continue
        local toggle="apps_build"
        [ "$t" = manage ] && toggle="apps_manage_full"
        _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle="$toggle" K2_CJ_value_b="$([ "$v" = on ] && echo 1 || echo 0)")"
    done
    if [ -n "$users" ]; then
        # "Let this agent manage its Apps' users" (0225): the owner, both ways.
        case "$users" in on|off) ;; *) echo "usage: --users on|off" >&2; exit 2 ;; esac
        _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle=app_users K2_CJ_value_b="$([ "$users" = on ] && echo 1 || echo 0)")"
    fi
    if [ -n "$cores_j" ]; then
        # The cores in one call (one note to the agent): the owner ticks, an
        # Admin may only untick.
        _compute_call POST /cli/agent-access/set "$(_compute_json K2_CJ_workspace="$ws" K2_CJ_toggle=app_core K2_CJ_cores_j="$cores_j")"
    fi
    if [ "$json_out" = 1 ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    # Show where the agent stands now (the read needs Apps on; without it,
    # say what changed).
    local last="$_CP_BODY" raw
    raw="$(_compute_send GET /cli/app/list "workspace=$(urlencode "$ws")")"
    _compute_split "$raw"
    if [ "$_CP_STATUS" = "200" ]; then
        printf '%s' "$_CP_BODY" | _app_access_print
    else
        printf '%s' "$last" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%s: changed (k2 app access %s shows it once Apps (beta) are on)" % (v.get("workspace"), v.get("workspace")))'
    fi
}

