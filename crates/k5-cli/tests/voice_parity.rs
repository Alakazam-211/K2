//! `k5-cli voice` against the shell CLI's `cli/k2 voice`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - Every verb's lines and `--json`: rooms (and list), status, repair,
//!   settings (the read and the change), recordings, transcript,
//!   transcripts (list, subscribe, unsubscribe, reads), set rooms, rename,
//!   close, join, leave, consent, kick, mute and the rest.
//! - The daemon's refusals (`_compute_fail`: 400 is exit 2, the rest 3; a
//!   route floor's `message`; not JSON; no body), a permission card's
//!   closing line, Python's raise part way, an empty 200.
//! - The words: `_voice_bad`'s lines, help for `voice` alone and
//!   `voice help` (with no port or token too: no gate there).
//! - A passport over TCP, the cell socket, a POST with no answer there,
//!   the id tool's lines.
//! - One large list (above 256 KiB): recordings.
//! - What the binary hands to the shell: a value flag with no value.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "voice";
const ROOMS: &str = r#"{"server":"acme","hosting":{"state":"degraded","line":"slow","detail":"x"},"rooms":[{"slug":"lounge","name":"Lounge","occupants":[{"user":"ann","muted":true,"deafened":1},{"user":"bob"}],"limit":10,"closing":true},{"slug":"b","unavailable":"sew down"}],"me":{"inCall":true,"here":true,"room":{"name":"Lounge"},"presenceLagSecs":30}}"#;
const STATUS: &str = r#"{"hosting":{"state":"ok"},"count":3,"tier":{"state":"ok","reading":"live","reason":"r"},"space":{"state":"made","spaceId":"s1","lastCheck":{"outcome":"remade","from":"a","to":"b"}},"spaceHealth":{"state":"suspect","line":"l","fix":"f"},"rooms":[{"slug":"l","wantName":"Lounge","state":"open","closeNow":1}],"members":[{"externalId":"ann","state":"added","wantLevel":1,"level":1.0},{"externalId":"bob","state":"added","wantLevel":"mod","level":"member","why":"w é"},{"externalId":"cy","state":"pending"}],"holds":[{"op":"add","subject":"x","state":"held","error":"e"}],"carriedAgents":["a","b"],"staleSpaces":[{"slug":"old"},{}],"lane":{"state":"up","reason":"fine"},"feed":{"state":"on","lastError":"late"}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn one(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

#[test]
fn voice_reads_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "reads");
    let v = vec![
        // covers: voice
        one("rooms", &["rooms"], ROOMS),
        // covers: voice --json
        one("list --json", &["list", "--json"], ROOMS),
        one(
            "rooms: none, hosting on",
            &["rooms"],
            r#"{"hosting":{"state":"ok"},"rooms":[],"me":{}}"#,
        ),
        one("status", &["status"], STATUS),
        one(
            "status: the plan as it is, no Space",
            &["status"],
            r#"{"tier":"free","space":{"state":"none","why":"w"},"members":[]}"#,
        ),
        one(
            "repair",
            &["repair"],
            r#"{"space":"s1","rooms":[{"room":"l","to":"c2","from":"c1"}],"members":{"added":3,"pending":1,"held":[{"externalId":"x","why":"y"}]},"fixed":["a"],"notRestored":"n","staleSpaces":[{"slug":"o","spaceId":"i","access":[{"workspace":"w","level":2},{"projectId":"p"}]},{"slug":"z","access":[]}],"staleHint":"h","holds":[]}"#,
        ),
        // covers: voice settings
        one(
            "settings: the read",
            &["settings"],
            r#"{"recording":{"flags":{"appRecording":true},"spaceId":"s","startLevel":"member","required":true}}"#,
        ),
        one(
            "settings: a change",
            &[
                "settings",
                "--require-recording",
                "inherit",
                "--start-level",
                "none",
            ],
            r#"{"changed":true,"recording":{"flags":{},"required":null}}"#,
        ),
        // covers: voice recordings
        one(
            "recordings",
            &["recordings", "--room", "a b", "--limit", "5"],
            r#"{"recordings":[{"id":1,"room":"lounge","startedAt":"s","trigger":"auto","transcriptReady":true},{"id":2,"channelId":"c"}],"next":5,"canRead":true}"#,
        ),
        one("recordings: none", &["recordings"], r#"{"recordings":[]}"#),
        // covers: voice transcript
        one(
            "transcript",
            &["transcript", "7", "--after", "3"],
            r#"{"recording":{"id":7,"roomName":"Lounge","startedAt":"t","transcriptReady":1},"lines":[{"startMs":65432,"speaker":"ann","text":"hi é"},{"startMs":-1500},{}],"next":9}"#,
        ),
        // covers: voice transcripts
        one(
            "transcripts",
            &["transcripts"],
            r#"{"readers":["bot","eve"],"rooms":[{"room":"l","name":"L","sendTo":["bot"]},{"room":"m","name":"M"}]}"#,
        ),
        one(
            "transcripts reads",
            &["transcripts", "reads", "--recording", "4"],
            r#"{"reads":[{"at":"t","reader":"r","recordingId":1,"lines":3}]}"#,
        ),
        one(
            "transcripts reads: none",
            &["transcripts", "reads"],
            r#"{"reads":null}"#,
        ),
        traceback(case(
            "not JSON",
            &["rooms"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an empty 200",
            &["status"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "occupants with no len, part way",
            &["rooms"],
            &[],
            vec![ok(
                r#"{"hosting":{"state":"ok"},"rooms":[{"slug":"a","occupants":[]},{"occupants":5}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a hold's error that is no string",
            &["status"],
            &[],
            vec![ok(r#"{"holds":[{"error":3}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a start in ms that is no number",
            &["transcript", "1"],
            &[],
            vec![ok(r#"{"lines":[{"startMs":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn voice_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let v = vec![
        one(
            "set rooms",
            &["set", "rooms", "04", "--now"],
            r#"{"count":4,"closing":["a","b"],"closeNowAt":1}"#,
        ),
        one(
            "set rooms: none closing",
            &["set", "rooms", "0"],
            r#"{"count":0}"#,
        ),
        one(
            "rename",
            &["rename", "lounge", "Big", "Room"],
            r#"{"room":"lounge","name":"Big Room"}"#,
        ),
        one(
            "close",
            &["close", "lounge"],
            r#"{"count":2,"closing":["lounge"]}"#,
        ),
        one(
            "join",
            &["join", "lounge"],
            r#"{"room":{"name":"Lounge"},"callId":"c1"}"#,
        ),
        one(
            "join --json",
            &["join", "lounge", "--json"],
            r#"{"room":{"name":"Lounge"},"callId":"c1","token":"t"}"#,
        ),
        one("leave", &["leave", "--call", "c1"], r#"{"ok":true}"#),
        one(
            "leave: already",
            &["leave"],
            r#"{"already":true,"why":"sew_dropped"}"#,
        ),
        one(
            "leave: already, a reason of its own",
            &["leave"],
            r#"{"already":true,"why":"odd"}"#,
        ),
        one(
            "consent yes",
            &["consent", "4", "yes", "--notice", "2"],
            r#"{"recordingId":4,"consent":true}"#,
        ),
        one(
            "consent no",
            &["consent", "4", "no"],
            r#"{"recordingId":4,"consent":false}"#,
        ),
        one(
            "subscribe",
            &["transcripts", "subscribe", "lounge", "ws"],
            r#"{"agent":"ws","on":true,"changed":true,"readers":[]}"#,
        ),
        one(
            "unsubscribe",
            &["transcripts", "unsubscribe", "lounge", "ws"],
            r#"{"agent":"ws","on":false}"#,
        ),
        one(
            "mute",
            &["mute", "lounge", "ann"],
            r#"{"action":"mute","person":"ann","room":{"name":"Lounge"}}"#,
        ),
        one(
            "kick",
            &["kick", "lounge", "bob"],
            r#"{"action":"kick","person":"bob"}"#,
        ),
        case(
            "a refusal: exit 3",
            &["join", "l"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"voice_rooms_off","hint":"preview é"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a bad request: exit 2",
            &["rename", "l", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"bad_name","message":"too long"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not found: exit 3",
            &["close", "l"],
            &[],
            vec![Reply::Answer(404, "not json\n".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 500 with no body",
            &["status"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a permission card",
            &["mute", "l", "a"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a reset: status 000",
            &["rooms"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn voice_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let bad = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], owner_and_port, 2, 0)
    };
    let v = vec![
        case(
            "voice alone: its help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "voice help",
            &["help", "x"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "voice alone with no port or token: its help",
            &[],
            &[],
            vec![],
            nothing,
            0,
            0,
        ),
        case(
            "voice help with no port",
            &["help"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        ),
        case(
            "an empty verb with no port: no gate, the unknown line",
            &[""],
            &[],
            vec![],
            owner_only,
            2,
            0,
        ),
        case(
            "rooms with no port: the gate",
            &["rooms"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        bad("an unknown verb", &["nope"]),
        bad("an unknown flag, as it is", &["rooms", "--x\"y"]),
        bad("rooms: a word", &["rooms", "x"]),
        bad("--now on rooms", &["rooms", "--now"]),
        bad("--notice on join", &["join", "l", "--notice", "1"]),
        bad(
            "--start-level on status",
            &["status", "--start-level", "none"],
        ),
        bad("--room on join", &["join", "l", "--room", "r"]),
        bad("--limit not a number", &["recordings", "--limit", "x"]),
        bad(
            "recordings --recording",
            &["recordings", "--recording", "1"],
        ),
        bad("transcript: no id", &["transcript"]),
        bad("transcript: an id that is no number", &["transcript", "x"]),
        bad("transcript --room", &["transcript", "1", "--room", "r"]),
        bad("transcripts: a bad verb", &["transcripts", "x"]),
        bad(
            "transcripts subscribe: two",
            &["transcripts", "subscribe", "l"],
        ),
        bad("transcripts reads: extra", &["transcripts", "reads", "x"]),
        bad("settings: admin", &["settings", "--start-level", "admin"]),
        bad(
            "settings: maybe",
            &["settings", "--require-recording", "maybe"],
        ),
        bad("set: eleven", &["set", "rooms", "11"]),
        bad("set: no number", &["set", "rooms", "x"]),
        bad("set: not rooms", &["set", "seats", "1"]),
        bad("rename: one", &["rename", "l"]),
        bad("close: none", &["close"]),
        bad("leave: a word", &["leave", "x"]),
        bad("consent: yes with no notice", &["consent", "4", "yes"]),
        bad("consent: maybe", &["consent", "4", "maybe"]),
        bad("consent: no id", &["consent", "x", "no"]),
        bad(
            "consent: a notice that is no number",
            &["consent", "4", "no", "--notice", "v2"],
        ),
        bad("kick: one", &["kick", "l"]),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["rooms"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn voice_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            with_passport(&[]),
            one("a host session: TCP", &["rooms"], ROOMS),
        ),
        with(
            cell(),
            case(
                "the cell socket",
                &["status"],
                &[],
                vec![ok(STATUS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served: TCP",
                &["join", "l"],
                &[],
                vec![not_served(), ok(r#"{"room":{}}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "a GET with no answer there: TCP",
                &["rooms"],
                &[],
                vec![Reply::Close, ok(ROOMS)],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(),
            case(
                "a POST with no answer there: not sent again",
                &["join", "l"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn voice_hands_these_to_the_shell() {
    // A value flag with no value is the shell's too (`parse` says so),
    // but its line names the script's own path, which differs between a
    // run of cli/k2 and the binary's hand-off: no row can compare it.
    let rig = Rig::new(VERB, "shell");
    rig.check(&Case {
        compare: Compare::Shell,
        ..case(
            "_K2_REFUSAL_AREA set: the shell reads it",
            &["rooms"],
            &[("_K2_REFUSAL_AREA", "app")],
            vec![ok(ROOMS)],
            owner_and_port,
            0,
            1,
        )
    });
}

/// `_CP_404_EXIT` exported: nothing in the shell reads it any more
/// (`_compute_fail` takes its exit for a 404 from the caller), so the
/// binary answers.
#[test]
fn voice_an_exported_404_exit_changes_nothing() {
    let rig = Rig::new(VERB, "cp404");
    for c in [
        case(
            "_CP_404_EXIT set: nothing reads it",
            &["rooms"],
            &[("_CP_404_EXIT", "4")],
            vec![ok(ROOMS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "_CP_404_EXIT set: a 404 is 3",
            &["rooms"],
            &[("_CP_404_EXIT", "4")],
            vec![Reply::Answer(404, r#"{"error":"x"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

/// The large list: above 256 KiB, thousands of rows. The answer reaches
/// the shell's Python on stdin, so bash prints it too.
#[test]
fn voice_recordings_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| format!(r#"{{"id":{i},"room":"lounge","startedAt":"2026-11-20T10:{:02}:00Z","trigger":"auto","transcriptReady":true}}"#, i % 60))
        .collect();
    let body = format!(r#"{{"recordings":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "a large list",
        &["recordings"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn voice_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens: status 000, exit 3",
        &["rooms"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        3,
        0,
    ));
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell. Where bash acts anyway (`--call` takes
/// `-h`/`--help` as its value, so the loop never sees it: a bash bug,
/// cli/src/hostchat-voice.sh:215), the rows copy bash and say so.
#[test]
fn voice_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str,
                 args: &'static [&'static str],
                 replies: Vec<Reply>,
                 requests: usize| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], replies, owner_and_port, 0, requests)
    };
    for c in [
        shell("join --help", &["join", "lounge", "--help"], vec![], 0),
        shell("set rooms -h", &["set", "rooms", "2", "-h"], vec![], 0),
        shell(
            "rename -h first",
            &["rename", "-h", "lounge", "Big"],
            vec![],
            0,
        ),
        shell(
            "close --help",
            &["close", "lounge", "--now", "--help"],
            vec![],
            0,
        ),
        shell("leave -h", &["leave", "-h"], vec![], 0),
        shell(
            "consent --help",
            &["consent", "4", "no", "--help"],
            vec![],
            0,
        ),
        shell("mute -h", &["mute", "lounge", "ann", "-h"], vec![], 0),
        shell(
            "settings --help",
            &["settings", "--start-level", "member", "--help"],
            vec![],
            0,
        ),
        shell(
            "transcripts subscribe -h",
            &["transcripts", "subscribe", "l", "ws", "-h"],
            vec![],
            0,
        ),
        shell("repair --help", &["repair", "--help"], vec![], 0),
        // The bash bug, copied: --call eats the help word and the verb runs.
        shell(
            "BASH BUG: leave --call --help leaves the call",
            &["leave", "--call", "--help"],
            vec![ok(r#"{"ok":true}"#)],
            1,
        ),
        shell(
            "BASH BUG: join with --call -h joins",
            &["join", "lounge", "--call", "-h"],
            vec![ok(r#"{"room":{"name":"Lounge"},"callId":"c"}"#)],
            1,
        ),
    ] {
        rig.check(&c);
    }
}
