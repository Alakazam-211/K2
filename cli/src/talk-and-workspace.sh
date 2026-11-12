# ── BETA: `k2 talk` — fused read+send inter-agent collaboration verb ──
#
# The HITL-aware companion to `msg`. `msg` is a deliberate, blind
# fire-and-forget send (the ~95%-working path); `talk` FUSES "read the
# peer's live screen" + "send" into one call so the calling agent
# CAN'T forget to look before it injects — the exact slip that gets a
# message *eaten* by a human-in-the-loop prompt (a y/n / 1·2·3 /
# confirm / arrow-menu the recipient is mid-answering, which silently
# consumes the injected bytes as the answer).
#
# A CLI cannot comprehend — `talk` is a guided PROTOCOL the CALLING
# agent (which IS an LLM) follows. The CLI only surfaces the screen +
# guidance; the agent reads it, judges ready-vs-HITL, and decides to
# drive / back off / deliver. There is NO daemon-side HITL detector and
# NO new daemon route here: `talk` reuses `k2 read` (GET
# /cli/terminal/read) for the read and `k2 msg` (/cli/workspace/msg)
# for the send.
#
#   k2 talk <workspace> "<message>"   read the peer's screen, then send
#   k2 talk <workspace> --read-only   read + judge only; do NOT send
#                                     (use to BACK OFF / re-check later)
#
# ── GH#8 #69 (Wave 2) — batch HITL "scout" + batch-answer ────────────
# Claude's AskUserQuestion renders N questions CO-RESIDENT: ←/→ move
# between them, a number key auto-selects + ADVANCES (no Enter), and the
# last question lands on a "Review your answers" → "1. Submit / 2.
# Cancel" screen. `--scout` surfaces the WHOLE form in one call
# (arrow-only, NON-destructive, left on Q1); `--answers a,b,c` drives the
# whole form (N digits, auto-advancing, + a final `1` to submit) in one
# call. Both reuse the SAME /cli/terminal/write?...&no_submit=true raw
# path the single `--answer` uses — no daemon changes.
#
# Tunables are named so the test agent can tweak timing/keys without
# hunting; each is env-overridable for quick iteration.
K2_TALK_NAV_SETTLE="${K2_TALK_NAV_SETTLE:-0.35}"       # settle (s) between an arrow-nav and the re-read (scout)
K2_TALK_ANSWER_SETTLE="${K2_TALK_ANSWER_SETTLE:-0.35}" # settle (s) between batch answer digits
K2_TALK_SUBMIT_KEY="${K2_TALK_SUBMIT_KEY:-1}"          # key selecting "1. Submit answers" on the review screen
K2_TALK_SCOUT_MAX="${K2_TALK_SCOUT_MAX:-20}"           # safety cap on questions walked when N is unknown
K2_TALK_ARROW_RIGHT=$'\033[C'   # → move to the NEXT question (Esc [ C)
K2_TALK_ARROW_LEFT=$'\033[D'     # ← move to the PREVIOUS question (Esc [ D)

# Read a session's screen as raw newline-joined text (no daemon JSON
# wrapper) for parsing. Best-effort; empty on any failure.
_talk_read_raw() {
    local sid="$1" lines="$2"
    cli_request "/cli/terminal/read" "id=$(urlencode "$sid")&lines=$lines" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    print('\n'.join(d.get('lines', [])))
except Exception:
    pass
" 2>/dev/null
}

# GH#8 — `talk`'s HITL DETECTION step. Calls the daemon's
# /cli/terminal/classify over the peer's live screen and echoes the
# verdict as "<is_hitl>|<source>" where <is_hitl> is `true`/`false`/`null`
# and <source> is `fastpath`/`unavailable`. Empty on request failure
# (caller treats empty as "not detected" → deliver).
#
# The endpoint matches known prompt patterns only: a no-marker screen comes
# back `null|unavailable`. `talk` keys ONLY on is_hitl==true to auto-scout;
# `false` and `unavailable` both deliver (the agent already saw the screen).
# Read-only; addresses the session by workspace[+agent], same as `read`.
_talk_classify() {
    local target="$1" agent="$2" lines="$3"
    local params="workspace=$(urlencode "$target")&lines=$lines"
    [[ -n "$agent" ]] && params="${params}&agent=$(urlencode "$agent")"
    cli_request "/cli/terminal/classify" "$params" | python3 -c "
import json, sys
try:
    d = json.loads(sys.stdin.read())
    h = d.get('is_hitl', None)
    s = d.get('source', '')
    hs = 'null' if h is None else ('true' if h else 'false')
    print(hs + '|' + s)
except Exception:
    pass
" 2>/dev/null
}

# Parse N (total questions) from a 'Question X of Y' header. Echoes Y, or
# nothing if the header is absent.
_talk_parse_total() {
    # NON-FATAL (GH#12): the trailing `|| true` is load-bearing. The
    # leading grep exits 1 when there's no "Question X of Y" header —
    # and Claude Code's AskUserQuestion renders a TAB BAR, not that
    # header, so it NEVER matches on a real form. Under `set -euo
    # pipefail` an unguarded `total=$(_talk_parse_total …)` would inherit
    # that non-zero and abort --scout/--answers before they do anything.
    # Empty output here just routes both to their no-header fallbacks
    # (scout: walk-until-screen-stable; answers: best-effort drive).
    # TODO(GH#12): replace header-parse with tab-bar (☐…✔ Submit) count.
    printf '%s' "$1" \
        | grep -oiE 'Question[[:space:]]+[0-9]+[[:space:]]+of[[:space:]]+[0-9]+' \
        | head -1 | grep -oE '[0-9]+' | tail -1 || true
}

# GH#12 — Parse the TAB BAR Claude Code's AskUserQuestion actually
# renders (there is NO "Question X of Y" header). The bar looks like:
#
#     ←  ☐ Embodiment  ☒ Hot Take  ☐ Memoir  ✔ Submit  →
#
# Every `☐`/`☒ <name>` segment is a REAL question; the trailing
# `✔ Submit` segment is nav, NOT a question. Echoes one question name
# per line, IN ORDER, excluding Submit. Empty output ⇒ no parseable tab
# bar (caller falls back to walk-until-stable). Read-only.
_talk_parse_tabbar() {
    printf '%s' "$1" | python3 -c '
import sys, re
text = sys.stdin.read()
# The tab-bar line carries a question marker (☐/☒) AND the Submit tab.
tab = None
for ln in text.splitlines():
    if ("☐" in ln or "☒" in ln) and ("✔" in ln or "✓" in ln or "Submit" in ln):
        tab = ln
        break
if tab is None:
    sys.exit(0)
# Split into "<marker> <name>" segments: a marker, then everything up to
# the NEXT marker or a nav arrow (← →). Question markers = ☐ ☒; the
# Submit tab uses ✔/✓ (and/or is literally named "Submit") — excluded.
for marker, name in re.findall(r"([☐☒✔✓])([^☐☒✔✓→←]*)", tab):
    name = name.strip().strip("→←").strip()
    if marker in ("☐", "☒") and name and name != "Submit":
        print(name)
' 2>/dev/null
}

# GH#12 (optional tidy) — Trim a full-screen capture to the question
# region: from the tab-bar line through the LAST numbered option
# (`❯ 1. …` / `2. …`). Best-effort — echoes the FULL screen UNCHANGED if
# the region can't be bounded (correctness over tidiness).
_talk_trim_screen() {
    printf '%s' "$1" | python3 -c '
import sys, re
lines = sys.stdin.read().splitlines()
start = None
for i, ln in enumerate(lines):
    if ("☐" in ln or "☒" in ln) and ("✔" in ln or "✓" in ln or "Submit" in ln):
        start = i
        break
opt = re.compile(r"^\s*[❯>]?\s*\d+[.)]\s")
end = None
for i in range(len(lines) - 1, -1, -1):
    if opt.match(lines[i]):
        end = i
        break
if start is None or end is None or end < start:
    sys.stdout.write("\n".join(lines))
else:
    sys.stdout.write("\n".join(lines[start:end + 1]))
' 2>/dev/null
}

# GH#8 #69 / GH#12 — `--scout`: surface an ENTIRE multi-question form in
# ONE call. NON-destructive: presses ONLY arrow keys (never a number),
# walks → over every question capturing each rendered screen, then walks
# ← back the SAME number of steps so the form is left untouched, focused
# on Q1. The TAB BAR (`← ☐ name … ✔ Submit →`) is the authoritative count
# + names — the walk STOPS before the `✔ Submit` tab (never captures it
# as a phantom question). Walk-until-stable is the last-resort fallback.
talk_scout() {
    local sid="$1" lines="$2" target="$3"
    local settle="$K2_TALK_NAV_SETTLE"
    local right_url left_url
    right_url=$(urlencode "$K2_TALK_ARROW_RIGHT")
    left_url=$(urlencode "$K2_TALK_ARROW_LEFT")

    echo "── talk(BETA) scout: walking $target's form (arrow-only, non-destructive) ──" >&2

    # GH#12 — Authoritative count + names from the tab bar on the FIRST
    # screen (the full bar is visible regardless of focused question).
    # `total` empty ⇒ no parseable tab bar ⇒ walk-until-stable fallback.
    local first_screen names_blob
    first_screen=$(_talk_read_raw "$sid" "$lines")
    names_blob=$(_talk_parse_tabbar "$first_screen")
    local names=()
    if [[ -n "$names_blob" ]]; then
        local _oldifs="$IFS"; IFS=$'\n'
        while IFS= read -r _n; do [[ -n "$_n" ]] && names+=("$_n"); done <<< "$names_blob"
        IFS="$_oldifs"
    fi
    local total=""
    [[ "${#names[@]}" -gt 0 ]] && total="${#names[@]}"

    local captures=() prev="" screen
    local i=1 rights=0
    while :; do
        screen=$(_talk_read_raw "$sid" "$lines")
        captures+=("$screen")
        if [[ -n "$total" ]]; then
            # Tab-bar count is authoritative: stop after N captures so the
            # walk NEVER advances onto (or captures) the ✔ Submit tab.
            [[ "$i" -ge "$total" ]] && break
        else
            # Fallback (no tab bar): stop when the screen stops changing —
            # drop the duplicate trailing capture and back off one nav.
            if [[ -n "$prev" && "$screen" == "$prev" ]]; then
                unset 'captures[${#captures[@]}-1]'
                break
            fi
        fi
        [[ "$i" -ge "$K2_TALK_SCOUT_MAX" ]] && break
        prev="$screen"
        # Advance → to the next question.
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$sid")&message=${right_url}&no_submit=true" >/dev/null
        sleep "$settle"
        rights=$((rights + 1))
        i=$((i + 1))
    done

    # Walk ← back to Q1: exactly as many lefts as rights → form unchanged.
    local b
    for ((b = 0; b < rights; b++)); do
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$sid")&message=${left_url}&no_submit=true" >/dev/null
        sleep "$settle"
    done

    # Comma-joined question names for the count line (GH#12).
    local names_csv=""
    if [[ "${#names[@]}" -gt 0 ]]; then
        local _nm
        for _nm in "${names[@]}"; do
            names_csv="${names_csv:+$names_csv, }$_nm"
        done
    fi

    # Raw screens (the source of truth) → stdout, delimited per question.
    # Each capture is TRIMMED to the question + options region (best-effort;
    # falls back to the full screen if it can't be bounded — GH#12).
    local idx=1 cap qname trimmed
    for cap in "${captures[@]}"; do
        qname="${names[$((idx - 1))]:-}"
        trimmed=$(_talk_trim_screen "$cap")
        [[ -z "$trimmed" ]] && trimmed="$cap"
        echo ""
        if [[ -n "$qname" ]]; then
            echo "=== Question $idx of ${total} — ${qname} ==="
        else
            echo "=== Question $idx${total:+ of $total} (raw screen) ==="
        fi
        printf '%s\n' "$trimmed"
        idx=$((idx + 1))
    done

    # Light best-effort parsed summary → stderr (raw screens above win).
    {
        echo ""
        echo "── scout parsed summary (best-effort; raw screens above are authoritative) ──"
        if [[ -n "$total" ]]; then
            echo "   form: $total question(s): ${names_csv}; left focused on Q1, unmodified."
        else
            echo "   form: ? question(s) (tab bar unparsed); left focused on Q1, unmodified."
        fi
        idx=1
        for cap in "${captures[@]}"; do
            qname="${names[$((idx - 1))]:-}"
            echo "Q$idx: ${qname:-(name unparsed — see raw screen)}"
            printf '%s\n' "$cap" | grep -E '^[[:space:]]*❯?[[:space:]]*[0-9]+[\.\)][[:space:]]' | sed 's/^[[:space:]]*/   /' || true
            idx=$((idx + 1))
        done
        echo ""
        echo "── to answer: k2 talk $target --answers <a,b,c>  (N digits + auto final 1 to submit) ──"
    } >&2

    # GH#12 — scout succeeded: explicit success so the dispatch (under
    # set -e) returns 0 even if the last grep above matched nothing.
    return 0
}

# GH#8 #69 — `--answers a,b,c`: drive a form currently focused on Q1.
# Sends each digit (auto-selects + advances), settling between, then a
# final K2_TALK_SUBMIT_KEY ('1') to pick "Submit answers" on the review
# screen. Re-reads + prints the outcome.
talk_answers() {
    local sid="$1" lines="$2" csv="$3" target="$4"
    local settle="$K2_TALK_ANSWER_SETTLE"

    # Split the comma-list (trim incidental spaces around each answer).
    local ans=() oldifs="$IFS"
    IFS=',' read -ra ans <<< "$csv"
    IFS="$oldifs"
    local cleaned=() a t
    for a in "${ans[@]}"; do
        t="${a//[[:space:]]/}"
        [[ -n "$t" ]] && cleaned+=("$t")
    done
    local count=${#cleaned[@]}
    if [[ "$count" -eq 0 ]]; then
        echo "Error: --answers needs a comma-list of answers (e.g. --answers 1,2,1)" >&2
        return 1
    fi

    # Best-effort N from the current screen's header → warn on mismatch
    # (read-only; still proceed best-effort).
    local total
    total=$(_talk_parse_total "$(_talk_read_raw "$sid" "$lines")")
    if [[ -n "$total" && "$total" != "$count" ]]; then
        echo "talk(BETA) answers: WARNING — $count answers given but form has $total question(s); proceeding best-effort." >&2
    fi

    echo "── talk(BETA) answers: driving $target — ${cleaned[*]} then submit ('$K2_TALK_SUBMIT_KEY') ──" >&2
    # One digit per question — auto-selects + advances (no Enter).
    for a in "${cleaned[@]}"; do
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$sid")&message=$(urlencode "$a")&no_submit=true" >/dev/null
        sleep "$settle"
    done
    # Final submit: pick "1. Submit answers" on the review screen.
    cli_request_post "/cli/terminal/write" "id=$(urlencode "$sid")&message=$(urlencode "$K2_TALK_SUBMIT_KEY")&no_submit=true" >/dev/null
    sleep "$settle"

    echo "── talk(BETA) answers: $target's screen after submit ──" >&2
    cmd_terminal_read "$sid" --lines "$lines" || true
    echo "───────────────────────────────────────────────────────────────" >&2
}

cmd_talk() {
    for arg in "$@"; do
        case "$arg" in
            --help|-h) cmd_help_talk; return 0 ;;
        esac
    done

    local target="" text="" from="" command="" lines=50 agent="" session="" token=""
    local read_only="false"
    local answer="" respond=""
    local scout="false" answers=""
    # Issue #9 — `talk` is the explicit "interact now" verb, so it
    # AUTO-WAKES a dormant canonical session by default (read → wake →
    # wait READY → deliver). `--no-wake` suppresses that; `--wake-timeout`
    # bounds the post-wake readiness wait.
    local wake="true"
    local wake_timeout=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --from)      from="$2"; shift 2 ;;
            --command)   command="$2"; shift 2 ;;
            --lines)     lines="$2"; shift 2 ;;
            --agent)     agent="$2"; shift 2 ;;
            --session)   session="$2"; shift 2 ;;
            --token)     token="$2"; shift 2 ;;
            --answer)    answer="$2"; shift 2 ;;
            --respond)   respond="$2"; shift 2 ;;
            --scout)     scout="true"; shift ;;
            --answers)   answers="$2"; shift 2 ;;
            --no-wake)   wake="false"; shift ;;
            --wake-timeout) wake_timeout="$2"; shift 2 ;;
            --read-only|--dry-run|--peek) read_only="true"; shift ;;
            -*)
                echo "Error: unknown flag '$1' (try 'k2 talk --help')" >&2
                exit 1 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"
                else text="$1"
                fi
                shift ;;
        esac
    done

    [[ -z "$target" ]] && {
        echo "Error: workspace required" >&2
        echo "Usage: k2 talk <workspace> \"text\" [--read-only] [--from <name>] [--lines N] [--agent <name>]" >&2
        echo "       Run 'k2 talk --help' for the full BETA protocol." >&2
        exit 1
    }

    # Remote addressing (K2-as-a-server): <workspace>@<host> talks to a
    # workspace on a CONNECTED remote daemon (owner token over the tunnel) —
    # the SAME model as local cross-workspace talk, just a remote base URL.
    # --session <id> scopes the read/drive/deliver to a specific session;
    # omit it for the workspace's canonical session (the default).
    _split_remote_target "$target"; target="$_BARE_TARGET"
    _apply_remote_host "$_REMOTE_HOST" "$token" || exit 1
    local resolve_target="$target"
    [[ -n "$session" ]] && resolve_target="$session"

    # ── act-returns-state (BETA, GH#8) ── the #1 round-trip win: fuse a
    # DRIVE step (answer a HITL) with the confirm re-read so it's ONE
    # call instead of write-then-separately-re-read. `--answer <N>` sends
    # key <N>; `--respond <y|n>` sends y/n — both via the SAME raw
    # TUI-control path cmd_terminal_write uses (/cli/terminal/write to the
    # resolved session), then IMMEDIATELY re-read + print the resulting
    # screen so the caller sees the result with no separate `read`.
    #
    # Deliberately NO option validation and NO structured-state parsing
    # here (that's the next wave — it needs screen-parsing). `--answer N`
    # just sends the digit, best-effort, exactly like a manual
    # `terminal write <id> "N"` but fused with the re-read.
    # ── batch HITL (BETA, GH#8 #69 / Wave 2) ── `--scout` surfaces the
    # WHOLE form in one call (arrow-only, non-destructive, left on Q1);
    # `--answers a,b,c` drives the whole form (N digits + final submit) in
    # one call. Both reuse the same raw /cli/terminal/write?...&no_submit
    # path the single `--answer` uses.
    if [[ "$scout" == "true" ]]; then
        local scout_sid
        scout_sid=$(resolve_write_session_id "$resolve_target" "$agent") || exit 1
        talk_scout "$scout_sid" "$lines" "$target"
        return 0
    fi
    if [[ -n "$answers" ]]; then
        local ans_sid
        ans_sid=$(resolve_write_session_id "$resolve_target" "$agent") || exit 1
        talk_answers "$ans_sid" "$lines" "$answers" "$target"
        return 0
    fi

    if [[ -n "$answer" || -n "$respond" ]]; then
        if [[ -n "$answer" && -n "$respond" ]]; then
            echo "Error: pass only one of --answer / --respond" >&2
            exit 1
        fi
        # --respond is an ALIAS of --answer for numbered menus: map the
        # y/n confirm words onto the standard "1. Yes / 2. No" ordering
        # (y→1, n→2); a bare number passes straight through.
        local key="$answer"
        if [[ -n "$respond" ]]; then
            case "$respond" in
                y|Y|yes|YES|Yes) key="1" ;;
                n|N|no|NO|No)     key="2" ;;
                *)                key="$respond" ;;
            esac
        fi
        local drive_sid
        drive_sid=$(resolve_write_session_id "$resolve_target" "$agent") || exit 1
        echo "── talk(BETA): driving $target — sending '$key' ───────────────" >&2
        # no_submit=true: send the key WITHOUT the daemon's trailing \r.
        # In Claude's AskUserQuestion menu the number key auto-selects +
        # advances, so an injected Enter then confirms the NEXT question's
        # default — answering two questions per call (GH#11). Send exactly
        # the one key and let the menu confirm it itself.
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$drive_sid")&message=$(urlencode "$key")&no_submit=true" >/dev/null
        # Settle: give the TUI a beat to repaint after the keypress before
        # re-reading so the printed screen reflects the answer.
        sleep 0.7
        echo "── talk(BETA): $target's screen after '$key' ──────────────────" >&2
        # Re-read the EXACT session we just drove (by its UUID) so the
        # printed screen is guaranteed to reflect the answer — even in a
        # multi-session workspace where the canonical read might pick a
        # different agent's session than the one resolved for the write.
        cmd_terminal_read "$drive_sid" --lines "$lines" || true
        echo "───────────────────────────────────────────────────────────────" >&2
        return 0
    fi

    if [[ "$read_only" != "true" && -z "$text" ]]; then
        echo "Error: message text required (or pass --read-only to just peek)" >&2
        echo "Usage: k2 talk <workspace> \"text\" [--read-only] [--from <name>] [--lines N] [--agent <name>]" >&2
        exit 1
    fi

    # ── 1. READ FIRST ── surface the peer's live screen so the agent
    # sees ready-vs-HITL BEFORE anything is sent. Reuses `k2 read`
    # (GET /cli/terminal/read). `|| read_rc=$?` keeps `set -e` from
    # aborting talk when the peer is asleep / not found — we still want
    # to proceed (msg can wake it) and report.
    echo "── talk(BETA): $target's live screen ──────────────────────────" >&2
    local read_args=("$target" --lines "$lines")
    [[ -n "$agent" ]] && read_args+=(--agent "$agent")
    [[ -n "$session" ]] && read_args+=(--session "$session")
    local read_rc=0
    cmd_read "${read_args[@]}" || read_rc=$?
    echo "───────────────────────────────────────────────────────────────" >&2

    # ── 2. JUDGE (the agent's job, not the CLI's) ── surface the
    # drive-or-back-off protocol from the message-composer PRD (D8).
    cat >&2 <<EOF

talk(BETA): you just READ $target's screen above. The COMPREHENSION is
yours — a CLI can't judge it. Before this message lands, decide:
  • READY (idle/empty prompt)? → deliver; it lands cleanly.
  • In a HITL you should NOT answer for it (a canonical/federation peer
    waiting on its OWNER's permission prompt, or any peer you lack
    standing over)? → BACK OFF: re-run with --read-only in a few minutes
    and only deliver once it's READY. Don't answer someone else's prompt.
  • In a HITL you DO have standing over? → DRIVE it first via raw TUI
    control ('k2 terminal write <id> "y"/"1"/Enter…'), re-read, repeat
    for multi-step chains, THEN deliver.
EOF

    if [[ "$read_only" == "true" ]]; then
        echo "talk(BETA): --read-only — nothing sent. Re-run without it to deliver." >&2
        [[ -n "$text" ]] && echo "  (held message: $text)" >&2
        return 0
    fi

    # ── 2b. AUTO-DETECT HITL (GH#8) ── before delivering, ask the daemon's
    # /cli/terminal/classify whether $target is sitting on a human-in-the-
    # loop prompt. Detection matches known prompt patterns only; a
    # no-marker screen returns `unavailable`.
    #
    #   • is_hitl == true  → AUTO-START the scout (surface the whole form,
    #     arrow-only / non-destructive) and HOLD the message — don't let it
    #     get eaten by the open prompt.
    #   • is_hitl == false / null / unavailable / (request failed) → DELIVER.
    #     The agent ALREADY saw the screen from the read above, and an
    #     `unavailable`/`false` verdict NEVER claims the peer is clear — so
    #     this preserves today's behavior (read-before-send) when there's no
    #     detector, exactly the degrade-gracefully contract.
    local verdict="" det_is_hitl="" det_source=""
    verdict=$(_talk_classify "$target" "$agent" "$lines" || true)
    if [[ -n "$verdict" ]]; then
        det_is_hitl="${verdict%%|*}"
        det_source="${verdict#*|}"
    fi
    if [[ "$det_is_hitl" == "true" ]]; then
        echo "── talk(BETA): HITL detected on $target (source: ${det_source:-?}) — holding the message, scouting the form ──" >&2
        local scout_sid
        if scout_sid=$(resolve_write_session_id "$resolve_target" "$agent"); then
            talk_scout "$scout_sid" "$lines" "$target"
        fi
        echo "" >&2
        echo "talk(BETA): $target is in a human-in-the-loop prompt — message HELD, NOT delivered:" >&2
        [[ -n "$text" ]] && echo "  (held message: $text)" >&2
        echo "talk(BETA): answer the form above with  k2 talk $target --answers <a,b,c>  (or single-step --answer <N> / --respond <y|n>)," >&2
        echo "            then re-run  k2 talk $target \"…\"  once it's READY to deliver." >&2
        return 0
    fi

    # ── 3. SEND ── deliver via the existing `k2 msg` path
    # (/cli/workspace/msg). `talk` adds NO new send mechanism — `msg`
    # stays the one delivery throat. Exit code mirrors msg's.
    echo "talk(BETA): delivering to ${target}…" >&2
    # --session: deliver straight into the addressed session (raw write +
    # submit) then re-read it — the per-session analogue of the canonical
    # msg deliver below. Used for "talk to THIS session" on a remote host.
    if [[ -n "$session" ]]; then
        local deliver_sid
        deliver_sid=$(resolve_write_session_id "$resolve_target" "$agent") || exit 1
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$deliver_sid")&message=$(urlencode "$text")" >/dev/null
        sleep 0.7
        cmd_terminal_read "$deliver_sid" --lines "$lines" || true
        return 0
    fi
    local msg_args=("$target" "$text")
    [[ -n "$from" ]] && msg_args+=(--from "$from")
    [[ -n "$command" ]] && msg_args+=(--command "$command")
    msg_args+=(--via talk)
    # Issue #9 — talk auto-wakes a dormant peer (suppress with --no-wake).
    if [[ "$wake" == "true" ]]; then
        msg_args+=(--wake)
        [[ -n "$wake_timeout" ]] && msg_args+=(--wake-timeout "$wake_timeout")
    else
        msg_args+=(--no-wake)
    fi
    local send_rc=0
    cmd_msg "${msg_args[@]}" || send_rc=$?
    return "$send_rc"
}

cmd_heartbeat_wake() {
    # Automated manager wake cycle:
    # 1. Check if any agents have inbox work
    # 2. If manager is running → send triage message
    # 3. If manager is asleep → launch with triage prompt
    # Check both agent inboxes and workspace inbox for work
    local has_work
    has_work=$(python3 -c "
import json, sys, subprocess, os

port = os.environ.get('K2SO_PORT', '')
token = os.environ.get('K2SO_HOOK_TOKEN', '')
if not port:
    try: port = open(os.path.expanduser('$K2_HOME/heartbeat.port')).read().strip()
    except: pass
if not token:
    try: token = open(os.path.expanduser('$K2_HOME/heartbeat.token')).read().strip()
    except: pass

project = '$PROJECT'
base = f'http://127.0.0.1:{port}'

agent_inbox = False
ws_inbox = False
try:
    import urllib.request
    req = urllib.request.Request(f'{base}/cli/agents/list?token={token}&project={urllib.parse.quote(project)}')
    with urllib.request.urlopen(req, timeout=5) as resp:
        agents = json.loads(resp.read())
        agent_inbox = any(a.get('inboxCount', 0) > 0 for a in agents)
except: pass
try:
    req = urllib.request.Request(f'{base}/cli/work/inbox?token={token}&project={urllib.parse.quote(project)}')
    with urllib.request.urlopen(req, timeout=5) as resp:
        items = json.loads(resp.read())
        ws_inbox = len(items) > 0 if isinstance(items, list) else False
except: pass

print('yes' if (agent_inbox or ws_inbox) else 'no')
" 2>/dev/null || echo "no")

    if [ "$has_work" != "yes" ]; then
        echo '{"status":"noop","reason":"no inbox work found"}'
        return
    fi

    # Check if manager is running via DB session first, fall back to terminal scan
    local manager_terminal=""
    local manager_status=""

    # Try DB session lookup
    local session_info
    session_info=$(python3 -c "
import json, os, sqlite3
try:
    db = os.path.expanduser('$(_k2_home_db)')
    # DB-A9: read-only, and never create a missing file.
    uri = 'file:' + db.replace('%', '%25').replace('?', '%3f').replace('#', '%23') + '?mode=ro'
    conn = sqlite3.connect(uri, uri=True)
    project = '$PROJECT'
    row = conn.execute('SELECT id FROM projects WHERE path = ?', (project,)).fetchone()
    if not row:
        print('{}')
    else:
        pid = row[0]
        sessions = conn.execute(
            'SELECT agent_name, terminal_id, status FROM agent_sessions WHERE project_id = ?', (pid,)
        ).fetchall()
        for name, tid, status in sessions:
            print(json.dumps({'agent': name, 'terminalId': tid or '', 'status': status}))
            break
        else:
            print('{}')
    conn.close()
except Exception as e:
    print('{}')
" 2>/dev/null)

    if [ -n "$session_info" ] && [ "$session_info" != "{}" ]; then
        manager_terminal=$(echo "$session_info" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); print(d.get('terminalId',''))" 2>/dev/null)
        manager_status=$(echo "$session_info" | python3 -c "import json,sys; d=json.loads(sys.stdin.read()); print(d.get('status',''))" 2>/dev/null)
    fi

    # Fall back to scanning running terminals if no DB session
    if [ -z "$manager_terminal" ] || [ "$manager_status" != "running" ]; then
        local running
        running=$(cli_request "/cli/agents/running" 2>/dev/null)
        manager_terminal=$(echo "$running" | python3 -c "
import json, sys
def agent_from_tid(tid):
    # New format: agent-chat:<project_id>:<agent>[:hb:<heartbeat>]
    # Worktree:   agent-chat:wt:<workspace_id>     -> no agent name
    # Legacy:     agent-chat-<agent>               (and -wt-<wsid>)
    if tid.startswith('agent-chat:'):
        rest = tid[len('agent-chat:'):]
        if rest.startswith('wt:'):
            return None
        parts = rest.split(':')
        return parts[1] if len(parts) >= 2 and parts[0] and parts[1] else None
    if tid.startswith('agent-chat-') and not tid.startswith('agent-chat-wt-'):
        return tid[len('agent-chat-'):] or None
    return None

try:
    data = json.loads(sys.stdin.read())
    for t in data:
        cwd = t.get('cwd', '')
        tid = t.get('terminalId', '')
        agent = agent_from_tid(tid)
        if '.k2/agents/manager' in cwd or '.k2so/agents/manager' in cwd or '.k2/agents/coordinator' in cwd or '.k2so/agents/coordinator' in cwd or agent in ('manager', 'coordinator'):
            print(tid)
            break
except: pass
" 2>/dev/null)
    fi

    if [ -n "$manager_terminal" ]; then
        # Manager is awake — send triage message
        local msg="New work detected. Run k2 inbox to see your inbox and triage work."
        cli_request_post "/cli/terminal/write" "id=$(urlencode "$manager_terminal")&message=$(urlencode "$msg")"
        echo "{\"status\":\"notified\",\"terminalId\":\"$manager_terminal\"}"
    else
        # Manager is asleep — launch it, then send triage message after startup
        cli_request_post "/cli/agents/launch" "agent=manager"
        # Wait for the terminal to be created and Claude to initialize
        local attempts=0
        while [ $attempts -lt 12 ]; do
            sleep 5
            attempts=$((attempts + 1))
            # Check if manager terminal now exists
            local check
            check=$(cli_request "/cli/agents/running" 2>/dev/null)
            local new_id
            new_id=$(echo "$check" | python3 -c "
import json, sys
try:
    data = json.loads(sys.stdin.read())
    for t in data:
        cwd = t.get('cwd', '')
        tid = t.get('terminalId', '')
        if '.k2/agents/manager' in cwd or '.k2so/agents/manager' in cwd or '.k2/agents/coordinator' in cwd or '.k2so/agents/coordinator' in cwd or tid.startswith('agent-chat-manager') or tid.startswith('agent-chat-coordinator'):
            print(tid)
            break
except: pass
" 2>/dev/null)
            if [ -n "$new_id" ]; then
                local msg="New work detected. Run k2 inbox to see your inbox and triage work."
                cli_request_post "/cli/terminal/write" "id=$(urlencode "$new_id")&message=$(urlencode "$msg")"
                echo "{\"status\":\"launched_and_notified\",\"terminalId\":\"$new_id\"}"
                return
            fi
        done
        echo '{"status":"launched","agent":"manager","note":"triage message not sent - terminal not ready"}'
    fi
}

cmd_agent_complete() {
    # Sub-agent completion: moves active work to done for human review
    # (Workspace States / auto-merge-by-source retired).
    local agent="" file=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --agent) agent="$2"; shift 2 ;;
            --file)  file="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    if [ -z "$agent" ] || [ -z "$file" ]; then
        echo "Usage: k2 agent complete --agent <name> --file <filename>" >&2
        exit 1
    fi
    cli_request_post "/cli/agent/complete" "agent=$(urlencode "$agent")&file=$(urlencode "$file")"
}

cmd_workspace_cleanup() {
    cli_request_post "/cli/workspace/cleanup"
}

# 0.37.5 — resolve resume-chat launch args for a workspace's pinned
# chat tab. Daemon-first: every thin client (Tauri pinned tab,
# companion, MCP, this CLI) hits the same daemon route. Returns JSON
# `{command, args, cwd, resumeSession, resumedExisting}`.
#
# When `workspace_sessions.session_id` points at an on-disk JSONL,
# returns `--resume <id>` (continues the conversation). Otherwise
# pre-allocates a fresh UUID, persists it to SQL, and returns
# `--session-id <new>`.
#
#   k2 workspace resume-chat-args <workspace>
cmd_workspace_resume_chat_args() {
    local target="${1:-}"
    [[ -z "$target" ]] && { echo "Usage: k2 workspace resume-chat-args <workspace>" >&2; exit 1; }
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    if [ -z "$resolved_path" ]; then
        resolved_path="$target"
    fi
    cli_request "/cli/workspace/resume-chat-args" "project=$(urlencode "$resolved_path")"
}

# Replace the workspace's pinned chat with a new session of another
# harness. The daemon copies a transcript seed (and optional notes)
# into that new chat. It does not resume the saved session. Heartbeats
# on the pinned chat follow it; ones with their own session stay put.
# The newer doors are `k2 agent harness <workspace> <harness>` and, for the
# same harness, `k2 agent refresh`. Since 0.46.6 an agent never moves its chat
# to another harness (exit 3, harness_needs_person); on the same harness it's
# a refresh, with that verb's rule (0.46.8).
#
#   k2 workspace swap-canonical <workspace> --agent claude
#   k2 workspace swap-canonical <workspace> --agent claude --full --notes-file handoff.md
cmd_workspace_swap_canonical() {
    local target="" agent="" mode="recent" notes_file="" model=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --model)
                model="${2:-}"
                if [[ -z "$model" ]]; then
                    echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
                    exit 2
                fi
                shift 2
                ;;
            --agent|--provider)
                agent="${2:-}"
                if [[ -z "$agent" ]]; then
                    echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
                    exit 2
                fi
                shift 2
                ;;
            --mode)
                mode="${2:-}"
                if [[ -z "$mode" ]]; then
                    echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
                    exit 2
                fi
                shift 2
                ;;
            --recent) mode="recent"; shift ;;
            --full) mode="full"; shift ;;
            --notes-file)
                notes_file="${2:-}"
                if [[ -z "$notes_file" ]]; then
                    echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
                    exit 2
                fi
                shift 2
                ;;
            -*)
                echo "Unknown flag: $1" >&2
                echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
                exit 2
                ;;
            *)
                if [[ -z "$target" ]]; then
                    target="$1"
                else
                    echo "Unexpected argument: $1" >&2
                    exit 2
                fi
                shift
                ;;
        esac
    done
    if [[ -z "$target" || -z "$agent" ]]; then
        echo "Usage: k2 workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]" >&2
        exit 2
    fi
    case "$mode" in
        recent|full) ;;
        *) echo "mode must be recent or full" >&2; exit 2 ;;
    esac
    if [[ -n "$notes_file" ]]; then
        if [[ ! -f "$notes_file" ]]; then
            echo "Error: file not found: $notes_file" >&2
            exit 1
        fi
    fi
    # Inside a K2 terminal the daemon must see this session's passport,
    # never the owner token: an agent may swap only its own workspace's
    # pinned chat, and its handoff is stamped as from that agent.
    local auth_token="$TOKEN"
    if [ -n "$(_hook_sock)" ] && ! _cli_skip_uds; then
        if [ -z "${SCOPED_TOKEN:-}" ]; then
            _cli_require_session_passport "workspace swap-canonical"
        fi
        auth_token="$SCOPED_TOKEN"
    fi
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null || true)
    if [ -z "$resolved_path" ]; then
        resolved_path="$target"
    fi
    # The notes file is read by python and the body goes to curl on stdin,
    # so a long file never hits the per-argument / environment size limit.
    local out rc=0
    out=$(SWAP_PROJECT="$resolved_path" SWAP_PROVIDER="$agent" SWAP_MODE="$mode" SWAP_NOTES_FILE="$notes_file" SWAP_MODEL="$model" python3 -c '
import json, os
notes = ""
path = os.environ.get("SWAP_NOTES_FILE", "")
if path:
    with open(path, encoding="utf-8", errors="replace") as f:
        notes = f.read()
print(json.dumps({
    "project": os.environ.get("SWAP_PROJECT", ""),
    "provider": os.environ.get("SWAP_PROVIDER", ""),
    "mode": os.environ.get("SWAP_MODE", "recent"),
    "notes": notes,
    "model": os.environ.get("SWAP_MODEL", ""),
}))
' | curl -sS -X POST "${BASE_URL}/cli/workspace/swap-canonical?token=$(urlencode "$auth_token")" \
        --connect-timeout 5 --max-time 120 \
        -H "Content-Type: application/json" \
        --data-binary @- 2>/dev/null) || rc=$?
    if [[ "$rc" -ne 0 ]]; then
        echo "Error: could not reach the daemon" >&2
        exit 1
    fi
    # Permission cards: an agent's refusal ends with its Ask-with line.
    _permission_note "$out"
    printf '%s' "$out" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:300], file=sys.stderr)
    sys.exit(1)
if d.get("error") and not d.get("swapped"):
    err = d["error"]
    code = err.get("code") if isinstance(err, dict) else ""
    if isinstance(err, dict):
        err = err.get("hint") or err.get("code") or err
    print("Error: %s" % err, file=sys.stderr)
    path = d.get("handoffPath") or ""
    if path:
        print("Handoff file: %s" % path, file=sys.stderr)
    # 0.46.6: a refused caller (an agent, a wrong credential) exits 3.
    sys.exit(3 if code in ("harness_needs_person", "chat_refresh_needs_person", "chat_refresh_main_only", "other_workspace", "wrong_credential", "invalid_token") else 1)
if not d.get("swapped"):
    print("Error: canonical chat was not swapped", file=sys.stderr)
    sys.exit(1)
print("Swapped the canonical chat to %s." % (d.get("provider") or ""))
if d.get("model"):
    print("Model: %s (for this start; a later resume uses the preset or workspace model again)" % d["model"])
prev = d.get("fromProvider") or ""
if prev:
    print("Previous harness: %s" % prev)
sid = d.get("sessionId") or ""
if sid:
    print("Terminal session: %s" % sid)
conv = d.get("claudeSessionId") or ""
if conv:
    print("New conversation: %s" % conv)
path = d.get("handoffPath") or ""
if path:
    print("Handoff file: %s" % path)
seed_error = d.get("seedError") or ""
if seed_error:
    print("Transcript seed: %s" % seed_error)
if d.get("messageDelivered"):
    print("The handoff was sent into the new chat.")
else:
    reason = d.get("messageReason") or "not delivered"
    hint = d.get("messageHint") or ""
    print("The new chat is up. The handoff was not sent (%s)." % reason)
    if hint:
        print(hint)
    print("The handoff file is on disk. Paste it into the new chat if it did not arrive.")
print("Heartbeats on the pinned chat follow it; heartbeats that resume their own session were not moved.")
'
}

# 0.37.4 Phase B: set a v2 session's authoritative label.
# Daemon-side; broadcasts LabelChanged to every WS subscriber so
# every Tauri window + the mobile companion converge.
#
# Optional --no-lock leaves label_source as Pty/Seed (PTY title
# events can still update). Default locks the label.
#
#   k2 sessions set-label <session-uuid> <new label>
#   k2 sessions set-label <session-uuid> <new label> --no-lock
cmd_sessions_set_label() {
    local session_id="${1:-}"
    [[ -z "$session_id" ]] && { echo "Usage: k2 sessions set-label <session-id> <label> [--no-lock]" >&2; exit 1; }
    shift
    local lock="true"
    local label_parts=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --no-lock) lock="false"; shift ;;
            *) label_parts+=("$1"); shift ;;
        esac
    done
    local label="${label_parts[*]}"
    [[ -z "$label" ]] && { echo "Usage: k2 sessions set-label <session-id> <label> [--no-lock]" >&2; exit 1; }
    local params="id=$(urlencode "$session_id")&label=$(urlencode "$label")&lock=$lock"
    cli_request_post "/cli/sessions/label" "$params" \
        | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("Set label to: %s (locked=%s)" % (d.get("label",""), d.get("locked","?")))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
}

# 0.37.4: read the friendly display name for a workspace's primary
# agent. Reads .k2so/agent/AGENT.md frontmatter (display_name → name)
# then falls back to projects.name. Total — always returns a string.
cmd_workspace_get_agent_display_name() {
    local target="${1:-}"
    [[ -z "$target" ]] && { echo "Usage: k2 workspace agent-name <workspace>" >&2; exit 1; }
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    if [ -z "$resolved_path" ]; then
        # Treat as raw path if resolver didn't match a registered workspace.
        resolved_path="$target"
    fi
    cli_request "/cli/workspace/agent-display-name" "project=$(urlencode "$resolved_path")" \
        | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print(d.get("display_name", ""))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
}

# 0.37.4: write the friendly display name for a workspace's primary
# agent. Atomic AGENT.md frontmatter rewrite, scaffolds the file if
# missing. Does NOT touch the technical agent name (`name:`) or
# infrastructure layers (v2_session_map, terminal_id, pending_live).
cmd_workspace_set_agent_display_name() {
    local target="${1:-}"
    local new_name="${2:-}"
    [[ -z "$target" ]] && { echo "Usage: k2 workspace set-agent-name <workspace> <new-name>" >&2; exit 1; }
    [[ -z "$new_name" ]] && { echo "Usage: k2 workspace set-agent-name <workspace> <new-name>" >&2; exit 1; }
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    if [ -z "$resolved_path" ]; then
        resolved_path="$target"
    fi
    local params="project=$(urlencode "$resolved_path")&name=$(urlencode "$new_name")"
    cli_request_post "/cli/workspace/set-agent-display-name" "$params" \
        | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("Set agent display name to: %s" % d.get("display_name", ""))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
}

# 0.46.1: `k2 agent identity` — an agent's name, color and picture, read
# or changed in one call through /cli/agent/identity (the route Apps use
# with agent:identity). The daemon checks everything: name rules,
# #rgb/#rrggbb, and the picture (PNG/JPEG/WebP only, shrunk to 256 px and
# re-encoded; SVG refused). `rgb(r,g,b)` / `r,g,b` become hex here, as for
# `k2 agent hire --color`.
#
# An `id` tool (agent-identity): inside a K2 agent session it sends the
# session passport, never the owner token. The daemon then pins the call
# to the agent's own workspace and allows color and picture only (a name
# is refused: agents can't rename themselves). Owners and Connect logins
# reach any agent and may set the name.
#
# `k2 workspace set-agent-identity` is a thin alias of this function.
cmd_help_agent_identity() {
    cat <<'EOF'
Show or change an agent's picture, color and name.

USAGE
  k2 agent identity [<agent>] [--json]                          show
  k2 agent identity [<agent>] [--color <hex|rgb>]
                    [--image <file.png|jpg|webp> | --clear-image]
                    [--name <name>] [--json]                    change

  <agent> is an agent name, handle or path. Omitted: from an agent's own
  session, the agent itself; from a terminal, the agent in the current
  directory.

FROM AN AGENT'S SESSION
  You may change only your own picture and color (any other agent is
  refused, exit 3). --name is refused: agents can't rename themselves;
  ask your owner. Writes are rate limited per agent (429: try later).

FLAGS
  --color <hex|rgb>       #rrggbb, #rgb, rgb(r,g,b) or r,g,b
  --image <file>          PNG, JPEG or WebP, at most 2 MiB; the daemon
                          checks the bytes, shrinks it to 256 px and
                          re-encodes it. SVG is refused.
  --clear-image           Remove the picture
  --name <name>           Display name (owners only; ≤ 64 chars, no / or :)
  --json                  Print the daemon's JSON

EXIT CODES
  0 ok · 1 error · 2 bad usage · 3 refused · 4 not found

EXAMPLES
  k2 agent identity                                  # who do I look like?
  k2 agent identity --color '#22c55e' --image me.png # my own look
  k2 agent identity scout --name "Scout" --clear-image   (owner)
EOF
}

cmd_agent_identity() {
    local usage="Usage: k2 agent identity [<agent>] [--color <hex|rgb>] [--image <file.png|jpg|webp> | --clear-image] [--name <name>] [--json]"
    local target="" name="" has_name="" color="" image="" clear_image="" as_json=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name)        [[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }; name="$2"; has_name=1; shift 2 ;;
            --color)       [[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }; color="$2"; shift 2 ;;
            --image)       [[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }; image="$2"; shift 2 ;;
            --clear-image) clear_image=1; shift ;;
            --json)        as_json=1; shift ;;
            -h|--help)     cmd_help_agent_identity; return 0 ;;
            -*)            echo "Unknown flag: $1" >&2; echo "$usage" >&2; exit 2 ;;
            *)
                if [[ -z "$target" ]]; then target="$1"; else echo "$usage" >&2; exit 2; fi
                shift ;;
        esac
    done
    if [[ -n "$image" && -n "$clear_image" ]]; then
        echo "Use --image or --clear-image, not both." >&2; exit 2
    fi
    if [[ -n "$image" && ! -f "$image" ]]; then
        echo "Error: no such file: $image" >&2; exit 2
    fi
    # A path (anything with "/") is sent absolute. No agent named: from an
    # agent session the daemon uses the passport's own workspace; from a
    # terminal, the workspace this CLI resolved from the current directory.
    case "$target" in
        */*) target="$(cli_abs_path "$target")" ;;
    esac
    if [[ -z "$target" ]] && ! _cli_caller_is_agent_passport; then
        target="$PROJECT"
    fi
    local method="GET" payload=""
    if [[ -n "$has_name$color$image$clear_image" ]]; then
        method="POST"
        payload="$(python3 -c 'import base64,json,re,sys
target, has_name, name, color, image, clear = sys.argv[1:7]
def rgb_to_hex(s):
    m = re.fullmatch(r"rgba?\(\s*([0-9]{1,3})\s*[, ]\s*([0-9]{1,3})\s*[, ]\s*([0-9]{1,3})(?:\s*[,/]\s*[0-9.]+\s*)?\)", s.strip(), re.I) \
        or re.fullmatch(r"([0-9]{1,3})\s*,\s*([0-9]{1,3})\s*,\s*([0-9]{1,3})", s.strip())
    if not m or any(int(g) > 255 for g in m.groups()):
        return s  # the daemon checks hex and refuses the rest
    return "#" + "".join("%02x" % int(g) for g in m.groups())
body = {}
if target: body["workspace"] = target
if has_name: body["displayName"] = name
if color: body["color"] = rgb_to_hex(color)
if clear: body["image"] = None
elif image:
    raw = open(image, "rb").read()
    if len(raw) > 2 * 1024 * 1024:
        print("Error: the picture must be at most 2 MiB", file=sys.stderr); sys.exit(2)
    if raw.startswith(b"\x89PNG\r\n\x1a\n"): mime = "image/png"
    elif raw.startswith(b"\xff\xd8\xff"): mime = "image/jpeg"
    elif raw[:4] == b"RIFF" and raw[8:12] == b"WEBP": mime = "image/webp"
    else:
        print("Error: the picture must be PNG, JPEG or WebP", file=sys.stderr); sys.exit(2)
    body["image"] = "data:%s;base64,%s" % (mime, base64.b64encode(raw).decode())
print(json.dumps(body))' "$target" "$has_name" "$name" "$color" "$image" "$clear_image")" || exit $?
    elif [[ -n "$target" ]]; then
        payload="workspace=$(urlencode "$target")"
    fi
    local out
    out="$(cli_send_json "$method" "/cli/agent/identity" "$payload")"
    OUT="$out" AS_JSON="$as_json" METHOD="$method" python3 -c 'import json,os,sys
raw = os.environ["OUT"]
body, _, status = raw.rpartition("\n")
status = status.strip()
if status == "000" or not body.strip():
    print("Error: no response from the K2 daemon (is it running?)", file=sys.stderr); sys.exit(1)
try:
    d = json.loads(body)
except Exception:
    print("Error: unexpected daemon response (HTTP %s): %s" % (status, body[:200]), file=sys.stderr); sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err is not None or not status.startswith("2"):
    msg = d.get("message") or err or ("HTTP " + status)
    if err == "rate_limited":
        msg = "too many changes; try again in %ss" % d.get("retryAfterSecs", "?")
    print("Error: %s" % msg, file=sys.stderr)
    sys.exit({"403": 3, "404": 4, "401": 3}.get(status, 1))
if os.environ.get("AS_JSON"):
    print(json.dumps(d)); sys.exit(0)
if os.environ["METHOD"] == "POST":
    changed = d.get("changed") or []
    print("Agent %s: %s" % (d.get("handle", ""), ", ".join(changed) + " updated" if changed else "no change"))
else:
    print("Agent %s" % d.get("handle", ""))
print("  name:    %s" % d.get("displayName", ""))
print("  color:   %s" % (d.get("color") or "-"))
print("  picture: %s" % ("set" if d.get("image") else "none"))
if os.environ["METHOD"] != "POST":
    can = []
    if d.get("canEditIdentity"): can.append("color, picture" if not d.get("canEditHandle") else "name, color, picture")
    if d.get("canEditHandle"): can.append("handle")
    print("  you can change: %s" % (", ".join(can) or "nothing"))'
}

# Leftover (0.45.5) spelling: one implementation, `k2 agent identity`.
cmd_workspace_set_agent_identity() {
    echo "note: 'k2 workspace set-agent-identity' is now 'k2 agent identity' (same flags)." >&2
    cmd_agent_identity "$@"
}

# D11: change the federated address. Requires an explicit confirm flag.
cmd_workspace_set_handle() {
    local confirm=""
    local args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --i-know-this-breaks-federation) confirm=1; shift ;;
            *) args+=("$1"); shift ;;
        esac
    done
    local target="${args[0]:-}"
    local new_handle="${args[1]:-}"
    if [[ -z "$target" || -z "$new_handle" ]]; then
        echo "Usage: k2 workspace set-handle <workspace> <handle> --i-know-this-breaks-federation" >&2
        exit 1
    fi
    if [[ -z "$confirm" ]]; then
        echo "Changing the handle changes this agent's address (old::host → new::host)." >&2
        echo "Existing federated connections will break until the other side reconnects or updates the handle." >&2
        echo "The old handle stays with this workspace (never given to another one)." >&2
        echo "Re-run with --i-know-this-breaks-federation to confirm." >&2
        exit 1
    fi
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    if [ -z "$resolved_path" ]; then
        resolved_path="$target"
    fi
    local body
    body=$(python3 -c 'import json,sys; print(json.dumps({"project":sys.argv[1],"handle":sys.argv[2]}))' \
        "$resolved_path" "$new_handle")
    cli_post_json "/cli/workspace/set-handle" "$body" \
        | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if "error" in d:
        err = d.get("error")
        if isinstance(err, dict):
            print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
        else:
            print("Error: %s" % err, file=sys.stderr)
        sys.exit(1)
    print("Set handle to: %s" % d.get("handle", ""))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
}

# Phase 0b (prd-wiki-public-chat-api-loopback-v1) — per-workspace owner API
# guest policy. Re-injected by the daemon on every /v1 host-session spawn and
# message-live so external callers are framed as non-owners. Callers cannot
# set/override this from the request body.
#
#   k2 workspace api-guest-policy get <ws>
#   k2 workspace api-guest-policy set <ws> <text…>
#   k2 workspace api-guest-policy set <ws> --file <path>
#   k2 workspace api-guest-policy set <ws> --default   # clear → platform default
#
# Storage: projects.api_guest_policy via POST /cli/workspace/set.
# Empty/NULL → platform default (settings::DEFAULT_API_GUEST_POLICY).
cmd_workspace_api_guest_policy() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        get)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace api-guest-policy get <workspace>" >&2
                exit 2
            }
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % err, file=sys.stderr)
    sys.exit(1)
policy = d.get("apiGuestPolicy")
if policy is None:
    print("Error: settings response missing apiGuestPolicy", file=sys.stderr)
    sys.exit(1)
sys.stdout.write(policy if policy.endswith("\n") else policy + "\n")
'
            ;;
        set)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace api-guest-policy set <workspace> <text|--file PATH|--default>" >&2
                exit 2
            }
            shift
            local text=""
            if [[ "${1:-}" == "--default" || "${1:-}" == "--clear" ]]; then
                text=""
            elif [[ "${1:-}" == "--file" ]]; then
                local fpath="${2:-}"
                [[ -z "$fpath" ]] && {
                    echo "Usage: k2 workspace api-guest-policy set <workspace> --file <path>" >&2
                    exit 2
                }
                if [[ "$fpath" == "-" ]]; then
                    text=$(cat)
                else
                    [[ -f "$fpath" ]] || {
                        echo "Error: file not found: $fpath" >&2
                        exit 1
                    }
                    text=$(cat "$fpath")
                fi
            elif [[ $# -eq 0 ]]; then
                echo "Usage: k2 workspace api-guest-policy set <workspace> <text|--file PATH|--default>" >&2
                echo "  (or pipe text: k2 workspace api-guest-policy set <ws> --file -)" >&2
                exit 2
            else
                # Remaining args joined as the policy text.
                text="$*"
            fi
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            local body
            body=$(K2_AGP_PATH="$resolved_path" K2_AGP_TEXT="$text" python3 -c 'import json,os
print(json.dumps({
    "project": os.environ["K2_AGP_PATH"],
    "fields": {"api_guest_policy": os.environ.get("K2_AGP_TEXT", "")},
}))')
            cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
val = ""
for a in d.get("actions") or []:
    if a.get("field") == "api_guest_policy":
        val = a.get("value") or ""
        st = a.get("status")
        if st == "skipped":
            print("API guest policy already set (unchanged).")
            sys.exit(0)
if not val.strip():
    print("API guest policy cleared (platform default will be used on inject).")
else:
    print("API guest policy set (%d chars)." % len(val))
'
            ;;
        ""|-h|--help|help)
            cat <<'EOF' >&2
Usage: k2 workspace api-guest-policy get|set <workspace> [text|--file PATH|--default]

Per-workspace owner policy re-injected on every /v1 host-session turn so
external API clients are framed as non-owners. Callers cannot set this.

  get <ws>                      Print effective policy (platform default if unset)
  set <ws> <text…>              Set custom policy text
  set <ws> --file <path>        Set policy from a file (use - for stdin)
  set <ws> --default            Clear custom text → platform default

Also writable via: k2 agent set / POST /cli/workspace/set field api_guest_policy
EOF
            [[ -z "$sub" || "$sub" == "help" || "$sub" == "-h" || "$sub" == "--help" ]] && exit 0
            exit 2
            ;;
        *)
            echo "Unknown api-guest-policy subcommand: $sub" >&2
            echo "Usage: k2 workspace api-guest-policy get|set <workspace> …" >&2
            exit 2
            ;;
    esac
}

# Host-sessions F1 / prd-api-skip-permissions-default-on-v1 —
# per-workspace flag: whether /v1 host-session spawns keep auto-approve flags.
# Product default ON; explicit off = fail-closed strip.
#
#   k2 workspace api-skip-permissions get <ws>
#   k2 workspace api-skip-permissions set <ws> on|off|1|0
#
# Storage: projects.api_skip_permissions via POST /cli/workspace/set.
cmd_workspace_api_skip_permissions() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        get)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace api-skip-permissions get <workspace>" >&2
                exit 2
            }
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            # Read via settings surface when present; fall back to workspace/set
            # round-trip is write-only — use projects list / a small probe through
            # daemon: POST set is not needed for get. Prefer settings GET if it
            # exposes the field; else use a lightweight SQLite-free path: the
            # daemon's workspace set allowlist is write-only, so we query
            # /cli/settings?project= which mirrors project columns used by UI.
            cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % err, file=sys.stderr)
    sys.exit(1)
# Accept camelCase or snake_case; default-ON when key absent (product default).
if "apiSkipPermissions" in d:
    v = d.get("apiSkipPermissions")
elif "api_skip_permissions" in d:
    v = d.get("api_skip_permissions")
else:
    # Settings may not surface the column — report default ON honestly.
    print("on  (default; settings response did not include api_skip_permissions)")
    sys.exit(0)
# Normalize bool/int/str
if v is True or v == 1 or v == "1" or (isinstance(v, str) and v.lower() in ("on", "true", "yes")):
    print("on")
elif v is False or v == 0 or v == "0" or (isinstance(v, str) and v.lower() in ("off", "false", "no")):
    print("off")
elif v is None:
    print("on  (default)")
else:
    print("on" if v else "off")
'
            ;;
        set)
            local target="${1:-}"
            local mode="${2:-}"
            [[ -z "$target" || -z "$mode" ]] && {
                echo "Usage: k2 workspace api-skip-permissions set <workspace> on|off|1|0" >&2
                exit 2
            }
            local val=""
            case "$(printf '%s' "$mode" | tr '[:upper:]' '[:lower:]')" in
                on|1|true|yes)  val="1" ;;
                off|0|false|no) val="0" ;;
                *)
                    echo "Error: expected on|off|1|0, got: $mode" >&2
                    exit 2
                    ;;
            esac
            # 0.44.4: an agent must not switch its own dangerous auto-approve
            # flags on (owner token in-cell). Switching off is fine.
            if [ "$val" = "1" ] && _cli_caller_is_agent_passport; then
                _cli_refuse_agent_self_grant "api_skip_permissions" \
                    "\`k2 workspace api-skip-permissions set <workspace> on\` from their own terminal"
            fi
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            local body
            body=$(python3 -c 'import json,sys
print(json.dumps({
    "project": sys.argv[1],
    "fields": {"api_skip_permissions": sys.argv[2]},
}))' "$resolved_path" "$val")
            cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
val = None
for a in d.get("actions") or []:
    if a.get("field") == "api_skip_permissions":
        val = a.get("value")
        if a.get("status") == "skipped":
            state = "on" if str(val) in ("1", "true", "True") else "off"
            print("api_skip_permissions already %s (unchanged)." % state)
            sys.exit(0)
if val is None:
    val = ""
state = "on" if str(val) in ("1", "true", "True") else "off"
print("api_skip_permissions set to %s." % state)
'
            ;;
        ""|-h|--help|help)
            cat <<'EOF' >&2
Usage: k2 workspace api-skip-permissions get|set <workspace> [on|off]

Whether /v1 host-session spawns keep agent auto-approve flags
(--dangerously-skip-permissions, etc.). Product default is ON (headless
/v1 cannot answer a HITL gate). Opt out for fail-closed stripping.

  get <ws>              Print on|off (default on when unset)
  set <ws> on|1         Keep auto-approve flags on API host-session spawns
  set <ws> off|0        Strip auto-approve flags (pre-0.40.x fail-closed)

Also: POST /cli/workspace/set {"project":"…","fields":{"api_skip_permissions":"0"|"1"}}
EOF
            [[ -z "$sub" || "$sub" == "help" || "$sub" == "-h" || "$sub" == "--help" ]] && exit 0
            exit 2
            ;;
        *)
            echo "Unknown api-skip-permissions subcommand: $sub" >&2
            echo "Usage: k2 workspace api-skip-permissions get|set <workspace> …" >&2
            exit 2
            ;;
    esac
}

# Workspace default model (prd-workspace-default-model-and-api-model-override-v1).
#
#   k2 workspace default-model get <ws>
#   k2 workspace default-model set <ws> <model|clear>
#
# Storage: projects.default_model via POST /cli/workspace/set.
cmd_workspace_default_model() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        get)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace default-model get <workspace>" >&2
                exit 2
            }
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % err, file=sys.stderr)
    sys.exit(1)
v = d.get("defaultModel")
if v is None:
    v = d.get("default_model")
if v is None or (isinstance(v, str) and not v.strip()):
    print("(unset)")
else:
    print(v)
'
            ;;
        set)
            local target="${1:-}"
            local model="${2:-}"
            [[ -z "$target" || -z "$model" ]] && {
                echo "Usage: k2 workspace default-model set <workspace> <model|clear>" >&2
                exit 2
            }
            local val="$model"
            case "$(printf '%s' "$model" | tr '[:upper:]' '[:lower:]')" in
                clear|unset|none|"") val="" ;;
            esac
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            local body
            body=$(python3 -c 'import json,sys
print(json.dumps({
    "project": sys.argv[1],
    "fields": {"default_model": sys.argv[2]},
}))' "$resolved_path" "$val")
            cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
val = None
for a in d.get("actions") or []:
    if a.get("field") == "default_model":
        val = a.get("value")
        if a.get("status") == "skipped":
            print("default_model already %s (unchanged)." % (val or "(unset)"))
            sys.exit(0)
if not val:
    print("default_model cleared.")
else:
    print("default_model set to %s." % val)
'
            ;;
        ""|-h|--help|help)
            cat <<'EOF' >&2
Usage: k2 workspace default-model get|set <workspace> [model|clear]

Per-workspace default LLM model spliced onto new agent sessions
(claude --model, codex -m, …). Empty/clear = today's preset/harness.

  get <ws>              Print the stored model, or (unset)
  set <ws> opus         Store an opaque model id
  set <ws> clear        Clear back to unset

API POST /v1/w/<ws>/host-sessions {"model":"…"} overrides this per call.
EOF
            [[ -z "$sub" || "$sub" == "help" || "$sub" == "-h" || "$sub" == "--help" ]] && exit 0
            exit 2
            ;;
        *)
            echo "Unknown default-model subcommand: $sub" >&2
            echo "Usage: k2 workspace default-model get|set <workspace> …" >&2
            exit 2
            ;;
    esac
}

# Force workspace default model on dead resume (new PTY). Default off.
#
#   k2 workspace force-model-on-resume get <ws>
#   k2 workspace force-model-on-resume set <ws> on|off|1|0
cmd_workspace_force_model_on_resume() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        get)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace force-model-on-resume get <workspace>" >&2
                exit 2
            }
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % err, file=sys.stderr)
    sys.exit(1)
if "forceModelOnResume" in d:
    v = d.get("forceModelOnResume")
elif "force_model_on_resume" in d:
    v = d.get("force_model_on_resume")
else:
    print("off  (default)")
    sys.exit(0)
if v is True or v == 1 or v == "1" or (isinstance(v, str) and v.lower() in ("on", "true", "yes")):
    print("on")
elif v is False or v == 0 or v == "0" or (isinstance(v, str) and v.lower() in ("off", "false", "no")):
    print("off")
else:
    print("on" if v else "off")
'
            ;;
        set)
            local target="${1:-}"
            local mode="${2:-}"
            [[ -z "$target" || -z "$mode" ]] && {
                echo "Usage: k2 workspace force-model-on-resume set <workspace> on|off|1|0" >&2
                exit 2
            }
            local val=""
            case "$(printf '%s' "$mode" | tr '[:upper:]' '[:lower:]')" in
                on|1|true|yes)  val="1" ;;
                off|0|false|no) val="0" ;;
                *)
                    echo "Error: expected on|off|1|0, got: $mode" >&2
                    exit 2
                    ;;
            esac
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            local body
            body=$(python3 -c 'import json,sys
print(json.dumps({
    "project": sys.argv[1],
    "fields": {"force_model_on_resume": sys.argv[2]},
}))' "$resolved_path" "$val")
            cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
val = None
for a in d.get("actions") or []:
    if a.get("field") == "force_model_on_resume":
        val = a.get("value")
        if a.get("status") == "skipped":
            state = "on" if str(val) in ("1", "true", "True") else "off"
            print("force_model_on_resume already %s (unchanged)." % state)
            sys.exit(0)
if val is None:
    val = ""
state = "on" if str(val) in ("1", "true", "True") else "off"
print("force_model_on_resume set to %s." % state)
'
            ;;
        ""|-h|--help|help)
            cat <<'EOF' >&2
Usage: k2 workspace force-model-on-resume get|set <workspace> [on|off]

When on, K2-direct dead resume (new PTY) also passes the workspace
default model. Off (default) keeps the transcript's model. API `model`
on a host-session still applies on resume regardless.

  get <ws>              Print on|off (default off)
  set <ws> on|1         Splice default model on resume
  set <ws> off|0        Leave resume argv unpinned
EOF
            [[ -z "$sub" || "$sub" == "help" || "$sub" == "-h" || "$sub" == "--help" ]] && exit 0
            exit 2
            ;;
        *)
            echo "Unknown force-model-on-resume subcommand: $sub" >&2
            echo "Usage: k2 workspace force-model-on-resume get|set <workspace> …" >&2
            exit 2
            ;;
    esac
}

# Per-workspace concurrent host-session (live cell) cap.
# Default 15 (env K2_SANDBOX_WORKSPACE_CELL_CAP); max 512 (daemon ceiling).
#
#   k2 workspace api-host-session-cap get <ws>     (preferred; api-* family)
#   k2 workspace host-session-cell-cap get <ws>  (alias)
#   k2 workspace api-host-session-cap set <ws> <n|default>
#
# Storage: projects.host_session_cell_cap via POST /cli/workspace/set.
cmd_workspace_host_session_cell_cap() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        get)
            local target="${1:-}"
            [[ -z "$target" ]] && {
                echo "Usage: k2 workspace host-session-cell-cap get <workspace>" >&2
                exit 2
            }
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            cli_request "/cli/settings" "project=$(urlencode "$resolved_path")" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if isinstance(d, dict) and d.get("error"):
    err = d["error"]
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % err, file=sys.stderr)
    sys.exit(1)
if "hostSessionCellCap" in d:
    v = d.get("hostSessionCellCap")
elif "host_session_cell_cap" in d:
    v = d.get("host_session_cell_cap")
else:
    print("15 (default; settings response did not include host_session_cell_cap)")
    sys.exit(0)
if v is None:
    # Inherit daemon default. Settings surface null for inherit; effective
    # value is env K2_SANDBOX_WORKSPACE_CELL_CAP or 15.
    print("15 (default)")
elif isinstance(v, (int, float)) and not isinstance(v, bool):
    print(str(int(v)))
elif isinstance(v, str) and v.strip().isdigit():
    print(v.strip())
else:
    print("15 (default)")
'
            ;;
        set)
            local target="${1:-}"
            local mode="${2:-}"
            [[ -z "$target" || -z "$mode" ]] && {
                echo "Usage: k2 workspace host-session-cell-cap set <workspace> <n|default>" >&2
                exit 2
            }
            local val=""
            local mode_lc
            mode_lc=$(printf '%s' "$mode" | tr '[:upper:]' '[:lower:]')
            case "$mode_lc" in
                default|null|"")
                    val="default"
                    ;;
                *)
                    if ! printf '%s' "$mode" | grep -Eq '^[1-9][0-9]*$'; then
                        echo "Error: expected positive integer 1..512 or 'default', got: $mode" >&2
                        exit 2
                    fi
                    if [ "$mode" -gt 512 ]; then
                        echo "Error: host_session_cell_cap max is 512 (daemon ceiling), got: $mode" >&2
                        exit 2
                    fi
                    val="$mode"
                    ;;
            esac
            local resolved_path
            resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
                | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
            if [ -z "$resolved_path" ]; then
                resolved_path="$target"
            fi
            local body
            body=$(python3 -c 'import json,sys
print(json.dumps({
    "project": sys.argv[1],
    "fields": {"host_session_cell_cap": sys.argv[2]},
}))' "$resolved_path" "$val")
            cli_post_json "/cli/workspace/set" "$body" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print("Error: daemon returned an unparseable response: %r" % raw[:200], file=sys.stderr)
    sys.exit(1)
if d.get("error") or d.get("ok") is False:
    err = d.get("error")
    if isinstance(err, dict):
        print("Error: %s" % (err.get("hint") or err.get("code") or err), file=sys.stderr)
    else:
        print("Error: %s" % (err or raw[:200]), file=sys.stderr)
    sys.exit(1)
val = None
status = None
for a in d.get("actions") or []:
    if a.get("field") == "host_session_cell_cap":
        val = a.get("value")
        status = a.get("status")
        break
if status == "skipped":
    if val in (None, "", "default", "null") or (isinstance(val, str) and val.lower() in ("default", "null")):
        print("host_session_cell_cap already default (inherit; unchanged).")
    else:
        print("host_session_cell_cap already %s (unchanged)." % val)
    sys.exit(0)
if val is None:
    print("host_session_cell_cap updated.")
    sys.exit(0)
if str(val).lower() in ("default", "null", ""):
    print("host_session_cell_cap cleared → inherit daemon default (15 or env).")
else:
    print("host_session_cell_cap set to %s." % val)
'
            ;;
        ""|-h|--help|help)
            cat <<'EOF' >&2
Usage: k2 workspace api-host-session-cap get|set <workspace> [n|default]
       (alias: host-session-cell-cap)

Per-workspace concurrent host-session (live cell) cap. Default is 15
(overridable via env K2_SANDBOX_WORKSPACE_CELL_CAP). Max 512 (daemon
ceiling so agents cannot set unbounded). Raising the cap is runway —
agents still need `k2 done` to avoid accretion.

  get <ws>              Print N, or "15 (default)" when inheriting
  set <ws> <n>          Set concurrent live host-session cap (1..512)
  set <ws> default      Clear override → inherit daemon default

Also: POST /cli/workspace/set {"project":"…","fields":{"host_session_cell_cap":"64"|"default"}}
EOF
            [[ -z "$sub" || "$sub" == "help" || "$sub" == "-h" || "$sub" == "--help" ]] && exit 0
            exit 2
            ;;
        *)
            echo "Unknown host-session-cell-cap subcommand: $sub" >&2
            echo "Usage: k2 workspace host-session-cell-cap get|set <workspace> …" >&2
            exit 2
            ;;
    esac
}

# B3a (sandbox) — set/clear the PER-WORKSPACE Anthropic API key (BYO key).
# Owner-only daemon route (`/cli/workspace/api-key`, POST). The key rides the
# JSON BODY (never the URL/query) so it isn't logged by intermediaries. An
# empty key CLEARS the stored value. The key is staged into a microVM-backed
# sandbox cell's guest env at spawn (per-workspace scoping). NEVER printed back.
#   k2 workspace api-key <workspace> --key-stdin   key from stdin (hidden prompt
#                                                  on a terminal, else one line)
#   k2 workspace api-key <workspace>               clear the key
#   k2 workspace api-key <workspace> <key>         deprecated (key on argv)
# The key goes bash → python (stdin) → curl (stdin): never on any argv.
_workspace_api_key_usage() {
    echo "Usage: k2 workspace api-key <workspace> --key-stdin   (key read from stdin; never an argument)" >&2
    echo "       k2 workspace api-key <workspace>               (no key: clears the stored key)" >&2
}

cmd_workspace_set_api_key() {
    local from_stdin=0 args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --key-stdin) from_stdin=1; shift ;;
            --help|-h) _workspace_api_key_usage; exit 0 ;;
            --) shift; while [[ $# -gt 0 ]]; do args+=("$1"); shift; done ;;
            *) args+=("$1"); shift ;;
        esac
    done
    local target="${args[0]:-}"
    local key="${args[1]:-}"
    if [[ -z "$target" ]] || [[ ${#args[@]} -gt 2 ]]; then
        _workspace_api_key_usage
        exit 1
    fi
    if [ "$from_stdin" = "1" ]; then
        if [[ ${#args[@]} -gt 1 ]]; then
            echo "Error: give the key on stdin (--key-stdin) or not at all, not both." >&2
            exit 2
        fi
        _cli_read_secret_stdin "Paste the API key: "
        key="$_K2_SECRET"
        _K2_SECRET=""
        if [ -z "$key" ]; then
            echo "Error: no key on stdin (to clear the key, run without --key-stdin and without a key)." >&2
            exit 2
        fi
    elif [ -n "$key" ]; then
        _cli_warn_key_on_argv
    fi
    local resolved_path
    resolved_path=$(cli_request "/cli/workspace/resolve" "q=$(urlencode "$target")" 2>/dev/null \
        | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("path",""))
except Exception:
    pass' 2>/dev/null)
    if [ -z "$resolved_path" ]; then
        resolved_path="$target"
    fi
    # Build the JSON body with python3 so the key is escaped correctly and is
    # never interpolated into a shell/URL context. The key reaches python on
    # stdin (printf is a builtin: no argv) and the body reaches curl on stdin.
    printf '%s' "$key" | K2_AK_PATH="$resolved_path" python3 -c 'import json,os,sys
print(json.dumps({"project": os.environ["K2_AK_PATH"], "key": sys.stdin.read().strip()}))' \
        | cli_post_json_stdin "/cli/workspace/api-key" \
        | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("Workspace API key %s." % ("set" if d.get("keySet") else "cleared"))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
}

# 0.40.43 (1c) — owner/admin switch for the public /v1 API surface.
#   k2 api on|off   → POST /cli/settings/update {"apiEnabled":true|false}
#   k2 api status   → effective gate (env OR setting) + the persisted setting
# The daemon evaluates the gate PER REQUEST (misc_routes::api_enabled() =
# K2_API env OR legacy K2_SANDBOX_API OR this setting), so on/off take
# effect IMMEDIATELY — no daemon restart, no confirm+reboot. The env flags
# remain a force-on override for headless boxes (a hosted box's systemd drop-in);
# where one is set, `k2 api off` clears only the setting and `status`
# explains why the surface is still up. The daemon 403s non-owner/admin
# writers of apiEnabled server-side.
cmd_api() {
    local sub="${1:-status}"
    case "$sub" in
        on|off)
            local val="true"
            [[ "$sub" == "off" ]] && val="false"
            cli_post_json "/cli/settings/update" "{\"apiEnabled\":${val}}" | python3 -c "
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception:
    print('Error: daemon returned an unparseable response: %r' % raw[:200])
    sys.exit(1)
if 'error' in d:
    print('Error: %s' % d['error'])
    sys.exit(1)
on = bool(d.get('apiEnabled'))
print('Public /v1 API setting: %s (live now — no restart needed)' % ('ON' if on else 'OFF'))
if not on:
    print('Note: where the K2_API (or legacy K2_SANDBOX_API) env flag is set, it still forces the surface on; check with: k2 api status')
"
            ;;
        status)
            # Effective state comes from the daemon itself: /boot-status is
            # the unauthenticated handshake carrying the api capability
            # object (env OR setting, evaluated per request); the persisted
            # switch rides the authenticated settings snapshot.
            # `api` rides the FULL body only (PRD boot-status-ghost): send
            # the token (config on stdin, never argv) so a remote host
            # (K2_HOST) answers it too; loopback is full anyway.
            local boot settings _K2_CFG=""
            _k2_cfg_add url "${BASE_URL}/boot-status"
            if [ -n "$TOKEN" ]; then
                _k2_cfg_add header "Authorization: Bearer ${TOKEN}"
            fi
            boot="$(_k2_curl "$_K2_CFG" -s --connect-timeout 5 --max-time 10 2>/dev/null || true)"
            settings="$(cli_request "/cli/settings/get" || true)"
            K2_API_BOOT="$boot" K2_API_SETTINGS="$settings" python3 <<'PY'
import json, os
def parse(name):
    try:
        return json.loads(os.environ.get(name, '') or '{}')
    except Exception:
        return {}
boot = parse('K2_API_BOOT')
settings = parse('K2_API_SETTINGS')
eff = boot.get('api', {}).get('enabled')
setting = settings.get('apiEnabled')
fmt = lambda v: '?' if v is None else ('on' if v else 'off')
print('Public /v1 API surface: %s' % fmt(eff))
print('  Settings switch:      %s' % fmt(setting))
if eff and setting is False:
    print('  (forced on by the K2_API or legacy K2_SANDBOX_API env flag)')
PY
            ;;
        *)
            echo "Usage: k2 api <on|off|status>" >&2
            echo "  on/off  — flip the public /v1 API surface (owner/admin; live immediately, no restart)" >&2
            echo "  status  — effective gate (env OR setting) plus the persisted Settings switch" >&2
            exit 2
            ;;
    esac
}

# P3a (sandbox / K2-as-a-server) — OWNER-only API-key auth-tier management for
# the external /v1/* surface. Subcommands:
#   k2 api-key create [--label L] [--key-stdin] \
#                     [--provider anthropic|openai|google|xai] [--base-url U] \
#                     [--workspaces '*'|ws1,ws2,...] \
#                     [--allow host-sessions,canonical-message,sandboxes,db] \
#                     [--deny  host-sessions,canonical-message,sandboxes,db] \
#                     [--hire [--max-per-day N] [--max-live N] [--max-per-team N] \
#                             [--hire-db off|read|write] [--link] [--link-targets ws1,ws2] \
#                             [--retire] [--reach none|message|host-sessions] [--folder SUB]
#                             [--app-login APP[:ROOMS[:ROLES]]]...]
#   k2 api-key list
#   k2 api-key revoke <id>
# --workspaces is the per-key WORKSPACE GRANT (PRD §G2 #4): '*' = every
# workspace, or a comma-separated list of workspace names. A key minted
# WITHOUT a grant is FAIL-CLOSED — it authorizes ZERO workspaces and every
# /v1/w/<ws>/* call returns the uniform 404 (the daemon route has accepted
# this field since 0.40.21; the flag was simply never exposed here, which
# made every CLI-minted key dead on arrival for /v1 — found on a customer box
# 2026-07-07).
# --allow / --deny (Phase 0, wiki public chat): per-key CAPABILITIES that gate
# which /v1 doors the key may open (orthogonal to the workspace grant):
#   host-sessions      (aliases: host)      → /v1/w/<ws>/host-sessions*
#   canonical-message  (aliases: canonical, message) → POST /v1/w/<ws>/message
#   sandboxes          (alias: sandbox)     → /v1/sandboxes* + /v1/w/<ws>/sessions*
#   db                 (aliases: sql, database) → /v1/w/<ws>/db* (create/migrate/dump)
# --hire (0.45.6, agent hire through the v1 API): the key may hire agents
# (POST /v1/agents/hire, one agent or a team) within the limits you set here;
# what a hired agent gets never exceeds the key. A hire key is hire-only
# unless --allow names other doors. Minting one asks for the owner confirm
# (Touch ID / login password, or the vault password) even with the owner
# token. Only the App access limit can change afterwards (`k2 api-key
# hire-ceiling <id>`, or Settings → API Keys; widening asks for the same
# confirm, lowering doesn't). For other limits, mint a new key and revoke
# the old one.
# Defaults for NEW keys: host-sessions ON, canonical-message OFF, sandboxes OFF
# (the wiki-chat recipe). --allow REPLACES the default set with only the listed
# doors; --deny turns listed doors OFF on top of the result.
# Wiki public-chat mint recipe:
#   k2 api on
#   k2 api-key create --label wiki-public-chat \
#     --workspaces '<wiki-ws-slug>' \
#     --allow host-sessions
# --key-stdin reads the LLM credential from stdin (hidden prompt on a
# terminal, else one line from a pipe); the key never rides any argv. The
# old --anthropic-key K / --llm-key K forms still work this release but warn
# (deprecated; refused in the next release).
# The RAW key is printed exactly ONCE on create ("store it now, not shown
# again"); it is never re-printed. The associated LLM credential (BYO cred;
# --llm-key is the provider-neutral alias for --anthropic-key) is never echoed
# back. W5: --provider decides WHICH env var(s) the credential stages into
# /v1-spawned sessions (default anthropic → ANTHROPIC_API_KEY; openai →
# OPENAI_API_KEY [+ OPENAI_BASE_URL from --base-url]; google/gemini →
# GEMINI_API_KEY+GOOGLE_API_KEY; xai/grok → XAI_API_KEY). All routes are
# owner-token gated at the daemon.
cmd_api_key() {
    local sub="${1:-}"
    shift || true
    case "$sub" in
        create)
            local label="" anthropic="" provider="" base_url="" workspaces="" allow="" deny=""
            local key_stdin=0 key_on_argv=0
            local hire=0 h_day="" h_live="" h_team="" h_db="" h_link=0 h_targets="" h_retire=0 h_reach="" h_folder="" h_apps=""
            local status_key=0 other_flags=0
            while [[ $# -gt 0 ]]; do
                [ "$1" != "--label" ] && [ "$1" != "--status" ] && other_flags=1
                case "$1" in
                    --label)        label="${2:-}"; shift 2 ;;
                    # PRD boot-status-ghost §5.1: a read-only STATUS key for a
                    # fleet monitor — full /boot-status only, no doors.
                    --status)       status_key=1; shift ;;
                    --hire)         hire=1; shift ;;
                    --max-per-day)  h_day="${2:-}"; shift 2 ;;
                    --max-live)     h_live="${2:-}"; shift 2 ;;
                    --max-per-team) h_team="${2:-}"; shift 2 ;;
                    --hire-db)      h_db="${2:-}"; shift 2 ;;
                    --link)         h_link=1; shift ;;
                    --link-targets) h_targets="${2:-}"; shift 2 ;;
                    --retire)       h_retire=1; shift ;;
                    --reach)        h_reach="${2:-}"; shift 2 ;;
                    --folder)       h_folder="${2:-}"; shift 2 ;;
                    # S5: Apps hired agents may get a login on. APP is a
                    # published service id or <workspace>/<name>; ROOMS a
                    # comma list (self = the hired agent); ROLES the highest
                    # role(s) — a hired agent may get that role or one no
                    # wider, whose rooms are all in ROOMS.
                    --app-login)    h_apps="${h_apps}${2:-}"$'\n'; shift 2 ;;
                    # The LLM credential is read from stdin (hidden prompt on
                    # a terminal, else one line). Never an argument.
                    --key-stdin)    key_stdin=1; shift ;;
                    # Deprecated: the key on argv (readable via `ps`).
                    --anthropic-key) anthropic="${2:-}"; key_on_argv=1; shift 2 ;;
                    # W5: provider-neutral alias for the same credential slot.
                    --llm-key)      anthropic="${2:-}"; key_on_argv=1; shift 2 ;;
                    --provider)     provider="${2:-}"; shift 2 ;;
                    --base-url)     base_url="${2:-}"; shift 2 ;;
                    --workspaces)   workspaces="${2:-}"; shift 2 ;;
                    --allow)        allow="${2:-}"; shift 2 ;;
                    --deny)         deny="${2:-}"; shift 2 ;;
                    *)              echo "Unknown flag: $1" >&2; exit 1 ;;
                esac
            done
            if [ "$status_key" = "1" ]; then
                if [ "$other_flags" = "1" ]; then
                    echo "Error: a status key is read-only: --status takes --label only." >&2
                    exit 2
                fi
                K2_AK_LABEL="$label" python3 -c 'import json,os; print(json.dumps({"label": os.environ.get("K2_AK_LABEL",""), "scope": "status"}))' \
                    | cli_post_json_stdin "/cli/api-keys/create" \
                    | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    print("Error: no answer from the daemon", file=sys.stderr); sys.exit(1)
if "error" in d:
    print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
print("Status key created (id: %s). It reads this server'"'"'s full /boot-status and nothing else." % d.get("id",""))
print("")
print("  %s" % d.get("key",""))
print("")
print("STORE THIS KEY NOW — it is shown only once. Send it as: Authorization: Bearer <key>")
print("Rotate: mint a new one, switch the monitor, then k2 api-key revoke <old id>.")' || exit 1
                return 0
            fi
            if [ "$key_stdin" = "1" ]; then
                if [ "$key_on_argv" = "1" ]; then
                    echo "Error: give the LLM key with --key-stdin or --llm-key, not both." >&2
                    exit 2
                fi
                _cli_read_secret_stdin "Paste the LLM API key: "
                anthropic="$_K2_SECRET"
                _K2_SECRET=""
                if [ -z "$anthropic" ]; then
                    echo "Error: --key-stdin given but no key on stdin." >&2
                    exit 2
                fi
            elif [ "$key_on_argv" = "1" ] && [ -n "$anthropic" ]; then
                _cli_warn_key_on_argv
            fi
            if [ "$hire" = "0" ] && { [ -n "$h_day$h_live$h_team$h_db$h_targets$h_reach$h_folder$h_apps" ] || [ "$h_link$h_retire" != "00" ]; }; then
                echo "Error: hire limits (--max-*, --hire-db, --link, --retire, --reach, --folder, --app-login) need --hire." >&2
                exit 2
            fi
            # A hire key needs a fresh owner confirm (Touch ID / login
            # password, or the vault password) — the daemon refuses without.
            local hire_confirm=""
            if [ "$hire" = "1" ]; then
                hire_confirm="$(_vault_confirm_token hire-key-mint "" "API key that can hire agents")" || exit 3
            fi
            # Build the JSON body via python3 so the (secret) LLM key is
            # escaped correctly and never interpolated into a shell/URL
            # context. The key reaches python on stdin (printf is a builtin:
            # no argv) and the body reaches curl on stdin.
            printf '%s' "$anthropic" | K2_AK_LABEL="$label" K2_AK_PROVIDER="$provider" K2_AK_BASE_URL="$base_url" K2_AK_WORKSPACES="$workspaces" K2_AK_ALLOW="$allow" K2_AK_DENY="$deny" \
                K2_AK_HIRE="$hire" K2_AK_HCONF="$hire_confirm" K2_AK_HDAY="$h_day" K2_AK_HLIVE="$h_live" K2_AK_HTEAM="$h_team" K2_AK_HDB="$h_db" \
                K2_AK_HLINK="$h_link" K2_AK_HTARGETS="$h_targets" K2_AK_HRETIRE="$h_retire" K2_AK_HREACH="$h_reach" K2_AK_HFOLDER="$h_folder" K2_AK_HAPPS="$h_apps" \
                python3 -c 'import json,os,sys
b={"label": os.environ.get("K2_AK_LABEL","")}
k=sys.stdin.read().strip()
if k: b["anthropicKey"]=k
p=os.environ.get("K2_AK_PROVIDER","")
if p.strip(): b["provider"]=p
u=os.environ.get("K2_AK_BASE_URL","")
if u.strip(): b["baseUrl"]=u
# Workspace grant: "*" passes through as the string; anything else is a
# comma-separated list -> JSON array of trimmed names. Absent -> no grant
# (the daemon stores NULL and the key authorizes nothing on /v1).
w=os.environ.get("K2_AK_WORKSPACES","").strip()
if w == "*": b["workspaces"]="*"
elif w:
    slugs=[s.strip() for s in w.split(",") if s.strip()]
    if slugs: b["workspaces"]=slugs
# Phase 0 capabilities: comma-separated --allow / --deny lists.
a=os.environ.get("K2_AK_ALLOW","").strip()
if a:
    b["allow"]=[s.strip() for s in a.split(",") if s.strip()]
d=os.environ.get("K2_AK_DENY","").strip()
if d:
    b["deny"]=[s.strip() for s in d.split(",") if s.strip()]
if os.environ.get("K2_AK_HIRE") == "1":
    h={}
    for env,key in (("K2_AK_HDAY","maxMembersPerDay"),("K2_AK_HLIVE","maxLiveMembers"),("K2_AK_HTEAM","maxMembersPerTeam")):
        v=os.environ.get(env,"").strip()
        if v:
            try: h[key]=int(v)
            except ValueError:
                print("Error: %s must be a number" % key, file=sys.stderr); sys.exit(2)
    if os.environ.get("K2_AK_HDB","").strip(): h["db"]=os.environ["K2_AK_HDB"].strip()
    if os.environ.get("K2_AK_HLINK")=="1": h["link"]=True
    t=os.environ.get("K2_AK_HTARGETS","").strip()
    if t: h["linkTargets"]=[s.strip() for s in t.split(",") if s.strip()]
    if os.environ.get("K2_AK_HRETIRE")=="1": h["retire"]=True
    if os.environ.get("K2_AK_HREACH","").strip(): h["reach"]=os.environ["K2_AK_HREACH"].strip()
    if os.environ.get("K2_AK_HFOLDER","").strip(): h["folder"]=os.environ["K2_AK_HFOLDER"].strip()
    apps=[]
    for line in os.environ.get("K2_AK_HAPPS","").splitlines():
        line=line.strip()
        if not line: continue
        parts=line.split(":")
        a={"app": parts[0]}
        if len(parts)>1 and parts[1].strip(): a["rooms"]=[r.strip() for r in parts[1].split(",") if r.strip()]
        if len(parts)>2 and parts[2].strip(): a["roles"]=[r.strip() for r in parts[2].split(",") if r.strip()]
        apps.append(a)
    if apps: h["apps"]=apps
    b["hire"]=h
    b["confirmToken"]=os.environ.get("K2_AK_HCONF","")
print(json.dumps(b))' \
                | cli_post_json_stdin "/cli/api-keys/create" \
                | K2_AK_WORKSPACES="$workspaces" python3 -c 'import json,os,sys
try:
    d=json.load(sys.stdin)
    if "error" in d:
        e=d["error"]
        if isinstance(e,dict): e="%s: %s" % (e.get("code",""), e.get("hint",""))
        print("Error: %s" % e, file=sys.stderr); sys.exit(1)
    print("API key created (id: %s)." % d.get("id",""))
    caps=d.get("capabilities") or {}
    if caps:
        on=[n for n,v in (("host-sessions",caps.get("hostSessions")),
                          ("canonical-message",caps.get("canonicalMessage")),
                          ("sandboxes",caps.get("sandboxes")),
                          ("db",caps.get("db")),
                          ("hire",caps.get("hire"))) if v]
        print("Capabilities: %s" % (", ".join(on) if on else "(none)"))
    hl=d.get("hireLimits")
    if hl:
        print("Hire limits: %s/day, %s live, %s per team, db %s, link %s, retire %s, reach %s%s" % (
            hl.get("maxMembersPerDay"), hl.get("maxLiveMembers"), hl.get("maxMembersPerTeam"), hl.get("db"),
            "on" if hl.get("link") else "off", "on" if hl.get("retire") else "off", hl.get("reach"),
            (", folder %s" % hl["folder"]) if hl.get("folder") else ""))
        for a in hl.get("apps") or []:
            print("App logins on %s: rooms %s%s (whole rooms until folder grants exist)" % (
                a.get("app"), ",".join(a.get("rooms") or []) or "-",
                (", roles " + ",".join(a["roles"])) if a.get("roles") else ""))
    print("")
    print("  %s" % d.get("key",""))
    print("")
    print("STORE THIS KEY NOW — it is shown only once and cannot be retrieved again.")
    if not os.environ.get("K2_AK_WORKSPACES","").strip():
        print("")
        print("NOTE: minted WITHOUT --workspaces — this key authorizes ZERO", file=sys.stderr)
        print("workspaces, so every /v1/w/<ws>/* call will 404. Re-mint with", file=sys.stderr)
        print("--workspaces \x27*\x27 (all) or --workspaces ws1,ws2 to grant access.", file=sys.stderr)
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
            ;;
        hire-ceiling|app-access)
            # Change a hire key's App access limit (0.45.6): which App its
            # hired agents may get a login on, which rooms, and the highest
            # role. Replaces the whole limit; --none = no App access.
            # Lowering needs no confirm; widening asks for the owner confirm
            # (the daemon decides and answers vault_confirm_required).
            local id="${1:-}"
            if [ -z "$id" ] || [ "${id#-}" != "$id" ]; then
                echo "Usage: k2 api-key hire-ceiling <id> (--app-login APP[:ROOMS[:ROLE]]... | --none)" >&2
                exit 2
            fi
            shift
            local c_apps="" c_none=0
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --app-login) c_apps="${c_apps}${2:-}"$'\n'; shift 2 ;;
                    --none)      c_none=1; shift ;;
                    *)           echo "Unknown flag: $1" >&2; exit 2 ;;
                esac
            done
            if [ "$c_none" = "1" ] && [ -n "$c_apps" ]; then
                echo "Error: --none and --app-login don't mix." >&2; exit 2
            fi
            if [ "$c_none" = "0" ] && [ -z "$c_apps" ]; then
                echo "Error: name the App access (--app-login APP[:ROOMS[:ROLE]]) or --none for no App access." >&2; exit 2
            fi
            local c_body c_resp c_code c_tok
            c_body="$(K2_AK_ID="$id" K2_AK_HAPPS="$c_apps" python3 -c 'import json,os
apps=[]
for line in os.environ.get("K2_AK_HAPPS","").splitlines():
    line=line.strip()
    if not line: continue
    parts=line.split(":")
    a={"app": parts[0]}
    if len(parts)>1 and parts[1].strip(): a["rooms"]=[r.strip() for r in parts[1].split(",") if r.strip()]
    if len(parts)>2 and parts[2].strip(): a["roles"]=[r.strip() for r in parts[2].split(",") if r.strip()]
    apps.append(a)
print(json.dumps({"id": os.environ["K2_AK_ID"], "apps": apps}))')"
            c_resp="$(printf '%s' "$c_body" | cli_post_json_stdin "/cli/api-keys/hire-ceiling")"
            c_code="$(printf '%s' "$c_resp" | python3 -c 'import json,sys
try:
    e=json.load(sys.stdin).get("error")
    print(e.get("code","") if isinstance(e,dict) else "")
except Exception:
    print("")')"
            if [ "$c_code" = "vault_confirm_required" ]; then
                c_tok="$(_vault_confirm_token hire-key-mint "" "Widen an API key's App access")" || exit 3
                c_body="$(printf '%s' "$c_body" | K2_AK_TOK="$c_tok" python3 -c 'import json,os,sys
b=json.load(sys.stdin); b["confirmToken"]=os.environ["K2_AK_TOK"]; print(json.dumps(b))')"
                c_tok=""
                c_resp="$(printf '%s' "$c_body" | cli_post_json_stdin "/cli/api-keys/hire-ceiling")"
            fi
            printf '%s' "$c_resp" | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    print("Error: no answer from the daemon", file=sys.stderr); sys.exit(1)
e=d.get("error") if isinstance(d,dict) else None
if e:
    if isinstance(e,dict): e="%s: %s" % (e.get("code",""), e.get("hint",""))
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)
apps=(d.get("hireLimits") or {}).get("apps") or []
print("App access %s for key %s." % ("widened" if d.get("change")=="raise" else "updated", d.get("id","")))
if not apps:
    print("No App access: agents hired with this key get no App login.")
for a in apps:
    roles=a.get("roles") or []
    print("  %s: rooms %s, %s" % (a.get("app"), ",".join(a.get("rooms") or []),
        ("up to role " + ",".join(roles)) if roles else "no role (Thread only in those rooms)"))'
            ;;
        list)
            cli_request "/cli/api-keys/list" \
                | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
    if isinstance(d,dict) and "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    keys=d.get("keys",[])
    if not keys:
        print("No API keys."); sys.exit(0)
    for k in keys:
        if k.get("revokedAt"):
            state="revoked"
        elif k.get("disabledAt"):
            state="disabled"
        else:
            state="active"
        prov=k.get("provider") or "anthropic"
        cred=("%s-key" % prov) if k.get("anthropicKeySet") else "no-llm-key"
        line="%s  [%s]  %s  %s  scope=%s" % (k.get("id",""), state, (k.get("label") or "-"), cred, k.get("scope",""))
        if k.get("baseUrl"): line += "  base-url=%s" % k["baseUrl"]
        caps=k.get("capabilities") or {}
        on=[n for n,v in (("host-sessions",caps.get("hostSessions")),
                          ("canonical-message",caps.get("canonicalMessage")),
                          ("sandboxes",caps.get("sandboxes")),
                          ("db",caps.get("db")),
                          ("hire",caps.get("hire"))) if v]
        line += "  caps=%s" % (",".join(on) if on else "none")
        if k.get("legacyNewWorkspace"): line += "  legacy-new-workspace"
        print(line)
        if caps.get("hire") and k.get("hireLimits") is not None:
            apps=(k.get("hireLimits") or {}).get("apps") or []
            if not apps:
                print("    App access: none (hired agents get no App login)")
            for a in apps:
                roles=a.get("roles") or []
                print("    App access: %s rooms=%s %s" % (a.get("app"), ",".join(a.get("rooms") or []),
                    ("up-to-role=" + ",".join(roles)) if roles else "no-role"))
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
            ;;
        revoke)
            local id="${1:-}"
            [[ -z "$id" ]] && { echo "Usage: k2 api-key revoke <id>" >&2; exit 1; }
            local body
            body=$(K2_AK_ID="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_AK_ID"]}))')
            cli_post_json "/cli/api-keys/revoke" "$body" \
                | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("API key revoked." if d.get("success") else "No matching active key (unknown or already revoked).")
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
            ;;
        disable)
            local id="${1:-}"
            [[ -z "$id" ]] && { echo "Usage: k2 api-key disable <id>" >&2; exit 1; }
            local body
            body=$(K2_AK_ID="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_AK_ID"]}))')
            cli_post_json "/cli/api-keys/disable" "$body" \
                | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("API key disabled (soft — re-enable with k2 api-key enable <id>)." if d.get("success") else "No matching key (unknown, revoked, or already disabled).")
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
            ;;
        enable)
            local id="${1:-}"
            [[ -z "$id" ]] && { echo "Usage: k2 api-key enable <id>" >&2; exit 1; }
            local body
            body=$(K2_AK_ID="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_AK_ID"]}))')
            cli_post_json "/cli/api-keys/enable" "$body" \
                | python3 -c 'import json,sys
try:
    d=json.load(sys.stdin)
    if "error" in d:
        print("Error: %s" % d["error"], file=sys.stderr); sys.exit(1)
    print("API key enabled." if d.get("success") else "No matching key (unknown, revoked, or already enabled).")
except Exception as e:
    print("Error: %s" % e, file=sys.stderr); sys.exit(1)'
            ;;
        *)
            echo "Usage: k2 api-key <create|list|hire-ceiling|revoke|disable|enable>" >&2
            echo "" >&2
            echo "  create  [--label L] [--key-stdin]   (LLM key read from stdin; never an argument)" >&2
            echo "          [--provider anthropic|openai|google|xai] [--base-url U]" >&2
            echo "          [--workspaces '*'|ws1,ws2,...]" >&2
            echo "          [--allow host-sessions,canonical-message,sandboxes,db]" >&2
            echo "          [--deny  host-sessions,canonical-message,sandboxes,db]" >&2
            echo "          [--hire [--max-per-day N] [--max-live N] [--max-per-team N] [--hire-db off|read|write]" >&2
            echo "                  [--link] [--link-targets ws1,ws2] [--retire] [--reach none|message|host-sessions]" >&2
            echo "                  [--folder SUB] [--app-login APP[:ROOMS[:ROLES]]]...]   (asks for the owner confirm)" >&2
            echo "  create --status [--label L]   A read-only status key (fleet monitor): reads this" >&2
            echo "                 server's full /boot-status and opens nothing else (not /v1, not /cli)" >&2
            echo "  list" >&2
            echo "  hire-ceiling <id> (--app-login APP[:ROOMS[:ROLE]]... | --none)" >&2
            echo "                 A hire key's App access limit: the App, rooms (self = the hired agent's own)" >&2
            echo "                 and highest role its agents may get. Widening asks for the owner confirm." >&2
            echo "  disable <id>   Soft emergency kill (re-enableable)" >&2
            echo "  enable  <id>   Clear soft-disable" >&2
            echo "  revoke  <id>   Permanent (cannot re-enable same secret)" >&2
            echo "" >&2
            echo "Capabilities (Phase 0) gate which /v1 doors a key may open:" >&2
            echo "  host-sessions      → /v1/w/<ws>/host-sessions*   (default ON for new keys)" >&2
            echo "  canonical-message  → POST /v1/w/<ws>/message     (default OFF)" >&2
            echo "  sandboxes          → /v1/sandboxes* + sessions*  (default OFF)" >&2
            echo "  db                 → /v1/w/<ws>/db*              (default OFF)" >&2
            echo "  hire (--hire)      → /v1/agents/* + /v1/agent-teams/* and new workspaces on POST /v1/w" >&2
            echo "                       (hire-only unless --allow names other doors; defaults 20/day," >&2
            echo "                        100 live, 12 per team, db off, no link/retire/reach)" >&2
            echo "" >&2
            echo "Wiki public-chat recipe:" >&2
            echo "  k2 api-key create --label wiki-public-chat \\" >&2
            echo "    --workspaces '<wiki-ws-slug>' --allow host-sessions" >&2
            exit 1 ;;
    esac
}

# Skin Access (slice 1+2) — guest list + k2skn_ capability passes + front-door
# stub. Separate from Connect users (`k2 users`) and `/v1` `k2sk_` API keys.
# Owner-tier daemon mutations. GET list admits a workspace-agent scoped hook.
# Secret shown once on skin-token create.
#
# stdin: raw JSON body. Teaching mapper (mirrors _hb_print_err / _db_py.die):
# string "Invalid or missing auth token" → stderr + exit 3;
# {code,hint} owner_only/forbidden/gated → 3; other errors → 1.
# Success (no error) exits 0 silently. Callers capture the body, run this,
# then format — never rely on pipe last-status.
# Sew S27: `k2 app grant hooks …` — grant-hook status, log, retry, skip,
# reconcile, target. Owner only (the daemon gates every route).
_app_grant_hooks() {
    # A bare `hooks`, or one that starts with --json, is `hooks status`
    # (0.46.1: `k2 app grant hooks --json` was a usage error).
    case "${1:-}" in ""|--json) set -- status "$@" ;; esac
    local action="$1"
    shift
    case "$action" in
        status|"")
            local json_out=0
            [[ "${1:-}" == "--json" ]] && json_out=1
            local raw
            raw=$(cli_request "/cli/skin/grant-hooks")
            printf '%s' "$raw" | _skin_die_if_err
            if [ "$json_out" = 1 ]; then printf '%s\n' "$raw"; return 0; fi
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
ts=d.get("targets") or []
if not ts:
    print("No grant-hook targets (chat is not enabled on this box).")
for t in ts:
    print("%s  %s  %s  socket %s" % (t.get("kind",""), "on" if t.get("enabled") else "off",
        "healthy" if t.get("healthy") else ("unhealthy: " + (t.get("lastError") or "?")), t.get("socket","")))
    def _when(ts):
        import datetime
        return datetime.datetime.fromtimestamp(ts, datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ") if ts else "?"
    def _pass(s):
        keys = [("provisioned","provisioned"),("updated","updated"),("deprovisioned","deprovisioned"),
                ("orphansRemoved","orphans removed"),("skippedBusy","skipped busy"),("skippedHeld","held"),
                ("ignoredLocal","ignored local"),("ignoredOther","ignored other")]
        parts = ["%s %s" % (s.get(k), label) for k, label in keys if s.get(k)]
        return ", ".join(parts) if parts else "no changes"
    if t.get("lastReconcile") is not None:
        print("  last reconcile: %s  %s" % (_when(t.get("lastReconcileAt")), _pass(t["lastReconcile"])))
    if t.get("lastChange"):
        if t.get("lastChangeAt") != t.get("lastReconcileAt"):
            print("  last change   : %s  %s" % (_when(t.get("lastChangeAt")), _pass(t["lastChange"])))
    elif t.get("lastReconcile") is not None:
        print("  last change   : none yet (every pass so far found nothing to fix)")
    hist = t.get("reconcileHistory") or []
    if len(hist) > 1:
        print("  recent passes (newest first):")
        for h in list(reversed(hist))[:5]:
            print("    %s  %s" % (_when(h.get("at")), _pass(h)))
o=d.get("outbox") or {}
age=o.get("oldestPendingAgeSecs")
print("outbox: %s pending, %s blocked, %s held%s" % (o.get("pending",0), o.get("blocked",0), o.get("held",0),
    ("  (oldest pending %ss)" % age) if age is not None else ""))
for r in d.get("blocked") or []:
    print("  BLOCKED %s  %s %s  %s" % (r.get("id",""), r.get("op",""), r.get("subjectKey",""), r.get("lastError") or ""))
for r in d.get("held") or []:
    print("  HELD    %s  %s %s  %s" % (r.get("id",""), r.get("op",""), r.get("subjectKey",""), r.get("lastError") or ""))
print("Actors are reported by K2; not proof a person acted.")'
            ;;
        log)
            local subject="" limit="50" json_out=0
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --subject) subject="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                    --subject=*) subject="${1#--subject=}"; shift ;;
                    --limit) limit="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                    --limit=*) limit="${1#--limit=}"; shift ;;
                    --json) json_out=1; shift ;;
                    *) echo "Usage: k2 app grant hooks log [--subject <ext id>] [--limit N] [--json]" >&2; exit 1 ;;
                esac
            done
            local raw
            if [[ -n "$subject" ]]; then
                raw=$(cli_request "/cli/skin/grant-hooks/log" "limit=$(urlencode "$limit")" "subject=$(urlencode "$subject")")
            else
                raw=$(cli_request "/cli/skin/grant-hooks/log" "limit=$(urlencode "$limit")")
            fi
            printf '%s' "$raw" | _skin_die_if_err
            if [ "$json_out" = 1 ]; then printf '%s\n' "$raw"; return 0; fi
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
rows=d.get("rows") or []
if not rows:
    print("No outbox rows.")
for r in rows:
    print("%s  %-9s %-11s %s  by %s  (%s, %s attempt(s))%s" % (r.get("id",""), r.get("state",""), r.get("op",""),
        r.get("subjectKey",""), r.get("actor",""), r.get("cause",""), r.get("attempts",0),
        ("  " + r["lastError"]) if r.get("lastError") else ""))'
            ;;
        retry|skip)
            local id="${1:-}"
            [[ -z "$id" ]] && { echo "Usage: k2 app grant hooks $action <outbox-id>" >&2; exit 1; }
            local body raw
            body=$(K2_HOOK_ROW="$id" python3 -c 'import json,os; print(json.dumps({"id": os.environ["K2_HOOK_ROW"]}))')
            raw=$(cli_post_json "/cli/skin/grant-hooks/$action" "$body")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Outbox row %s is now %s." % (d.get("id",""), d.get("state","")))'
            ;;
        reconcile)
            local raw
            raw=$(cli_post_json "/cli/skin/grant-hooks/reconcile" "{}")
            printf '%s' "$raw" | _skin_die_if_err
            echo "Reconcile started."
            ;;
        target)
            local kind="${1:-}" enabled="" socket="" token_file=""
            shift || true
            while [[ $# -gt 0 ]]; do
                case "$1" in
                    --on) enabled=true; shift ;;
                    --off) enabled=false; shift ;;
                    --socket) socket="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                    --socket=*) socket="${1#--socket=}"; shift ;;
                    --token-file) token_file="${2-}"; if [[ $# -ge 2 ]]; then shift 2; else shift; fi ;;
                    --token-file=*) token_file="${1#--token-file=}"; shift ;;
                    *) echo "Usage: k2 app grant hooks target chat --on|--off [--socket <path>] [--token-file <path>]" >&2; exit 1 ;;
                esac
            done
            if [[ -z "$kind" || -z "$enabled" ]]; then
                echo "Usage: k2 app grant hooks target chat --on|--off [--socket <path>] [--token-file <path>]" >&2
                exit 1
            fi
            local body raw
            body=$(K2_HOOK_KIND="$kind" K2_HOOK_ON="$enabled" K2_HOOK_SOCKET="$socket" K2_HOOK_TOKEN_FILE="$token_file" python3 -c '
import json,os
b={"kind": os.environ["K2_HOOK_KIND"], "enabled": os.environ["K2_HOOK_ON"] == "true"}
if os.environ.get("K2_HOOK_SOCKET"):
    b["socket"]=os.environ["K2_HOOK_SOCKET"]
if os.environ.get("K2_HOOK_TOKEN_FILE"):
    b["tokenFile"]=os.environ["K2_HOOK_TOKEN_FILE"]
print(json.dumps(b))')
            raw=$(cli_post_json "/cli/skin/grant-hooks/target" "$body")
            printf '%s' "$raw" | _skin_die_if_err
            printf '%s' "$raw" | python3 -c '
import json,sys
d=json.load(sys.stdin)
print("Target %s %s (socket %s)" % (d.get("kind",""), "on" if d.get("enabled") else "off", d.get("socket","")))'
            ;;
        --help|-h)
            echo "Usage: k2 app grant hooks [status|log|retry <id>|skip <id>|reconcile|target chat --on|--off]" >&2
            exit 0 ;;
        *)
            echo "Usage: k2 app grant hooks [status|log|retry <id>|skip <id>|reconcile|target chat --on|--off]" >&2
            exit 1 ;;
    esac
}

_skin_die_if_err() {
    python3 -c '
import json, sys
raw = sys.stdin.read().strip()
if not raw:
    print("Error: empty response from daemon (is it running?)", file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: " + raw[:200], file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    sys.exit(0)
err = d.get("error")
if err is None and d.get("ok") is False:
    err = d
if err is None:
    sys.exit(0)
if isinstance(err, dict):
    code = err.get("code") or "error"
    hint = err.get("hint") or err.get("message") or code
    if code in ("owner_only", "forbidden", "gated") and "ask your human" not in hint:
        hint = (hint + " — " if hint else "") + "requires owner/admin — ask your human"
    print("Error: " + hint, file=sys.stderr)
    sys.exit(3 if code in ("owner_only", "forbidden", "gated") else 2 if code == "usage" else 1)
print("Error: %s" % err, file=sys.stderr)
if isinstance(err, str) and ("token" in err.lower() or "forbidden" in err.lower() or "owner" in err.lower()):
    sys.exit(3)
sys.exit(1)
'
}

_teach_skin_now_app() {
    echo "k2 skin is now k2 app — this command still works." >&2
}

_teach_skin_token_now_app_token() {
    echo "k2 skin-token is now k2 app token — this command still works." >&2
}

_skin_usage() {
    cat >&2 <<'EOF'
Usage: k2 app <user|role|grant|token|validate|hydra|front-door> ...
       k2 app token <create|list|revoke|rooms> ...
       k2 app validate [<name>] [--dir <path>] [--json]
                                        Parse an App's JS/TS and run the leak lints (exit 1 on errors)
       k2 app <build|install|deploy|rollback|list|inspect|history|logs|errors|...>

  Guests, roles, and tokens for apps on this box, and (Apps beta) hosting
  server Apps: k2 app deploy (k2 app validate --help, k2 study app-deploys).
  A built static site (a web build, docs, a PWA) deploys as an App
  (host.mode "static"; k2 app init --template static). A site already on
  k2 publish run --skin keeps working; new static sites are Apps.
  Desktop binary updates are k2 update --app. See k2 study apps (leftover: k2 study skins).
  Your own sign-in page: post passwords straight to K2 at /.k2/auth/* on the
  App's host, never to your server (k2 app auth --help, k2 study auth-pages).

  Leftover aliases (still work): k2 skin …    k2 skin-token …

  k2 app user add <username> [--email e] [--password-stdin]
                                        Owner only. Add an app principal (not a Connect user) with the box login
  k2 app user add <username> --app <app> [--role R] [--email e] [--password-stdin | --temp-password]
                                        A guest of ONE App: that App's grant, no box login. The owner,
                                        or an agent with "Let this agent manage its Apps' users" for
                                        its own workspace's Apps. An agent's login works at once in the
                                        App's own room; any other room its role names waits until the
                                        person chooses their own password (the answer says so and how).
                                        Names people on this box use are refused too. --temp-password:
                                        K2 makes a password and prints it once; the person chooses their
                                        own at first sign-in (or k2 app user invite: a mailed link, so you
                                        handle no password)
  k2 app user add <app-id> <name> --test [--role R] [--room H] [--expires 24h] [--password-stdin]
                                        A test guest of ONE App: signs in only through that App's
                                        link, a role the App defines, ends on its own (24 h, at most
                                        7 d), at most 5 per App (k2 app test-guests). The owner, or
                                        the App's own agent with "Let this agent manage its Apps'
                                        users" (else one held request: a card for the owner, exit 2)
  k2 app user invite <username> --app <app> [--email e] [--role R] [--from <address>] [--json]
                                        Mail a one-time link to set their own password (new people
                                        need --email and are made App-only). The owner, or an agent
                                        for its own workspace's Apps (from its workspace's address,
                                        under its email sending mode). Links expire (k2 app login settings)
  k2 app user reset <username> [--app <app>] [--revoke] [--to <email> | --code] [--from <address>] [--json]
                                        An agent with "Let this agent manage its Apps' users" on, for a login
                                        that signs in only on your workspace's Apps (one also on another
                                        workspace's App answers not_found: ask the owner). K2 mails
                                        the person the App's reset link, to the email on the login, from the
                                        App's address (you never see the link). Sending it takes nothing away; access
                                        held back until they set a password starts when they do.
                                        --revoke takes back that login's unused links on the App and sends nothing.
                                        The owner, or an admin for App-only users (box logins stay
                                        the owner's). A one-time reset link on K2's own page: when used,
                                        extra access waiting for the person's own password starts. --code: the link is printed for you to hand over. Mailing it needs --from
                                        <an address no agent reads>; a tied person's stored email
                                        (or one from before 0.46.4 you haven't set) needs --to.
                                        An agent wanting --code, --to or --from asks a person on a card,
                                        k2 permission request apps.user-password-reset user=<username>
                                        [app=<app>] [mode=password|link|email] --why "<why>". The
                                        person who allows it sees the new password (or a one-time
                                        link) once, on their own screen; you never see it
  k2 app login settings <app> [--invite-ttl 72h] [--reset-ttl 24h] [--set-password-url <url>] [--max-devices <n>] …
                                        The App's link lives, set-password page, own mail text
                                        and how many devices a person may keep signed in
  k2 app profile settings <app> [--names on|off] [--image on|off]
                                        May the App's people change their own names and picture
                                        (default on; the App's agent may only turn them off)
  k2 app set <app> [--account-deletion request|immediate] [--deletion-grace <days>] [--client-logs-days <days>]
                                        How "Delete my account" works in the App (request by
                                        default, 30-day grace) and how long client logs stay
  k2 app recording <app> [set [--start-level L] [--audio-max-days N|org] [--required on|off] [--server-read on|off]]
                                        Recording in the App's calls (Sew 0.9.0). All off by default;
                                        each person still consents. --required on and --server-read on
                                        are the owner's only (an agent asks: k2 permission request
                                        apps.recording app=<app> server-read=on --why "…").
                                        k2 app recording --help, k2 study app-recording
  k2 app account-deletion <app> | complete|cancel <app> <request|user>
                                        Pending account deletions: list, complete now, undo
  k2 app user show <username> --app <app> [--json]
                                        Owner only. Their profile in that App: names (a self-set one
                                        names its username), email, picture, role, sign-ins, sessions
  k2 app user image <username> --clear  Owner only. Remove their picture
  k2 app user sessions <username> --app <app> [--revoke <id> | --revoke-all]
                                        Their browsers and devices in that App; end one or all (their chat
                                        session too). That App only, and their password stays the same (a stolen
                                        password: k2 permission request apps.user-password-reset). The owner, or
                                        the App's own agent with "Let this agent manage its Apps' users"; never
                                        a box login
  k2 app user email <username> [--email e]
                                        Set/clear guest email (omit/empty clears)
  k2 app user full-name <username> [--name <text>]
                                        Set/clear full name (omit/empty clears)
  k2 app user agent <username> <workspace> | <username> --clear
                                        Owner only. Mark this login as that workspace's agent
                                        (db actor kind agent, not guest); --clear = a person again
  k2 app user password <username> --password-stdin
                                        Set/reset K2-login password (empty stdin clears). They must
                                        choose a new one at their next sign-in (no session until then),
                                        except an agent's own test guest (k2 app user add <app> <name> --test)
  k2 app user unlock <username> [--json]
                                        Clear K2's sign-in lockout (the owner, an admin for App-only users,
                                        or an agent with the App-users switch for a login on its own App;
                                        audited). The password stays the same. An agent clears one login
                                        at most 6 times an hour (429 unlock_limit): at the limit it's being
                                        guessed or they need a reset (k2 app user reset), it ends by itself
                                        after 15 minutes, and the owner has no cap.
  k2 app user chat <username> [member|guest|none] [--app <app>] [--json]
                                        A person's chat (their chat account, k2:principal:<id>); no scope = show it.
                                        The owner: anyone (--app: that App's user). An agent: its own App's
                                        users under its guests-chat grant, once Sew has app_scoped_sessions.
  k2 app user list                     Roster (username  full name|—  role|—  rooms|—  email|—  [agent:<ws>] [waiting for reset: set up by <ws>'s agent]; no secrets)
  k2 app user remove <username>        Drop principal + login sessions (platform tokens survive)
  k2 app user rooms <username> --agent sales [--app <app>]
                                        Set default rooms (unassigned guests only); --app sets that App's rooms
                                        (the owner, or an agent for its own workspace's Apps: any room on this
                                        box). Signed-in people get the change
                                        at their next request
  k2 app user role <username> <role>   Assign an app role (not Connect owner/admin/member)
  k2 app user unassign <username>      Clear the assigned role (restore Thread-only + default rooms)
  k2 app role create <name> [--agent sales] [--app <app>]
                                        Named bundle. No --agent = Thread dark. --agent without --caps = Thread-only on that room.
                                        --app tags the role for one App (usable only on that App's grants).
  k2 app role room <role> <handle> [--caps thread:read,thread:post,files:read]
                                        Add/replace functions on one room. Omit --caps = Thread-only. Not --caps with --agent.
                                        files:write never reaches the agent's .k2/ folder or its root
                                        CLAUDE.md, AGENTS.md and GEMINI.md unless you add
                                        files:agentconfig too (not recommended). k2 study apps.
                                        files:upload = drop files into --upload folders, never see them.
                                        Chat caps (App-wide; any room line counts): chat:read, chat:post,
                                        chat:respond, voice:join, voice:video (needs voice:join). They cap the
                                        person's chat pass; Sew's roles still decide. k2 study app-chat.
  k2 app role room <role> <handle> --clear
                                        Drop that room from the role
  k2 app role paths <role> <room> [--read jobs/J1/,jobs/J3/] [--write jobs/J1/in/] [--upload Inbox/] [--upload-dir jobs/J1/in]
                   [--download reports/,price-list.pdf | --no-download]
                                        Folders this role reaches in one room (any room on the role). Unlisted
                                        folders get the same 403 as a missing cap. --write folders are readable
                                        too; caps still decide. --upload = drop folders (files:upload adds files
                                        in its own subfolder, never lists or reads). --upload-dir = default upload folder.
                                        --download = folders or files files:download may save (omitted = kept, --no-download clears; --download alone changes only them).
  k2 app role paths <role> <room> --clear
                                        Back to the whole room
  k2 app role list                     Roles (per-room lines; no secrets)
  k2 app role update <name> [--agent … | --clear]
                                        --clear empties the whole map. Always rewrites live sessions.
  k2 app role remove <name>            Drop an unassigned role
  k2 app grant list                    Grants (subject, kind, target, role|scope; chat rows show hook state).
                                        An agent sees the logins on its own workspace's Apps.
  k2 app grant add --subject-kind principal|workspace --subject <id> --kind app|mailbox|database|cli|chat --target <id> [--role <role> | --room <handle>…] [--scope <text>]
                                        Owner only. An App's own agent asks for a login on its App on a
                                        permission card: the refusal ends with the line to ask with
                                        (k2 permission request app.login-grant op=add|remove user=… app=… role=…);
                                        the owner's Allow makes it, so don't run the add again.
                                        kind=app is a login on one App: --role, or --room (repeatable;
                                        Thread-only rooms). --target: host, a service id, or <workspace>/<name>.
                                        Other kinds need --scope and no --role (recorded, not enforced).
  k2 app grant add --kind chat --subject-kind principal --subject <username|id> [--scope member|guest]
                                        (an App's user: k2 app user chat <user> member --app <app> instead)
  k2 app grant add --kind chat --subject-kind workspace --subject <handle|id> --scope coworker|assistant:<username|owner>
  k2 app grant add --kind chat --subject-kind connect --subject <login|id> [--scope member|guest]
                                        chat grants create the person's or agent's chat account (Linux, Chat / Voice Hosting enabled);
                                        roles and channels are set in Sew. --target is optional (sew).
                                        A Connect login's first chat pass writes its own connect grant;
                                        switch it off (not rm) to keep that login out of chat.
                                        The owner's linked Connect login is the owner in chat (k2:owner):
                                        no connect grant; revoking k2:owner's Sew sessions ends its
                                        sessions too (intended; one person, one member).
  k2 app grant set --id <grant-id> --scope <scope>
                                        Change a chat grant's scope (member↔guest, assistant re-pin).
  k2 app grant role <grant-id> --role <role>
                                        Owner only. Give an App login (kind=app grant) another role in
                                        place: its live logins stay signed in and get the new role's
                                        rooms now (rm + add would end them). The role must be box-wide
                                        or that App's. Ids: k2 app grant list.
  k2 app grant rm --subject-kind principal|workspace --subject <id> --kind <kind> --target <id>
  k2 app grant rm --id <id>            Revoke the grant. The role stays. Ends that App's live logins.
                                        A person's chat grant also removes assistant grants pinned to them.
  k2 app grant hooks [status] [--json] Grant-hook targets, outbox counts, blocked + held rows
  k2 app grant hooks log [--subject <ext id>] [--limit N] [--json]
  k2 app grant hooks retry <outbox-id> | skip <outbox-id> | reconcile
  k2 app grant hooks target chat --on|--off [--socket <path>] [--token-file <path>]
  k2 app grant copy-host <app> [--dry-run] [--json]
                                        Everyone with the box login gets the same access on <app>
                                        (an App published after 0.45.5 starts with nobody). The owner, at
                                        once; an agent's (its own App) is one held request listing the
                                        logins: a card for the owner (exit 2). --dry-run only looks.
  k2 app grant <app> --kind guests-rooms [--room <handle>]... [--revoke]
                                        <app>'s own code sets its OWN guests' rooms with its own pass (no
                                        owner token): its --room rooms plus its own, never above a role.
                                        An agent with "Let this agent manage its Apps' users" on doesn't
                                        need this to give people rooms (k2 app user rooms). --revoke stops
                                        the App's code handing out more; rooms already on a login or role
                                        stay until they're changed. The
                                        owner; or the App's own agent in its own room with "Let this
                                        agent manage its Apps' users" (else, or another workspace's
                                        room, one held request: a card for the owner, exit 2).
  k2 app grant <app> --kind guests-chat [--max N] [--revoke]
                                        Lets <app> give its own users chat (the owner only; works once Sew has
                                        app_scoped_sessions: k2 hostchat home). k2 study app-chat.
  k2 app grant <app> --kind chat-key [--caps "…"] [--max-level L] [--uploads on|off] [--max N] [--revoke]
                                        A chat key: K2 signs chat passes for <app>'s own users with no chat
                                        grant per person (Sew 0.6.0+). On or wider: the owner in person, with a
                                        confirm. Narrower or --revoke: the owner, an Admin or the App's agent.
                                        k2 app grant <app> --kind chat-key --help: every flag.
  k2 app guest rooms <app> <user> [set <room,...>]
                                        Read/set one guest's rooms on <app> (owner; an agent with the App-users switch).
                                        k2 app guest --help.
  k2 app guest ceiling <app> | list <app> [--json]
                                        What <app> may hand its guests; its guests with their rooms (0.46.4).
  k2 app hydra [on|off] [--json]       OIDC issuer sidecar (Linux; leftover)
  k2 app token create --name vercel --agent sales [--caps thread:read,thread:post,files:read]
                                        Mint platform k2skn_ pass (secret once; ≥1 --agent; no user)
  k2 app token list                    Platform tokens only (prefix + caps + rooms; hide sessions)
  k2 app token rooms <id> --agent sales
  k2 app token rooms <id> --clear      Empty rooms (Thread dark, keep secret)
  k2 app token revoke <id>

WHAT THIS IS
  Users + roles + tokens + scopes (caps + rooms), and (Apps beta) the App
  runtime: Seeds, deploys, rollback (k2 study app-deploys). Never grid.

PER-APP LOGINS (0.45.5)
  A login is for ONE App: the box login (host) or a published App. It needs
  that person's enabled grant for that App, and its pass only reaches that
  App's rooms. Wrong App = the same 401 as a bad password. A --skin App's
  helper always logs in for its own App. A BFF (--cmd) names its App on
  POST /cli/skin/login with {"app": $K2_APP_ID}; no app = the box login.
  See k2 study apps.

HOST THE UI (k2 study apps)
  A new App: k2 app init --template static for a folder of files, or
  k2 app install + k2 app deploy for your own code with a port (Apps beta;
  k2 study app-deploys). A site already on k2 publish run --skin keeps working;
  --skin makes no new ones.

Caps: thread:read (GET /cli/thread + overlay WS + find-only ensure-pinned-chat),
thread:post (POST /cli/thread/post),
files:read (GET /cli/fs/read-dir|read-file|read-binary|read-range + WS /cli/fs/events),
files:write (POST /cli/fs/write-file|upload-binary|upload-chunk|create|copy|move|rename;
  never into the agent's .k2/ folder),
files:agentconfig (opt-in: also lets files:write reach .k2/ and the root CLAUDE.md, AGENTS.md, GEMINI.md; not recommended),
files:download (opt-in: save Thread files, GET /cli/thread/attachment
  mode=download; never implied by thread:read or files:read; opening files is unchanged).
Rooms: workspace handles. Empty = Thread dark.
Grid/PTY is never an app room. Capability check paths (full):
  GET /cli/sessions/grid  and  GET /cli/sessions/bytes
  → 403 {"error":"skin tokens cannot use the terminal"}
  Do not probe /cli/grid or /cli/pty for that string.

LEFTOVER (not how you host an app)
  k2 app front-door [connect|direct] [--ui-port N] [--url URL] [--hint TEXT] [--json]
                                        Optional Caddy path-filter (owner). Not required.
EOF
}

# Shared printer for validate answers ({ok, status, errors, warnings, ...}):
# file:line:col lines, each followed by its suggestion. Loopable exit codes
# (decision-k2-runtime-v1 stage 0 (e)): 0 ok, 1 failed, 2 pending.
_validate_print() {
    V_JSON="$1" V_RAW="$2" V_WHAT="$3" python3 -c '
import json, os, sys
raw = os.environ.get("V_RAW") or ""
as_json = os.environ.get("V_JSON") == "true"
if not raw.strip():
    print("Error: no response from the K2 daemon on this computer", file=sys.stderr)
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    print("Error: the daemon answered something that is not JSON: %s" % raw[:200], file=sys.stderr)
    sys.exit(1)
if not isinstance(d, dict):
    print("Error: unexpected answer: %s" % raw[:200], file=sys.stderr)
    sys.exit(1)
err = d.get("error")
if err is not None:
    if as_json:
        print(json.dumps(d, separators=(",", ":")))
    msg = d.get("message") or (err.get("message") if isinstance(err, dict) else err)
    print("Error: %s" % msg, file=sys.stderr)
    if d.get("suggestion"):
        print("  suggestion: %s" % d["suggestion"], file=sys.stderr)
    sys.exit(1)
status = d.get("status") or ("ok" if d.get("ok") else "failed")
code = {"ok": 0, "pending": 2}.get(status, 1)
if as_json:
    print(json.dumps(d, indent=2))
    sys.exit(code)
def line(e, warn):
    s = "%s:%s:%s: %s%s" % (e.get("file"), e.get("line"), e.get("col"), "warning: " if warn else "", e.get("message"))
    if e.get("suggestion"):
        s += "\n  suggestion: %s" % e["suggestion"]
    return s
for e in d.get("errors") or []:
    print(line(e, False))
for w in d.get("warnings") or []:
    print(line(w, True))
for s in d.get("skipped") or []:
    print("skipped: %s" % s)
errs = len(d.get("errors") or [])
warns = len(d.get("warnings") or [])
what = os.environ.get("V_WHAT") or "files"
if errs:
    print("%d error(s), %d warning(s) in %s file(s) of %s" % (errs, warns, d.get("files"), d.get("target") or what), file=sys.stderr)
else:
    print("ok: %s file(s) of %s parse%s" % (d.get("files"), d.get("target") or what, "; %d warning(s)" % warns if warns else ""))
sys.exit(code)
'
}

# Stage 0 (decision-k2-runtime-v1): parse a folder's JS/TS (and inline
# scripts) and run the leak lints. GET /cli/publish/validate; read-only.
_app_validate_code() {
    local name="$1" dir="$2" as_json="$3"
    # An absolute path inside this workspace becomes workspace-relative.
    if [ -n "$dir" ] && [ "${dir#/}" != "$dir" ]; then
        local ws="${PROJECT%/}"
        if [ "$dir" = "$ws" ]; then dir="."
        elif [ "${dir#"$ws"/}" != "$dir" ]; then dir="${dir#"$ws"/}"
        fi
    fi
    local -a q=()
    [ -n "$name" ] && q+=("name=$(urlencode "$name")")
    [ -n "$dir" ] && q+=("dir=$(urlencode "$dir")")
    local raw
    raw=$(cli_request "/cli/publish/validate" ${q[@]+"${q[@]}"}) || raw=""
    _validate_print "$as_json" "$raw" "${name:-${dir:-this workspace}}"
}

# `k2 app validate` — ONE command for stage 0 and the App runtime (vs-live 2):
#   a folder with manifest.json, or a Seed  -> POST /cli/app/validate
#     (manifest, contract, the same oxc code pass, --types) when Apps (beta)
#     are on; with them off, a folder falls back to the code pass;
#   a plain folder (--dir or a path)        -> the code pass only;
#   a published App name, or nothing       -> the code pass on its folder.
cmd_app_validate() {
    local name="" dir="" target="" as_json=false types=0 manifest=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --dir) dir="${2-}"; [ $# -ge 2 ] && shift 2 || shift ;;
            --dir=*) dir="${1#--dir=}"; shift ;;
            --manifest) manifest="${2-}"; [ $# -ge 2 ] && shift 2 || shift ;;
            --manifest=*) manifest="${1#--manifest=}"; shift ;;
            --types) types=1; shift ;;
            --json) as_json=true; shift ;;
            --help|-h)
                cat <<'EOF'
k2 app validate — check an App's code before you publish, build or deploy it

USAGE
  k2 app validate <folder>        a folder (a path, or --dir <path>)
  k2 app validate <seed>          a stored Seed (k2 app build prints its hash)
  k2 app validate <name>          a published App (k2 publish ps): its --skin dir or --cwd
  k2 app validate                 this workspace's folder
  k2 app validate <folder> --manifest <file>
                                  check the folder as k2 app deploy --manifest
                                  packs it: <file> becomes its manifest.json
                                  (a build output like dist/ is checked too)
  ... --types                     also run the TypeScript checker (Seeds and
                                  manifest folders; its errors block)
  ... --json                      the report as JSON

WHAT IT CHECKS (it never runs your code)
  - every .js .mjs .cjs .jsx .ts .tsx .mts .cts file and every inline <script>
    in .html pages parses (oxc): errors, file:line:col, each with a suggestion
  - leak lints, as warnings: module-level Map/Set/array/object growth inside
    request/event handlers, setInterval started in a handler with no clear,
    listeners added per request to long-lived targets, session stores with
    no expiry (express-session with no store, MemoryStore)
  - a folder with manifest.json, or a Seed (Apps (beta) on: Settings → Apps →
    This server, on by default): also manifest.json and the K2 contract (caps,
    bindings, roles, requires). With Apps off a folder gets the code
    checks only (said on stderr).
  Skipped inside the folder: node_modules, dist, build, .next and other
  build/hidden folders, symlinks, files over 2 MB; anything skipped is listed.

  A path that exists as a folder wins over a published App with that name.

EXIT CODES
  0 no errors (warnings may be listed)
  1 errors, or the check could not run
  2 bad usage
  3 the daemon refused (JSON with a code on stderr)
EOF
                exit 0 ;;
            -*) echo "Error: unknown flag '$1' — usage: k2 app validate [<folder|seed|name>] [--dir <path>] [--manifest <file>] [--types] [--json]" >&2; exit 2 ;;
            *)
                if [ -z "$target" ]; then target="$1"; shift
                else echo "Error: unexpected argument '$1'" >&2; exit 2; fi ;;
        esac
    done
    if [ -n "$target" ] && [ -n "$dir" ]; then
        echo "Error: give a folder, Seed or App name, or --dir, not both" >&2
        exit 2
    fi
    # A folder given as a path is relative to here; --dir is relative to the
    # workspace (as it always was).
    if [ -n "$target" ] && [ -d "$target" ]; then
        dir="$(_app_rt_abs "$target")"; target=""
    fi
    if [ -n "$manifest" ]; then
        [ -f "$manifest" ] || { echo "Error: --manifest $manifest is not a file" >&2; exit 2; }
        [ -n "$dir" ] || { echo "Error: --manifest goes with a folder (k2 app validate <folder> --manifest <file>)" >&2; exit 2; }
        manifest="$(_app_rt_abs_file "$manifest")"
    fi
    if [ -n "$dir" ]; then
        local abs="$dir"
        [ "${dir#/}" = "$dir" ] && abs="${PROJECT%/}/$dir"
        if [ -f "$abs/manifest.json" ] || [ -n "$manifest" ]; then
            _app_validate_seed folder "$abs" "$types" "$as_json" "$manifest" && return 0
            # 10 = Apps (beta) are off: the code pass still runs.
            [ "$?" = 10 ] || exit 1
            echo "note: Apps (beta) are off on this server (Settings → Apps → This server), so manifest.json and the contract were not checked; code checks only" >&2
        fi
        if [ "$types" = 1 ]; then
            echo "Error: --types needs a folder with manifest.json (or a Seed) and Apps (beta) on" >&2
            exit 2
        fi
        _app_validate_code "" "$dir" "$as_json"
        return
    fi
    # A Seed hash (or a unique prefix of one: hex, 12+ characters).
    if [ -n "$target" ] && [[ "$target" =~ ^[0-9a-f]{12,64}$ ]]; then
        _app_validate_seed seed "$target" "$types" "$as_json" && return 0
        [ "$?" = 10 ] && _compute_fail "404" "$_CP_BODY"
        exit 1
    fi
    if [ "$types" = 1 ]; then
        echo "Error: --types needs a folder with manifest.json or a Seed" >&2
        exit 2
    fi
    _app_validate_code "$target" "" "$as_json"
}

