# ── k2 desktop — this computer's screen, for people (0.46.11) ──────────
# prd-remote-desktop-tab-v1 §7. Thin: every verb calls /cli/desktop/*;
# the daemon decides who may do what. People only: an agent's call is
# desktop_people_only (exit 3) and "It can't be requested."
_desktop_usage() {
    cat <<'EOF'
k2 desktop — this computer's screen in a Desktop tab (Remote desktop, Beta)

People only: the owner, and the logins the owner allows. Agents can't use
this in v1: they can't see or drive the screen with K2, and can't ask to
(an agent gets desktop_people_only, exit 3, "It can't be requested.").
The one agent verb is k2 desktop env (below): a desktop of the agent's own
on a Linux server, to put windows on.

  k2 desktop [status] [--json]      on or off, the machine's state, what you may do;
                                    the owner also sees viewers, grants, hidden apps
  k2 desktop enable [--json]        turn it on (Owner); prints what that allows
  k2 desktop disable [--json]       turn it off (Owner); closes every viewer
  k2 desktop policy --hide-app <bundle id> [--hide-app …] | --no-hide-apps [--json]
                                    the apps viewers never see on macOS (Owner);
                                    replaces the list (default: Passwords,
                                    Keychain Access, 1Password, Bitwarden)
  k2 desktop grant <login> [--view] [--json]
                                    let a Member login watch (Owner). --view is the
                                    default and the only kind: Members never drive.
                                    Owners and Admins open it without a grant. A
                                    grant lasts until revoked, or until that login
                                    is removed, disabled or changes role. Refused:
                                    an Owner or Admin (grant_not_needed), a Voice &
                                    Chat only or disabled login, an unknown login
                                    (no_such_user, exit 4)
  k2 desktop revoke <login> [--json] end a grant; closes that login's viewers
  k2 desktop grants [--json]        the grants (Owner)
  k2 desktop viewers [--json]       who is watching or driving, from where (Owner)
  k2 desktop kick <viewer-id|login> [--json]
                                    close one viewer, or every viewer of a login (Owner)
  k2 desktop audit [--since 1d] [--limit 50] [--json]
                                    the access log, newest first (Owner). Who,
                                    when, from where; never what was on screen
                                    or typed
  k2 desktop open [--display <id>] [--json]
                                    open the Desktop tab in your own K2 client. Run
                                    from a terminal on this computer it succeeds,
                                    but the tab opens in this computer's K2 app and
                                    only says "You're on this computer"; run it on
                                    the device you watch from
  k2 desktop displays [--json]      the screens you can pick in the tab

On a Linux server with no screen (a virtual desktop K2 starts; never on a Mac):
  k2 desktop setup [--timeout 10m] [--no-wait] [--json]
                                    install the packages it needs (Owner). The
                                    install runs on the server; this waits for
                                    it (10 minutes at most) and prints progress.
                                    --no-wait returns at once (k2 desktop status
                                    shows setting_up until it ends)
  k2 desktop list [--json]          the desktops you may open, with their state
  k2 desktop start [--size WxH] [--json]
                                    start it, or wake it (Owners and Admins);
                                    1280x720 to 1920x1080, default 1280x720
  k2 desktop stop [--json]          close it; what was open on it is lost
  --desktop <id>                    any verb above: which desktop. virtual is
                                    This server, virtual:<workspace id> an
                                    agent's own; default This server
  k2 desktop env [--release] [--timeout 2m] [--json]
                                    AGENTS, on a Linux server: your own desktop.
                                    Starts or wakes it and prints shell lines;
                                    run  eval "$(k2 desktop env)"  then start a
                                    GUI program (agent-browser --headed, a
                                    browser with --user-data-dir
                                    "$K2_DESKTOP_PROFILE"). Each call holds it
                                    for you 30 minutes; --release when done.
                                    Needs your workspace's "Allow a virtual
                                    desktop" switch (exit 3 + Ask with: if off)
  k2 desktop allow [on|off] [--workspace <ws>] [--json]
                                    "Allow a virtual desktop" for one agent
                                    (Owner or Admin; default: this workspace).
                                    No on/off: show it. On lets that agent get
                                    a desktop of its own with k2 desktop env

(Owner) means an Owner login, or the owner at this computer: a plain
terminal or a K2 shell tab here. The owner at this computer can run every
verb above; they just never watch from here. You watch from another device, over K2 Connect or the LAN, signed in
with a login: Owner and Admin logins watch and drive; a Member login only
with a grant, and only to watch; Voice & Chat only never. No one at the
screen has to click Allow. It stays on until an Owner turns it off. Also:
Settings → Remote Desktop, and + → Desktop (Beta) in a tab strip. More:
k2 study desktop.
EOF
}

_desktop_render() {
    # _desktop_render <as_json> <verb> <raw "body\ncode">
    local as_json="$1" verb="$2" raw="$3" code body
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 desktop: daemon unreachable" >&2
        exit 1
    fi
    if [ "$code" != "200" ]; then
        K2_DT_BODY="$body" K2_DT_CODE="$code" python3 -c '
import json, os, sys
raw, code = os.environ["K2_DT_BODY"], os.environ["K2_DT_CODE"]
try:
    d = json.loads(raw)
except Exception:
    print("k2 desktop: error: HTTP " + code + ": " + raw.strip()[:300], file=sys.stderr); sys.exit(0)
e = d.get("error")
c = ""
if isinstance(e, dict):
    c = e.get("code") or ""
    print("k2 desktop: error: %s: %s" % (c, e.get("hint") or ""), file=sys.stderr)
elif e is not None:
    c = str(e)
    print("k2 desktop: error: %s" % e, file=sys.stderr)
else:
    print("k2 desktop: error: HTTP " + code, file=sys.stderr)
if d.get("closable"):
    print("desktops you could close: " + ", ".join(d["closable"]), file=sys.stderr)
p = d.get("permission")
if isinstance(p, dict) and p.get("command"):
    print("Ask with: " + p["command"], file=sys.stderr)
elif c == "desktop_people_only" or (isinstance(p, dict) and p.get("requestable") is False):
    print("It can\x27t be requested.", file=sys.stderr)
'
        [ "$as_json" = "1" ] && printf '%s\n' "$body"
        exit "$(_sidecar_exit_for "$code")"
    fi
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$body"
        return 0
    fi
    K2_DT_BODY="$body" K2_DT_VERB="$verb" python3 -c '
import datetime, json, os
d = json.loads(os.environ["K2_DT_BODY"])
verb = os.environ["K2_DT_VERB"]
def when(ms):
    try:
        return datetime.datetime.fromtimestamp(ms / 1000).strftime("%Y-%m-%d %H:%M")
    except Exception:
        return "-"
def at(s):
    try:
        return datetime.datetime.fromtimestamp(s).strftime("%Y-%m-%d %H:%M")
    except Exception:
        return "-"
def virtual_lines(d):
    v = d.get("virtual")
    if not isinstance(v, dict):
        return
    sz = v.get("size")
    print("size           : %s" % ("%sx%s" % (sz["width"], sz["height"]) if sz else "-"))
    if v.get("memoryBytes") is not None:
        print("memory held    : %d MB" % (v["memoryBytes"] // (1024 * 1024)))
    pr = v.get("present") or []
    print("viewing now    : %s" % (", ".join(p["user"] for p in pr) or "nobody"))
    h = v.get("hold")
    if h:
        print("agent hold     : %s until %s" % (h.get("by"), at(h.get("until", 0))))
    if v.get("nextSleepAt"):
        print("sleeps at      : %s" % at(v["nextSleepAt"]))
    if v.get("nextCloseAt"):
        print("closes at      : %s (a day asleep)" % at(v["nextCloseAt"]))
    if not v.get("canSleep"):
        print("sleep          : this server can\x27t pause a desktop, so it closes when nobody is viewing it")
    w = v.get("waiting")
    if w:
        print("waiting        : place %s; this server runs %s awake at a time" % (w.get("position"), w.get("awakeLimit")))
    lim = v.get("limits") or {}
    if lim:
        print("limits         : %s/%s desktops, %s/%s awake, %d MB for all (%d MB each), %s cores" % (
            lim["desktops"]["used"], lim["desktops"]["max"], lim["awake"]["used"], lim["awake"]["max"],
            lim["memoryBytes"]["max"] // (1024 * 1024), lim["memoryBytes"]["each"] // (1024 * 1024), lim["cpuCores"]))
    if v.get("label"):
        print("note           : %s (until agent isolation, any agent on this server could reach it)" % v["label"])
    if v.get("manual"):
        print("to set up      : k2 desktop setup, or by hand: %s" % v["manual"])
    if v.get("suggestRecycle"):
        print("tip            : it has been up for a week; k2 desktop stop and start frees what it leaked")
if verb in ("status", "start"):
    y = d.get("you") or {}
    print("remote desktop : %s on %s" % ("on" if d.get("enabled") else "off", d.get("machine") or "this computer"))
    print("desktop        : %s (%s%s)" % (d.get("desktop"), d.get("name") or "This server", ", an agent" if d.get("kind") == "agent" else ""))
    print("state          : %s (%s)" % (d.get("state"), d.get("message") or ""))
    print("backend        : %s, lane %s" % (d.get("backend"), d.get("lane")))
    virtual_lines(d)
    if y.get("sameMachine"):
        you = "you are on this computer: you can manage it, but you watch it from another device"
    elif y.get("mayControl"):
        you = "you may watch and drive"
    elif y.get("mayView"):
        you = "you may watch (view only)"
    else:
        you = "you have no access to this screen"
    print("you            : %s (%s)%s%s" % (you, y.get("role"), "; you manage it" if y.get("mayManage") and not y.get("sameMachine") else "", "; you may start and close it" if y.get("mayStart") and d.get("virtual") is not None else ""))
    if "hideApps" in d:
        print("hidden apps    : %s" % (", ".join(d["hideApps"]) or "none"))
    if "grants" in d:
        g = d["grants"]
        print("grants         : %s" % (", ".join("%s (%s)" % (x["user"], x["mode"]) for x in g) or "none"))
    if "viewers" in d:
        v = d["viewers"]
        print("viewers        : %s" % (", ".join("%s %s (%s)" % (x["user"], x["mode"], x["ingress"]) for x in v) or "none"))
elif verb in ("enable", "disable"):
    if d.get("enabled"):
        print("Remote desktop is on%s. State: %s (%s)" % ("" if d.get("changed") else " (it already was)", d.get("state"), d.get("message") or ""))
        if d.get("card"):
            print()
            print(d["card"])
    else:
        print("Remote desktop is off%s; closed %s viewer(s)." % ("" if d.get("changed") else " (it already was)", d.get("closed", 0)))
elif verb == "policy":
    print("hidden apps: %s" % (", ".join(d.get("hideApps") or []) or "none"))
elif verb == "grant":
    g = d.get("grant") or {}
    print("%s may watch (%s) until you revoke it: k2 desktop revoke %s" % (g.get("user"), g.get("mode"), g.get("user")))
elif verb == "revoke":
    print("%s; closed %s viewer(s)." % ("revoked" if d.get("revoked") else "there was no grant", d.get("closed", 0)))
elif verb == "grants":
    g = d.get("grants") or []
    if not g:
        print("no grants (Owners and Admins open it without one; k2 desktop grant <login> lets a Member watch)")
    for x in g:
        print("%-20s %-8s by %s" % (x["user"], x["mode"], x.get("grantedBy")))
elif verb == "viewers":
    v = d.get("viewers") or []
    if not v:
        print("nobody is watching")
    for x in v:
        print("%-14s %-16s %-8s %-8s %s since %s" % (x["id"], x["user"], x["mode"], x["ingress"], x.get("ip") or "-", when(x.get("since", 0))))
elif verb == "kick":
    print("closed %s viewer(s)" % d.get("kicked", 0))
elif verb == "audit":
    lines = d.get("lines") or []
    if not lines:
        print("no access log lines")
    for l in lines:
        extra = " ".join("%s=%s" % (k, l[k]) for k in ("target", "mode", "display", "viewer", "ip") if l.get(k))
        print("%s  %-20s %-18s %-8s %s %s" % (l.get("ts", "-"), l.get("event"), l.get("who", "-"), l.get("ingress", "-"), l.get("outcome", ""), extra))
elif verb == "open":
    print("asked your K2 client to open the Desktop tab (for %s)" % d.get("for"))
elif verb == "list":
    ds = d.get("desktops") or []
    if not ds:
        print("no desktops you may open (the owner grants one: k2 desktop grant <login> --desktop <id>)")
    for x in ds:
        print("%-46s %-10s %-20s %s" % (x["id"], x.get("state", "-"), x.get("name", "-"), x.get("label") or ""))
elif verb == "stop":
    print("%s %s" % ("Closed." if d.get("closed") else "It wasn\x27t running.", d.get("message") or ""))
elif verb == "setup":
    print(d.get("message") or ("Installed. k2 desktop start starts it." if d.get("ok") else "Installed, but still missing: %s" % ", ".join(d.get("missing") or [])))
elif verb == "displays":
    ds = d.get("displays") or []
    if not ds:
        print("no displays yet (k2 desktop status says why)")
    for x in ds:
        print("%-8s %-20s %sx%s pt%s" % (x["id"], x["name"], x["widthPt"], x["heightPt"], "  main" if x.get("main") else ""))
elif verb == "allow":
    # agent-access/set answers {workspace, agentsCanUseDesktop}; status
    # answers the agent desktop with agentsAllowed.
    if "agentsCanUseDesktop" in d:
        on, who = d["agentsCanUseDesktop"], d.get("workspace")
    else:
        on, who = d.get("agentsAllowed"), d.get("name") or d.get("workspaceId")
    print("Allow a virtual desktop is %s for %s." % ("on" if on else "off", who))
    if on:
        print("That agent can get a desktop of its own with k2 desktop env (separate, not private).")
'
}

# _desktop_setup_wait <as_json> <no_wait> <timeout> <raw "body\ncode">: the
# install runs on the server in the background (contract note 11-11):
# setup answers setting_up at once; this polls status until the state is
# closed (installed) or not_set_up (with why), printing progress on stderr.
_desktop_setup_wait() {
    local as_json="$1" no_wait="$2" wait_for="$3" raw="$4" code body secs=600 waited=0 state st
    code="${raw##*$'\n'}"
    body="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" != "200" ]; then
        _desktop_render "$as_json" setup "$raw"
        return 0
    fi
    state="$(printf '%s' "$body" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state") or "")' 2>/dev/null || true)"
    if [ "$state" != "setting_up" ] || [ "$no_wait" = 1 ]; then
        if [ "$state" = "setting_up" ] && [ "$as_json" != 1 ]; then
            echo "Installing the desktop packages on the server. k2 desktop status says setting_up until it ends."
            return 0
        fi
        _desktop_render "$as_json" setup "$raw"
        return 0
    fi
    if [ -n "$wait_for" ]; then
        case "$wait_for" in
            *[!0-9smh]*|"") echo "usage: --timeout 10m|90s|600" >&2; exit 2 ;;
            *h) secs=$(( ${wait_for%h} * 3600 )) ;;
            *m) secs=$(( ${wait_for%m} * 60 )) ;;
            *s) secs="${wait_for%s}" ;;
            *) secs="$wait_for" ;;
        esac
        case "$secs" in ""|*[!0-9]*) echo "usage: --timeout 10m|90s|600" >&2; exit 2 ;; esac
    fi
    [ "$as_json" = 1 ] || echo "Installing the desktop packages on the server (this can take a few minutes)…" >&2
    while :; do
        sleep 2
        waited=$((waited + 2))
        st="$(_sidecar_send GET /cli/desktop/status "desktop=virtual")"
        code="${st##*$'\n'}"
        body="${st%$'\n'*}"
        if [ -z "$st" ] || [ "$code" = "$st" ] || [ "$code" = "000" ]; then
            echo "k2 desktop: daemon unreachable while waiting for the install (k2 desktop status shows how it ends)" >&2
            exit 1
        fi
        [ "$code" = "200" ] || _desktop_render "$as_json" status "$st"
        state="$(printf '%s' "$body" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state") or "")')"
        [ "$state" = "setting_up" ] || break
        if [ "$waited" -ge "$secs" ]; then
            echo "k2 desktop: still installing after ${waited} s. It goes on on the server: k2 desktop status says setting_up until it ends." >&2
            exit 1
        fi
        if [ "$as_json" != 1 ] && [ $((waited % 10)) -eq 0 ]; then
            echo "  still installing… (${waited} s)" >&2
        fi
    done
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$body"
        [ "$state" = "not_set_up" ] && exit 1
        return 0
    fi
    K2_DT_BODY="$body" python3 -c '
import json, os, sys
d = json.loads(os.environ["K2_DT_BODY"])
if d.get("state") == "not_set_up":
    print("k2 desktop: error: setup_failed: %s" % (d.get("message") or "the packages did not install"), file=sys.stderr)
    sys.exit(1)
print("Installed. k2 desktop start starts it.")
' || exit 1
}

# _desktop_env <as_json> <release> <timeout> <desktop or empty>: POST
# /cli/desktop/env. Prints shell lines for eval (export DISPLAY=…), a
# comment line with the state and the hold; waits (another call every 2 s,
# which also renews the hold) while the desktop is waiting, starting or
# waking, up to --timeout (2 minutes).
_desktop_env() {
    local as_json="$1" release="$2" wait_for="$3" desk="$4" body raw code out state secs=120 waited=0
    if [ "$release" = 1 ]; then
        body='{"release": true}'
    else
        body='{}'
    fi
    body="$(_desktop_body "$body" "$desk")"
    if [ -n "$wait_for" ]; then
        case "$wait_for" in
            *[!0-9smh]*|"") echo "usage: --timeout 2m|90s|120" >&2; exit 2 ;;
            *h) secs=$(( ${wait_for%h} * 3600 )) ;;
            *m) secs=$(( ${wait_for%m} * 60 )) ;;
            *s) secs="${wait_for%s}" ;;
            *) secs="$wait_for" ;;
        esac
        case "$secs" in ""|*[!0-9]*) echo "usage: --timeout 2m|90s|120" >&2; exit 2 ;; esac
    fi
    while :; do
        raw="$(_sidecar_send POST /cli/desktop/env "$body")"
        code="${raw##*$'\n'}"
        out="${raw%$'\n'*}"
        [ "$code" = "200" ] || _desktop_render "$as_json" env "$raw"
        [ "$release" = 1 ] && break
        state="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("state") or "")')"
        case "$state" in waiting|starting|waking) ;; *) break ;; esac
        if [ "$waited" -ge "$secs" ]; then
            echo "k2 desktop: your desktop is still $state after ${waited} s; run k2 desktop env again later" >&2
            [ "$as_json" = 1 ] && printf '%s\n' "$out"
            exit 1
        fi
        [ "$as_json" = 1 ] || echo "# $state: $(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("message") or "")')" >&2
        sleep 2
        waited=$((waited + 2))
    done
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    K2_DT_BODY="$out" python3 -c '
import datetime, json, os, shlex
d = json.loads(os.environ["K2_DT_BODY"])
if "released" in d:
    print("# %s: %s" % (d.get("desktop"), "released: it can sleep when nobody is watching" if d.get("released") else "you held nothing"))
    raise SystemExit(0)
env = d.get("env") or {}
for k in ("DISPLAY", "XAUTHORITY", "K2_DESKTOP_PROFILE"):
    if k in env:
        print("export %s=%s" % (k, shlex.quote(env[k])))
h = d.get("hold") or {}
try:
    until = datetime.datetime.fromtimestamp(h.get("until", 0)).strftime("%H:%M")
except Exception:
    until = "?"
print("# %s (%s): %s Yours until %s: run k2 desktop env again to keep it, k2 desktop env --release when done. %s: people can watch and use it in a Desktop tab." % (
    d.get("desktop"), d.get("name") or "", d.get("message") or d.get("state"), until, d.get("label") or "Separate, not private"))
' || exit 1
}

# _desktop_body <json object> <desktop id or empty>: the body with "desktop".
_desktop_body() {
    if [ -z "${2:-}" ]; then
        printf '%s' "$1"
        return 0
    fi
    python3 -c 'import json,sys; d=json.loads(sys.argv[1]); d["desktop"]=sys.argv[2]; print(json.dumps(d))' "$1" "$2"
}

cmd_desktop() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    case "$sub" in
        help|--help|-h) _desktop_usage; return 0 ;;
        --json) sub=status; set -- --json "$@" ;;
    esac
    local as_json=0 since="" limit="" display="" no_hide=0 raw body mode=view qs desk="" size="" wait_for="" no_wait=0 ws="" release=0
    local args=() hides=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --desktop) desk="${2:-}"; shift 2 || { echo "usage: --desktop <id> (virtual, virtual:<workspace id>, console; k2 desktop list)" >&2; exit 2; } ;;
            --desktop=*) desk="${1#--desktop=}"; shift ;;
            --size) size="${2:-}"; shift 2 || { echo "usage: --size WxH (1280x720 to 1920x1080)" >&2; exit 2; } ;;
            --size=*) size="${1#--size=}"; shift ;;
            --timeout) wait_for="${2:-}"; shift 2 || { echo "usage: --timeout 10m|90s|600 (how long k2 desktop setup waits)" >&2; exit 2; } ;;
            --timeout=*) wait_for="${1#--timeout=}"; shift ;;
            --no-wait) no_wait=1; shift ;;
            --release) release=1; shift ;;
            --workspace) ws="${2:-}"; shift 2 || { echo "usage: --workspace <path|name> (k2 desktop allow)" >&2; exit 2; } ;;
            --workspace=*) ws="${1#--workspace=}"; shift ;;
            --since) since="${2:-}"; shift 2 || { echo "usage: --since 1d|2h|30m|<RFC 3339 time>" >&2; exit 2; } ;;
            --since=*) since="${1#--since=}"; shift ;;
            --limit) limit="${2:-}"; shift 2 || { echo "usage: --limit N" >&2; exit 2; } ;;
            --limit=*) limit="${1#--limit=}"; shift ;;
            --display) display="${2:-}"; shift 2 || { echo "usage: --display <id> (k2 desktop displays)" >&2; exit 2; } ;;
            --display=*) display="${1#--display=}"; shift ;;
            --hide-app) hides+=("${2:-}"); shift 2 || { echo "usage: --hide-app <bundle id>" >&2; exit 2; } ;;
            --hide-app=*) hides+=("${1#--hide-app=}"); shift ;;
            --no-hide-apps) no_hide=1; shift ;;
            --view) mode=view; shift ;;
            --control)
                echo "usage: grants are view only: Members never drive (Owners and Admins drive without a grant). Use --view." >&2
                exit 2 ;;
            --ttl|--ttl=*)
                echo "usage: grants have no length: they last until k2 desktop revoke <login>" >&2
                exit 2 ;;
            --consent|--consent=*|--active-window|--active-window=*)
                echo "usage: there is no consent policy in this version (no Allow click at the screen); k2 desktop policy takes --hide-app" >&2
                exit 2 ;;
            -h|--help) _desktop_usage; return 0 ;;
            -*) echo "usage: unknown flag '$1' (k2 desktop --help)" >&2; exit 2 ;;
            *) args+=("$1"); shift ;;
        esac
    done
    # --desktop: the query on GET, a body field on POST (_desktop_body).
    local dq=""
    [ -n "$desk" ] && dq="desktop=$(urlencode "$desk")"
    if [ -n "$size" ] && [ "$sub" != "start" ]; then
        echo "usage: --size goes with k2 desktop start" >&2; exit 2
    fi
    if [ "$no_wait" = 1 ] && [ "$sub" != "setup" ]; then
        echo "usage: --no-wait goes with k2 desktop setup" >&2; exit 2
    fi
    if [ -n "$wait_for" ] && [ "$sub" != "setup" ] && [ "$sub" != "env" ]; then
        echo "usage: --timeout goes with k2 desktop setup or k2 desktop env" >&2; exit 2
    fi
    if [ "$release" = 1 ] && [ "$sub" != "env" ]; then
        echo "usage: --release goes with k2 desktop env" >&2; exit 2
    fi
    if [ -n "$ws" ] && [ "$sub" != "allow" ]; then
        echo "usage: --workspace goes with k2 desktop allow" >&2; exit 2
    fi
    case "$sub" in
        status)
            raw="$(_sidecar_send GET /cli/desktop/status "$dq")" ;;
        list)
            raw="$(_sidecar_send GET /cli/desktop/list "")" ;;
        start)
            body="{}"
            if [ -n "$size" ]; then
                [[ "$size" =~ ^[0-9]+[xX][0-9]+$ ]] || { echo "usage: --size WxH, e.g. 1600x900 (1280x720 to 1920x1080)" >&2; exit 2; }
                body="$(python3 -c 'import json,sys; w,h=sys.argv[1].lower().split("x"); print(json.dumps({"width": int(w), "height": int(h)}))' "$size")"
            fi
            raw="$(_sidecar_send POST /cli/desktop/start "$(_desktop_body "$body" "$desk")")" ;;
        stop|close)
            sub=stop
            raw="$(_sidecar_send POST /cli/desktop/stop "$(_desktop_body "{}" "$desk")")" ;;
        setup)
            raw="$(_sidecar_send POST /cli/desktop/setup "{}")"
            _desktop_setup_wait "$as_json" "$no_wait" "$wait_for" "$raw"
            return 0 ;;
        enable|on)
            sub=enable
            raw="$(_sidecar_send POST /cli/desktop/enable "$(_desktop_body "{}" "$desk")")" ;;
        disable|off)
            sub=disable
            raw="$(_sidecar_send POST /cli/desktop/disable "$(_desktop_body "{}" "$desk")")" ;;
        policy)
            if [ "$no_hide" = 1 ] && [ "${#hides[@]}" -gt 0 ]; then
                echo "usage: --no-hide-apps or --hide-app, not both" >&2; exit 2
            fi
            if [ "$no_hide" = 0 ] && [ "${#hides[@]}" -eq 0 ]; then
                echo "usage: k2 desktop policy --hide-app <bundle id> [--hide-app …] | --no-hide-apps" >&2; exit 2
            fi
            body="$(python3 -c 'import json,sys; print(json.dumps({"hideApps": sys.argv[1:]}))' ${hides[@]+"${hides[@]}"})"
            raw="$(_sidecar_send POST /cli/desktop/policy "$body")" ;;
        grant)
            [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 desktop grant <login> [--view]" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; print(json.dumps({"user": sys.argv[1], "mode": sys.argv[2]}))' "${args[0]}" "$mode")"
            raw="$(_sidecar_send POST /cli/desktop/grant "$(_desktop_body "$body" "$desk")")" ;;
        revoke)
            [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 desktop revoke <login>" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; print(json.dumps({"user": sys.argv[1]}))' "${args[0]}")"
            raw="$(_sidecar_send POST /cli/desktop/revoke "$(_desktop_body "$body" "$desk")")" ;;
        grants)
            raw="$(_sidecar_send GET /cli/desktop/grants "$dq")" ;;
        viewers)
            raw="$(_sidecar_send GET /cli/desktop/viewers "$dq")" ;;
        kick)
            [ "${#args[@]}" -eq 1 ] || { echo "usage: k2 desktop kick <viewer-id|login> (k2 desktop viewers lists them)" >&2; exit 2; }
            case "${args[0]}" in
                dv_*) body="$(python3 -c 'import json,sys; print(json.dumps({"viewer": sys.argv[1]}))' "${args[0]}")" ;;
                *) body="$(python3 -c 'import json,sys; print(json.dumps({"user": sys.argv[1]}))' "${args[0]}")" ;;
            esac
            raw="$(_sidecar_send POST /cli/desktop/kick "$(_desktop_body "$body" "$desk")")" ;;
        audit)
            if [ -n "$limit" ] && ! [[ "$limit" =~ ^[0-9]+$ ]]; then echo "usage: --limit N" >&2; exit 2; fi
            qs="limit=${limit:-50}"
            [ -n "$since" ] && qs="${qs}&since=$(urlencode "$since")"
            raw="$(_sidecar_send GET /cli/desktop/audit "$qs")" ;;
        open)
            if [ -n "$display" ]; then
                body="$(python3 -c 'import json,sys; print(json.dumps({"display": sys.argv[1]}))' "$display")"
            else
                body="{}"
            fi
            raw="$(_sidecar_send POST /cli/desktop/open "$(_desktop_body "$body" "$desk")")" ;;
        displays)
            raw="$(_sidecar_send GET /cli/desktop/displays "$dq")" ;;
        env)
            # An agent's own desktop (L8): the passport picks it.
            [ "${#args[@]}" -eq 0 ] || { echo "usage: k2 desktop env [--release] [--timeout 2m] [--json]" >&2; exit 2; }
            _desktop_env "$as_json" "$release" "$wait_for" "$desk"
            return 0 ;;
        allow)
            # "Allow a virtual desktop" for one agent (LD-38): the Owner or
            # an Admin, like the Settings row. Default: this workspace.
            [ -n "$desk" ] && { echo "usage: k2 desktop allow [on|off] [--workspace <ws>] (no --desktop)" >&2; exit 2; }
            [ "${#args[@]}" -le 1 ] || { echo "usage: k2 desktop allow [on|off] [--workspace <ws>]" >&2; exit 2; }
            [ -z "$ws" ] && ws="$PROJECT"
            case "${args[0]:-}" in
                on|off)
                    body="$(python3 -c 'import json,sys; print(json.dumps({"workspace": sys.argv[1], "toggle": "desktop", "value": sys.argv[2] == "on"}))' "$ws" "${args[0]}")"
                    raw="$(_sidecar_send POST /cli/agent-access/set "$body")" ;;
                ""|status)
                    raw="$(_sidecar_send GET /cli/desktop/status "workspace=$(urlencode "$ws")")" ;;
                *) echo "usage: k2 desktop allow [on|off] [--workspace <ws>]" >&2; exit 2 ;;
            esac ;;
        *)
            echo "usage: unknown desktop verb '$sub' (status|enable|disable|policy|grant|revoke|grants|viewers|kick|audit|open|displays|list|start|stop|setup|allow)" >&2
            exit 2 ;;
    esac
    _desktop_render "$as_json" "$sub" "$raw"
}

# k2 front passthrough (0.46.6): names the owner's own web server answers,
# passed through by SNI from the :443 front to a loopback port.
_front_passthrough() {
    local verb="${1:-list}" as_json=0 to="" proxy=0 label="" host="" raw body
    case "$verb" in
        -*) verb=list ;;   # `k2 front passthrough --json`
        *) [ $# -gt 0 ] && shift ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --to) to="${2:-}"; shift 2 || { echo "usage: --to 127.0.0.1:<port>" >&2; exit 2; } ;;
            --to=*) to="${1#--to=}"; shift ;;
            --proxy-v2) proxy=1; shift ;;
            --label) label="${2:-}"; shift 2 || { echo "usage: --label <why>" >&2; exit 2; } ;;
            -h|--help) _front_usage; return 0 ;;
            -*) echo "usage: unknown flag '$1' (k2 front --help)" >&2; exit 2 ;;
            *) host="$1"; shift ;;
        esac
    done
    case "$verb" in
        list|ls) raw="$(_sidecar_send GET /cli/front/passthrough "")" ;;
        add)
            { [ -n "$host" ] && [ -n "$to" ]; } || { echo "usage: k2 front passthrough add <host>|'*' --to 127.0.0.1:<port> [--proxy-v2] [--label <why>]" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; print(json.dumps({"host": sys.argv[1], "to": sys.argv[2], "proxyV2": sys.argv[3] == "1", "label": sys.argv[4]}))' "$host" "$to" "$proxy" "$label")"
            raw="$(_sidecar_send POST /cli/front/passthrough/add "$body")" ;;
        rm|remove)
            [ -n "$host" ] || { echo "usage: k2 front passthrough rm <host>|'*'" >&2; exit 2; }
            body="$(python3 -c 'import json,sys; print(json.dumps({"host": sys.argv[1]}))' "$host")"
            raw="$(_sidecar_send POST /cli/front/passthrough/rm "$body")" ;;
        *) echo "usage: k2 front passthrough [list|add|rm]" >&2; exit 2 ;;
    esac
    local code="${raw##*$'\n'}" out="${raw%$'\n'*}"
    if [ -z "$raw" ] || [ "$code" = "$raw" ] || [ "$code" = "000" ]; then
        echo "k2 front: daemon unreachable" >&2; exit 1
    fi
    if [ "$as_json" = 1 ]; then printf '%s\n' "$out"; [ "$code" = 200 ] || exit "$(_sidecar_exit_for "$code")"; return 0; fi
    if [ "$code" != 200 ]; then
        _firewall_render 0 status "$raw"
    fi
    K2_FP_BODY="$out" python3 -c '
import json, os
d = json.loads(os.environ["K2_FP_BODY"])
rows = d.get("passthrough") or []
if not rows:
    print("no passthrough names (k2 front passthrough add <host> --to 127.0.0.1:<port>)")
for r in rows:
    print("%-32s -> %s%s%s" % (r.get("host"), r.get("to"), "  PROXY v2" if r.get("proxyV2") else "", ("  (" + r["label"] + ")") if r.get("label") else ""))
for w in d.get("warnings") or []:
    print("warning: " + w)
if d.get("next") and d.get("next") != "routed now":
    print("note: " + d["next"])'
}

# k2 front http on|off (0.46.8, custom domains PB-8): the :80 front.
_front_http() {
    local mode="" as_json=0 raw body
    while [ $# -gt 0 ]; do
        case "$1" in
            on|off) mode="$1"; shift ;;
            --json) as_json=1; shift ;;
            -h|--help) _front_usage; return 0 ;;
            *) echo "usage: k2 front http on|off [--json]" >&2; exit 2 ;;
        esac
    done
    [ -n "$mode" ] || { echo "usage: k2 front http on|off [--json]" >&2; exit 2; }
    body='{"on":false}'
    [ "$mode" = on ] && body='{"on":true}'
    raw="$(_sidecar_send POST /cli/front/http "$body")" || true
    _front_render "$as_json" "$raw"
}

cmd_front() {
    local sub="${1:-status}"
    [ $# -gt 0 ] && shift
    local as_json=0 mail=0 raw
    case "$sub" in
        help|--help|-h) _front_usage; return 0 ;;
        passthrough) _front_passthrough "$@"; return ;;
        http) _front_http "$@"; return ;;
    esac
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --mail) mail=1; shift ;;
            -h|--help) _front_usage; return 0 ;;
            *) echo "usage: unknown argument '$1' (k2 front --help)" >&2; exit 2 ;;
        esac
    done
    case "$sub" in
        status)
            raw="$(_sidecar_send GET /cli/front/status "")" || true
            _front_render "$as_json" "$raw"
            ;;
        enable)
            local body='{"mail":false}'
            [ "$mail" = 1 ] && body='{"mail":true}'
            raw="$(_sidecar_send POST /cli/front/enable "$body")" || true
            _front_render "$as_json" "$raw"
            ;;
        disable)
            [ "$mail" = 1 ] && { echo "usage: --mail is for enable (disable takes everything back)" >&2; exit 2; }
            raw="$(_sidecar_send POST /cli/front/disable "{}")" || true
            _front_render "$as_json" "$raw"
            ;;
        *)
            echo "usage: unknown front verb '$sub' (status|enable|disable)" >&2
            exit 2
            ;;
    esac
}

