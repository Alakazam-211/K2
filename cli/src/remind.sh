# ── One-time reminders (a heartbeat that fires once, 0.46.11) ─────────
cmd_help_remind() {
    cat <<'EOF'
k2 heartbeat remind — a one-time heartbeat: fires once, then goes away
(Short form: k2 remind. Some people call this a reminder, an alarm or a
one-shot; repeating ones are heartbeats: k2 heartbeat --help.)

SET ONE
  k2 heartbeat remind "<text>" (--in 45m | --at 13:00 | --on 2026-10-30 [--at 09:00])
                      [--tz Area/City] [--name n] [--file path]
                      [--session pinned|own] [--workspace <ws>] [--json]

  --in 45m      s m h d w, combined: 1h30m. At least 1 minute, at most 365 days.
  --at 13:00    The next 13:00: today if it's still ahead, else tomorrow.
                (For "tomorrow at 13:00" on a certain day, use --on.)
  --at 2026-10-30T09:00            That date and time.
  --at 2026-10-30T09:00:00-06:00   That exact instant (--tz is ignored).
  --on 2026-10-30                  That date at 09:00.
  --on 2026-10-30 --at 14:30       That date and time ('--on "2026-10-30 14:30"' too).
  --tz Area/City  IANA zone. Default: the owner's timezone
                  (Settings → General), else this computer's clock.
  --name n      Lowercase letters, digits, hyphens. Default: r-<6 chars>.
                The name of a finished reminder can be used again.
  --file path   A file in this workspace the reminder points the agent to.
  --session     pinned (default): the workspace's pinned chat.
                own: a session of its own, started for this fire.

  Times in the past are refused. Text is at most 2000 characters; put more
  in a file and use --file.

MANAGE
  k2 heartbeat remind list [--all] [--json]   pending; --all adds finished (90 days)
  k2 heartbeat remind show <name> [--json]    one, with its fire history
  k2 heartbeat remind snooze <name> <10m>     fires again that long FROM NOW (or
                                              --in/--at/--on [--tz] for a set time);
                                              also re-arms one that already fired
  k2 heartbeat remind cancel <name>           stops a pending one, keeps the history
  k2 reminders                                = k2 heartbeat remind list

WHAT HAPPENS
  At its time the agent gets one line in its session:
    [reminder:call-jane · set by Rosson 10-26 09:12] Call Jane back.
  A reminder a person set also posts "Reminder fired: …" to the Thread. An
  agent's own reminder stays in its session.

  Late (this computer was asleep, K2 was off): it fires up to 24 hours late,
  with "· late by 3h 10m" in the stamp. Later than that it finishes as
  missed and tells the owner on the Thread. A reminder never vanishes.
  If it can't be delivered: retried with backoff, then after 5 tries it
  finishes as failed and tells the owner.

  It fires once, even across a restart or a sleep: it is retired in the same
  step that records the fire. Limits: 100 pending per workspace, 20 fires an
  hour per workspace, and the agent's 30 automatic wakes an hour shared with
  mail and Sources (anything over waits for the next minute).

Exit codes: 0 ok, 1 error, 2 bad input (a past time, a bad flag), 3 gated.
More: k2 study heartbeats
EOF
}

_remind_show_result() {
    # stdin: a daemon reply. Prints an error and exits by its code, or passes
    # the JSON on to $1 (python source reading stdin) with $2.. as argv.
    local resp code
    resp="$(cat)"
    if [ -z "$resp" ] || printf '%s' "$resp" | python3 -c "
import json,sys
try:
    d=json.loads(sys.stdin.read())
except Exception:
    sys.exit(0)
sys.exit(0 if isinstance(d,dict) and ('error' in d or d.get('ok') is False) else 1)
" 2>/dev/null; then
        printf '%s' "$resp" | _hb_print_err
        return $?
    fi
    printf '%s' "$resp"
}

# Common --workspace flag. The daemon refuses an agent naming another
# workspace (exit 3).
_remind_workspace_param() {
    local w="$1"
    if [ -d "$w" ]; then w="$(cd "$w" && pwd)"; fi
    printf 'workspace=%s' "$(urlencode "$w")"
}

cmd_heartbeat_remind() {
    local first="${1:-}" has_when=0 a
    for a in "$@"; do
        case "$a" in --in|--at|--on) has_when=1 ;; esac
    done
    # `list` as a reminder's text is `k2 remind list --in 1m`: with a time,
    # the first word is text; without one, a verb.
    if [ "$has_when" = 0 ]; then
        case "$first" in
            ""|-h|--help|help) cmd_help_remind; return 0 ;;
            list)   shift; _remind_list "$@"; return $? ;;
            show)   shift; _remind_show "$@"; return $? ;;
            snooze) shift; _remind_snooze "$@"; return $? ;;
            cancel) shift; _remind_cancel "$@"; return $? ;;
        esac
    fi
    _remind_add "$@"
}

_remind_add() {
    local usage="Usage: k2 heartbeat remind \"<text>\" (--in 45m | --at 13:00 | --on 2026-10-30 [--at 09:00]) [--tz Area/City] [--name n] [--file path] [--session pinned|own] [--json]"
    local text="" in_="" at="" on="" tz="" name="" file="" session="" as_json=0 ws_param=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_remind; return 0 ;;
            --in|--at|--on|--tz|--name|--file|--session|--workspace|--project)
                if [ $# -lt 2 ]; then
                    echo "Error: $1 needs a value" >&2; echo "$usage" >&2; exit 2
                fi
                case "$1" in
                    --in) in_="$2" ;;
                    --at) at="$2" ;;
                    --on) on="$2" ;;
                    --tz) tz="$2" ;;
                    --name) name="$2" ;;
                    --file) file="$2" ;;
                    --session) session="$2" ;;
                    --workspace|--project) ws_param="$(_remind_workspace_param "$2")" ;;
                esac
                shift 2 ;;
            --json) as_json=1; shift ;;
            --*)
                echo "Error: unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *)
                if [ -z "$text" ]; then text="$1"; else text="$text $1"; fi
                shift ;;
        esac
    done
    if [ -z "$text" ] && [ -z "$file" ]; then
        echo "Error: a reminder needs some text, or --file" >&2; echo "$usage" >&2; exit 2
    fi
    if [ -z "$in_$at$on" ]; then
        echo "Error: say when: --in 45m, --at 13:00 or --on 2026-10-30" >&2; echo "$usage" >&2; exit 2
    fi
    local params="text=$(urlencode "$text")"
    [ -n "$in_" ] && params="$params&in=$(urlencode "$in_")"
    [ -n "$at" ] && params="$params&at=$(urlencode "$at")"
    [ -n "$on" ] && params="$params&on=$(urlencode "$on")"
    [ -n "$tz" ] && params="$params&tz=$(urlencode "$tz")"
    [ -n "$name" ] && params="$params&name=$(urlencode "$name")"
    [ -n "$file" ] && params="$params&file=$(urlencode "$file")"
    [ -n "$session" ] && params="$params&session=$(urlencode "$session")"
    [ -n "$ws_param" ] && params="$params&$ws_param"
    local out rc=0
    out="$(cli_send cli_request_post "/cli/heartbeat/reminder/add" "$params" | _remind_show_result)" || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
n = d["name"]
print("Reminder %s set: %s." % (n, d["fires"]))
print("  Cancel: k2 heartbeat remind cancel %s   Snooze: k2 heartbeat remind snooze %s <when>" % (n, n))
'
}

_remind_list() {
    local as_json=0 all=0 ws_param=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_remind; return 0 ;;
            --json) as_json=1; shift ;;
            --all) all=1; shift ;;
            --workspace|--project)
                [ $# -ge 2 ] || { echo "Error: $1 needs a value" >&2; exit 2; }
                ws_param="$(_remind_workspace_param "$2")"; shift 2 ;;
            *) echo "Error: unknown argument: $1" >&2; echo "Usage: k2 heartbeat remind list [--all] [--json]" >&2; exit 2 ;;
        esac
    done
    local args=()
    [ "$all" = 1 ] && args+=("all=1")
    [ -n "$ws_param" ] && args+=("$ws_param")
    local out rc=0
    out="$(cli_send cli_request "/cli/heartbeat/reminder/list" ${args[@]+"${args[@]}"} | _remind_show_result)" || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    printf '%s' "$out" | python3 -c '
import json, sys
rows = json.load(sys.stdin)
if not rows:
    print("No reminders. Set one: k2 heartbeat remind \"text\" --in 45m")
    sys.exit(0)
for r in rows:
    when = r.get("fireAt") or "?"
    state = r.get("state")
    text = (r.get("text") or "").replace("\n", " ")
    if len(text) > 70: text = text[:67] + "..."
    print("%-22s %-10s %s  %s" % (r["name"], state, when, text))
'
}

_remind_show() {
    local name="" as_json=0 ws_param=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_remind; return 0 ;;
            --json) as_json=1; shift ;;
            --workspace|--project)
                [ $# -ge 2 ] || { echo "Error: $1 needs a value" >&2; exit 2; }
                ws_param="$(_remind_workspace_param "$2")"; shift 2 ;;
            --*) echo "Error: unknown flag: $1" >&2; exit 2 ;;
            *) [ -z "$name" ] && name="$1"; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat remind show <name> [--json]" >&2; exit 2; }
    local args=("name=$(urlencode "$name")")
    [ -n "$ws_param" ] && args+=("$ws_param")
    local out rc=0
    out="$(cli_send cli_request "/cli/heartbeat/reminder/show" "${args[@]}" | _remind_show_result)" || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    printf '%s' "$out" | python3 -c '
import json, sys
r = json.load(sys.stdin)
print("Reminder %s (%s)" % (r["name"], r["state"]))
print("  fires:   %s%s" % (r.get("fireAt"), " (%s)" % r["tz"] if r.get("tz") else ""))
print("  text:    %s" % (r.get("text") or ""))
print("  set by:  %s" % (r.get("createdBy") or "?"))
if r.get("firedAt"): print("  fired:   %s" % r["firedAt"])
if r.get("waitReason"): print("  waiting: %s%s" % (r["waitReason"], " (%s)" % r["waitDetail"] if r.get("waitDetail") else ""))
fires = r.get("fires") or []
if fires:
    print("  history:")
    for f in fires:
        print("    %s  %s  %s" % (f.get("firedAt", f.get("fired_at", "")), f.get("decision", ""), f.get("reason") or ""))
'
}

_remind_snooze() {
    local name="" when="" in_="" at="" on="" tz="" ws_param="" as_json=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_remind; return 0 ;;
            --in|--at|--on|--tz|--workspace|--project)
                [ $# -ge 2 ] || { echo "Error: $1 needs a value" >&2; exit 2; }
                case "$1" in
                    --in) in_="$2" ;;
                    --at) at="$2" ;;
                    --on) on="$2" ;;
                    --tz) tz="$2" ;;
                    *) ws_param="$(_remind_workspace_param "$2")" ;;
                esac
                shift 2 ;;
            --json) as_json=1; shift ;;
            --*) echo "Error: unknown flag: $1" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"
                elif [ -z "$when" ]; then when="$1"
                else echo "Error: unexpected argument: $1" >&2; exit 2
                fi
                shift ;;
        esac
    done
    [ -z "$in_" ] && in_="$when"
    if [ -z "$name" ] || [ -z "$in_$at$on" ]; then
        echo "Usage: k2 heartbeat remind snooze <name> <10m> (or --in/--at/--on [--tz Area/City])" >&2
        exit 2
    fi
    local params="name=$(urlencode "$name")"
    [ -n "$in_" ] && params="$params&in=$(urlencode "$in_")"
    [ -n "$at" ] && params="$params&at=$(urlencode "$at")"
    [ -n "$on" ] && params="$params&on=$(urlencode "$on")"
    [ -n "$tz" ] && params="$params&tz=$(urlencode "$tz")"
    [ -n "$ws_param" ] && params="$params&$ws_param"
    local out rc=0
    out="$(cli_send cli_request_post "/cli/heartbeat/reminder/snooze" "$params" | _remind_show_result)" || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("Reminder %s now fires %s." % (d["name"], d["fires"]))
'
}

_remind_cancel() {
    local name="" ws_param="" as_json=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) cmd_help_remind; return 0 ;;
            --json) as_json=1; shift ;;
            --workspace|--project)
                [ $# -ge 2 ] || { echo "Error: $1 needs a value" >&2; exit 2; }
                ws_param="$(_remind_workspace_param "$2")"; shift 2 ;;
            --*) echo "Error: unknown flag: $1" >&2; exit 2 ;;
            *) [ -z "$name" ] && name="$1"; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat remind cancel <name>" >&2; exit 2; }
    local params="name=$(urlencode "$name")"
    [ -n "$ws_param" ] && params="$params&$ws_param"
    local out rc=0
    out="$(cli_send cli_request_post "/cli/heartbeat/reminder/cancel" "$params" | _remind_show_result)" || rc=$?
    [ "$rc" -ne 0 ] && exit "$rc"
    if [ "$as_json" = 1 ]; then
        printf '%s\n' "$out"
        return 0
    fi
    echo "Reminder $name cancelled."
}

cmd_heartbeat_add() {
    # 0.40.41 (GH#23/#24): --instructions/--instructions-file wire the
    # WAKEUP.md at creation time. Before this, an agent had to create,
    # then separately edit a file — and `signal wakeup --edit` opens
    # $EDITOR, which is useless headless. A schedule could therefore
    # fire with no defined job.
    local name="" instructions="" instructions_file=""
    local spec_args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --instructions) instructions="$2"; shift 2 ;;
            --instructions-file) instructions_file="$2"; shift 2 ;;
            *) spec_args+=("$1"); shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat schedule add --name <name> --daily|--weekly|... [--time HH:MM] [--days ...] [--instructions \"...\"|--instructions-file <path>]" >&2; exit 1; }
    if [ -n "$instructions_file" ]; then
        [ -f "$instructions_file" ] || { echo "Error: --instructions-file not found: $instructions_file" >&2; exit 1; }
        instructions="$(cat "$instructions_file")"
    fi
    local spec_output
    spec_output=$(hb_parse_spec ${spec_args[@]+"${spec_args[@]}"})
    local spec_json
    spec_json=$(echo "$spec_output" | head -1)
    local freq
    freq=$(echo "$spec_output" | grep '^FREQ:' | sed 's/^FREQ://')
    local params="name=$(urlencode "$name")&frequency=$freq&spec=$(urlencode "$spec_json")"
    # S5 HB32: the DAEMON writes the instructions, on its own disk. The
    # CLI used to write the returned `wakeupAbs` itself, which is the
    # wrong machine when the daemon is remote.
    if [ -n "$instructions" ]; then
        params="${params}&instructions=$(urlencode "$instructions")"
    fi
    local resp
    resp=$(cli_send cli_request_post "/cli/heartbeat/add" "$params")
    local wakeup_abs written
    if printf '%s' "$resp" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); sys.exit(0 if isinstance(d,dict) and 'error' in d else 1)" 2>/dev/null; then
        printf '%s' "$resp" | _hb_print_err
        return $?
    fi
    wakeup_abs=$(printf '%s' "$resp" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    print(d.get('wakeupAbs') or '')
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
")
    # true | false | missing (a daemon older than S5 ignores the param).
    written=$(printf '%s' "$resp" | python3 -c "
import json, sys
d = json.loads(sys.stdin.read())
v = d.get('instructionsWritten')
print('missing' if v is None else ('true' if v else 'false'))
")
    # What WAKEUP.md holds now, from the daemon (0.46.3): body bytes, or
    # -1 when it did not say (an older daemon: fall back to its wait
    # reason). A WAKEUP.md that came over in a clone is NOT empty.
    local body_bytes kept_existing
    body_bytes=$(printf '%s' "$resp" | python3 -c "
import json, sys
d = json.loads(sys.stdin.read())
b = d.get('wakeupBodyBytes')
if isinstance(b, int):
    print(b)
else:
    print(0 if d.get('waitReason') == 'wakeup_empty' else -1)
")
    kept_existing=$(printf '%s' "$resp" | python3 -c "
import json, sys
print('true' if json.loads(sys.stdin.read()).get('instructionsKeptExisting') else 'false')
")
    echo "Created heartbeat: $name"
    [ -n "$wakeup_abs" ] && echo "Wakeup file (on the daemon's machine): $wakeup_abs"
    echo
    echo "NOTE: this schedule lands in the workspace PINNED chat." >&2
    echo "      A sidecar + \`k2 heartbeat session $name --set sales/reviewer\`" >&2
    echo "      (or --set <session-id> --provider <p>) is often better for a trained flow." >&2
    echo
    if [ -n "$instructions" ] && [ "$written" = "missing" ]; then
        echo "Error: this daemon is older than 'heartbeat add --instructions' and did not write them." >&2
        echo "       The heartbeat exists with an empty WAKEUP.md. Write the instructions to" >&2
        echo "       $wakeup_abs on the daemon's machine, or update the daemon." >&2
        return 1
    fi
    if [ "$written" = "true" ]; then
        echo "Wake instructions written ($(printf '%s' "$instructions" | wc -c | tr -d ' ') bytes)."
    elif [ "$kept_existing" = "true" ]; then
        echo "Kept the existing WAKEUP.md ($body_bytes bytes); --instructions were NOT written over it."
        echo "Edit it with: k2 heartbeat signal wakeup $name --edit (or change the file on the daemon's machine)."
    elif [ "$body_bytes" -gt 0 ] 2>/dev/null; then
        echo "Using the existing WAKEUP.md ($body_bytes bytes)."
    elif [ "$body_bytes" = "0" ]; then
        echo "Needs instructions: WAKEUP.md is empty. The heartbeat stays enabled and waits;"
        echo "it fires at the next scheduled slot after you add them (or re-run with --instructions)."
    fi
    hb_warn_if_transport_down
}

# GH#23: heartbeats silently never fired when the schedule transport
# (launchd agent on macOS / crontab entry on Linux) wasn't installed or
# had stopped ticking. Warn loudly at create time instead of letting the
# user walk away from a heartbeat that can never fire. Best-effort —
# never fails the command it follows.
hb_warn_if_transport_down() {
    cli_send cli_request "/cli/heartbeat/scheduler-status" 2>/dev/null | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
except Exception:
    sys.exit(0)
installed = d.get('transportInstalled')
stale = d.get('staleSecs')
if installed is False:
    print('', file=sys.stderr)
    print('WARNING: no schedule transport is installed on this machine (launchd/cron).', file=sys.stderr)
    print('         This heartbeat will NOT fire until the daemon installs it —', file=sys.stderr)
    print('         check \`k2 daemon status\` and the Heartbeat Settings page.', file=sys.stderr)
elif stale is not None and stale > 900 and d.get('enabledCount', 0) > 0:
    print('', file=sys.stderr)
    print(f'WARNING: the heartbeat scheduler has not ticked for {stale // 60} minutes.', file=sys.stderr)
    print('         Scheduled fires are not being delivered — check \`k2 daemon status\`.', file=sys.stderr)
" || true
}

cmd_heartbeat_list() {
    # GH#27: --json dumps the raw daemon array (pass-through, no table);
    # --archived delegates to the archived listing (either flag order).
    local as_json=0 archived=0
    # 0.46.6: --workspace <ws> lists another workspace's (people; the
    # daemon refuses an agent's passport with other_workspace, exit 3).
    local -a hb_ws=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json)     as_json=1; shift ;;
            --archived) archived=1; shift ;;
            --workspace|--project)
                [ -n "${2:-}" ] || { echo "Usage: k2 heartbeat schedule list [--archived] [--workspace <ws>] [--json]" >&2; exit 2; }
                local hb_w="$2"
                if [ -d "$hb_w" ]; then hb_w="$(cd "$hb_w" && pwd)"; fi
                hb_ws=("workspace=$(urlencode "$hb_w")")
                shift 2 ;;
            *) shift ;;
        esac
    done
    if [ "${#hb_ws[@]}" -gt 0 ] && [ "$archived" = "1" ]; then
        echo "Error: --archived with --workspace isn't supported; list the active ones, or run it from that workspace" >&2
        exit 2
    fi
    if [ "$archived" = "1" ]; then
        if [ "$as_json" = "1" ]; then
            cmd_heartbeat_list_archived --json
        else
            cmd_heartbeat_list_archived
        fi
        return
    fi
    if [ "$as_json" = "1" ]; then
        cli_send cli_request "/cli/heartbeat/list" ${hb_ws[@]+"${hb_ws[@]}"}
        return
    fi
    cli_send cli_request "/cli/heartbeat/list" ${hb_ws[@]+"${hb_ws[@]}"} | python3 -c "
import json, sys
try:
    rows = json.loads(sys.stdin.read())
    if isinstance(rows, dict):
        # Daemon error object, not a row list (e.g. unregistered
        # workspace, missing route on an older daemon) — GH#23 asked
        # for a real message here instead of a parser crash.
        # owner_only / forbidden → exit 3 teaching (not fake invalid token).
        err = rows.get('error', rows)
        if isinstance(err, dict):
            code = err.get('code') or 'error'
            hint = err.get('hint') or err.get('message') or code
            if code in ('owner_only', 'forbidden', 'gated') and 'ask your human' not in hint:
                hint = (hint + ' — ' if hint else '') + 'requires owner/admin — ask your human'
            print(f'Error: {hint}', file=sys.stderr)
            sys.exit(3 if code in ('owner_only', 'forbidden', 'gated', 'other_workspace') else 1)
        print(f'Error: {err}', file=sys.stderr)
        sys.exit(4 if 'workspace not found' in str(err) else 1)
    if not rows:
        print('No heartbeats configured for this workspace.'); sys.exit(0)
    # Heartbeat S3 (HB24): Next + Waiting come from the daemon's stored
    # nextFireAt / waitReason / waitDetail. The CLI does no schedule
    # math; an older daemon without the fields shows '-'.
    from datetime import datetime
    def hb_next(r):
        s = r.get('nextFireAt')
        if not s: return '-'
        try:
            return datetime.fromisoformat(s.replace('Z', '+00:00')).astimezone().strftime('%Y-%m-%d %H:%M')
        except ValueError:
            return s
    def hb_waiting(r):
        reason = r.get('waitReason')
        if not reason: return '-'
        detail = r.get('waitDetail')
        # S5's two reasons keep their own line below.
        if detail and reason not in ('wakeup_empty', 'schedule_error'):
            return f'{reason}: {detail}'
        return reason
    print(f'{\"Name\":<20} {\"Frequency\":<10} {\"Schedule\":<30} {\"Last Fired\":<22} {\"Enabled\":<8} {\"Next\":<17} Waiting')
    print('-' * 140)
    for r in rows:
        spec = json.loads(r.get('specJson', '{}'))
        sched_parts = []
        if 'time' in spec: sched_parts.append(f'at {spec[\"time\"]}')
        if 'days' in spec: sched_parts.append(f'on {\", \".join(spec[\"days\"])}')
        if 'days_of_month' in spec: sched_parts.append(f'day(s) {\",\".join(str(d) for d in spec[\"days_of_month\"])}')
        if 'months' in spec: sched_parts.append(f'in {\",\".join(spec[\"months\"])}')
        # GH#27: hourly rows showed '-' — derive a human string from
        # every_seconds / start / end (e.g. 'every 30m, 09:00-17:00').
        if 'every_seconds' in spec:
            es = int(spec['every_seconds'])
            if es and es % 3600 == 0: every = f'{es // 3600}h'
            elif es and es % 60 == 0: every = f'{es // 60}m'
            else: every = f'{es}s'
            hourly = f'every {every}'
            if spec.get('start') or spec.get('end'):
                hourly += f\", {spec.get('start', '00:00')}-{spec.get('end', '23:59')}\"
            sched_parts.append(hourly)
        elif spec.get('start') or spec.get('end'):
            sched_parts.append(f\"{spec.get('start', '00:00')}-{spec.get('end', '23:59')}\")
        sched = ' '.join(sched_parts) or '-'
        last = (r.get('lastFired') or 'never')[:19].replace('T', ' ')
        en = 'yes' if r.get('enabled') else 'no'
        print(f'{r.get(\"name\",\"?\"):<20} {r.get(\"frequency\",\"?\"):<10} {sched:<30} {last:<22} {en:<8} {hb_next(r):<17} {hb_waiting(r)}')
        # S5: the two creation-trap reasons get a line that says what to do.
        wr = r.get('waitReason')
        if wr == 'wakeup_empty':
            print('    waiting: WAKEUP.md is empty (needs instructions)')
        elif wr == 'schedule_error':
            print(f\"    invalid schedule: {r.get('waitDetail') or r.get('scheduleError') or '?'}\")
        if not r.get('enabled') and r.get('disabledReason') == 'wakeup_empty':
            print('    disabled: WAKEUP.md was empty (add instructions, then enable)')
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

cmd_heartbeat_remove() {
    local name=""
    local purge=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --purge) purge=true; shift ;;
            -*) shift ;;
            *) [ -z "$name" ] && name="$1"; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat remove <name> [--purge]" >&2; exit 1; }

    if $purge; then
        cli_send cli_request_post "/cli/heartbeat/remove" "name=$(urlencode "$name")" | NAME="$name" python3 -c "
import json, os, sys
try:
    d = json.loads(sys.stdin.read())
    n = os.environ['NAME']
    if d.get('success'): print(f'Purged heartbeat: {n} (row deleted, folder removed)')
    else: print(f'Error: {d.get(\"error\", \"unknown\")}', file=sys.stderr); sys.exit(1)
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
    else
        # Default: soft-archive (matches Settings UI's Archive button).
        # Stops firing on schedule, hidden from `heartbeat list`, but
        # the WAKEUP.md and chat history stay readable via
        # `heartbeat list-archived` and the sidebar's Archived section.
        cli_send cli_request_post "/cli/heartbeat/archive" "name=$(urlencode "$name")" | NAME="$name" python3 -c "
import json, os, sys
try:
    d = json.loads(sys.stdin.read())
    n = os.environ['NAME']
    if d.get('success'): print(f'Archived heartbeat: {n}')
    else: print(f'Error: {d.get(\"error\", \"unknown\")}', file=sys.stderr); sys.exit(1)
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
    fi
}

cmd_heartbeat_unarchive() {
    local name="${1:-}"
    [ -z "$name" ] && { echo "Usage: k2 heartbeat unarchive <name>" >&2; exit 1; }
    cli_send cli_request_post "/cli/heartbeat/unarchive" "name=$(urlencode "$name")" | NAME="$name" python3 -c "
import json, os, sys
try:
    d = json.loads(sys.stdin.read())
    n = os.environ['NAME']
    if d.get('success'): print(f'Unarchived heartbeat: {n}')
    else: print(f'Error: {d.get(\"error\", \"unknown\")}', file=sys.stderr); sys.exit(1)
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

cmd_heartbeat_list_archived() {
    # GH#27: --json = raw daemon array pass-through, no table.
    local as_json=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) as_json=1; shift ;;
            *) shift ;;
        esac
    done
    if [ "$as_json" = "1" ]; then
        cli_send cli_request "/cli/heartbeat/list-archived"
        return
    fi
    cli_send cli_request "/cli/heartbeat/list-archived" | python3 -c "
import json, sys
try:
    rows = json.loads(sys.stdin.read())
    if isinstance(rows, dict):
        print(f\"Error: {rows.get('error', rows)}\", file=sys.stderr)
        sys.exit(1)
    if not rows:
        print('No archived heartbeats in this workspace.'); sys.exit(0)
    print(f'{\"NAME\":<20} {\"FREQ\":<10} {\"ARCHIVED\":<22} {\"LAST FIRED\":<22}')
    for r in rows:
        archived = (r.get('archivedAt') or '')[:19].replace('T',' ') or '—'
        last = (r.get('lastFired') or '')[:19].replace('T',' ') or '—'
        print(f'{r.get(\"name\",\"?\"):<20} {r.get(\"frequency\",\"?\"):<10} {archived:<22} {last:<22}')
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

# Manually fire one specific heartbeat (skips the schedule window
# but respects the agent lock so this never double-spawns).
# GH#27: the daemon refuses to fire a DISABLED heartbeat unless force=1;
# --force passes that override.
cmd_heartbeat_fire() {
    local name="" force=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --force) force=1; shift ;;
            -*) echo "Unknown flag for 'heartbeat signal fire': $1" >&2
                echo "Usage: k2 heartbeat signal fire <name> [--force]" >&2
                exit 2 ;;
            *) if [ -z "$name" ]; then name="$1"; fi; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat signal fire <name> [--force]" >&2; exit 1; }
    local params="name=$(urlencode "$name")"
    [ "$force" = "1" ] && params="${params}&force=1"
    cli_send cli_request_post "/cli/heartbeat/fire" "$params" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if 'error' in d:
        print(f\"Error: {d['error']}\", file=sys.stderr); sys.exit(1)
    decision = d.get('decision', '?')
    name = d.get('name', '?')
    agent = d.get('agent', '?')
    if d.get('success'):
        tid = d.get('terminalId', '?')
        print(f'Fired heartbeat: {name} (agent: {agent})')
        print(f'  terminal: {tid}')
        print(f'  audit:    decision=fired (manual fire via CLI)')
    else:
        reason = d.get('reason', 'unknown')
        print(f'Did not fire \"{name}\": {decision}', file=sys.stderr)
        print(f'  reason: {reason}', file=sys.stderr)
        sys.exit(1)
except SystemExit:
    raise
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

# 0.46.9: stop a heartbeat's own session so its next fire starts a new
# one (which picks the LLM token again). An agent may restart its own
# workspace's heartbeats with no switch; the daemon takes the workspace
# from the caller's token. --workspace (another workspace's) is people only.
cmd_heartbeat_restart() {
    local name="" ws="" as_json=0
    local usage="Usage: k2 heartbeat restart <name> [--workspace <ws>] [--json]"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --workspace|--project)
                ws="${2:-}"
                [ -n "$ws" ] || { echo "$usage" >&2; exit 2; }
                shift 2 ;;
            --workspace=*|--project=*) ws="${1#*=}"; shift ;;
            -*) echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; shift
                else echo "Unexpected argument: $1" >&2; echo "$usage" >&2; exit 2; fi ;;
        esac
    done
    [ -z "$name" ] && { echo "$usage" >&2; exit 2; }
    local extra=()
    if [ -n "$ws" ]; then
        if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
        extra=("workspace=$(urlencode "$ws")")
    fi
    local out rc=0
    out="$(cli_send cli_request_post "/cli/heartbeat/restart" "name=$(urlencode "$name")" ${extra[@]+"${extra[@]}"})" || true
    printf '%s' "$out" | HB_NAME="$name" HB_JSON="$as_json" python3 -c '
import json, os, sys
name = os.environ["HB_NAME"]
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print("Error: unexpected reply: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err == "no_such_heartbeat":
    msg = d.get("message") or ("no heartbeat named %s" % name)
    print("Error (no_such_heartbeat): %s. List them with: k2 heartbeat schedule list" % msg, file=sys.stderr)
    sys.exit(1)
if isinstance(err, dict):
    code = err.get("code") or "error"
    print("Error (%s): %s" % (code, err.get("hint") or err.get("message") or code), file=sys.stderr)
    refused = ("other_workspace", "admin_only", "owner_only", "forbidden", "wrong_credential", "invalid_token", "role_required")
    sys.exit(3 if code in refused else 1)
if err == "role_required":
    print("Error (role_required): %s" % (d.get("message") or err), file=sys.stderr)
    sys.exit(3)
if err:
    err = str(err)
    print("Error: %s" % err, file=sys.stderr)
    if err.startswith("not_a_heartbeat"):
        sys.exit(2)
    sys.exit(4 if "workspace not found" in err else 1)
if os.environ["HB_JSON"] == "1":
    print(json.dumps(d, indent=2))
    sys.exit(0)
hb = d.get("name") or name
print("Restarted heartbeat session: %s" % hb)
if d.get("stopped"):
    print("  stopped session: %s" % d.get("terminalId"))
else:
    print("  no session was running")
if d.get("clearedSessionId"):
    print("  cleared saved conversation: %s" % d["clearedSessionId"])
print("The next fire starts a new session. To fire it now: k2 heartbeat signal fire %s" % hb)
' || rc=$?
    return "$rc"
}

cmd_heartbeat_enable_disable() {
    local enabled="$1"; shift
    local name="" ws="" as_json=0
    local usage="Usage: k2 heartbeat schedule enable|disable <name> [--workspace <ws>] [--json]"
    # 0.46.6: --workspace <ws> (or <ws>/<name>) names another workspace's
    # heartbeat. People only: the owner, an Admin, or a person in a K2
    # shell tab. An agent gets exit 3 (other_workspace) and, with
    # permission cards on, the Ask-with line (heartbeat.pause-other /
    # heartbeat.resume-other). Before, the flag was taken as the name and
    # every spelling answered no_such_heartbeat.
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --workspace|--project)
                ws="${2:-}"
                [ -n "$ws" ] || { echo "$usage" >&2; exit 2; }
                shift 2 ;;
            --workspace=*|--project=*) ws="${1#*=}"; shift ;;
            -*) echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *)
                if [ -z "$name" ]; then name="$1"; shift
                else echo "Unexpected argument: $1" >&2; echo "$usage" >&2; exit 2; fi ;;
        esac
    done
    [ -z "$name" ] && { echo "$usage" >&2; exit 1; }
    local extra=()
    if [ -n "$ws" ]; then
        if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; fi
        extra=("workspace=$(urlencode "$ws")")
    fi
    # GH#27: this used to print "Heartbeat 'x' trued/falsed" and swallowed
    # daemon errors (> /dev/null). Surface the error and say
    # Enabled/Disabled like a human would.
    local out rc=0
    out="$(cli_send cli_request_post "/cli/heartbeat/enable" "name=$(urlencode "$name")&enabled=$enabled" ${extra[@]+"${extra[@]}"})" || true
    printf '%s' "$out" | HB_NAME="$name" HB_ENABLED="$enabled" HB_JSON="$as_json" python3 -c '
import json, os, sys
name = os.environ["HB_NAME"]
verb = "Enabled" if os.environ["HB_ENABLED"] == "true" else "Disabled"
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print("Error: unexpected reply: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err == "no_such_heartbeat":
    msg = d.get("message") or ("no heartbeat named %s" % name)
    print("Error (no_such_heartbeat): %s" % msg, file=sys.stderr)
    sys.exit(1)
if isinstance(err, dict):
    code = err.get("code") or "error"
    print("Error (%s): %s" % (code, err.get("hint") or code), file=sys.stderr)
    refused = ("other_workspace", "admin_only", "owner_only", "forbidden", "wrong_credential", "invalid_token")
    sys.exit(3 if code in refused else 1)
if err:
    print("Error: %s" % err, file=sys.stderr)
    sys.exit(4 if "workspace not found" in str(err) else 1)
if os.environ["HB_JSON"] == "1":
    print(json.dumps(d, indent=2))
    sys.exit(0)
if d.get("workspace"):
    print("%s heartbeat: %s (workspace %s)" % (verb, d.get("heartbeat") or name, d["workspace"]))
else:
    print("%s heartbeat: %s" % (verb, name))
' || rc=$?
    return "$rc"
}

# 0.37.8 — flip the per-heartbeat opt-in to deliver WAKEUP.md into the
# workspace's pinned chat session (instead of the heartbeat's own saved
# session). When enabled, the heartbeat's last_session_id stays in the
# DB but is no longer targeted on new fires; the prompt routes through
# `workspace_msg::deliver_live` (same primitive as `k2 msg --wake`).
cmd_heartbeat_use_pinned_session() {
    local enabled="$1"; shift
    local name="${1:-}"
    [ -z "$name" ] && { echo "Usage: k2 heartbeat use-pinned-session [on|off] <name>" >&2; exit 1; }
    cli_send cli_request_post "/cli/heartbeat/set-use-workspace-session" \
        "name=$(urlencode "$name")&enabled=$enabled" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if d.get('success'):
        state = 'enabled' if '$enabled' == 'true' else 'disabled'
        print(f'Pinned-session delivery {state} for heartbeat: $name')
    else:
        print(f\"Error: {d.get('error', 'unknown')}\", file=sys.stderr); sys.exit(1)
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

# 0073 — heartbeat DELIVERY-SESSION control: where a fire's wake
# message lands.
#   k2 heartbeat session <name>                    # show current target
#   k2 heartbeat session <name> --pinned           # workspace pinned chat
#   k2 heartbeat session <name> --auto             # own session, new on next fire
#   k2 heartbeat session <name> --set <session-id|workspace/handle> [--provider <p>]
# The set path hits /cli/heartbeat/set-session; the daemon validates
# the provider, refuses the reserved pinned-chat session, and requires
# the session to exist on disk. Read path is client-side off
# /cli/heartbeat/list (same pattern as `heartbeat show`).
cmd_heartbeat_session() {
    local name="" mode="" session_id="" provider=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --pinned)   mode="pinned"; shift ;;
            --auto)     mode="auto"; shift ;;
            --set)      mode="session"; session_id="${2:-}"; shift 2 ;;
            --provider) provider="${2:-}"; shift 2 ;;
            -*)
                echo "Unknown flag for 'heartbeat session': $1" >&2
                echo "Usage: k2 heartbeat session <name> [--pinned | --auto | --set <session-id|workspace/handle> [--provider <p>]]" >&2
                exit 1
                ;;
            *)  if [ -z "$name" ]; then name="$1"; fi; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat session <name> [--pinned | --auto | --set <session-id|workspace/handle> [--provider <p>]]" >&2; exit 1; }

    if [ -z "$mode" ]; then
        # Read-only: print the current delivery target.
        cli_send cli_request "/cli/heartbeat/list" | NAME="$name" python3 -c "
import json, sys, os
name = os.environ['NAME']
try:
    rows = json.loads(sys.stdin.read())
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
row = next((r for r in rows if r.get('name') == name), None)
if row is None:
    print(f'No heartbeat named \"{name}\" in this workspace.', file=sys.stderr)
    sys.exit(1)
if row.get('useWorkspaceSession'):
    print('pinned  (delivers into the workspace pinned chat)')
elif row.get('lastSessionId'):
    provider = row.get('sessionProvider') or 'default-agent'
    print(f\"{provider}:{row['lastSessionId']}\")
else:
    print('auto  (own session - new on next fire)')
"
        return
    fi

    if [ "$mode" = "session" ]; then
        [ -z "$session_id" ] && { echo "Usage: k2 heartbeat session <name> --set <session-id|workspace/handle> [--provider <p>]" >&2; exit 1; }
        # UUID still wants --provider. Sidecar handle (sales/reviewer or 1)
        # lets the daemon infer the provider.
    elif [ -n "$provider" ]; then
        echo "--provider only applies with --set <session-id|workspace/handle>" >&2; exit 1
    fi

    cli_send cli_request_post "/cli/heartbeat/set-session" \
        "name=$(urlencode "$name")&mode=$mode&session_id=$(urlencode "$session_id")&provider=$(urlencode "$provider")" \
        | NAME="$name" python3 -c "
import json, sys, os
name = os.environ['NAME']
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
if not d.get('success'):
    print(f\"Error: {d.get('error', 'unknown')}\", file=sys.stderr); sys.exit(1)
mode = d.get('mode')
if mode == 'pinned':
    print(f'Heartbeat \"{name}\" now delivers into the workspace pinned chat.')
elif mode == 'auto':
    print(f'Heartbeat \"{name}\" now uses its own session (new on next fire).')
else:
    print(f\"Heartbeat \\\"{name}\\\" now delivers into {d.get('provider')}:{d.get('sessionId')}.\")
"
}

cmd_heartbeat_rename() {
    local from="${1:-}"
    local to="${2:-}"
    if [ -z "$from" ] || [ -z "$to" ]; then
        echo "Usage: k2 heartbeat rename <old-name> <new-name>" >&2
        exit 1
    fi
    cli_send cli_request_post "/cli/heartbeat/rename" "from=$(urlencode "$from")&to=$(urlencode "$to")" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    if d.get('success'): print(f'Renamed heartbeat: $from -> $to')
    else: print(f\"Error: {d.get('error', 'unknown')}\", file=sys.stderr); sys.exit(1)
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

# Change an existing heartbeat's schedule and/or wake instructions.
# Mirrors the flag surface of `cmd_heartbeat_add` — the NAME is
# positional-or-flag, schedule args go through hb_parse_spec, and
# (GH#27) --instructions/--instructions-file rewrite the WAKEUP.md the
# same way add does. With ONLY instructions (no schedule flags) the
# schedule-edit call is skipped entirely and just the file is rewritten.
cmd_heartbeat_edit() {
    local name="" instructions="" instructions_file="" have_instructions=0 have_spec=0
    # Support both `edit <name>` and `edit --name <name>` to stay
    # consistent with add/remove/rename argument styles.
    if [[ $# -gt 0 && "$1" != --* ]]; then
        name="$1"; shift
    fi
    local spec_args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --instructions) instructions="$2"; have_instructions=1; shift 2 ;;
            --instructions-file) instructions_file="$2"; have_instructions=1; shift 2 ;;
            *) spec_args+=("$1"); have_spec=1; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat edit <name> [--daily|--weekly|... <spec>] [--instructions \"...\"|--instructions-file <path>]" >&2; exit 1; }
    if [ "$have_spec" != "1" ] && [ "$have_instructions" != "1" ]; then
        echo "Usage: k2 heartbeat edit <name> [--daily|--weekly|... <spec>] [--instructions \"...\"|--instructions-file <path>]" >&2
        exit 1
    fi
    if [ -n "$instructions_file" ]; then
        [ -f "$instructions_file" ] || { echo "Error: --instructions-file not found: $instructions_file" >&2; exit 1; }
        instructions="$(cat "$instructions_file")"
    fi
    # AH32: one daemon call. The daemon writes the instructions on its own
    # disk (keeping the frontmatter) and records who edited them.
    local params="name=$(urlencode "$name")"
    local freq=""
    if [ "$have_spec" = "1" ]; then
        local spec_output
        spec_output=$(hb_parse_spec ${spec_args[@]+"${spec_args[@]}"})
        local spec_json
        spec_json=$(echo "$spec_output" | head -1)
        freq=$(echo "$spec_output" | grep '^FREQ:' | sed 's/^FREQ://')
        params="${params}&frequency=$freq&spec=$(urlencode "$spec_json")"
    fi
    if [ "$have_instructions" = "1" ]; then
        if [ -z "$(printf '%s' "$instructions" | tr -d '[:space:]')" ]; then
            echo "Error: instructions cannot be blank" >&2
            exit 1
        fi
        params="${params}&instructions=$(urlencode "$instructions")"
    fi
    cli_send cli_request_post "/cli/heartbeat/edit" "$params" | NAME="$name" FREQ="$freq" HAVE_INSTR="$have_instructions" python3 -c "
import json, os, sys
try:
    d = json.loads(sys.stdin.read())
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
if not d.get('success'):
    err = d.get('error', 'unknown')
    if 'frequency' in str(err) and os.environ['FREQ'] == '':
        print('Error: this daemon is older than heartbeat edit --instructions without a schedule.', file=sys.stderr)
        print('       Update the host, or pass the schedule flags too.', file=sys.stderr)
    else:
        print(f'Error: {err}', file=sys.stderr)
    sys.exit(1)
name = os.environ['NAME']
if os.environ['FREQ']:
    print(f'Updated heartbeat: {name} ({os.environ[\"FREQ\"]})')
if os.environ['HAVE_INSTR'] == '1':
    print(f'Wake instructions written by the daemon: {name}')
"
}

# Print a single heartbeat's full details plus its WAKEUP.md instructions,
# from GET /cli/heartbeat/show (AH9). --json dumps the daemon row.
cmd_heartbeat_show() {
    local name=""
    local as_json=0
    local ws=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) as_json=1; shift ;;
            # 0.46.6: another workspace's heartbeat (people; an agent reads
            # its own). The daemon resolves `workspace=` (handle, name, path)
            # and refuses a passport naming another workspace.
            --workspace|--project)
                ws="${2:-}"
                [ -n "$ws" ] || { echo "Usage: k2 heartbeat show <name> [--workspace <ws>] [--json]" >&2; exit 2; }
                shift 2 ;;
            -*) shift ;;
            *) if [ -z "$name" ]; then name="$1"; fi; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat show <name> [--workspace <ws>] [--json]" >&2; exit 1; }
    local extra=()
    if [ -n "$ws" ]; then
        # A directory also becomes PROJECT, for daemons older than the
        # `workspace=` parameter.
        if [ -d "$ws" ]; then ws="$(cd "$ws" && pwd)"; PROJECT="$ws"; fi
        extra=("workspace=$(urlencode "$ws")")
    fi
    cli_send cli_request "/cli/heartbeat/show" "name=$(urlencode "$name")" ${extra[@]+"${extra[@]}"} | NAME="$name" AS_JSON="$as_json" WS="$ws" python3 -c "
import json, os, sys
name = os.environ['NAME']
as_json = os.environ['AS_JSON'] == '1'
ws = os.environ.get('WS', '')
where = f'workspace {ws}' if ws else 'this workspace'
try:
    row = json.loads(sys.stdin.read())
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
if not isinstance(row, dict):
    print(f'Error: unexpected reply: {row}', file=sys.stderr); sys.exit(1)
if row.get('error') == 'no_such_heartbeat':
    print(f'No heartbeat named \"{name}\" in {where}.', file=sys.stderr)
    sys.exit(1)
err = row.get('error')
if isinstance(err, dict):
    print(f\"Error ({err.get('code')}): {err.get('hint') or err.get('code')}\", file=sys.stderr)
    sys.exit(3 if err.get('code') == 'other_workspace' else 1)
if err and 'workspace not found' in str(err):
    print(f'Error: {err}', file=sys.stderr)
    sys.exit(4)
if err:
    print(f\"Error: {err} (an older host has no heartbeat show; update it)\", file=sys.stderr)
    sys.exit(1)
if as_json:
    print(json.dumps(row, indent=2))
    sys.exit(0)
spec = json.loads(row.get('specJson') or '{}')
enabled = 'yes' if row.get('enabled') else 'no'
last = row.get('lastFired') or 'never'
created = row.get('createdAt') or '?'
pinned = 'yes' if row.get('useWorkspaceSession') else 'no'
print(f'Name:           {row.get(\"name\")}')
if ws:
    print(f'Workspace:      {ws}')
print(f'Frequency:      {row.get(\"frequency\")}')
print(f'Enabled:        {enabled}')
print(f'Pinned chat:    {pinned}')
print(f'Last fired:     {last}')
# Heartbeat S3 (HB24): daemon-owned next fire + wait reason.
print(f'Next fire:      {row.get(\"nextFireAt\") or \"-\"}')
waiting = row.get('waitReason') or '-'
if row.get('waitDetail'): waiting += f' ({row.get(\"waitDetail\")})'
if row.get('waitSince'): waiting += f' since {row.get(\"waitSince\")}'
print(f'Waiting:        {waiting}')
print(f'Wakeup file:    {row.get(\"wakeupPath\")}')
print(f'Created:        {created}')
print(f'Spec:')
for k, v in spec.items():
    print(f'  {k}: {v}')
print('Instructions:')
body = row.get('instructions') or ''
if body.strip():
    for line in body.splitlines():
        print(f'  {line}')
else:
    print('  (empty — the heartbeat waits until instructions are written)')
"
}

# Read or edit the wakeup.md that fires for this heartbeat.
#   k2 heartbeat wakeup <name>              # cat the file
#   k2 heartbeat wakeup <name> --path-only  # print absolute path only (for scripting)
#   k2 heartbeat wakeup <name> --edit       # open $EDITOR on the file
#
# The wakeup.md path is workspace-relative in the DB; we resolve it
# against the workspace root (K2SO_PROJECT_PATH or $PWD).
cmd_heartbeat_wakeup() {
    local name=""
    local mode="cat"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --path-only) mode="path"; shift ;;
            --edit)      mode="edit"; shift ;;
            -*)          shift ;;
            *) if [ -z "$name" ]; then name="$1"; fi; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat wakeup <name> [--path-only|--edit]" >&2; exit 1; }

    local rel_path
    rel_path=$(cli_send cli_request "/cli/heartbeat/list" | NAME="$name" python3 -c "
import json, os, sys
try:
    rows = json.loads(sys.stdin.read())
    row = next((r for r in rows if r.get('name') == os.environ['NAME']), None)
    if not row:
        print(f'No heartbeat named \"{os.environ[\"NAME\"]}\" in this workspace.', file=sys.stderr)
        sys.exit(1)
    print(row.get('wakeupPath') or '')
except SystemExit:
    raise
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
")
    local exit_code=$?
    if [ $exit_code -ne 0 ]; then exit $exit_code; fi
    [ -z "$rel_path" ] && { echo "Heartbeat '$name' has no wakeup path on file." >&2; exit 1; }

    local base="${K2_PROJECT_PATH:-${K2SO_PROJECT_PATH:-$PWD}}"
    local abs_path="$base/$rel_path"

    case "$mode" in
        path)
            echo "$abs_path"
            ;;
        edit)
            if [ ! -f "$abs_path" ]; then
                echo "Note: $abs_path does not exist yet; $EDITOR will create it."
            fi
            exec "${EDITOR:-vi}" "$abs_path"
            ;;
        cat)
            if [ ! -f "$abs_path" ]; then
                echo "(wakeup.md not present yet — $abs_path)" >&2
                exit 0
            fi
            echo "# $abs_path"
            echo
            cat "$abs_path"
            ;;
    esac
}

cmd_heartbeat_status() {
    local name=""
    local limit="10"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --limit|-n) limit="$2"; shift 2 ;;
            *) if [ -z "$name" ]; then name="$1"; fi; shift ;;
        esac
    done
    [ -z "$name" ] && { echo "Usage: k2 heartbeat status <name> [--limit N]" >&2; exit 1; }
    # GH#27: an unknown name used to print 'No fire history ... yet',
    # implying the heartbeat exists. Check existence first (same message
    # shape as `heartbeat show`).
    cli_send cli_request "/cli/heartbeat/list" | NAME="$name" python3 -c "
import json, os, sys
try:
    rows = json.loads(sys.stdin.read())
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
if isinstance(rows, dict):
    print(f\"Error: {rows.get('error', rows)}\", file=sys.stderr); sys.exit(1)
row = next((r for r in rows if r.get('name') == os.environ['NAME']), None)
if row is None:
    print(f'No heartbeat named \"{os.environ[\"NAME\"]}\" in this workspace.', file=sys.stderr)
    sys.exit(1)
# Heartbeat S3 (HB24): what the daemon is waiting on, before the history.
if row.get('waitReason'):
    detail = f' ({row[\"waitDetail\"]})' if row.get('waitDetail') else ''
    print(f'Next fire: {row.get(\"nextFireAt\") or \"-\"}   Waiting: {row[\"waitReason\"]}{detail}')
    print()
" || exit 1
    cli_send cli_request "/cli/heartbeat/status" "name=$(urlencode "$name")&limit=$limit" | python3 -c "
import json, sys
try:
    rows = json.loads(sys.stdin.read())
    if not rows:
        print(f'No fire history for heartbeat \"$name\" yet.'); sys.exit(0)
    print(f'Last {len(rows)} fire(s) for heartbeat \"$name\":')
    print(f'{\"Time\":<22} {\"Decision\":<22} {\"By\":<22} Reason')
    print('-' * 110)
    def who(a):
        # AH18 actor words (twin of k2_core actor_phrase).
        if not a: return 'scheduler'
        if a == 'owner-token': return 'owner'
        for pre, words in (('user:', 'user'), ('agent:', 'agent'), ('app:', 'app user'), ('app-token:', 'app token')):
            if a.startswith(pre): return f'{words} {a[len(pre):]}'
        return a
    for r in rows:
        t = (r.get('firedAt') or '')[:19].replace('T', ' ')
        print(f'{t:<22} {r.get(\"decision\",\"?\"):<22} {who(r.get(\"actor\")):<22} {r.get(\"reason\",\"\") or \"\"}')
except Exception as e:
    print(f'Error: {e}', file=sys.stderr); sys.exit(1)
"
}

# Show the recent heartbeat decision log for this workspace.
# Every tick writes one row per agent — fired OR skipped — with the
# reason. This makes "why didn't my agent wake?" directly answerable.
cmd_heartbeat_log() {
    local limit="50"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --limit) limit="$2"; shift 2 ;;
            -n) limit="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    cli_send cli_request "/cli/heartbeat-log" "limit=$limit" | python3 -c "
import json, sys
try:
    rows = json.loads(sys.stdin.read())
    if not rows:
        print('No heartbeat activity recorded yet.')
        sys.exit(0)
    print(f'{'Fired At':<22} {'Agent':<20} {'Decision':<22} {'Inbox':<8} Reason')
    print('-' * 100)
    for r in rows:
        fired = r.get('firedAt', '?')[:19].replace('T', ' ')
        agent = r.get('agentName') or '(project)'
        decision = r.get('decision', '?')
        inbox_count = r.get('inboxCount')
        inbox_priority = r.get('inboxPriority') or ''
        inbox = f'{inbox_count or 0}{\" \" + inbox_priority if inbox_priority else \"\"}'
        reason = r.get('reason') or ''
        print(f'{fired:<22} {agent:<20} {decision:<22} {inbox:<8} {reason}')
except Exception as e:
    print(f'Error parsing response: {e}', file=sys.stderr)
    sys.exit(1)
" 2>/dev/null || cat
}

cmd_hooks_status() {
    local limit="20"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --limit|-n) limit="$2"; shift 2 ;;
            --json) local as_json=1; shift ;;
            *) shift ;;
        esac
    done
    local raw
    raw=$(cli_request "/cli/hooks/status" "limit=$limit")
    if [ "${as_json:-0}" = "1" ]; then
        echo "$raw"
        return 0
    fi
    echo "$raw" | python3 -c "
import json, sys, time
data = json.loads(sys.stdin.read())
inj = data.get('injections', {})
script = inj.get('notify_script', {})
clis = data.get('clis', {})
def ago(ms):
    if not ms:
        return 'never'
    s = max(0, int(time.time() - ms / 1000))
    return f'{s}s ago' if s < 120 else (f'{s // 60}m ago' if s < 7200 else f'{s // 3600}h ago')
print(f\"K2 hook server port: {data.get('port')}\")
print(f\"Notify script: {script.get('path','?')}  exists={script.get('exists',False)}  version={script.get('version') or '?'}\")
owner = data.get('ownerCheck')
if owner:
    note = '  (Windows: conversation id only, no process check)' if owner == 'conversation' else ''
    print(f'Owner check: {owner}{note}')
last = data.get('lastInstall') or {}
if last.get('skipped'):
    print(f\"Last install pass: skipped ({last['skipped']})\")
elif last.get('report'):
    r = last['report']
    print(f\"Last install pass: {r.get('action')} ({last.get('trigger')}), agentHooks setting {'on' if last.get('agentHooksSetting') else 'off'}\")
print()
print('Config injections:')
for key in ('claude', 'cursor', 'gemini'):
    c = inj.get(key, {})
    d = clis.get(key, {})
    if c.get('configUnreadable'):
        tag = '!! '
    else:
        tag = 'OK ' if c.get('injected') else ('NO ' if c.get('exists') else '-- ')
    extra = []
    if key == 'claude' and d.get('detectedVersion'):
        extra.append(f\"claude {d['detectedVersion']} ({d.get('eventSet') or '?'} set)\")
    if c.get('configUnreadable'):
        extra.append('config_unreadable: left untouched')
    events = d.get('installedEvents') or c.get('events') or []
    if events:
        extra.append(f'{len(events)} events')
    extra.append(f\"last event {ago(d.get('lastEventMs'))}\")
    print(f\"  [{tag}] {key:<8} {c.get('path','?')}  {'; '.join(extra)}\")
sessions = data.get('sessions') or {}
if sessions:
    print()
    print('Live sessions:')
    print(f\"  {'Session':<38} {'Owner':<16} {'Evidence':<10} {'Accepted':>8} {'Foreign':>8} {'Unknown':>8} {'Parse':>6}  Last\")
    for sid, e in sessions.items():
        o = e.get('owner') or {}
        proc = o.get('proc') or {}
        who = f\"pid {proc['pid']}\" if proc.get('pid') else (o.get('via') or 'none')
        c = e.get('counters') or {}
        ev = e.get('evidence') or '-'
        print(f\"  {sid:<38} {who:<16} {ev:<10} {c.get('accepted',0):>8} {c.get('foreign',0):>8} {c.get('unknownPane',0):>8} {c.get('parseError',0):>6}  {ago(e.get('lastEnvelopeMs'))}\")
totals = data.get('totals')
if totals:
    print(f\"Totals: accepted {totals.get('accepted',0)}, foreign {totals.get('foreign',0)}, unknown pane {totals.get('unknownPane',0)}, parse errors {totals.get('parseError',0)}\")
print()
events = data.get('recent_events', [])
cap = data.get('recent_events_cap', 50)
if not events:
    print(f'Recent events (0 of last {cap}): none received yet — launch a Claude session and run a prompt.')
else:
    print(f'Recent events (showing last {len(events)} of {cap}):')
    print(f\"{'Time':<20} {'Verdict':<13} {'Raw':<22} {'Tool':<14} Pane\")
    print('-' * 100)
    for e in events:
        ts = (e.get('timestamp') or '')[:19].replace('T',' ')
        verdict = e.get('verdict') or e.get('canonical') or '(unmatched)'
        raw = e.get('raw_event') or ''
        tool = e.get('tool_name') or ''
        pane = (e.get('pane_id') or '')
        print(f'{ts:<20} {verdict:<13} {raw:<22} {tool:<14} {pane}')
" 2>/dev/null || echo "$raw"
}

# `k2 hooks install` / `k2 hooks uninstall` (prd-daemon-activity-and-
# thread-working-v1 DA17, A2): re-run the daemon's hook installer now, or
# remove K2's entries (and only K2's) from the agent CLI configs.
cmd_hooks_install() {
    local remove="${1:-0}"
    shift || true
    local as_json=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --json) as_json=1; shift ;;
            *) shift ;;
        esac
    done
    local body='{}'
    [ "$remove" = "1" ] && body='{"remove":true}'
    local raw
    raw=$(cli_post_json "/cli/hooks/install" "$body")
    if [ "$as_json" = "1" ]; then
        echo "$raw"
        return 0
    fi
    echo "$raw" | python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
if 'error' in data:
    print(f\"Error: {data['error']}\", file=sys.stderr)
    sys.exit(1)
if data.get('skipped'):
    print(f\"Hooks not changed: {data['skipped']}.\")
    sys.exit(0)
r = data.get('report') or {}
action = r.get('action', '?')
s = r.get('script') or {}
print(f\"Hook {action}: notify.sh {s.get('state','?')} ({s.get('path','?')})\")
if action == 'install':
    print(f\"Claude {r.get('claudeVersion') or 'version unknown'}: {r.get('claudeEventSet','?')} event set\")
if not data.get('agentHooksSetting', True) and action == 'remove':
    print('The agentHooks setting is off, so K2 hooks were removed.')
for c in r.get('clis', []):
    note = ''
    if c.get('state') == 'config_unreadable':
        note = '  (did not parse; left untouched)'
    elif c.get('error'):
        note = f\"  ({c['error']})\"
    print(f\"  {c.get('cli','?'):<8} {c.get('state','?'):<18} {c.get('path','?')}{note}\")
" 2>/dev/null || echo "$raw"
}

# `k2 hooks emit` (GL45, prd-gardens-local-services-v1 Appendix A/B): the
# supported way for a custom-preset program (no model CLI, so no hook
# config of its own) to report its lifecycle to the daemon, as Claude's
# hooks do through notify.sh. Same route (POST /hook/event), same owner
# check (the sender must run inside this session's PTY), source `custom`.
# Auth is ONLY this session's own passport ($K2_HOOK_TOKEN, over the cell
# socket and then loopback TCP): never the owner token, so it never
# re-sources ~/.k2/heartbeat.token, and outside a K2 session it refuses.
_HOOKS_EMIT_EVENTS="SessionStart UserPromptSubmit PreToolUse PostToolUse Notification Stop SessionEnd"

_hooks_emit_usage() {
    cat <<'EOF'
Usage: k2 hooks emit <event> [--json '<object>'] [--session-id <id>] [--quiet]

Report a lifecycle event from a custom program running in this K2 session
(the session's own passport; never the owner token).

Events (Claude's names):
  SessionStart       the program is up (add --json '{"source":"startup"}')
  UserPromptSubmit   a turn starts: the agent shows as working
  PreToolUse         a tool call starts (--json '{"tool_name":…,"tool_use_id":…}')
  PostToolUse        that tool call ended (same tool_use_id)
  Notification       waiting on a person
  Stop               the turn is done: the agent shows as idle
  SessionEnd         the program is exiting

  --json '<object>'  extra fields merged into the event (object only;
                     hook_event_name always comes from <event>)
  --session-id <id>  the program's own conversation id, if it has one
                     (informational; it never changes the Thread pin)
  --quiet            print nothing on success

Exit: 0 accepted; 1 not counted (foreign / unknown_pane / parse_error /
daemon unreachable); 2 usage; 3 no session passport (run it inside a K2
session). More: k2 study hooks
EOF
}

cmd_hooks_emit() {
    local event="" extra="" sid="" quiet=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) _hooks_emit_usage; return 0 ;;
            --json) [ $# -ge 2 ] || { echo "Error: --json needs an object" >&2; exit 2; }; extra="$2"; shift 2 ;;
            --session-id) [ $# -ge 2 ] || { echo "Error: --session-id needs a value" >&2; exit 2; }; sid="$2"; shift 2 ;;
            --quiet|-q) quiet=1; shift ;;
            -*) echo "Error: unknown flag $1" >&2; _hooks_emit_usage >&2; exit 2 ;;
            *)
                if [ -n "$event" ]; then
                    echo "Error: one event per call (got '$event' and '$1')" >&2
                    exit 2
                fi
                event="$1"; shift ;;
        esac
    done
    if [ -z "$event" ]; then
        _hooks_emit_usage >&2
        exit 2
    fi
    local known=0 e
    for e in $_HOOKS_EMIT_EVENTS; do
        [ "$e" = "$event" ] && known=1
    done
    if [ "$known" != "1" ]; then
        echo "Error: unknown event '$event'. One of: $_HOOKS_EMIT_EVENTS" >&2
        exit 2
    fi

    local pane="${K2_PANE_ID:-${K2SO_PANE_ID:-}}"
    local sock="${K2_HOOK_SOCK:-${K2SO_HOOK_SOCK:-}}"
    if [ -z "$SCOPED_TOKEN" ] || [ -z "$pane" ]; then
        echo "Error: k2 hooks emit runs inside a K2 session (no session passport: K2_HOOK_TOKEN / K2_PANE_ID unset)." >&2
        echo "It reports for the program in this PTY; it never uses the owner token. See k2 study hooks." >&2
        exit 3
    fi

    local body
    body="$(python3 -c '
import json, sys
event, extra, sid = sys.argv[1], sys.argv[2], sys.argv[3]
obj = {}
if extra:
    try:
        obj = json.loads(extra)
    except ValueError as e:
        sys.stderr.write("Error: --json is not JSON: %s\n" % e)
        sys.exit(2)
    if not isinstance(obj, dict):
        sys.stderr.write("Error: --json must be a JSON object\n")
        sys.exit(2)
obj["hook_event_name"] = event
if sid:
    obj["session_id"] = sid
print(json.dumps(obj, separators=(",", ":")))
' "$event" "$extra" "$sid")" || exit 2

    local _K2_CFG=""
    _k2_cfg_add header "Authorization: Bearer ${SCOPED_TOKEN}"
    _k2_cfg_add data-raw "$body"
    local hdr=(-H "Content-Type: application/json"
               -H "X-K2-Pane: $pane"
               -H "X-K2-Hook-Source: custom"
               -H "X-K2-Hook-Version: 2"
               -H "X-K2-Hook-Report: 1"
               -H "X-K2-Hook-Event: $event"
               -H "X-K2-Agent-Pid: $PPID")
    local out="" status=""
    # Per-cell socket first (the session's own door), then loopback TCP
    # with the SAME passport (the socket can be stale after a restart).
    if [ -n "$sock" ] && [ -S "$sock" ]; then
        out="$(_k2_curl "$_K2_CFG" -s -X POST --unix-socket "$sock" --connect-timeout 2 --max-time 5 \
            "${hdr[@]}" -w $'\n%{http_code}' "http://localhost/hook/event" 2>/dev/null)" || out=""
        status="${out##*$'\n'}"
    fi
    if [ -z "$status" ] || [ "$status" = "000" ]; then
        local port="${PORT:-}"
        if [ -z "$port" ] && [ -r "${K2_HOME}/heartbeat.port" ]; then
            port="$(cat "${K2_HOME}/heartbeat.port" 2>/dev/null)"
        fi
        if [ -z "$port" ]; then
            echo "Error: K2 daemon unreachable (no cell socket, no port)." >&2
            exit 1
        fi
        out="$(_k2_curl "$_K2_CFG" -s -X POST --connect-timeout 2 --max-time 5 \
            "${hdr[@]}" -w $'\n%{http_code}' "http://127.0.0.1:${port}/hook/event" 2>/dev/null)" || out=""
        status="${out##*$'\n'}"
    fi
    local resp="${out%$'\n'*}"
    case "$status" in
        200)
            local verdict
            verdict="$(printf '%s' "$resp" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("verdict",""))
except Exception: print("")')"
            if [ "$verdict" = "accepted" ]; then
                [ "$quiet" = "1" ] || echo "hooks emit: $event accepted"
                return 0
            fi
            echo "Error: $event not counted: ${verdict:-no verdict}. The sender must run inside this session's PTY (k2 hooks status)." >&2
            exit 1
            ;;
        204)
            echo "Error: this daemon does not report hook verdicts (update K2 to 0.45.6+)." >&2
            exit 1
            ;;
        401|403)
            echo "Error: the session passport was refused for this pane ($status)." >&2
            exit 3
            ;;
        *)
            echo "Error: K2 daemon unreachable or failed (HTTP ${status:-none})." >&2
            exit 1
            ;;
    esac
}

