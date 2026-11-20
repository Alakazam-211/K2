# ── Agent Skill Surface ─────────────────────────────────────────────

_done_usage() {
    cat <<'EOF'
%include cli/text/help/done.txt
EOF
}

cmd_done() {
    local agent
    _envn_into agent AGENT_NAME
    local blocked=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --agent)
                [[ $# -ge 2 && "$2" != -* ]] || { echo "k2 done: '--agent' needs a name." >&2; exit 2; }
                agent="$2"; shift 2 ;;
            --blocked)
                [[ $# -ge 2 ]] || { echo "k2 done: '--blocked' needs a reason." >&2; exit 2; }
                blocked="$2"; shift 2 ;;
            -h|--help) _done_usage; return 0 ;;
            # Before, every unknown word was skipped, so `k2 done --help`
            # marked the task done. A flag done doesn't take sends nothing.
            -*) echo "k2 done: unknown flag '$1'. k2 done takes --blocked \"reason\" and --agent <name> (k2 done --help)." >&2; exit 2 ;;
            *) shift ;;
        esac
    done
    [[ -z "$agent" ]] && { echo "Error: --agent required (or set K2_AGENT_NAME)" >&2; exit 1; }
    # The name is escaped like every other value: raw, a '#' cut the query
    # (and dropped --blocked) and a '&' added a parameter of its own.
    local params="agent=$(urlencode "$agent")"
    [[ -n "$blocked" ]] && params="$params&blocked=$(urlencode "$blocked")"
    cli_request_post "/cli/done" "$params"
}

cmd_whoami() {
    local json="false"
    for arg in "$@"; do
        case "$arg" in
            --help|-h) cmd_help_whoami; return 0 ;;
            --json) json="true" ;;
            -*)
                echo "Error: unknown flag '$arg' (try 'k2 whoami --help')" >&2
                exit 1 ;;
        esac
    done
    local q="" w_sid w_cell w_side w_prim
    _envn_into w_sid SESSION_ID
    _envn_into w_cell CELL
    _envn_into w_side SIDECAR_NAME
    _envn_into w_prim PRIMARY
    [ -n "$w_sid" ] && q="${q}session=$(urlencode "$w_sid")&"
    [ -n "$w_cell" ] && q="${q}cell=$(urlencode "$w_cell")&"
    [ -n "$w_side" ] && q="${q}sidecar_name=$(urlencode "$w_side")&"
    [ -n "$w_prim" ] && q="${q}primary=$(urlencode "$w_prim")&"
    local resp
    resp=$(cli_send cli_request "/cli/whoami" "${q%&}")
    if [ "$json" = "true" ]; then
        printf '%s\n' "$resp"
        if printf '%s' "$resp" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); sys.exit(0 if isinstance(d,dict) and 'role' in d else 1)" 2>/dev/null; then
            return 0
        fi
        return 1
    fi
    printf '%s' "$resp" | python3 -c "
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write('Error: daemon returned an unparseable whoami response\n')
    sys.exit(1)
if isinstance(d, dict) and 'error' in d:
    err = d.get('error')
    hint = d.get('hint') or ''
    if isinstance(err, dict):
        sys.stderr.write('Error: %s\n' % (err.get('hint') or err.get('code') or err))
    else:
        sys.stderr.write('Error: %s\n' % err)
        if hint:
            sys.stderr.write('%s\n' % hint)
    sys.exit(1)
def field(label, key):
    return '%-10s %s' % (label + ':', d.get(key, ''))
print(field('workspace', 'workspace'))
print(field('role', 'role'))
print(field('address', 'address'))
print(field('primary', 'primary'))
print(field('session', 'session'))
sib = d.get('siblings')
if isinstance(sib, dict) and 'live' in sib:
    print('%-10s %d live here (k2 sessions)' % ('others:', sib.get('live', 0)))
"
}

# _msg_summary_hint <text> <summary>: on stderr, when a message is over
# ~600 characters and has no --summary (plan-projects-threads-chatter-v1).
_msg_summary_hint() {
    local text="$1" summary="$2" n
    [[ -n "$summary" ]] && return 0
    n=$(printf '%s' "$text" | python3 -c 'import sys; print(len(sys.stdin.read()))' 2>/dev/null || printf '%s' "${#text}")
    if [[ "$n" -gt 600 ]]; then
        echo "hint: this message is $n characters with no --summary. Add --summary \"one line: what you need or what you did\" so people can scan it (K2 uses its first line otherwise)." >&2
    fi
    return 0
}

cmd_msg() {
    # `k2 msg <workspace> "text" [--from <name>]`
    #
    # 0.38.6 deliver-or-loudly-fail contract: tries to deliver live to
    # the workspace's running agent, spawning the agent if needed.
    # Returns success only when the message has landed in the agent's
    # session. Never silently writes to inbox. For queued tasks the
    # recipient reads on their own schedule, use `k2 msg <workspace>
    # --inbox-wake|--inbox-silent <file>`.
    #
    # <workspace>   workspace name (preferred), absolute path, or UUID
    # --from <name> sender identity (default: auto-derived from CWD/
    #               K2SO_PROJECT_PATH; falls back to `external`)
    #
    # Legacy flags `--wake` and `--agent <name>` are accepted as
    # silent no-ops for one release so external scripts don't break
    # overnight. They'll be removed in 0.39.x.

    # `--help` / `-h` must be detected BEFORE arg-required validation
    # below — closes the chronic UX bug where `k2 msg --help`
    # errored with "message text required" instead of showing help.
    for arg in "$@"; do
        [ "$arg" = "--" ] && break
        case "$arg" in
            --help|-h)
                cmd_help_msg
                return 0
                ;;
        esac
    done

    local target=""
    local text=""
    local explicit_from=""
    local explicit_command=""
    local explicit_via=""
    # Issue #9 — `msg` AUTO-WAKES a dormant canonical session by DEFAULT
    # (resume/spawn → wait READY → deliver), preserving the legacy
    # wake-on-message UX (PRD D7). `--no-wake` opts out (blind live-only
    # send); `--wake` is accepted as an explicit no-op for symmetry.
    # `--wake-timeout` bounds the post-wake readiness wait.
    local wake="true"
    local wake_timeout=""
    local secret_name=""
    local summary=""
    local session_id=""
    local idem_key=""
    # The flags as typed, in order, without their values: Postal mail
    # refuses the first one it can't honour, as the address-first form does.
    local flags_seen=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --idempotency-key|--session|--summary|--secret|--from|--command|--wake|--no-wake|--wake-timeout|--via)
                flags_seen+=("$1") ;;
        esac
        case "$1" in
            --idempotency-key)
                # 0.46.15: ask again for a send that may have landed (the
                # key its "may have been delivered" line printed).
                [[ -n "${2:-}" ]] || { echo "usage: --idempotency-key <key>  (the key a 'may have been delivered' line printed)" >&2; exit 2; }
                idem_key="$2"; shift 2 ;;
            --session)
                # prd-workspace-sessions-cli-v1 WS-M2: a conversation by id
                # (k2 sessions --past shows the short ids). Whoever holds it
                # gets the message; a past one is reopened, then delivered.
                [[ -n "${2:-}" ]] || { echo "usage: k2 msg --session <id> \"text\"  (ids: k2 sessions --past)" >&2; exit 2; }
                session_id="$2"; shift 2 ;;
            --summary)
                # 0.46.3: one line for people (Chatter, the Projects view,
                # phone clients). Never injected into the recipient's PTY.
                [[ $# -ge 2 ]] || { echo "usage: --summary \"one line: what you need or what you did\"" >&2; exit 2; }
                summary="$2"; shift 2 ;;
            --secret)
                # `k2 msg <agent> "text" --secret NAME` (prd-agent-secret-
                # handoff-v1 §4.2): a vault handoff whose note is the text.
                [[ -n "${2:-}" ]] || { echo "usage: --secret NAME (a secret in your vault)" >&2; exit 2; }
                secret_name="$2"; shift 2 ;;
            --from) explicit_from="$2"; shift 2 ;;
            --command) explicit_command="$2"; shift 2 ;;
            --wake)
                wake="true"; shift ;;
            --no-wake)
                wake="false"; shift ;;
            --wake-timeout)
                wake_timeout="$2"; shift 2 ;;
            --via)
                # Internal: `k2 talk` stamps via=talk so chatter does not
                # vanish into msg. Not a documented agent flag.
                explicit_via="$2"; shift 2 ;;
            --agent)
                # 0.39.0f Phase 2.1b: hard-deprecated. The agent-keyed
                # routing path is gone; `msg` is workspace-implicit.
                fail_deprecated "k2 msg --agent <name>" "k2 msg <workspace> \"text\"  (workspace-implicit; pass a workspace name/path)" ;;
            --)
                # The rest is the address and the text, as typed.
                shift
                while [[ $# -gt 0 ]]; do
                    if [[ -z "$target" ]]; then target="$1"; else text="$1"; fi
                    shift
                done ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 msg --help')" >&2
                exit 1 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                else text="$1"
                fi
                shift ;;
        esac
    done

    if [[ -n "$session_id" ]]; then
        # `k2 msg --session <id> "text"`: the one positional is the text;
        # the conversation is in this workspace (the owner names another
        # with K2_PROJECT_PATH / --workspace on k2 sessions).
        if [[ -n "$target" && -z "$text" ]]; then
            text="$target"
        elif [[ -n "$target" && -n "$text" ]]; then
            echo "usage: k2 msg --session <id> \"text\"  (no address with --session)" >&2
            exit 2
        fi
        target="${PROJECT:-$PWD}"
    fi

    # A Postal address that msg's own loop read (a value flag came first):
    # Postal mail, as the address-first form is. The flags first, so one
    # Postal mail can't honour is refused, never dropped; then `--` and the
    # text, sent as typed.
    if [[ -z "$session_id" ]] && _is_postal_address "$target"; then
        _postal_send_teach "$target" ${flags_seen[@]+"${flags_seen[@]}"} -- ${text:+"$text"}
    fi

    [[ -z "$target" ]] && {
        echo "Error: workspace required" >&2
        echo "Usage: k2 msg <workspace> \"text\" [--from <name>] [--command \"<text>\"]" >&2
        echo "       Run 'k2 msg --help' for full reference." >&2
        exit 1
    }
    [[ -z "$text" ]] && {
        echo "Error: message text required" >&2
        echo "Usage: k2 msg <workspace> \"text\" [--from <name>] [--command \"<text>\"]" >&2
        exit 1
    }
    # 0.46.3: long messages should say in one line what they want; a hint,
    # never an error (the daemon derives a first-line summary otherwise).
    _msg_summary_hint "$text" "$summary"
    if [[ -n "$session_id" && -n "$secret_name" ]]; then
        echo "usage: --secret goes to an address, not --session (k2 sessions shows addresses)" >&2
        exit 2
    fi

    # --secret: one handoff; the text rides as its note. If the send fails
    # no message goes either, so the text never arrives without its secret.
    if [[ -n "$secret_name" ]]; then
        _vault_handoff send "$secret_name" --to "$target" --note "$text" --via msg
        return $?
    fi
    # 0.46.15: one key for this send (BEGIN_CLI_SEND_ONCE).
    _k2_idem_init "$idem_key"

    # Cross-server peer: `k2 msg <agent>::<host> "text"` (or legacy @)
    # routes a CONNECTED remote peer over federation (same verb as a local
    # peer). Wire form `fp::ws::agent` is for `k2 fed send` only — teach.
    # Local-workspace path below is untouched when _REMOTE_HOST is empty.
    # (cmd_talk's @host is a different, owner-remote path and is
    # intentionally NOT unified here.)
    _REMOTE_HOST=""
    _IS_WIRE_FORM=0
    [[ -z "$session_id" ]] && _split_remote_target "$target"
    if [ "${_IS_WIRE_FORM:-0}" = "1" ]; then
        echo "k2 msg: '${target}' looks like federation wire form (<fp>::<workspace>::<agent>)." >&2
        echo "  For agents use the user form:  k2 msg <agent>::<host> \"text\"" >&2
        echo "  Wire form is for:              k2 fed send <fp>::<ws>::<agent> \"text\"" >&2
        return 2
    fi
    if [ -n "$_REMOTE_HOST" ]; then
        _send_remote_msg "$_BARE_TARGET" "$_REMOTE_HOST" "$text" "$PROJECT"
        return $?
    fi

    # Resolve sender identity.
    #
    # Priority:
    #   1. Explicit --from <name>
    #   2. Auto-derive from K2SO_PROJECT_PATH or CWD (look up workspace name)
    #   3. Fallback: "external"
    #
    # The daemon also substitutes "external" defensively if it gets an
    # empty string, so the recipient PTY always sees a [from ...] prefix.
    local from="$explicit_from"
    if [[ -z "$from" ]]; then
        # Sidecar cells stamp sales/reviewer (D15). Daemon rewrite is
        # SSOT on scoped UDS; this covers TCP / env-only fallbacks.
        local f_cell f_prim f_side
        _envn_into f_cell CELL
        _envn_into f_prim PRIMARY
        _envn_into f_side SIDECAR_NAME
        if [[ "$f_cell" == "sidecar" && -n "$f_prim" && -n "$f_side" ]]; then
            from="${f_prim}/${f_side}"
        fi
    fi
    if [[ -z "$from" ]]; then
        # Prefer the workspace handle (D18) so [from] is a reply address,
        # never a pretty display name or a passport UUID.
        local sender_path
        _envn_into sender_path PROJECT_PATH "$PWD"
        if [[ -n "$sender_path" ]]; then
            local sender_name
            sender_name=$(_K2_PERMISSION_QUIET=1 cli_request "/cli/workspace/handle" \
                "project=$(urlencode "$sender_path")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    name = (d.get("handle") or "").strip()
    if name:
        print(name)
except Exception:
    pass' 2>/dev/null)
            if [[ -z "$sender_name" ]]; then
                sender_name=$(_K2_PERMISSION_QUIET=1 cli_request "/cli/workspace/agent-display-name" \
                    "project=$(urlencode "$sender_path")" 2>/dev/null \
                    | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    name = (d.get("handle") or "").strip()
    if name:
        print(name)
except Exception:
    pass' 2>/dev/null)
            fi
            if [[ -z "$sender_name" ]]; then
                sender_name=$(_K2_PERMISSION_QUIET=1 cli_request "/cli/workspace/resolve" "q=$(urlencode "$sender_path")" 2>/dev/null \
                    | python3 -c 'import json,sys,os
try:
    data = json.load(sys.stdin)
    path = data.get("path","")
    if path:
        print(os.path.basename(path.rstrip("/")))
except Exception:
    pass' 2>/dev/null)
            fi
            from="${sender_name:-external}"
        else
            from="external"
        fi
    fi

    # Single canonical endpoint. Returns the canonical MsgResponse
    # shape: {success, target_session_id, attempts, reason, hint}.
    #
    # 0.39.45 (#29/#35/#37): POST with the text in the form body — long
    # messages used to ride the URL query string and got silently
    # truncated at the daemon's request-head cap (~2.7KB of payload).
    local params="workspace=$(urlencode "$target")"
    params="${params}&text=$(urlencode "$text")"
    params="${params}&from=$(urlencode "$from")"
    # 0.39.25: optional slash-command, prepended at the very front of the
    # delivered message (before the [from <name>] prefix). Only sent when
    # non-empty; an older daemon simply ignores this unknown param.
    [[ -n "$explicit_command" ]] && params="${params}&command=$(urlencode "$explicit_command")"
    # Issue #9 — wake gating. Send the resolved value EXPLICITLY (both
    # true and false): the daemon now defaults wake=ON, so `--no-wake`
    # MUST send `wake=false` to actually opt out. (An older daemon that
    # predates the param ignores it and never auto-woke — acceptable skew.)
    params="${params}&wake=${wake}"
    [[ -n "$wake_timeout" ]] && params="${params}&wake_timeout=$(urlencode "$wake_timeout")"
    [[ -n "$explicit_via" ]] && params="${params}&via=$(urlencode "$explicit_via")"
    [[ -n "$summary" ]] && params="${params}&summary=$(urlencode "$summary")"
    [[ -n "$session_id" ]] && params="${params}&session=$(urlencode "$session_id")"
    params="${params}&idempotency_key=$(urlencode "$_K2_IDEM_KEY")"

    local response
    # #58 Phase-1 close: prefer the scoped per-cell UDS for msg (the daemon
    # stamps `from` server-side); falls back to TCP+owner transparently.
    # 0.46.15: the socket-then-TCP fallback and a retry after no answer
    # carry the same key, so the daemon delivers once.
    _permission_note_reset
    response=$(_k2_idem_send cli_post_form "/cli/workspace/msg" "$params")
    local rc=$?
    # 0.44.4: the legacy GET retry for pre-0.39.45 daemons is gone — the
    # daemon refuses GET on this route (it changes state).
    echo "$response"
    _k2_moved_from_note "$response"
    _k2_idem_replay_note "$response"

    # Issue #9 — if the daemon WOKE a dormant canonical session to deliver,
    # surface it on stderr so the caller attributes the latency rather than
    # mistaking a multi-second wake for a hang. `woke`/`wake_ms` are only
    # present when a wake actually happened (older daemons omit them).
    echo "$response" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if d.get("woke"):
        ms = d.get("wake_ms")
        secs = " (%.1fs)" % (ms/1000.0) if isinstance(ms,(int,float)) else ""
        sys.stderr.write("note: peer was dormant — woke canonical session%s — delivered\n" % secs)
    if d.get("revived"):
        sys.stderr.write("note: reopened a past conversation as %s; your message is its first turn. Reply arrives as [from %s].\n" % (d.get("addr"), d.get("addr")))
except Exception:
    pass' 2>/dev/null

    # C2 (0.40.45): 403 not_connected / gated → exit 3 + teaching stderr
    # (mail/dns shape). deliver-or-fail MsgResponse still exits 2.
    local gate_rc
    gate_rc=$(echo "$response" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    err = d.get("error")
    if isinstance(err, dict):
        code = err.get("code") or ""
        if code in ("not_connected", "gated", "forbidden"):
            hint = err.get("hint") or code
            sys.stderr.write(json.dumps({"error": {"code": code, "hint": hint}},
                                        ensure_ascii=False, separators=(",", ":")) + "\n")
            print(3)
            raise SystemExit
    if d.get("success") is True:
        print(0)
    else:
        print(2)
except SystemExit:
    pass
except Exception:
    print(2)
' 2>/dev/null)
    if [[ "$gate_rc" == "3" ]]; then
        return 3
    fi
    if [[ "$gate_rc" != "0" ]]; then
        return 2
    fi
    return $rc
}

