//! `k5-cli setup` against the shell CLI's `cli/k2 setup`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for each form `cmd_setup` has.
//!
//! - The list (`setup`, `setup --json`) in the help_native environments
//!   where they apply: the owner's terminal, the owner with `HOOK_SOCK`
//!   set, an agent with no daemon, an agent over TCP and an agent in a K2
//!   terminal (a live cell socket; the request still goes over TCP), each
//!   with plain answers and with a refusal carrying the permission cards'
//!   object; the owner token on disk in a session, which neither side
//!   sends (0.46.17).
//! - The answers: the daemon's lines as they came, its JSON and a newline,
//!   each refusal kind `tests/cli/setup_no_python.sh` has (exit 2, 3, 4,
//!   1), a body that is not JSON (cut at 200 bytes, or 200 characters in
//!   a UTF-8 locale), migrating, the bulletin line, a heal, a dead
//!   passport, an older daemon's JSON list read by the shell's Python
//!   (tracebacks too), the stray `000` a retried connect leaves (today's
//!   shell; 0.46.16's d14cb86da removes it).
//! - No answer: in time is the slow test; an empty reply, a reset, an
//!   empty 200, not HTTP, no daemon; TMPDIR unusable.
//! - The prelude: the gates (before the words), a port that is no port,
//!   `chief-of-staff`'s locked tool in a K2 terminal with no passport.
//! - Every usage mistake, sending nothing.
//! - What the binary hands to the shell: `help` as a word, a host that is
//!   not plain, and `agent-reports` (send-feedback's settings).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "setup";
const LIST: &str = "Setup: 1 of 3 done\n  [ ] Install developer tools (The Apple tools include git, which workspaces need: k2 git-tools install)\n  [x] Sign in to an AI tool\n  [-] Link your subdomain\nNext: meet Kessel, the agent that runs K2: k2 setup kessel\n";
const STATUS: &str = r#"{"enabled":true,"hidden":false,"finished":false,"rows":[{"id":"dev_tools","state":"todo","opens":"dev_tools"}],"offer":"meet_kessel"}"#;
const PERSON_ONLY: &str = r#"{"ok":false,"error":{"code":"first_run_person_only","hint":"the setup list belongs to a person: sign in with your own login (agents, app passes and API keys have none)"}}"#;

fn text(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn refusal(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

/// An agent's settings: the passport over TCP to the stub, and `extra`.
fn agent(extra: &[(&'static str, &str)], c: Case) -> Case {
    Case {
        env: with_passport(extra),
        ..c
    }
}

/// An environment's name, settings, files and how it compares.
type Environment = (&'static str, Vec<(&'static str, String)>, fn(&Rig), Compare);

/// The help_native environments, by name, as (env, setup, compare): the
/// owner (A), the owner with `HOOK_SOCK` (As), an agent over TCP (T), an
/// agent in a K2 terminal (C), and the last with the owner token on disk
/// too: since 0.46.17 the shell sends the passport there, as the binary
/// does (`_setup_token`), and the owner with `HOOK_SOCK` sends no
/// credential at all.
fn environments() -> Vec<Environment> {
    let sock = |extra: Vec<(&'static str, String)>| {
        let mut e = extra;
        e.push(("K2_HOOK_SOCK", "{sock}".to_string()));
        e
    };
    vec![
        ("owner", vec![], owner_and_port as fn(&Rig), Compare::Exact),
        (
            "owner, HOOK_SOCK set",
            sock(vec![]),
            owner_and_port,
            Compare::Exact,
        ),
        (
            "agent over TCP",
            with_passport(&[]),
            port_only,
            Compare::Exact,
        ),
        (
            "agent, a live cell socket",
            sock(with_passport(&[])),
            port_only,
            Compare::Exact,
        ),
        (
            "agent, a cell socket, the owner token on disk",
            sock(with_passport(&[])),
            owner_and_port,
            Compare::Exact,
        ),
    ]
}

/// The list and its refusals in each environment.
#[test]
fn the_list_in_each_environment() {
    let rig = Rig::new(VERB, "envs");
    let ask = || refusal(403, PERMISSION_ASK);
    let mut v = Vec::new();
    for (env_name, env, setup, compare) in environments() {
        let rows: Vec<(&'static str, &'static [&'static str], Reply, i32)> = vec![
            ("the list", &[], text(LIST), 0),
            ("--json", &["--json"], text(STATUS), 0),
            ("refused", &[], refusal(403, PERSON_ONLY), 3),
            ("--json refused", &["--json"], refusal(403, PERSON_ONLY), 3),
            ("a refusal with the cards' object", &[], ask(), 1),
        ];
        for (what, args, reply, code) in rows {
            let name: &'static str = Box::leak(format!("{env_name}: {what}").into_boxed_str());
            v.push(Case {
                env: env.clone(),
                compare,
                ..case(name, args, &[], vec![reply], setup, code, 1)
            });
        }
    }
    for c in v {
        rig.check(&c);
    }
}

/// An agent with no daemon: the virtual desktop's switch and a passport,
/// a port nothing listens on. `_k2_curl` tries four times, then no answer.
#[test]
fn an_agent_with_no_daemon() {
    let rig = Rig::new(VERB, "nodaemon");
    let port = dead_port();
    for (name, args) in [
        ("agent, no daemon: the list", &[][..]),
        ("agent, no daemon: --json", &["--json"][..]),
    ] {
        let c = Case {
            env: vec![
                ("K2_PORT", port.clone()),
                ("K2_HOOK_TOKEN", PASSPORT.to_string()),
                ("K2_SECRETS_DIR", "{secrets}".to_string()),
                ("K2_NO_OWNER_FALLBACK", "1".to_string()),
            ],
            ..case(name, args, &[], vec![], nothing, 1, 0)
        };
        rig.check(&c);
    }
    let c = case(
        "the owner, no daemon",
        &[],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    );
    rig.check(&c);
}

/// What the daemon answers, and how each is printed.
#[test]
fn the_answers() {
    let rig = Rig::new(VERB, "answers");
    let r = |s: u16, b: &str| refusal(s, b);
    let v = vec![
        // covers: setup
        case(
            "lines as they came",
            &[],
            &[],
            vec![text("a\n\n b \n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no newline at the end",
            &[],
            &[],
            vec![text("one line")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "leading spaces, then text",
            &[],
            &[],
            vec![text("  \n x")],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup --json
        case(
            "--json: the JSON and a newline",
            &["--json"],
            &[],
            vec![text("{\"a\":1}\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json: text as it came",
            &["--json"],
            &[],
            vec![text("plain")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty 200",
            &[],
            &[],
            vec![text("")],
            owner_and_port,
            1,
            1,
        ),
        case("a 204", &[], &[], vec![r(204, "")], owner_and_port, 1, 1),
        case(
            "unknown_row",
            &[],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"unknown_row","hint":"\"nope\" is not a row on your setup list (k2 setup shows them)"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "tool_not_signed_in",
            &[],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"tool_not_signed_in","hint":"grok isn't signed in on this server: sign in first"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "usage",
            &[],
            &[],
            vec![r(400, r#"{"error":{"code":"usage","message":"bad"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "kessel_first_run_off",
            &[],
            &[],
            vec![r(
                404,
                r#"{"ok":false,"error":{"code":"kessel_first_run_off","hint":"Kessel at first run is off on this server (the owner turns on kesselFirstRun)"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "kessel_session_only",
            &[],
            &[],
            vec![r(
                403,
                r#"{"ok":false,"error":{"code":"kessel_session_only","hint":"a shell tab is a person"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "role_required",
            &[],
            &[],
            vec![r(
                403,
                r#"{"error":"role_required","message":"This server gives you Voice & Chat only."}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a bad token",
            &[],
            &[],
            vec![r(403, r#"{"error":"invalid or missing token"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "forbidden in the hint",
            &[],
            &[],
            vec![r(403, r#"{"error":{"hint":"FORBIDDEN here"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "unknown_focus_group",
            &[],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"unknown_focus_group","hint":"no focus group is \"Nope\": nothing was made (Settings → Workspaces lists them)"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "ambiguous_focus_group, no hint",
            &[],
            &[],
            vec![r(
                400,
                r#"{"error":{"code":"ambiguous_focus_group","hint":""}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a hint with escapes",
            &[],
            &[],
            vec![r(
                500,
                r#"{"ok":false,"error":{"code":"store","hint":"disk full\twhile saving \\ the list é 😀 \/x"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the code in the hint",
            &[],
            &[],
            vec![r(400, r#"{"error":{"code":"x_y","hint":"see x_y"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a message, no hint",
            &[],
            &[],
            vec![r(400, r#"{"error":{"code":"c","message":"m"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a code alone",
            &[],
            &[],
            vec![r(400, r#"{"error":{"code":"just_code"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error that is a number",
            &[],
            &[],
            vec![r(400, r#"{"error":5}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no error at all",
            &[],
            &[],
            vec![r(409, r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "spaces around the colon",
            &[],
            &[],
            vec![r(
                400,
                "{\"error\" :\n {\"code\" : \"usage\" , \"hint\":\"h\"}}",
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not JSON",
            &[],
            &[],
            vec![r(502, "<html>bad gateway</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long, cut at 200 bytes",
            &[],
            &[],
            vec![r(502, &format!("<p>{}</p>", "\u{e9}x".repeat(150)))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "migrating, then the list",
            &[],
            &[],
            vec![
                r(503, r#"{"state":"migrating"}"#),
                r(503, r#"{"state":"migrating"}"#),
                text(LIST),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "migrating four times",
            &[],
            &[],
            vec![
                r(503, r#"{"state":"migrating"}"#),
                r(503, r#"{"state":"migrating"}"#),
                r(503, r#"{"state":"migrating"}"#),
                r(503, r#"{"state":"migrating"}"#),
            ],
            owner_and_port,
            1,
            4,
        ),
        case(
            "a 503 that is not migrating",
            &[],
            &[],
            vec![r(503, r#"{"error":{"code":"busy","hint":"later"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line",
            &["--json"],
            &[],
            vec![text(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no bulletin line when it failed",
            &[],
            &[],
            vec![r(
                400,
                r#"{"error":{"code":"x"},"bulletin":{"day":"2026-11-17","unread":true,"key":"cd34"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the cards' object, can't be requested",
            &[],
            &[],
            vec![r(
                403,
                r#"{"error":{"code":"x","hint":"no"},"permission":{"requestable":false,"hint":"Ask a person."}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the cards' object on a list: no line",
            &["--json"],
            &[],
            vec![text(
                r#"{"rows":[],"permission":{"command":"k2 permission request --scope a"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    // A body that is not JSON, cut at 200 characters in a UTF-8 locale.
    let utf8 = if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    };
    for (name, body) in [
        (
            "a UTF-8 locale: cut at 200 characters",
            format!("<p>{}</p>", "\u{e9}x".repeat(150)),
        ),
        (
            "a UTF-8 locale: a character of three bytes counts once",
            format!("{}{}", "\u{2192}".repeat(10), "y".repeat(300)),
        ),
    ] {
        let c = case(
            name,
            &[],
            &[("LC_ALL", utf8)],
            vec![r(502, &body)],
            owner_and_port,
            1,
            1,
        );
        rig.check(&c);
    }
}

/// The passport: a re-minted one from the start, a heal, a dead one.
#[test]
fn the_passport() {
    let rig = Rig::new(VERB, "passport");
    let v = vec![
        agent(
            &[],
            case(
                "a re-minted passport is used from the start",
                &[],
                &[],
                vec![text(LIST)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        agent(
            &[],
            case(
                "a refused passport heals once",
                &[],
                &[],
                vec![heal(&rig), text(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        agent(
            &[],
            case(
                "a refused passport heals once, --json",
                &["--json"],
                &[],
                vec![heal(&rig), text(STATUS)],
                port_only,
                0,
                2,
            ),
        ),
        agent(
            &[],
            case(
                "a dead passport: the probe, then the restart line",
                &[],
                &[],
                vec![refusal(403, REFUSED), refusal(403, REFUSED)],
                port_only,
                3,
                2,
            ),
        ),
        agent(
            &[],
            case(
                "a refused passport the probe finds good: no line",
                &[],
                &[],
                vec![refusal(403, REFUSED), text(r#"{"role":"canonical"}"#)],
                port_only,
                3,
                2,
            ),
        ),
        agent(
            &[("K2_NO_OWNER_FALLBACK", "1")],
            case(
                "the guard lets a passport through",
                &[],
                &[],
                vec![text(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "the owner's refusal text is no passport",
            &[],
            &[],
            vec![refusal(403, REFUSED)],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The list from a daemon before `?format=text` (it answers JSON), read by
/// the shell's Python; `tests/cli/setup_dev_tools.sh`'s rows among them.
#[test]
fn an_older_daemon_s_json() {
    let rig = Rig::new(VERB, "older");
    let rest = r#"{"id":"llm_sign_in","state":"todo","opens":"llm_sign_in"},{"id":"first_agent","state":"todo","opens":"add_workspace"}"#;
    let with = |dev: &str| {
        format!(r#"{{"enabled": true, "hidden": false, "finished": false, "rows": [{dev}{rest}]}}"#)
    };
    let bodies: Vec<(&'static str, String, i32)> = vec![
        ("mac, git missing", with(r#"{"id":"dev_tools","state":"todo","opens":"dev_tools"},"#), 0),
        ("installing", with(r#"{"id":"dev_tools","state":"todo","opens":"dev_tools","note":"installing"},"#), 0),
        ("done", with(r#"{"id":"dev_tools","state":"done","opens":"dev_tools"},"#), 0),
        ("linux", with(r#"{"id":"dev_tools","state":"todo","opens":null,"note":"sudo apt install git"},"#), 0),
        ("skipped", with(r#"{"id":"dev_tools","state":"skipped","opens":"dev_tools"},"#), 0),
        ("git runs: no row", with(""), 0),
        ("every line", r#"{"enabled":false,"hidden":true,"finished":true,"rows":[{"id":"phone","state":"todo","note":"needs_tunnel"},{"id":"chief_of_staff","state":"todo","note":"needs_kessel"},{"id":"own_sign_in","state":"odd"},{"id":7},{"state":"done"},{"id":"tunnel","state":"todo","note":"odd note"},{"id":"dev_tools","note":5}],"kessel":{"name":"Kessel","path":"/k/kessel"},"offer":"meet_kessel"}"#.to_string(), 0),
        ("hidden, not finished", r#"{"enabled":true,"hidden":1,"rows":[]}"#.to_string(), 0),
        ("an error object", r#"{"error":{"code":"usage","hint":"no"}}"#.to_string(), 2),
        ("an error string", r#"{"error":"Invalid TOKEN"}"#.to_string(), 3),
        ("no rows", r#"{"ok":true,"note":"first_run_person_only here"}"#.to_string(), 3),
        ("not JSON after all", "{not json".to_string(), 1),
        ("leading spaces", format!("\n  {STATUS}"), 0),
        ("unicode", r#"{"enabled":true,"rows":[{"id":"café →","state":"todo","note":"x"}],"kessel":{"name":"Ké","path":"/é"}}"#.to_string(), 0),
    ];
    let mut v = Vec::new();
    for (name, body, code) in bodies {
        let name: &'static str = Box::leak(format!("older daemon: {name}").into_boxed_str());
        v.push(case(
            name,
            &[],
            &[],
            vec![text(&body)],
            owner_and_port,
            code,
            1,
        ));
    }
    let raises: Vec<(&'static str, &'static str)> = vec![
        (
            "a code that is a number",
            r#"{"error":{"code":5,"hint":"h"}}"#,
        ),
        (
            "a hint that is a list",
            r#"{"error":{"code":"c","hint":["c"]}}"#,
        ),
        (
            "a hint that is a number",
            r#"{"error":{"code":"c","hint":7}}"#,
        ),
        ("rows a number", r#"{"rows":5}"#),
        ("rows a string", r#"{"rows":"ab"}"#),
        ("a state that is a list", r#"{"rows":[{"state":["x"]}]}"#),
        ("an id that is an object", r#"{"rows":[{"id":{"a":1}}]}"#),
        (
            "a note that is a list",
            r#"{"rows":[{"id":"x","note":[1]}]}"#,
        ),
        ("kessel a list", r#"{"rows":[],"kessel":[1]}"#),
    ];
    for (name, body) in raises {
        let name: &'static str = Box::leak(format!("older daemon raises: {name}").into_boxed_str());
        v.push(Case {
            compare: Compare::Traceback,
            ..case(name, &[], &[], vec![text(body)], owner_and_port, 1, 1)
        });
    }
    for c in v {
        rig.check(&c);
    }
}

/// No answer, and a retried connect.
#[test]
fn no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let v = vec![
        case(
            "the daemon closes with nothing (52)",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset (56)",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP (status 000)",
            &[],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx with an empty body",
            &[],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable: the list",
            &[],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable: a refusal",
            &[],
            &[],
            vec![refusal(403, PERSON_ONLY)],
            owner_and_port,
            3,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable: a reset",
            &[],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a port that is no port",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    // A refused connect, then the daemon on the port it published: the
    // shell's answer carries the try's 000 above it.
    let dead = dead_port();
    let published = |r: &Rig| {
        r.owner_on_disk();
        std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub().port)).unwrap();
    };
    for (name, args, reply) in [
        ("a moved port: the stray 000 line", &[][..], text(LIST)),
        ("a moved port: --json", &["--json"][..], text(STATUS)),
        (
            "a moved port: a refusal",
            &[][..],
            refusal(403, PERSON_ONLY),
        ),
    ] {
        let c = case(
            name,
            args,
            &[("K2_PORT", dead.as_str())],
            vec![reply],
            published,
            0,
            1,
        );
        let code = if name.ends_with("refusal") { 1 } else { 0 };
        rig.check(&Case { code, ..c });
    }
}

/// The prelude and the words: each stops before anything is sent.
#[test]
fn usage_and_the_prelude() {
    let rig = Rig::new(VERB, "usage");
    let mut v = Vec::new();
    let usage: Vec<(&'static str, &'static [&'static str])> = vec![
        ("skip without a row", &["skip"]),
        ("skip with an empty row", &["skip", ""]),
        (
            "chief-of-staff without --about or --tool",
            &["chief-of-staff", "--name", "Ada"],
        ),
        (
            "chief-of-staff without --tool",
            &["chief-of-staff", "--name", "Ada", "--about", "x"],
        ),
        ("--tool off kessel", &["hide", "--tool", "claude"]),
        ("--tool with no verb", &["--tool", "claude"]),
        (
            "--focus-group off kessel",
            &["skip", "dev_tools", "--focus-group", "Ops"],
        ),
        ("--name with no value", &["kessel", "--name"]),
        ("an unknown flag", &["--bogus"]),
        ("an empty word", &[""]),
        ("k5 is no verb", &["k5"]),
        ("kessel --about", &["kessel", "--about", "x"]),
        (
            "chief-of-staff --handle",
            &["chief-of-staff", "--handle", "x"],
        ),
        ("two verbs", &["hide", "show"]),
        ("skip after a verb", &["kessel", "skip", "x"]),
        ("a verb after skip", &["skip", "x", "dismiss-offer"]),
        ("--json then a bad word", &["--json", "nope"]),
    ];
    for (name, args) in usage {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.extend([
        case(
            "the gate comes before the words",
            &["--bogus"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case("no port", &[], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "chief-of-staff in a K2 terminal with no passport",
            &[
                "chief-of-staff",
                "--name",
                "A",
                "--about",
                "b",
                "--tool",
                "claude",
            ],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "chief-of-staff's tool before its words",
            &["chief-of-staff"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        // 0.46.17: the locked tool after leading flags is still locked.
        case(
            "the locked tool after a leading flag, no passport: refused",
            &["--json", "chief-of-staff"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "the locked tool after a value flag, no passport: refused",
            &["--tool", "claude", "chief-of-staff"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ]);
    for c in v {
        rig.check(&c);
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn setup_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "help as a word",
            &["help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "help after --json",
            &["--json", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "help as a value",
            &["kessel", "--name", "help"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "a bad word, then help",
            &["--bogus", "help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "a host that is not plain",
            &[],
            &[("K2_HOST", "::1")],
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

/// send-feedback's settings as a 0.46.17 daemon answers them.
const FEEDBACK_SETTINGS: &str = r#"{"enabled":true,"agents":true,"agentPlatform":false,"agentAttach":["computer"],"agentDiagnostics":false,"agentScopes":["cli"],"agentsAsked":true,"agentsAskedAt":1763538000000,"agentsAskedBy":"owner","attachParts":[{"id":"computer","label":"Computer size and free space"},{"id":"log-tail","label":"The last lines of the log"}],"rememberDiagnostics":false,"canEdit":true,"canSend":true,"endpoint":"https://feedback.example.test","contactAddress":"","pending":0,"service":{},"senderKind":"person","ok":true}"#;
/// The same after `--agents all --attach computer,log-tail`.
const FEEDBACK_ALL: &str = r#"{"enabled":true,"agents":true,"agentPlatform":true,"agentAttach":["computer","log-tail"],"agentDiagnostics":false,"agentScopes":["cli","platform"],"agentsAsked":true,"agentsAskedAt":1763538000000,"agentsAskedBy":"owner","attachParts":[{"id":"computer","label":"Computer size and free space"},{"id":"log-tail","label":"The last lines of the log"}],"canSend":true,"endpoint":"https://feedback.example.test","pending":2,"senderKind":"person","ok":true}"#;

/// `setup agent-reports` (send-feedback's settings, 0.46.17) is on the
/// binary's hand-off list ([`k5_cli::families::setup::SHELL_VERBS`]):
/// both sides are the shell's, byte for byte, for each of its forms. With
/// no python3 the binary hands nothing over: the missing-Python line,
/// exit 69, nothing sent (the rig's third run). After another verb it is
/// the binary's usage.
#[test]
fn setup_agent_reports_goes_to_the_shell() {
    let rig = Rig::new(VERB, "agent-reports");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let people_only = || {
        refusal(
            403,
            r#"{"ok":false,"error":"settings_people_only","message":"Only a person signed in to this server changes these settings."}"#,
        )
    };
    let v = vec![
        // covers: setup agent-reports
        shell(case(
            "agent-reports: the read",
            &["agent-reports"],
            &[],
            vec![text(FEEDBACK_SETTINGS)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "agent-reports --agents all --attach: the version read, then the write",
            &[
                "agent-reports",
                "--agents",
                "all",
                "--attach",
                "computer,log-tail",
            ],
            &[],
            vec![text(FEEDBACK_SETTINGS), text(FEEDBACK_ALL)],
            owner_and_port,
            0,
            2,
        )),
        // covers: setup agent-reports --json
        shell(case(
            "agent-reports --json",
            &["agent-reports", "--json"],
            &[],
            vec![text(FEEDBACK_SETTINGS)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "--json before agent-reports",
            &["--json", "agent-reports", "--agents", "off"],
            &[],
            vec![text(FEEDBACK_SETTINGS)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "agent-reports: a usage error, nothing sent",
            &["agent-reports", "--enabled", "off"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "agent-reports --agents maybe: a usage error",
            &["agent-reports", "--agents", "maybe"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "agent-reports refused (403): exit 3",
            &["agent-reports", "--agents", "off"],
            &[],
            vec![people_only()],
            owner_and_port,
            3,
            1,
        )),
        shell(case(
            "agent-reports, a bad part (400): exit 2",
            &["agent-reports", "--attach", "nope"],
            &[],
            vec![
                text(FEEDBACK_SETTINGS),
                refusal(
                    400,
                    r#"{"ok":false,"error":"bad_attach_part","message":"nope isn't a part agents may attach."}"#,
                ),
            ],
            owner_and_port,
            2,
            2,
        )),
        shell(agent(
            &[],
            case(
                "an agent's agent-reports: refused, exit 3",
                &["agent-reports", "--agents", "cli"],
                &[],
                vec![text(FEEDBACK_SETTINGS), people_only()],
                port_only,
                3,
                2,
            ),
        )),
        case(
            "agent-reports after a verb: the usage",
            &["hide", "agent-reports"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a mistake before agent-reports is the binary's",
            &["--bogus", "agent-reports"],
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

const MADE: &str = "Made Kessel: Kessel (kessel) at /home/me/k2-agents/kessel\nKessel is starting and will say hello on its Thread: k2 thread kessel\n";

/// Each write's request and what it prints, the bodies of
/// `tests/cli/setup_no_python.sh` among them.
#[test]
fn the_writes() {
    let rig = Rig::new(VERB, "writes");
    let r = |s: u16, b: &str| refusal(s, b);
    let v = vec![
        // covers: setup hide
        case(
            "hide",
            &["hide"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup show
        case(
            "show",
            &["show"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup dismiss-offer
        case(
            "dismiss-offer",
            &["dismiss-offer"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup hide --json
        case(
            "hide --json",
            &["hide", "--json"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup show --json
        case(
            "show --json",
            &["show", "--json"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup dismiss-offer --json
        case(
            "--json dismiss-offer",
            &["--json", "dismiss-offer"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup kessel --json
        case(
            "kessel --json with a flag",
            &["kessel", "--json", "--name", "Kes"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup skip --json
        case(
            "--json skip",
            &["--json", "skip", "dev_tools"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup skip
        case(
            "skip",
            &["skip", "dev_tools"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "skip: the row escaped",
            &["skip", "od\"d\\row"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "skip: control bytes and UTF-8",
            &["skip", "a\u{1}\u{7f}\tb\u{e9}\u{2192}"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "skip: a row like a flag",
            &["skip", "--json"],
            &[],
            vec![text(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup kessel
        case(
            "kessel: no flags",
            &["kessel"],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kessel: every flag",
            &[
                "kessel",
                "--tool",
                "claude",
                "--name",
                "Kay",
                "--handle",
                "kay",
                "--focus-group",
                "Ops",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kessel: the fields escaped",
            &[
                "kessel",
                "--tool",
                "claude",
                "--name",
                "K \"Five\" \\ 5",
                "--handle",
                "kessel",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kessel: the last of a flag",
            &["kessel", "--name", "a", "--name", "b"],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kessel: an empty focus group is left out",
            &["kessel", "--focus-group", ""],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "kessel: a focus group escaped",
            &[
                "kessel",
                "--tool",
                "claude",
                "--handle",
                "kessel",
                "--focus-group",
                "Bo\"s \\ crew",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup chief-of-staff
        case(
            "chief-of-staff: about escaped",
            &[
                "chief-of-staff",
                "--name",
                "Ada",
                "--about",
                "runs \"ops\"\tand\nbooks",
                "--tool",
                "codex",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chief-of-staff: an empty about is sent",
            &[
                "chief-of-staff",
                "--name",
                "Ada",
                "--about",
                "",
                "--tool",
                "codex",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chief-of-staff: a focus group after about",
            &[
                "chief-of-staff",
                "--name",
                "Ada",
                "--about",
                "",
                "--tool",
                "codex",
                "--focus-group",
                "fg_\"1\"",
            ],
            &[],
            vec![text(MADE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: setup chief-of-staff --json
        case(
            "chief-of-staff --json",
            &[
                "chief-of-staff",
                "--json",
                "--name",
                "A",
                "--about",
                "b",
                "--tool",
                "grok",
            ],
            &[],
            vec![text(r#"{"workspaceId":"w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "skip: unknown_row",
            &["skip", "nope"],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"unknown_row","hint":"\"nope\" is not a row on your setup list (k2 setup shows them)"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "kessel: tool_not_signed_in",
            &["kessel", "--tool", "grok"],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"tool_not_signed_in","hint":"grok isn't signed in on this server: sign in first"}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "kessel: off",
            &["kessel"],
            &[],
            vec![r(
                404,
                r#"{"ok":false,"error":{"code":"kessel_first_run_off","hint":"Kessel at first run is off on this server (the owner turns on kesselFirstRun)"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "kessel: a handle taken",
            &["kessel"],
            &[],
            vec![r(
                409,
                r#"{"ok":false,"error":{"code":"kessel_handle_taken","hint":"the handles kessel and kessel5 are both taken by other agents: pass another with --handle (k2 setup kessel --handle <h>), or free one of them"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "chief-of-staff: a shell tab is a person",
            &[
                "chief-of-staff",
                "--name",
                "A",
                "--about",
                "b",
                "--tool",
                "claude",
            ],
            &[],
            vec![r(
                403,
                r#"{"ok":false,"error":{"code":"kessel_session_only","hint":"a shell tab is a person: ask Kessel on its Thread to hire your Chief of Staff"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "kessel: no such focus group",
            &["kessel", "--focus-group", "Nope"],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"unknown_focus_group","hint":"no focus group is \"Nope\": nothing was made (Settings → Workspaces lists them)"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "chief-of-staff: two focus groups",
            &[
                "chief-of-staff",
                "--name",
                "A",
                "--about",
                "b",
                "--tool",
                "claude",
                "--focus-group",
                "Ops",
            ],
            &[],
            vec![r(
                400,
                r#"{"ok":false,"error":{"code":"ambiguous_focus_group","hint":"more than one focus group is named \"Ops\": send its id"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "hide: a refusal with the cards' object",
            &["hide"],
            &[],
            vec![r(403, PERMISSION_ASK)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "kessel: an older daemon's JSON",
            &["kessel", "--tool", "codex"],
            &[],
            vec![text(
                r#"{"workspaceId":"w1","name":"Kessel","handle":"kessel","path":"/a/kessel","permissions":{"granted":["msg","inbox"],"left":[{"action":"compute.run","reason":"owner asks"},{"action":"vault.read"}]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chief-of-staff: an older daemon's JSON",
            &[
                "chief-of-staff",
                "--name",
                "Ada",
                "--about",
                "x",
                "--tool",
                "codex",
            ],
            &[],
            vec![text(
                r#"{"workspaceId":"w2","name":"Ada","handle":"ada","path":"/a/ada"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "skip: an older daemon's list",
            &["skip", "phone"],
            &[],
            vec![text(STATUS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "hide: an older daemon's answer with no rows",
            &["hide"],
            &[],
            vec![text(r#"{"ok":true}"#)],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "kessel: an older daemon's granted list of numbers",
                &["kessel"],
                &[],
                vec![text(r#"{"workspaceId":"w","permissions":{"granted":[1]}}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "kessel: an older daemon's left list of strings",
                &["kessel"],
                &[],
                vec![text(r#"{"workspaceId":"w","permissions":{"left":["x"]}}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..case(
                "kessel: an older daemon's permissions a list",
                &["kessel"],
                &[],
                vec![text(r#"{"workspaceId":"w","permissions":[1]}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        case(
            "a POST is not asked again when migrating",
            &["hide"],
            &[],
            vec![r(503, r#"{"state":"migrating"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line after a write",
            &["hide", "--json"],
            &[],
            vec![text(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The writes with a passport: every verb sends it in a K2 terminal, even
/// with the owner token on disk (0.46.17, `_setup_token`).
#[test]
fn the_writes_with_a_passport() {
    let rig = Rig::new(VERB, "wpass");
    let chief: &'static [&'static str] = &[
        "chief-of-staff",
        "--name",
        "A",
        "--about",
        "b",
        "--tool",
        "claude",
    ];
    let sock = [("K2_HOOK_SOCK", "{sock}")];
    let v = vec![
        agent(
            &[],
            case(
                "chief-of-staff over TCP",
                chief,
                &[],
                vec![text(MADE)],
                port_only,
                0,
                1,
            ),
        ),
        agent(
            &sock,
            case(
                "chief-of-staff in a K2 terminal",
                chief,
                &[],
                vec![text(MADE)],
                port_only,
                0,
                1,
            ),
        ),
        agent(
            &sock,
            case(
                "chief-of-staff in a K2 terminal, the owner token on disk: the passport",
                chief,
                &[],
                vec![text(MADE)],
                owner_and_port,
                0,
                1,
            ),
        ),
        agent(
            &sock,
            case(
                "hide in a K2 terminal, the owner token on disk: the passport",
                &["hide"],
                &[],
                vec![text(LIST)],
                owner_and_port,
                0,
                1,
            ),
        ),
        agent(
            &[],
            case(
                "an agent's refusal",
                &["dismiss-offer"],
                &[],
                vec![refusal(403, PERSON_ONLY)],
                port_only,
                3,
                1,
            ),
        ),
        agent(
            &[],
            case(
                "a refused passport: the write once more, healed",
                &["skip", "phone"],
                &[],
                vec![heal(&rig), text(LIST)],
                port_only,
                0,
                2,
            ),
        ),
        agent(
            &[],
            case(
                "a dead passport after a write",
                &["hide"],
                &[],
                vec![refusal(403, REFUSED), refusal(403, REFUSED)],
                port_only,
                3,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// A write that may have gone out is never sent again: one request for
/// each way an answer fails to come, TMPDIR usable or not (`_k2_curl` has
/// no local check, so nothing becomes 75); only a refused connect, which
/// reached nothing, is tried again.
#[test]
fn a_write_is_sent_once() {
    let rig = Rig::new(VERB, "once");
    let mut v = Vec::new();
    for no_tmpdir in [false, true] {
        for (name, reply) in [
            ("an empty reply (52)", Reply::Close),
            ("a reset (56)", Reply::Reset),
            ("an empty 200", text("")),
            ("not HTTP (000)", Reply::Raw(GARBAGE.to_vec())),
            ("a 5xx with no body", Reply::Answer(502, String::new())),
        ] {
            for args in [&["hide"][..], &["kessel", "--name", "K"]] {
                let name: &'static str = Box::leak(
                    format!(
                        "{}{name}: {}",
                        if no_tmpdir { "TMPDIR unusable: " } else { "" },
                        args.join(" ")
                    )
                    .into_boxed_str(),
                );
                v.push(Case {
                    no_tmpdir,
                    ..case(name, args, &[], vec![reply.clone()], owner_and_port, 1, 1)
                });
            }
        }
    }
    for c in v {
        rig.check(&c);
    }
    let dead = dead_port();
    rig.check(&case(
        "nothing listens (7): nothing sent",
        &["skip", "phone"],
        &[("K2_PORT", dead.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    let published = |r: &Rig| {
        r.owner_on_disk();
        std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub().port)).unwrap();
    };
    rig.check(&case(
        "a refused connect, then the published port: sent once",
        &["hide"],
        &[("K2_PORT", dead.as_str())],
        vec![text(LIST)],
        published,
        0,
        1,
    ));
}

/// No answer in time (28): one request, the line, exit 1. Slow: 30 s a
/// side.
#[test]
fn a_write_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    let c = case(
        "no answer in time (28)",
        &["skip", "dev_tools"],
        &[],
        vec![Reply::Hang(std::time::Duration::from_secs(35))],
        owner_and_port,
        1,
        1,
    );
    rig.check(&c);
}
