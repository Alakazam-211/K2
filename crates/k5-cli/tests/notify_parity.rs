//! `k5-cli notify` against the shell CLI's, on the parity rig
//! ([`common::parity`]): the help, `email off`, `mute`, `unmute` and a
//! notice from a word or a file, their usage lines, a refusal and a reader
//! that can't read the answer. `-h` and `--help` are the shell's.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn run(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>, code: i32) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn notice_file(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(
        rig.proj.join("notice.txt"),
        "Line one é\n\nline three\n\n\n",
    )
    .unwrap();
}

#[test]
fn notify_answers_as_the_shell_does() {
    let rig = Rig::new("notify", "all");
    let v = vec![
        // covers: notify
        run("the help", &[], vec![], 0),
        run("email: the help", &["email"], vec![], 0),
        stop("another word", &["sms"], 2),
        // covers: notify email off
        run("email off", &["email", "off"], vec![ok("{}")], 0),
        stop("email off: a word after", &["email", "off", "x"], 2),
        run(
            "email off: refused",
            &["email", "off"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
        ),
        // covers: notify email mute
        run(
            "email mute",
            &["email", "mute", "ws"],
            vec![ok(r#"{"muted":true,"workspace":"ws"}"#)],
            0,
        ),
        // covers: notify email unmute
        run(
            "email unmute",
            &["email", "unmute", "ws é"],
            vec![ok(r#"{"muted":false,"workspace":null}"#)],
            0,
        ),
        stop("email mute: no workspace", &["email", "mute"], 2),
        // covers: notify email
        run(
            "a notice",
            &[
                "email",
                "owner",
                "--subject",
                "Build done é",
                "--text",
                "It passed.",
            ],
            vec![Reply::Answer(
                202,
                r#"{"to":"owner","id":"n1","status":"queued"}"#.into(),
            )],
            0,
        ),
        case(
            "a notice from a file",
            &[
                "email",
                "ada",
                "--text-file",
                "notice.txt",
                "--subject",
                "s",
            ],
            &[],
            vec![ok(r#"{"to":"ada","id":"n2","status":"sent"}"#)],
            notice_file,
            0,
            1,
        ),
        stop(
            "no file",
            &["email", "ada", "--text-file", "none.txt", "--subject", "s"],
            2,
        ),
        stop(
            "--text and --text-file",
            &[
                "email",
                "ada",
                "--text",
                "t",
                "--text-file",
                "none.txt",
                "--subject",
                "s",
            ],
            2,
        ),
        stop("no subject", &["email", "ada", "--text", "t"], 2),
        stop("--subject last", &["email", "ada", "--subject"], 2),
        stop("a stray word", &["email", "ada", "now"], 2),
        traceback(run(
            "the answer is not JSON",
            &["email", "ada", "--subject", "s", "--text", "t"],
            vec![ok("<html>")],
            1,
        )),
        // `-h` there is an unknown argument to bash (no request).
        shell(stop("-h: an unknown argument", &["email", "ada", "-h"], 2)),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new("notify", "brand");
    for (args, want) in [
        (
            &["email", "off", "x"][..],
            format!("usage: {cli} notify email off\n"),
        ),
        (
            &["email", "mute"],
            format!("usage: {cli} notify email mute <workspace>\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// `notify email mute --help` and `-h` (`text::FAMILY_FORMS`): `mute`
/// reads the flag as the workspace and mutes it, as the shell's does (a
/// bash bug: help acts; these rows flip when the shell's fix lands). A
/// help line a gate stops is the shell's.
#[test]
fn the_mute_help_lines_are_the_family_s() {
    let rig = Rig::new("notify", "help");
    let v = vec![
        // native: notify email mute --help
        run(
            "email mute --help mutes the workspace --help",
            &["email", "mute", "--help"],
            vec![ok(r#"{"muted":true,"workspace":"--help"}"#)],
            0,
        ),
        run(
            "email mute -h mutes the workspace -h",
            &["email", "mute", "-h"],
            vec![ok(r#"{"muted":true,"workspace":"-h"}"#)],
            0,
        ),
        shell(case(
            "email mute --help, the virtual desktop's switch: the shell's gate",
            &["email", "mute", "--help"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            1,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
