# ── k2 sim — the workspace's mobile device (prd-mobile-dev-v1 §4, S2) ──
# The agent's hands and eyes on the device the Mobile tab shows. The daemon
# owns the sim session (one per workspace), the tree (k2-core::sim::tree: one
# format for every lane, ids tied to a snapshot) and the action diffs; this
# family only sends verbs and prints what the daemon renders (`text`,
# `line`). Routes and bodies: k2-core::sim::api. Inside a K2
# terminal the session passport is sent (the daemon keeps you to your own
# workspace and checks the "Allow mobile devices" switch); outside K2 the
# owner token picks a workspace with --workspace (default: this one).
#
# Exit: 0 ok · 1 daemon unreachable · 2 bad usage, or held (a person is
# using the device: `k2 sim wait`, then try again) · 3 the daemon refused
# (switch off → "Ask with: …", stale ids, no device, …).

_sim_usage() {
    cat <<'EOF'
k2 sim — drive this workspace's mobile device (the Mobile tab's device)

Read first, act second, read the diff:
  k2 sim tree [--all] [--find <text>] [--boxes]   what is on screen: ids (e5) tied
                                              to a snapshot (t7); interactive only
                                              by default
  k2 sim tap e5 | tap --at 120,300            each action prints one line:
  k2 sim type [--into e3] "text"                ok tap e5 · 140ms · +e9 text "Saved"
  k2 sim type [--into e3] --secret <vault-name>   types a vault secret; you never see it
  k2 sim swipe up|down|left|right [--from e7]
  k2 sim key enter|backspace|delete|tab|escape|arrow-up|arrow-down|arrow-left|arrow-right
  k2 sim button home|back|lock|volume-up|volume-down|app-switch|dev-menu
                                              (back: Android)
  k2 sim rotate left|right
  k2 sim open-url <url> | location <lat,lon> | appearance dark|light
  k2 sim wait [--for "text"] [--timeout 10s]  until the text is on screen (or the
                                              device is free again)

The device:
  k2 sim status                               lanes this server can run, the device
  k2 sim list-devices [--platform android|ios]  device profiles you can boot
  k2 sim boot [--platform android|ios] [--device "Pixel 9"] [--no-tab]
                                              (opens the Mobile tab unless --no-tab)
  k2 sim stop | erase [--yes]                 erase wipes the device's data
  k2 sim install <app.apk|App.app>            build in your own terminal first
  k2 sim launch <package|bundleId> | terminate <id>

Eyes and ears:
  k2 sim screenshot [--region e7]             saves a PNG, prints its path
  k2 sim logs [--app <id>] [--since 2m] [--grep <text>] [--lines N]
                                              bounded and redacted

Every command takes --workspace <path|name> (outside K2; agents: only their
own) and --json (the daemon's JSON as is).
A person's tap holds the device for 3 s (Take over: until they hand it
back); an action then exits 2 "held". Run `k2 sim wait`, then `k2 sim tree`.

Exit: 0 ok · 1 daemon unreachable · 2 usage, or held · 3 refused (the
"Allow mobile devices" switch is off, a stale id, no device running, …).
See `k2 study sim`.
EOF
}

_SIM_JSON=0
_SIM_WS=""

# _sim_target [<--workspace value>] → _SIM_WS (empty = the passport's own).
_sim_target() {
    local ws="${1:-}" resolved=""
    _SIM_WS=""
    if [ -n "$ws" ]; then
        if [ -d "$ws" ]; then
            _SIM_WS="$(cd "$ws" && pwd)"
            return 0
        fi
        resolved="$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$ws")" 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read() or "{}")
except Exception:
    d = {}
print((d.get("path") or d.get("projectPath") or "") if isinstance(d, dict) else "")
' 2>/dev/null || true)"
        _SIM_WS="${resolved:-$ws}"
        return 0
    fi
    if ! _changes_in_cell; then
        _SIM_WS="$PROJECT"
    fi
}

# _sim_die CODE HINT [RC] → a CLI-side refusal, the daemon's error shape.
_sim_die() {
    local code="$1" hint="$2" rc="${3:-2}"
    [ "${_SIM_JSON:-0}" = "1" ] || echo "Error: $hint" >&2
    python3 -c 'import json,sys; print(json.dumps({"error": {"code": sys.argv[1], "hint": sys.argv[2]}}), file=sys.stderr)' "$code" "$hint"
    exit "$rc"
}

# _sim_body key=value … → one JSON object. Prefixes: s: string (skipped when
# empty), b: bool (1/0, skipped when 0), n: number, j: raw JSON. A dotted key
# nests (`s:action.verb=tap` → {"action":{"verb":"tap"}}). The workspace is
# added when _SIM_WS is set.
_sim_body() {
    python3 - "$_SIM_WS" "$@" <<'PY'
import json, sys
ws, args = sys.argv[1], sys.argv[2:]
d = {}
if ws:
    d["workspace"] = ws
def put(key, v):
    t = d
    *path, last = key.split(".")
    for k in path:
        t = t.setdefault(k, {})
    t[last] = v
for a in args:
    kind, _, kv = a.partition(":")
    key, _, val = kv.partition("=")
    if kind == "s":
        if val != "":
            put(key, val)
    elif kind == "b":
        if val == "1":
            put(key, True)
    elif kind == "n":
        put(key, float(val) if "." in val else int(val))
    elif kind == "j":
        put(key, json.loads(val))
    else:
        raise SystemExit("bad _sim_body arg: " + a)
print(json.dumps(d))
PY
}

# _sim_query key=value … → urlencoded GET params (+ workspace). Empty skipped.
_sim_query() {
    local out="" kv k v
    [ -n "$_SIM_WS" ] && out="workspace=$(urlencode "$_SIM_WS")"
    for kv in "$@"; do
        k="${kv%%=*}"
        v="${kv#*=}"
        [ -n "$v" ] || continue
        out="${out:+$out&}${k}=$(urlencode "$v")"
    done
    printf '%s' "$out"
}

# _sim_call METHOD ROUTE PAYLOAD → _SIM_STATUS/_SIM_BODY; exits on non-2xx
# with the documented code (400 → 2, 409 held → 2, other 4xx → 3, else 1).
_sim_call() {
    local raw rc
    raw="$(cli_send_json "$1" "$2" "${3:-}")"
    _SIM_STATUS="${raw##*$'\n'}"
    _SIM_BODY="${raw%$'\n'*}"
    [ "$_SIM_STATUS" = "$raw" ] && _SIM_BODY=""
    [ -z "$_SIM_STATUS" ] && _SIM_STATUS="000"
    case "$_SIM_STATUS" in 2??) return 0 ;; esac
    rc=0
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
status, as_json = sys.argv[1], sys.argv[2] == "1"
raw = sys.stdin.read()
by_status = {"400": "bad_request", "401": "unauthorized", "403": "forbidden", "404": "not_found",
             "405": "method_not_allowed", "409": "conflict", "000": "daemon_unreachable"}
code = by_status.get(status, "error")
hint = ""
try:
    v = json.loads(raw)
except Exception:
    v = None
if isinstance(v, dict):
    e = v.get("error")
    if isinstance(e, dict):
        code = e.get("code") or code
        hint = e.get("hint") or e.get("message") or ""
    else:
        hint = v.get("message") or (str(e) if e is not None else "")
        code = v.get("code") or (e if isinstance(e, str) and " " not in e else code)
elif raw.strip():
    hint = raw.strip()[:300]
if status == "000":
    hint = "no answer from the K2 daemon"
held = code in ("sim_held", "held")
if held and not hint:
    hint = "held: a person is using the device"
if not hint:
    hint = "the daemon refused (HTTP %s)" % status
if not as_json:
    print("Error: %s" % hint, file=sys.stderr)
    if held:
        print("Run `k2 sim wait`, then `k2 sim tree` before acting again.", file=sys.stderr)
print(json.dumps({"error": {"code": code, "hint": hint, "status": int(status) if status.isdigit() else 0}}), file=sys.stderr)
if status == "000" or status.startswith("5"):
    sys.exit(1)
if status == "400" or held:
    sys.exit(2)
sys.exit(3)
' "$_SIM_STATUS" "${_SIM_JSON:-0}" || rc=$?
    exit "${rc:-3}"
}

# Print the daemon's answer: --json as is, else one field (or a fallback).
_sim_print() {
    local field="$1" fallback="${2:-ok}"
    if [ "${_SIM_JSON:-0}" = "1" ]; then
        printf '%s\n' "$_SIM_BODY"
        return 0
    fi
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
field, fallback = sys.argv[1], sys.argv[2]
try:
    v = json.loads(sys.stdin.read() or "{}")
except Exception:
    v = {}
out = v.get(field) if isinstance(v, dict) else None
if isinstance(out, str) and out:
    sys.stdout.write(out if out.endswith("\n") else out + "\n")
else:
    print(fallback)
' "$field" "$fallback"
}

# "10s" / "2m" / "500ms" / "1h" / "30" (seconds) → milliseconds, or fail.
_sim_ms() {
    python3 - "$1" <<'PY' || return 1
import re, sys
m = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h)?\s*", sys.argv[1])
if not m:
    raise SystemExit(1)
n, unit = float(m.group(1)), m.group(2) or "s"
print(int(n * {"ms": 1, "s": 1000, "m": 60_000, "h": 3_600_000}[unit]))
PY
}

_sim_is_id() {
    [[ "${1:-}" =~ ^(t[0-9]+:)?e[0-9]+$ ]]
}

# The flags every verb takes; the rest go back in _SIM_ARGS.
_sim_common() {
    local ws=""
    _SIM_ARGS=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) _SIM_JSON=1; shift ;;
            --workspace|-w) [ $# -ge 2 ] || _sim_die usage "--workspace needs a path or name"; ws="$2"; shift 2 ;;
            --workspace=*) ws="${1#*=}"; shift ;;
            *) _SIM_ARGS+=("$1"); shift ;;
        esac
    done
    _sim_target "$ws"
}

_sim_status() {
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 0 ] || _sim_die usage "k2 sim status takes no arguments: ${_SIM_ARGS[*]}"
    _sim_call GET /cli/sim/status "$(_sim_query)"
    if [ "$_SIM_JSON" = "1" ]; then printf '%s\n' "$_SIM_BODY"; return 0; fi
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
lanes = v.get("lanes") or []
for l in lanes:
    setup = l.get("setup") or {}
    word = "ready" if l.get("ready") else ("setting up" if setup.get("running") else "needs a step" if l.get("usable") else "not available")
    line = "%-9s %s" % (l.get("lane", "?"), word)
    if l.get("detail"):
        line += " (%s)" % l["detail"]
    print("lane " + line)
    needs = l.get("needs") or []
    licences = [n for n in needs if n.get("kind") == "licence"]
    accepted = setup.get("licencesAccepted") or 0
    for n in needs:
        k = n.get("kind")
        if k == "licence":
            title, lid = n.get("title") or n.get("id"), n.get("id") or ""
            print("  the owner must accept the %s%s (a card; never you)" % (title, "" if lid in title else " [%s]" % lid))
        elif k == "install":
            if not setup.get("error"):
                print("  first use downloads %s" % n.get("what"))
        elif k == "owner-step":
            print("  owner step: %s: %s" % (n.get("what"), n.get("command")))
    if len(licences) > 1:
        print("  one card covers all %d licences; the owner accepts them together" % len(licences))
    if setup.get("running"):
        if accepted:
            print("  the owner accepted %s; nothing left to accept" % ("the licence" if accepted == 1 else "all %d licences" % accepted))
        print("  installing now; devices can boot when it finishes. k2 sim status again to follow it")
    elif setup.get("error") and not licences:
        print("  setup failed%s; the owner of this server runs it again (Retry setup on the Mobile tab), never you" % ("; the licences stay accepted" if accepted else ""))
s = v.get("sim")
if not s or s.get("state") in ("stopped", "failed"):
    ready = any(l.get("ready") for l in lanes)
    tail = ""
    if s and s.get("state") == "failed":
        tail = " (last: failed%s)" % ((": " + s["reason"]) if s.get("reason") else "")
    print("device: none running%s%s" % (tail, "; k2 sim boot" if ready else ""))
else:
    p = s.get("profile") or {}
    size = (" · %sx%s pt" % (p.get("widthPt"), p.get("heightPt"))) if p.get("widthPt") else ""
    app = (" · " + s["appId"]) if s.get("appId") else ""
    print("device: sim %s · %s · %s · %s%s%s" % (s.get("sim"), s.get("lane"), p.get("name", "?"), s.get("state"), size, app))
    h = s.get("hold") or {}
    if h.get("kind") == "person":
        print("held: %s is using the device" % (h.get("by") or "a person"))
    elif h.get("kind") == "takeover":
        print("held: %s has taken over (until Hand back)" % (h.get("by") or "a person"))
print("agents: %s" % ("allowed (Allow mobile devices is on)" if v.get("agentsAllowed") else "not allowed (Allow mobile devices is off)"))
'
}

_sim_list_devices() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local platform=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --platform) [ $# -ge 2 ] || _sim_die usage "--platform needs android or ios"; platform="$2"; shift 2 ;;
            --platform=*) platform="${1#*=}"; shift ;;
            *) _sim_die usage "unknown argument for k2 sim list-devices: $1" ;;
        esac
    done
    case "$platform" in ""|android|ios) ;; *) _sim_die usage "--platform is android or ios, not $platform" ;; esac
    _sim_call GET /cli/sim/devices "$(_sim_query "platform=$platform")"
    if [ "$_SIM_JSON" = "1" ]; then printf '%s\n' "$_SIM_BODY"; return 0; fi
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
ds = v.get("devices") or []
if not ds:
    print("No device profiles on this server (k2 sim status says why).")
for d in ds:
    print("%-28s %-18s %sx%s pt  %s%s" % (d.get("id", "?"), d.get("name", "?"), d.get("widthPt", "?"), d.get("heightPt", "?"), d.get("os", ""), "  (default)" if d.get("default") else ""))
'
}

_sim_boot() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local platform="" device="" notab=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --platform) [ $# -ge 2 ] || _sim_die usage "--platform needs android or ios"; platform="$2"; shift 2 ;;
            --platform=*) platform="${1#*=}"; shift ;;
            --device) [ $# -ge 2 ] || _sim_die usage "--device needs a profile (k2 sim list-devices)"; device="$2"; shift 2 ;;
            --device=*) device="${1#*=}"; shift ;;
            --no-tab) notab=1; shift ;;
            *) _sim_die usage "unknown argument for k2 sim boot: $1" ;;
        esac
    done
    case "$platform" in ""|android|ios) ;; *) _sim_die usage "--platform is android or ios, not $platform" ;; esac
    # The daemon answers when the device runs: up to 240 s of boot (plus a
    # first-use system image), so allow 330 s.
    _K2_SEND_MAX_TIME=330 _sim_call POST /cli/sim/boot "$(_sim_body "s:platform=$platform" "s:device=$device" "b:noTab=$notab")"
    if [ "$_SIM_JSON" = "1" ]; then printf '%s\n' "$_SIM_BODY"; return 0; fi
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
s = json.load(sys.stdin) or {}
state = s.get("state", "?")
name = (s.get("profile") or {}).get("name", "?")
if state == "running":
    print("Running: sim %s · %s · %s. Next: k2 sim install <app>, or k2 sim tree" % (s.get("sim"), s.get("lane"), name))
else:
    print("Booting: sim %s · %s · %s (%s). Next: k2 sim wait, then k2 sim tree" % (s.get("sim"), s.get("lane"), name, state))
'
}

_sim_simple_post() {
    local verb="$1" route="$2" done_msg="$3"
    shift 3
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 0 ] || _sim_die usage "k2 sim $verb takes no arguments: ${_SIM_ARGS[*]}"
    _sim_call POST "$route" "$(_sim_body)"
    _sim_print line "$done_msg"
}

_sim_erase() {
    _sim_common "$@"
    local yes=0 a
    for a in ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}; do
        case "$a" in
            --yes|-y) yes=1 ;;
            *) _sim_die usage "unknown argument for k2 sim erase: $a" ;;
        esac
    done
    if [ "$yes" != 1 ]; then
        if [ -t 0 ] && [ -t 1 ]; then
            printf 'Erase this workspace'"'"'s device? Its apps and data are deleted. [y/N] ' >&2
            local reply=""
            read -r reply || true
            case "$reply" in y|Y|yes|YES) ;; *) echo "Not erased." >&2; exit 2 ;; esac
        else
            _sim_die confirm_required "k2 sim erase deletes the device's apps and data; pass --yes" 2
        fi
    fi
    _sim_call POST /cli/sim/erase "$(_sim_body)"
    _sim_print line "Erased. The next k2 sim boot starts a fresh device."
}

_sim_install() {
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 1 ] || _sim_die usage "usage: k2 sim install <app.apk|App.app>"
    local p="${_SIM_ARGS[0]}"
    [ -e "$p" ] || _sim_die not_found "no such file: $p (build the app in your terminal first)"
    p="$(cd "$(dirname "$p")" && pwd)/$(basename "$p")"
    # A large app can take minutes to push and install.
    _K2_SEND_MAX_TIME=330 _sim_call POST /cli/sim/install "$(_sim_body "s:path=$p")"
    _sim_print line "Installed $(basename "$p")"
}

_sim_app_verb() {
    local verb="$1"
    shift
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 1 ] || _sim_die usage "usage: k2 sim $verb <package|bundleId>"
    _sim_call POST "/cli/sim/$verb" "$(_sim_body "s:app=${_SIM_ARGS[0]}")"
    _sim_print line "ok $verb ${_SIM_ARGS[0]}"
}

_sim_tree() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local all="" find="" boxes=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --all) all=1; shift ;;
            --boxes) boxes=1; shift ;;
            --find) [ $# -ge 2 ] || _sim_die usage "--find needs text"; find="$2"; shift 2 ;;
            --find=*) find="${1#*=}"; shift ;;
            *) _sim_die usage "unknown argument for k2 sim tree: $1" ;;
        esac
    done
    _sim_call GET /cli/sim/tree "$(_sim_query "all=$all" "find=$find" "boxes=$boxes")"
    _sim_print text "(the daemon sent no tree)"
}

# POST /cli/sim/act {workspace?, action: {verb, …}} and print its one line.
_sim_act() {
    _sim_call POST /cli/sim/act "$(_sim_body "$@")"
    _sim_print line "ok"
}

_sim_tap() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local el="" at=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --at) [ $# -ge 2 ] || _sim_die usage "--at needs x,y (dp)"; at="$2"; shift 2 ;;
            --at=*) at="${1#*=}"; shift ;;
            -*) _sim_die usage "unknown flag for k2 sim tap: $1" ;;
            *) [ -z "$el" ] || _sim_die usage "k2 sim tap takes one element"; el="$1"; shift ;;
        esac
    done
    if [ -n "$at" ]; then
        [ -z "$el" ] || _sim_die usage "k2 sim tap takes an element OR --at x,y, not both"
        [[ "$at" =~ ^[0-9]+(\.[0-9]+)?,[0-9]+(\.[0-9]+)?$ ]] || _sim_die usage "--at is x,y in dp, like 120,300"
        _sim_act "s:action.verb=tap" "n:action.x=${at%,*}" "n:action.y=${at#*,}"
        return
    fi
    [ -n "$el" ] || _sim_die usage "usage: k2 sim tap e5 | k2 sim tap --at 120,300"
    _sim_is_id "$el" || _sim_die bad_element_id "\"$el\" is not an element id (use one from k2 sim tree, like e5)"
    _sim_act "s:action.verb=tap" "s:action.element=$el"
}

_sim_type() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local into="" secret="" text="" have_text=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --into) [ $# -ge 2 ] || _sim_die usage "--into needs an element id"; into="$2"; shift 2 ;;
            --into=*) into="${1#*=}"; shift ;;
            --secret) [ $# -ge 2 ] || _sim_die usage "--secret needs a vault name"; secret="$2"; shift 2 ;;
            --secret=*) secret="${1#*=}"; shift ;;
            --) shift; [ $# -ge 1 ] && { text="$1"; have_text=1; shift; } ;;
            -*) _sim_die usage "unknown flag for k2 sim type: $1" ;;
            *) [ "$have_text" = 0 ] || _sim_die usage "k2 sim type takes one text (quote it)"; text="$1"; have_text=1; shift ;;
        esac
    done
    if [ -n "$into" ]; then
        _sim_is_id "$into" || _sim_die bad_element_id "\"$into\" is not an element id (use one from k2 sim tree, like e3)"
    fi
    if [ -n "$secret" ]; then
        [ "$have_text" = 0 ] || _sim_die usage "k2 sim type takes text OR --secret <vault-name>, not both"
        _sim_act "s:action.verb=type" "s:action.into=$into" "s:action.secret=$secret"
        return
    fi
    [ "$have_text" = 1 ] || _sim_die usage "usage: k2 sim type [--into e3] \"text\" | --secret <vault-name>"
    _sim_act "s:action.verb=type" "s:action.into=$into" "j:action.text=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$text")"
}

_sim_swipe() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local dir="" from=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --from) [ $# -ge 2 ] || _sim_die usage "--from needs an element id"; from="$2"; shift 2 ;;
            --from=*) from="${1#*=}"; shift ;;
            up|down|left|right) [ -z "$dir" ] || _sim_die usage "one direction"; dir="$1"; shift ;;
            *) _sim_die usage "k2 sim swipe up|down|left|right [--from e7] (not: $1)" ;;
        esac
    done
    [ -n "$dir" ] || _sim_die usage "usage: k2 sim swipe up|down|left|right [--from e7]"
    if [ -n "$from" ]; then
        _sim_is_id "$from" || _sim_die bad_element_id "\"$from\" is not an element id"
    fi
    _sim_act "s:action.verb=swipe" "s:action.direction=$dir" "s:action.from=$from"
}

_sim_choice() {
    local verb="$1" key="$2" choices="$3"
    shift 3
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 1 ] || _sim_die usage "usage: k2 sim $verb ${choices// /|}"
    local v="${_SIM_ARGS[0]}" c ok=0
    for c in $choices; do [ "$c" = "$v" ] && ok=1; done
    [ "$ok" = 1 ] || _sim_die usage "k2 sim $verb takes ${choices// /, } (not $v)"
    _sim_act "s:action.verb=$verb" "s:action.$key=$v"
}

_sim_open_url() {
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 1 ] || _sim_die usage "usage: k2 sim open-url <url or deep link>"
    [[ "${_SIM_ARGS[0]}" == *:* ]] || _sim_die usage "open-url needs a URL with a scheme (https://…, myapp://…)"
    _sim_act "s:action.verb=open-url" "s:action.url=${_SIM_ARGS[0]}"
}

_sim_location() {
    _sim_common "$@"
    [ ${#_SIM_ARGS[@]} -eq 1 ] || _sim_die usage "usage: k2 sim location <lat,lon>"
    local ll="${_SIM_ARGS[0]}"
    [[ "$ll" =~ ^-?[0-9]+(\.[0-9]+)?,-?[0-9]+(\.[0-9]+)?$ ]] || _sim_die usage "location is lat,lon, like 37.33,-122.03"
    _sim_act "s:action.verb=location" "n:action.lat=${ll%,*}" "n:action.lon=${ll#*,}"
}

_sim_screenshot() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local region=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --region) [ $# -ge 2 ] || _sim_die usage "--region needs an element id"; region="$2"; shift 2 ;;
            --region=*) region="${1#*=}"; shift ;;
            *) _sim_die usage "unknown argument for k2 sim screenshot: $1" ;;
        esac
    done
    if [ -n "$region" ]; then
        _sim_is_id "$region" || _sim_die bad_element_id "\"$region\" is not an element id"
    fi
    _sim_call POST /cli/sim/screenshot "$(_sim_body "s:region=$region")"
    _sim_print path "(the daemon sent no path)"
}

_sim_logs() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local app="" since="" grep_="" lines="" ms=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --app) [ $# -ge 2 ] || _sim_die usage "--app needs a package or bundle id"; app="$2"; shift 2 ;;
            --since) [ $# -ge 2 ] || _sim_die usage "--since needs a duration (2m)"; since="$2"; shift 2 ;;
            --grep) [ $# -ge 2 ] || _sim_die usage "--grep needs text"; grep_="$2"; shift 2 ;;
            --lines) [ $# -ge 2 ] || _sim_die usage "--lines needs a number"; lines="$2"; shift 2 ;;
            *) _sim_die usage "unknown argument for k2 sim logs: $1" ;;
        esac
    done
    if [ -n "$since" ]; then
        ms="$(_sim_ms "$since")" || _sim_die usage "--since is a duration like 30s, 2m or 1h"
        ms=$(( (ms + 999) / 1000 ))
    fi
    if [ -n "$lines" ]; then
        [[ "$lines" =~ ^[0-9]+$ ]] && [ "$lines" -ge 1 ] || _sim_die usage "--lines is a positive number"
    fi
    _sim_call GET /cli/sim/logs "$(_sim_query "app=$app" "sinceSecs=$ms" "grep=$grep_" "maxLines=$lines")"
    if [ "$_SIM_JSON" = "1" ]; then printf '%s\n' "$_SIM_BODY"; return 0; fi
    printf '%s' "$_SIM_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
lines = v.get("lines") or []
for l in lines:
    print(l)
if not lines:
    print("(no log lines)")
if v.get("truncated"):
    print("(cut at the line/byte cap; narrow with --since, --app or --grep)", file=sys.stderr)
'
}

_sim_wait() {
    _sim_common "$@"
    set -- ${_SIM_ARGS[@]+"${_SIM_ARGS[@]}"}
    local for_="" timeout="10s" ms=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --for) [ $# -ge 2 ] || _sim_die usage "--for needs text"; for_="$2"; shift 2 ;;
            --for=*) for_="${1#*=}"; shift ;;
            --timeout) [ $# -ge 2 ] || _sim_die usage "--timeout needs a duration (10s)"; timeout="$2"; shift 2 ;;
            --timeout=*) timeout="${1#*=}"; shift ;;
            *) _sim_die usage "unknown argument for k2 sim wait: $1" ;;
        esac
    done
    ms="$(_sim_ms "$timeout")" || _sim_die usage "--timeout is a duration like 10s or 2m"
    [ "$ms" -le 120000 ] || _sim_die usage "--timeout is at most 2m (wait again if you must)"
    # The daemon holds the request up to --timeout (2 m at most); allow 30 s more.
    _K2_SEND_MAX_TIME=$(( ms / 1000 + 30 )) _sim_call POST /cli/sim/wait "$(_sim_body "s:for=$for_" "n:timeoutMs=$ms")"
    _sim_print line "ok wait"
}

cmd_sim() {
    _SIM_JSON=0
    local sub="${1:-}"
    if _wants_help "$@" || [ -z "$sub" ] || [ "$sub" = "help" ]; then
        _sim_usage
        return 0
    fi
    shift
    case "$sub" in
        status) _sim_status "$@" ;;
        list-devices|devices) _sim_list_devices "$@" ;;
        boot|start) _sim_boot "$@" ;;
        stop) _sim_simple_post stop /cli/sim/stop "Stopped. Its data is kept; k2 sim boot starts it again." "$@" ;;
        erase) _sim_erase "$@" ;;
        install) _sim_install "$@" ;;
        launch) _sim_app_verb launch "$@" ;;
        terminate|kill) _sim_app_verb terminate "$@" ;;
        tree) _sim_tree "$@" ;;
        tap) _sim_tap "$@" ;;
        type) _sim_type "$@" ;;
        swipe) _sim_swipe "$@" ;;
        key) _sim_choice key key "enter backspace delete tab escape arrow-up arrow-down arrow-left arrow-right" "$@" ;;
        button) _sim_choice button button "home back lock volume-up volume-down app-switch dev-menu" "$@" ;;
        rotate) _sim_choice rotate turn "left right" "$@" ;;
        appearance) _sim_choice appearance appearance "dark light" "$@" ;;
        open-url|open) _sim_open_url "$@" ;;
        location) _sim_location "$@" ;;
        screenshot|shot) _sim_screenshot "$@" ;;
        logs|log) _sim_logs "$@" ;;
        wait) _sim_wait "$@" ;;
        *)
            echo "Error: unknown k2 sim command: $sub (status, list-devices, boot, stop, erase, install, launch, terminate, tree, tap, type, swipe, key, button, rotate, open-url, location, appearance, screenshot, logs, wait)" >&2
            _sim_usage >&2
            exit 2 ;;
    esac
}
# END_CLI_SIM
# BEGIN_CLI_DOC
