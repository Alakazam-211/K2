# ── k2 llm tokens — LLM subscriptions and API tokens ─────────────────
# (`k2 llm accounts` is a working alias.) One token per tool is the
# SERVER DEFAULT, kept where the tool always keeps it; the other
# subscriptions and API tokens wait in ~/.k2/llm-accounts. GET
# /cli/llm/accounts/list|status|usage|login/status|pins|storage; POST
# add|add-key|login|login/input|login/cancel|switch|next|rename|remove|
# refresh|pin|unpin|storage/set (JSON bodies; route names are internal and
# unchanged).
# `llm` is an `id` tool: inside a K2 terminal the request carries this
# session's passport, and the daemon keeps passports read-only
# (list/status/usage/pins). Change tokens on Settings → Language Models → Subs / Tokens, or
# from a terminal outside K2.

_llm_accounts_usage() {
    cat <<'EOF'
k2 llm tokens — LLM tokens: subscriptions and API tokens
(Claude, Codex, Grok subscriptions; API tokens also Gemini)

  k2 llm tokens list [--tool <tool>] [--json]
  k2 llm tokens status <tool> <label|id> [--json]
  k2 llm tokens usage [--json]
  k2 llm tokens add <tool> <label> [--device]        add a subscription (the tool's own sign-in)
  k2 llm tokens login <tool> <label> [--device]      sign a subscription in again
  k2 llm tokens switch <tool> <label>                use this token as the server default (Admin or Owner)
  k2 llm tokens next <tool>                          use the next signed-in subscription (Admin or Owner)
  k2 llm tokens rename <tool> <label> <new-label>
  k2 llm tokens remove <tool> <label> [--clear-default]
                                                     kept in Trash; the server default API token
                                                     only with --clear-default (Admin or Owner)
  k2 llm tokens clear-default <tool>                 no server default for that tool (an API token
                                                     default only; Admin or Owner)
  k2 llm tokens refresh [<tool> <label>] [--usage]   re-read state (and usage)
  k2 llm tokens add-key <tool> <label>               add an API token (read from stdin)
  k2 llm tokens pin --workspace <ws> <tool> <label>  use this token for a workspace
  k2 llm tokens pin --session <key> <tool> <label>   use this token for one chat
  k2 llm tokens unpin --workspace <ws>|--session <key> <tool>
                                                     back to Server default
  k2 llm tokens pins [--json]                        workspaces and chats with their own token
  k2 llm tokens for <agent> [--refresh] [--json]     the token one chat runs on (<agent> = ws or a
                                                     sidecar ws/2) and its usage, the top-bar chip's
                                                     number; --refresh re-reads it (once a minute)
  k2 llm tokens storage [--json]                     where saved tokens are kept (files or the K2
                                                     vault) and whether each is sealed
  k2 llm tokens storage files|vault                  change it (Admin or Owner; audited). The plain
                                                     files stay either way, so going back is instant

`k2 llm accounts …` is the same command (alias).

Each tool has ONE server default token per server. Changing the server
default affects every chat that uses Server default on this server;
running sessions pick up the new token.
add/login print the sign-in page and any device code; when the tool asks
for a code, paste it at the prompt (it is read from stdin, never an argument).
--device signs in from another device (device code where the tool has one).
Which token a chat uses: its own token > its workspace's token > Server
default. A chat's own token (--session <key>: the pinned chat's key is its
workspace id, a tab's is tab-<id>; or conversation:<id>) applies every time
that chat starts or is revived. Otherwise a resumed chat keeps the token it
started on. A subscription set for a workspace or chat is never also the
server default, and `next` skips it.
API tokens (add-key) are billed per token, go only to sessions that use
them, and are never shown again. The key is read from stdin, never an argument.
Agents and K2 terminals are read-only (list, status, usage, pins, for). Change
tokens on Settings → Language Models → Subs / Tokens, or run k2 from a terminal outside K2.
More: k2 study llm-tokens
EOF
}

# Paths (kept in variables: none of these is ever sent as a GET unless it
# is a read route).
_LLMA_BASE="/cli/llm/accounts"

# _llma_json k1 v1 k2 v2 … → a JSON object. Values "__true__"/"__false__"
# become booleans; empty values are dropped.
_llma_json() {
    python3 -c '
import json, sys
a = sys.argv[1:]
out = {}
for k, v in zip(a[::2], a[1::2]):
    if v == "__true__":
        out[k] = True
    elif v == "__false__":
        out[k] = False
    elif v != "":
        out[k] = v
print(json.dumps(out))
' "$@"
}

# Tools that take API keys (and so can be pinned): the sign-in tools
# plus Gemini.
_llma_key_tool_ok() {
    case "${1:-}" in
        claude|codex|grok|gemini) return 0 ;;
        cursor|cursor-agent|pi|hermes)
            echo "usage: ${1} has no LLM tokens yet (claude, codex, grok, gemini)" >&2
            exit 2
            ;;
        "")
            echo "usage: missing <tool> (claude, codex, grok or gemini) — k2 llm tokens --help" >&2
            exit 2
            ;;
        *)
            echo "usage: unknown tool '$1' (claude, codex, grok or gemini) — k2 llm tokens --help" >&2
            exit 2
            ;;
    esac
}

_llma_tool_ok() {
    case "${1:-}" in
        claude|codex|grok) return 0 ;;
        gemini|cursor|cursor-agent|pi|hermes)
            echo "usage: ${1} has no subscriptions yet (claude, codex, grok)" >&2
            exit 2
            ;;
        "")
            echo "usage: missing <tool> (claude, codex or grok) — k2 llm tokens --help" >&2
            exit 2
            ;;
        *)
            echo "usage: unknown tool '$1' (claude, codex or grok) — k2 llm tokens --help" >&2
            exit 2
            ;;
    esac
}

# GET a read route; sets _SC_BODY/_SC_STATUS, exits on a refusal.
_llma_get() {
    local path="$1" query="${2:-}"
    _sidecar_split "$(_sidecar_send GET "${_LLMA_BASE}/${path}" "$query")"
    [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
}

# POST a JSON body; sets _SC_BODY/_SC_STATUS, exits on a refusal.
_llma_post() {
    local path="$1" body="$2"
    _sidecar_split "$(_sidecar_send POST "${_LLMA_BASE}/${path}" "$body")"
    [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
}

# Resolve <tool> <label|id> to an account id through the list route.
_llma_resolve() {
    local tool="$1" key="$2" id
    _llma_get list
    id="$(printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
tool, key = sys.argv[1], sys.argv[2]
v = json.load(sys.stdin)
for t in v.get("tools", []):
    if t.get("tool") != tool:
        continue
    for a in t.get("accounts", []):
        if a.get("id") == key or (a.get("label") or "").lower() == key.lower():
            print(a["id"]); sys.exit(0)
' "$tool" "$key" 2>/dev/null)"
    if [ -z "$id" ]; then
        echo "not_found: no ${tool} token named '${key}' (k2 llm tokens list --tool ${tool})" >&2
        exit 4
    fi
    printf '%s' "$id"
}

_llma_render_list() {
    local tool_filter="${1:-}"
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
want = sys.argv[1]
v = json.load(sys.stdin)

import time

def ago(ts):
    # "checked 12 min ago"; ts is unix seconds.
    if not ts:
        return "never checked"
    d = max(0, int(time.time()) - int(ts))
    if d < 60:
        return "checked just now"
    if d < 3600:
        return "checked %d min ago" % (d // 60)
    if d < 86400:
        return "checked %d h ago" % (d // 3600)
    return "checked %d d ago" % (d // 86400)

def usage_summary(u, checked):
    if not u:
        return "-"
    wins = u.get("windows") or []
    if wins:
        s = " · ".join("%s %d%%" % (w.get("label", "?"), round(float(w.get("used", 0)) * 100)) for w in wins)
    else:
        s = u.get("status") or "-"
    s += " (%s)" % ago(checked)
    if u.get("error"):
        s += " [last check failed: %s]" % u["error"]
    return s

for t in v.get("tools", []):
    name = t.get("tool")
    if want and name != want and not (want == "cursor-agent" and name == "cursor"):
        continue
    disp = t.get("display") or name
    if not t.get("supported"):
        print("%s: no tokens yet" % disp)
        continue
    accts = t.get("accounts") or []
    if not accts:
        if t.get("subscription") is False:
            print("%s: no API tokens (add one: k2 llm tokens add-key %s <label>)" % (disp, name))
        else:
            print("%s: no tokens (add a subscription: k2 llm tokens add %s <label>)" % (disp, name))
        continue
    since = ""
    if t.get("switchedAt"):
        since = " (server default since %s%s)" % (
            time.strftime("%Y-%m-%d %H:%M", time.localtime(int(t["switchedAt"]))),
            (" by %s" % t["switchedBy"]) if t.get("switchedBy") else "")
    print("%s:%s" % (disp, since))
    for a in accts:
        mark = "*" if a.get("active") else " "
        if a.get("kind") == "api_key":
            line = "  %s %-20s %-12s %s" % (mark, (a.get("label") or "-")[:20], a.get("state") or "-",
                                            "API token · billed per token")
        else:
            line = "  %s %-20s %-12s %-8s %s" % (mark, (a.get("label") or "-")[:20], a.get("state") or "-",
                                                 (a.get("plan") or "-")[:8],
                                                 usage_summary(a.get("usage"), a.get("usageCheckedAt")))
        print(line.rstrip())
        pins = a.get("pinnedTo") or []
        if pins:
            print("      used by: %s%s" % (", ".join("%s %s" % ("chat" if p.get("scopeKind") == "session" else p.get("scopeKind"), p.get("label") or p.get("scopeId")) for p in pins),
                                         " (in use)" if a.get("inUse") else ""))
        if a.get("detail"):
            print("      %s" % a["detail"])
if v.get("airgap"):
    print("Air-gap is on: no sign-in, refresh or usage calls.")
for r in v.get("keychainRepairs") or []:
    print("WARN keychain: macOS asks for your password on every Claude read of \"%s\" (%s): its partition list is %s, without apple-tool:." % (
        r.get("service"), r.get("holds"), ",".join(r.get("partitions") or []) or "empty"))
    print("     Fix once in Terminal (asks for your login password): %s" % r.get("command"))
print("* = the server default token.")
print(v.get("switchNote") or "Changing the server default token affects every chat that uses Server default on this server.")
' "$tool_filter"
}

# Follow a login started by add/login until it ends. Prints the sign-in
# page and device code once each; reads a pasted code from stdin when the
# tool asks for one (never echoed, never an argument).
_llma_follow_login() {
    local start_body="$1"
    local login_id
    login_id="$(printf '%s' "$start_body" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print((v.get("login") or {}).get("loginId") or "")
' 2>/dev/null)"
    if [ -z "$login_id" ]; then
        echo "daemon_error: the daemon did not start a sign-in" >&2
        exit 1
    fi
    _LLMA_LOGIN_ID="$login_id"
    trap '_llma_post login/cancel "$(_llma_json loginId "$_LLMA_LOGIN_ID")" >/dev/null 2>&1; echo "cancelled" >&2; exit 130' INT TERM
    local poll="${K2_LLMA_POLL_SECS:-2}"
    local shown_url="" shown_code="" shown_banner="" prompted=0
    local deadline=$(( $(date +%s) + 1200 ))
    while :; do
        _llma_get login/status "loginId=$(urlencode "$login_id")"
        local fields
        fields="$(printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
l = json.load(sys.stdin).get("login") or {}
def one(x):
    return ("" if x is None else str(x)).replace("\n", " ").replace("\t", " ")
print("\x1f".join([one(l.get("state")), one(l.get("url")), one(l.get("code")), one(l.get("banner")),
                 one(l.get("error")), "1" if l.get("done") else "0", "1" if l.get("offerMakeActive") else "0",
                 one(l.get("tool")), one(l.get("label"))]))
')"
        local state url code banner err done offer tool label
        IFS=$'\x1f' read -r state url code banner err done offer tool label <<< "$fields"
        if [ -n "$banner" ] && [ "$banner" != "$shown_banner" ]; then
            echo "$banner" >&2
            shown_banner="$banner"
        fi
        if [ -n "$url" ] && [ "$url" != "$shown_url" ]; then
            if [ -t 1 ]; then
                # The URL alone on its own line, as an OSC 8 link: a
                # terminal that wraps it still opens/copies it whole.
                echo "Open this page to sign in:"
                printf '\033]8;;%s\033\\%s\033]8;;\033\\\n' "$url" "$url"
            else
                echo "Open this page to sign in: $url"
            fi
            shown_url="$url"
        fi
        if [ -n "$code" ] && [ "$code" != "$shown_code" ]; then
            echo "Enter this code on that page: $code"
            shown_code="$code"
        fi
        case "$state" in
            signed_in)
                trap - INT TERM
                echo "Signed in: ${tool} subscription '${label}'."
                if [ "$offer" = "1" ]; then
                    echo "It is not the server default. To use it now: k2 llm tokens switch ${tool} '${label}'"
                    echo "Changing the server default token affects every chat that uses Server default on this server."
                fi
                _setup_kessel_hint
                exit 0
                ;;
            failed|timed_out|cancelled)
                trap - INT TERM
                echo "sign-in ${state}${err:+: $err}" >&2
                exit 1
                ;;
            waiting_for_code)
                if [ "$prompted" = "0" ]; then
                    prompted=1
                    local pasted=""
                    printf 'Paste the code: ' >&2
                    if [ -t 0 ]; then
                        IFS= read -rs pasted || pasted=""
                        echo >&2
                    else
                        IFS= read -r pasted || pasted=""
                    fi
                    if [ -z "$pasted" ]; then
                        _llma_post login/cancel "$(_llma_json loginId "$login_id")" >/dev/null 2>&1 || true
                        trap - INT TERM
                        echo "no code entered; sign-in cancelled" >&2
                        exit 1
                    fi
                    _llma_post login/input "$(_llma_json loginId "$login_id" text "$pasted")"
                    pasted=""
                    continue
                fi
                ;;
        esac
        if [ "$done" = "1" ]; then
            trap - INT TERM
            echo "sign-in ended (${state:-unknown})${err:+: $err}" >&2
            exit 1
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            _llma_post login/cancel "$(_llma_json loginId "$login_id")" >/dev/null 2>&1 || true
            trap - INT TERM
            echo "sign-in timed out" >&2
            exit 1
        fi
        sleep "$poll"
    done
}

cmd_llm_accounts() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _llm_accounts_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        _llm_accounts_usage
        return 0
    fi
    local as_json=0 device=0 usage=0 tool_filter="" pin_ws="" pin_sess="" clear_default="" for_refresh=0
    local pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --device) device=1; shift ;;
            --refresh) for_refresh=1; shift ;;
            --clear-default) clear_default="__true__"; shift ;;
            --usage) usage=1; shift ;;
            --workspace)
                [ -n "${2:-}" ] || { echo "usage: --workspace needs a value" >&2; exit 2; }
                pin_ws="$2"; shift 2 ;;
            --session)
                [ -n "${2:-}" ] || { echo "usage: --session needs a value" >&2; exit 2; }
                pin_sess="$2"; shift 2 ;;
            --tool)
                [ -n "${2:-}" ] || { echo "usage: --tool needs a value" >&2; exit 2; }
                tool_filter="$2"; shift 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 llm tokens --help)" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local mode="this_computer"
    [ "$device" = "1" ] && mode="other_device"
    case "$sub" in
        list|ls)
            [ "${#pos[@]}" -eq 0 ] || { echo "usage: k2 llm tokens list [--tool <tool>] [--json]" >&2; exit 2; }
            [ -z "$tool_filter" ] || _llma_tool_ok "$tool_filter"
            _llma_get list
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _llma_render_list "$tool_filter"
            ;;
        status)
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens status <tool> <label|id> [--json]" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            _llma_get status "id=$(urlencode "${pos[1]}")&tool=$(urlencode "${pos[0]}")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
a = json.load(sys.stdin).get("account") or {}
print("%s %s%s" % (a.get("tool"), a.get("label"), " (server default)" if a.get("active") else ""))
for k in ("state", "detail", "plan", "email", "org"):
    if a.get(k):
        print("  %-8s %s" % (k, a[k]))
u = a.get("usage") or {}
for w in u.get("windows") or []:
    print("  %-8s %d%% used, resets %s" % (w.get("label"), round(float(w.get("used", 0)) * 100), w.get("resetsAt") or "-"))
'
            ;;
        usage)
            [ "${#pos[@]}" -eq 0 ] || { echo "usage: k2 llm tokens usage [--json]" >&2; exit 2; }
            _llma_get usage
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("rows") or []
if not rows:
    print("(no tokens)")
for r in rows:
    u = r.get("usage") or {}
    wins = u.get("windows") or []
    summ = " · ".join("%s %d%%" % (w.get("label", "?"), round(float(w.get("used", 0)) * 100)) for w in wins) or (u.get("status") or "-")
    print("%s %-7s %-20s %s" % ("*" if r.get("active") else " ", r.get("tool"), (r.get("label") or "-")[:20], summ))
'
            ;;
        add)
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens add <tool> <label> [--device]" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            _llma_post add "$(_llma_json tool "${pos[0]}" label "${pos[1]}" mode "$mode")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _llma_follow_login "$_SC_BODY"
            ;;
        login)
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens login <tool> <label> [--device]" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            local lid
            lid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
            _llma_post login "$(_llma_json id "$lid" mode "$mode")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _llma_follow_login "$_SC_BODY"
            ;;
        switch|use)
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens switch <tool> <label>" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            local sid
            sid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
            _llma_post switch "$(_llma_json id "$sid")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _llma_render_switch
            ;;
        next)
            [ "${#pos[@]}" -eq 1 ] || { echo "usage: k2 llm tokens next <tool>" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            _llma_post next "$(_llma_json tool "${pos[0]}")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            _llma_render_switch
            ;;
        rename)
            [ "${#pos[@]}" -eq 3 ] || { echo "usage: k2 llm tokens rename <tool> <label> <new-label>" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            local rid
            rid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
            _llma_post rename "$(_llma_json id "$rid" label "${pos[2]}")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            echo "renamed ${pos[0]} '${pos[1]}' to '${pos[2]}'"
            ;;
        remove|rm)
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens remove <tool> <label> [--clear-default]" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            local xid
            xid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
            _llma_post remove "$(_llma_json id "$xid" clearDefault "$clear_default")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            echo "removed ${pos[0]} token '${pos[1]}' (its folder is kept in ~/.k2/llm-accounts/.removed)"
            ;;
        clear-default)
            [ "${#pos[@]}" -eq 1 ] || { echo "usage: k2 llm tokens clear-default <tool>" >&2; exit 2; }
            _llma_tool_ok "${pos[0]}"
            _llma_post clear-default "$(_llma_json tool "${pos[0]}")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
c = json.load(sys.stdin).get("cleared") or {}
tool = c.get("tool") or sys.argv[1]
if not c.get("cleared"):
    print("%s had no server default token" % tool)
elif c.get("nowDefault"):
    print("%s server default: %s → %s (the subscription signed in to %s itself)" % (tool, c.get("clearedLabel"), c.get("nowDefaultLabel"), tool))
else:
    print("%s server default cleared (was %s): chats on Server default get no K2 token" % (tool, c.get("clearedLabel")))
' "${pos[0]}"
            ;;
        refresh)
            local fid="" u=""
            [ "$usage" = "1" ] && u="__true__"
            case "${#pos[@]}" in
                0) ;;
                2)
                    _llma_tool_ok "${pos[0]}"
                    fid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
                    ;;
                *) echo "usage: k2 llm tokens refresh [<tool> <label>] [--usage]" >&2; exit 2 ;;
            esac
            _llma_post refresh "$(_llma_json id "$fid" usage "$u")"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
rows = json.load(sys.stdin).get("accounts") or []
for a in rows:
    print("%s %-7s %-20s %s" % ("*" if a.get("active") else " ", a.get("tool"), (a.get("label") or "-")[:20], a.get("state") or "-"))
'
            ;;
        storage)
            case "${#pos[@]}" in
                0)
                    _llma_get storage
                    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                    ;;
                1)
                    case "${pos[0]}" in
                        files|vault) ;;
                        *) echo "usage: k2 llm tokens storage [files|vault]" >&2; exit 2 ;;
                    esac
                    _llma_post storage/set "$(_llma_json storage "${pos[0]}")"
                    if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                    ;;
                *) echo "usage: k2 llm tokens storage [files|vault]" >&2; exit 2 ;;
            esac
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
src = v.get("source")
print("saved tokens are kept in: %s%s" % (v.get("storage"), "" if src == "setting" else " (%s)" % ("K2_LLM_ACCOUNTS_STORAGE" if src == "env" else "the default")))
print("K2 vault: %s" % ("open" if v.get("vaultOpen") else "NOT open (%s)" % (v.get("vaultError") or "?")))
for k in ("banner", "nudge"):
    if v.get(k):
        print(v[k])
s = v.get("sync")
if s:
    print("sealed %d, already sealed %d, removed kept disabled %d, waiting %d" % (s.get("sealed", 0), s.get("unchanged", 0), s.get("disabled", 0), s.get("waiting", 0)))
e = v.get("export")
if e and e.get("restored"):
    print("wrote %d file(s) back from the vault" % e.get("restored"))
items = v.get("items") or []
if items:
    print("%-7s %-22s %-5s %-9s %s" % ("tool", "token", "file", "vault", "in step"))
for i in items:
    step = {True: "yes", False: "NO", None: "-"}[i.get("inStep")]
    print("%-7s %-22s %-5s %-9s %s%s" % (i.get("tool"), (i.get("label") or "-")[:22], "yes" if i.get("file") else "no", i.get("vault"), step, "  (removed)" if i.get("removed") else ""))
'
            ;;
        pins)
            [ "${#pos[@]}" -eq 0 ] || { echo "usage: k2 llm tokens pins [--json]" >&2; exit 2; }
            _llma_get pins
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            local pins_body="$_SC_BODY"
            _llma_get list
            printf '%s\n%s' "$pins_body" "$_SC_BODY" | python3 -c '
import json, sys
pins_raw, list_raw = sys.stdin.read().split("\n", 1)
pins = json.loads(pins_raw).get("pins") or []
labels = {}
for t in json.loads(list_raw).get("tools", []):
    for a in t.get("accounts") or []:
        labels[a.get("id")] = a.get("label")
if not pins:
    print("(no workspace or chat has its own token — every chat uses Server default)")
for p in pins:
    print("%-7s %-9s %-28s -> %s" % (p.get("tool"), "chat" if p.get("scopeKind") == "session" else p.get("scopeKind"), (p.get("label") or p.get("scopeId") or "-")[:28],
                                    labels.get(p.get("accountId")) or p.get("accountId")))
'
            ;;
        for)
            [ "${#pos[@]}" -eq 1 ] || { echo "usage: k2 llm tokens for <agent> [--refresh] [--json]   (<agent> = a workspace or a sidecar ws/2)" >&2; exit 2; }
            if [ "$for_refresh" = "1" ]; then
                _llma_post for/refresh "$(_llma_json addr "${pos[0]}")"
            else
                _llma_get for "addr=$(urlencode "${pos[0]}")"
            fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
addr = v.get("address") or sys.argv[1]
if not v.get("tool"):
    print("%s: not an LLM chat (no token)" % addr)
    sys.exit(0)
scope = {"chat": "set for this chat", "workspace": "set for its workspace", "person": "set for this person"}
why = {"pinned": scope.get(v.get("pinScope"), "set for it"),
       "resumed": "the token its conversation started on",
       "default": "the server default"}.get(v.get("source"), v.get("source") or "-")
print("%s: %s token %s (%s)%s" % (addr, v.get("tool"), v.get("label") or "-", why, "" if v.get("live") else " · next start"))
if v.get("unavailable"):
    print("  %s" % v["unavailable"])
mp = v.get("maxPercent")
wins = v.get("windows") or []
def pct(w):
    # The chip rule: 0 once reset, else usedPercent clamped and rounded half up.
    if w.get("resetPassed"):
        return 0
    p = float(w.get("usedPercent", float(w.get("used", 0)) * 100))
    return int(min(100.0, max(0.0, p)) + 0.5)
if mp is None:
    print("  usage: no reading" + (" (API token: billed per token)" if v.get("kind") == "api_key" else ""))
else:
    print("  usage: %d%% (%s)" % (mp, " · ".join("%s %d%%" % (w.get("label", "?"), pct(w)) for w in wins)))
    if v.get("updatedAt"):
        print("  read at %s" % v["updatedAt"])
ns = v.get("nextStart")
if ns:
    print("  from its next start: %s (%s)" % (ns.get("accountId") or "the server default", ns.get("source")))
if "refreshed" in v and not v.get("refreshed"):
    r = v.get("reason") or "-"
    if r == "rate_limited":
        print("  not re-read: read less than a minute ago (try in %ss)" % v.get("retryAfterSecs"))
    else:
        print("  not re-read (%s)" % r)
' "${pos[0]}"
            ;;
        pin|unpin)
            if [ -n "$pin_ws" ] && [ -n "$pin_sess" ]; then
                echo "usage: pass --workspace or --session, not both" >&2; exit 2
            fi
            local scope="" scope_id=""
            local scope_word=""
            if [ -n "$pin_ws" ]; then scope="workspace"; scope_id="$pin_ws"
            elif [ -n "$pin_sess" ]; then scope="session"; scope_id="$pin_sess"
            else
                echo "usage: k2 llm tokens $sub --workspace <ws>|--session <key> <tool>$([ "$sub" = pin ] && echo ' <label>')" >&2
                exit 2
            fi
            scope_word="$scope"
            [ "$scope" = "session" ] && scope_word="chat"
            if [ "$sub" = "pin" ]; then
                [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens pin --workspace <ws>|--session <key> <tool> <label>" >&2; exit 2; }
                _llma_key_tool_ok "${pos[0]}"
                local pid
                pid="$(_llma_resolve "${pos[0]}" "${pos[1]}")" || exit $?
                _llma_post pin "$(_llma_json scope "$scope" scopeId "$scope_id" tool "${pos[0]}" id "$pid")"
                if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v.get("pin") or {}
print("%s %s now uses %s token %s" % ("chat" if p.get("scopeKind") == "session" else p.get("scopeKind"), p.get("label") or p.get("scopeId"), p.get("tool"), sys.argv[1]))
if v.get("note"):
    print(v["note"])
' "${pos[1]}"
            else
                [ "${#pos[@]}" -eq 1 ] || { echo "usage: k2 llm tokens unpin --workspace <ws>|--session <key> <tool>" >&2; exit 2; }
                _llma_key_tool_ok "${pos[0]}"
                _llma_post unpin "$(_llma_json scope "$scope" scopeId "$scope_id" tool "${pos[0]}")"
                if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
                if printf '%s' "$_SC_BODY" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("unpinned") else 1)'; then
                    echo "${scope_word} ${scope_id} ${pos[0]}: back to Server default"
                else
                    echo "${scope_word} ${scope_id} has no ${pos[0]} token of its own (it uses Server default)"
                fi
            fi
            ;;
        add-key)
            if [ "${#pos[@]}" -gt 2 ]; then
                echo "usage: the API token is read from stdin, never an argument — k2 llm tokens add-key <tool> <label>" >&2
                exit 2
            fi
            [ "${#pos[@]}" -eq 2 ] || { echo "usage: k2 llm tokens add-key <tool> <label>   (the API token is read from stdin)" >&2; exit 2; }
            _llma_key_tool_ok "${pos[0]}"
            local api_key=""
            if [ -t 0 ]; then
                printf 'Paste the API token: ' >&2
                IFS= read -rs api_key || api_key=""
                echo >&2
            else
                IFS= read -r api_key || true
            fi
            if [ -z "$api_key" ]; then
                echo "usage: no API token on stdin" >&2
                exit 2
            fi
            # The key reaches python on stdin and curl in its config on
            # stdin (_sidecar_send): never on any process's argv, never printed.
            local key_body
            key_body="$(printf '%s' "$api_key" | python3 -c '
import json, sys
print(json.dumps({"tool": sys.argv[1], "label": sys.argv[2], "key": sys.stdin.read().strip()}))
' "${pos[0]}" "${pos[1]}")"
            api_key=""
            _llma_post add-key "$key_body"
            key_body=""
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            echo "added ${pos[0]} API token '${pos[1]}' (billed per token; the key is not shown again)"
            echo "Use it: k2 llm tokens pin --workspace <ws> ${pos[0]} '${pos[1]}', or make it the server default (k2 llm tokens switch)."
            _setup_kessel_hint
            ;;
        *)
            echo "usage: unknown llm tokens verb '$sub' (list|status|usage|add|add-key|login|switch|next|rename|remove|refresh|pin|unpin|pins|for — k2 llm tokens --help)" >&2
            exit 2
            ;;
    esac
}

_llma_render_switch() {
    printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
s = v.get("switch") or {}
a = v.get("account") or {}
tool = s.get("tool") or a.get("tool")
if s.get("alreadyActive"):
    print("%s: %s is already the server default" % (tool, a.get("label") or s.get("to")))
else:
    was = (" (was %s)" % s["fromLabel"]) if s.get("fromLabel") else ""
    print("%s: server default is now %s%s" % (tool, a.get("label") or s.get("toLabel") or s.get("to"), was))
    for w in v.get("warnings") or []:
        print("WARN: %s" % w)
    for d in s.get("adopted") or []:
        if d.get("created"):
            print("Saved a sign-in made outside K2 (%s) as the new token \"%s\"; the token \"%s\" was left as it was." % (d.get("account"), d.get("label"), d.get("foundUnder")))
        else:
            print("Saved a sign-in made outside K2 (%s) to its own token \"%s\"." % (d.get("account"), d.get("label")))
    for n in s.get("needsSignIn") or []:
        print("\"%s\" held another account'"'"'s sign-in; it needs signing in again (k2 llm tokens login %s \"%s\")." % (n, tool, n))
    if tool == "claude" and not s.get("profileSwapped"):
        print("Note: ~/.claude.json was not updated, so Claude /status may still name the previous account.")
    print("Changing the server default token affects every chat that uses Server default on this server.")
'
}

