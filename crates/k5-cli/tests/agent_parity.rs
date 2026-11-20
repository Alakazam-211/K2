//! `k5-cli agent ...` against the shell CLI's `cli/k2 agent ...` on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code, for the verbs the binary answers: `conf`, `get`, `list`,
//! `retire` and `restart`.
//!
//! - Help anywhere on a line (the shell's, nothing sent), and every
//!   verb's usage lines.
//! - `conf` and `get`: the conf by name, by path and from this folder, the
//!   persona file, each key, the daemon's errors (each exit), a refusal's
//!   permission card line, an answer that is not JSON, not ok or not a
//!   dict, no answer, no daemon.
//! - `list`: the table, `--json`, none, the errors.
//! - `retire` and `restart`: the plan, the change, `--json`, the refusals
//!   (exit 3), the daemon's errors, an HTTP error, no answer to the POST.
//! - A session with a passport: the named difference
//!   `owner-token-with-passport` (the shell sends the owner token).
//! - The named difference `agent-large-answer`: a list above what an
//!   environment variable may hold ([`a_list_above_the_environment_s_limit`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "agent";

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn leak(args: &[&str]) -> &'static [&'static str] {
    let owned: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(owned.into_boxed_slice())
}

const CONF: &str = r#"{"ok":true,"name":"qa","path":"/srv/qa","mode":"agent","enabled":true,"personaPath":null,"connections":[{"peer":"ops"},{"peer":"far","remote":true}],"projects":[{"name":"Acme","isPoc":true},{"name":"Globex"}],"live":{"active":true,"sessionId":"sess-0123456789","uptimeSec":3725},"charterProblems":[]}"#;

const CONF_IDLE: &str = r#"{"ok":true,"name":"","path":"/srv/idle-one","mode":"custom","enabled":false,"live":{"active":false},"charterProblems":[{"problem":"dangling","file":"CLAUDE.md","target":"AGENTS.md"},{"problem":"missing","file":"AGENTS.md"}]}"#;

const CONF_PERSONA: &str = r#"{"ok":true,"name":"qa","path":"/srv/qa","personaPath":"persona.md","live":{"active":true,"sessionId":"s1","uptimeSec":59.9}}"#;

fn persona(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("persona.md"), "# QA\r\nline two\rline three\n\u{e9}t\u{e9}").unwrap();
}

#[test]
fn help_and_the_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    // covers: agent conf
    // covers: agent get
    // covers: agent list
    // covers: agent retire
    // covers: agent restart
    for args in [
        &["conf", "--help"][..],
        &["get", "qa", "mode", "-h"],
        &["list", "--help"],
        &["retire", "qa", "-h"],
        &["retire", "qa", "--force", "--help"],
        &["retire", "--archive-to", "/x", "--help"],
        &["restart", "qa", "--help"],
        &["restart", "qa", "--harness", "codex", "-h"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    for args in [
        &["conf", "-x"][..],
        &["conf", "a", "b"],
        &["conf", "-"],
        &["conf", "a", "--json", "b"],
        &["get"],
        &["get", "qa"],
        &["get", "qa", "nope"],
        &["get", "qa", "mode", "x"],
        &["get", "--x"],
        &["get", "", "mode"],
        &["get", "qa", "mode", "--jsn"],
        &["list", "x"],
        &["list", "--x"],
        &["retire"],
        &["retire", "--archive-to"],
        &["retire", "qa", "b"],
        &["retire", "--nope"],
        &["retire", "--json"],
        &["restart"],
        &["restart", "qa", "--harness"],
        &["restart", "qa", "--harness", ""],
        &["restart", "qa", "--harness", "-c"],
        &["restart", "qa", "b"],
        &["restart", "-z"],
        &["restart", "--dry-run", "--json"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    v.push(case(
        "no daemon: the gate",
        &["list"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    check(&rig, v);
}

#[test]
fn conf_and_get() {
    let rig = Rig::new(VERB, "conf");
    let mut v = vec![
        // covers: agent conf
        case("by name", &["conf", "qa"], &[], vec![ok(CONF)], owner_and_port, 0, 1),
        // covers: agent conf --json
        case(
            "by name, --json",
            &["conf", "qa", "--json"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "this folder, no live session, charter problems",
            &["conf"],
            &[],
            vec![ok(CONF_IDLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a path, made absolute",
            &["conf", "./sub/../x y/"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a path from home",
            &["conf", "~/agents/qa"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the persona file's lines",
            &["conf", "qa"],
            &[],
            vec![ok(CONF_PERSONA)],
            persona,
            0,
            1,
        ),
        case(
            "a persona file that isn't there",
            &["conf", "qa"],
            &[],
            vec![ok(CONF_PERSONA)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not found: exit 4",
            &["conf", "nobody"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found","hint":"no agent 'nobody' — did you mean 'qa'?"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "ambiguous: exit 4",
            &["conf", "q"],
            &[],
            vec![ok(r#"{"error":{"code":"ambiguous","hint":"q is qa or qb"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an error string",
            &["get", "qa", "mode"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"boom"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal: the permission card's line",
            &["conf", "qa"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not ok",
            &["conf", "qa"],
            &[],
            vec![ok(r#"{"ok":false,"name":"qa"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["conf", "qa"],
            &[],
            vec![ok("<html>it's down</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["conf", "qa"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "not a dict",
            &["conf", "qa"],
            &[],
            vec![ok("[1, 2]")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an error that is not a dict",
            &["conf", "qa"],
            &[],
            vec![ok(r#"{"error":[1]}"#)],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "no answer",
            &["conf", "qa"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        // covers: agent get
        case("get mode", &["get", "qa", "mode"], &[], vec![ok(CONF)], owner_and_port, 0, 1),
        case("get enabled", &["get", "qa", "enabled"], &[], vec![ok(CONF_IDLE)], owner_and_port, 0, 1),
        case("get name", &["get", "qa", "name"], &[], vec![ok(CONF)], owner_and_port, 0, 1),
        case("get path", &["get", "qa", "path"], &[], vec![ok(CONF)], owner_and_port, 0, 1),
        case("get projects", &["get", "qa", "projects"], &[], vec![ok(CONF)], owner_and_port, 0, 1),
        case("get projects, none", &["get", "qa", "projects"], &[], vec![ok(CONF_IDLE)], owner_and_port, 0, 1),
        case("get a key the conf lacks", &["get", "qa", "mode"], &[], vec![ok(CONF_PERSONA)], owner_and_port, 0, 1),
        case("get persona", &["get", "qa", "persona"], &[], vec![ok(CONF_PERSONA)], persona, 0, 1),
        case(
            "get persona, none set",
            &["get", "qa", "persona"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "get persona, the file isn't there",
            &["get", "qa", "persona"],
            &[],
            vec![ok(CONF_PERSONA)],
            owner_and_port,
            1,
            1,
        ),
        // covers: agent get --json
        case(
            "get mode --json",
            &["get", "--json", "qa", "mode"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "get persona --json",
            &["get", "qa", "persona", "--json"],
            &[],
            vec![ok(CONF_PERSONA)],
            persona,
            0,
            1,
        ),
        case(
            "get projects --json",
            &["get", "qa", "projects", "--json"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "get, not found",
            &["get", "nobody", "mode"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found","hint":"no agent"}}"#)],
            owner_and_port,
            4,
            1,
        ),
    ];
    let port = dead_port();
    v.push(case(
        "no daemon",
        &["conf", "qa"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    v.push(Case {
        compare: Compare::OwnerTokenWithPassport,
        ..Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a session with a passport, the owner on disk (PRD 13.9 #4)",
                &["conf", "qa"],
                &[],
                vec![ok(CONF)],
                owner_and_port,
                0,
                1,
            )
        }
    });
    v.push(Case {
        env: vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
        ..case(
            "a session with no passport: the owner on disk",
            &["get", "qa", "mode"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        )
    });
    check(&rig, v);
}

/// One agent row.
fn agent_row(name: &str, mode: &str, enabled: bool, live: bool, path: &str) -> String {
    format!(
        r#"{{"name":"{name}","mode":"{mode}","enabled":{enabled},"live":{live},"path":"{path}"}}"#
    )
}

#[test]
fn list() {
    let rig = Rig::new(VERB, "list");
    let home = rig.home.to_string_lossy().into_owned();
    let rows = [
        agent_row("qa", "agent", true, true, &format!("{home}/agents/qa")),
        agent_row("operations", "custom", false, false, "/srv/ops"),
        agent_row("h\u{e9}", "agent", true, false, &home),
        agent_row("near", "agent", true, false, &format!("{home}x/near")),
    ]
    .join(",");
    let answer: &'static str = leak(&[&format!(r#"{{"ok":true,"agents":[{rows}]}}"#)])[0];
    let mut v = vec![
        // covers: agent list
        case("the table", &["list"], &[], vec![ok(answer)], owner_and_port, 0, 1),
        // covers: agent list --json
        case("--json", &["list", "--json"], &[], vec![ok(answer)], owner_and_port, 0, 1),
        case(
            "none",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true,"agents":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a row with no fields",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true,"agents":[{}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an error",
            &["list"],
            &[],
            vec![Reply::Answer(500, r#"{"error":{"code":"db","hint":"locked"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error string",
            &["list", "--json"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not ok",
            &["list"],
            &[],
            vec![ok(r#"{"ok":false}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal: the permission card's line",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case("not JSON", &["list"], &[], vec![ok("nope")], owner_and_port, 1, 1),
        case("an empty answer", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        traceback(case(
            "a row that is not a dict",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true,"agents":["qa"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "no answer",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
    ];
    let port = dead_port();
    v.push(case(
        "no daemon",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    v.push(Case {
        compare: Compare::OwnerTokenWithPassport,
        ..Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a session with a passport, the owner on disk (PRD 13.9 #4)",
                &["list"],
                &[],
                vec![ok(answer)],
                owner_and_port,
                0,
                1,
            )
        }
    });
    check(&rig, v);
}

/// `agent-large-answer`: the list above what an environment variable may
/// hold. Bash stops at its Python (exit 126, "Argument list too long");
/// the binary prints the table. Both send the one request.
#[test]
fn a_list_above_the_environment_s_limit() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..12_000)
        .map(|i| {
            agent_row(
                &format!("agent-{i:05}"),
                "agent",
                i % 3 == 0,
                i % 5 == 0,
                &format!("/srv/agents/agent-number-{i:05}"),
            )
        })
        .collect();
    let big: &'static str =
        leak(&[&format!(r#"{{"ok":true,"agents":[{}]}}"#, rows.join(","))])[0];
    assert!(big.len() > 1024 * 1024, "above macOS's whole ARG_MAX too");
    let c = Case {
        no_python3: NoPython3::Own,
        ..case(
            "list, an answer above 1 MiB",
            &["list"],
            &[],
            vec![ok(big)],
            owner_and_port,
            126,
            1,
        )
    };
    let mut logs = Vec::new();
    let mut outs = Vec::new();
    for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
        rig.prepare(side, &c);
        let out = rig.run(bash, &c);
        let sent: Vec<String> = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| {
                let head = String::from_utf8_lossy(&r.bytes).into_owned();
                fold_query_escapes(head.lines().next().unwrap_or(""))
            })
            .collect();
        logs.push(sent);
        outs.push(out);
    }
    let (b, r) = (&outs[0], &outs[1]);
    assert_eq!(logs[0].len(), 1, "bash's requests: {:?}", logs[0]);
    assert_eq!(logs[0], logs[1], "the requests");
    assert_eq!(b.code, Some(126), "bash: {b:?}");
    assert_eq!(b.stdout, "");
    assert!(
        b.stderr.contains("Argument list too long"),
        "bash: {}",
        b.stderr
    );
    assert_eq!(r.code, Some(0), "the binary: {:?}", r.code);
    assert_eq!(r.stderr, "");
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 12_001);
    assert_eq!(
        lines[0],
        "NAME         MODE   ENABLED  LIVE  PATH",
        "the head"
    );
    assert_eq!(
        lines[1],
        "agent-00000  agent  yes      yes   /srv/agents/agent-number-00000"
    );
    assert_eq!(
        lines[12_000],
        "agent-11999  agent  no       no    /srv/agents/agent-number-11999"
    );
}

const RETIRED: &str = r#"{"ok":true,"dryRun":false,"archivedTo":"/srv/.archive/qa-1","actions":[{"step":"stop-session","status":"done","sessionId":"sess-0123456789"},{"step":"remove-connections","status":"done","removed":3},{"step":"clean-dependents","status":"done","chatSessionNames":0},{"step":"archive-folder","status":"done","to":"/srv/.archive/qa-1"},{"step":"unregister","status":"failed"}]}"#;

const PLANNED: &str = r#"{"ok":true,"dryRun":true,"actions":[{"step":"stop-session","status":"pending"},{"step":"remove-connections","status":"pending","edges":2},{"step":"archive-folder","status":"skipped","reason":"--archive-to is a file"},{"step":"odd","status":"weird"}]}"#;

#[test]
fn retire() {
    let rig = Rig::new(VERB, "retire");
    let refused = r#"{"refused":true,"dryRun":true,"error":{"code":"dirty","hint":"uncommitted work in /srv/qa"}}"#;
    let v = vec![
        // covers: agent retire
        case(
            "retired, archived",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), ok(RETIRED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the plan",
            &["retire", "qa", "--dry-run", "--force"],
            &[],
            vec![ok(CONF), ok(PLANNED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "retired, nothing archived, the name from the path",
            &["retire", "/srv/idle-one", "--archive-to", "~/old"],
            &[],
            vec![ok(CONF_IDLE), ok(r#"{"ok":true,"actions":[]}"#)],
            owner_and_port,
            0,
            2,
        ),
        // covers: agent retire --json
        case(
            "--json",
            &["retire", "qa", "--json", "--archive-to", "rel/dir"],
            &[],
            vec![ok(CONF), ok(RETIRED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "refused: exit 3",
            &["retire", "qa", "--dry-run"],
            &[],
            vec![ok(CONF), Reply::Answer(409, refused.into())],
            owner_and_port,
            3,
            2,
        ),
        case(
            "refused, --json",
            &["retire", "qa", "--json"],
            &[],
            vec![ok(CONF), ok(refused)],
            owner_and_port,
            3,
            2,
        ),
        case(
            "refused with no hint",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), ok(r#"{"refused":true}"#)],
            owner_and_port,
            3,
            2,
        ),
        case(
            "the conf not found: exit 4",
            &["retire", "nobody"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found","hint":"no agent"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "the retire not found: exit 4",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(404, r#"{"error":{"code":"not_found","hint":"gone"}}"#.into())],
            owner_and_port,
            4,
            2,
        ),
        case(
            "an error string",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(500, r#"{"error":"disk full"}"#.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an HTTP error that is not JSON",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(502, "bad gateway".into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "not ok",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), ok(r#"{"ok":false,"why":"é"}"#)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an answer that is not JSON",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), ok("nope")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "no answer to the POST",
            &["retire", "qa"],
            &[],
            vec![ok(CONF), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        traceback(case(
            "a conf with no path",
            &["retire", "qa"],
            &[],
            vec![ok(r#"{"ok":true,"name":"qa"}"#)],
            owner_and_port,
            1,
            1,
        )),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["retire", "qa", "--dry-run"],
                    &[],
                    vec![ok(CONF), ok(PLANNED)],
                    owner_and_port,
                    0,
                    2,
                )
            }
        },
    ];
    check(&rig, v);
}

const RESTARTED: &str = r#"{"ok":true,"sessionId":"sess-new","claudeSessionId":"conv-1","resumedExisting":true,"provider":"claude"}"#;

#[test]
fn restart() {
    let rig = Rig::new(VERB, "restart");
    let choice = r#"{"error":{"code":"harness_choice_needed","harnesses":[{"harness":"claude","installed":true},{"harness":"codex"},{"harness":""}]}}"#;
    let v = vec![
        // covers: agent restart
        case(
            "restarted",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), ok(RESTARTED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "started, a new conversation",
            &["restart", "/srv/idle-one", "--harness", " codex "],
            &[],
            vec![
                ok(CONF_IDLE),
                ok(r#"{"sessionId":"s2","claudeSessionId":"c2","resumedExisting":false}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "started, no conversation",
            &["restart", "qa", "--harness=grok"],
            &[],
            vec![ok(CONF_IDLE), ok(r#"{"sessionId":"s3"}"#)],
            owner_and_port,
            0,
            2,
        ),
        // covers: agent restart --json
        case(
            "--json",
            &["restart", "qa", "--json"],
            &[],
            vec![ok(CONF), ok(RESTARTED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the plan",
            &["restart", "qa", "--dry-run"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the plan, no live session",
            &["restart", "qa", "--dry-run"],
            &[],
            vec![ok(CONF_IDLE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the plan, --json",
            &["restart", "qa", "--dry-run", "--json"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the first chat waits for a pick",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(409, choice.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "a pick with no harnesses named",
            &["restart", "qa"],
            &[],
            vec![
                ok(CONF),
                Reply::Answer(409, r#"{"error":{"code":"harness_choice_needed"}}"#.into()),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an HTTP error with a hint",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(500, r#"{"error":{"hint":"spawn failed"}}"#.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an HTTP error with a message",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(500, r#"{"error":{"message":"no pty"}}"#.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an HTTP error that is not JSON",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), Reply::Answer(503, "down for a bit".into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an answer with no session",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), ok(r#"{"ok":true}"#)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "an answer that is not JSON",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), ok("nope")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "no answer to the POST",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "the conf not found: exit 4",
            &["restart", "nobody", "--dry-run"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found","hint":"no agent"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        traceback(case(
            "an answer that is not a dict",
            &["restart", "qa"],
            &[],
            vec![ok(CONF), ok("[]")],
            owner_and_port,
            1,
            2,
        )),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["restart", "qa"],
                    &[],
                    vec![ok(CONF), ok(RESTARTED)],
                    owner_and_port,
                    0,
                    2,
                )
            }
        },
    ];
    check(&rig, v);
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    for (args, want, code, replies) in [
        (
            &["list", "x"][..],
            format!("{{\"error\": {{\"code\": \"bad_usage\", \"hint\": \"unexpected argument 'x' \\u2014 usage: {cli} agent list [--json]\"}}}}\n"),
            2,
            vec![],
        ),
        (
            &["get", "qa"],
            format!("{{\"error\": {{\"code\": \"bad_usage\", \"hint\": \"usage: {cli} agent get <name> <key> \\u2014 keys: mode, enabled, name, path, persona\"}}}}\n"),
            2,
            vec![],
        ),
        (
            &["restart"],
            format!("{{\"error\": {{\"code\": \"bad_usage\", \"hint\": \"missing <name> \\u2014 usage: {cli} agent restart <name> [flags]\"}}}}\n"),
            2,
            vec![],
        ),
        (
            &["conf", "qa"],
            format!("{{\"error\": {{\"code\": \"daemon_unreachable\", \"hint\": \"no response from the {product} daemon (is it running?)\"}}}}\n"),
            1,
            vec![ok("")],
        ),
    ] {
        let sent = replies.len();
        let c = case("brand", args, &[], replies, owner_and_port, code, sent);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert_eq!(log.len(), sent, "{args:?}");
    }
    let c = case(
        "brand",
        &["list"],
        &[],
        vec![ok(r#"{"ok":true,"agents":[]}"#)],
        owner_and_port,
        0,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!("No agents registered on this server. Hire one: {cli} agent hire <dir>\n")
    );
}

const LAYERS: &str = r#"{"ok":true,"pinned":[{"id":"pinned:agent","label":"Agent (persona)","path":".k2/agent/ROLE.md"},{"id":"pinned:project","label":"Project","path":".k2/PROJECT.md","enabled":"on"},{"id":"pinned:tooling","label":"Tooling","enabled":false}],"layers":[{"id":"lay-b","position":"1","path":"docs/runbook.md","label":"Runbook","enabled":true},{"id":"lay-a","position":0,"source":"catalog:wiki-index","exists":false,"path":".k2/wiki/_Index.md"},{"id":"lay-c","source":"preset:hygiene","path":"notes/a.long.name.md","enabled":0},{"id":"lay-d-0123456789abcdef"}]}"#;

const CATALOG: &str = r#"{"ok":true,"catalog":[{"id":"wiki:index","label":"Wiki index","path":".k2/wiki/_Index.md","description":"  The vault's map. "},{"id":"team:roster","source":"connections-roster","name":"Roster"},{"id":"manager:pack","kind":" static ","defaultPath":"catalog/manager.md"},{"id":"x"},"a bare entry"]}"#;

#[test]
fn context_words() {
    let rig = Rig::new(VERB, "ctxwords");
    let mut v = vec![
        // covers: agent context
        case(
            "the family's help",
            &["context"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: agent context --json
        case(
            "--json alone: an unknown verb",
            &["context", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for args in [
        &["context", "--help"][..],
        &["context", "list", "-h"],
        &["context", "add", "x.md", "--help"],
        &["context", "remove", "x", "-h"],
        &["context", "on", "agent", "--help"],
        &["context", "off", "-h"],
        &["context", "move", "a", "up", "-h"],
        &["context", "regen", "--help"],
        &["context", "show", "-h"],
        &["context", "catalog", "--help"],
        &["context", "--workspace", "x", "rm", "y", "--help"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    for args in [
        &["context", "nope"][..],
        &["context", "list", "x"],
        &["context", "list", "--x"],
        &["context", "add"],
        &["context", "add", "--label"],
        &["context", "add", "a", "b"],
        &["context", "add", "-q"],
        &["context", "remove"],
        &["context", "rm", "a", "b"],
        &["context", "delete", "--f"],
        &["context", "on"],
        &["context", "off", "--x"],
        &["context", "move", "a"],
        &["context", "move", "a", "sideways"],
        &["context", "move", "a", "-1"],
        &["context", "move", "a", "b", "c"],
        &["context", "show", "x"],
        &["context", "show", "--y"],
        &["context", "regen", "x"],
        &["context", "catalog", "--z"],
        &["context", "list", "--workspace"],
        &["context", "--project"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn context_reads() {
    let rig = Rig::new(VERB, "ctxread");
    let v = vec![
        // covers: agent context list
        case("list", &["context", "list"], &[], vec![ok(LAYERS)], owner_and_port, 0, 1),
        // covers: agent context list --json
        case(
            "list --json",
            &["context", "list", "--json"],
            &[],
            vec![ok(leak(&[&format!("{LAYERS}\n\n")])[0])],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, nothing set",
            &["context", "list"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, soft warnings",
            &["context", "list"],
            &[],
            vec![ok(r#"{"ok":true,"softWarn":["AGENTS.md is 40 KB",{"a":1}],"layers":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, a soft warning",
            &["context", "list", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"warn":{"message":"big"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a workspace folder",
            &["context", "--workspace", "/tmp", "list"],
            &[],
            vec![ok(LAYERS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a workspace the daemon resolves",
            &["context", "list", "--project=acme"],
            &[],
            vec![ok(r#"{"ok":true,"path":"/srv/acme"}"#), ok(LAYERS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "a workspace it doesn't know",
            &["context", "--workspace", "ghost", "show"],
            &[],
            vec![ok(r#"{"ok":false}"#), ok(r#"{"ok":true,"content":"x"}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "PROJECT_PATH names the workspace",
            &["context", "list"],
            &[("K2_PROJECT_PATH", "~/w/../acme")],
            vec![ok(LAYERS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent context catalog
        case("catalog", &["context", "catalog"], &[], vec![ok(CATALOG)], owner_and_port, 0, 1),
        // covers: agent context catalog --json
        case(
            "catalog --json",
            &["context", "catalog", "--json"],
            &[],
            vec![ok(CATALOG)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "catalog, none",
            &["context", "catalog"],
            &[],
            vec![ok(r#"{"ok":true,"catalog":[],"softWarn":"not shown"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent context show
        case(
            "show",
            &["context", "show"],
            &[],
            vec![ok(r##"{"ok":true,"content":"# AGENTS\n\nbody"}"##)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show --outline",
            &["context", "show", "--outline"],
            &[],
            vec![ok(r#"{"ok":true,"outline":[{"level":1,"title":"AGENTS"},{"depth":"3","heading":"Deep"},{"title":"No level"},"bare"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show --outline, none: the text",
            &["context", "show", "--outline"],
            &[],
            vec![ok(r#"{"ok":true,"outline":[],"text":"ends\n"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, the JSON when no text",
            &["context", "show"],
            &[],
            vec![ok(r#"{"ok":true,"layers":[1,{"a":"é"}],"empty":{}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent context show --json
        case(
            "show --json",
            &["context", "show", "--json", "--outline"],
            &[],
            vec![ok(r#"{"ok":true,"outline":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not found",
            &["context", "list"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"not_found","hint":"no workspace"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "refused",
            &["context", "show"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"Refused","hint":"no"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "bad usage",
            &["context", "catalog"],
            &[],
            vec![ok(r#"{"error":{"code":"catalog_unknown"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error string",
            &["context", "list"],
            &[],
            vec![ok(r#"{"error":"broken"}"#)],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "an error code that is not a string",
            &["context", "list"],
            &[],
            vec![ok(r#"{"error":{"code":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "an HTTP error with no body",
            &["context", "list"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["context", "list"],
            &[],
            vec![ok(" \n")],
            owner_and_port,
            1,
            1,
        ),
        case("not JSON", &["context", "list"], &[], vec![ok("<html>")], owner_and_port, 1, 1),
        case(
            "not ok",
            &["context", "list"],
            &[],
            vec![ok(r#"{"ok":false,"why":"x"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 404 with no context fields",
            &["context", "catalog"],
            &[],
            vec![Reply::Answer(404, r#"{"msg":"no route"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "no answer",
            &["context", "list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["context", "list"],
                    &[],
                    vec![ok(LAYERS)],
                    owner_and_port,
                    0,
                    1,
                )
            }
        },
    ];
    check(&rig, v);
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["context", "list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

#[test]
fn context_writes() {
    let rig = Rig::new(VERB, "ctxwrite");
    let done = r#"{"ok":true,"regenerated":true}"#;
    let v = vec![
        // covers: agent context add
        case(
            "add a path, a label",
            &["context", "add", "docs/runbook.md", "--label", "Runbook"],
            &[],
            vec![ok(r#"{"ok":true,"layer":{"id":"lay-b","label":"Runbook","path":"docs/runbook.md"},"regenerated":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add a catalog id",
            &["context", "add", "wiki:index"],
            &[],
            vec![ok(r#"{"ok":true,"added":{"id":"lay-w"},"path":".k2/wiki/_Index.md","regenerated":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add, the id as the path",
            &["context", "add", "~/notes"],
            &[],
            vec![ok(r#"{"ok":true,"id":"same","path":"same"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent context add --json
        case(
            "add --json",
            &["context", "add", "x.markdown", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"layer":{"id":"x"},"softWarn":"large"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add, a duplicate: exit 3",
            &["context", "add", "./a"],
            &[],
            vec![Reply::Answer(409, r#"{"error":{"code":"duplicate_layer","hint":"already there"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        // covers: agent context remove
        case(
            "remove a pinned layer, no lookup",
            &["context", "remove", "tooling"],
            &[],
            vec![ok(done)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove by label",
            &["context", "rm", "runbook"],
            &[],
            vec![ok(LAYERS), ok(done)],
            owner_and_port,
            0,
            2,
        ),
        // covers: agent context remove --json
        case(
            "remove by path, --json",
            &["context", "delete", "a.long.name.md", "--json"],
            &[],
            vec![ok(LAYERS), ok(done)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "remove, no such layer: exit 4",
            &["context", "remove", "nothing-here"],
            &[],
            vec![ok(LAYERS)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "remove, the lookup fails",
            &["context", "remove", "x"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        // covers: agent context on
        case("on a pinned layer", &["context", "on", "pinned:project"], &[], vec![ok(done)], owner_and_port, 0, 1),
        // covers: agent context on --json
        case(
            "on by id, --json",
            &["context", "on", "lay-d-0123456789abcdef", "--json"],
            &[],
            vec![ok(LAYERS), ok(done)],
            owner_and_port,
            0,
            2,
        ),
        // covers: agent context off
        case("off by source", &["context", "off", "hygiene"], &[], vec![ok(LAYERS), ok(done)], owner_and_port, 0, 2),
        // covers: agent context off --json
        case("off --json", &["context", "off", "agent", "--json"], &[], vec![ok(done)], owner_and_port, 0, 1),
        // covers: agent context move
        case("move up", &["context", "move", "Runbook", "up"], &[], vec![ok(LAYERS), ok(done)], owner_and_port, 0, 2),
        case("move to a place", &["context", "move", "lay-a", "007"], &[], vec![ok(LAYERS), ok(done)], owner_and_port, 0, 2),
        // covers: agent context move --json
        case(
            "move --json, refused",
            &["context", "move", "lay-a", "top", "--json"],
            &[],
            vec![ok(LAYERS), Reply::Answer(400, r#"{"error":{"code":"invalid_move","hint":"already top"}}"#.into())],
            owner_and_port,
            2,
            2,
        ),
        // covers: agent context regen
        case(
            "regen",
            &["context", "regen"],
            &[],
            vec![ok(r#"{"ok":true,"path":"/w/.k2/AGENTS.md","bytes":2048}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "regen, the default path",
            &["context", "regen"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent context regen --json
        case(
            "regen --json",
            &["context", "regen", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"size":3}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no answer to the POST",
            &["context", "regen"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
    ];
    check(&rig, v);
}

/// No answer on the left of the pipe into the template readers: the line,
/// then SIGPIPE. macOS's bash 3.2 runs the reader on the empty input
/// (`bash32-pipe-reader`).
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader {
            stdout: "",
            stderr: "Error: daemon answered: \n",
        },
        ..c
    }
}

const TEMPLATES: &str = r#"{"ok":true,"templates":[{"id":"chief-of-staff","name":"Chief of staff","summary":"Runs the office","kind":"system","aliases":["cos","chief"]},{"id":"researcher","name":"Researcher","summary":"Reads and reports"},{"id":"qa-bot","name":"QA","summary":"Tests things","origin":"library","author":"ana","version":3}],"problems":[{"dir":"/srv/t/bad","error":"template.toml: missing id"}]}"#;

const TEMPLATE: &str = r##"{"ok":true,"template":{"id":"qa-bot","name":"QA","version":3,"author":"ana","summary":"Tests things","aliases":["qa"],"agent":{"role":"tester","tool":"claude","model":"opus"},"params":{"team":{"label":"Team name","max":40,"required":true},"tone":{"label":"Tone","max":20}},"questions":[{"id":"stack","ask":"What stack?","fills":"role"},{"id":"repo","ask":"Which repo?","fills":"wiki:Work#Repos"},{"id":"x","ask":"Other?"}],"wiki":["Work"],"catalog":["wiki:index"],"skills":["k2-cli"],"heartbeats":[{"name":"nightly","frequency":"daily"}],"permissions":{"cards":true,"expanded":["compute.run"]},"hello":"Hi.\nI test.","versions":[1,2,3],"hash":"abc123","files":{"template.toml":"id = \"qa-bot\"\n\n","ROLE.md":"# QA\n"}}}"##;

#[test]
fn template_words() {
    let rig = Rig::new(VERB, "tplwords");
    let mut v = Vec::new();
    for args in [
        &["template"][..],
        &["template", "help"],
        &["templates", "--help"],
        &["template", "list", "-h"],
        &["template", "save", ".", "--id", "x", "--help"],
        &["template", "add", "d", "-h"],
        &["template", "remove", "x", "--help"],
        &["template", "upgrade", ".", "-h"],
        &["template", "diff", "--help"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    for args in [
        &["template", "nope"][..],
        &["template", "list", "x"],
        &["template", "show"],
        &["template", "show", "a", "b"],
        &["template", "show", "-x"],
        &["template", "save"],
        &["template", "save", "."],
        &["template", "save", ".", "--id"],
        &["template", "save", "--name"],
        &["template", "save", "--summary"],
        &["template", "save", "--wiki"],
        &["template", "save", "a", "b", "--id", "x"],
        &["template", "save", "--zz"],
        &["template", "add"],
        &["template", "add", "a", "b"],
        &["template", "add", "-q"],
        &["template", "remove"],
        &["template", "remove", "--version"],
        &["template", "remove", "x", "--version", "2a"],
        &["template", "rm", "a", "b"],
        &["template", "remove", "-x"],
        &["template", "diff", "a", "b"],
        &["template", "diff", "a", "--dry-run"],
        &["template", "upgrade"],
        &["template", "upgrade", "a", "b"],
        &["template", "upgrade", "-x"],
        &["template", "diff", "qa-bot", "x", "2"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    v.push(case(
        "add: not a folder",
        &["template", "add", "/no/such/template-dir"],
        &[],
        vec![],
        owner_and_port,
        4,
        0,
    ));
    check(&rig, v);
}

#[test]
fn template_reads() {
    let rig = Rig::new(VERB, "tplread");
    let v = vec![
        // covers: agent template list
        case("list", &["template", "list"], &[], vec![ok(TEMPLATES)], owner_and_port, 0, 1),
        case("list --all", &["templates", "list", "--all"], &[], vec![ok(TEMPLATES)], owner_and_port, 0, 1),
        // covers: agent template list --json
        case(
            "list --json",
            &["template", "list", "--json", "--all"],
            &[],
            vec![ok(TEMPLATES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, none",
            &["template", "list"],
            &[],
            vec![ok(r#"{"ok":true,"templates":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, refused",
            &["template", "list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"forbidden","hint":"owner only"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "list, a token problem",
            &["template", "list"],
            &[],
            vec![Reply::Answer(401, r#"{"error":"Invalid TOKEN"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "list, a refusal's permission card line",
            &["template", "list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case("list, not JSON", &["template", "list"], &[], vec![ok("<html>")], owner_and_port, 1, 1),
        traceback(case(
            "list, a row that is not a dict",
            &["template", "list"],
            &[],
            vec![ok(r#"{"templates":["x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        in_pipe(case(
            "list, no answer: the line, SIGPIPE",
            &["template", "list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        // covers: agent template show
        case("show", &["template", "show", "qa-bot"], &[], vec![ok(TEMPLATE)], owner_and_port, 0, 1),
        case(
            "show --files",
            &["template", "show", "--files", "qa bot/x"],
            &[],
            vec![ok(TEMPLATE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, a bare template",
            &["template", "show", "x"],
            &[],
            vec![ok(r#"{"ok":true,"template":{"id":"x"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template show --json
        case(
            "show --json",
            &["template", "show", "qa-bot", "--json"],
            &[],
            vec![ok(TEMPLATE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, unknown: exit 4",
            &["template", "show", "nope"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"unknown_template","hint":"no template nope"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "show, bad: exit 2",
            &["template", "show", "x"],
            &[],
            vec![ok(r#"{"error":{"code":"bad_template","hint":"broken toml"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["template", "list"],
                    &[],
                    vec![ok(TEMPLATES)],
                    owner_and_port,
                    0,
                    1,
                )
            }
        },
    ];
    check(&rig, v);
    let port = dead_port();
    rig.check(&case(
        "no daemon: the reader's line",
        &["template", "list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

const SAVED: &str = r#"{"ok":true,"id":"qa-bot","version":4,"author":"ana","path":"/srv/t/qa-bot/v4","files":[{"path":"template.toml","bytes":120},{"path":"ROLE.md","bytes":900}],"catalog":["wiki:index"],"heartbeats":["nightly"],"suggest":["compute.run"],"skipped":[{"what":"secret","name":".env","why":"secrets stay"}]}"#;

#[test]
fn template_writes() {
    let rig = Rig::new(VERB, "tplwrite");
    let plan = r#"{"ok":true,"name":"QA","id":"qa-bot","from":2,"to":3,"dryRun":true,"rows":[{"status":"unchanged","step":"x"},{"status":"update","step":"role","target":"ROLE.md","reason":"edited","proposed":"new line","diff":"-old\n+new"},{"status":"add","step":"wiki","target":"Work"}]}"#;
    let v = vec![
        // covers: agent template save
        case(
            "save this workspace",
            &["template", "save", ".", "--id", "qa-bot", "--name", "QA", "--summary", "", "--wiki", "Work", "--wiki", "Notes"],
            &[],
            vec![ok(SAVED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "save a handle, a dry run",
            &["template", "save", "acme", "--id", "x", "--dry-run"],
            &[],
            vec![ok(r#"{"ok":true,"dryRun":true,"id":"x","version":1,"author":"ana","path":"/srv/t/x/v1"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template save --json
        case(
            "save --json, a folder",
            &["template", "save", "/tmp", "--id", "x", "--json"],
            &[],
            vec![ok(SAVED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "save, a secret found",
            &["template", "save", ".", "--id", "x"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"secret_found","hint":"a GitHub token in ROLE.md"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "save, not the author",
            &["template", "save", ".", "--id", "x"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"not_author","message":"ana saved it"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        // covers: agent template add
        case(
            "add a folder",
            &["template", "add", "/tmp", "--dry-run"],
            &[],
            vec![ok(SAVED)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template add --json
        case(
            "add --json, no charter",
            &["template", "add", "/tmp", "--json"],
            &[],
            vec![ok(r#"{"error":{"code":"no_charter","hint":"no ROLE.md"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        // covers: agent template remove
        case(
            "remove one version",
            &["template", "remove", "qa-bot", "--version", "v02"],
            &[],
            vec![ok(r#"{"ok":true,"id":"qa-bot","removed":[2],"remaining":[1,3]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove all",
            &["template", "rm", "qa-bot"],
            &[],
            vec![ok(r#"{"ok":true,"id":"qa-bot","removed":[1,2,3]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template remove --json
        case(
            "remove --json, unknown",
            &["template", "remove", "nope", "--json"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"no_template","hint":"none"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        // covers: agent template upgrade
        case(
            "upgrade, the plan",
            &["template", "upgrade", "acme", "--dry-run"],
            &[],
            vec![ok(plan)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade, done",
            &["template", "upgrade", "acme"],
            &[],
            vec![ok(r#"{"ok":true,"name":"QA","id":"qa-bot","from":3,"to":3,"changed":false,"rows":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "upgrade, changed",
            &["template", "upgrade", "/tmp"],
            &[],
            vec![ok(r#"{"ok":true,"name":"QA","id":"qa-bot","from":2,"to":3,"changed":true,"rows":[{"status":"update","step":"role","target":"ROLE.md"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template upgrade --json
        case(
            "upgrade --json",
            &["template", "upgrade", "acme", "--json"],
            &[],
            vec![ok(plan)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template diff
        case(
            "diff a workspace",
            &["template", "diff", "acme"],
            &[],
            vec![ok(plan)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "diff two versions",
            &["template", "diff", "qa-bot", "v1", "3"],
            &[],
            vec![ok(r#"{"ok":true,"id":"qa-bot","from":1,"to":3,"files":[{"status":"changed","path":"ROLE.md","diff":"-a\r\n+b"},{"status":"added","path":"x.md"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent template diff --json
        case(
            "diff --json",
            &["template", "diff", "qa-bot", "1", "2", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"id":"qa-bot","from":1,"to":2,"files":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an answer the printer doesn't know",
            &["template", "upgrade", "acme"],
            &[],
            vec![ok(r#"{"ok":true,"odd":"é"}"#)],
            owner_and_port,
            0,
            1,
        ),
        in_pipe(case(
            "no answer: the line, SIGPIPE",
            &["template", "remove", "x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a session with a passport: the passport",
            &["template", "save", "acme", "--id", "x"],
            &[],
            vec![ok(SAVED)],
            owner_and_port,
            0,
            1,
        ),
    ];
    let mut v = v;
    let last = v.pop().unwrap();
    v.push(Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{nosock}"), ("K2_PROJECT_PATH", "/srv/acme")]),
        ..last
    });
    check(&rig, v);
}

const IDENTITY: &str = r##"{"ok":true,"handle":"qa","displayName":"QA Bot","color":"#33aa77","image":"data:image/png;base64,AA==","canEditIdentity":true,"canEditHandle":true}"##;

fn pictures(r: &Rig) {
    owner_and_port(r);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend((0u8..=255).cycle().take(3000));
    std::fs::write(r.proj.join("pic.png"), png).unwrap();
    std::fs::write(r.proj.join("pic.gif"), b"GIF89a....").unwrap();
    std::fs::write(r.proj.join("pic.webp"), b"RIFF\x10\0\0\0WEBPVP8 ").unwrap();
}

#[test]
fn identity() {
    let rig = Rig::new(VERB, "identity");
    let mut v = vec![
        // covers: agent identity
        case("read by name", &["identity", "qa"], &[], vec![ok(IDENTITY)], owner_and_port, 0, 1),
        case(
            "read this workspace",
            &["identity"],
            &[],
            vec![ok(r#"{"handle":"me","displayName":"","canEditIdentity":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read by path",
            &["identity", "sub/agent one"],
            &[],
            vec![ok(r#"{"handle":"one"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent identity --json
        case("read --json", &["identity", "qa", "--json"], &[], vec![ok(IDENTITY)], owner_and_port, 0, 1),
        case(
            "change the color and name",
            &["identity", "qa", "--color", " rgb(51, 170, 119) ", "--name", "QA Bot"],
            &[],
            vec![ok(r##"{"ok":true,"handle":"qa","changed":["name","color"],"displayName":"QA Bot","color":"#33aa77"}"##)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a color the daemon checks",
            &["identity", "qa", "--color", "1,2,300"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad_color","message":"use #rgb or #rrggbb"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty name, no change",
            &["identity", "--name", "", "qa"],
            &[],
            vec![ok(r#"{"ok":true,"handle":"qa","changed":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "clear the picture --json",
            &["identity", "qa", "--clear-image", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"handle":"qa","changed":["image"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a PNG picture",
            &["identity", "qa", "--image", "pic.png"],
            &[],
            vec![ok(r#"{"ok":true,"handle":"qa","changed":["image"],"image":"x"}"#)],
            pictures,
            0,
            1,
        ),
        case(
            "a WebP picture",
            &["identity", "--image", "pic.webp"],
            &[],
            vec![ok(r#"{"ok":true,"handle":"me","changed":["image"]}"#)],
            pictures,
            0,
            1,
        ),
        case(
            "a GIF: refused here",
            &["identity", "qa", "--image", "pic.gif"],
            &[],
            vec![],
            pictures,
            2,
            0,
        ),
        case(
            "not found",
            &["identity", "nobody"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not_found","message":"no agent nobody"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "refused",
            &["identity", "qa", "--name", "X"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"agents can't rename themselves"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "rate limited",
            &["identity", "qa", "--color", "#fff"],
            &[],
            vec![Reply::Answer(429, r#"{"error":"rate_limited","retryAfterSecs":12}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an answer that is not JSON",
            &["identity", "qa"],
            &[],
            vec![Reply::Answer(500, "oops".into())],
            owner_and_port,
            1,
            1,
        ),
        case("an empty answer", &["identity", "qa"], &[], vec![ok("")], owner_and_port, 1, 1),
        case("no answer", &["identity", "qa"], &[], vec![Reply::Close], owner_and_port, 1, 1),
        case(
            "a session with no passport: refused",
            &["identity", "qa"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a session with a passport: its own workspace (the probe first)",
                &["identity", "--color", "#123456"],
                &[],
                vec![ok(r#"{"ok":true}"#), ok(r#"{"ok":true,"handle":"me","changed":["color"]}"#)],
                owner_and_port,
                0,
                2,
            )
        },
    ];
    for args in [
        &["identity", "--name"][..],
        &["identity", "qa", "--zz"],
        &["identity", "a", "b"],
        &["identity", "qa", "--image", "pic.png", "--clear-image"],
        &["identity", "qa", "--image", "missing.png"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            pictures,
            2,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn access() {
    let rig = Rig::new(VERB, "access");
    let mut v = vec![
        // covers: agent access
        case("alone: the switches, exit 2", &["access"], &[], vec![], owner_and_port, 2, 0),
        case(
            "chat_manage on",
            &["access", "set", "qa", "chat_manage", "on"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"qa","agentsCanManageChat":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "chat_refresh off",
            &["access", "set", "qa", "chat_refresh", "off"],
            &[],
            vec![ok(r#"{"ok":true,"agentsCanRefreshChat":false,"workspace":"qa"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "agents_hire on",
            &["access", "set", "/srv/qa", "agents_hire", "on"],
            &[],
            vec![ok(r#"{"ok":true,"agentsCanHire":true,"workspace":"/srv/qa"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "agents_manage on",
            &["access", "set", "qa", "agents_manage", "on"],
            &[],
            vec![ok(r#"{"ok":true,"toggle":"agents_manage","agentsCanManage":true,"workspace":"qa"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent access --json
        case(
            "--json",
            &["access", "set", "--json", "qa", "chat_manage", "off"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"qa","agentsCanManageChat":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refused",
            &["access", "set", "qa", "chat_manage", "on"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"owner or admin only"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a bad request",
            &["access", "set", "qa", "chat_manage", "on"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"unknown workspace qa"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        traceback(case(
            "an answer that is not JSON",
            &["access", "set", "qa", "agents_hire", "off"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        )),
    ];
    for args in [
        &["access", "get"][..],
        &["access", "set"],
        &["access", "set", "qa"],
        &["access", "set", "qa", "nope", "on"],
        &["access", "set", "qa", "chat_manage", "maybe"],
        &["access", "set", "--x"],
        &["access", "set", "a", "chat_manage", "on", "d"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    check(&rig, v);
}

fn notes(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("notes.md"), "Carry on with the release.\r\nThen \u{e9}cho.\n").unwrap();
}

const MOVED: &str = r#"{"ok":true,"workspace":"qa","harness":"codex","fromHarness":"claude","model":"o3","claudeSessionId":"conv-2","sessionId":"term-2","fromSessionId":"term-1","oldChatInHistory":false,"ownSessionHeartbeats":["nightly","weekly"],"messageDelivered":true,"seedError":"too large"}"#;

#[test]
fn harness() {
    let rig = Rig::new(VERB, "harness");
    let mut v = vec![
        // covers: agent harness
        case("move", &["harness", "qa", "codex"], &[], vec![ok(MOVED)], owner_and_port, 0, 1),
        case(
            "move a folder, the whole transcript, a model, a notes file",
            &["harness", "/tmp", "codex", "--full", "--model", "o3", "--notes-file", "notes.md"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"/tmp","harness":"codex","messageDelivered":false,"messageHint":"Open the chat.","handoffPath":"/tmp/h.md"}"#)],
            notes,
            0,
            1,
        ),
        case(
            "already on it",
            &["harness", "qa", "claude", "--recent"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"qa","harness":"claude","alreadyOn":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the plan",
            &["harness", "qa", "codex", "--dry-run"],
            &[],
            vec![ok(r#"{"ok":true,"dryRun":true,"workspace":"qa","harness":"codex","fromHarness":"claude","model":"o3","launches":"/usr/bin/codex","ownSessionHeartbeats":["n"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the plan, the same harness",
            &["harness", "qa", "codex", "--dry-run"],
            &[],
            vec![ok(r#"{"ok":true,"dryRun":true,"workspace":"qa","harness":"codex","fromHarness":"codex"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent harness --json
        case("--json", &["harness", "qa", "codex", "--json"], &[], vec![ok(MOVED)], owner_and_port, 0, 1),
        case(
            "an agent is refused: exit 3",
            &["harness", "qa", "codex"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"harness_needs_person","hint":"ask a person"},"handoffPath":"/h.md"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "refused, --json",
            &["harness", "qa", "codex", "--json"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"harness_needs_person","hint":"ask"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an older daemon",
            &["harness", "qa", "codex"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"route not found"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an unknown harness: exit 2",
            &["harness", "qa", "nope"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"unknown_harness","message":"no harness nope"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error with no words",
            &["harness", "qa", "codex"],
            &[],
            vec![Reply::Answer(500, r#"{"ok":false}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case("not JSON", &["harness", "qa", "codex"], &[], vec![ok("nope")], owner_and_port, 1, 1),
        case("not a dict", &["harness", "qa", "codex"], &[], vec![ok("[1]")], owner_and_port, 1, 1),
        case("no answer", &["harness", "qa", "codex"], &[], vec![Reply::Close], owner_and_port, 1, 1),
        case(
            "a notes file that isn't there",
            &["harness", "qa", "codex", "--notes-file", "gone.md"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "a session with no passport: refused",
            &["harness", "qa", "codex"],
            &[("K2_HOOK_SOCK", "{nosock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a session with a passport: the passport (the probe first)",
                &["harness", "qa", "codex"],
                &[],
                vec![ok(r#"{"ok":true}"#), Reply::Answer(403, r#"{"error":{"code":"harness_needs_person","hint":"ask"}}"#.into())],
                owner_and_port,
                3,
                2,
            )
        },
    ];
    for args in [
        &["harness"][..],
        &["harness", "qa"],
        &["harness", "qa", "codex", "x"],
        &["harness", "--zz"],
        &["harness", "qa", "codex", "--notes-file"],
        &["harness", "qa", "codex", "--model", ""],
        &["harness", "qa", "codex", "--note", "x"],
    ] {
        v.push(case("a usage line", leak(args), &[], vec![], owner_and_port, 2, 0));
    }
    check(&rig, v);
}

const REFRESHED: &str = r#"{"ok":true,"workspace":"qa","harness":"claude","claudeSessionId":"conv-3","fromSessionId":"term-1","messageDelivered":true}"#;

#[test]
fn refresh() {
    let rig = Rig::new(VERB, "refresh");
    let mut v = vec![
        // covers: agent refresh
        case(
            "refresh with a note",
            &["refresh", "qa", "--note", "Pick up the review."],
            &[],
            vec![ok(REFRESHED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refresh with a notes file, not delivered",
            &["refresh", "/tmp", "--notes-file", "notes.md", "--full"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"/tmp","harness":"codex","sessionId":"t9","messageDelivered":false,"handoffPath":"/tmp/h.md"}"#)],
            notes,
            0,
            1,
        ),
        case(
            "this workspace from PROJECT_PATH",
            &["refresh"],
            &[("K2_PROJECT_PATH", "/srv/qa")],
            vec![ok(REFRESHED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the plan, a person must allow it",
            &["refresh", "qa", "--dry-run"],
            &[],
            vec![ok(r#"{"ok":true,"dryRun":true,"workspace":"qa","harness":"claude","needsPerson":true,"launches":"/usr/bin/claude","noteChars":42}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent refresh --json
        case("--json", &["refresh", "qa", "--json"], &[], vec![ok(REFRESHED)], owner_and_port, 0, 1),
        case(
            "refused",
            &["refresh", "qa"],
            &[],
            vec![Reply::Answer(401, r#"{"error":"not allowed"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an older daemon",
            &["refresh", "qa"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"hint":"route /cli/agent/refresh not found"}}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a usage error from the daemon",
            &["refresh", "qa"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"usage","hint":"bad mode"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case("no answer", &["refresh", "qa"], &[], vec![Reply::Close], owner_and_port, 1, 1),
        case(
            "no workspace and no passport",
            &["refresh", "--dry-run"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..case(
                "a session with a passport: its own chat (the probe first)",
                &["refresh", "--note", "x"],
                &[],
                vec![ok(r#"{"ok":true}"#), ok(REFRESHED)],
                owner_and_port,
                0,
                2,
            )
        },
    ];
    for args in [
        &["refresh", "a", "b"][..],
        &["refresh", "--note"],
        &["refresh", "--note", "x", "--notes-file", "y"],
        &["refresh", "--model", "x"],
        &["refresh", "--zz"],
    ] {
        v.push(case("a usage line", leak(args), &[], vec![], owner_and_port, 2, 0));
    }
    check(&rig, v);
}

const CONF_IDS: &str = r#"{"ok":true,"name":"qa","path":"/srv/qa","mode":"custom","enabled":false,"projects":[{"id":"pg1","name":"Acme","isPoc":true},{"id":"pg3","name":"Initech"}]}"#;

#[test]
fn set() {
    let rig = Rig::new(VERB, "set");
    let acme = r#"{"ok":true,"id":"pg1","name":"Acme","members":[]}"#;
    let globex = r#"{"ok":true,"id":"pg2","name":"Globex","members":[{"workspaceId":"w"}]}"#;
    let mut v = vec![
        // covers: agent set
        case(
            "the plan",
            &["set", "qa", "--mode", "k2", "--enabled", "false", "--name", "QA Two", "--dry-run"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        // covers: agent set --json
        case(
            "the plan --json",
            &["set", "qa", "--mode", "k2so", "--enabled", "true", "--dry-run", "--json"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the mode",
            &["set", "qa", "--mode", "manager"],
            &[],
            vec![ok(CONF), ok(r#"{"ok":true,"actions":[{"field":"agent_mode","status":"done"}]}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "the mode and enabled, --json",
            &["set", "qa", "--mode", "off", "--enabled", "false", "--json"],
            &[],
            vec![ok(CONF), ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "every value already so",
            &["set", "qa", "--enabled", "false", "--name", "qa", "--mode", "custom"],
            &[],
            vec![ok(CONF_IDS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "already so",
            &["set", "qa", "--enabled", "true", "--name", "qa"],
            &[],
            vec![ok(CONF)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a new name",
            &["set", "qa", "--name", "QA Two"],
            &[],
            vec![ok(CONF), ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "a new name, refused",
            &["set", "qa", "--name", "QA Two"],
            &[],
            vec![ok(CONF), Reply::Answer(403, r#"{"error":"owner only"}"#.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "a persona",
            &["set", "qa", "--persona", "persona.md"],
            &[],
            vec![ok(CONF), ok(r#"{"ok":true}"#), ok(r#"{"success":true}"#)],
            persona,
            0,
            3,
        ),
        case(
            "the same persona",
            &["set", "qa", "--persona", "persona.md"],
            &[],
            vec![ok(CONF_PERSONA)],
            persona,
            0,
            1,
        ),
        case(
            "join a project, leave one it isn't in",
            &["set", "qa", "--add-project", "Acme", "--add-project", "acme", "--remove-project", "Globex"],
            &[],
            vec![ok(CONF), ok(acme), ok(acme), ok(globex), ok(r#"{"ok":true,"becamePoc":true}"#)],
            owner_and_port,
            0,
            5,
        ),
        case(
            "the Point of Contact can't leave: the plan",
            &["set", "qa", "--remove-project", "Acme", "--add-project", "Initech", "--dry-run"],
            &[],
            vec![ok(CONF_IDS), ok(acme), ok(r#"{"ok":true,"id":"pg3","name":"Initech"}"#)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "the Point of Contact can't leave",
            &["set", "qa", "--remove-project", "Acme"],
            &[],
            vec![
                ok(CONF_IDS),
                ok(acme),
                Reply::Answer(409, r#"{"error":{"code":"poc_successor_required","hint":"qa is the Point of Contact"}}"#.into()),
            ],
            owner_and_port,
            1,
            3,
        ),
        case(
            "an already-member answer",
            &["set", "qa", "--add-project", "Globex", "--json"],
            &[],
            vec![ok(CONF), ok(globex), ok(r#"{"ok":true,"alreadyMember":true}"#)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "a project that isn't there: exit 4",
            &["set", "qa", "--add-project", "Nope"],
            &[],
            vec![ok(CONF), Reply::Answer(404, r#"{"error":{"code":"not_found"}}"#.into())],
            owner_and_port,
            4,
            2,
        ),
        case(
            "a project the daemon can't resolve",
            &["set", "qa", "--add-project", "X"],
            &[],
            vec![ok(CONF), Reply::Answer(500, "down".into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "in both lists",
            &["set", "qa", "--add-project", "Acme", "--remove-project", "acme"],
            &[],
            vec![ok(CONF), ok(acme), ok(acme)],
            owner_and_port,
            2,
            3,
        ),
        case(
            "the settings write fails",
            &["set", "qa", "--mode", "k2"],
            &[],
            vec![ok(CONF), Reply::Answer(500, r#"{"error":{"hint":"db locked"}}"#.into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "no answer to the settings write",
            &["set", "qa", "--enabled", "false"],
            &[],
            vec![ok(CONF), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "the conf not found",
            &["set", "nobody", "--mode", "k2"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found","hint":"no agent"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a session with a passport, the owner on disk (PRD 13.9 #4)",
                    &["set", "qa", "--mode", "k2"],
                    &[],
                    vec![ok(CONF), ok(r#"{"ok":true}"#)],
                    owner_and_port,
                    0,
                    2,
                )
            }
        },
    ];
    for args in [
        &["set"][..],
        &["set", "qa"],
        &["set", "qa", "--mode"],
        &["set", "qa", "--mode", "x"],
        &["set", "qa", "--enabled", "maybe"],
        &["set", "qa", "--zz"],
        &["set", "qa", "b", "--mode", "k2"],
        &["set", "qa", "--persona", "nofile.md"],
        &["set", "--add-project"],
        &["set", "qa", "--mode", ""],
    ] {
        v.push(case("a usage line", leak(args), &[], vec![], owner_and_port, 2, 0));
    }
    check(&rig, v);
}
