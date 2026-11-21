//! The front door's own forms against the shell CLI's `cli/k2`, on the
//! parity rig ([`common::parity`]):
//!
//! - `version`, `--version` and `-v`: the same line (`k2 <version>` in the
//!   default brand), exit 0, nothing sent.
//! - `cli status [--json]`: the binary's own verb. The shell's `cli)` arm
//!   only says so (one line, exit 2, nothing sent), so this is a named
//!   difference by design: the binary prints its table (or its JSON) from
//!   its route table and one `/cli/cli-usage` read, exit 0.
//!   tests/front_native.rs checks the table's contents.
//! - `cli <a word the front has no form for>`: the shell's one line, with
//!   the command's name on both sides, exit 2, nothing sent. The binary
//!   prints it itself (no hand-off: the same with no python3).

#![cfg(unix)]

mod common;

use common::parity::*;

const USAGE: &str =
    r#"{"rows":[{"kind":"fallthrough","key":"docs","count":2,"first_at":1,"last_at":2}]}"#;

#[test]
fn the_version_as_the_shell_prints_it() {
    for (verb, tag) in [
        ("version", "v-word"),
        ("--version", "v-flag"),
        ("-v", "v-short"),
    ] {
        let rig = Rig::new(verb, tag);
        rig.check(&case("the version", &[], &[], vec![], owner_and_port, 0, 0));
    }
}

/// `cli status` and `cli status --json`: the shell's line, and the
/// binary's own answer.
#[test]
fn cli_status_is_the_binary_own_verb() {
    let rig = Rig::new("cli", "status");
    let cases = [
        case(
            "cli status",
            &["status"],
            &[],
            vec![ok(USAGE)],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cli status --json",
            &["status", "--json"],
            &[],
            vec![ok(USAGE)],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in &cases {
        let name = c.name;
        rig.prepare(Side::Bash, c);
        let b = rig.run(true, c);
        assert_eq!(b.code, Some(2), "{name}: bash's exit: {b:?}");
        assert_eq!(b.stdout, "", "{name}: bash's stdout");
        assert_eq!(
            b.stderr, "k2 cli status is answered by the k2 CLI binary: run k2 cli status\n",
            "{name}: bash's line"
        );
        assert!(
            rig.read(Side::Bash, false, name).is_empty(),
            "{name}: bash sent"
        );
        let (r, sent) = rig.binary(c, false);
        assert_eq!((r.code, r.signal), (Some(0), None), "{name}: {r:?}");
        assert_eq!(r.stderr, "", "{name}: the binary's stderr");
        assert_eq!(sent.len(), 1, "{name}: the binary's requests: {sent:?}");
        assert!(sent[0].contains("/cli/cli-usage?"), "{name}: {}", sent[0]);
        if name.ends_with("--json") {
            let doc = k5_cli::out::json::parse(&r.stdout)
                .unwrap_or_else(|e| panic!("{name}: not JSON: {e:?}\n{}", r.stdout));
            assert!(
                matches!(doc, k5_cli::out::json::Value::Object(_)),
                "{name}: {}",
                r.stdout
            );
        } else {
            let first = r.stdout.lines().next().unwrap_or_default();
            assert!(
                first.contains(" native, ") && first.ends_with(" bash"),
                "{name}: {first}"
            );
        }
    }
}

/// `cli` with a word the front has no form for: the shell's `cli)` arm's
/// line, which the binary prints itself. With no python3 a hand-off would
/// exit 69 with its own line, so the same answer there proves it native.
#[test]
fn cli_with_another_word_is_the_shell_s_line_answered_here() {
    let rig = Rig::new("cli", "word");
    let line = format!("{}\n", k5_cli::cli_status::shell_line());
    let cases = [
        // native: cli <word>
        case("cli x", &["x"], &[], vec![], owner_and_port, 2, 0),
        case("cli usage", &["usage"], &[], vec![], owner_and_port, 2, 0),
        case(
            "cli status --bogus",
            &["status", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cli --json status",
            &["--json", "status"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "cli x --help",
            &["x", "--help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in &cases {
        let name = c.name;
        rig.check(c);
        let (r, sent) = rig.binary(c, true);
        assert_eq!(
            (r.code, r.stdout.as_str(), r.stderr.as_str()),
            (Some(2), "", line.as_str()),
            "{name}: the binary with no python3"
        );
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
    }
}

/// The line names the command by the brand's name, on both sides (in a
/// build with `K2_BRAND_DIR=brand/xy` it says `xy`, and never k2 or k5).
#[test]
fn cli_word_line_says_the_brand_s_name() {
    let n = k2_client::brand::CLI_NAME;
    let want = format!("{n} cli status is answered by the {n} CLI binary: run {n} cli status\n");
    let rig = Rig::new("cli", "brand");
    let c = case("brand", &["x"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(out.stderr, want);
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty(), "{log:?}");
    if n != "k2" {
        assert!(!out.stderr.contains("k2"), "{:?}", out.stderr);
        assert!(!out.stderr.contains("k5"), "{:?}", out.stderr);
    }
}
