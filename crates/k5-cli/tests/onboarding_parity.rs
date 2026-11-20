//! `k5-cli onboarding` against the shell CLI's, on the parity rig
//! ([`common::parity`]): `scan`, `adopt`, `defer`, `start-fresh` and the
//! old `later` and `fresh` (with their warning), the folder as `cd` and
//! `pwd` name it, a refusal's body with exit 0, no answer, the usage
//! lines. A folder `cd` can't enter is the shell's: its line names the
//! script, which differs by how bash was started, so it has no row here.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

fn one(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(
        name,
        args,
        &[],
        vec![ok(r#"{"ok":true}"#)],
        owner_and_port,
        code,
        1,
    )
}

#[test]
fn onboarding_answers_as_the_shell_does() {
    let rig = Rig::new("onboarding", "all");
    let v = vec![
        one("scan: this folder", &["scan"], 0),
        one("scan: a folder by its words", &["scan", "./../proj/."], 0),
        one(
            "adopt: a file under this folder",
            &["adopt", "CLAUDE.md"],
            0,
        ),
        one(
            "adopt: a path and a folder",
            &["adopt", "/x/GEMINI.md", ".."],
            0,
        ),
        one("defer", &["defer"], 0),
        one("start-fresh", &["start-fresh", ""], 0),
        one("later: the old name", &["later"], 0),
        one("fresh: the old name", &["fresh"], 0),
        case(
            "a refusal: its body, exit 0",
            &["defer"],
            &[],
            vec![Reply::Answer(403, REFUSED.into())],
            owner_and_port,
            0,
            1,
        ),
        Case {
            compare: Compare::Waited,
            ..case(
                "no answer",
                &["defer"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            )
        },
        case("no verb", &[], &[], vec![], owner_and_port, 1, 0),
        case(
            "adopt: no file",
            &["adopt"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case("no port", &["scan"], &[], vec![], owner_only, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}
