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
            vec![ok(&*leak(&[&format!("{LAYERS}\n\n")])[0])],
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
