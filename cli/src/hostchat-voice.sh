# ── k2 hostmail (server-admin family) ───────────────────────────────────

# ── k2 hostchat — the Sew chat (and voice) server on this box ───────────
# Sew S29 K14 over /cli/sew/*. Owner/admin, or an agent whose workspace has
# "Manage Sew" (chat_manage) on; person-only steps (purge and the launch
# key: the owner; dropping the old master key: the owner or an admin) need a
# vault confirm from a terminal outside K2. Setup links print to stdout only (never stderr).
# Exit: 0 ok · 1 you said no at a [y/N] prompt · 2 bad request · 3 refused / infra.

# ── `k2 voice` — voice rooms on this server (prd-k2-voice-rooms-v1 §9.3) ──
# Preview: every route answers voice_rooms_off until the owner runs
# `k2 settings set voiceRoomsPreview true`. The daemon (/cli/voice/*) owns the rooms, who's in
# them and the hosting state; this CLI only reads and asks.

_voice_usage() {
    cat <<'EOF'
Usage: k2 voice <command> [args] [--json]

Voice rooms on this server (a preview; Linux servers with Pro host them).
People talk in them from the K2 app; this CLI lists them, shows why a
server can't host, and lets the owner or an Admin change them. Read
`k2 study voice-rooms` first.

  k2 voice rooms                      the rooms and who is in each (anyone, agents too)
  k2 voice status                     hosting state and why, the K5 Space,
                                      members, rooms, sync holds, the voice lane
                                      (owner/Admin; agents read it)
  k2 voice repair                     rooms stopped working (joins refused, status
                                      says the K5 Space is "checking" or "suspect")?
                                      Check the K5 Space with Sew and fix it: make
                                      it and its rooms again if Sew lost them, re-link
                                      the rooms, re-add every member, retry what Sew
                                      refused. Waits up to 30 s, then says what it
                                      found and fixed ("still working" = K2 keeps at
                                      it; run it again later). Safe to run twice.
                                      Owner/Admin, or an agent with Manage Chat /
                                      Voice Hosting
  k2 voice set rooms <0-10> [--now]   how many rooms this server hosts (owner/Admin).
                                      Lowering closes rooms from the top: an empty
                                      room closes at once, a room with people closes
                                      when the last one leaves. --now: people get a
                                      10 s notice, then the room closes and calls end.
  k2 voice rename <room> <name>       rename a room (owner/Admin). <room> is 1, room-1
                                      or its name. Numbers and slugs never change
  k2 voice close <room> [--now]       close the top room: <room> must be the highest
                                      one (a check), else close_from_top. Same as
                                      lowering the count by one (owner/Admin)
  k2 voice join <room>                join as your own login (for voice clients and
                                      tests: the CLI has no audio)
  k2 voice leave [--call <callId>]    leave your call
  k2 voice consent <recording-id> yes --notice <version>
  k2 voice consent <recording-id> no  answer a call's recording notice as your own
                                      login, while you are in that call. yes records
                                      you; no declines, or withdraws a yes; the last
                                      answer counts. Both values come from the notice
                                      your voice client shows (recording id and
                                      notice_version); yes must name the version
                                      you were shown. Like join, for voice clients
                                      and tests: the CLI sees no notices.
                                      HTTP shape: k2 study voice-rooms
  k2 voice kick|mute|unmute|deafen|undeafen <room> <person>
                                      moderate (owner/Admin, from their own login on
                                      this server; Sew records who did it). kick ends
                                      their call; mute stops them speaking and deafen
                                      stops them hearing, until unmute/undeafen.
                                      <person> is a login name or owner as
                                      k2 voice rooms shows it (--json: memberId)
  k2 voice settings [--start-level none|moderator|member|inherit]
                    [--require-recording on|off|inherit]
                                      recording in the K5 rooms (Sew 0.9.0); no flags:
                                      show it. --start-level: the lowest level that may
                                      start a recording (none = nobody, the default;
                                      owner/Admin). --require-recording on: every call
                                      with two or more people is recorded (the owner
                                      only). In these rooms people are told, not asked:
                                      the notice is OK or leave, and someone who
                                      doesn't answer in a minute is dropped.
                                      inherit = the server's own rule
                                      (k2 hostchat recording policy)

Call transcripts (after a call; k2 study voice-rooms):
  <room> is 1, room-1, or the room's name, as k2 voice rooms shows it.
  k2 voice recordings [--room <room>] the K5 rooms' recordings, oldest first
                                      (id, room, start time, transcript ready or not;
                                      no text). More: --after the last id shown
  k2 voice transcript <recording-id> [--after <line-id>] [--limit N]
                                      one recording's transcript, 200 lines a page;
                                      the output ends with the command for the next
                                      page. Every read is logged with who read it
  k2 voice transcripts [--room <room>]
                                      who may read them, and which agents each room
                                      sends its transcripts to
  k2 voice transcripts subscribe <room> <workspace>
  k2 voice transcripts unsubscribe <room> <workspace>
                                      a note in that agent's inbox when a recording in
                                      <room> has its transcript (the owner, an Admin,
                                      or the agent itself; the agent needs "Read call
                                      transcripts")
  k2 voice transcripts reads [--recording <id>]
                                      the read log (owner/Admin)
  Agents read only with the switch "Read call transcripts" on (its card id
  is voice.transcripts-read; only the owner turns it on; ask once with
  k2 permission request voice.transcripts-read --why "…" and wait). The
  bare owner token lists but never reads a transcript
  (owner_token_cannot_read): an agent seeing it is running without its
  passport (K2_* unset in that shell); run it from the agent's own session.

Agents: read only (rooms, status). Every change, join, consent and
moderation refuses an agent's passport (owner_or_admin_only,
agents_not_in_voice, agents_cannot_moderate): ask your human in your
Thread. One exception: an agent whose workspace has Manage Chat / Voice
Hosting on may run k2 voice repair (other agents get chat_manage_off).
The bare owner token reads and changes rooms but never joins,
answers a recording notice or moderates (owner_token_cannot_join,
owner_token_cannot_moderate).

Talks to this server (or the one --host names), like every k2 verb.
Exit codes: 0 ok, 2 bad usage, 3 refused or unavailable. stderr carries
{"error": <code>, "hint": …}: voice_rooms_off (preview off: the owner
runs k2 settings set voiceRoomsPreview true), owner_or_admin_only, moderator_only, agents_not_in_voice,
room_closing / room_full / room_not_found, not_in_room, close_from_top,
voice_unavailable or voice_member_pending (retry in a few seconds).
consent: recording_not_found, recording_not_active, not_in_voice (join
the call first), consent_notice_changed (show the new notice in
details.consent_prompt, then answer again).
Full list and fixes: k2 study voice-rooms.
EOF
}

# Print _CP_BODY for a verb: raw JSON with --json, else a summary.
_voice_print() {
    local verb="$1" as_json="$2"
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$_CP_BODY"
        return 0
    fi
    printf '%s' "$_CP_BODY" | K2_VC_VERB="$verb" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
verb = os.environ["K2_VC_VERB"]
h = v.get("hosting") or {}
def hosting_line():
    if h.get("state") == "ok":
        return "on"
    s = "%s: %s" % (h.get("state"), h.get("line") or "")
    if h.get("detail"):
        s += " (%s)" % h["detail"]
    return s
if verb == "rooms":
    print("Voice rooms on %s: %s" % (v.get("server") or "this server", hosting_line()))
    rooms = v.get("rooms") or []
    if not rooms:
        print("  no rooms open")
    for r in rooms:
        occ = r.get("occupants") or []
        tag = ""
        if r.get("closing"):
            tag = "  closing"
        elif r.get("unavailable"):
            tag = "  unavailable: %s" % r["unavailable"]
        print("  %-8s %-24s %d/%s%s" % (r.get("slug"), r.get("name"), len(occ), r.get("limit"), tag))
        for o in occ:
            flags = [f for f, on in (("muted", o.get("muted")), ("deafened", o.get("deafened"))) if on]
            print("      %s%s" % (o.get("user"), (" (" + ", ".join(flags) + ")") if flags else ""))
    me = v.get("me") or {}
    if me.get("inCall"):
        where = "this login" if me.get("here") else "another device or session"
        print("you: in %s from %s (if that app was closed, this can lag up to %s s)" % ((me.get("room") or {}).get("name") or me.get("channelId"), where, me.get("presenceLagSecs")))
elif verb == "status":
    print("hosting:    %s" % hosting_line())
    t = v.get("tier")
    if isinstance(t, dict):
        t = "%s, %s reading" % (t.get("state"), t.get("reading")) + ((": " + t["reason"]) if t.get("reason") else "")
    print("rooms:      %s (plan: %s)" % (v.get("count"), t))
    sp = v.get("space") or {}
    sh = v.get("spaceHealth") or {}
    state = sh.get("state") or sp.get("state") or "not made yet"
    if state == "none":
        state = "not made yet"
    print("K5 Space: %s%s" % (state, (" " + sp["spaceId"]) if sp.get("spaceId") else ""))
    if sh.get("state") in ("checking", "suspect", "blocked"):
        print("  why:      %s" % sh.get("line"))
        print("  fix:      %s" % sh.get("fix"))
    elif sp.get("why"):
        print("  why:      %s" % sp["why"])
    lc = sp.get("lastCheck") or {}
    if lc.get("outcome") in ("remade", "relinked"):
        print("  last check: %s (was %s, now %s)" % ("Sew made it again" if lc["outcome"] == "remade" else "K2 re-linked a stale id", lc.get("from"), lc.get("to")))
    for r in v.get("rooms") or []:
        print("  %-8s %-24s %s%s" % (r.get("slug"), r.get("wantName") or r.get("name") or "", r.get("state"), "  (close now)" if r.get("closeNow") else ""))
    m = v.get("members") or []
    print("members:    %d" % len(m))
    for x in m:
        if x.get("state") != "added" or (x.get("wantLevel") and x.get("wantLevel") != x.get("level")):
            print("  %s: %s%s" % (x.get("externalId"), x.get("state"), (" — " + x["why"]) if x.get("why") else ""))
    for x in v.get("holds") or []:
        print("hold:       %s %s %s%s" % (x.get("op"), x.get("subject"), x.get("state"), (" — " + x["error"]) if x.get("error") else ""))
    if v.get("carriedAgents"):
        print("agents carried to k5-admin: %s" % ", ".join(v["carriedAgents"]))
    st = v.get("staleSpaces") or []
    if st:
        print("set aside:  %d agent Space(s) from an earlier Sew install (%s): k2 voice repair says how to make them again" % (len(st), ", ".join(s.get("slug") or "?" for s in st)))
    ln = v.get("lane") or {}
    print("voice lane: %s%s" % (ln.get("state"), (" — " + ln["reason"]) if ln.get("reason") else ""))
    fd = v.get("feed") or {}
    print("feed:       %s%s" % (fd.get("state"), (" — " + fd["lastError"]) if fd.get("lastError") else ""))
elif verb == "repair":
    print("K5 Space:   %s" % v.get("space"))
    for r in v.get("rooms") or []:
        print("  %s: re-linked to channel %s (was %s)" % (r.get("room"), r.get("to"), r.get("from") or "none"))
    m = v.get("members") or {}
    print("members:    %s in the Space, %s still being added" % (m.get("added"), m.get("pending")))
    for x in m.get("held") or []:
        print("  held:     %s — %s" % (x.get("externalId"), x.get("why")))
    fixed = v.get("fixed") or []
    for f in fixed:
        print("fixed:      %s" % f)
    if not fixed and v.get("settled"):
        print("fixed:      nothing needed fixing")
    if v.get("notRestored"):
        print("note:       %s" % v["notRestored"])
    st = v.get("staleSpaces") or []
    if st:
        print("set aside:  %d agent Space(s) from an earlier Sew install:" % len(st))
        for s in st:
            who = ", ".join("%s (level %s)" % (a.get("workspace") or a.get("projectId"), a.get("level")) for a in s.get("access") or [])
            print("  %s (old id %s): %s" % (s.get("slug"), s.get("spaceId"), who or "no agents"))
        print("  how:      %s" % v.get("staleHint"))
    for x in v.get("holds") or []:
        print("hold:       %s %s %s%s" % (x.get("op"), x.get("subject"), x.get("state"), (" — " + x["error"]) if x.get("error") else ""))
    if not v.get("settled"):
        print("still working: K2 keeps at it; k2 voice status shows progress")
elif verb in ("set", "close"):
    print("rooms: %s" % v.get("count"))
    if v.get("closing"):
        print("closing: %s%s" % (", ".join(v["closing"]), " (calls end in 10 s)" if v.get("closeNowAt") else " (when empty)"))
elif verb == "rename":
    print("%s is now %s" % (v.get("room"), v.get("name")))
elif verb == "join":
    r = v.get("room") or {}
    print("joined %s (call %s). The CLI has no audio: a voice client connects with the answer (--json)." % (r.get("name"), v.get("callId")))
elif verb == "leave":
    why = {"already_left": "you left it before", "moved": "you joined another room from here",
           "joined_elsewhere": "you joined from another device or session; that call goes on",
           "left_elsewhere": "you left from another device or session",
           "sew_dropped": "the voice server had already dropped you",
           "in_another_room": "you are in another room now; that call goes on"}
    w = v.get("why") or ""
    print("nothing to leave: that call already ended (%s) [%s]" % (why.get(w, w), w) if v.get("already") else "left")
elif verb == "settings":
    rec = v.get("recording") if isinstance(v.get("recording"), dict) else v
    fl = rec.get("flags") or {}
    if "changed" in v:
        print("K5 rooms recording: %s" % ("saved" if v.get("changed") else "nothing changed (already so)"))
    if not fl.get("appRecording"):
        print("This server'"'"'s chat can'"'"'t record in rooms yet (it needs Sew 0.9.0: k2 hostchat upgrade).")
    req = rec.get("required")
    print("K5 rooms Space:     %s" % (rec.get("spaceId") or "not made yet"))
    print("start level:        %s" % (rec.get("startLevel") or "inherit (the server'"'"'s rule)"))
    print("require recording:  %s" % ("inherit (the server'"'"'s rule)" if req is None else ("on" if req else "off")))
    if rec.get("locked"):
        print("LOCKED: no recording in the K5 rooms (CUI/FCI); only the owner lifts it, in person")
    if rec.get("agentReaders"):
        print("transcripts:        agents on this server may read them (each person'"'"'s notice says so; k2 voice transcripts)")
elif verb == "recordings":
    items = v.get("recordings") or []
    if not items:
        print("no recordings in this server'"'"'s voice rooms")
    for r in items:
        ready = "transcript ready" if r.get("transcriptReady") else "transcript %s" % (r.get("transcriptState") or "not ready")
        print("  %-20s %-10s %-20s %s  %s" % (r.get("id"), r.get("room") or r.get("channelId"), r.get("startedAt"), r.get("trigger"), ready))
    if v.get("next"):
        print("more: k2 voice recordings --after %s" % v["next"])
    if items and v.get("canRead"):
        print("read one: k2 voice transcript <id>")
elif verb == "transcript":
    r = v.get("recording") or {}
    print("Recording %s in %s, started %s (%s)" % (r.get("id"), r.get("roomName") or r.get("room"), r.get("startedAt"), "transcript ready" if r.get("transcriptReady") else "transcript still being written"))
    lines = v.get("lines") or []
    if not lines:
        print("  (no lines on this page)")
    for t in lines:
        ms = int(t.get("startMs") or 0) // 1000
        print("[%02d:%02d] %s: %s" % (ms // 60, ms % 60, t.get("speaker"), t.get("text")))
    if v.get("next"):
        print("more: k2 voice transcript %s --after %s" % (r.get("id"), v["next"]))
    print("(this read is logged)")
elif verb in ("transcripts", "subscribe"):
    if verb == "subscribe":
        print("%s %s" % (v.get("agent"), ("now gets this room'"'"'s transcripts" if v.get("on") else "no longer gets this room'"'"'s transcripts") + ("" if v.get("changed") else " (already so)")))
    rd = v.get("readers") or []
    print("agents that may read transcripts: %s" % (", ".join(rd) if rd else "none (Read call transcripts is off for every agent)"))
    for r in v.get("rooms") or []:
        print("  %-8s %-24s sends to: %s" % (r.get("room"), r.get("name"), ", ".join(r.get("sendTo") or []) or "nobody"))
elif verb == "reads":
    rows = v.get("reads") or []
    if not rows:
        print("no transcript reads logged")
    for x in rows:
        print("  %s  %-28s recording %s, %s line(s)" % (x.get("at"), x.get("reader"), x.get("recordingId"), x.get("lines")))
elif verb == "consent":
    if v.get("consent"):
        print("recording %s: you said yes; it records you" % v.get("recordingId"))
    else:
        print("recording %s: you said no; it does not record you (a yes you gave before is withdrawn)" % v.get("recordingId"))
else:
    print("%s %s in %s" % (v.get("action"), v.get("person"), (v.get("room") or {}).get("name")))
'
}

cmd_voice() {
    if [ $# -eq 0 ]; then _voice_usage; return 0; fi
    local sub="$1"; shift
    case "$sub" in -h|--help|help) _voice_usage; return 0 ;; esac
    local as_json=0 now=0 call="" notice="" rec_req="" rec_level="" vt_room="" vt_after="" vt_limit="" vt_rec="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --now) now=1; shift ;;
            --room) vt_room="${2:?--room <room>}"; shift 2 ;;
            --after) vt_after="${2:?--after <id>}"; shift 2 ;;
            --limit) vt_limit="${2:?--limit N}"; shift 2 ;;
            --recording) vt_rec="${2:?--recording <id>}"; shift 2 ;;
            --call) call="${2:?--call <callId>}"; shift 2 ;;
            --notice) notice="${2:?--notice <version>}"; shift 2 ;;
            --require-recording) rec_req="${2:?--require-recording on|off|inherit}"; shift 2 ;;
            --start-level) rec_level="${2:?--start-level none|moderator|member|inherit}"; shift 2 ;;
            -h|--help) _voice_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 voice --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local n=${#pos[@]}
    _voice_bad() { echo "{\"error\":\"usage\",\"hint\":\"$1 (k2 voice --help)\"}" >&2; exit 2; }
    if [ "$now" = "1" ]; then
        case "$sub" in set|close) ;; *) _voice_bad "--now only applies to k2 voice set rooms and k2 voice close" ;; esac
    fi
    if [ -n "$notice" ] && [ "$sub" != "consent" ]; then
        _voice_bad "--notice only applies to k2 voice consent"
    fi
    if { [ -n "$rec_req" ] || [ -n "$rec_level" ]; } && [ "$sub" != "settings" ]; then
        _voice_bad "--require-recording and --start-level only apply to k2 voice settings"
    fi
    if { [ -n "$vt_room" ] || [ -n "$vt_after" ] || [ -n "$vt_limit" ] || [ -n "$vt_rec" ]; } && \
        [ "$sub" != "recordings" ] && [ "$sub" != "transcript" ] && [ "$sub" != "transcripts" ]; then
        _voice_bad "--room, --after, --limit and --recording only apply to k2 voice recordings, transcript and transcripts"
    fi
    for _vt in "$vt_after" "$vt_limit" "$vt_rec"; do
        case "$_vt" in *[!0-9]*) _voice_bad "--after, --limit and --recording are whole numbers" ;; esac
    done
    local vt_q=""
    [ -n "$vt_room" ] && vt_q="${vt_q}&room=$(urlencode "$vt_room")"
    [ -n "$vt_after" ] && vt_q="${vt_q}&after=${vt_after}"
    [ -n "$vt_limit" ] && vt_q="${vt_q}&limit=${vt_limit}"
    case "$sub" in
        recordings)
            [ "$n" -eq 0 ] || _voice_bad "k2 voice recordings [--room <room>]"
            [ -z "$vt_after$vt_rec" ] || [ -z "$vt_rec" ] || _voice_bad "--recording only applies to k2 voice transcripts reads"
            _hostchat_call GET /cli/voice/recordings "${vt_q#&}"
            _voice_print recordings "$as_json" ;;
        transcript)
            [ "$n" -eq 1 ] || _voice_bad "k2 voice transcript <recording-id> [--after <line-id>] [--limit N]"
            case "${pos[0]}" in ''|*[!0-9]*) _voice_bad "the recording id is a number (k2 voice recordings lists them)" ;; esac
            [ -z "$vt_room$vt_rec" ] || _voice_bad "k2 voice transcript takes --after and --limit"
            _hostchat_call GET /cli/voice/transcript "id=${pos[0]}${vt_q}"
            _voice_print transcript "$as_json" ;;
        transcripts)
            local tsub="${pos[0]:-list}"
            case "$tsub" in
                list)
                    [ "$n" -le 1 ] || _voice_bad "k2 voice transcripts [--room <room>]"
                    _hostchat_call GET /cli/voice/transcripts "${vt_q#&}"
                    _voice_print transcripts "$as_json" ;;
                subscribe|unsubscribe)
                    [ "$n" -eq 3 ] || _voice_bad "k2 voice transcripts $tsub <room> <workspace>"
                    local onb=1; [ "$tsub" = "unsubscribe" ] && onb=0
                    _hostchat_call POST /cli/voice/transcripts/subscribe "$(_compute_json K2_CJ_room="${pos[1]}" K2_CJ_agent="${pos[2]}" K2_CJ_on_b="$onb")"
                    _voice_print subscribe "$as_json" ;;
                reads)
                    [ "$n" -eq 1 ] || _voice_bad "k2 voice transcripts reads [--recording <id>]"
                    local rq=""
                    [ -n "$vt_rec" ] && rq="id=${vt_rec}"
                    [ -n "$vt_limit" ] && rq="${rq:+$rq&}limit=${vt_limit}"
                    _hostchat_call GET /cli/voice/transcripts/reads "$rq"
                    _voice_print reads "$as_json" ;;
                *) _voice_bad "k2 voice transcripts [list|subscribe|unsubscribe|reads]" ;;
            esac ;;
        settings)
            # Sew 0.9.0: the K5 rooms' recording. Read through voice/status.
            [ "$n" -eq 0 ] || _voice_bad "k2 voice settings [--start-level none|moderator|member|inherit] [--require-recording on|off|inherit]"
            if [ -z "$rec_req" ] && [ -z "$rec_level" ]; then
                _hostchat_call GET /cli/voice/status ""
            else
                local rq_j lv_j
                rq_j="$(_rec_switch_json "$rec_req" inherit)" || _voice_bad "--require-recording is on, off or inherit"
                case "$rec_level" in
                    ""|none|moderator|member|inherit) lv_j="$(_rec_level_json "$rec_level")" ;;
                    *) _voice_bad "--start-level is none, moderator, member or inherit (the K5 rooms have no admin level)" ;;
                esac
                _hostchat_call POST /cli/voice/recording/settings "$(_compute_json K2_CJ_required_j="$rq_j" K2_CJ_startLevel_j="$lv_j")"
            fi
            _voice_print settings "$as_json" ;;
        rooms|list)
            [ "$n" -eq 0 ] || _voice_bad "k2 voice rooms takes no arguments"
            _hostchat_call GET /cli/voice/rooms ""
            _voice_print rooms "$as_json" ;;
        status)
            [ "$n" -eq 0 ] || _voice_bad "k2 voice status takes no arguments"
            _hostchat_call GET /cli/voice/status ""
            _voice_print status "$as_json" ;;
        repair)
            [ "$n" -eq 0 ] || _voice_bad "k2 voice repair takes no arguments"
            _hostchat_call POST /cli/voice/repair "{}"
            _voice_print repair "$as_json" ;;
        set)
            [ "$n" -eq 2 ] && [ "${pos[0]}" = "rooms" ] || _voice_bad "k2 voice set rooms <0-10> [--now]"
            case "${pos[1]}" in ''|*[!0-9]*) _voice_bad "the room count is a number 0-10" ;; esac
            [ "${pos[1]}" -le 10 ] || _voice_bad "the room count is 0-10"
            local nb=""; [ "$now" = "1" ] && nb=1
            _hostchat_call POST /cli/voice/settings "$(_compute_json K2_CJ_count_n="${pos[1]}" K2_CJ_now_b="$nb")"
            _voice_print set "$as_json" ;;
        rename)
            [ "$n" -ge 2 ] || _voice_bad "k2 voice rename <room> <name>"
            local name="${pos[*]:1}"
            _hostchat_call POST /cli/voice/rooms/rename "$(_compute_json K2_CJ_room="${pos[0]}" K2_CJ_name="$name")"
            _voice_print rename "$as_json" ;;
        close)
            [ "$n" -eq 1 ] || _voice_bad "k2 voice close <room> [--now]"
            local nb=""; [ "$now" = "1" ] && nb=1
            _hostchat_call POST /cli/voice/rooms/close "$(_compute_json K2_CJ_room="${pos[0]}" K2_CJ_now_b="$nb")"
            _voice_print close "$as_json" ;;
        join)
            [ "$n" -eq 1 ] || _voice_bad "k2 voice join <room>"
            _hostchat_call POST /cli/voice/join "$(_compute_json K2_CJ_room="${pos[0]}")"
            _voice_print join "$as_json" ;;
        leave)
            [ "$n" -eq 0 ] || _voice_bad "k2 voice leave [--call <callId>]"
            _hostchat_call POST /cli/voice/session/leave "$(_compute_json K2_CJ_callId="$call")"
            _voice_print leave "$as_json" ;;
        consent)
            local cu="k2 voice consent <recording-id> yes --notice <version> | k2 voice consent <recording-id> no"
            [ "$n" -eq 2 ] || _voice_bad "$cu"
            case "${pos[0]}" in ''|*[!0-9]*) _voice_bad "the recording id is a number (from the notice): $cu" ;; esac
            local cb
            case "${pos[1]}" in
                yes) cb=1; [ -n "$notice" ] || _voice_bad "yes needs --notice <version>, the notice_version you were shown" ;;
                no) cb=0 ;;
                *) _voice_bad "say yes or no: $cu" ;;
            esac
            case "$notice" in *[!0-9]*) _voice_bad "--notice is the notice's version number" ;; esac
            _hostchat_call POST /cli/voice/recording/consent "$(_compute_json K2_CJ_recording_id="${pos[0]}" K2_CJ_consent_b="$cb" K2_CJ_notice_version_n="$notice")"
            _voice_print consent "$as_json" ;;
        kick|mute|unmute|deafen|undeafen)
            [ "$n" -eq 2 ] || _voice_bad "k2 voice $sub <room> <person>"
            _hostchat_call POST /cli/voice/moderate "$(_compute_json K2_CJ_room="${pos[0]}" K2_CJ_person="${pos[1]}" K2_CJ_action="$sub")"
            _voice_print moderate "$as_json" ;;
        *) _voice_bad "unknown command $sub" ;;
    esac
}

_hostchat_usage() {
    cat <<'EOF'
k2 hostchat — Chat / Voice Hosting on this box: the chat and voice server (powered by Sew)
The core's id is sew: k2 app access <workspace> --core sew=on lets that workspace's Apps use it.

Server:
  k2 hostchat status [--health] [--json]
  k2 hostchat preflight [--tarball FILE] [--json]
  k2 hostchat enable [--tarball FILE] [--recovery-key-file FILE]
                     [--identity k2|standalone]
                     [--voice [--voice-url wss://HOST] [--media-tarball FILE] [--node-ip IP]]
                     [--no-wait] [--json]
         --identity picks how people sign in. It is fixed for the life of an
         install: changing it later means a fresh install (k2 hostchat purge,
         then enable). Default k2; standalone = Sew's own password logins.
         Sew's launch key (its recovery key): K2 keeps it in the K2 vault as
         sew-launch-key, so you don't need to copy it. Only the server's owner
         can see it: k2 vault reveal sew-launch-key --scope engine:sew (or
         k2 hostchat recovery-key).
         --recovery-key-file FILE brings an existing key (to reuse Sew data
         already on this box); K2 keeps that one in the vault too (a key
         different from the one the vault holds is refused; K2 leaves the
         file as it is). rotate-master replaces it there with the new key
         (the old one stays an older version); purge deletes it.
         Where Sew comes from: with no --tarball, enable and upgrade (and voice)
         download the release this K2 pins from
         github.com/Alakazam-211/sew-releases into ~/.k2/cache/sew, or reuse
         the copy already there. Nothing is installed unless its sha256
         matches the one built into K2. Sew's App kit, for
         k2 app sdk add --sew, comes with it.
         --tarball FILE (and --media-tarball FILE) are for a box without
         internet, and are checked against the same sha256. On such a box,
         k2 hostchat preflight (it changes nothing) names the exact file to
         fetch, its URL and its sha256. Put sew-app-kit-<version>.tar.gz
         beside --tarball to keep the App kit too (or later:
         k2 app sdk kit FILE).
  k2 hostchat disable | doctor | backup | backups [--json]
  k2 hostchat restart [--json]              (bring Sew back in order: sew-db, sew, then voice; waits for
                                             it to be ready; K2 never restarts Sew on its own)
  k2 hostchat reconfigure [--json]          (re-run Sew's installer over this install; an upgrade from
                                             Sew 0.3.0 does it by itself; needs root helper protocol 11)
  k2 hostchat rollback [--media-tarball FILE] [--json]
                                            (with voice: the previous release's sew-media tarball when the
                                             box doesn't keep it; the old Sew puts that engine back, then
                                             sew doctor's media checks are shown; exit 3 unless all is well.
                                             From Sew 0.7.3 Sew puts its voice engine back itself: --json
                                             .media.state is rolled_back, unchanged, not_installed or left
                                             (done); manual (exit 0: run the one sew media upgrade line it
                                             prints, .mediaCommand); failed (exit 3, media_rollback_failed:
                                             Sew itself is rolled back and running). Never run it again)
  k2 hostchat rollback --accept-data-loss [--media-tarball FILE] [--json]
                                            (after an upgrade that changed how Sew stores chat, the plain
                                             rollback refuses. This one goes back anyway: chat written since the
                                             upgrade is lost from the live chat and stays only in a backup.
                                             Run it as the owner, in a terminal outside K2; it shows what will
                                             happen and asks you to confirm (Touch ID or the vault password).
                                             K2 backs up the current chat, rolls back to the backup taken just
                                             before the upgrade, then checks Sew is healthy, and names both
                                             backups. Chat is offline for a few minutes while it runs. To bring
                                             the newer chat back later: k2 hostchat upgrade, then
                                             k2 hostchat restore <kept backup> --confirm. --media-tarball: as
                                             for the plain rollback, with voice on. An agent asks the owner on
                                             a card instead:
                                               k2 permission request sew.rollback-data-loss --why "<why>")
                                            (K2 0.46.11, Sew 0.9.2: that upgrade changes how Sew stores
                                             chat (0.9.0's 990001, 990002, 990011 and 0.9.2's 991001), so after
                                             it the plain rollback is refused. Going back needs this
                                             --accept-data-loss rollback, by the owner in person; K2 runs
                                             Sew's rollback with --accept-data-loss-since=<the backup
                                             taken just before the upgrade>)
  k2 hostchat upgrade [--tarball FILE] [--media-tarball FILE] [--dry-run] [--no-wait] [--json]
                                            (no flags: downloads the pin, as enable does; --tarball and
                                             --media-tarball (the sew-media release, used when voice is on)
                                             are for a box without internet;
                                             waits for the result; exit 3 unless it is done.
                                             The order: Sew itself, then (voice on) its voice engine, then
                                             (only where the recorder is installed) the recorder and
                                             transcriber, when no call is being recorded.
                                             To Sew 0.9.2 (K2 0.46.11): no new config, recording stays off
                                             until someone turns it on; the way back is
                                             k2 hostchat rollback --accept-data-loss, never the plain one.
                                             A box on Sew 0.8.3 or 0.9.0 upgrades like any release)
  k2 hostchat upgrade --accept-binary SHA256 [--tarball FILE] [...]
                                            (the owner, from a terminal outside K2, with a confirm: upgrade
                                             FROM a /usr/local/bin/sew K2 doesn't know, only if its sha256 is
                                             exactly SHA256; the target is the pin; K2 never rolls back onto it)
  k2 hostchat restore <bundle-id> --confirm [--json]
  k2 hostchat rotate-master [--json]
  k2 hostchat config --public-url https://HOST [--json]
         Sew's server.public_url; password setup links become <public-url>/setup.
         Until it is set, status (and enable) warn with this fix.
  k2 hostchat uninstall [--drop-grants] [--json]          (data, keys, backups kept)
  k2 hostchat job <id> [--json]
  k2 hostchat app-url set https://HOST[/path] | clear | show [--json]
         where the owner's Sew web app lives (k2 hostchat open goes there)
Off-box copies (a folder on ANOTHER disk you mounted: NFS, SMB, sshfs, a replicated volume):
  k2 hostchat ship dir <path> | box-backup | none [--json]
         backup bundles: each new one is copied there (the newest 7 kept).
         box-backup = this box's own nightly backup (k2-backup) already takes
           /var/lib/sew/backups off the box, so K2 copies nothing. The owner
           only (owner token, Owner login or a K2 shell tab; never an admin
           or an agent). The alarm k2_backup_offbox_none clears only while
           K2 sees all three: /etc/k2-backup/paths lists that folder or one
           above it (/, /var, /var/lib, /var/lib/sew; simple globs count),
           /etc/k2-backup/excludes doesn't drop it, and k2-backup.timer is
           enabled. K2 checks on every status read (cached 30 s), so the
           alarm comes back if one changes. It checks the setup, not whether
           last night's run worked. k2 hostchat status names what is missing
           (--json: backup.ship.boxBackup.reason). Root-only files are read
           by the root helper; one older than protocol 13 keeps the alarm and
           says so. Nothing restarts. Undo: ship dir <path> or ship none.
           The keyring mirror stays separate (mirror below).
         none = backups stay on this box only (alarm k2_backup_offbox_none).
  k2 hostchat mirror dir <path> | none [--json]
         Sew's keyring mirror (restarts Sew). Never the K2 vault or ~/.k2.
App Spaces (members: the App's users who also have chat; never Connect owners/admins by role):
  k2 hostchat app-spaces [--json]
  k2 hostchat app-space bind <app> <space-id> [--json]      (<app>: its id or name, k2 app list)
  k2 hostchat app-space unbind <app> <space-id> [--remove-app-members] [--json]
          everyone stays by default (K2 stops managing the Space).
          --remove-app-members: the members K2 added there leave; people added by hand never do.
          On Sew 0.5.0+ (k2 hostchat status shows the version) the Space becomes read-only: nobody
          can post; history and export still work. On an older Sew it stays writable until Sew is
          upgraded. Removing an App does the same to its Spaces. app-space bind with the same App
          makes it writable again; another App can bind it only after the Sew install owner
          releases it in Sew's own admin (until then that bind is refused: space_bound_elsewhere).
  k2 hostchat app-space governance <app> <space-id> app|sew [--json]
          who governs a bound Space (Sew 0.6.0). app: the App decides who is in which private
          channel and at what level (its manifest's chat block); people can't change the Space's
          structure. Needs the App's chat key (k2 app grant <app> --kind chat-key). Box users
          (the owner, admins) in the Space leave first: K2 removes them itself, then switches (k2 app
          chat <app> shows them leaving); their messages stay. Without the chat key it is refused
          (chat_key_required); on a Sew older than 0.6.0, sew_feature_missing. sew: back to Sew's
          own rules. The owner or an admin, in person (Touch ID or the vault password), from a
          terminal outside K2. An agent is refused (person_only); with permission cards on it asks
          on a card: k2 permission request sew.space-governance app=<app> space=<space-id> to=app|sew.
          The <app> must be the one the Space is bound to. app-spaces shows each Space's governance.
  k2 hostchat app-space recording <app> <space-id> [--start-level L|inherit] [--required on|off|inherit] [--json]
          a bound Space's own recording rule (Sew 0.9.0); no flags: show it.
  k2 hostchat app-space recording-lock <space-id> on|off [--json]
          the owner's no-recording lock for a Space that holds CUI or FCI (in person, both ways).
Recording in Apps (Sew 0.9.0; all off by default, and each person still consents):
  k2 hostchat recording policy [--json]                     the server's recording policies, the locks
  k2 hostchat recording policy set KEY VALUE [--json]       change one (the owner or an admin, in person)
          Every key and who may change what: k2 hostchat recording --help. An App's own settings:
          k2 app recording <app>. Recordings are never deleted by uninstalling an App.
Home client (each App session sees only its own App's Spaces once Sew has app_scoped_sessions):
  k2 hostchat home [--json]                     the settings, Sew's feature flags, whether Sew has them
  k2 hostchat home client <app>|none            the App whose sessions see every Space (Hearth).
                                                The owner only (owner token, Owner login or a K2 shell
                                                tab); never an admin or an agent, even with Manage Chat / Voice Hosting
  k2 hostchat home app <app> --hidden on|off    "App-only": its Spaces never show in the home client
                                                (on: Manage Chat / Voice Hosting; off: the owner or an admin, never an agent)
          The human output says "app-scoped sessions yes|no", "app origin yes|no", "every App
          origin yes|no" (Sew's flags app_scoped_sessions, app_origin, app_origins): k2 study app-chat.
Chat-pass signing keys (K2 signs every chat pass; Sew checks it against K2's published keys):
  k2 hostchat pass-keys [--json]                each key: kid, state (next, current, retiring), when
  k2 hostchat pass-keys rotate [--json]         switch to the next key now (it must have been published
                                                10+ minutes, else lead_too_short). Sessions keep working.
  k2 hostchat pass-keys drop-previous [--json]  stop publishing the retiring key now (passes it signed
                                                stop working; sessions already open keep working)
          A pass is the short-lived ticket (60 s) an App's server trades for a chat session; a
          session is the open chat connection. lead_too_short: wait until `pass-keys` shows the
          next key published 10+ minutes ago, then rotate again. If you think passes are being
          forged: rotate, then drop-previous. The owner or an Admin. K2 rotates on its own every
          30 days. There is one set of keys for the box, never one per App: an App's chat key
          (k2 app grant <app> --kind chat-key) is a permission, not a key, and revoking it ends
          that App's sessions instead.
Person only (outside K2, with Touch ID / your login / the vault password):
  k2 hostchat purge --confirm-hostname HOST [--keep-backups]    (the owner; HOST = purgeConfirmHostname
                                            in status: Sew's public host, else the box hostname)
  k2 hostchat recovery-key                  (the owner only; shows Sew's launch key, the vault's
                                             sew-launch-key: the same as k2 vault reveal sew-launch-key
                                             --scope engine:sew, also in Settings → Vault → K2's own
                                             secrets. Admins, members and agents are refused)
  k2 hostchat rotate-master --drop-prev     (the owner or an admin)
  k2 hostchat open [--print]                (the owner)
         a one-time 60 s link to Sew's web app as its install owner. You
         confirm first; --print prints the link instead of opening it.
         An agent with Manage Chat / Voice Hosting (chat_manage) asks the owner on a card instead:
           k2 permission request sew.owner-handoff --why "<why>"
         The owner presses Allow and confirms (Touch ID, the vault password or
         their Connect password) in K2 on the box's own computer or its LAN
         (never through the tunnel: Connect or a phone is refused and the card
         stays open); K2 then opens Sew as its owner right there (a one-time
         link, 60 s from Allow).
         The agent never sees the link and has nothing to run afterwards.
         Needs k2 hostchat app-url set first. Promoting someone to install
         owner or admin in Sew lifts their Sew bans.

Voice (sew-media, TURN over TLS on 5349, WebRTC DTLS-SRTP):
  k2 hostchat media preflight [--json]
  k2 hostchat media install [--url wss://HOST] [--node-ip IP] [--tarball FILE] [--no-wait] [--json]
  k2 hostchat media url <wss://HOST>        change voice.public_url (an App origin that proxies /rtc, or
                                            voice.<box>): re-runs sew media install --reconfigure, restarts sew
  k2 hostchat media status [--json]
  k2 hostchat media renew                   renew the TURN certificate now
  k2 hostchat media rebind [--json]         voice through the tunnel only: stop the voice engine listening
                                            on the box's IPv6 addresses (IPv4 stays on 127.0.0.1, as the
                                            tunnel set it). Runs sew media install --reconfigure --restart,
                                            then checks sew doctor's media.tunnel_bind: it fails unless that
                                            check is ok, and the answer shows it (tunnelBind). Upgrading Sew
                                            alone doesn't fix a box set up before Sew 0.7.4, but on a tunnel
                                            box k2 hostchat upgrade runs this step itself (step tunnel-bind),
                                            so after an upgrade you don't need it. Use it when that step
                                            failed, or on a box already on Sew 0.7.4 or later. On an older
                                            Sew it is refused (sew_upgrade_needed) before anything changes:
                                            run k2 hostchat upgrade. Restarts sew-media once (calls in
                                            progress drop: tell the owner first). On the direct tier it
                                            changes nothing and exits 0 (changed: false).
  k2 hostchat media tier tunnel|direct [--json]
                                            (preview: k2 settings set voiceTunnelPreview true, or
                                            K2_VOICE_TUNNEL=1; direct also works with it off) tunnel: all voice through K2 Connect, at
                                            turn.<label>.k2.dev:443 (the relay's port, nothing to open
                                            here), on a second frpc. The box's IP leaves voice DNS and K2
                                            opens no voice port. Needs K2 Connect Pro, Sew 0.7.0 and root
                                            helper protocol 12: media preflight checks them.
                                            direct (the way back): public TCP 5349 + UDP 7882 at
                                            turn-<label>.k2.dev. The box's IP becomes public, in DNS, in
                                            certificate logs and to everyone in a call: tell the owner
                                            first. Both restart sew-media (calls in progress drop). With
                                            the preview on, media install uses tunnel.
  k2 hostchat media probe --udp ok|fail --tcp ok|fail --turn-tls ok|fail
                                            record the result of a real call made from outside the box
                                            (this tests nothing itself); a failed leg
                                            makes status degraded (k2_voice_probe_failed) until a
                                            passing probe; probes go stale after 30 days or a node
                                            IP / TURN tier change
  k2 hostchat media uninstall [--yes] [--json]
                                            remove voice (live calls end; chat keeps running). Asks
                                            on a terminal; with no terminal, --yes is required. A box
                                            where Sew's voice is already gone still has K2's voice
                                            records (TURN DNS, label, key) cleaned up

Recording (Sew, this box's chat and voice service: its recorder and transcriber; Sew 0.9.0+):
  k2 hostchat recording status [--json]     installed, on or off, versions, Sew's recording settings
                                            and whether this Sew can record (its app_recording flag)
  k2 hostchat recording preflight [--json]  what enable checks, changing nothing; each failed check
                                            names its fix
  k2 hostchat recording enable [--recorder-tarball FILE] [--transcriber-tarball FILE]
                               [--model-file FILE] [--no-wait] [--json]
                                            install the recorder, the transcriber and the speech
                                            model (when not installed yet), then turn recording on
                                            (Sew's own switch, recording.enabled). An Owner or Admin
                                            does it in person, with Touch ID on a Mac or the vault
                                            password. An agent never runs enable: it asks on a card
                                            (k2 permission request sew.recording-enable --why "...");
                                            the Owner or Admin who allows it there installs it, and
                                            the agent has nothing to run afterwards.
                                            Stops, changing nothing, unless: the box has at least
                                            8 GiB of RAM; voice is installed (k2 hostchat media
                                            install first); every Sew service fits in RAM with 40%
                                            kept for K2's own database; and /var/lib has the disk
                                            guard's reserve (10 GiB by default) free beyond the model
                                            and room for recordings. Run preflight to see which.
                                            K2 downloads the files from Sew's release and checks each
                                            against the sha256 built into K2; a file that doesn't
                                            match stops everything. On a box without internet the
                                            error names each file and its sha256: download them
                                            elsewhere and pass the FILE flags.
                                            With --model-file, put the model's licence files from
                                            the same release (asr-license-*.txt and
                                            sew-asr-models-NOTICE.txt) in the same folder: K2
                                            checks and installs them with the model.
                                            --no-wait returns while it installs (status shows how
                                            far it got).
  k2 hostchat recording disable [--json]    turn recording off (live recordings stop). The recorder
                                            stays installed and every recording is kept.
  k2 hostchat recording uninstall [--yes] [--json]
                                            remove the recorder, the transcriber and the model.
                                            Never deletes recordings: they stay in Sew. Asks on a
                                            terminal; with no terminal, --yes is required.
  Turning recording on doesn't record anyone: Apps and rooms choose who may record, and each
  person consents for themselves. On a Sew older than 0.9.0 every change is refused with
  sew_feature_missing (exit 3).

Meet (the reserved meet.<box>.k2.dev label):
  k2 hostchat meet bind <app-id> [--json]   point meet.<box>.k2.dev at an installed App (K2 Connect
                                            needed); restart the App once so K2_APP_ORIGINS has it
  k2 hostchat meet unbind [--json]

Users (standalone installs; setup links go to stdout only):
  k2 hostchat users list [--json]
  k2 hostchat users owner <name> [--display-name NAME]
  k2 hostchat users invite <name|email> [--display-name NAME] [--role member|admin|guest] [--email EMAIL]
  k2 hostchat users reset <name>            a new setup link (password reset)
  k2 hostchat users revoke <name> | reactivate <name>
  k2 hostchat users role <name> admin|member

Config:
  k2 hostchat config                        every key, its value and allowed values
  k2 hostchat config set approvals.by people|people+agents
  k2 hostchat config set default_space <space-slug>|none   (the Space whose access governs DMs;
                                            fixed to k5-admin while voice rooms are on: 409 default_space_fixed)
                                            who may approve queued posts (agents need Manage Chat / Voice Hosting; nobody
                                            approves their own post; agent approvals are labelled as an agent's)

People (K2 users who can use chat and voice here: member by default; a guest sees only channels they're invited to):
  k2 hostchat people list | people add <user> [--guest] | people set <user> --guest on|off | people remove <user>
  (Spaces add them separately. An App's users: k2 app user chat <user> member --app <app>, and its
   bound Spaces take them in; k2 study app-chat.)

Agents (alias of k2 chat access):
  k2 hostchat agents [list] [--space S] [--workspace W]
  k2 hostchat agents grant <space> <workspace> --level read|reply|send|manage [--approval required|none]
  k2 hostchat agents set <space> <workspace> [--level …] [--approval …] | agents revoke <space> <workspace>

Approvals (everyone's queued posts land here):
  k2 hostchat approvals [list] [--workspace W]
  k2 hostchat approvals approve <out_…> [<out_…>…] [--note "…"]   (a person confirms first)
  k2 hostchat approvals deny <out_…> --note "…"                   (the agent sees the note)

On someone's behalf (an assistant's post is its own and goes out at once; the person reviews it later):
  k2 hostchat on-behalf [list] [--state pending|approved|declined|all] [--limit N]
  k2 hostchat on-behalf approve <act_…|msg_…> [--note "…"]   (that person, with a confirm)
  k2 hostchat on-behalf decline <act_…|msg_…> [--note "…"]

  k2 hostchat log [--limit N]               who changed chat access and community settings

Deliveries (each chat agent's hub: what reaches its terminal):
  k2 hostchat deliveries [list] [<workspace>]
  k2 hostchat deliveries set <workspace> [--quiet-hours 22:00-07:00|off] [--tz Europe/Berlin|box] [--batch 10m|off]
  k2 hostchat deliveries reset <workspace>    prefs back to defaults (follows stay, their --batch goes)

Agents need "Manage Chat / Voice Hosting" on their workspace:
  k2 agent access set <workspace> chat_manage on   (the owner or an admin)
Using chat inside Spaces is k2 chat.
EOF
}

# Call /cli/sew/*: _hostchat_call METHOD PATH [payload] [tcp] → _CP_BODY.
_hostchat_call() {
    local raw
    raw="$(_compute_send "$@")"
    _compute_split "$raw"
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
}

# Print _CP_BODY: raw JSON with --json, else the verb's summary.
_hostchat_print() {
    local verb="$1" as_json="$2"
    if [ "$as_json" = "1" ]; then
        printf '%s\n' "$_CP_BODY"
        return 0
    fi
    printf '%s' "$_CP_BODY" | K2_HC_VERB="$verb" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
verb = os.environ["K2_HC_VERB"]
def line(k, x):
    if x is not None and x != "" and x != []:
        print("%-16s %s" % (k + ":", x))
if verb in ("status",):
    if v.get("released") is False and v.get("state") == "not-installed":
        print("Chat / Voice Hosting: " + (v.get("notice") or "not available in this K2 release yet")); sys.exit(0)
    if not v.get("supported", True):
        print("Chat / Voice Hosting: not supported here (Linux boxes only)"); sys.exit(0)
    line("Chat / Voice Hosting", v.get("state"))
    if v.get("released") is False:
        line("notice", v.get("notice"))
    line("identity", v.get("identityProvider"))
    line("version", v.get("installedVersion") or ("pin " + str(v.get("pinnedVersion"))))
    line("install id", v.get("installId"))
    line("public URL", v.get("publicUrl"))
    line("app URL", v.get("appUrl"))
    if v.get("owner") not in (None, "none"):
        line("owner in Sew", v.get("owner"))
    sh = (v.get("backup") or {}).get("ship") or {}
    if sh.get("kind") == "box-backup":
        line("off-box copy", (sh.get("boxBackup") or {}).get("line") or "the box backup (k2-backup)")
    elif sh.get("kind"):
        line("off-box copy", sh.get("path") if sh.get("kind") == "dir" else "none (backups stay on this box)")
        line("last copied", sh.get("lastShippedId"))
        line("copy error", sh.get("lastError"))
    mi = v.get("mirror") or {}
    if mi.get("kind"):
        line("key mirror", (mi.get("path") + (" (follows the off-box folder)" if mi.get("tied") else "")) if mi.get("kind") == "dir" else "none")
    tie = mi.get("tie") or {}
    if tie.get("state") == "pending":
        line("key mirror", "waiting: %s" % (tie.get("error") or "K2 retries on its own"))
    if v.get("recoveryKeyInVault"):
        lk = v.get("recoveryKeyVaultName") or "sew-launch-key"
        line("launch key", "in the K2 vault as %s (the owner only); to see it: k2 vault reveal %s --scope engine:sew" % (lk, lk))
    mo = v.get("moveOffK2") or {}
    if mo.get("line"):
        line("move off K2", mo.get("line"))
    au = v.get("autoSetup") or {}
    if au.get("line"):
        line("set up", au.get("line"))
    mt = v.get("meet") or {}
    if mt.get("app"):
        line("meet", "%s → App %s" % (mt.get("url"), mt.get("app")))
    if v.get("upgradeAvailable"):
        line("upgrade", "available: k2 hostchat upgrade")
    line("alarms", ", ".join(v.get("alarms") or []))
    for w in v.get("warnings") or []:
        line("warning", w.get("message") or w.get("code"))
    line("last error", v.get("lastError"))
    rc = v.get("recording") or {}
    if rc.get("installed"):
        line("recording", "%s (recorder %s)" % ("on" if rc.get("enabled") else "off, recorder kept", rc.get("version") or "?"))
    vo = v.get("voice") or {}
    if vo:
        line("voice", vo.get("state"))
        line("voice URL", vo.get("publicUrl"))
        line("TURN", "%s:5349 (node %s)" % (vo.get("turnHost"), vo.get("nodeIp")) if vo.get("turnHost") else None)
        c = vo.get("cert") or {}
        if c.get("daysLeft") is not None:
            line("TURN cert", "%s days left" % c.get("daysLeft"))
        t = vo.get("turnTls") or {}
        line("TURN-TLS check", t.get("served"))
        p = vo.get("progress") or {}
        if p.get("result") in ("running", "error"):
            line("voice step", "%s (%s)" % (p.get("step"), p.get("result")))
        line("voice error", vo.get("lastError"))
        ex = vo.get("exposure") or {}
        if ex.get("note"):
            line("exposure", ex.get("note"))
elif verb == "recording-status":
    line("recording", ("on" if v.get("enabled") else "off") if v.get("installed") else "not installed")
    ver = v.get("version") or {}
    line("recorder", ver.get("recorder"))
    line("transcriber", ver.get("transcriber"))
    line("model", v.get("model"))
    pol = v.get("policies") or {}
    line("policies", ", ".join("%s=%s" % (k, json.dumps(pol[k])) for k in sorted(pol)))
    fl = v.get("flags") or {}
    line("Sew flags", ", ".join(k for k in sorted(fl) if fl[k]) or "none of the Sew 0.9.0 recording flags")
    p = v.get("progress") or {}
    if p.get("result") in ("running", "error"):
        line("step", "%s (%s)" % (p.get("step"), p.get("result")))
    line("last error", v.get("lastError"))
    r = v.get("ready") or {}
    if r.get("ok") is False:
        line("not ready", "%s: %s" % (r.get("code"), r.get("hint")))
elif verb in ("recording-enable", "recording-disable", "recording-uninstall"):
    if verb == "recording-enable":
        line("recording", "installing (k2 hostchat recording status)" if v.get("state") == "installing" else "on")
        line("version", v.get("version"))
        line("model", v.get("model"))
        line("next", v.get("next"))
    else:
        line("recording", "off" if v.get("installed") else "removed")
        line("kept", v.get("kept"))
        for w in v.get("warnings") or []:
            line("warning", w)
elif verb in ("preflight", "media-preflight", "recording-preflight"):
    print("ready" if v.get("ready") else "NOT ready")
    for c in v.get("checks") or []:
        print("  %-5s %-14s %s%s" % (c.get("status"), c.get("id"), c.get("detail"), ("  (" + c["hint"] + ")") if c.get("hint") else ""))
elif verb.startswith("users-") and verb != "users-list":
    line("user", v.get("username"))
    line("role", v.get("installRole"))
    if v.get("sessionsRevoked") is not None:
        line("sessions ended", v.get("sessionsRevoked"))
    sl = v.get("setupLink") or {}
    if sl.get("url"):
        print("Setup link (one use, until %s; share it privately):" % sl.get("expiresAt"))
        print(sl["url"])
elif verb == "users-list":
    rows = v.get("users") or []
    if not rows:
        print("(no users)")
    for u in rows:
        print("%-24s %-8s %-6s %s%s" % (u.get("username") or u.get("externalId"), u.get("installRole"), u.get("kind"),
              u.get("displayName") or "", "  (deactivated)" if u.get("deactivated") else ""))
elif verb == "app-spaces":
    rows = v.get("spaces") or []
    if not rows:
        print("(no App Spaces bound)")
    for r in rows:
        m = r.get("members") or {}
        gov = ""
        if r.get("governance"):
            conf = r.get("governanceConfirmed")
            gov = "; governed by: %s%s" % (r["governance"], "" if conf == r["governance"] else " (sending to Sew)")
        print("%-24s %-28s added %s, held %s, pending %s%s" % (r.get("appId"), r.get("spaceId"), m.get("added", 0), m.get("held", 0), m.get("pending", 0), gov))
    gone = v.get("unbound") or []
    if gone:
        print("Unbound Spaces (an App removed, or app-space unbind; Sew keeps each one read-only for its old App; history and export still work;")
        print("  to reuse one for another App, the Sew install owner releases it in Sew first):")
    for r in gone:
        state = "read-only in Sew" if r.get("doneAt") else "waiting: sent once Sew can unbind (Sew 0.5.0+)"
        print("  %-24s %-28s %s" % (r.get("appId"), r.get("spaceId"), state))
    if v.get("note"):
        print(v["note"], file=sys.stderr)
elif verb == "app-space-governance":
    print("Space %s: governed by %s (%s)" % (v.get("spaceId"), v.get("governance"), "changed" if v.get("changed") else "no change"))
    if v.get("hint"):
        print(v["hint"])
elif verb == "rollback":
    line("Chat / Voice Hosting", v.get("state"))
    line("version", v.get("version"))
    m = v.get("media") or {}
    if m.get("state"):
        # Sew 0.7.3+: Sew put its voice engine back itself (media.state).
        line("voice engine", m.get("state") + ((" (%s -> %s)" % (m.get("from"), m.get("to"))) if m.get("from") and m.get("to") else ""))
    elif m.get("result"):
        line("voice engine", "%s (sew-media %s)" % (m.get("result"), m.get("to")))
    if v.get("mediaCommand"):
        print("Voice engine NOT rolled back. Run this once on the box, as root:")
        print("  " + v["mediaCommand"])
        print("The rollback itself is done: do not run k2 hostchat rollback again.")
    elif v.get("mediaNote"):
        print(v["mediaNote"])
    for w in v.get("warnings") or []:
        line("warning", w)
    d = v.get("doctor") or {}
    if d:
        line("sew doctor", "voice engine ok" if d.get("ok") else "voice engine checks fail (see --json)")
    line("error", v.get("error"))
elif verb == "ship":
    sh = v.get("ship") or {}
    bb = sh.get("boxBackup")
    if bb is not None:
        line("off-box copy", bb.get("line"))
        line("covered by", bb.get("coveredBy"))
        line("timer", bb.get("timer"))
    else:
        line("off-box copy", sh.get("path") if sh.get("kind") == "dir" else "none (backups stay on this box)")
    mi = v.get("mirror") or {}
    line("key mirror", "%s%s" % (mi.get("state"), (" " + mi["path"]) if mi.get("path") else ""))
    if mi.get("error"):
        line("key mirror", "waiting: %s" % mi.get("error"))
    line("note", v.get("note"))
elif verb == "recovery-key":
    print(v.get("recoveryKey") or v.get("key") or "")
    if v.get("warning"):
        print(v["warning"], file=sys.stderr)
else:
    keep = {k: x for k, x in v.items() if k not in ("ok",)}
    for k, x in keep.items():
        line(k, x if not isinstance(x, (dict, list)) else json.dumps(x))
'
}

# Wait until GET /cli/sew/status leaves the installing step(s).
_hostchat_wait() {
    local voice="${1:-0}" as_json="${2:-0}" i=0 last=""
    while [ "$i" -lt 600 ]; do
        _hostchat_call GET /cli/sew/status ""
        local now
        now="$(printf '%s' "$_CP_BODY" | K2_HC_VOICE="$voice" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
st = v.get("state")
vo = v.get("voice") or {}
p = vo.get("progress") or {}
busy = st == "installing" or (os.environ["K2_HC_VOICE"] == "1" and p.get("result") == "running")
step = ((v.get("enableProgress") or {}).get("current")) if st == "installing" else p.get("step")
# 0.46.7: the release download, while it runs ("downloading Sew 0.5.0: 12.3 of 24.7 MB").
dl = ((v.get("enableProgress") or {}) if st == "installing" else p).get("download") or {}
if step == "stage" and dl.get("source") == "download" and dl.get("bytes") is not None:
    mb = lambda n: "%.1f" % (n / 1e6)
    step = "stage (downloading %s: %s%s MB)" % (dl.get("label") or "Sew", mb(dl["bytes"]), (" of " + mb(dl["total"])) if dl.get("total") else "")
elif step == "stage" and dl.get("source") == "cache":
    step = "stage (%s from the K2 download cache)" % (dl.get("label") or "Sew")
# Sew 0.3.1 health: wait through "upgrading"; never wait on "stopping".
h = v.get("apiHealth") or {}
if not busy and h.get("reason") == "upgrading":
    busy, step = True, "sew-upgrading"
print(("busy " if busy else "done ") + "%s/%s" % (st, step or "-"))
')"
        case "$now" in
            done*)
                [ "$as_json" = "1" ] || echo "${now#done }" | sed 's#/-$##' >&2
                _hostchat_print status "$as_json"
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
vo = v.get("voice") or {}
h = v.get("apiHealth") or {}
bad = v.get("state") == "error" or (vo.get("progress") or {}).get("result") == "error"
if h.get("reason") == "stopping":
    print("Sew is stopping (a stop, disable or restart is under way); not waiting. Check k2 hostchat status, or k2 hostchat restart.", file=sys.stderr)
    bad = True
sys.exit(3 if bad else 0)
'
                return $?
                ;;
        esac
        if [ "$now" != "$last" ] && [ "$as_json" != "1" ]; then
            echo "… ${now#busy }" >&2
            last="$now"
        fi
        sleep 2
        i=$((i + 1))
    done
    echo '{"error":"timeout","hint":"still installing after 20 minutes: k2 hostchat status"}' >&2
    exit 3
}

# Wait until GET /cli/sew/status leaves "upgrading", then print the result
# and its reason (upgradeProgress.lastError, else lastError). Exit 0 only on
# result "done"; anything else (refused, aborted, rolled_back, media_failed, tunnel_bind_failed,
# rollback_failed) exits 3 with the reason on stderr.
_hostchat_upgrade_wait() {
    local as_json="${1:-0}" i=0 shown=""
    while [ "$i" -lt 1800 ]; do
        _hostchat_call GET /cli/sew/status ""
        local now steps
        now="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v.get("upgradeProgress") or {}
busy = v.get("state") == "upgrading" or (p.get("result") is None and p.get("current") is not None)
print(("busy " if busy else "done ") + str(p.get("current") or "-"))
')"
        # Each step once, as it completes (upgradeProgress.steps, in time order).
        steps="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
p = json.load(sys.stdin).get("upgradeProgress") or {}
st = p.get("steps") or {}
# Execution order: seq from the daemon; an older daemon has only whole-second
# `at` (ties) and a key-sorted object, so ties fall back to the machine order.
order = ["stage", "upgrade", "health", "rollback", "media", "tunnel-bind"]
def key(kv):
    k, v = kv[0], kv[1] or {}
    return (v.get("seq") or 0, v.get("at") or 0, order.index(k) if k in order else len(order))
print(" ".join(k for k, _ in sorted(st.items(), key=key)))
')"
        if [ "$as_json" != "1" ]; then
            local s
            for s in $steps; do
                case " $shown " in *" $s "*) ;; *) echo "… $s: done" >&2; shown="$shown $s" ;; esac
            done
        fi
        case "$now" in
            done*)
                if [ "$as_json" = "1" ]; then
                    printf '%s\n' "$_CP_BODY"
                fi
                printf '%s' "$_CP_BODY" | K2_HC_JSON="$as_json" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
p = v.get("upgradeProgress") or {}
res = p.get("result")
why = p.get("lastError") or v.get("lastError")
quiet = os.environ["K2_HC_JSON"] == "1"
if res == "done":
    if not quiet:
        print("upgraded: %s -> %s" % (p.get("from"), p.get("to")))
    sys.exit(0)
if not why:
    why = "the daemon recorded no reason (a K2 bug: send the k2-daemon log)"
print(json.dumps({"error": "upgrade_" + str(res), "result": res, "from": p.get("from"), "to": p.get("to"),
                  "hint": why}), file=sys.stderr)
sys.exit(3)
'
                return $?
                ;;
        esac
        sleep 1
        i=$((i + 1))
    done
    echo '{"error":"timeout","hint":"still upgrading after 30 minutes: k2 hostchat status"}' >&2
    exit 3
}

# One top-level string field of _CP_BODY ("" when absent).
_hostchat_field() {
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; v=json.load(sys.stdin).get(sys.argv[1]); print(v if isinstance(v, str) else "")' "$1"
}

# The install id a person confirm is bound to (or `none`).
_hostchat_install_id() {
    _hostchat_call GET /cli/sew/status ""
    printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("installId") or "none")'
}

# k2 hostchat rollback --accept-data-loss (Rosson 2026-10-28): the
# emergency rollback across a schema change. In a K2 terminal (an agent),
# the daemon refuses with rollback_needs_person and its Ask-with line (the
# owner answers on a card). Outside K2: the plan, the words, the owner's
# confirm bound to this install and that bundle, then the rollback.
_hostchat_rollback_data_loss() {
    local media="$1" as_json="$2"
    if _compute_in_session; then
        _hostchat_call POST /cli/sew/server/rollback "$(_compute_json K2_CJ_acceptDataLoss_b=1)"
        return 0
    fi
    _hostchat_call POST /cli/sew/server/rollback "$(_compute_json K2_CJ_acceptDataLoss_b=1 K2_CJ_plan_b=1)"
    local plan="$_CP_BODY" bundle to from since
    bundle="$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("bundle") or "")')"
    to="$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("to") or "")')"
    from="$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("from") or "")')"
    since="$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("since") or "")')"
    [ -n "$bundle" ] || { echo '{"error":"no_rollback_point","hint":"K2 found no backup to roll back to (k2 hostchat backups)"}' >&2; exit 3; }
    {
        echo "Nothing has changed yet. Confirming will:"
        echo "  roll this server's chat (Sew) back from $from to $to, for everyone on it."
        echo "  Chat written since $since will be lost from the live chat; a backup of it is kept."
        echo "  K2 first backs up the chat on Sew $from, then rolls back to backup $bundle (taken just before the upgrade)."
        echo "  Chat is offline for a few minutes while it runs."
        echo "Backups are listed by k2 hostchat backups. To bring the newer chat back later:"
        echo "  k2 hostchat upgrade, then k2 hostchat restore <kept backup> --confirm."
    } >&2
    local id tok; id="$(_hostchat_install_id)"
    tok="$(_vault_confirm_token sew-rollback-data-loss "$id:$bundle" "Roll Sew back to $to: chat written since $since leaves the live chat (a backup of it is kept)")" || exit 3
    K2_CP_MAX_TIME=3600 _hostchat_call POST /cli/sew/server/rollback "$(_compute_json K2_CJ_acceptDataLoss_b=1 K2_CJ_bundleId="$bundle" \
        K2_CJ_confirmToken="$tok" K2_CJ_mediaTarballPath="$media")" tcp
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; fi
    printf '%s' "$_CP_BODY" | K2_HC_JSON="$as_json" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
r = v.get("result")
if os.environ.get("K2_HC_JSON") != "1":
    print(v.get("message") or r or json.dumps(v))
    for k, lbl in (("keptBackup", "kept backup"), ("bundle", "rolled back to"), ("version", "Sew now")):
        if v.get(k):
            print("%-16s %s" % (lbl + ":", v[k]))
    if v.get("doctorFails"):
        print("doctor fails:    " + ", ".join(v["doctorFails"]))
codes = {"rolled_back": 0}
if r not in codes:
    print(json.dumps({"error": r or "rollback_incomplete", "hint": v.get("message") or "see the answer above"}), file=sys.stderr)
    sys.exit(3)
'
}

_hostchat_person() {
    if _compute_in_session; then
        echo '{"error":"person_only","hint":"a person does this from a terminal outside K2 (Touch ID or the vault password), never an agent"}' >&2
        exit 3
    fi
}

cmd_hostchat() {
    if [ $# -eq 0 ]; then _hostchat_usage; return 0; fi
    local sub="$1"; shift
    case "$sub" in
        -h|--help|help) _hostchat_usage; return 0 ;;
        media) _hostchat_media "$@"; return ;;
        recording) _hostchat_recording "$@"; return ;;
        users|user) _hostchat_users "$@"; return ;;
        meet) _hostchat_meet "$@"; return ;;
        people|person) _hostchat_people "$@"; return ;;
        approvals|approval) _hostchat_approvals "$@"; return ;;
        on-behalf|behalf) _hostchat_on_behalf "$@"; return ;;
        act-as) echo '{"error":"usage","hint":"act-as is gone: assistants post on a person'"'"'s behalf at once and the person reviews it later: k2 hostchat on-behalf"}' >&2; exit 2 ;;
        log) _hostchat_log "$@"; return ;;
        deliveries|delivery) _hostchat_deliveries "$@"; return ;;
        app-space|app-spaces) _hostchat_app_space "$sub" "$@"; return ;;
        recording) _hostchat_recording "$@"; return ;;
        home) _hostchat_home "$@"; return ;;
        pass-keys) _hostchat_pass_keys "$@"; return ;;
        agents|agent)
            # Alias of `k2 chat access …` (no sub = the box-wide list).
            local _a="${1:-list}"; [ $# -gt 0 ] && shift
            case "$_a" in -*) set -- "$_a" "$@"; _a="list" ;; esac
            cmd_chat access "$_a" "$@"; return ;;
    esac
    local as_json=0 tarball="" recovery="" identity="" voice=0 voice_flag=0 voice_url="" media_tarball="" node_ip="" wait=1
    local health=0 dry=0 confirm=0 drop_grants=0 drop_prev=0 keep_backups=0 host="" public_url="" accept_binary="" print_only=0 accept_loss=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --tarball) tarball="${2:?--tarball FILE}"; shift 2 ;;
            --recovery-key-file) recovery="${2:?--recovery-key-file FILE}"; shift 2 ;;
            --identity) identity="${2:?--identity k2|standalone}"; shift 2 ;;
            --voice) voice=1; voice_flag=1; shift ;;
            --voice-url) voice=1; voice_flag=1; voice_url="${2:?--voice-url wss://HOST}"; shift 2 ;;
            --media-tarball) voice=1; media_tarball="${2:?--media-tarball FILE}"; shift 2 ;;
            --node-ip) node_ip="${2:?--node-ip IP}"; shift 2 ;;
            --no-wait) wait=0; shift ;;
            --health) health=1; shift ;;
            --dry-run) dry=1; shift ;;
            --confirm) confirm=1; shift ;;
            --drop-grants) drop_grants=1; shift ;;
            --drop-prev) drop_prev=1; shift ;;
            --accept-binary) accept_binary="${2:?--accept-binary SHA256}"; shift 2 ;;
            --accept-data-loss) accept_loss=1; shift ;;
            --keep-backups) keep_backups=1; shift ;;
            --confirm-hostname) host="${2:?--confirm-hostname HOST}"; shift 2 ;;
            --public-url) public_url="${2:?--public-url https://HOST}"; shift 2 ;;
            --print) print_only=1; shift ;;
            --setup-url)
                echo '{"error":"usage","hint":"Sew builds setup links as <public-url>/setup: k2 hostchat config --public-url https://HOST"}' >&2
                exit 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$identity" in ""|k2|standalone) ;; *) echo '{"error":"usage","hint":"--identity is k2 or standalone"}' >&2; exit 2 ;; esac
    # The voice flags ride `enable` only, and --media-tarball also rides
    # upgrade and rollback (Sew's upgrade contract: the voice engine moves
    # with the core). Anywhere else they were silently dropped. Refuse
    # before any request.
    if [ "$sub" != "enable" ] && { [ "$voice_flag" = "1" ] || [ -n "$node_ip" ]; }; then
        echo "{\"error\":\"usage\",\"hint\":\"--voice, --voice-url and --node-ip only apply to k2 hostchat enable. k2 hostchat $sub does not set up voice; to install or replace voice from a tarball: k2 hostchat media install --tarball FILE\"}" >&2
        exit 2
    fi
    if [ -n "$media_tarball" ]; then
        case "$sub" in
            enable|upgrade|rollback) ;;
            *)
                echo "{\"error\":\"usage\",\"hint\":\"--media-tarball only applies to k2 hostchat enable, upgrade and rollback. To install or replace voice from a tarball: k2 hostchat media install --tarball FILE\"}" >&2
                exit 2 ;;
        esac
    fi
    if [ "$accept_loss" = "1" ] && [ "$sub" != "rollback" ]; then
        echo '{"error":"usage","hint":"--accept-data-loss only applies to k2 hostchat rollback"}' >&2
        exit 2
    fi
    abs() { case "$1" in ""|/*) printf '%s' "$1" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }
    case "$sub" in
        status)
            local q=""; [ "$health" = "1" ] && q="health=1"
            _hostchat_call GET /cli/sew/status "$q"
            _hostchat_print status "$as_json" ;;
        preflight)
            local q=""; [ -n "$tarball" ] && q="tarballPath=$(urlencode "$(abs "$tarball")")"
            _hostchat_call GET /cli/sew/preflight "$q"
            _hostchat_print preflight "$as_json" ;;
        enable)
            if [ "$identity" = "standalone" ] && [ "$as_json" != "1" ]; then
                echo "Identity: standalone (Sew's own password logins). This is fixed for this install; switching later means a fresh install." >&2
            fi
            local vj=""
            if [ "$voice" = "1" ]; then
                vj="$(_compute_json K2_CJ_publicUrl="$voice_url" K2_CJ_nodeIp="$node_ip" K2_CJ_mediaTarballPath="$(abs "$media_tarball")")"
            fi
            _hostchat_call POST /cli/sew/server/enable "$(_compute_json K2_CJ_tarballPath="$(abs "$tarball")" \
                K2_CJ_recoveryKeyFile="$(abs "$recovery")" K2_CJ_identity="$identity" K2_CJ_voice_j="$vj")"
            if [ "$wait" = "1" ] && [ "$(_hostchat_field state)" = "installing" ]; then
                _hostchat_wait "$voice" "$as_json"
                return $?
            fi
            _hostchat_print enable "$as_json" ;;
        disable) _hostchat_call POST /cli/sew/server/disable "{}"; _hostchat_print disable "$as_json" ;;
        rollback)
            if [ "$accept_loss" = "1" ]; then
                _hostchat_rollback_data_loss "$(abs "$media_tarball")" "$as_json"
                return $?
            fi
            _hostchat_call POST /cli/sew/server/rollback "$(_compute_json K2_CJ_mediaTarballPath="$(abs "$media_tarball")")"
            _hostchat_print rollback "$as_json"
            # Sew 0.7.3: media.state passes through (.media.state); manual is
            # done (exit 0, Sew's command printed above), failed exits 3 with
            # code media_rollback_failed. Never rerun the rollback either way.
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if v.get("ok") is False:
    out = {"error": v.get("code") or "rollback_incomplete", "hint": v.get("error") or "see the answer above"}
    if v.get("mediaCommand"):
        out["command"] = v["mediaCommand"]
    print(json.dumps(out), file=sys.stderr)
    sys.exit(3)
' || return $? ;;
        doctor) _hostchat_call POST /cli/sew/server/doctor "{}"; _hostchat_print doctor "$as_json" ;;
        reconfigure) _hostchat_call POST /cli/sew/server/reconfigure "{}"; _hostchat_print reconfigure "$as_json" ;;
        restart) _hostchat_call POST /cli/sew/server/restart "{}"; _hostchat_print restart "$as_json" ;;
        backup) K2_CP_MAX_TIME=1900 _hostchat_call POST /cli/sew/server/backup "{}"; _hostchat_print backup "$as_json" ;;
        backups) _hostchat_call GET /cli/sew/backups ""; _hostchat_print backups "$as_json" ;;
        job)
            [ -n "${pos[0]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat job <id>"}' >&2; exit 2; }
            _hostchat_call GET /cli/sew/job "id=$(urlencode "${pos[0]}")"; _hostchat_print job "$as_json" ;;
        upgrade)
            local b; b="$(_compute_json K2_CJ_tarballPath="$(abs "$tarball")" K2_CJ_mediaTarballPath="$(abs "$media_tarball")" K2_CJ_dryRun_b="$dry")"
            if [ -n "$accept_binary" ]; then
                # 0.46.5 item 12: the owner accepts exactly this binary,
                # with a confirm bound to this install and that sha256.
                accept_binary="$(printf '%s' "$accept_binary" | tr 'A-F' 'a-f')"
                case "$accept_binary" in
                    *[!0-9a-f]*) echo '{"error":"usage","hint":"--accept-binary is the binary'"'"'s sha256: 64 hex characters (k2 hostchat upgrade names it when it refuses)"}' >&2; exit 2 ;;
                esac
                [ "${#accept_binary}" -eq 64 ] || { echo '{"error":"usage","hint":"--accept-binary is the binary'"'"'s sha256: 64 hex characters"}' >&2; exit 2; }
                _hostchat_person
                local id tok; id="$(_hostchat_install_id)"
                tok="$(_vault_confirm_token sew-accept-binary "$id:$accept_binary" "Upgrade Sew from a binary K2 doesn't know (sha256 $accept_binary)")" || exit 3
                b="$(_compute_json K2_CJ_tarballPath="$(abs "$tarball")" K2_CJ_mediaTarballPath="$(abs "$media_tarball")" K2_CJ_dryRun_b="$dry" \
                    K2_CJ_acceptBinary="$accept_binary" K2_CJ_confirmToken="$tok")"
                _hostchat_call POST /cli/sew/server/upgrade "$b" tcp
            else
                _hostchat_call POST /cli/sew/server/upgrade "$b"
            fi
            if [ "$wait" = "1" ] && [ "$(_hostchat_field state)" = "upgrading" ]; then
                _hostchat_upgrade_wait "$as_json"
                return $?
            fi
            _hostchat_print upgrade "$as_json" ;;
        restore)
            [ -n "${pos[0]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat restore <bundle-id> --confirm"}' >&2; exit 2; }
            local cb=0; [ "$confirm" = "1" ] && cb=1
            _hostchat_call POST /cli/sew/server/restore "$(_compute_json K2_CJ_bundleId="${pos[0]}" K2_CJ_confirm_b="$cb")"
            _hostchat_print restore "$as_json" ;;
        rotate-master)
            if [ "$drop_prev" = "1" ]; then
                _hostchat_person
                local id tok; id="$(_hostchat_install_id)"
                tok="$(_vault_confirm_token sew-drop-prev "$id" "Drop Sew's old master key")" || exit 3
                _hostchat_call POST /cli/sew/server/rotate-master "$(_compute_json K2_CJ_phase=drop-prev K2_CJ_confirmToken="$tok")" tcp
            else
                _hostchat_call POST /cli/sew/server/rotate-master "$(_compute_json K2_CJ_phase=start)"
            fi
            _hostchat_print rotate-master "$as_json" ;;
        recovery-key)
            _hostchat_person
            local id tok; id="$(_hostchat_install_id)"
            tok="$(_vault_confirm_token sew-recovery-key "$id" "Show Sew's launch key")" || exit 3
            _hostchat_call POST /cli/sew/server/recovery-key "$(_compute_json K2_CJ_confirmToken="$tok")" tcp
            _hostchat_print recovery-key "$as_json" ;;
        config)
            if [ "${pos[0]:-}" = "set" ]; then
                case "${pos[1]:-}" in
                    approvals.by|approvals_by)
                        case "${pos[2]:-}" in
                            people|people+agents|person_only|agents_allowed) ;;
                            *) echo '{"error":"usage","hint":"k2 hostchat config set approvals.by people|people+agents"}' >&2; exit 2 ;;
                        esac
                        _chat_call POST /cli/sew/approvals/policy/set "$(_compute_json K2_CJ_approvalsBy="${pos[2]}")"
                        _chat_print config "$as_json"
                        return ;;
                    default_space|default-space|chat.default_space)
                        [ -n "${pos[2]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat config set default_space <space-slug>|none"}' >&2; exit 2; }
                        _chat_call POST /cli/sew/approvals/policy/set "$(_compute_json K2_CJ_defaultSpace="${pos[2]}")"
                        _chat_print config "$as_json"
                        return ;;
                    server.public_url|public_url)
                        [ -n "${pos[2]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat config set server.public_url https://HOST"}' >&2; exit 2; }
                        public_url="${pos[2]}" ;;
                    *)
                        echo "{\"error\":\"usage\",\"hint\":\"settable keys: approvals.by people|people+agents, default_space <space-slug>|none, server.public_url https://HOST (k2 hostchat config lists them)\"}" >&2
                        exit 2 ;;
                esac
            fi
            if [ -z "$public_url" ]; then
                if [ -n "${pos[0]:-}" ]; then
                    echo '{"error":"usage","hint":"k2 hostchat config [set <key> <value>] | config --public-url https://HOST"}' >&2; exit 2
                fi
                _chat_call GET /cli/sew/approvals/policy ""
                if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
                printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("%-20s %-14s %s" % ("key", "value", "allowed"))
print("%-20s %-14s %s" % ("approvals.by", v.get("approvalsBy"), "|".join(v.get("allowed") or ["people", "people+agents"])))
print("%-20s %-14s %s" % ("default_space", v.get("defaultSpace") or "(none)", "a Space slug: its access governs DMs outside every Space; none clears (fixed to k5-admin while voice rooms are on)"))
print("%-20s %-14s %s" % ("server.public_url", "(see status)", "https://HOST on this box (k2 hostchat config --public-url)"))
'
                return 0
            fi
            _hostchat_call POST /cli/sew/server/config "$(_compute_json K2_CJ_publicUrl="$public_url")"
            _hostchat_print config "$as_json" ;;
        uninstall)
            local dg=0; [ "$drop_grants" = "1" ] && dg=1
            _hostchat_call POST /cli/sew/server/uninstall "$(_compute_json K2_CJ_dropGrants_b="$dg")"
            _hostchat_print uninstall "$as_json" ;;
        app-url)
            case "${pos[0]:-show}" in
                show) _hostchat_call GET /cli/sew/status ""
                    if [ "$as_json" = "1" ]; then
                        printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps({"appUrl": json.load(sys.stdin).get("appUrl")}))'
                    else
                        local u; u="$(_hostchat_field appUrl)"; echo "${u:-(not set)}"
                    fi ;;
                set)
                    [ -n "${pos[1]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat app-url set https://HOST[/path]"}' >&2; exit 2; }
                    _hostchat_call POST /cli/sew/server/app-url "$(_compute_json K2_CJ_url="${pos[1]}")"
                    _hostchat_print app-url "$as_json" ;;
                clear)
                    _hostchat_call POST /cli/sew/server/app-url '{"url":""}'
                    _hostchat_print app-url "$as_json" ;;
                *) echo '{"error":"usage","hint":"k2 hostchat app-url set <url> | clear | show"}' >&2; exit 2 ;;
            esac ;;
        ship|mirror)
            local route="/cli/sew/server/$sub" kinds="dir <path> | none"
            [ "$sub" = "ship" ] && kinds="dir <path> | box-backup | none"
            case "${pos[0]:-}" in
                none) _hostchat_call POST "$route" '{"kind":"none"}' ;;
                dir)
                    [ -n "${pos[1]:-}" ] || { echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat $sub dir <path> (a folder on another disk)\"}" >&2; exit 2; }
                    _hostchat_call POST "$route" "$(_compute_json K2_CJ_kind=dir K2_CJ_path="$(abs "${pos[1]}")")" ;;
                box-backup)
                    [ "$sub" = "ship" ] || { echo '{"error":"usage","hint":"box-backup is for k2 hostchat ship; the keyring mirror is k2 hostchat mirror dir <path> | none"}' >&2; exit 2; }
                    _hostchat_call POST "$route" '{"kind":"box-backup"}' ;;
                *) echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat $sub $kinds\"}" >&2; exit 2 ;;
            esac
            _hostchat_print "$sub" "$as_json" ;;
        open)
            _hostchat_person
            _hostchat_call GET /cli/sew/status ""
            local app_url; app_url="$(_hostchat_field appUrl)"
            [ -n "$app_url" ] || { echo '{"error":"app_url_unset","hint":"set where the Sew web app lives first: k2 hostchat app-url set https://HOST"}' >&2; exit 3; }
            local tok; tok="$(_vault_confirm_token chat-owner-handoff "" "Open Sew as its owner")" || exit 3
            _hostchat_call POST /cli/sew/owner-handoff "$(_compute_json K2_CJ_confirmToken="$tok")" tcp
            tok=""
            local link
            link="$(printf '%s' "$_CP_BODY" | K2_HC_APP="$app_url" python3 -c '
import json, os, sys
code = json.load(sys.stdin).get("code") or ""
if not code.startswith("k2sho_"):
    sys.exit(3)
# The code rides in the fragment: never in a server log or a Referer.
print(os.environ["K2_HC_APP"].rstrip("/") + "/k2/owner-handoff#code=" + code)
')" || { echo '{"error":"no_code","hint":"K2 did not return a handoff code"}' >&2; exit 3; }
            if [ "$print_only" = "1" ]; then
                printf '%s\n' "$link"
            elif [ "$(uname -s)" = "Darwin" ] && command -v open >/dev/null 2>&1; then
                open "$link" || exit 3
                echo "Opened Sew as its owner (the link works once, for 60 s)." >&2
            elif command -v xdg-open >/dev/null 2>&1; then
                xdg-open "$link" >/dev/null 2>&1 || exit 3
                echo "Opened Sew as its owner (the link works once, for 60 s)." >&2
            else
                echo '{"error":"no_opener","hint":"no browser opener here: k2 hostchat open --print, then open the link within 60 s"}' >&2
                exit 3
            fi
            link="" ;;
        purge)
            [ -n "$host" ] || { echo '{"error":"usage","hint":"k2 hostchat purge --confirm-hostname HOST (k2 hostchat status shows it)"}' >&2; exit 2; }
            _hostchat_person
            local id tok kb=0; id="$(_hostchat_install_id)"; [ "$keep_backups" = "1" ] && kb=1
            tok="$(_vault_confirm_token sew-purge "$id" "Delete all chat and voice data on this box")" || exit 3
            _hostchat_call POST /cli/sew/server/uninstall "$(_compute_json K2_CJ_purge_b=1 K2_CJ_confirmHostname="$host" \
                K2_CJ_keepBackups_b="$kb" K2_CJ_confirmToken="$tok")" tcp
            _hostchat_print purge "$as_json" ;;
        *)
            echo "{\"error\":\"usage\",\"hint\":\"unknown hostchat command '$sub' (k2 hostchat --help)\"}" >&2
            exit 2 ;;
    esac
}

# k2 hostchat app-spaces | app-space bind|unbind <app-id> <space-id> (K31):
# which Sew Space belongs to which App. Members come only from that App's
# user grants; Admin Access (Connect owners/admins) never adds anyone.
_hostchat_app_space() {
    local noun="$1"; shift
    local sub="${1:-}"
    case "$sub" in
        recording) shift; _hostchat_app_space_recording "$@"; return ;;
        recording-lock) shift; _hostchat_app_space_lock "$@"; return ;;
        bind|unbind|list|governance) shift ;;
        *) sub="list" ;;
    esac
    local as_json=0 remove="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --remove-app-members) remove=1; shift ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat app-space bind|unbind <app-id> <space-id> [--remove-app-members])\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        list)
            _hostchat_call GET /cli/sew/app-spaces ""
            _hostchat_print app-spaces "$as_json" ;;
        bind|unbind)
            [ -n "${pos[0]:-}" ] && [ -n "${pos[1]:-}" ] || { echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat $noun $sub <app-id> <space-id>\"}" >&2; exit 2; }
            if [ -n "$remove" ] && [ "$sub" != "unbind" ]; then
                echo '{"error":"usage","hint":"--remove-app-members is for unbind"}' >&2; exit 2
            fi
            local rm_b=""; [ "$sub" = "unbind" ] && [ -n "$remove" ] && rm_b=1
            _hostchat_call POST "/cli/sew/app-spaces/$sub" "$(_compute_json K2_CJ_app="${pos[0]}" K2_CJ_spaceId="${pos[1]}" K2_CJ_removeAppMembers_b="$rm_b")"
            _hostchat_print "app-space-$sub" "$as_json" ;;
        governance)
            # Sew 0.6.0: who governs a bound Space. The owner or an admin, in
            # person (a vault confirm); an agent gets person_only from the
            # daemon, with the card it asks on.
            [ -n "${pos[0]:-}" ] && [ -n "${pos[1]:-}" ] && [ -n "${pos[2]:-}" ] || { echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat app-space governance <app> <space-id> app|sew\"}" >&2; exit 2; }
            case "${pos[2]}" in
                app|sew) ;;
                *) echo '{"error":"usage","hint":"governance is app or sew (k2 hostchat app-space governance <app> <space-id> app|sew)"}' >&2; exit 2 ;;
            esac
            [ -z "$remove" ] || { echo '{"error":"usage","hint":"--remove-app-members is for unbind"}' >&2; exit 2; }
            local gtok="" gvia=""
            if ! _compute_in_session; then
                gtok="$(_vault_confirm_token chat-governance "${pos[0]}:${pos[1]}:${pos[2]}" "Space ${pos[1]} governed by: ${pos[2]}")" || exit 3
                gvia=tcp
            fi
            _hostchat_call POST /cli/sew/app-spaces/governance "$(_compute_json K2_CJ_app="${pos[0]}" K2_CJ_spaceId="${pos[1]}" K2_CJ_governance="${pos[2]}" K2_CJ_confirmToken="$gtok")" $gvia
            _hostchat_print app-space-governance "$as_json" ;;
    esac
}

# Recording in Apps (Sew 0.9.0, K2 0.46.11): a bound Space's own rule, the
# owner's CUI/FCI lock, the server's recording policies.
_hostchat_recording_usage() {
    cat <<'EOF'
Recording in Apps (needs Sew 0.9.0; every setting starts off, and each person in a call
still answers their own consent notice):
  k2 hostchat app-space recording <app> <space-id> [--json]
          the Space's own rule: its start level and required (inherit = the App's rule,
          k2 app recording <app>), and whether it is locked.
  k2 hostchat app-space recording <app> <space-id> [--start-level none|admin|moderator|member|inherit]
                                                   [--required on|off|inherit] [--json]
          change it (each flag optional). Down (start level none, required off, inherit):
          the owner, an admin, or the App's own agent (its "…and manage them end to end"
          switch; Manage Chat / Voice Hosting isn't needed). Up (a start level above
          none): the owner or an admin, in person. --required on: the owner only, in
          person. In person = from a terminal outside K2, with a confirm (Touch ID or the
          vault password); an agent is refused (person_only). A locked Space takes no
          change up (space_locked). Refused too: space_unbound (bound to no App) and
          app_mismatch (another App's Space).
  k2 hostchat app-space recording-lock <space-id> on|off [--json]
          the no-recording lock for a Space that holds CUI or FCI: on, nobody records
          there, whatever its App or the server's policies say, even after an unbind.
          The server's owner only, in person, both ways. An agent gets person_only (there
          is no card: ask your human in your Thread), an admin owner_only.
  k2 hostchat recording policy [--json]
          the server's recording policies: what K2 asked for, what Sew shows, whether
          Sew has it (delivered), the locked Spaces.
  k2 hostchat recording policy set KEY VALUE [KEY VALUE …] [--json]
          recording.enabled on|off           recordings at all on this server
          recording.dm_enabled on|off        recordings in direct messages
          recording.require_consent_to_join on|off
                                             someone who says no to the notice can't join
                                             (off: they join without being recorded)
          recording.required off|multi_person
                                             every call with two or more people is recorded
          recording.audio_max_days N         the longest any App keeps audio (1 to 3650)
          The owner or an admin, in person; recording.required the owner only.
An App's own settings (who starts recordings, keep days, required, its server's
transcript reads): k2 app recording <app> --help. Recordings themselves (holds,
exports): Sew's own admin, k2 hostchat open (the owner). Uninstalling an App never
deletes its recordings (k2 hostchat uninstall keeps them too; only purge deletes
chat data). More: k2 study app-recording.
Exit codes: 0 ok, 2 bad usage, 3 refused (sew_feature_missing: k2 hostchat upgrade).
EOF
}

# A level for a Space: none|admin|moderator|member, or inherit (null).
_rec_level_json() {
    case "$1" in
        "") ;;
        none|admin|moderator|member) python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1" ;;
        inherit) echo null ;;
        *) return 1 ;;
    esac
}

_hostchat_app_space_recording() {
    local as_json=0 level="" required="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --start-level) level="${2:-}"; shift 2 || { echo '{"error":"usage","hint":"--start-level none|admin|moderator|member|inherit"}' >&2; exit 2; } ;;
            --start-level=*) level="${1#*=}"; shift ;;
            --required) required="${2:-}"; shift 2 || { echo '{"error":"usage","hint":"--required on|off|inherit"}' >&2; exit 2; } ;;
            --required=*) required="${1#*=}"; shift ;;
            -h|--help) _hostchat_recording_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat app-space recording --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local app="${pos[0]:-}" space="${pos[1]:-}"
    if [ -z "$app" ] || [ -z "$space" ] || [ -n "${pos[2]:-}" ]; then
        echo '{"error":"usage","hint":"k2 hostchat app-space recording <app> <space-id> [--start-level none|admin|moderator|member|inherit] [--required on|off|inherit]"}' >&2; exit 2
    fi
    local level_j req_j
    level_j="$(_rec_level_json "$level")" || { echo '{"error":"usage","hint":"--start-level is none, admin, moderator, member or inherit"}' >&2; exit 2; }
    req_j="$(_rec_switch_json "$required" inherit)" || { echo '{"error":"usage","hint":"--required is on, off or inherit"}' >&2; exit 2; }
    if [ -z "$level" ] && [ -z "$required" ]; then
        # No change named: show the Space's rule from the App's view.
        _compute_call GET /cli/app/recording "app=$(urlencode "$app")"
        if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
        printf '%s' "$_CP_BODY" | K2_RC_SPACE="$space" python3 -c '
import json, os, sys
v = json.load(sys.stdin)
want = os.environ["K2_RC_SPACE"]
a = v.get("app") or {}
s = next((x for x in v.get("spaces") or [] if str(x.get("spaceId")) == want), None)
if s is None:
    print(json.dumps({"error": "space_unbound", "hint": "Space %s is not bound to %s (k2 hostchat app-spaces)" % (want, a.get("name"))}), file=sys.stderr)
    sys.exit(3)
req = s.get("required")
print("Space %s (App %s, governed by %s)" % (want, a.get("name"), s.get("governance") or "sew"))
if s.get("locked"):
    print("  LOCKED: no recording here (CUI/FCI). Only the server'"'"'s owner lifts it, in person.")
print("  start level  %s" % (s.get("startLevel") or "inherit (the App'"'"'s rule)"))
print("  required     %s" % ("inherit (the App'"'"'s rule)" if req is None else ("on" if req else "off")))'
        return 0
    fi
    local body
    body="$(_compute_json K2_CJ_app="$app" K2_CJ_spaceId="$space" K2_CJ_startLevel_j="$level_j" K2_CJ_required_j="$req_j")"
    _app_rt_post /cli/sew/app-spaces/recording "$body"
    if _rec_needs_confirm; then
        # The confirm is bound to <the App's service id>:<space id>.
        _compute_call GET /cli/app/recording "app=$(urlencode "$app")"
        local svc
        svc="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print((json.load(sys.stdin).get("app") or {}).get("serviceId") or "")')"
        [ -n "$svc" ] || { echo '{"error":"not_found","hint":"K2 found no App by that name (k2 app list)"}' >&2; exit 3; }
        _rec_send_confirmed /cli/sew/app-spaces/recording "$body" recording-space "$svc:$space" "Recording in Space $space"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
req = v.get("required")
print("Space %s (App %s): %s" % (v.get("spaceId"), v.get("app"), "saved" if v.get("changed") else "nothing changed (already so)"))
print("  start level  %s" % (v.get("startLevel") or "inherit"))
print("  required     %s" % ("inherit" if req is None else ("on" if req else "off")))
if v.get("locked"):
    print("  LOCKED: no recording here (CUI/FCI); the rule above waits until the owner lifts the lock")'
}

_hostchat_app_space_lock() {
    local as_json=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            -h|--help) _hostchat_recording_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat app-space recording-lock <space-id> on|off)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local space="${pos[0]:-}" lock_b=""
    case "${pos[1]:-}" in on|true|yes) lock_b=1 ;; off|false|no) lock_b=0 ;; esac
    if [ -z "$space" ] || [ -z "$lock_b" ] || [ -n "${pos[2]:-}" ]; then
        echo '{"error":"usage","hint":"k2 hostchat app-space recording-lock <space-id> on|off"}' >&2; exit 2
    fi
    local body
    body="$(_compute_json K2_CJ_spaceId="$space" K2_CJ_lock_b="$lock_b")"
    _app_rt_post /cli/sew/app-spaces/recording-lock "$body"
    if _rec_needs_confirm; then
        _rec_send_confirmed /cli/sew/app-spaces/recording-lock "$body" recording-lock "$space" "Recording lock ${pos[1]} for Space $space"
    fi
    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("Space %s: recording lock %s%s" % (v.get("spaceId"), "on" if v.get("locked") else "off", "" if v.get("changed") else " (already so)"))
if v.get("hint"):
    print("  " + v["hint"])'
}

# k2 hostchat recording <sub>: `policy` here. The recorder's own install
# (status, preflight, enable, disable, uninstall) is another build's.
_hostchat_recording() {
    local sub="${1:-}"
    case "$sub" in
        policy|policies) shift; _hostchat_recording_policy "$@" ;;
        -h|--help|help|"") _hostchat_recording_usage ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat recording $sub isn't in this build; here: k2 hostchat recording policy [set KEY VALUE] (k2 hostchat recording --help)\"}" >&2; exit 2 ;;
    esac
}

_hostchat_recording_policy() {
    local as_json=0 set_mode=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            -h|--help) _hostchat_recording_usage; return 0 ;;
            set) [ "$set_mode" = 0 ] && [ ${#pos[@]} -eq 0 ] && { set_mode=1; shift; continue; }; pos+=("$1"); shift ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat recording policy [set KEY VALUE])\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    if [ "$set_mode" = 0 ]; then
        [ ${#pos[@]} -eq 0 ] || { echo '{"error":"usage","hint":"k2 hostchat recording policy [--json] | k2 hostchat recording policy set KEY VALUE"}' >&2; exit 2; }
        _hostchat_call GET /cli/sew/recording/policies ""
    else
        local n=${#pos[@]}
        if [ "$n" -eq 0 ] || [ $((n % 2)) -ne 0 ]; then
            echo '{"error":"usage","hint":"k2 hostchat recording policy set KEY VALUE [KEY VALUE …] (k2 hostchat recording --help lists the keys)"}' >&2; exit 2
        fi
        local changes
        changes="$(python3 -c '
import json, sys
args = sys.argv[1:]
out = {}
bools = ("recording.enabled", "recording.dm_enabled", "recording.require_consent_to_join")
def bad(h):
    print(json.dumps({"error": "usage", "hint": h}), file=sys.stderr); sys.exit(2)
for k, v in zip(args[0::2], args[1::2]):
    if not k.startswith("recording."):
        k = "recording." + k
    if k in bools:
        if v in ("on", "true", "yes"): out[k] = True
        elif v in ("off", "false", "no"): out[k] = False
        else: bad("%s is on or off" % k)
    elif k == "recording.required":
        if v not in ("off", "multi_person"): bad("recording.required is off or multi_person")
        out[k] = v
    elif k == "recording.audio_max_days":
        d = v[:-1] if v.endswith("d") else v
        if not d.isdigit() or not 1 <= int(d) <= 3650: bad("recording.audio_max_days is a number of days, 1 to 3650")
        out[k] = int(d)
    else:
        bad("unknown policy %s: recording.enabled, recording.dm_enabled, recording.require_consent_to_join, recording.required, recording.audio_max_days" % k)
print(json.dumps({"changes": out}))
' "${pos[@]}")" || exit 2
        _app_rt_post /cli/sew/recording/policy "$changes"
        if _rec_needs_confirm; then
            local id; id="$(_hostchat_install_id)"
            _rec_send_confirmed /cli/sew/recording/policy "$changes" recording-policy "$id" "Change this server's recording policies"
        fi
        [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY"
    fi
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_RC_SET="$set_mode" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
show = lambda x: "-" if x is None else ("on" if x is True else "off" if x is False else str(x))
if os.environ.get("K2_RC_SET") == "1":
    ch = v.get("changed") or []
    print("saved: %s" % ", ".join(ch) if ch else "nothing changed (already so)")
f = v.get("flags")
if isinstance(f, dict):
    yn = lambda b: "yes" if b else "no"
    print("Sew flags: record in Apps %s · voice:record cap %s · required %s · server reads %s" % (
        yn(f.get("appRecording")), yn(f.get("voiceRecordCap")), yn(f.get("recordingRequired")), yn(f.get("mgmtRecordingReads"))))
    if not f.get("appRecording"):
        print("  This server'"'"'s chat can'"'"'t record in Apps yet (it needs Sew 0.9.0: k2 hostchat upgrade).")
print("%-36s %-14s %-14s %s" % ("policy", "K2 asked", "Sew shows", ""))
for p in v.get("policies") or []:
    note = ""
    if p.get("want") is not None:
        note = "delivered" if p.get("delivered") else "not yet delivered"
        if p.get("setBy"):
            note += " (by %s)" % p["setBy"]
    print("%-36s %-14s %-14s %s" % (p.get("key"), show(p.get("want")), show(p.get("seen")), note))
locks = v.get("locks")
if isinstance(locks, list):
    if not locks:
        print("locked Spaces: none")
    for l in locks:
        print("locked Space %s (no recording, CUI/FCI) by %s, %s" % (l.get("spaceId"), l.get("lockedBy"), time.strftime("%Y-%m-%d %H:%M", time.localtime(l.get("lockedAt") or 0))))
if v.get("k5SpaceId"):
    print("K5 rooms Space: %s (k2 voice settings)" % v["k5SpaceId"])'
}

# k2 hostchat home [client <app-id>|none] | home app <app-id> --hidden on|off
# (Sew decision 13): the install's home client (Hearth sees every Space a
# person is in) and an App's "App-only" mark. Delivered by the grant-hook
# sweep once Sew reports the home_client feature flag.
# App-governed Spaces S6 (0.46.7): the chat-pass signing keys, on the
# existing routes (GET /cli/sew/keys, POST /cli/sew/keys/{rotate,drop-previous}).
# The emergency wipe stays off the CLI (a person, with a confirm).
_hostchat_pass_keys() {
    local sub="show" as_json=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            -h|--help) _hostchat_usage; return 0 ;;
            rotate|drop-previous) sub="$1"; shift ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat pass-keys [rotate|drop-previous] [--json])\"}" >&2; exit 2 ;;
            *) echo '{"error":"usage","hint":"k2 hostchat pass-keys [rotate|drop-previous] [--json]"}' >&2; exit 2 ;;
        esac
    done
    case "$sub" in
        show) _hostchat_call GET /cli/sew/keys "" ;;
        rotate) _hostchat_call POST /cli/sew/keys/rotate '{}' ;;
        drop-previous) _hostchat_call POST /cli/sew/keys/drop-previous '{}' ;;
    esac
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | K2_PK_SUB="$sub" python3 -c '
import json, os, sys, time
v = json.load(sys.stdin)
sub = os.environ["K2_PK_SUB"]
when = lambda t: time.strftime("%Y-%m-%d %H:%M", time.localtime(t)) if t else "-"
if sub == "rotate":
    print("rotated: the next key signs chat passes now")
elif sub == "drop-previous":
    print("dropped the retiring key")
keys = v.get("keys") or []
if not keys and sub == "show":
    print("no chat-pass keys yet (K2 makes them on the first chat pass)")
for k in keys:
    extra = ""
    if k.get("state") == "current" and k.get("rotateDueAt"):
        extra = "  rotates by %s" % when(k["rotateDueAt"])
    if k.get("retireAfter"):
        extra = "  retires after %s" % when(k["retireAfter"])
    print("%-9s %s  published %s%s" % (k.get("state"), k.get("kid"), when(k.get("publishedAt")), extra))'
}

_hostchat_home() {
    local sub="${1:-}"
    case "$sub" in
        client|app) shift ;;
        ""|-*) sub="show" ;;
        *) echo '{"error":"usage","hint":"k2 hostchat home [client <app-id>|none] | home app <app-id> --hidden on|off"}' >&2; exit 2 ;;
    esac
    local as_json=0 hidden="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --hidden) hidden="${2:-}"; shift 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat home --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        show) _hostchat_call GET /cli/sew/home "" ;;
        client)
            [ -n "${pos[0]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat home client <app-id>|none"}' >&2; exit 2; }
            if [ "${pos[0]}" = none ]; then
                _hostchat_call POST /cli/sew/home/client '{"app":null}'
            else
                _hostchat_call POST /cli/sew/home/client "$(_compute_json K2_CJ_app="${pos[0]}")"
            fi ;;
        app)
            local h_b
            case "$hidden" in on|true|yes) h_b=1 ;; off|false|no) h_b=0 ;; *) echo '{"error":"usage","hint":"--hidden on|off"}' >&2; exit 2 ;; esac
            [ -n "${pos[0]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat home app <app-id> --hidden on|off"}' >&2; exit 2; }
            _hostchat_call POST /cli/sew/home/app "$(_compute_json K2_CJ_app="${pos[0]}" K2_CJ_hidden_b="$h_b")" ;;
    esac
    if [ "$as_json" = "1" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
    printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
if not v.get("ok", True):
    print(json.dumps(v)); sys.exit(0)
sew = v.get("sew") or {}
yn = lambda b: "yes" if b else "no"
print("Sew flags:    app-scoped sessions %s · home client %s · app origin (voice for Apps) %s · every App origin %s · K2-managed avatar %s" % (
    yn(sew.get("appScopedSessions")), yn(sew.get("homeClient")), yn(sew.get("appOrigin")), yn(sew.get("appOrigins")), yn(sew.get("k2ManagedAvatar"))))
if not sew.get("appScopedSessions"):
    print("              not app-scoped yet: only the owner gives chat (k2 app user chat <user> member --app <app>),")
    print("              and an App can see all chat its people can")
hc = v.get("homeClient")
print("home client:  %s%s" % (hc or "none", "" if v.get("homeClientDelivered", True) else ("  (wanted, not set: %s)" % v["homeClientError"]) if v.get("homeClientError") else "  (sending)"))
for a in v.get("apps") or []:
    print("  %-38s %-20s %s%s%s" % (a.get("app"), a.get("name") or "", "App-only" if a.get("hiddenFromHome") else "shown in the home client",
        ("  origin " + a["origin"]) if a.get("origin") else "", "" if a.get("delivered") else "  (sending)"))'
}

# k2 hostchat meet bind <app-id> | unbind (K-M2).
_hostchat_meet() {
    local sub="${1:-}"; [ $# -gt 0 ] && shift
    local as_json=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat meet bind <app-id> | unbind)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$sub" in
        bind)
            [ -n "${pos[0]:-}" ] || { echo '{"error":"usage","hint":"k2 hostchat meet bind <app-id>"}' >&2; exit 2; }
            _hostchat_call POST /cli/sew/meet/bind "$(_compute_json K2_CJ_app="${pos[0]}")"
            _hostchat_print meet-bind "$as_json" ;;
        unbind)
            _hostchat_call POST /cli/sew/meet/unbind "{}"
            _hostchat_print meet-unbind "$as_json" ;;
        ""|-h|--help|help) _hostchat_usage ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"unknown meet command '$sub' (k2 hostchat meet bind <app-id> | unbind)\"}" >&2; exit 2 ;;
    esac
}

_hostchat_recording_usage() {
    cat <<'EOF'
k2 hostchat recording — Sew's call recorder on this box (Chat / Voice Hosting)

Recording (Sew, this box's chat and voice service: its recorder and transcriber; Sew 0.9.0+):
  k2 hostchat recording status [--json]     installed, on or off, versions, Sew's recording settings
                                            and whether this Sew can record (its app_recording flag)
  k2 hostchat recording preflight [--json]  what enable checks, changing nothing; each failed check
                                            names its fix
  k2 hostchat recording enable [--recorder-tarball FILE] [--transcriber-tarball FILE]
                               [--model-file FILE] [--no-wait] [--json]
                                            install the recorder, the transcriber and the speech
                                            model (when not installed yet), then turn recording on
                                            (Sew's own switch, recording.enabled). An Owner or Admin
                                            does it in person, with Touch ID on a Mac or the vault
                                            password. An agent never runs enable: it asks on a card
                                            (k2 permission request sew.recording-enable --why "...");
                                            the Owner or Admin who allows it there installs it, and
                                            the agent has nothing to run afterwards.
                                            Stops, changing nothing, unless: the box has at least
                                            8 GiB of RAM; voice is installed (k2 hostchat media
                                            install first); every Sew service fits in RAM with 40%
                                            kept for K2's own database; and /var/lib has the disk
                                            guard's reserve (10 GiB by default) free beyond the model
                                            and room for recordings. Run preflight to see which.
                                            K2 downloads the files from Sew's release and checks each
                                            against the sha256 built into K2; a file that doesn't
                                            match stops everything. On a box without internet the
                                            error names each file and its sha256: download them
                                            elsewhere and pass the FILE flags.
                                            With --model-file, put the model's licence files from
                                            the same release (asr-license-*.txt and
                                            sew-asr-models-NOTICE.txt) in the same folder: K2
                                            checks and installs them with the model.
                                            --no-wait returns while it installs (status shows how
                                            far it got).
  k2 hostchat recording disable [--json]    turn recording off (live recordings stop). The recorder
                                            stays installed and every recording is kept.
  k2 hostchat recording uninstall [--yes] [--json]
                                            remove the recorder, the transcriber and the model.
                                            Never deletes recordings: they stay in Sew. Asks on a
                                            terminal; with no terminal, --yes is required.
  Turning recording on doesn't record anyone: Apps and rooms choose who may record, and each
  person consents for themselves. On a Sew older than 0.9.0 every change is refused with
  sew_feature_missing (exit 3).
EOF
}

# k2 hostchat recording (0.46.11 K-a): Sew's recorder install. Enable is an
# Owner's or Admin's, in person (a vault confirm bound to this install); in
# a K2 terminal the daemon answers person_only with the card's Ask-with line.
_hostchat_recording() {
    local sub="${1:-}"; [ $# -gt 0 ] && shift
    local as_json=0 yes=0 wait=1 rec="" tra="" model=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --yes|-y)
                [ "$sub" = "uninstall" ] || { echo "{\"error\":\"usage\",\"hint\":\"$1 only applies to k2 hostchat recording uninstall\"}" >&2; exit 2; }
                yes=1; shift ;;
            --no-wait) wait=0; shift ;;
            --recorder-tarball|--transcriber-tarball|--model-file)
                [ "$sub" = "enable" ] || { echo "{\"error\":\"usage\",\"hint\":\"$1 only applies to k2 hostchat recording enable\"}" >&2; exit 2; }
                [ $# -ge 2 ] || { echo "{\"error\":\"usage\",\"hint\":\"$1 FILE\"}" >&2; exit 2; }
                local f="$2"; case "$f" in /*) ;; *) f="$PWD/$f" ;; esac
                case "$1" in
                    --recorder-tarball) rec="$f" ;;
                    --transcriber-tarball) tra="$f" ;;
                    *) model="$f" ;;
                esac
                shift 2 ;;
            -h|--help) _hostchat_recording_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat recording --help)\"}" >&2; exit 2 ;;
            *) echo "{\"error\":\"usage\",\"hint\":\"unexpected '$1' (k2 hostchat recording --help)\"}" >&2; exit 2 ;;
        esac
    done
    case "$sub" in
        status) _hostchat_call GET /cli/sew/recording ""; _hostchat_print recording-status "$as_json" ;;
        preflight) _hostchat_call GET /cli/sew/recording/preflight ""; _hostchat_print recording-preflight "$as_json" ;;
        enable)
            if _compute_in_session; then
                # An agent: the daemon refuses (person_only) with the card's line.
                _hostchat_call POST /cli/sew/recording/enable '{}'
                _hostchat_print recording-enable "$as_json"
                return 0
            fi
            # Refuse before asking for Touch ID when the box can't do it.
            _hostchat_call GET /cli/sew/recording ""
            local notready
            notready="$(printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
r = (json.load(sys.stdin).get("ready") or {})
if r.get("ok") is False:
    print(json.dumps({"error": r.get("code") or "not_ready", "hint": r.get("hint") or ""}))
')"
            if [ -n "$notready" ]; then echo "$notready" >&2; exit 3; fi
            local id tok
            id="$(_hostchat_install_id)"
            tok="$(_vault_confirm_token sew-recording-enable "$id" "Install Sew's recorder and turn recording on")" || exit 3
            _hostchat_call POST /cli/sew/recording/enable "$(_compute_json K2_CJ_confirmToken="$tok" K2_CJ_recorderTarballPath="$rec" \
                K2_CJ_transcriberTarballPath="$tra" K2_CJ_modelFilePath="$model")" tcp
            if [ "$wait" = "1" ] && [ "$(_hostchat_field state)" = "installing" ]; then
                local i=0 res=""
                while [ "$i" -lt 900 ]; do
                    _hostchat_call GET /cli/sew/recording ""
                    res="$(printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; p=json.load(sys.stdin).get("progress") or {}; print(p.get("result") or "")')"
                    [ "$res" = "running" ] || break
                    sleep 2; i=$((i + 1))
                done
                _hostchat_print recording-status "$as_json"
                [ "$res" = "done" ] || exit 3
                return 0
            fi
            _hostchat_print recording-enable "$as_json" ;;
        disable) _hostchat_call POST /cli/sew/recording/disable '{}'; _hostchat_print recording-disable "$as_json" ;;
        uninstall)
            if [ "$yes" != "1" ]; then
                if [ ! -t 0 ]; then
                    echo '{"error":"confirm_required","hint":"Remove the recorder? Live recordings stop; every recording is kept. There is no terminal to ask on: pass --yes (k2 hostchat recording uninstall --yes)"}' >&2
                    exit 2
                fi
                printf 'Remove the recorder? Live recordings stop; every recording is kept. [y/N] ' >&2
                local ans; IFS= read -r ans || ans=""
                case "$ans" in
                    y|Y|yes|YES|Yes) ;;
                    *) echo "Recorder not removed." >&2; exit 1 ;;
                esac
            fi
            _hostchat_call POST /cli/sew/recording/uninstall '{"confirm": true}'
            _hostchat_print recording-uninstall "$as_json" ;;
        ""|-h|--help|help) _hostchat_recording_usage ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"unknown recording command '$sub' (k2 hostchat recording --help)\"}" >&2; exit 2 ;;
    esac
}

_hostchat_media() {
    local sub="${1:-}"; [ $# -gt 0 ] && shift
    local as_json=0 url="" node_ip="" tarball="" wait=1 udp="" tcp="" tls="" yes=0 pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --yes|-y)
                [ "$sub" = "uninstall" ] || { echo "{\"error\":\"usage\",\"hint\":\"$1 only applies to k2 hostchat media uninstall\"}" >&2; exit 2; }
                yes=1; shift ;;
            --url|--voice-url|--public-url) url="${2:?--url wss://HOST}"; shift 2 ;;
            --node-ip) node_ip="${2:?--node-ip IP}"; shift 2 ;;
            --tarball|--media-tarball) tarball="${2:?--tarball FILE}"; shift 2 ;;
            --no-wait) wait=0; shift ;;
            --udp) udp="${2:?--udp ok|fail}"; shift 2 ;;
            --tcp) tcp="${2:?--tcp ok|fail}"; shift 2 ;;
            --turn-tls) tls="${2:?--turn-tls ok|fail}"; shift 2 ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    case "$tarball" in ""|/*) ;; *) tarball="$PWD/$tarball" ;; esac
    case "$sub" in
        preflight) _hostchat_call GET /cli/sew/media/preflight ""; _hostchat_print media-preflight "$as_json" ;;
        status) _hostchat_call GET /cli/sew/status "health=1"
            if [ "$as_json" = "1" ]; then
                printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("voice")))'
            else
                _hostchat_print status 0
            fi ;;
        install)
            _hostchat_call POST /cli/sew/media/install "$(_compute_json K2_CJ_publicUrl="$url" K2_CJ_nodeIp="$node_ip" K2_CJ_mediaTarballPath="$tarball")"
            if [ "$wait" = "1" ] && [ "$(_hostchat_field voice)" = "installing" ]; then
                _hostchat_wait 1 "$as_json"
                return $?
            fi
            _hostchat_print media-install "$as_json" ;;
        url)
            local u="${pos[0]:-$url}"
            [ -n "$u" ] || { echo '{"error":"usage","hint":"k2 hostchat media url wss://HOST"}' >&2; exit 2; }
            _hostchat_call POST /cli/sew/media/url "$(_compute_json K2_CJ_publicUrl="$u")"
            _hostchat_print media-url "$as_json" ;;
        renew) _hostchat_call POST /cli/sew/media/renew "{}"; _hostchat_print media-renew "$as_json" ;;
        rebind) _hostchat_call POST /cli/sew/media/rebind "{}"; _hostchat_print media-rebind "$as_json" ;;
        tier)
            # 0.46.8 voice rooms (preview): voice through the tunnel or direct.
            local t="${pos[0]:-}"
            case "$t" in
                tunnel|direct) ;;
                *) echo '{"error":"usage","hint":"k2 hostchat media tier tunnel|direct (tunnel: relay-only TURN through K2 Connect, no public port; direct: public 5349/7882, the box IP in DNS)"}' >&2; exit 2 ;;
            esac
            _hostchat_call POST /cli/sew/media/tier "$(_compute_json K2_CJ_tier="$t")"
            _hostchat_print media-tier "$as_json" ;;
        probe)
            [ -n "$udp" ] && [ -n "$tcp" ] && [ -n "$tls" ] || { echo '{"error":"usage","hint":"k2 hostchat media probe --udp ok|fail --tcp ok|fail --turn-tls ok|fail"}' >&2; exit 2; }
            _hostchat_call POST /cli/sew/media/probe "$(_compute_json K2_CJ_udp="$udp" K2_CJ_tcp="$tcp" K2_CJ_turnTls="$tls")"
            _hostchat_print media-probe "$as_json" ;;
        uninstall)
            # Sew's question (live calls end): asked here, on a terminal;
            # --yes answers it. The daemon runs nothing without confirm.
            if [ "$yes" != "1" ]; then
                if [ ! -t 0 ]; then
                    echo '{"error":"confirm_required","hint":"Uninstall voice? Live calls end. There is no terminal to ask on: pass --yes (k2 hostchat media uninstall --yes)"}' >&2
                    exit 2
                fi
                printf 'Uninstall voice? Live calls end. [y/N] ' >&2
                local ans; IFS= read -r ans || ans=""
                case "$ans" in
                    y|Y|yes|YES|Yes) ;;
                    *) echo "Voice not removed." >&2; exit 1 ;;
                esac
            fi
            _hostchat_call POST /cli/sew/media/uninstall '{"confirm": true}'
            _hostchat_print media-uninstall "$as_json" ;;
        ""|-h|--help|help) _hostchat_usage ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"unknown media command '$sub' (k2 hostchat --help)\"}" >&2; exit 2 ;;
    esac
}

_hostchat_users() {
    local sub="${1:-}"; [ $# -gt 0 ] && shift
    local as_json=0 display="" role="" email="" pos=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --json) as_json=1; shift ;;
            --display-name) display="${2:?--display-name NAME}"; shift 2 ;;
            --role) role="${2:?--role member|admin|guest}"; shift 2 ;;
            --email) email="${2:?--email EMAIL}"; shift 2 ;;
            --admin) role="admin"; shift ;;
            -h|--help) _hostchat_usage; return 0 ;;
            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 hostchat --help)\"}" >&2; exit 2 ;;
            *) pos+=("$1"); shift ;;
        esac
    done
    local user="${pos[0]:-}"
    case "$sub" in
        list|"") _hostchat_call GET /cli/sew/users ""; _hostchat_print users-list "$as_json"; return ;;
        -h|--help|help) _hostchat_usage; return 0 ;;
    esac
    [ -n "$user" ] || { echo "{\"error\":\"usage\",\"hint\":\"k2 hostchat users $sub <name>\"}" >&2; exit 2; }
    case "$sub" in
        owner) _hostchat_call POST /cli/sew/users/owner "$(_compute_json K2_CJ_user="$user" K2_CJ_displayName="$display")" ;;
        invite) _hostchat_call POST /cli/sew/users/invite "$(_compute_json K2_CJ_user="$user" K2_CJ_displayName="$display" K2_CJ_role="${role:-member}" K2_CJ_email="$email")" ;;
        reset) _hostchat_call POST /cli/sew/users/reset "$(_compute_json K2_CJ_user="$user")" ;;
        revoke) _hostchat_call POST /cli/sew/users/revoke "$(_compute_json K2_CJ_user="$user")" ;;
        reactivate) _hostchat_call POST /cli/sew/users/reactivate "$(_compute_json K2_CJ_user="$user")" ;;
        role)
            local r="${pos[1]:-$role}"
            [ -n "$r" ] || { echo '{"error":"usage","hint":"k2 hostchat users role <name> admin|member"}' >&2; exit 2; }
            _hostchat_call POST /cli/sew/users/role "$(_compute_json K2_CJ_user="$user" K2_CJ_role="$r")" ;;
        *) echo "{\"error\":\"usage\",\"hint\":\"unknown users command '$sub' (k2 hostchat --help)\"}" >&2; exit 2 ;;
    esac
    _hostchat_print "users-$sub" "$as_json"
}

