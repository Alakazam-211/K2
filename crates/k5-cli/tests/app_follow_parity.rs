//! `k5-cli app logs --follow` against the shell CLI's (`cli/src/app-doctor.sh`),
//! on the parity rig ([`common::parity`]): the first read, then a read
//! every 2 seconds printing only the lines that are new, until a request
//! is refused or the Python raises.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

/// A framed answer with these lines between the frame's head and end.
fn framed(lines: &[&str]) -> String {
    let mut f = vec!["--- untrusted App text ---".to_string(), "app: acme".into()];
    f.extend(lines.iter().map(|l| l.to_string()));
    f.push("--- end ---".into());
    let text = f
        .join("\n")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!(r#"{{"framed":"{text}"}}"#)
}

#[test]
fn logs_follow_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "follow");
    let refused = Reply::Answer(
        404,
        r#"{"error":{"code":"no_such_app","hint":"gone"}}"#.to_string(),
    );
    let v = vec![
        // native: app logs
        case(
            "follow: new lines each poll, then refused",
            &["logs", "acme", "--follow", "--tail", "3"],
            &[],
            vec![
                ok(&framed(&["a", "b"])),
                ok(&framed(&["a", "b", "c é"])),
                ok(&framed(&["b", "c é", "d", "e"])),
                ok(&framed(&["x"])),
                refused.clone(),
            ],
            owner_and_port,
            3,
            5,
        ),
        // native: app logs
        case(
            "-f: nothing before, then lines",
            &["logs", "-f", "acme", "--deployment", "d1"],
            &[],
            vec![ok(&framed(&[])), ok(&framed(&["one", "two"])), refused],
            owner_and_port,
            3,
            3,
        ),
        // native: app logs
        Case {
            compare: Compare::Traceback,
            ..case(
                "follow: an answer with no framed text",
                &["logs", "acme", "--follow"],
                &[],
                vec![ok(&framed(&["a"])), ok(r#"{"x":1}"#)],
                owner_and_port,
                1,
                2,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}
