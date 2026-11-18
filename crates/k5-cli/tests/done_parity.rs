//! `k5-cli done` against the shell CLI's `cli/k2 done`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every flag and error path `cmd_done` has, the cases of
//! `tests/cli/done_flags.sh` among them.
//!
//! - The flags: --agent, --blocked, words skipped, each usage line, the
//!   agent from AGENT_NAME under each prefix, unescaped as the shell sends
//!   it, the reason escaped.
//! - The prelude: the gates (before the flags), a port that is no port,
//!   PROJECT_PATH, PWD through a link.
//! - The credential: the owner from disk, a passport over TCP, a live cell
//!   socket (still TCP), PRD 13.9 #4's owner token, a re-minted passport,
//!   a heal, a dead passport's restart line.
//! - The answers: printed as they came with exit 0 whatever the status, a
//!   refusal's closing line never (exit 0), a pre-0.44.4 daemon's 405 (the
//!   GET again), migrating, the bulletin line.
//! - No answer, as the 0.46.15 no-resend test makes each: in time (28), an
//!   empty reply (52), a reset (56), an empty 200, not HTTP (curl 1), a 5xx
//!   with no body; each with TMPDIR unusable too (`_cli_curl`'s 75); no
//!   daemon (7). `done` posts once over TCP and never resends: it has no
//!   cell-socket arm.
//! - What the binary hands to the shell: -h and --help (as a flag and as
//!   --blocked's value), a session the API started, an agent with a `#`
//!   or a space.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "done";
const OK: &str = r#"{"success":true}"#;
const AGENT: (&str, &str) = ("K2_AGENT_NAME", "tester");

/// `case` with the agent's name set.
fn named(name: &'static str, args: &'static [&'static str], env: &[(&'static str, &str)], replies: Vec<Reply>, setup: fn(&Rig), code: i32, requests: usize) -> Case {
    let mut env: Vec<(&'static str, &str)> = env.to_vec();
    env.insert(0, AGENT);
    case(name, args, &env, replies, setup, code, requests)
}

/// `c` in an agent's session: the passport's settings, `extra` and the
/// agent's name.
fn passport_case(rig: &Rig, extra: &[(&'static str, &str)], c: Case) -> Case {
    let mut env = with_passport(rig.stub.port, extra);
    env.push((AGENT.0, AGENT.1.to_string()));
    Case { env, ..c }
}

#[test]
fn the_waited_mask() {
    assert_eq!(mask_waited("x within 30 s; y after 0 s z"), ("x within <n> s; y after <n> s z".to_string(), vec![30, 0]));
    assert_eq!(mask_waited("after all, within reach"), ("after all, within reach".to_string(), vec![]));
}

#[test]
fn done_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let ask = Reply::Answer(403, PERMISSION_ASK.into());
    let mut v = vec![
        named("owner from disk", &[], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("--blocked", &["--blocked", "waiting on review"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("--agent", &["--agent", "other"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("a summary word is skipped", &["shipped the fix"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("a reason with odd bytes", &["--blocked", "é & = /x \"q\" 'y' %41"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("an empty reason sends none", &["--blocked", ""], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("a reason may start with a dash", &["--blocked", "-x"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("the last of each flag", &["--agent", "a", "--blocked", "b", "--agent", "c", "--blocked", "d"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        named("the agent goes unescaped", &["--agent", "a&b=c%2F?é"], &[], vec![ok(OK)], owner_and_port, 0, 1),
        case("the agent under each prefix", &[], &[("K5_AGENT_NAME", ""), ("K2_AGENT_NAME", "k2name"), ("K2SO_AGENT_NAME", "old")], vec![ok(OK)], owner_and_port, 0, 1),
        case("the K5 name first", &[], &[("K5_AGENT_NAME", "k5name"), ("K2_AGENT_NAME", "k2name")], vec![ok(OK)], owner_and_port, 0, 1),
        case("the K2SO name last", &[], &[("K2SO_AGENT_NAME", "old")], vec![ok(OK)], owner_and_port, 0, 1),
        case("--agent over the setting", &["--agent", "flag"], &[("K2_AGENT_NAME", "env")], vec![ok(OK)], owner_and_port, 0, 1),
        case("no agent", &[], &[], vec![], owner_and_port, 1, 0),
        named("an empty --agent is no agent", &["--agent", ""], &[], vec![], owner_and_port, 1, 0),
        named("--agent with no name", &["--agent"], &[], vec![], owner_and_port, 2, 0),
        named("--agent before a flag", &["--agent", "-x"], &[], vec![], owner_and_port, 2, 0),
        named("--blocked with no reason", &["--blocked"], &[], vec![], owner_and_port, 2, 0),
        named("-x", &["-x"], &[], vec![], owner_and_port, 2, 0),
        named("--bogus", &["--bogus"], &[], vec![], owner_and_port, 2, 0),
        named("--json is no flag of done's", &["--json"], &[], vec![], owner_and_port, 2, 0),
        named("a lone dash", &["word", "-"], &[], vec![], owner_and_port, 2, 0),
        named("--blocked x --bogus", &["--blocked", "x", "--bogus"], &[], vec![], owner_and_port, 2, 0),
        case("an unknown flag before the agent check", &["-x"], &[], vec![], owner_and_port, 2, 0),
        named("the gate comes before the flags", &["--bogus"], &[], vec![], owner_only, 1, 0),
        named("no port", &[], &[], vec![], owner_only, 1, 0),
        named("no token", &[], &[], vec![], port_only, 1, 0),
        named("an empty port setting is no port", &[], &[("K5_PORT", "")], vec![], owner_only, 1, 0),
        named("a port that is no port", &[], &[("K2_PORT", "41o0")], vec![], owner_only, 3, 0),
        named("the virtual desktop's guard", &[], &[("K2_NO_OWNER_FALLBACK", "1")], vec![], owner_and_port, 3, 0),
        named("the guard comes before the flags", &["--bogus"], &[("K5_NO_OWNER_FALLBACK", "1")], vec![], owner_only, 3, 0),
        passport_case(&rig, &[("K2_NO_OWNER_FALLBACK", "1")], case("the guard lets a passport through", &[], &[], vec![ok(OK)], port_only, 0, 1)),
        passport_case(&rig, &[], case("a passport over TCP", &["--blocked", "ci"], &[], vec![ok(OK)], port_only, 0, 1)),
        passport_case(&rig, &[("K2_HOOK_SOCK", "{sock}")], case("a passport and a live cell socket: still TCP", &[], &[], vec![ok(OK)], port_only, 0, 1)),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..passport_case(&rig, &[("K2_HOOK_SOCK", "{sock}")], case("a passport, a cell socket and the owner token on disk (PRD 13.9 #4)", &[], &[], vec![ok(OK)], owner_and_port, 0, 1))
        },
        passport_case(&rig, &[], case("a re-minted passport is used from the start", &[], &[], vec![ok(OK)], healed_on_disk, 0, 1)),
        passport_case(&rig, &[], case("a refused passport heals once", &[], &[], vec![heal(&rig), ok(OK)], port_only, 0, 2)),
        passport_case(&rig, &[], case("a dead passport: the probe, then the restart line", &[], &[], vec![Reply::Answer(403, REFUSED.into()), Reply::Answer(403, REFUSED.into())], port_only, 0, 2)),
        passport_case(&rig, &[], case("a refused passport the probe finds good: no line", &[], &[], vec![Reply::Answer(403, REFUSED.into()), ok(r#"{"role":"canonical"}"#)], port_only, 0, 2)),
        named("a refusal is printed, exit 0, no closing line", &[], &[], vec![ask], owner_and_port, 0, 1),
        named("a 4xx answer is printed as it came", &[], &[], vec![Reply::Answer(400, r#"{"error":"bad agent"}"#.into())], owner_and_port, 0, 1),
        named("a pre-0.44.4 daemon: the same as a GET", &["--blocked", "é"], &[], vec![Reply::Answer(405, r#"{"error":"method not allowed for this route"}"#.into()), ok(OK)], owner_and_port, 0, 2),
        named("migrating: asked again", &[], &[], vec![Reply::Answer(503, r#"{"state":"migrating"}"#.into()), ok(OK)], owner_and_port, 0, 2),
        named("the bulletin line when it worked", &[], &[], vec![ok(BULLETIN)], owner_and_port, 0, 1),
        named("the answer's bytes as they came", &[], &[], vec![ok("{\"a\":1}\n\n x \n")], owner_and_port, 0, 1),
        named("PROJECT_PATH", &[], &[("K2_PROJECT_PATH", "/w/proj é&")], vec![ok(OK)], owner_and_port, 0, 1),
    ];
    let link = link_to_proj(&rig);
    v.push(Case {
        env: vec![AGENT, ("PWD", &*link.to_string_lossy())].into_iter().map(|(k, v)| (k, v.to_string())).collect(),
        cwd: Some(link.clone()),
        ..case("PWD through a link", &[], &[], vec![ok(OK)], owner_and_port, 0, 1)
    });
    for c in v {
        rig.check(&c);
    }
}

/// The 0.46.15 no-resend test's no-answer kinds, each with TMPDIR
/// usable and not. `done` sends once over TCP, so each is one request.
#[test]
fn done_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let waited = |c: Case| Case { compare: Compare::Waited, ..c };
    let no_tmp = |c: Case| Case { no_tmpdir: true, ..c };
    let v = vec![
        waited(named("the daemon closes with nothing (52)", &[], &[], vec![Reply::Close], owner_and_port, 1, 1)),
        named("a reset (56)", &[], &[], vec![Reply::Reset], owner_and_port, 56, 1),
        named("an empty 200", &[], &[], vec![ok("")], owner_and_port, 0, 1),
        named("not HTTP (status 000, curl 1)", &[], &[], vec![Reply::Raw(GARBAGE.to_vec())], owner_and_port, 1, 1),
        waited(named("a 5xx with an empty body", &[], &[], vec![Reply::Answer(502, String::new())], owner_and_port, 1, 1)),
        no_tmp(named("TMPDIR unusable: an empty reply is 75", &[], &[], vec![Reply::Close], owner_and_port, 75, 1)),
        no_tmp(named("TMPDIR unusable: a reset is 75", &[], &[], vec![Reply::Reset], owner_and_port, 75, 1)),
        no_tmp(named("TMPDIR unusable: not HTTP is 75", &[], &[], vec![Reply::Raw(GARBAGE.to_vec())], owner_and_port, 75, 1)),
        no_tmp(waited(named("TMPDIR unusable: a 5xx with no body still says so", &[], &[], vec![Reply::Answer(502, String::new())], owner_and_port, 1, 1))),
        no_tmp(named("TMPDIR unusable: an answer is an answer", &[], &[], vec![ok(OK)], owner_and_port, 0, 1)),
        no_tmp(named("TMPDIR unusable: a port that is no port is 75", &[], &[("K2_PORT", "41o0")], vec![], owner_only, 75, 0)),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Nothing accepted the connection: exit 7 after the tries, TMPDIR or
/// not (a refused connect is never the machine's).
#[test]
fn done_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    for no_tmpdir in [false, true] {
        let c = Case { no_tmpdir, ..named("nothing listens", &[], &[("K2_PORT", port.as_str())], vec![], owner_only, 7, 0) };
        rig.check(&c);
    }
}

/// No answer within 30 s: the line, exit 1. Slow: 30 s a side.
#[test]
fn done_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    let c = Case { compare: Compare::Waited, ..named("no answer in time (28)", &[], &[], vec![Reply::Hang(Duration::from_secs(35))], owner_and_port, 1, 1) };
    rig.check(&c);
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn done_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case { compare: Compare::Shell, ..c };
    let mut v = vec![
        shell(named("--help", &["--help"], &[], vec![], owner_and_port, 0, 0)),
        shell(named("-h after a word", &["word", "-h"], &[], vec![], owner_and_port, 0, 0)),
        shell(named("--help as the reason", &["--blocked", "--help"], &[], vec![ok(OK)], owner_and_port, 0, 1)),
        shell(named("-h after --agent", &["--agent", "-h"], &[], vec![], owner_and_port, 2, 0)),
        shell(named("an agent with a #", &["--agent", "a#b", "--blocked", "x"], &[], vec![ok(OK)], owner_and_port, 0, 1)),
        shell(named("an agent with a space", &["--agent", "a b"], &[], vec![], owner_and_port, 3, 0)),
        shell(case("an agent setting with a #", &[], &[("K2_AGENT_NAME", "a#b")], vec![ok(OK)], owner_and_port, 0, 1)),
    ];
    let api: [(&'static str, &'static [&'static str], i32, usize); 3] = [
        ("a session the API started", &[], 0, 1),
        ("a session the API started: --reason", &["--reason", "no answer needed"], 0, 1),
        ("a session the API started: --bogus", &["--bogus"], 2, 0),
    ];
    for (name, args, code, requests) in api {
        let replies = if requests == 1 { vec![ok(OK)] } else { vec![] };
        let mut c = passport_case(&rig, &[("K2_API_CELL", "1")], case(name, args, &[], replies, port_only, code, requests));
        c.compare = Compare::Shell;
        v.push(c);
    }
    for c in v {
        rig.check(&c);
    }
}
