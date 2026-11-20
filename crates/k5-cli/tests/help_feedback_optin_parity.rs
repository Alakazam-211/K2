//! The feedback agent opt-in (0.46.17) on the pages the binary prints
//! itself: while an agent's server says agents may not report, the daily
//! tier of the help leaves out send-feedback's block
//! (`cli/text/help/_daily-feedback.txt`) and `k2 study send-feedback` is
//! one line, in `--json` too ([`k5_cli::feedback_optin`]). The shell asks
//! once with a passport (`_sf_agents_off_here`), before the cards probe
//! on the daily tier, and so does the binary: each row on `cli/k2` and the
//! binary through the shared rig, the same requests and the same bytes.
//!
//! The rows: an agent while agents are off (hidden), with a drain hold in
//! `why` (still hidden: the switch counts, not `canSend`), agents on, a
//! person in a K2 terminal tab (a passport whose `senderKind` is
//! `person`), a person outside K2 (nothing asked), no daemon, a refusal,
//! an answer that is not JSON or has no `senderKind` (an older daemon),
//! no answer in time, on the cell socket and over TCP, and cards on with
//! agents off (the cards line where the shell puts it, after `_daily` and
//! without the block). With no `python3` the binary reads the answer
//! itself (K2's ruling; `scripts/cli-parity.sh`'s named difference
//! `feedback-optin-without-python3`): every row prints what it prints
//! with one (the rig's third run), and the rows that differ there on
//! purpose are checked on their own.

#![cfg(unix)]

mod common;

use common::cards;
use common::parity::{
    case, dead_port, ok, owner_and_port, with_passport, Case, Compare, NoPython3, Rig, PASSPORT,
    PORT, REFUSED,
};
use common::stub::Reply;
use std::time::Duration;

/// The daily tier's forms (`help_daily_cards_parity.rs`'s), then the
/// study page as text and JSON: the first word, the words after it, a tag
/// for the rig's folder, and whether the cards probe follows.
const FORMS: [(&str, &[&str], &str, bool); 11] = [
    ("", &[], "bare", true),
    ("help", &[], "word", true),
    ("--help", &[], "flag", true),
    ("-h", &[], "h", true),
    ("", &[""], "empty", true),
    ("help", &["nope"], "nope", true),
    ("--help", &[""], "flag-empty", true),
    ("-h", &["app"], "h-app", true),
    ("help", &["--json"], "json", true),
    // covers: study
    ("study", &["send-feedback"], "study", false),
    // covers: study --json
    ("study", &["send-feedback", "--json"], "study-json", false),
];

/// An agent's server while agents are off.
const OFF: &str = r#"{"enabled":true,"agents":false,"agentPlatform":false,"agentAttach":[],"canSend":false,"why":"agents_off","message":"Agents don't report problems on this server: a person hasn't allowed it.","senderKind":"agent","ok":true}"#;
/// Agents off and a drain hold, which `why` names before `agents_off`.
const OFF_HELD: &str = r#"{"enabled":true,"agents":false,"canSend":false,"why":"needs_connect","message":"This server isn't paired.","senderKind":"agent","ok":true}"#;
/// Agents on.
const ON: &str = r#"{"enabled":true,"agents":true,"agentPlatform":false,"canSend":true,"senderKind":"agent","ok":true}"#;
/// Agents on and a drain hold.
const ON_HELD: &str = r#"{"enabled":true,"agents":true,"canSend":false,"why":"needs_connect","senderKind":"agent","ok":true}"#;
/// A person in a K2 terminal tab while agents are off.
const PERSON_OFF: &str =
    r#"{"enabled":true,"agents":false,"canSend":true,"senderKind":"person","ok":true}"#;
/// An older daemon's settings: no `senderKind`.
const OLDER: &str = r#"{"enabled":true,"agents":false,"agentDiagnostics":false,"canSend":false}"#;

/// No `daemon.port` from an earlier row.
fn fresh(r: &Rig) {
    let _ = std::fs::remove_file(r.k2home.join("daemon.port"));
}

fn fresh_owner(r: &Rig) {
    fresh(r);
    owner_and_port(r);
}

/// The daemon moved to the stub's port: `daemon.port` says so.
fn moved(r: &Rig) {
    std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub().port)).unwrap();
}

fn cell() -> Vec<(&'static str, String)> {
    with_passport(&[("K2_HOOK_SOCK", "{sock}")])
}

fn tcp() -> Vec<(&'static str, String)> {
    with_passport(&[("K2_HOOK_SOCK", "{nosock}")])
}

/// The row that hides on the cell socket, whose page the rows that
/// differ with no python3 print there.
const HIDDEN_ON_THE_CELL: &str = "an agent, agents off, on the cell socket: hidden";

/// A row: its settings, and the settings read's answer (`None`: nothing
/// reaches a daemon). On the daily tier the cards probe follows on the
/// same channel and gets `cards_on`'s answer.
fn row(
    name: &'static str,
    args: &'static [&'static str],
    daily: bool,
    env: Vec<(&'static str, String)>,
    settings: Option<Reply>,
    cards_on: bool,
) -> Case {
    let reached = settings.is_some();
    let mut replies: Vec<Reply> = settings.into_iter().collect();
    if daily && reached {
        replies.push(ok(if cards_on { cards::ON } else { cards::OFF }));
    }
    let requests = if reached { 1 + usize::from(daily) } else { 0 };
    Case {
        env,
        ..case(name, args, &[], replies, fresh, 0, requests)
    }
}

fn rows(args: &'static [&'static str], daily: bool) -> Vec<Case> {
    let r = |name, env, settings: Option<Reply>| row(name, args, daily, env, settings, false);
    let mut v = vec![
        r(HIDDEN_ON_THE_CELL, cell(), Some(ok(OFF))),
        r(
            "an agent, agents off, over TCP (the socket is gone): hidden",
            tcp(),
            Some(ok(OFF)),
        ),
        r(
            "an agent, agents off, over TCP (no socket set): hidden",
            with_passport(&[]),
            Some(ok(OFF)),
        ),
        r(
            "agents off with a drain hold in why: hidden",
            cell(),
            Some(ok(OFF_HELD)),
        ),
        r(
            "agents off with a drain hold, over TCP: hidden",
            tcp(),
            Some(ok(OFF_HELD)),
        ),
        r("agents on: shown", cell(), Some(ok(ON))),
        r("agents on, over TCP: shown", tcp(), Some(ok(ON))),
        r(
            "agents on with a drain hold: shown",
            cell(),
            Some(ok(ON_HELD)),
        ),
        r(
            "a person in a K2 terminal tab, agents off: shown",
            cell(),
            Some(ok(PERSON_OFF)),
        ),
        r(
            "a person in a K2 terminal tab over TCP: shown",
            tcp(),
            Some(ok(PERSON_OFF)),
        ),
        r(
            "a refused read (403): shown",
            cell(),
            Some(Reply::Answer(403, REFUSED.to_string())),
        ),
        r(
            "a 500 that says agents off: shown",
            cell(),
            Some(Reply::Answer(500, OFF.to_string())),
        ),
        r(
            "a 200 that is not JSON: shown",
            cell(),
            Some(ok("agents: false")),
        ),
        r(
            "an older daemon (no senderKind): shown",
            cell(),
            Some(ok(OLDER)),
        ),
        r("an older daemon over TCP: shown", tcp(), Some(ok(OLDER))),
        r(
            "an empty 200: shown",
            cell(),
            Some(Reply::Answer(200, String::new())),
        ),
        r(
            "closed with nothing (52): shown",
            cell(),
            Some(Reply::Close),
        ),
        r(
            "no answer in time (curl 28, at most 2 s): shown",
            cell(),
            Some(Reply::Hang(Duration::from_secs(3))),
        ),
        r(
            "nothing listens on the socket: the tries, then shown",
            with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            None,
        ),
        r(
            "no daemon on the port: the tries, then shown",
            vec![
                ("K2_PORT", dead_port()),
                ("K2_HOOK_TOKEN", PASSPORT.to_string()),
                ("K2_HOOK_SOCK", "{nosock}".to_string()),
            ],
            None,
        ),
        Case {
            setup: fresh_owner,
            ..r(
                "a person outside K2 (the owner token): shown, nothing asked",
                vec![],
                None,
            )
        },
        Case {
            setup: fresh_owner,
            ..r(
                "the owner with HOOK_SOCK set: shown, nothing asked",
                vec![
                    ("K2_HOOK_SOCK", "{sock}".to_string()),
                    ("K2_PORT", PORT.to_string()),
                ],
                None,
            )
        },
        Case {
            compare: Compare::Shell,
            no_python3: NoPython3::Own,
            ..r(
                "a PYTHON setting: the shell's",
                cell()
                    .into_iter()
                    .chain([("PYTHONPATH", "/nonexistent".to_string())])
                    .collect(),
                Some(ok(OFF)),
            )
        },
        // The refused try's `\n000` comes before the answer in the
        // shell's `$(…)`, so its Python reads no JSON: shown, though the
        // moved daemon said agents off.
        Case {
            setup: moved,
            no_python3: NoPython3::Own,
            ..r(
                "the daemon moved its port: asked again there, read as shown",
                vec![
                    ("K2_PORT", dead_port()),
                    ("K2_HOOK_TOKEN", PASSPORT.to_string()),
                ],
                Some(ok(OFF)),
            )
        },
    ];
    if daily {
        v.push(row(
            "cards on and agents off: the cards line, no block",
            args,
            daily,
            cell(),
            Some(ok(OFF)),
            true,
        ));
        v.push(row(
            "cards on and agents on: the block and the cards line",
            args,
            daily,
            tcp(),
            Some(ok(ON)),
            true,
        ));
    }
    v
}

/// The page while hidden or shown, as the rows print it: the study page's
/// one line and its JSON, and the daily tier without the block.
fn check_the_pages(rig: &Rig, args: &'static [&'static str], daily: bool) {
    let all = rows(args, daily);
    let find = |name: &str| {
        all.iter()
            .find(|c| c.name.starts_with(name))
            .unwrap_or_else(|| panic!("no row {name}"))
    };
    let (hidden, _) = rig.binary(find(HIDDEN_ON_THE_CELL), false);
    let (shown, _) = rig.binary(find("agents on: shown"), false);
    assert_eq!((hidden.code, shown.code), (Some(0), Some(0)), "{args:?}");
    let line = k5_cli::feedback_optin::OFF_LINE;
    if !daily {
        let want = if args.contains(&"--json") {
            format!("{{\"id\":\"send-feedback\",\"body\":\"{line}\"}}\n")
        } else {
            format!("{line}\n")
        };
        assert_eq!(hidden.stdout, want, "{args:?}: hidden");
        assert!(shown.stdout.len() > 1000, "{args:?}: the full page");
        return;
    }
    let block = k5_cli::text::help("_daily-feedback").expect("the block");
    assert_eq!(
        hidden.stdout,
        k5_cli::pages::daily_text(false, true).unwrap(),
        "{args:?}: hidden"
    );
    assert_eq!(
        shown.stdout,
        k5_cli::pages::daily_text(false, false).unwrap(),
        "{args:?}: shown"
    );
    assert_eq!(
        shown.stdout.replacen(block, "", 1),
        hidden.stdout,
        "{args:?}: only the block goes"
    );
    let (both, _) = rig.binary(find("cards on and agents off"), false);
    assert_eq!(
        both.stdout,
        k5_cli::pages::daily_text(true, true).unwrap(),
        "{args:?}: cards on, hidden"
    );
}

#[test]
fn the_opt_in_with_a_passport() {
    for (verb, args, tag, daily) in FORMS {
        let rig = Rig::new(verb, tag);
        for c in rows(args, daily) {
            rig.check(&c);
        }
        check_the_pages(&rig, args, daily);
    }
}

/// The rows that answer otherwise with no `python3`
/// ([`NoPython3::Own`]): the binary sends what the row says and prints
/// the hidden page, as on the cell socket with `python3`.
#[test]
fn the_opt_in_reads_the_daemon_itself_without_python3() {
    for (verb, args, tag, daily) in FORMS {
        let rig = Rig::new(verb, &format!("{tag}-nopy"));
        let all = rows(args, daily);
        let hidden = all
            .iter()
            .find(|c| c.name == HIDDEN_ON_THE_CELL)
            .expect("the hidden row");
        let (want, _) = rig.binary(hidden, false);
        assert_eq!(want.code, Some(0), "{args:?}: {want:?}");
        let own: Vec<&Case> = all
            .iter()
            .filter(|c| c.no_python3 == NoPython3::Own)
            .collect();
        assert_eq!(own.len(), 2, "{args:?}: the rows that differ");
        for c in own {
            let (got, log) = rig.binary(c, true);
            let name = c.name;
            println!("== {name} {args:?} (no python3)\n-- rust: {got:?}");
            assert_eq!(rig.stub().left(), 0, "{name}: answers left unused");
            assert_eq!(log.len(), c.requests, "{name}: the requests {log:?}");
            assert_eq!(
                (got.code, got.stdout.as_str(), got.stderr.as_str()),
                (want.code, want.stdout.as_str(), want.stderr.as_str()),
                "{name} {args:?}: with no python3, the hidden page"
            );
        }
    }
}
