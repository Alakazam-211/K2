//! `k5-cli app exec` against the shell CLI's (`cli/src/app-doctor.sh`),
//! on the parity rig ([`common::parity`]): the same request (the body
//! with the stdin in base64, the timeout), stdout, stderr and exit code.
//!
//! - No stdin, `--no-stdin`, a pipe on stdin (bytes that aren't UTF-8 too);
//!   `--timeout` plain and as a duration; `--json`; the command's exit,
//!   124 at its timeout, 1 for one out of range; a refusal; the words.
//! - An answer above 256 KiB; help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

fn with_stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

#[test]
fn exec_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "exec");
    let ran = |code: i32| {
        format!(r#"{{"framed":"--- untrusted ---\nhello é\n--- end ---","exitCode":{code}}}"#)
    };
    let v = vec![
        // native: app exec
        case(
            "no stdin, exit 0",
            &["exec", "acme", "--", "ls", "-l"],
            &[],
            vec![ok(&ran(0))],
            owner_and_port,
            0,
            1,
        ),
        // native: app exec
        case(
            "the command's exit",
            &["exec", "acme", "sh", "-c", "exit 7"],
            &[],
            vec![ok(&ran(7))],
            owner_and_port,
            7,
            1,
        ),
        // native: app exec
        case(
            "an exit out of range: 1",
            &["exec", "acme", "x"],
            &[],
            vec![ok(&ran(300))],
            owner_and_port,
            1,
            1,
        ),
        // native: app exec
        case(
            "timed out: 124, the note",
            &["exec", "acme", "--timeout", "1m30s", "--", "sleep", "99"],
            &[],
            vec![ok(r#"{"framed":"f","timedOut":true,"exitCode":0}"#)],
            owner_and_port,
            124,
            1,
        ),
        // native: app exec
        with_stdin(
            case(
                "a pipe on stdin",
                &["exec", "acme", "--timeout", "30", "--", "cat"],
                &[],
                vec![ok(&ran(0))],
                owner_and_port,
                0,
                1,
            ),
            b"line one\nline two \xff\xfe\n",
        ),
        // native: app exec
        with_stdin(
            case(
                "an empty pipe on stdin",
                &["exec", "acme", "--", "wc"],
                &[],
                vec![ok(&ran(0))],
                owner_and_port,
                0,
                1,
            ),
            b"",
        ),
        // native: app exec
        with_stdin(
            case(
                "--no-stdin with a pipe",
                &["exec", "--no-stdin", "acme", "--", "cat"],
                &[],
                vec![ok(&ran(0))],
                owner_and_port,
                0,
                1,
            ),
            b"not sent",
        ),
        // native: app exec
        case(
            "--json",
            &["exec", "acme", "--json", "--", "ls"],
            &[],
            vec![ok(&ran(3))],
            owner_and_port,
            3,
            1,
        ),
        // native: app exec
        case(
            "refused",
            &["exec", "acme", "--", "ls"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // native: app exec
        Case {
            compare: Compare::Traceback,
            ..case(
                "no framed text",
                &["exec", "acme", "ls"],
                &[],
                vec![ok(r#"{"exitCode":0}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        // native: app exec
        Case {
            no_tmpdir: true,
            compare: Compare::MktempTried { stem: "k2-exec-" },
            ..with_stdin(
                case(
                    "a pipe on stdin, no TMPDIR: mktemp ends it, nothing sent",
                    &["exec", "acme", "--", "cat"],
                    &[],
                    vec![],
                    owner_and_port,
                    1,
                    0,
                ),
                b"x",
            )
        },
        // native: app exec
        case(
            "no command",
            &["exec", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // native: app exec
        case(
            "an unknown flag",
            &["exec", "acme", "--zap", "--", "ls"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // native: app exec
        case(
            "a bad duration",
            &["exec", "acme", "--timeout", "soon", "ls"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // native: app exec
        case(
            "--timeout last",
            &["exec", "acme", "--timeout"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `doc-mktemp-wording`'s check for a folder mktemp couldn't make holds
/// only the random name apart: the same folder and stem, the same reason,
/// the same exit and every other line the same.
#[test]
fn mktemp_tried_holds_only_the_random_name() {
    const DIR: &str = "/t/no-tmp";
    const WHY: &str = "No such file or directory";
    const T: &str = "k2-exec-XXXXXX";
    let tried = |name: &str| format!("mktemp: mkdtemp failed on {DIR}/{name}: {WHY}\n");
    let binary = tried(T);
    let gnu = format!("mktemp: failed to create directory via template '{DIR}/{T}': {WHY}\n");
    let row = |what: &str, code: Option<i32>, bash: &str, want: bool| {
        let got = mktemp_tried_same("k2-exec-", (Some(1), binary.as_str()), (code, bash));
        assert_eq!(got, want, "{what}: the binary {binary:?}, bash {bash:?}");
    };
    row(
        "macOS names the folder it tried",
        Some(1),
        &tried("k2-exec-iHMPOm"),
        true,
    );
    row("an older macOS prints the template", Some(1), &binary, true);
    row(
        "a dot in the random name",
        Some(1),
        &tried("k2-exec-a.Bc12"),
        true,
    );
    row("another exit", Some(2), &tried("k2-exec-iHMPOm"), false);
    row("no exit", None, &tried("k2-exec-iHMPOm"), false);
    row("another stem", Some(1), &tried("k2-exed-iHMPOm"), false);
    row(
        "another folder",
        Some(1),
        &binary.replace(DIR, "/t/other"),
        false,
    );
    row(
        "another reason",
        Some(1),
        &binary.replace(WHY, "Permission denied"),
        false,
    );
    row(
        "a slash in the name",
        Some(1),
        &tried("k2-exec-iH/POm"),
        false,
    );
    row(
        "a dash in the name",
        Some(1),
        &tried("k2-exec-iH-POm"),
        false,
    );
    row("no name", Some(1), &tried("k2-exec-"), false);
    row("GNU's line", Some(1), &gnu, false);
    row("a line more", Some(1), &format!("{binary}x\n"), false);
    let other: &str = &format!("a line\n{binary}");
    let changed: &str = &format!("another line\n{}", tried("k2-exec-iHMPOm"));
    let same = mktemp_tried_same("k2-exec-", (Some(1), other), (Some(1), changed));
    assert!(!same, "a line other than mktemp's must be the same");
}

/// One answer above 256 KiB: a command that printed a lot.
#[test]
fn exec_a_large_answer() {
    let rig = Rig::new(VERB, "execbig");
    let lines: Vec<String> = (0..12000)
        .map(|n| format!("output line {n:08} é"))
        .collect();
    let body = format!(r#"{{"framed":"{}","exitCode":0}}"#, lines.join("\\n"));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [
        &["exec", "acme", "--", "ls"][..],
        &["exec", "acme", "--json", "--", "ls"],
    ] {
        rig.check(&case(
            "a large answer",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
    }
}

/// Help never acts.
#[test]
fn exec_help_never_acts() {
    let rig = Rig::new(VERB, "exechelp");
    for args in [
        &["exec", "acme", "-h"][..],
        &["exec", "--help", "acme", "--", "rm", "x"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help never acts", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
