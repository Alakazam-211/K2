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

#![cfg(unix)]

mod common;

use common::parity::*;

const USAGE: &str = r#"{"rows":[{"kind":"fallthrough","key":"docs","count":2,"first_at":1,"last_at":2}]}"#;

#[test]
fn the_version_as_the_shell_prints_it() {
    for (verb, tag) in [("version", "v-word"), ("--version", "v-flag"), ("-v", "v-short")] {
        let rig = Rig::new(verb, tag);
        rig.check(&case(
            "the version",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
}

/// `cli status` and `cli status --json`: the shell's line, and the
/// binary's own answer.
#[test]
fn cli_status_is_the_binary_own_verb() {
    let rig = Rig::new("cli", "status");
    let cases = [
        case("cli status", &["status"], &[], vec![ok(USAGE)], owner_and_port, 2, 0),
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
            b.stderr,
            "k5 cli status is answered by the k5 binary: run k5 cli status\n",
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
        assert!(
            sent[0].contains("/cli/cli-usage?"),
            "{name}: {}",
            sent[0]
        );
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
