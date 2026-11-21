use super::*;
use crate::exit::Exit;
use k2_client::brand::PRODUCT_NAME;

fn out(verb: &str, body: &str) -> String {
    let r = print::print(body, verb);
    assert_eq!((r.exit, r.stderr.as_str()), (Exit::Ok, ""), "{verb} {body}");
    r.stdout
}

#[test]
fn status_lines_as_the_shell_prints_them() {
    assert_eq!(
        out("status", r#"{"state":"running","installedVersion":"0.6.0","alarms":["a","b"],"voice":{"cert":{"daysLeft":3}}}"#),
        "Chat / Voice Hosting: running\nversion:         0.6.0\nalarms:          a, b\nTURN cert:       3 days left\n"
    );
    assert_eq!(
        out("status", r#"{"state":"x","pinnedVersion":null}"#),
        "Chat / Voice Hosting: x\nversion:         pin None\n"
    );
    assert_eq!(
        out(
            "job",
            r#"{"ok":true,"a":[],"b":"","c":null,"d":{"e":[1,"\u00e9"]},"f":0}"#
        ),
        "a:               []\nd:               {\"e\": [1, \"\\u00e9\"]}\nf:               0\n"
    );
}

#[test]
fn a_raise_part_way() {
    let r = print::print(
        r#"{"state":"x","mirror":{"kind":"dir","path":3}}"#,
        "status",
    );
    assert_eq!(r.exit, Exit::Error);
    assert!(
        r.stdout.starts_with("Chat / Voice Hosting: x\n"),
        "{}",
        r.stdout
    );
    assert!(
        r.stderr
            .ends_with("TypeError: unsupported operand type(s) for +: 'int' and 'str'\n"),
        "{}",
        r.stderr
    );
}

/// `_compute_json`: a key ending in `Token` first, the rest in order; a
/// value is one field whatever it holds.
#[test]
fn compute_json_puts_a_token_first() {
    assert_eq!(
        cj(&[
            ("phase", Field::S("drop-prev".into())),
            ("confirmToken", Field::S("t1".into()))
        ]),
        r#"{"confirmToken": "t1", "phase": "drop-prev"}"#
    );
    // `Token=` in a value is no secret key: it keeps its place.
    assert_eq!(
        cj(&[
            ("purge", Field::B("1".into())),
            ("keep", Field::B("0".into())),
            ("host", Field::S("aToken=b".into()))
        ]),
        r#"{"purge": true, "keep": false, "host": "aToken=b"}"#
    );
    // A newline in a value never splits it into keys.
    assert_eq!(
        cj(&[("sid", Field::S("own_a\nToken=b".into()))]),
        r#"{"sid": "own_a\nToken=b"}"#
    );
    // An empty secret is left out, as any empty value is.
    assert_eq!(
        cj(&[
            ("confirmToken", Field::S(String::new())),
            ("all", Field::B("1".into()))
        ]),
        r#"{"all": true}"#
    );
}

/// Every line that names the command names it by the brand's name (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2`
/// or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let named = |line: &str| {
        assert!(line.contains(&format!("{CLI_NAME} ")), "{line}");
        if CLI_NAME != "k2" {
            assert!(!line.contains("k2 ") && !line.contains("k5 "), "{line}");
        }
    };
    let lines: &[&[&str]] = &[
        &["nope"],
        &["status", "--x"],
        &["status", "--voice"],
        &["status", "--media-tarball", "m"],
        &["upgrade", "--accept-data-loss"],
        &["act-as"],
        &["config", "--setup-url"],
        &["enable", "--tarball", ""],
        &["purge", "--confirm-hostname", ""],
    ];
    for &words in lines {
        let Plan::Bad(line) = parse(words) else {
            panic!("not a usage line: {words:?}")
        };
        named(&line);
    }
    for plan in [
        media::parse(&["install", "--url", ""]),
        users::parse(&["invite", "ann", "--role", ""]),
        admin::owner_sessions(&["list", "x"]),
        admin::owner_sessions(&["end"]),
        admin::owner_sessions(&["end", "x"]),
        admin::owner_sessions(&["nope"]),
        admin::owner_sessions(&["--x"]),
    ] {
        let Plan::Bad(line) = plan else {
            panic!("not a usage line: {plan:?}")
        };
        named(&line);
    }
    named(&out("status", r#"{"upgradeAvailable":true}"#));
    named(&out("status", r#"{"recoveryKeyInVault":true}"#));
    named(&out("rollback", r#"{"mediaCommand":"x"}"#));
    let tie = out("status", r#"{"mirror":{"tie":{"state":"pending"}}}"#);
    assert!(tie.contains(&format!("{PRODUCT_NAME} retries")), "{tie}");
}

/// `recording policy`'s usage lines name the command by the brand's name
/// (in a build with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed
/// `k2` or `k5`.
#[test]
fn recording_policy_lines_say_the_brand_s_name() {
    let lines: &[(&[&str], String)] = &[
        (
            &["policy", "x"],
            format!("{CLI_NAME} hostchat recording policy [--json] | {CLI_NAME} hostchat recording policy set KEY VALUE"),
        ),
        (
            &["policy", "--x"],
            format!("unknown flag --x ({CLI_NAME} hostchat recording policy [set KEY VALUE])"),
        ),
        (
            &["policy", "set"],
            format!("{CLI_NAME} hostchat recording policy set KEY VALUE [KEY VALUE \u{2026}] ({CLI_NAME} hostchat recording --help lists the keys)"),
        ),
        (
            &["policy", "set", "dm_enabled"],
            format!("{CLI_NAME} hostchat recording policy set KEY VALUE [KEY VALUE \u{2026}] ({CLI_NAME} hostchat recording --help lists the keys)"),
        ),
    ];
    for (words, hint) in lines {
        let Plan::Bad(line) = recording::parse(words) else {
            panic!("not a usage line: {words:?}")
        };
        assert_eq!(
            line,
            format!("{{\"error\":\"usage\",\"hint\":\"{hint}\"}}"),
            "{words:?}"
        );
        if CLI_NAME != "k2" {
            assert!(!line.contains("k2 ") && !line.contains("k5 "), "{line}");
        }
    }
    // A key error names no command: the same in every brand.
    let Plan::Bad(line) = recording::parse(&["policy", "set", "dm_enabled", "maybe"]) else {
        panic!("not a usage line")
    };
    assert_eq!(
        line,
        r#"{"error": "usage", "hint": "recording.dm_enabled is on or off"}"#
    );
}

/// A value flag as the last word: the shell's `_need_value` line. One
/// with an empty value: one plain line, exit 2 (K2's D6), where the shell
/// stops on its own `${2:?…}` line.
#[test]
fn a_value_flag_with_no_value_is_the_d6_line() {
    let want = |flag: &str| {
        Plan::Bad(format!(
            "Error: {flag} needs a value (try '{CLI_NAME} hostchat --help')"
        ))
    };
    let last = |flag: &str| Plan::Bad(format!("Error: {flag} takes a value"));
    assert_eq!(parse(&["enable", "--voice-url"]), last("--voice-url"));
    assert_eq!(parse(&["upgrade", "--tarball", ""]), want("--tarball"));
    // The first flag that has none, in the shell's order.
    assert_eq!(
        parse(&["status", "--public-url", "", "--bogus"]),
        want("--public-url")
    );
    assert_eq!(media::parse(&["probe", "--udp"]), last("--udp"));
    assert_eq!(
        media::parse(&["install", "--voice-url", ""]),
        want("--voice-url")
    );
    assert_eq!(users::parse(&["invite", "ann", "--email"]), last("--email"));
}

/// `owner-sessions` (follows 5b6759be0d): the words as the 0.46.17
/// queue's shell reads them.
#[test]
fn owner_sessions_words() {
    use admin::Admin;
    let list = |json| Plan::Admin(Admin::OwnerSessions { json });
    assert_eq!(admin::owner_sessions(&[]), list(false));
    assert_eq!(admin::owner_sessions(&[""]), list(false));
    assert_eq!(admin::owner_sessions(&["--json"]), list(true));
    assert_eq!(admin::owner_sessions(&["list", "--json"]), list(true));
    assert_eq!(admin::owner_sessions(&["list", "-h"]), Plan::Shell);
    assert_eq!(
        admin::owner_sessions(&["revoke", "own_1"]),
        Plan::Admin(Admin::OwnerSessionsEnd {
            payload: r#"{"sid": "own_1"}"#.into(),
            label: "End owner chat session own_1".into(),
            json: false,
        })
    );
    assert_eq!(
        admin::owner_sessions(&["end", "--all", "--json"]),
        Plan::Admin(Admin::OwnerSessionsEnd {
            payload: r#"{"all": true}"#.into(),
            label: "End every owner chat session".into(),
            json: true,
        })
    );
    let usage = |hint: &str| Plan::Bad(crate::families::chat::bad_line(hint));
    assert_eq!(
        admin::owner_sessions(&["end", "--all", "own_1"]),
        usage(&format!(
            "{CLI_NAME} hostchat owner-sessions end <own_\u{2026}> | end --all"
        ))
    );
    assert_eq!(
        admin::owner_sessions(&["end", "sid_1"]),
        usage(&format!(
            "an owner session's id starts with own_ ({CLI_NAME} hostchat owner-sessions)"
        ))
    );
    assert_eq!(
        admin::owner_sessions(&["list", "--all"]),
        usage(&format!(
            "{CLI_NAME} hostchat owner-sessions [list] [--json]"
        ))
    );
}
