//! A family's help in a session with `HOOK_SOCK` set: `cli/k2` and the
//! binary, run as people run them against one logging stub daemon
//! ([`common::parity`]), must send the same requests and print the same
//! bytes with the same exit. The shell runs its session checks first
//! (`_cli_check_session_passport`, `_cli_apply_auth_for_verb`); the
//! binary runs the same checks ([`k5_cli::pages::session_checks`]) and
//! prints the page, or the check's lines, itself.

#![cfg(unix)]

mod common;

use common::parity::{
    case, dead_port, heal, nothing, ok, owner_and_port, with_passport, Case, Compare, Rig, REFUSED,
};
use common::stub::Reply;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn a_family_s_help_with_hook_sock() {
    let rig = Rig::new("mail", "help");
    let port = rig.stub.port;
    let sock = |s: &'static str| with_passport(port, &[("K2_HOOK_SOCK", s)]);
    let base = |name, replies, code, requests| {
        case(
            name,
            &["send", "--help"],
            &[],
            replies,
            nothing,
            code,
            requests,
        )
    };
    let cases = vec![
        with(
            sock("{sock}"),
            base(
                "a passport and its live socket: the page, nothing sent",
                vec![],
                0,
                0,
            ),
        ),
        with(
            sock("{deadsock}"),
            base(
                "a socket file nothing listens on is still a socket: the page",
                vec![],
                0,
                0,
            ),
        ),
        with(
            sock("{nosock}"),
            base(
                "the socket gone, the daemon knows the passport: one probe, the page",
                vec![ok(r#"{"workspace":"acme"}"#)],
                0,
                1,
            ),
        ),
        with(
            sock("{nosock}"),
            base(
                "the socket gone, the daemon refuses the passport: the stale lines, exit 3",
                vec![Reply::Answer(401, REFUSED.to_string())],
                3,
                1,
            ),
        ),
        with(
            sock("{nosock}"),
            base(
                "the probe gets no answer in time: the page",
                vec![Reply::Hang(std::time::Duration::from_secs(7))],
                0,
                1,
            ),
        ),
        with(
            sock("{nosock}"),
            base(
                "the probe's connection closes with nothing: the page",
                vec![Reply::Close],
                0,
                1,
            ),
        ),
        with(
            sock("{nosock}"),
            base(
                "a refusal that re-mints the passport: the probe is not healed",
                vec![heal(&rig)],
                3,
                1,
            ),
        ),
        with(
            vec![
                ("K2_PORT", dead_port()),
                ("K2_HOOK_TOKEN", common::parity::PASSPORT.to_string()),
                ("K2_HOOK_SOCK", "{nosock}".to_string()),
            ],
            base(
                "the socket gone and nothing on the port: the probe's tries, the page",
                vec![],
                0,
                0,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            Case {
                setup: owner_and_port,
                ..base("no passport: an id tool is refused, exit 3", vec![], 3, 0)
            },
        ),
        with(
            sock("{sock}")
                .into_iter()
                .chain([("CODEX_THREAD_ID", "t1".to_string())])
                .collect(),
            Case {
                compare: Compare::Shell,
                ..base(
                    "a CODEX_ setting: the shell looks for a shared Codex server",
                    vec![],
                    0,
                    0,
                )
            },
        ),
        with(
            vec![
                ("K2_PORT", format!("0{port}")),
                ("K2_HOOK_TOKEN", common::parity::PASSPORT.to_string()),
                ("K2_HOOK_SOCK", "{nosock}".to_string()),
            ],
            Case {
                compare: Compare::Shell,
                ..base(
                    "a port with a leading zero: the shell's probe",
                    vec![ok("{}")],
                    0,
                    1,
                )
            },
        ),
    ];
    for c in &cases {
        rig.check(c);
    }
}

#[test]
fn the_tool_is_the_first_two_words() {
    let rig = Rig::new("agent", "help");
    let no_passport = vec![("K2_HOOK_SOCK", "{nosock}".to_string())];
    for (name, args, code) in [
        ("agent --help is no tool: the page", &["--help"][..], 0),
        (
            "agent harness --help is an id tool: refused without a passport",
            &["harness", "--help"],
            3,
        ),
        ("agent hire --help is open", &["hire", "--help"], 0),
    ] {
        let c = with(
            no_passport.clone(),
            Case {
                setup: owner_and_port,
                ..case(name, args, &[], vec![], nothing, code, 0)
            },
        );
        rig.check(&c);
    }
}
