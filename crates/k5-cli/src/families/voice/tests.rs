use super::*;

/// The shell's text with the brand's names: `k2 ` the command, `K2 ` the
/// product.
fn b(text: &str) -> String {
    text.replace("k2 ", &format!("{CLI_NAME} "))
        .replace("K2 ", &format!("{PRODUCT_NAME} "))
}

fn out(verb: &str, body: &str) -> String {
    let r = print(body, verb);
    assert_eq!((r.exit, r.stderr.as_str()), (Exit::Ok, ""), "{verb} {body}");
    r.stdout
}

#[test]
fn the_lines_as_the_shell_prints_them() {
    assert_eq!(
        out("rooms", r#"{"server":"acme","hosting":{"state":"degraded","line":"slow","detail":"x"},"rooms":[{"slug":"lounge","name":"Lounge","occupants":[{"user":"ann","muted":true,"deafened":1},{"user":"bob"}],"limit":10,"closing":true},{"slug":"b","unavailable":"sew down"}],"me":{"inCall":true,"room":{"name":"Lounge"},"presenceLagSecs":30}}"#),
        "Voice rooms on acme: degraded: slow (x)\n  lounge   Lounge                   2/10  closing\n      ann (muted, deafened)\n      bob\n  b        None                     0/None  unavailable: sew down\nyou: in Lounge from another device or session (if that app was closed, this can lag up to 30 s)\n"
    );
    assert_eq!(
        out("status", r#"{"hosting":{"state":"ok"},"count":3,"tier":{"state":"ok","reading":"live","reason":"r"},"space":{"state":"made","spaceId":"s1","lastCheck":{"outcome":"relinked","from":"a","to":"b"}},"rooms":[{"slug":"l","wantName":"Lounge","state":"open","closeNow":1}],"members":[{"externalId":"ann","state":"added"},{"externalId":"bob","state":"added","wantLevel":"mod","level":"member","why":"w"},{"externalId":"cy","state":"pending"}],"holds":[{"op":"add","subject":"x","state":"held","error":"e"}],"carriedAgents":["a","b"],"staleSpaces":[{"slug":"old"},{}],"lane":{"state":"up","reason":"fine"},"feed":{"state":"on"}}"#),
        b("hosting:    on\nrooms:      3 (plan: ok, live reading: r)\nK5 Space: made s1\n  last check: K2 re-linked a stale id (was a, now b)\n  l        Lounge                   open  (close now)\nmembers:    3\n  bob: added \u{2014} w\n  cy: pending\nhold:       add x held \u{2014} e\nagents carried to k5-admin: a, b\nset aside:  2 agent Space(s) from an earlier Sew install (old, ?): k2 voice repair says how to make them again\nvoice lane: up \u{2014} fine\nfeed:       on\n")
    );
    assert_eq!(
        out("repair", r#"{"space":"s1","rooms":[{"room":"l","to":"c2"}],"members":{"added":3,"pending":1,"held":[{"externalId":"x","why":"y"}]},"fixed":[],"settled":true,"notRestored":"n","staleSpaces":[{"slug":"o","spaceId":"i","access":[{"workspace":"w","level":2},{"projectId":"p"}]},{"slug":"z","access":[]}],"staleHint":"h"}"#),
        "K5 Space:   s1\n  l: re-linked to channel c2 (was none)\nmembers:    3 in the Space, 1 still being added\n  held:     x \u{2014} y\nfixed:      nothing needed fixing\nnote:       n\nset aside:  2 agent Space(s) from an earlier Sew install:\n  o (old id i): w (level 2), p (level None)\n  z (old id None): no agents\n  how:      h\n"
    );
    assert_eq!(
        out("transcript", r#"{"recording":{"id":7,"room":"l","startedAt":"t"},"lines":[{"startMs":"65432","speaker":"ann","text":"hi"},{"startMs":-1500},{}],"next":9}"#),
        b("Recording 7 in l, started t (transcript still being written)\n[01:05] ann: hi\n[-1:58] None: None\n[00:00] None: None\nmore: k2 voice transcript 7 --after 9\n(this read is logged)\n")
    );
    assert_eq!(
        out("leave", r#"{"already":true,"why":"moved"}"#),
        "nothing to leave: that call already ended (you joined another room from here) [moved]\n"
    );
    assert_eq!(out("leave", r#"{}"#), "left\n");
    assert_eq!(
        out("settings", r#"{"changed":false,"recording":{"flags":{},"required":false,"locked":1,"agentReaders":1}}"#),
        b("K5 rooms recording: nothing changed (already so)\nThis server's chat can't record in rooms yet (it needs Sew 0.9.0: k2 hostchat upgrade).\nK5 rooms Space:     not made yet\nstart level:        inherit (the server's rule)\nrequire recording:  off\nLOCKED: no recording in the K5 rooms (CUI/FCI); only the owner lifts it, in person\ntranscripts:        agents on this server may read them (each person's notice says so; k2 voice transcripts)\n")
    );
    assert_eq!(
        out("recordings", r#"{"recordings":[{"id":1,"channelId":"c","startedAt":"s","trigger":"auto","transcriptState":"queued"}],"next":5,"canRead":true}"#),
        b("  1                    c          s                    auto  transcript queued\nmore: k2 voice recordings --after 5\nread one: k2 voice transcript <id>\n")
    );
    assert_eq!(
        out("subscribe", r#"{"agent":"bot","on":true,"readers":["bot"],"rooms":[{"room":"l","name":"L","sendTo":[]}]}"#),
        "bot now gets this room's transcripts (already so)\nagents that may read transcripts: bot\n  l        L                        sends to: nobody\n"
    );
    assert_eq!(
        out(
            "reads",
            r#"{"reads":[{"at":"t","reader":"r","recordingId":1,"lines":3}]}"#
        ),
        "  t  r                            recording 1, 3 line(s)\n"
    );
    assert_eq!(
        out("consent", r#"{"recordingId":4}"#),
        "recording 4: you said no; it does not record you (a yes you gave before is withdrawn)\n"
    );
    assert_eq!(
        out(
            "moderate",
            r#"{"action":"mute","person":"ann","room":{"name":"L"}}"#
        ),
        "mute ann in L\n"
    );
    assert_eq!(
        out("set", r#"{"count":2,"closing":["a"],"closeNowAt":1}"#),
        "rooms: 2\nclosing: a (calls end in 10 s)\n"
    );
}

#[test]
fn where_the_python_raises() {
    let raises = |verb: &str, body: &str, stdout: &str, last: &str| {
        let r = print(body, verb);
        assert_eq!(r.exit, Exit::Error, "{verb} {body}");
        assert_eq!(r.stdout, stdout, "{verb} {body}");
        assert!(
            r.stderr.starts_with("Traceback (most recent call last):\n"),
            "{}",
            r.stderr
        );
        assert!(
            r.stderr.ends_with(&format!("{last}\n")),
            "{verb} {body}: {}",
            r.stderr
        );
    };
    raises(
        "rooms",
        "",
        "",
        "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)",
    );
    raises(
        "rooms",
        "[1]",
        "",
        "AttributeError: 'list' object has no attribute 'get'",
    );
    raises(
        "rooms",
        r#"{"hosting":"x"}"#,
        "",
        "AttributeError: 'str' object has no attribute 'get'",
    );
    raises(
        "rooms",
        r#"{"rooms":[{"occupants":5}]}"#,
        "Voice rooms on this server: None: \n",
        "TypeError: object of type 'int' has no len()",
    );
    raises(
        "status",
        r#"{"tier":{"reason":5}}"#,
        "hosting:    None: \n",
        "TypeError: can only concatenate str (not \"int\") to str",
    );
    raises(
        "set",
        r#"{"closing":[1]}"#,
        "rooms: None\n",
        "TypeError: sequence item 0: expected str instance, int found",
    );
    raises(
        "set",
        r#"{"closing":1}"#,
        "rooms: None\n",
        "TypeError: can only join an iterable",
    );
    raises(
        "leave",
        r#"{"already":1,"why":["a"]}"#,
        "",
        "TypeError: unhashable type: 'list'",
    );
    raises(
        "transcript",
        r#"{"lines":[{"startMs":"x"}]}"#,
        "Recording None in None, started None (transcript still being written)\n",
        "ValueError: invalid literal for int() with base 10: 'x'",
    );
}

#[test]
fn the_words() {
    let call = |post, path, payload: &str, verb, json| Plan::Call {
        post,
        path,
        payload: payload.into(),
        verb,
        json,
    };
    let bad = |t: &str| Plan::Bad(b(t));
    assert_eq!(parse(&[]), Plan::Help);
    assert_eq!(parse(&["help", "x"]), Plan::Help);
    assert_eq!(
        parse(&["rooms", "--json"]),
        call(false, "/cli/voice/rooms", "", "rooms", true)
    );
    assert_eq!(
        parse(&["list"]),
        call(false, "/cli/voice/rooms", "", "rooms", false)
    );
    assert_eq!(
        parse(&["rooms", "x"]),
        bad("k2 voice rooms takes no arguments")
    );
    assert_eq!(parse(&["rooms", "--x"]), bad("unknown flag --x"));
    assert_eq!(
        parse(&["rooms", "--now"]),
        bad("--now only applies to k2 voice set rooms and k2 voice close")
    );
    assert_eq!(parse(&["rooms", "--room"]), Plan::Shell);
    assert_eq!(parse(&["rooms", "--room", ""]), Plan::Shell);
    assert_eq!(
        parse(&[
            "recordings",
            "--room",
            "a b",
            "--after",
            "5",
            "--limit",
            "2"
        ]),
        call(
            false,
            "/cli/voice/recordings",
            "room=a%20b&after=5&limit=2",
            "recordings",
            false
        )
    );
    assert_eq!(
        parse(&["recordings", "--recording", "3"]),
        bad("--recording only applies to k2 voice transcripts reads")
    );
    assert_eq!(
        parse(&["recordings", "--after", "x"]),
        bad("--after, --limit and --recording are whole numbers")
    );
    assert_eq!(
        parse(&["transcript", "7", "--after", "3"]),
        call(
            false,
            "/cli/voice/transcript",
            "id=7&after=3",
            "transcript",
            false
        )
    );
    assert_eq!(
        parse(&["transcript", "x"]),
        bad("the recording id is a number (k2 voice recordings lists them)")
    );
    assert_eq!(
        parse(&["transcripts"]),
        call(false, "/cli/voice/transcripts", "", "transcripts", false)
    );
    assert_eq!(
        parse(&["transcripts", "reads", "--recording", "4", "--limit", "9"]),
        call(
            false,
            "/cli/voice/transcripts/reads",
            "id=4&limit=9",
            "reads",
            false
        )
    );
    assert_eq!(
        parse(&["transcripts", "unsubscribe", "lounge", "ws"]),
        call(
            true,
            "/cli/voice/transcripts/subscribe",
            r#"{"room": "lounge", "agent": "ws", "on": false}"#,
            "subscribe",
            false
        )
    );
    assert_eq!(
        parse(&["transcripts", "x"]),
        bad("k2 voice transcripts [list|subscribe|unsubscribe|reads]")
    );
    assert_eq!(
        parse(&["settings"]),
        call(false, "/cli/voice/status", "", "settings", false)
    );
    assert_eq!(
        parse(&[
            "settings",
            "--require-recording",
            "yes",
            "--start-level",
            "member"
        ]),
        call(
            true,
            "/cli/voice/recording/settings",
            r#"{"required": true, "startLevel": "member"}"#,
            "settings",
            false
        )
    );
    assert_eq!(
        parse(&["settings", "--start-level", "inherit"]),
        call(
            true,
            "/cli/voice/recording/settings",
            r#"{"startLevel": null}"#,
            "settings",
            false
        )
    );
    assert_eq!(parse(&["settings", "--start-level", "admin"]), bad("--start-level is none, moderator, member or inherit (the K5 rooms have no admin level)"));
    assert_eq!(
        parse(&["settings", "--require-recording", "maybe"]),
        bad("--require-recording is on, off or inherit")
    );
    assert_eq!(
        parse(&["set", "rooms", "04", "--now"]),
        call(
            true,
            "/cli/voice/settings",
            r#"{"count": 4, "now": true}"#,
            "set",
            false
        )
    );
    assert_eq!(
        parse(&["set", "rooms", "11"]),
        bad("the room count is 0-10")
    );
    assert_eq!(
        parse(&["set", "rooms", "1x"]),
        bad("the room count is a number 0-10")
    );
    assert_eq!(
        parse(&["set", "seats", "1"]),
        bad("k2 voice set rooms <0-10> [--now]")
    );
    assert_eq!(
        parse(&["rename", "l", "Big", "Room"]),
        call(
            true,
            "/cli/voice/rooms/rename",
            r#"{"room": "l", "name": "Big Room"}"#,
            "rename",
            false
        )
    );
    assert_eq!(
        parse(&["close", "l"]),
        call(
            true,
            "/cli/voice/rooms/close",
            r#"{"room": "l"}"#,
            "close",
            false
        )
    );
    assert_eq!(
        parse(&["join", "l"]),
        call(true, "/cli/voice/join", r#"{"room": "l"}"#, "join", false)
    );
    assert_eq!(
        parse(&["leave"]),
        call(true, "/cli/voice/session/leave", "{}", "leave", false)
    );
    assert_eq!(
        parse(&["leave", "--call", "c1"]),
        call(
            true,
            "/cli/voice/session/leave",
            r#"{"callId": "c1"}"#,
            "leave",
            false
        )
    );
    assert_eq!(
        parse(&["consent", "4", "yes", "--notice", "2"]),
        call(
            true,
            "/cli/voice/recording/consent",
            r#"{"recording_id": "4", "consent": true, "notice_version": 2}"#,
            "consent",
            false
        )
    );
    assert_eq!(
        parse(&["consent", "4", "no"]),
        call(
            true,
            "/cli/voice/recording/consent",
            r#"{"recording_id": "4", "consent": false}"#,
            "consent",
            false
        )
    );
    assert_eq!(
        parse(&["consent", "4", "yes"]),
        bad("yes needs --notice <version>, the notice_version you were shown")
    );
    assert_eq!(parse(&["consent", "4", "maybe"]), bad("say yes or no: k2 voice consent <recording-id> yes --notice <version> | k2 voice consent <recording-id> no"));
    assert_eq!(
        parse(&["mute", "l", "ann"]),
        call(
            true,
            "/cli/voice/moderate",
            r#"{"room": "l", "person": "ann", "action": "mute"}"#,
            "moderate",
            false
        )
    );
    assert_eq!(parse(&["kick", "l"]), bad("k2 voice kick <room> <person>"));
    assert_eq!(parse(&["nope"]), bad("unknown command nope"));
    assert_eq!(
        bad_line("say \"x\""),
        b("{\"error\":\"usage\",\"hint\":\"say \"x\" (k2 voice --help)\"}")
    );
}

#[test]
fn the_shell_s_lines() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/hostchat-voice.sh");
    let text = std::fs::read_to_string(&path).unwrap();
    for shell in [
        r#"    _voice_bad() { echo "{\"error\":\"usage\",\"hint\":\"$1 (k2 voice --help)\"}" >&2; exit 2; }"#,
        r#"            -*) echo "{\"error\":\"usage\",\"hint\":\"unknown flag $1 (k2 voice --help)\"}" >&2; exit 2 ;;"#,
        r#"            --room) vt_room="${2:?--room <room>}"; shift 2 ;;"#,
        r#"    raw="$(_compute_send "$@")""#,
        r#"    [ "$_CP_STATUS" = "200" ] || _compute_fail "$_CP_STATUS" "$_CP_BODY""#,
        r#"        printf '%s\n' "$_CP_BODY""#,
        r#"    printf '%s' "$_CP_BODY" | K2_VC_VERB="$verb" python3 -c '"#,
        r#"            local name="${pos[*]:1}""#,
    ] {
        assert!(
            text.lines().any(|l| l == shell),
            "hostchat-voice.sh no longer has {shell:?}"
        );
    }
    let policy = std::fs::read_to_string(path.with_file_name("cli-tools-policy.sh")).unwrap();
    assert!(policy.contains("        voice) echo \"hostchat\" ;;\n"));
}
