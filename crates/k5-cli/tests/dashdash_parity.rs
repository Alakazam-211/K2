//! A bare `--` the verb doesn't take, against the shell CLI's `cli/k2` on
//! the parity rig ([`common::parity`]): the shell refuses it before it
//! does anything else (`_k2_takes_dashdash` and the loop after it,
//! `cli/src/cli-tools-policy.sh`), one line on stderr, exit 2, nothing
//! sent, and so does the binary ([`k5_cli::front::Route::NoDashdash`]).
//! With no daemon, with a passport, and before any help word.

#![cfg(unix)]

mod common;

use common::parity::{case, nothing, owner_and_port, port_only, with_passport, Case, Rig};

/// The lines, as the rig runs them: the first word, the words after it.
const LINES: &[(&str, &[&str])] = &[
    ("--", &["ls"]),
    ("--", &[]),
    ("whoami", &["--"]),
    ("workspace", &["list", "--", "x"]),
    ("compute", &["status", "--", "x"]),
    ("msg", &["verify", "--", "x"]),
    ("msg", &["who", "m_1", "--"]),
    ("vault", &["list", "--"]),
    ("vault", &["fetch", "--", "--help"]),
    ("agent", &["list", "--", "-h"]),
    ("heartbeat", &["list", "--", "-h"]),
    ("inbox", &["read", "--", "x"]),
    ("--version", &["--"]),
    ("cli", &["status", "--"]),
    ("study", &["--"]),
    ("help", &["--"]),
    ("browser", &["redeliver", "--", "x"]),
];

#[test]
fn a_stray_dashdash_is_refused_before_anything() {
    for (i, &(verb, args)) in LINES.iter().enumerate() {
        let rig = Rig::new(verb, &format!("stray{i}"));
        let rows: [Case; 3] = [
            case("the owner", args, &[], vec![], owner_and_port, 2, 0),
            case("no daemon", args, &[], vec![], nothing, 2, 0),
            Case {
                env: with_passport(&[]),
                ..case("a passport", args, &[], vec![], port_only, 2, 0)
            },
        ];
        for c in &rows {
            rig.check(c);
        }
    }
}
