# ── k2 usage (prd-usage-v2 §5) ───────────────────────────────────────
# Token usage and subscription limits on this server. An `id` tool: in a
# K2 terminal it sends the session passport, so an agent sees its own
# workspace's tokens / turns and read-only limits; outside K2 the owner
# token sees everything. Members are refused by the daemon (exit 3).

_usage_cli_usage() {
    cat <<'EOF'
k2 usage — token usage and subscription limits on this server (Owner and Admin).

  k2 usage tokens [--workspace PATH|--here] [--outside] [--harness H] [--login ID]
                  [--model M] [--since 7d] [--until T]
                  [--by workspace|model|login|harness] [--tz ZONE] [--json]
                                  totals, estimated cost, days, and a breakdown
  k2 usage turns [--limit N] [--workspace PATH|--here] [--harness H] [--login ID]
                 [--since T] [--before CURSOR] [--json]
                                  the newest requests first
  k2 usage limits [--refresh] [--tool claude|codex|grok] [--login ID] [--json]
                                  every login's subscription windows

Times: 7d, 24h, 90m, an RFC3339 time, or unix ms. --tz defaults to this
computer's zone. Input is uncached input; output includes reasoning; one
request is one model call (Grok: one prompt). Cost is an estimate at API
list prices, not your bill.
In a K2 terminal (an agent) you see your own workspace only, and limits are
read-only (--refresh needs a terminal outside K2).
EOF
}

# This computer's IANA zone (TZ, /etc/localtime, /etc/timezone), else UTC.
_usage_local_tz() {
    local z="${TZ:-}"
    z="${z#:}"
    if [ -z "$z" ] && [ -L /etc/localtime ]; then
        z="$(readlink /etc/localtime 2>/dev/null)"
        z="${z##*zoneinfo/}"
    fi
    if [ -z "$z" ] && [ -r /etc/timezone ]; then
        z="$(head -n 1 /etc/timezone 2>/dev/null)"
    fi
    case "$z" in ''|/*|*' '*) z="UTC" ;; esac
    printf '%s' "$z"
}

# GET a usage route; sets _SC_BODY/_SC_STATUS, exits on a refusal.
_usage_get() {
    local path="$1" query="${2:-}"
    _sidecar_split "$(_sidecar_send GET "/cli/usage/${path}" "$query")"
    [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
}

cmd_usage() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _usage_cli_usage; return 0 ;;
    esac
    shift
    if _wants_help "$@"; then
        _usage_cli_usage
        return 0
    fi
    local as_json=0 refresh=0 here=0 outside=0
    local ws="" harness="" login="" model="" since="" until="" by="" tz="" limit="" before="" tool=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --refresh) refresh=1; shift ;;
            --here) here=1; shift ;;
            --outside) outside=1; shift ;;
            --workspace|--harness|--login|--model|--since|--until|--by|--tz|--limit|--before|--tool)
                [ -n "${2:-}" ] || { echo "usage: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --workspace) ws="$2" ;;
                    --harness) harness="$2" ;;
                    --login) login="$2" ;;
                    --model) model="$2" ;;
                    --since) since="$2" ;;
                    --until) until="$2" ;;
                    --by) by="$2" ;;
                    --tz) tz="$2" ;;
                    --limit) limit="$2" ;;
                    --before) before="$2" ;;
                    --tool) tool="$2" ;;
                esac
                shift 2 ;;
            --*) echo "usage: unknown flag '$1' (k2 usage --help)" >&2; exit 2 ;;
            *) echo "usage: unexpected argument '$1' (k2 usage --help)" >&2; exit 2 ;;
        esac
    done
    if [ "$here" = "1" ]; then
        [ -z "$ws" ] || { echo "usage: --here and --workspace are the same thing; pick one" >&2; exit 2; }
        ws="${K2_PROJECT_PATH:-$PWD}"
    fi
    if [ -n "$ws" ] && [ "$outside" = "1" ]; then
        echo "usage: --workspace and --outside cannot both be set" >&2; exit 2
    fi
    case "$harness" in ""|claude|codex|grok) ;; *) echo "usage: --harness must be claude, codex, or grok" >&2; exit 2 ;; esac
    case "$tool" in ""|claude|codex|grok) ;; *) echo "usage: --tool must be claude, codex, or grok" >&2; exit 2 ;; esac
    case "$by" in ""|workspace|model|login|harness) ;; *) echo "usage: --by must be workspace, model, login, or harness" >&2; exit 2 ;; esac
    case "$limit" in ""|[1-9]|[1-9][0-9]|[1-9][0-9][0-9]) ;; *) echo "usage: --limit must be 1-200" >&2; exit 2 ;; esac
    local q=""
    _uq() { [ -n "$2" ] && q="${q}${q:+&}$1=$(urlencode "$2")"; return 0; }
    case "$sub" in
        tokens)
            if [ "$refresh" = "1" ] || [ -n "$before$tool$limit" ]; then
                echo "usage: k2 usage tokens takes no --refresh / --before / --tool / --limit (k2 usage --help)" >&2; exit 2
            fi
            [ -n "$tz" ] || tz="$(_usage_local_tz)"
            _uq workspace "$ws"; _uq harness "$harness"; _uq account "$login"; _uq model "$model"
            _uq since "$since"; _uq until "$until"; _uq group_by "$by"; _uq tz "$tz"
            [ "$outside" = "1" ] && _uq outside 1
            _usage_get tokens "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys
r = json.load(sys.stdin)
def n(x):
    x = int(x or 0)
    for unit, size in (("B", 10**9), ("M", 10**6), ("k", 10**3)):
        if x >= size:
            return "%.1f%s" % (x / size, unit)
    return str(x)
def cost(t):
    c = t.get("cost")
    if not c:
        return ""
    s = "$%.2f" % c.get("usd", 0)
    if c.get("unpriced_requests"):
        s += " (+%d unpriced)" % c["unpriced_requests"]
    return s
def line(label, t):
    return "  %-32s %8s req  in %7s  out %7s  cache r %7s  w %7s  %s" % (
        label[:32], n(t.get("requests")), n(t.get("input_tokens")), n(t.get("output_tokens")),
        n(t.get("cache_read_tokens")), n(t.get("cache_write_tokens")), cost(t))
scope = r.get("workspace") or "this server"
print("Usage on %s (days in %s)" % (scope, r.get("tz")))
t = r.get("total") or {}
print(line("Total", t))
c = t.get("cost")
if c:
    print("  Estimated API-equivalent cost: $%.2f — an estimate, not your bill." % c.get("usd", 0))
groups = r.get("groups")
if groups is None:
    groups = [{"label": h["harness"], **h} for h in r.get("harnesses") or []]
if groups:
    print("By %s:" % (sys.argv[1] or "harness"))
    for g in groups:
        print(line(g.get("label") or g.get("key") or "-", g))
days = (r.get("days") or [])[-14:]
if days:
    print("Days:")
    for d in days:
        print(line(d["day"], d))
print("Input is uncached; output includes reasoning.")
' "$by"
            ;;
        turns)
            if [ "$refresh" = "1" ] || [ "$outside" = "1" ] || [ -n "$tool$by$tz$until$model" ]; then
                echo "usage: k2 usage turns [--limit N] [--workspace PATH|--here] [--harness H] [--login ID] [--since T] [--before CURSOR] [--json]" >&2; exit 2
            fi
            _uq workspace "$ws"; _uq harness "$harness"; _uq account "$login"; _uq since "$since"
            _uq limit "$limit"; _uq before "$before"
            _usage_get turns "$q"
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys, time
p = json.load(sys.stdin)
rows = p.get("rows") or []
if not rows:
    print("(no requests)")
for r in rows:
    ms = r.get("recorded_ms")
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(ms / 1000)) if ms else "-"
    ws = (r.get("workspace") or r.get("workspace_path") or "-").rstrip("/").split("/")[-1] or "-"
    c = r.get("cost_usd")
    cost = ("$%.4f" % c) if c is not None else "unpriced"
    print("%s  %-6s %-24s %-18s in %-7s out %-6s cr %-8s cw %-6s %s" % (
        when, r.get("harness"), (r.get("model") or "-")[:24], ws[:18], r.get("input_tokens"), r.get("output_tokens"),
        r.get("cache_read_tokens"), r.get("cache_write_tokens"), cost))
if p.get("next_cursor"):
    print("older: k2 usage turns --before %s" % p["next_cursor"])
'
            ;;
        limits)
            if [ "$outside" = "1" ] || [ -n "$ws$harness$model$since$until$by$tz$limit$before" ]; then
                echo "usage: k2 usage limits [--refresh] [--tool claude|codex|grok] [--login ID] [--json]" >&2; exit 2
            fi
            if [ "$refresh" = "1" ]; then
                local body
                body="$(python3 -c 'import json,sys; d={}; t,a=sys.argv[1],sys.argv[2]; d.update({"tool": t} if t else {}); d.update({"accountId": a} if a else {}); print(json.dumps(d))' "$tool" "$login")"
                _sidecar_split "$(_sidecar_send POST "/cli/usage/limits/refresh" "$body")"
                [ "$_SC_STATUS" = "200" ] || _sidecar_fail "$_SC_STATUS" "$_SC_BODY"
            else
                _usage_get limits ""
            fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$_SC_BODY"; exit 0; fi
            printf '%s' "$_SC_BODY" | python3 -c '
import json, sys, datetime
d = json.load(sys.stdin)
want_tool, want_login = sys.argv[1], sys.argv[2]
now = datetime.datetime.now(datetime.timezone.utc)
def parse(s):
    try:
        return datetime.datetime.fromisoformat((s or "").replace("Z", "+00:00"))
    except ValueError:
        return None
def ago(s):
    t = parse(s)
    if not t:
        return "never checked"
    m = int((now - t).total_seconds() // 60)
    return "updated %s ago" % ("%dm" % m if m < 120 else "%dh" % (m // 60))
def until(s):
    t = parse(s)
    if not t:
        return ""
    m = int((t - now).total_seconds() // 60)
    if m <= 0:
        return "reset — re-checking"
    return "resets in %s" % ("%dm" % m if m < 120 else ("%dh" % (m // 60) if m < 2880 else "%dd" % (m // 1440)))
STATES = {"not_signed_in": "not signed in", "expired": "sign-in needs a refresh (k2 llm tokens login)",
          "probe_failed": "last check failed", "unsupported": "no limits for this login", "airgap": "off in air-gap mode",
          "no_window": "no usage window"}
if d.get("airgap"):
    print("Air-gap mode is on: limits are not checked.")
r = d.get("refresh") or {}
if r.get("retryAfterSecs"):
    why = "read less than a minute ago" if r.get("outcome") == "too_soon" else "backing off after a failed check"
    print("Not re-read: %s (try in %ss)." % (why, r["retryAfterSecs"]))
for t in d.get("tools") or []:
    if want_tool and t.get("tool") != want_tool:
        continue
    logins = [l for l in t.get("logins") or [] if not want_login or l.get("accountId") == want_login or l.get("label") == want_login]
    if not t.get("supported"):
        if logins:
            print("%s: no limits API" % t.get("display"))
        continue
    print("%s:" % t.get("display"))
    if not logins:
        print("  (not signed in)")
    for l in logins:
        mark = "*" if l.get("isDefault") else " "
        wins = []
        for w in l.get("windows") or []:
            pct = w.get("usedPercent")
            if pct is None:
                pct = float(w.get("used") or 0) * 100
            u = until(w.get("resetsAt"))
            wins.append("%s %d%%%s" % (w.get("label"), round(pct), (" (" + u + ")") if u else ""))
        state = l.get("state") or "ok"
        note = STATES.get(state, "")
        if state == "probe_failed" and l.get("error"):
            note += ": " + l["error"]
        pinned = " [pinned]" if l.get("pinned") else ""
        src = " · live" if (l.get("source") or "").startswith("live") else ""
        print("%s %-22s %s%s" % (mark, (l.get("label") or "-")[:22], " · ".join(wins) or "-", pinned))
        print("    %s%s%s" % (ago(l.get("checkedAt")), src, (" · " + note) if note else ""))
print("* = the server default login.")
' "$tool" "$login"
            ;;
        *)
            echo "usage: unknown usage verb '$sub' (k2 usage --help)" >&2
            exit 2
            ;;
    esac
}

cmd_llm() {
    local sub="${1:-}"
    case "$sub" in
        ""|help|--help|-h) _llm_accounts_usage; return 0 ;;
        # `tokens` is the noun; `accounts` stays a working alias.
        tokens|token|accounts|account) shift; cmd_llm_accounts "$@" ;;
        *)
            echo "usage: unknown llm verb '$sub' (k2 llm tokens --help)" >&2
            exit 2
            ;;
    esac
}

cmd_reserve() {
    local agent="${K2SO_AGENT_NAME:-}"
    local paths=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --agent) agent="$2"; shift 2 ;;
            *) paths+=("$1"); shift ;;
        esac
    done
    [[ -z "$agent" ]] && { echo "Error: --agent required (or set K2SO_AGENT_NAME)" >&2; exit 1; }
    [[ ${#paths[@]} -eq 0 ]] && { echo "Error: at least one path required" >&2; exit 1; }
    local joined=$(IFS=,; echo "${paths[*]}")
    cli_request_post "/cli/reserve" "agent=$agent&paths=$(urlencode "$joined")"
}

cmd_release() {
    local agent="${K2SO_AGENT_NAME:-}"
    local paths=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --agent) agent="$2"; shift 2 ;;
            *) paths+=("$1"); shift ;;
        esac
    done
    [[ -z "$agent" ]] && { echo "Error: --agent required (or set K2SO_AGENT_NAME)" >&2; exit 1; }
    local params="agent=$agent"
    if [[ ${#paths[@]} -gt 0 ]]; then
        local joined=$(IFS=,; echo "${paths[*]}")
        params="$params&paths=$(urlencode "$joined")"
    fi
    cli_request_post "/cli/release" "$params"
}

cmd_connections() {
    # #37: `k2 connections --json` (no subcommand) == list --json
    # Same rewrite for `--users` so `k2 connections --users` lists people.
    if [[ "${1:-}" == "--json" || "${1:-}" == "--users" || "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
        set -- list "$@"
    fi
    local action="${1:-list}"
    shift 2>/dev/null || true
    case "$action" in
        list)
            # 0.39.0 (workspace==agent model): connections list renders
            # a bidirectional, deduped peer list — both workspaces in a
            # connection see each other as just "connected." Direction
            # is a data-model concept (source → target rows), not a
            # user-visible distinction; the daemon-side dispatch in
            # `core::connections::connections("list", ...)` returns the
            # deduped Peer shape (`projectId`, `projectName`,
            # `relationTypes`) — no `direction` / `type` fields.
            # 0.39.45 (#31): print WHOSE connections these are (the
            # daemon-resolved source workspace) so an inherited/parent
            # resolution is self-evident, and flag unreachable peers.
            # #37: honor --json (same contract as tunnel/dns/mail status).
            local as_json=false include_users=false
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --json) as_json=true; shift ;;
                    --users) include_users=true; shift ;;
                    --help|-h) cmd_help_connections; return 0 ;;
                    -*)
                        echo "Error: unknown flag: $1" >&2
                        echo "Usage: k2 connections list [--json] [--users]" >&2
                        exit 2
                        ;;
                    *)
                        echo "Error: unexpected argument: $1" >&2
                        echo "Usage: k2 connections list [--json] [--users]" >&2
                        exit 2
                        ;;
                esac
            done
            # Prefer UDS when present so scoped agents list their own mesh.
            local raw
            if $include_users; then
                raw=$(cli_send cli_request "/cli/connections" "action=list" "users=1")
            else
                raw=$(cli_send cli_request "/cli/connections" "action=list")
            fi
            AS_JSON="$as_json" INCLUDE_USERS="$include_users" python3 -c "
import json, sys, os
raw = sys.stdin.read()
as_json = os.environ.get('AS_JSON') == 'true'
try:
    d = json.loads(raw)
except Exception:
    sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])
    sys.exit(1)
err = d.get('error')
if err is not None:
    if isinstance(err, dict):
        sys.stderr.write(json.dumps({'error': err}, ensure_ascii=False) + '\n')
    else:
        sys.stdout.write('Error: %s\n' % err)
    sys.exit(1)
ws = d.get('workspace') or {}
conns = d.get('connections', []) or []
include_users = os.environ.get('INCLUDE_USERS') == 'true'
if as_json:
    out = {
        'ok': True,
        'workspace': {
            'name': ws.get('name') or '',
            'path': ws.get('path') or '',
            'id': ws.get('id') or ws.get('projectId') or '',
        },
        'connections': [],
    }
    for c in conns:
        if c.get('remote'):
            addr = c.get('address') or ('%s::%s' % (c.get('agent', '?'), c.get('host', '?')))
            out['connections'].append({
                'name': addr,
                'kind': 'remote',
                'address': addr,
                'agent': c.get('agent'),
                'host': c.get('host'),
                # A7: the peer workspace this row is bound to (null until
                # a roster fetch binds it) and the names its roster showed.
                'remoteWorkspaceId': c.get('remoteWorkspaceId'),
                'handle': c.get('handle'),
                'displayName': c.get('displayName'),
                # 0159: the bound workspace left the peer's roster.
                'remoteWorkspaceGone': bool(c.get('remoteWorkspaceGone')),
            })
        else:
            out['connections'].append({
                'name': c.get('projectName') or c.get('name') or '?',
                'kind': 'local',
                'projectId': c.get('projectId') or c.get('id'),
                'reachable': c.get('reachable', True),
            })
    if include_users:
        users = d.get('users')
        if not isinstance(users, list):
            sys.stdout.write('Error: daemon omitted users[] (asked with users=1)\\n')
            sys.exit(1)
        out['users'] = []
        for u in users:
            if not isinstance(u, dict):
                continue
            out['users'].append({
                'username': u.get('username') or '',
                'role': u.get('role') or '',
                'disabled': bool(u.get('disabled')),
                # 0.46.0: display only — address people by username.
                'firstName': u.get('firstName'),
                'lastName': u.get('lastName'),
                'displayName': u.get('displayName') or u.get('username') or '',
            })
    if 'thisWorkspace' in d:
        out['thisWorkspace'] = d.get('thisWorkspace')
    print(json.dumps(out, ensure_ascii=False, separators=(',', ':')))
    raise SystemExit(0)
label = ws.get('name') or ws.get('path') or '?'
# WS-C2: who is working in THIS workspace comes first (k2 sessions).
tw = d.get('thisWorkspace')
if isinstance(tw, dict) and not include_users:
    if tw.get('error'):
        sys.stderr.write('This workspace: (could not read sessions: %s)\\n' % tw.get('error'))
    else:
        rows = tw.get('sessions') or []
        print('This workspace (%s): %d live' % (tw.get('handle') or label, len(rows)))
        for r in rows:
            title = (r.get('title') or '').replace('\\n', ' ')
            if len(title) > 50:
                title = title[:49] + '…'
            print('  %s %-16s %-9s %-7s %-8s %s' % ('*' if r.get('you') else ' ', r.get('address') or r.get('handle') or '-',
                  r.get('role') or '-', r.get('cli') or '-', r.get('state') or '-', title))
        tail = '  * = you. ' if any(r.get('you') for r in rows) else '  '
        print(tail + 'Same workspace: always allowed to k2 msg. More: k2 sessions (--past for older chats)')
    print('')
print('Connections for %s:' % label)
if not conns:
    print('  (none)')
for c in conns:
    if c.get('remote'):
        addr = c.get('address') or ('%s::%s' % (c.get('agent', '?'), c.get('host', '?')))
        # A7: a bound row shows the workspace it reaches, e.g.
        # seoca::quahog.k2.dev → quahog-website (Seoca).
        handle = c.get('handle') or ''
        display = c.get('displayName') or ''
        bound = ''
        if c.get('remoteWorkspaceId') and handle:
            bound = ' → %s' % handle
            if display and display.strip().lower() != handle.strip().lower():
                bound += ' (%s)' % display
        repair = '  [older pairing code: messages still work; re-pair recommended]' if c.get('repairRecommended') else ''
        # 0159: never moves to another workspace that takes the name.
        gone = '  [GONE: that workspace is no longer on the host; handles are never reused, so this connection stays closed until your human adds it again]' if c.get('remoteWorkspaceGone') else ''
        print('  - %s%s   (remote)%s%s' % (addr, bound, repair, gone))
    else:
        flag = '' if c.get('reachable', True) else '  [UNREACHABLE — project missing or path gone]'
        print('  - %s%s' % (c.get('projectName', '?'), flag))
if include_users:
    users = d.get('users')
    if not isinstance(users, list):
        sys.stdout.write('Error: daemon omitted users[] (asked with users=1)\\n')
        sys.exit(1)
    print('')
    print('People on this host (humans — not workspace-agents; do not k2 msg):')
    rows = []
    for u in users:
        if not isinstance(u, dict):
            continue
        # 0.46.0: the real name when set (display only; use the username).
        named = bool(u.get('firstName') or u.get('lastName'))
        rows.append((
            str(u.get('username') or '?'),
            (str(u.get('displayName') or '') if named else '') or '-',
            str(u.get('role') or '?'),
            'yes' if u.get('disabled') else 'no',
        ))
    if not rows:
        print('  (none)')
    else:
        w_user = max([len(r[0]) for r in rows] + [len('USERNAME')])
        w_name = max([len(r[1]) for r in rows] + [len('NAME')])
        w_role = max([len(r[2]) for r in rows] + [len('ROLE')])
        print('%s  %s  %s  %s' % ('USERNAME'.ljust(w_user), 'NAME'.ljust(w_name), 'ROLE'.ljust(w_role), 'DISABLED'))
        for un, nm, role, dis in rows:
            print('%s  %s  %s  %s' % (un.ljust(w_user), nm.ljust(w_name), role.ljust(w_role), dis))
else:
    print('These are workspace-agents you can k2 msg. Humans on this box are not listed — k2 connections list --users (do not k2 msg those names).')
" <<< "$raw"
            ;;
        add)
            local target="${1:-}"
            [[ -z "$target" ]] && { echo "Usage: k2 connections add <workspace-name>" >&2; exit 1; }
            # 0.39.45 (#32): NEVER exit silently. Errors used to go to
            # stderr and then get eaten by a 2>/dev/null on this pipe —
            # a failed add looked identical to success with no output.
            # C1 (0.40.45): gated agents get exit 3 + JSON teaching
            # Settings path (agents_create_connections_disabled).
            cli_request_post "/cli/connections" "action=add&target=$(urlencode "$target")" | python3 -c "
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])
    sys.exit(1)
err = d.get('error')
if err is not None:
    code, hint = '', ''
    if isinstance(err, dict):
        code = err.get('code') or ''
        hint = err.get('hint') or json.dumps(err)
    else:
        hint = str(err)
    gated = (
        code in ('agents_create_connections_disabled', 'gated', 'forbidden')
        or 'Allow agents to create connections' in hint
        or \"isn't allowed to create or remove connections\" in hint
    )
    if gated:
        if 'Settings' not in hint:
            hint = (hint + ' — ' if hint else '') + (
                \"this agent isn't allowed to create or remove connections — \"
                'the owner can enable it in Settings \u2192 K2 Connect '
                '(Allow agents to create connections) or Workspaces \u2192 '
                '(workspace) \u2192 Allow agents to create connections'
            )
        sys.stderr.write(json.dumps({'error': {'code': code or 'agents_create_connections_disabled', 'hint': hint}}, ensure_ascii=False) + '\n')
        sys.exit(3)
    sys.stdout.write('Error: %s\n' % hint)
    sys.exit(1)
if d.get('noop'):
    print('Already connected to %s (%s)' % (d.get('target', '?'), d.get('message', '')))
else:
    print('Connected to %s' % d.get('target', '?'))
# A7 / CA8: a remote add checks the peer's roster when it can. Bound →
# say which workspace; otherwise warn (the connection is still saved).
if d.get('bound') and d.get('handle'):
    host = (d.get('target') or '').rpartition('::')[2]
    label = d.get('handle') + ('::' + host if host else '')
    if d.get('displayName') and d['displayName'].strip().lower() != d['handle'].strip().lower():
        label += ' (%s)' % d['displayName']
    print('  reaches %s' % label)
if d.get('bindWarning'):
    sys.stderr.write('warning: %s\n' % d['bindWarning'])
if d.get('warning'):
    sys.stderr.write('warning: %s\n' % d['warning'])
"
            ;;
        remove)
            local target="${1:-}"
            [[ -z "$target" ]] && { echo "Usage: k2 connections remove <workspace-name>" >&2; exit 1; }
            # C1 (0.40.45): same gated exit-3 teaching as add.
            cli_request_post "/cli/connections" "action=remove&target=$(urlencode "$target")" | python3 -c "
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    sys.stdout.write('Error: daemon returned an unparseable response: %r\n' % raw[:200])
    sys.exit(1)
err = d.get('error')
if err is not None:
    code, hint = '', ''
    if isinstance(err, dict):
        code = err.get('code') or ''
        hint = err.get('hint') or json.dumps(err)
    else:
        hint = str(err)
    gated = (
        code in ('agents_create_connections_disabled', 'gated', 'forbidden')
        or 'Allow agents to create connections' in hint
        or \"isn't allowed to create or remove connections\" in hint
    )
    if gated:
        if 'Settings' not in hint:
            hint = (hint + ' — ' if hint else '') + (
                \"this agent isn't allowed to create or remove connections — \"
                'the owner can enable it in Settings \u2192 K2 Connect '
                '(Allow agents to create connections) or Workspaces \u2192 '
                '(workspace) \u2192 Allow agents to create connections'
            )
        sys.stderr.write(json.dumps({'error': {'code': code or 'agents_create_connections_disabled', 'hint': hint}}, ensure_ascii=False) + '\n')
        sys.exit(3)
    sys.stdout.write('Error: %s\n' % hint)
    sys.exit(1)
print('Disconnected.')
"
            ;;
        *)
            echo "Usage: k2 connections [list|add|remove] [workspace-name] [--json] [--users]" >&2
            echo "  list [--json] [--users]  List connections for this workspace (exit 0)" >&2
            echo "  add <name>        Connect (agents need Settings → Allow agents to create connections; exit 3 when gated)" >&2
            echo "  remove <name>     Disconnect (same agent create/remove gate as add; exit 3 when gated)" >&2
            exit 1
            ;;
    esac
}

# POST to a query-string route (no JSON body). The daemon parses the
# token + params from the query; the body is empty/drained. Used by the
# tunnel start/stop control routes (mutating → POST-allowlisted, with a
# per-handler `require_post` gate on the daemon side).
cli_post() {
    local endpoint="$1"; shift
    local params="token=${TOKEN}&project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    local _K2_CFG=""
    _k2_cfg_add url "${BASE_URL}${endpoint}?${params}"
    _k2_curl "$_K2_CFG" -s -X POST \
        --connect-timeout 5 --max-time 30 \
        --data-raw "" 2>/dev/null
}

# UDS twin of cli_post: query-string POST over the cell socket. Omit
# token= / project= — Bearer + stamp_principal own identity.
cli_post_uds() {
    local endpoint="$1"; shift
    local params=""
    for param in "$@"; do
        params="${params}${params:+&}${param}"
    done
    local _K2_CFG=""
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    _k2_curl "$_K2_CFG" -s -X POST --unix-socket "$(_hook_sock)" "http://localhost${endpoint}${params:+?${params}}" \
        --connect-timeout 5 --max-time 30 \
        --data-raw "" 2>/dev/null
}

# K2 Connect tunnel — expose this workspace's daemon at
# https://<user>.k2.dev by running an frpc client that dials the hosted
# K2 Connect server. Routes:
#   POST /cli/tunnel/start[?subdomain=<s>]   GET /cli/tunnel/status   POST /cli/tunnel/stop
#   GET /cli/tunnel/subdomains — the daemon's cached nested-subdomain
#   routing map (label -> internal target); read-only, distinct from
#   `k2 publish subdomain list` which asks the CONTROL PLANE directly.
# Print a /cli/tunnel/release response (release or forget). 0.45.2: a
# release reports upstream only when this device's tunnel is connected (it
# holds the name); otherwise it is local only, and nothing is queued.
_k2_tunnel_release_print() {
    printf '%s' "$1" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    err = d["error"]
    print(f"Failed to release tunnel identity: {err}", file=sys.stderr); sys.exit(1)
sub = (d.get("subdomain") or "").strip()
label = f"{sub}.k2.dev" if sub else "its subdomain"
if d.get("alreadyReleased"):
    print(f"This device was already released from {label}.")
elif d.get("localOnly"):
    print(f"Forgotten on this device: {label} no longer routes here.")
else:
    print(f"Released: this device is unpaired from {label}.")
if d.get("upstreamReported"):
    print("K2 Connect recorded the release.")
elif d.get("queued"):
    print("K2 Connect was unreachable; the release is queued and replays on the next daemon boot.")
note = d.get("note")
if note:
    print(f"Note: {note}")
print("Your account still owns the subdomain; pair it on any device any time.")
'
}

cmd_tunnel() {
    local sub="${1:-status}"
    shift || true
    case "$sub" in
        start)
            local subdomain=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --subdomain) subdomain="$2"; shift 2 ;;
                    *) shift ;;
                esac
            done
            local extra=""
            [ -n "$subdomain" ] && extra="subdomain=$(urlencode "$subdomain")"
            cli_post "/cli/tunnel/start" $extra | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    err = d["error"]
    print(f"Failed to start tunnel: {err}", file=sys.stderr); sys.exit(1)
url = d.get("public_url") or ""
# 0.45.6: a "running" tunnel whose frpc was failing (or whose config was
# gone) is repaired, not reported as already started.
if d.get("repaired"):
    print("Tunnel was running but not serving: " + d["repaired"])
    print("Re-rendered the frpc config and restarted frpc.")
print("Tunnel started." + (f" Public URL: {url}" if url else " (subdomain will be assigned by the server)"))
'
            ;;
        stop)
            cli_post "/cli/tunnel/stop" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("ok"):
    print("Tunnel stopped.")
else:
    err = d.get("error") or "unknown error"
    print(f"Failed to stop tunnel: {err}", file=sys.stderr); sys.exit(1)
'
            ;;
        # PRD tunnel-disable-unpair §2A — the persisted PAUSE. Unlike
        # `stop` (this daemon session only), `disable` writes
        # `enabled:false` into ~/.k2/tunnel.json, so daemon restarts,
        # reboots, and orphaned daemons all keep the tunnel dark until
        # `enable`. Identity and subdomain are untouched.
        disable)
            cli_post "/cli/tunnel/disable" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    err = d["error"]
    print(f"Failed to disable tunnel: {err}", file=sys.stderr); sys.exit(1)
print("Tunnel disabled (persisted — survives daemon restarts and reboots).")
print("Re-enable with: k2 tunnel enable")
'
            ;;
        # 0.45.2: `enable` also persists auto-start (re-launch on daemon
        # boot / reboot) unless --no-auto-start — before, a box with
        # auto_start:false said "enabled and running" and stayed dark after
        # a reboot. Same tunnel.json field as Settings' "Re-launch tunnel on
        # restart" toggle.
        enable)
            _k2_refuse_if_airgap
            local subdomain="" auto_start="1"
            while [ $# -gt 0 ]; do
                case "$1" in
                    --subdomain) subdomain="$2"; shift 2 ;;
                    --no-auto-start) auto_start="0"; shift ;;
                    --auto-start) auto_start="1"; shift ;;
                    *) shift ;;
                esac
            done
            local extra="auto_start=$auto_start"
            [ -n "$subdomain" ] && extra="$extra&subdomain=$(urlencode "$subdomain")"
            cli_post "/cli/tunnel/enable" "$extra" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    err = d["error"]
    print(f"Failed to enable tunnel: {err}", file=sys.stderr); sys.exit(1)
if d.get("running"):
    url = d.get("public_url") or "(subdomain assigned by the server)"
    print(f"Tunnel enabled and running. Public URL: {url}")
else:
    print("Tunnel enabled (persisted). Not running yet — pair a subdomain in")
    print("Settings -> K2 Connect (or run: k2 tunnel start).")
if d.get("autoStart") is True:
    print("Auto-start: on (comes back by itself after a daemon restart or reboot).")
elif d.get("autoStart") is False:
    print("Auto-start: off (after a restart, run k2 tunnel enable again).")
'
            ;;
        # 0.45.2: move this device to another subdomain in one step. The
        # daemon binds the new name, moves the encrypted listener, its
        # certificate and renewer, the nested routes and frpc, and rolls
        # back to the old name if any step fails. No daemon restart.
        switch)
            _k2_refuse_if_airgap
            local subdomain="" token="" start=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --token) token="$2"; shift 2 ;;
                    --start) start="true"; shift ;;
                    --no-start) start="false"; shift ;;
                    -*) echo "k2 tunnel switch: unknown flag $1" >&2; exit 2 ;;
                    *) subdomain="$1"; shift ;;
                esac
            done
            if [ -z "$subdomain" ]; then
                echo "usage: k2 tunnel switch <subdomain> [--token <tunnel token>] [--start|--no-start]" >&2
                exit 2
            fi
            local body
            body=$(SUB="$subdomain" TOK="$token" START="$start" python3 -c '
import json, os
b = {"subdomain": os.environ["SUB"]}
if os.environ.get("TOK"): b["token"] = os.environ["TOK"]
if os.environ.get("START"): b["start"] = os.environ["START"] == "true"
print(json.dumps(b))
')
            cli_post_json "/cli/tunnel/switch" "$body" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse daemon response: {e}", file=sys.stderr); sys.exit(1)
if d.get("error") and "steps" not in d:
    err = d["error"]
    print(f"Switch refused: {err}", file=sys.stderr); sys.exit(1)
for s in d.get("steps") or []:
    mark = "ok " if s.get("ok") else "ERR"
    step = s.get("step", "")
    detail = s.get("detail", "")
    print(f"  [{mark}] {step}: {detail}")
if d.get("ok"):
    sub = d.get("subdomain") or ""
    print(f"Switched to {sub}.k2.dev.")
else:
    err = d.get("error") or "unknown error"
    print(f"Switch failed: {err}", file=sys.stderr)
    if d.get("rolledBack"):
        print("Rolled back: the previous subdomain is still in place.", file=sys.stderr)
    sys.exit(1)
'
            ;;
        # PRD tunnel-disable-unpair §2B — the DESTRUCTIVE unpair. Deletes
        # this DEVICE's tunnel identity (tunnel.json + rendered frpc config
        # + E2E keypair), tombstones ~/.k2/unpaired.json, and reports the
        # release upstream so the relay refuses the old identity. The
        # subdomain stays owned by your ACCOUNT — attach it to another
        # device by pairing there. Requires --confirm.
        release)
            local confirmed=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --confirm) confirmed=1; shift ;;
                    *) shift ;;
                esac
            done
            if [ "$confirmed" != "1" ]; then
                local _sub
                _sub=$(python3 -c '
import json, sys
try:
    print((json.load(open(sys.argv[1])).get("subdomain") or "").strip())
except Exception:
    print("")
' "${K2_HOME}/tunnel.json" 2>/dev/null || true)
                echo "k2 tunnel release unpairs THIS DEVICE from its subdomain:" >&2
                if [ -n "$_sub" ]; then
                    echo "  - ${_sub}.k2.dev stops routing here; this copy of the identity is refused" >&2
                else
                    echo "  - this device's tunnel identity is deleted" >&2
                fi
                echo "  - K2 Connect is told only if this device's tunnel is connected (it holds the name)" >&2
                echo "  - your ACCOUNT keeps the subdomain — pair it on any device any time" >&2
                echo "  - this is NOT a pause; for that use: k2 tunnel disable" >&2
                echo "  - stale config from another machine? use: k2 tunnel forget --confirm" >&2
                echo "  - moving to another subdomain? use: k2 tunnel switch <subdomain> --token <t>" >&2
                echo "Re-run with --confirm to proceed: k2 tunnel release --confirm" >&2
                exit 1
            fi
            _k2_tunnel_release_print "$(cli_post "/cli/tunnel/release" "confirm=1")"
            ;;
        # 0.45.2: forget this device's tunnel LOCALLY — delete tunnel.json,
        # frpc.toml and the old name's certificate, send NOTHING to K2
        # Connect. For a stale config (a migrated or cloned machine) whose
        # subdomain another box holds: a release from here could otherwise
        # contest it.
        forget)
            local confirmed=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --confirm) confirmed=1; shift ;;
                    *) shift ;;
                esac
            done
            if [ "$confirmed" != "1" ]; then
                echo "k2 tunnel forget deletes this device's tunnel config (tunnel.json, frpc.toml," >&2
                echo "the certificate) and tells K2 Connect NOTHING — the subdomain and whichever" >&2
                echo "device holds it now are untouched. Re-run with --confirm." >&2
                exit 1
            fi
            _k2_tunnel_release_print "$(cli_post "/cli/tunnel/release" "confirm=1&mode=forget")"
            ;;
        status)
            local as_json=0
            [ "${1:-}" = "--json" ] && as_json=1
            local raw
            raw=$(cli_request "/cli/tunnel/status")
            if [ "$as_json" = "1" ]; then printf '%s\n' "$raw"; return 0; fi
            printf '%s' "$raw" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse tunnel status: {e}", file=sys.stderr); sys.exit(1)
print("K2 Connect tunnel")
running = "yes" if d.get("running") else "no"
# 0.45.6: "running" is the desired state; frpc_healthy says whether frpc
# is actually up (2026-10-14: "Running: yes" over a two-hour crash loop).
if d.get("running") and d.get("frpc_healthy") is False:
    n = d.get("frpc_consecutive_failures") or 0
    print(f"  Running:    yes, but frpc is FAILING ({n} consecutive failures)")
    if d.get("frpc_last_error"):
        print("  Last error: " + d["frpc_last_error"])
    if d.get("frpc_config_present") is False:
        print("  Config:     MISSING " + (d.get("frpc_config") or ""))
    print("  Repair:     k2 tunnel start")
else:
    print(f"  Running:    {running}")
# PRD connect-login-edge-only S2: who may password-login over the tunnel
# (edge = only via the K2 edge, any = pre-PRD, off = never). Pre-PRD
# daemons omit the field -> print nothing.
if d.get("loginIngress"):
    li = d["loginIngress"]
    print(f"  Login ingress: {li}")
# PRD boot-status-ghost: what strangers see on /boot-status (compat|ghost).
# Pre-0.46.3 daemons omit it -> print nothing.
if d.get("bootStatus"):
    bs = d["bootStatus"]
    note = "" if d.get("bootStatusExplicit") else " (default)"
    print(f"  Boot status: {bs}{note}")
# Tri-state truth (PRD tunnel-disable-unpair §4): never hide WHY the
# tunnel is down. Pre-PRD daemons omit the fields -> show nothing extra.
if not d.get("running"):
    if d.get("released"):
        print("  State:      RELEASED — this device is unpaired (re-pair to mint a fresh identity)")
    elif d.get("enabled") is False:
        print("  State:      DISABLED (by you, persisted) — re-enable with: k2 tunnel enable")
    # prd-box-move-carry-v1 BM-A12: a closed vault holds the token; the
    # device is still paired. Older daemons omit it.
    if d.get("token_locked"):
        print("  Token:      LOCKED — " + d["token_locked"])
if d.get("running"):
    public_url = d.get("public_url") or "(assigned by server)"
    print(f"  Public URL: {public_url}")
    # 0.46.8 (PB-6): does the relay pass the visitor address yet?
    vi = d.get("visitorIps") or {}
    if vi.get("words"):
        print("  " + vi["words"])
        if vi.get("relayAsClient"):
            print("    a relay named its own address " + str(vi["relayAsClient"]) + " times (never counted as a visitor)")
    if d.get("local_port"):
        lp = d["local_port"]
        print(f"  Local port: {lp}")
    if d.get("server_addr"):
        sa = d["server_addr"]
        # 0.45.7: port + whether this is the primary relay or a fallback.
        if d.get("server_port"):
            sa = sa + ":" + str(d["server_port"])
        if d.get("relay_primary") is True:
            sa += " (primary relay)"
        elif d.get("relay_primary") is False:
            sa += " (fallback relay; returns to the primary once it answers again)"
        print(f"  Server:     {sa}")
    # 0.46.3: the failover rules and what they are doing, e.g.
    # "waiting for DNS (standby inactive per k2.dev)". Running with 2+
    # relays only; older daemons omit it.
    fo = d.get("failover")
    if isinstance(fo, dict) and fo.get("detail"):
        print("  Failover:   " + fo["detail"])
# 0.45.7: the relay list and where it came from (fetched from k2.dev,
# provisioned, or the built-in default). Older daemons omit it.
rl = d.get("relay_list")
if isinstance(rl, dict):
    import datetime
    relays = ", ".join(rl.get("relays") or []) or "(none)"
    src = rl.get("source") or "?"
    # 0.46.6: an old built-in relay list is dialed as relay.k2.dev (in
    # memory; tunnel.json keeps the old addresses). Older daemons omit it.
    if rl.get("replaced"):
        src += "; replaces the old built-in " + ", ".join(rl["replaced"])
    print(f"  Relays:     {relays} [{src}]")
    if rl.get("relay_default"):
        by = {"env": "K2_RELAY_DEFAULT in the daemon environment, which wins over tunnel.json", "tunnel.json": "relay_default in " + (rl.get("config_file") or "tunnel.json")}.get(rl.get("relay_default_set_by"), "tunnel.json relay_default or K2_RELAY_DEFAULT")
        what = "legacy (pre-0.46.6, the old built-in IP" if rl["relay_default"] == "legacy" else rl["relay_default"] + " (relay.k2.dev, the 0.46.6 default"
        print("  Relay default: " + what + "; set by " + by + ")")
    if rl.get("config_file"):
        print("  Tunnel config: " + rl["config_file"])
    if rl.get("checked_at"):
        ts = datetime.datetime.fromtimestamp(rl["checked_at"]).strftime("%Y-%m-%d %H:%M")
        print(f"  Relay list: checked with k2.dev {ts}")
    if rl.get("active"):
        # 0.46.3: where the k2.dev DNS check says *.k2.dev points.
        print("  Live relay: *.k2.dev points at " + rl["active"] + " (per k2.dev)")
    if rl.get("last_error") and str(rl["last_error"]).startswith("401"):
        # A pre-0.46.3 daemon reports the k2.dev 401 as an error.
        print("  Relay list: k2.dev does not hand out a relay list for this tunnel token; the saved list stays in use")
    elif rl.get("last_error"):
        print("  Relay list: last refresh failed: " + rl["last_error"])
    elif rl.get("notice"):
        # 0.46.3: k2.dev does not list this tunnel token (normal for seeded
        # and freshly provisioned tokens); the saved list stays in use.
        print("  Relay list: " + rl["notice"])
# 0.46.8 voice rooms (preview): the voice lane (frpc-voice) and the media
# path. Absent unless the voice tunnel preview is on or the lane is armed.
vl = d.get("voiceLane")
if isinstance(vl, dict):
    st = vl.get("state") or "?"
    what = {"off": "off", "waiting_for_tunnel": "armed, waiting for the tunnel (E2E, a confirmed label)",
            "starting": "starting (frps has not accepted its turn proxy yet)", "up": "up",
            "restarting": "restarting (frpc-voice exited)"}.get(st, st)
    if vl.get("turnHost") and st != "off":
        what += " for " + vl["turnHost"]
    if vl.get("serverAddr"):
        what += " via " + vl["serverAddr"] + ":" + str(vl.get("serverPort") or "")
    if vl.get("adopted"):
        what += " (adopted)"
    print("  Voice lane: " + what)
    if vl.get("lastError") and st != "up":
        print("  Voice lane error: " + vl["lastError"])
    mp = vl.get("mediaPath") or {}
    if mp.get("state") and mp.get("state") != "off":
        print("  Media path: " + mp["state"] + ("" if mp.get("sewHasRoute") else " (this Sew has no media-path route)"))
# 0.45.2: re-launch on boot, and which name the encrypted listener serves.
if "autoStart" in d:
    print("  Auto-start: " + ("on" if d.get("autoStart") else "off (k2 tunnel enable turns it on)"))
if "e2eListener" in d:
    li = d.get("e2eListener")
    if li:
        lsub = li.get("subdomain", "")
        lport = li.get("port", "")
        print(f"  Encrypted listener: {lsub}.k2.dev (port {lport})")
        want = (d.get("subdomain") or "").strip()
        if d.get("running") and want and want.lower() != str(lsub).lower():
            print(f"  WARNING: the tunnel serves {want}.k2.dev but the listener holds the {lsub}.k2.dev certificate; run: k2 tunnel switch {want}")
'
            ;;
        subdomains)
            local as_json=0
            [ "${1:-}" = "--json" ] && as_json=1
            local raw
            raw=$(cli_request "/cli/tunnel/subdomains")
            if [ "$as_json" = "1" ]; then printf '%s\n' "$raw"; return 0; fi
            # Workspace-name resolution for the 0074 attribution column:
            # one cheap projects-list call (id -> name); best-effort — on
            # failure the table falls back to the raw project id.
            local projects_raw
            projects_raw=$(cli_request "/cli/companion/projects" 2>/dev/null)
            printf '%s' "$raw" | K2_PROJECTS_JSON="$projects_raw" python3 -c '
import json, os, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse tunnel subdomains: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    err = d["error"]
    print(f"Failed to read tunnel subdomains: {err}", file=sys.stderr); sys.exit(1)
names = {}
try:
    for p in json.loads(os.environ.get("K2_PROJECTS_JSON") or "[]"):
        if isinstance(p, dict) and p.get("id"):
            names[p["id"]] = p.get("name") or p["id"]
except Exception:
    pass  # best-effort — fall back to raw project ids below
primary = (d.get("primary") or "").strip()
targets = d.get("targets") or {}
print("K2 Connect subdomains (daemon routing map)")
if not primary and not targets:
    print("  (empty — the daemon has not learned a routing map yet;")
    print("   requires the tunnel running with E2E on)")
    sys.exit(0)
rows = []
if primary:
    rows.append((f"{primary}.k2.dev", "daemon (primary)", "-"))
for label in sorted(targets):
    host = f"{label}.{primary}.k2.dev" if primary else label
    t = targets[label]
    # 0074 shape: {"target": ..., "projectId": ...}; pre-0074 daemons
    # sent the bare endpoint string — render both honestly.
    if isinstance(t, dict):
        endpoint = t.get("target") or "?"
        pid = t.get("projectId")
        ws = names.get(pid, pid) if pid else "-"
    else:
        endpoint, ws = str(t), "-"
    rows.append((host, endpoint, ws))
# NOTE: %-format, not nested-quote f-strings — this whole program lives
# inside a single-quoted bash string, so a bare apostrophe would cut it.
hw = max([len(r[0]) for r in rows] + [len("HOST")])
tw = max([len(r[1]) for r in rows] + [len("TARGET")])
print("  %-*s  %-*s  %s" % (hw, "HOST", tw, "TARGET", "WORKSPACE"))
for host, endpoint, ws in rows:
    print("  %-*s  %-*s  %s" % (hw, host, tw, endpoint, ws))
if not targets:
    print("  (no nested subdomains configured)")
'
            ;;
        --help|-h|help)
            cat <<'EOF'
k2 tunnel — device tunnel to primary *.k2.dev (frpc)

PRODUCT BOUNDARY
  Device tunnel to primary *.k2.dev         → k2 tunnel (this family)
  Nested label.user.k2.dev + local target   → k2 publish subdomain
  Custom zone records (A/TXT/MX/…)          → k2 dns
  Hosted mail domain / MX                   → k2 hostmail domain (owner)

USAGE
  k2 tunnel start [--subdomain <s>]
  k2 tunnel stop | disable
  k2 tunnel enable [--subdomain <s>] [--no-auto-start]   (persists auto-start on by default)
  k2 tunnel switch <subdomain> [--token <t>] [--start|--no-start]
  k2 tunnel release --confirm      (tells K2 Connect only when this device holds the name)
  k2 tunnel forget --confirm       (local only: stale config from another machine)
  k2 tunnel status [--json]
  k2 tunnel subdomains [--json]
  k2 tunnel boot-status [ghost|compat]   what strangers see on /boot-status (Owner/Admin to set)

RELAYS (which relay this box dials)
  The built-in relay is relay.k2.dev (since 0.46.6; before, a fixed IP,
  178.156.232.105). k2.dev may hand out a list of relays instead; it is
  refreshed at daemon start and every few hours.
  k2 tunnel status shows it:
    Server:   the relay frpc dials right now (also while it is still
              connecting or retrying), e.g.
                Server:     relay.k2.dev:7000 (primary relay)
                Server:     203.0.113.10:7000 (fallback relay; returns to the primary once it answers again)
              Whenever Server: is printed, --json has server_addr and
              server_port; stopped, there is no Server: line and --json
              has neither.
    Relays:   always printed: the list the tunnel dials, primary first,
              then [source]:
              fetched (a list from k2.dev), provisioned (a relay list
              written into tunnel.json by hand or a setup script) or
              default (no relay list stored, only tunnel.json's single
              server_addr: the built-in relay, which a box paired before
              0.46.6 has saved as 178.156.232.105). Stopped, the first entry
              is what k2 tunnel start dials. Examples:
                Relays:     relay.k2.dev:7000 [default]
                Relays:     relay.k2.dev:7000 [default; replaces the old built-in 178.156.232.105:7000]
                Relays:     relay.k2.dev:7000 [provisioned; replaces the old built-in 178.156.232.105:7000, 40.160.53.25:7000]
              "replaces the old built-in …" means tunnel.json still holds
              the old relay IPs and the tunnel dials relay.k2.dev instead
              (in memory; the file is not rewritten). With the legacy switch
              below on, the boxes of all three examples show (and dial) the
              old IPs instead, and a running one shows
              e.g. Server:     178.156.232.105:7000 (primary relay):
                Relays:     178.156.232.105:7000 [default]
                Relays:     178.156.232.105:7000, 40.160.53.25:7000 [provisioned]
    Relay default:  only while something sets the switch below, e.g.
                Relay default: legacy (pre-0.46.6, the old built-in IP; set by relay_default in /home/k2/.k2/tunnel.json)
                Relay default: name (relay.k2.dev, the 0.46.6 default; set by relay_default in /home/k2/.k2/tunnel.json)
                Relay default: name (relay.k2.dev, the 0.46.6 default; set by K2_RELAY_DEFAULT in the daemon environment, which wins over tunnel.json)
              Values are "legacy" or "name", any case. Anything else is
              ignored as if unset (no Relay default: line).
    Tunnel config:  the tunnel.json this daemon reads (the file to edit),
              printed even when the file does not exist yet.
  k2 tunnel status --json (relay_list is always there, running or not):
  server_addr + server_port = the relay in use (whenever Server: is
  printed); relay_list.relays = the list dialed, a JSON array of
  "host:port" strings; relay_list.source = "fetched"|"provisioned"|"default";
  relay_list.replaced = the old list it stands in for, an array of
  "host:port" strings (the key is absent otherwise); relay_list.relay_default
  = "legacy"|"name" and relay_list.relay_default_set_by = "env"|"tunnel.json"
  whenever the switch is set (else both absent); relay_list.config_file =
  the path of that tunnel.json.
  Timing: Relays: shows any change (a new list, the switch) at once. Every
  minute the running tunnel compares that list with the one it uses: if
  its relay is still listed it takes the new list with no reconnect; if
  not, it reconnects once, to the new first entry, after a random 0-60 s.
  So Server: follows within about two minutes.
  Go back to the pre-0.46.6 default: the built-in relay is 178.156.232.105
  again (a box showing relay.k2.dev:7000 [default] then dials
  178.156.232.105, whether or not tunnel.json names relay.k2.dev), and
  saved old IPs are dialed as saved, never replaced. It changes only boxes
  whose Relays: says "replaces the old built-in" or shows relay.k2.dev as
  [default]; any other fetched or provisioned list is dialed as it is
  either way, as before 0.46.6:
    put "relay_default": "legacy" at the top level of the file that
    k2 tunnel status prints as Tunnel config: (if it is missing the box is
    not paired yet; create it as {"relay_default": "legacy"}, and pairing
    adds the rest). No restart: a running tunnel follows within about two
    minutes, a stopped one at the next k2 tunnel start. K2's own writes to
    the file (enable, switch, pairing) keep the key.
  Undo: delete the key, or set "relay_default": "name" (the 0.46.6
  default: relay.k2.dev, replacing the old IPs); same timing.
  Or, on Linux only (on a Mac use the file), set K2_RELAY_DEFAULT=legacy in
  the daemon's own environment: k2 daemon status prints "Unit: <name>
  (user)" or "(system)"; run systemctl edit <name> (systemctl --user edit
  <name> for a user unit), add the two lines
      [Service]
      Environment=K2_RELAY_DEFAULT=legacy
  save, and run k2 daemon restart (systemctl edit reloads systemd itself).
  While the variable is set (legacy or name) the file's key is ignored;
  Relay default: says so. Undo: run systemctl edit <name> again (--user
  for a user unit), delete those two lines, save, then k2 daemon restart.
  On a Mac the variable is only there if someone added it to the daemon's
  launchd plist; remove it there, then k2 daemon restart. The k2 CLI
  itself never needs it. Editing the file needs no K2 role, only
  access to that file (the box owner). The switch is temporary: a release
  after the old relay is retired drops it, and "legacy" then reads as
  "name".

OUTPUT CONTRACT
  status / subdomains: human by default; --json → single JSON object on stdout
  ERRORS on stderr (human text today for most tunnel verbs)

EXIT CODES
  0 ok · 1 error
EOF
            exit 0
            ;;
        boot-status)
            # PRD boot-status-ghost §2.3: what a caller with no valid sign-in
            # sees on /boot-status over the tunnel, the app edge, a front
            # door or the LAN. This computer and signed-in callers always
            # get the full body. Writing it is Owner/Admin (REMOTE_ACCESS_KEYS).
            local mode="${1:-}"
            case "$mode" in
                "")
                    cli_request "/cli/tunnel/status" | python3 -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f"Failed to parse tunnel status: {e}", file=sys.stderr); sys.exit(1)
if d.get("error"):
    print("Error: " + str(d["error"]), file=sys.stderr); sys.exit(1)
bs = d.get("bootStatus")
if not bs:
    print("This daemon predates boot-status views (0.46.3): every caller gets the full /boot-status.")
    sys.exit(0)
note = "" if d.get("bootStatusExplicit") else " (the default)"
print(f"Boot status: {bs}{note}")
if bs == "ghost":
    print("Strangers see: phase, protocol range, restart id. /ping says k2-daemon (no version).")
else:
    print("Strangers see: phase, protocol range, restart id, version, features, serverId, publicHost, airgap.enabled.")
print("Signed-in callers and this computer always get the full status.")
'
                    ;;
                ghost|compat)
                    cli_post_json "/cli/settings/update" "{\"bootStatusPublic\":\"${mode}\"}" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr); sys.exit(1)
if "error" in d:
    print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
print("Boot status: %s (live now, no restart)" % (d.get("bootStatusPublic") or sys.argv[1]))
' "$mode" || exit 1
                    ;;
                *)
                    echo "Usage: k2 tunnel boot-status [ghost|compat]" >&2
                    echo "  ghost   strangers see only phase, protocol range and restart id" >&2
                    echo "  compat  strangers also see version + features (older phone apps need them; the 0.46.3 default)" >&2
                    exit 2
                    ;;
            esac
            ;;
        *)
            echo "Usage: k2 tunnel <start [--subdomain <s>]|stop|disable|enable [--subdomain <s>]|release --confirm|status [--json]|subdomains [--json]|boot-status [ghost|compat]>" >&2
            echo "  disable  pause the tunnel, persisted on disk (survives restarts/reboots)" >&2
            echo "  enable   undo disable and bring the tunnel up" >&2
            echo "  release  permanently unpair THIS DEVICE from its subdomain (destructive)" >&2
            echo "  See: k2 tunnel --help for product boundary vs publish/dns" >&2
            exit 1
            ;;
    esac
}

