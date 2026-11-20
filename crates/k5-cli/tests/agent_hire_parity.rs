//! `k5-cli agent hire ...` against the shell CLI's `cli/k2 agent hire ...`
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - Help anywhere on the line (the shell's, nothing sent) and every
//!   usage line, each before anything is sent.
//! - A new folder: the conf, the hire core, the settings; `--json`; the
//!   dry run and the hire core's own answer to it (refused: exit 3 and the
//!   `Ask with:` line).
//! - Every step at once: the presets, a project, a color, a focus group,
//!   two context layers, the launch, the onboard message and the App's
//!   handover, for real, as a dry run and as JSON.
//! - An agent already there: every step skipped, a connection the daemon
//!   resolves to a peer it has, the handover filed on its own.
//! - A template: the folder from the hire core's dry run, the rows, the
//!   questions, the hello (sent by the daemon, sent after the launch, or
//!   waiting for the first start), the switch a dry run names.
//! - Failures: each look-up's (exit 4, 2 or 1), a step that fails while
//!   the rest go on, the workspace step that stops the rest (exit 3 and
//!   the card's lines), an answer the block's Python can't read (its
//!   traceback), no answer, no daemon.
//! - A session with a passport: the shell's ([`Compare::Shell`]). A brief
//!   with a NUL in it is the shell's too, but bash's warning names its
//!   own path, so that hand-back is the unit test's
//!   (`families::agent::hire::tests`).

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

fn leak(args: &[&str]) -> &'static [&'static str] {
    let owned: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(owned.into_boxed_slice())
}

fn with(c: Case, compare: Compare) -> Case {
    Case { compare, ..c }
}

/// The folders and files the rows name, beside the owner token and port.
fn files(r: &Rig) {
    owner_and_port(r);
    let hm = r.proj.join("hire-me");
    std::fs::create_dir_all(&hm).unwrap();
    std::fs::write(hm.join("notes.md"), "# notes\n").unwrap();
    std::fs::write(r.proj.join("notes2.md"), "x\n").unwrap();
    std::fs::write(r.proj.join("plain.txt"), "x\n").unwrap();
    std::fs::write(
        r.proj.join("persona.md"),
        "# Ann\r\nline two\rthree\n\u{e9}t\u{e9}",
    )
    .unwrap();
    std::fs::write(
        r.proj.join("brief.txt"),
        b"notes for you\n\xff caf\xc3\xa9\n\n".as_slice(),
    )
    .unwrap();
    std::fs::write(r.proj.join("nul.txt"), b"a\0b\n".as_slice()).unwrap();
}

/// The conf of a folder no workspace has.
fn not_found() -> Reply {
    Reply::Answer(
        404,
        r#"{"ok":false,"error":{"code":"not_found","hint":"no agent at that path"}}"#.to_string(),
    )
}

const CORE_OK: &str = r#"{"ok":true,"steps":[{"step":"create-dir","status":"done"},{"step":"write-persona","status":"done"},{"step":"register-workspace","status":"done"}]}"#;

const SET_OK: &str = r#"{"ok":true,"actions":[{"field":"agent_mode","status":"done"}]}"#;

const PRESETS: &str = r#"[{"id":"claude","label":"Claude","command":"claude --x","enabled":true},{"id":"codex","label":"Codex","command":"codex","enabled":false}]"#;

const PROJECT: &str = r#"{"ok":true,"id":"g1","name":"Acme","members":[]}"#;

const GROUPS: &str = r#"[{"id":"f1","name":"Team"},{"id":"f2","name":"Other"}]"#;

const LOCAL_OPS: &str = r#"{"ok":true,"kind":"local","path":"/srv/ops"}"#;

const REFUSED_HIRE: &str = r#"{"ok":false,"error":{"code":"hire_off","hint":"Let this agent hire agents is off"},"permission":{"command":"k2 permission request agents.hire"}}"#;

#[test]
fn hire_words() {
    let rig = Rig::new(VERB, "hirewords");
    let mut v = Vec::new();
    // covers: agent hire
    // covers: agent hire --json
    for args in [
        &["hire", "-h"][..],
        &["hire", "--help"],
        &["hire", "d", "--launch", "-h"],
        &["hire", "--name", "--help"],
        &["hire", "--template", "t", "--dry-run", "-h"],
        &["hire", "d", "--json", "--help"],
        &["hire", "d", "--take-app", "a1", "--onboard", "hi", "-h"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            files,
            0,
            0,
        ));
    }
    for args in [
        &["hire"][..],
        &["hire", "--json"],
        &["hire", "", "--template", ""],
        &["hire", "--name"],
        &["hire", "--agent"],
        &["hire", "--template"],
        &["hire", "--param"],
        &["hire", "--answer"],
        &["hire", "--persona"],
        &["hire", "--mode"],
        &["hire", "--enabled"],
        &["hire", "--connect"],
        &["hire", "--project"],
        &["hire", "--focus-group"],
        &["hire", "--color"],
        &["hire", "--context"],
        &["hire", "--take-app"],
        &["hire", "--brief"],
        &["hire", "--onboard"],
        &["hire", "--db-access"],
        &["hire", "d", "--param", "nokey"],
        &["hire", "d", "--answer", "=x"],
        &["hire", "d", "--answer", "x"],
        &["hire", "--bogus"],
        &["hire", "-"],
        &["hire", "a", "b"],
        &["hire", "--template", "t", "--persona", "persona.md"],
        &["hire", "d", "--param", "a=1"],
        &["hire", "d", "--answer", "a=1"],
        &["hire", "d", "--persona", "no-such.md"],
        &["hire", "d", "--persona", "hire-me"],
        &["hire", "d", "--mode", "zz"],
        &["hire", "d", "--enabled", "yes"],
        &["hire", "d", "--db-access", "all"],
        &["hire", "d", "--brief", "brief.txt"],
        &["hire", "d", "--take-app", "a1", "--brief", "no-such.txt"],
        &[
            "hire",
            "--template",
            "t",
            "--answer",
            "a=1",
            "--answer",
            " a =2",
        ],
        &["hire", "d", "--json", "--mode", "k2x"],
    ] {
        v.push(case(
            "a usage line, nothing sent",
            leak(args),
            &[],
            vec![],
            files,
            2,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn hire_a_new_folder() {
    let rig = Rig::new(VERB, "hirenew");
    let three = || vec![not_found(), ok(CORE_OK), ok(SET_OK)];
    let v = vec![
        // covers: agent hire
        case("the conf, the core, the settings", &["hire", "newbie"], &[], three(), files, 0, 3),
        // covers: agent hire --json
        case("the same as JSON", &["hire", "newbie", "--json"], &[], three(), files, 0, 3),
        case(
            "the persona file, its line ends as Python reads them",
            &["hire", "newbie", "--persona", "persona.md", "--mode", "k2so", "--enabled", "false"],
            &[],
            three(),
            files,
            0,
            3,
        ),
        case(
            "the brief and a handover the daemon couldn't file",
            &["hire", "newbie", "--take-app", " app1 ", "--brief", "brief.txt", "--no-agents-md"],
            &[],
            vec![
                not_found(),
                ok(r#"{"ok":true,"steps":[],"handover":{"error":{"code":"app_not_found","hint":"no App app1"}}}"#),
                ok(SET_OK),
            ],
            files,
            1,
            3,
        ),
        case(
            "the dry run: the core's gate answers (an older daemon)",
            &["hire", "newbie", "--dry-run"],
            &[],
            vec![
                not_found(),
                Reply::Answer(400, r#"{"error":{"code":"usage","hint":"missing path"}}"#.to_string()),
            ],
            files,
            0,
            2,
        ),
        // covers: agent hire --json
        case(
            "the dry run as JSON",
            &["hire", "newbie", "--dry-run", "--json", "--fanout", "--no-wiki"],
            &[],
            vec![not_found(), ok(r#"{"ok":true,"dryRun":true}"#)],
            files,
            0,
            2,
        ),
        case(
            "the dry run of a hire the core refuses: exit 3 and the card",
            &["hire", "newbie", "--dry-run", "--name", "Bo", "--launch"],
            &[],
            vec![not_found(), Reply::Answer(403, REFUSED_HIRE.to_string())],
            files,
            3,
            2,
        ),
        case(
            "the workspace step fails: the rest not run, exit 3, the card's lines",
            &["hire", "newbie", "--db-access", "write", "--name", "Bo"],
            &[],
            vec![
                not_found(),
                Reply::Answer(
                    403,
                    r#"{"ok":false,"error":{"code":"owner_only","hint":"only a person may raise DB access"},"permission":{"requestable":false,"hint":"Ask a person."}}"#.to_string(),
                ),
            ],
            files,
            3,
            2,
        ),
        case(
            "the workspace step fails as JSON",
            &["hire", "newbie", "--json", "--connect", "ops"],
            &[],
            vec![
                not_found(),
                ok(LOCAL_OPS),
                Reply::Answer(500, r#"{"error":"disk full"}"#.to_string()),
            ],
            files,
            1,
            3,
        ),
        with(
            case(
                "a core answer that is not a dict: the block's traceback",
                &["hire", "newbie"],
                &[],
                vec![not_found(), ok("[1]")],
                files,
                1,
                2,
            ),
            Compare::Traceback,
        ),
        case(
            "a core answer that is not JSON",
            &["hire", "newbie"],
            &[],
            vec![not_found(), ok("not json")],
            files,
            1,
            2,
        ),
        case(
            "no answer to the conf",
            &["hire", "newbie"],
            &[],
            vec![Reply::Close],
            files,
            1,
            1,
        ),
        case(
            "an HTTP error with no JSON on the conf",
            &["hire", "newbie"],
            &[],
            vec![Reply::Answer(500, "oops".to_string())],
            files,
            1,
            1,
        ),
        case(
            "a session with no passport: the owner token from disk",
            &["hire", "newbie"],
            &[],
            three(),
            files,
            0,
            3,
        ),
    ];
    let mut v = v;
    let last = v.pop().unwrap();
    v.push(Case {
        env: vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
        ..last
    });
    let port = dead_port();
    v.push(case(
        "no daemon",
        &["hire", "newbie"],
        &[("K2_PORT", port.as_str())],
        vec![],
        files,
        1,
        0,
    ));
    check(&rig, v);
}

/// Every step's flag at once (`--onboard` brings `--launch`).
const EVERY: &[&str] = &[
    "hire",
    "hire-me",
    "--name",
    "Ann",
    "--mode",
    "manager",
    "--enabled",
    "true",
    "--db-access",
    "read",
    "--agent",
    "claude",
    "--connect",
    "ops",
    "--project",
    "Acme",
    "--color",
    "rgb(1, 2, 3)",
    "--focus-group",
    "team",
    "--context",
    "wiki:index",
    "--context",
    "notes.md",
    "--onboard",
    "hello there",
    "--take-app",
    "app1",
    "--fanout",
    "--no-wiki",
];

fn every_reply(rig: &Rig) -> Vec<Reply> {
    vec![
        ok(PRESETS),
        ok(PROJECT),
        ok(GROUPS),
        not_found(),
        ok(LOCAL_OPS),
        Reply::WriteThen {
            file: rig.proj.join("hire-me").join("AGENTS.md"),
            content: "<!-- GENERATED by K2 -->\n".to_string(),
            status: 200,
            body: r#"{"ok":true,"steps":[{"step":"write-persona","status":"done"},{"step":"register-workspace","status":"done"},{"step":"set-db-access","status":"done","reason":"raised"}],"handover":{"id":"h1","state":"offered"}}"#.to_string(),
        },
        ok(r#"{"ok":true,"actions":[{"field":"agent_mode","status":"done"},{"field":"agent_enabled","status":"skipped"},{"field":"default_agent","status":"done"}]}"#),
        ok(r#"{"success":true}"#),
        ok(r#"{"ok":true,"id":"w1"}"#),
        ok(r#"{"ok":true}"#),
        ok(r#"{"ok":true}"#),
        ok(r#"{"ok":true,"warning":"bound late"}"#),
        ok(r#"{"ok":true,"becamePoc":true}"#),
        ok(r#"{"ok":true,"layer":{"id":"L1","path":".k2/wiki/_Index.md","exists":false}}"#),
        ok(r#"{"ok":true,"layer":{"id":"L2","path":"notes.md","exists":true}}"#),
        ok(r#"{"success":true,"session_id":"sess-0123456789"}"#),
        ok(r#"{"success":true}"#),
    ]
}

fn plus(base: &[&str], more: &[&str]) -> &'static [&'static str] {
    let mut v: Vec<&str> = base.to_vec();
    v.extend_from_slice(more);
    leak(&v)
}

#[test]
fn hire_every_step() {
    let rig = Rig::new(VERB, "hireall");
    let v = vec![
        // covers: agent hire
        case("every step", EVERY, &[], every_reply(&rig), files, 0, 17),
        // covers: agent hire --json
        case(
            "every step as JSON",
            plus(EVERY, &["--json"]),
            &[],
            every_reply(&rig),
            files,
            0,
            17,
        ),
        case(
            "every step's plan: the hire core refuses it",
            plus(EVERY, &["--dry-run"]),
            &[],
            vec![
                ok(PRESETS),
                ok(PROJECT),
                ok(GROUPS),
                not_found(),
                ok(LOCAL_OPS),
                Reply::Answer(403, REFUSED_HIRE.to_string()),
            ],
            files,
            3,
            6,
        ),
        case(
            "failed steps and the rest go on: the first failure's exit",
            &[
                "hire",
                "hire-me",
                "--connect",
                "ghost",
                "--context",
                "wiki:nope",
                "--context",
                "subagents",
                "--launch",
                "--onboard",
                "x",
            ],
            &[],
            vec![
                not_found(),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no agent ghost"}}"#.to_string(),
                ),
                ok(CORE_OK),
                ok(r#"{"ok":false,"error":"settings locked"}"#),
                ok(r#"{"error":{"code":"catalog_unknown","hint":"no such id"}}"#),
                ok(r#"{"error":{"code":"no_file","message":"pack missing"}}"#),
                ok(r#"{"success":false,"error":"no harness"}"#),
            ],
            files,
            1,
            7,
        ),
        case(
            "failed steps as JSON",
            &[
                "hire",
                "hire-me",
                "--json",
                "--project",
                "Acme",
                "--color",
                "#ABC",
                "--focus-group",
                "none",
                "--context",
                "notes.md",
            ],
            &[],
            vec![
                ok(PROJECT),
                not_found(),
                ok(CORE_OK),
                ok(SET_OK),
                ok(r#"{"ok":false}"#),
                ok(r#"{"ok":false}"#),
                Reply::Answer(409, r#"{"error":"group locked"}"#.to_string()),
                ok(r#"{"error":{"code":"duplicate_layer","hint":"there"}}"#),
            ],
            files,
            1,
            8,
        ),
    ];
    check(&rig, v);
}

#[test]
fn hire_the_look_ups() {
    let rig = Rig::new(VERB, "hirelook");
    let one = |name, args: &[&str], reply: Reply, code| {
        case(name, leak(args), &[], vec![reply], files, code, 1)
    };
    let v = vec![
        one(
            "no preset matches",
            &["hire", "d", "--agent", "zz"],
            ok(PRESETS),
            4,
        ),
        one(
            "two presets by label",
            &["hire", "d", "--agent", "Claude"],
            ok(
                r#"[{"id":"a","label":"Claude","enabled":true},{"id":"b","label":"claude","enabled":true}]"#,
            ),
            2,
        ),
        one(
            "a preset that is off",
            &["hire", "d", "--agent", "codex"],
            ok(PRESETS),
            2,
        ),
        one(
            "no preset list",
            &["hire", "d", "--agent", "x"],
            ok(r#"{"error":"boom"}"#),
            1,
        ),
        one(
            "a project no one has",
            &["hire", "d", "--project", "X"],
            Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no project X"}}"#.to_string(),
            ),
            4,
        ),
        one(
            "a project prefix that is two",
            &["hire", "d", "--project", "A"],
            ok(r#"{"error":{"code":"ambiguous_id"}}"#),
            4,
        ),
        one(
            "a project error as a string",
            &["hire", "d", "--project", "X"],
            ok(r#"{"error":"db locked"}"#),
            1,
        ),
        one(
            "a project answer with no id",
            &["hire", "d", "--project", "X"],
            ok(r#"{"ok":true}"#),
            1,
        ),
        one(
            "no focus group matches",
            &["hire", "d", "--focus-group", "zz"],
            ok(GROUPS),
            4,
        ),
        one(
            "two focus groups by name",
            &["hire", "d", "--focus-group", "team"],
            ok(r#"[{"id":"f1","name":"Team"},{"id":"f2","name":"team"}]"#),
            4,
        ),
        one(
            "no focus group list",
            &["hire", "d", "--focus-group", "x"],
            ok(r#"{"ok":false}"#),
            1,
        ),
        one(
            "the conf's error",
            &["hire", "d"],
            ok(r#"{"error":{"code":"db","hint":"locked"}}"#),
            1,
        ),
        one(
            "the conf's error as a string",
            &["hire", "d"],
            ok(r#"{"error":"locked"}"#),
            1,
        ),
        one(
            "a context file not there",
            &["hire", "hire-me", "--context", "missing.md"],
            not_found(),
            4,
        ),
        one(
            "a context file outside",
            &["hire", "hire-me", "--context", "../notes2.md"],
            not_found(),
            2,
        ),
        one(
            "an empty context",
            &["hire", "hire-me", "--context", "  "],
            not_found(),
            2,
        ),
        with(
            one(
                "a conf that is a list: the traceback",
                &["hire", "d"],
                ok("[]"),
                1,
            ),
            Compare::Traceback,
        ),
        case(
            "a color the patterns don't take",
            &["hire", "d", "--color", "rgb(1,2,3 )"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "a file where the folder goes",
            &["hire", "plain.txt"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
    ];
    check(&rig, v);
}

const CONF_THERE: &str = r##"{"ok":true,"id":"w7","name":"qa","path":"/srv/qa","mode":"custom","enabled":true,"personaPath":"persona.md","color":"#AABBCC","focusGroupId":null,"connections":[{"peer":"ops"},{"peer":"far@box"}],"projects":[{"id":"g1"}],"live":{"active":true}}"##;

#[test]
fn hire_an_agent_already_there() {
    let rig = Rig::new(VERB, "hirethere");
    let layers = r#"{"ok":true,"layers":[{"path":"./notes.md","source":"catalog:wiki-index"}]}"#;
    let v = vec![
        // covers: agent hire
        case(
            "every step already so",
            &[
                "hire",
                ".",
                "--name",
                "qa",
                "--enabled",
                "true",
                "--project",
                "Acme",
                "--color",
                "#abc",
                "--focus-group",
                "none",
                "--connect",
                "ops",
                "--connect",
                "far",
                "--context",
                "wiki:index",
                "--launch",
            ],
            &[],
            vec![
                ok(PROJECT),
                ok(CONF_THERE),
                ok(layers),
                ok(r#"{"ok":true,"kind":"remote","address":"FAR@box","warning":"slow link"}"#),
            ],
            files,
            0,
            4,
        ),
        // covers: agent hire --json
        case(
            "every step already so, as JSON",
            &["hire", ".", "--json", "--connect", "ops"],
            &[],
            vec![ok(CONF_THERE), ok(layers)],
            files,
            0,
            2,
        ),
        case(
            "the handover filed on its own, with its card",
            &["hire", ".", "--take-app", "app1", "--brief", "brief.txt"],
            &[],
            vec![
                ok(CONF_THERE),
                ok(layers),
                ok(r#"{"handover":{"id":"h9","state":"pending"},"request":{"id":"c1"}}"#),
            ],
            files,
            0,
            3,
        ),
        case(
            "the handover refused",
            &["hire", ".", "--take-app", "app1", "--json"],
            &[],
            vec![
                ok(CONF_THERE),
                ok(r#"{"ok":false}"#),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"not yours"}}"#.to_string(),
                ),
            ],
            files,
            3,
            3,
        ),
    ];
    check(&rig, v);
}

const TPL_APPLIED: &str = r#"{"ok":true,"steps":[{"step":"write-persona","status":"done"},{"step":"register-workspace","status":"done"}],"rows":[{"step":"folder","status":"done","target":"/srv/hire-root/ann"},{"step":"role","status":"done","target":"ROLE.md"},{"step":"starter","status":"skip","target":"wiki/Home.md","reason":"exists"},{"step":"lock","status":"yours","target":"template.lock"},{"step":"x","status":"odd","target":"y"}],"template":{"id":"qa-bot","version":3},"permissions":{"suggested":["compute.run"]},"questions":[{"id":"stack","answered":true},{"id":"repo"}],"hello":"Hi.","helloDelivered":true,"onboardQueued":true}"#;

const TPL_OLD: &str = r#"{"ok":true,"steps":[],"rows":[{"step":"role","status":"done","target":"ROLE.md"}],"template":{"id":"qa-bot","version":1},"questions":[{"id":"stack","answered":true}],"hello":"Hi.\nI test."}"#;

const TPL_DRY: &str = r#"{"ok":true,"rows":[{"step":"folder","status":"write","target":"/srv/hire-root/qa-bot"},{"step":"role","status":"write","target":"ROLE.md"}],"template":{"id":"qa-bot","version":3},"permissions":{"cards":true},"questions":[{"id":"stack","answered":true},{"id":"repo"}],"hireSwitch":{"code":"hire_off","command":"k2 permission request agents.hire"}}"#;

fn probe(path: &str) -> Reply {
    ok(&format!(r#"{{"ok":true,"path":"{path}"}}"#))
}

#[test]
fn hire_a_template() {
    let rig = Rig::new(VERB, "hiretpl");
    let ok_ = || ok(r#"{"ok":true}"#);
    let v = vec![
        // covers: agent hire
        case(
            "under the hire root: the hello sent by the daemon",
            &[
                "hire",
                "--template",
                " qa-bot ",
                "--name",
                "Ann",
                "--param",
                "team=Acme",
                "--param",
                "tone=calm",
                "--answer",
                "stack=rust",
                "--launch",
                "--onboard",
                "go",
                "--agent",
                "claude",
            ],
            &[],
            vec![
                probe("/srv/hire-root/ann"),
                ok(PRESETS),
                not_found(),
                ok(TPL_APPLIED),
                ok(SET_OK),
                ok(r#"{"success":true}"#),
                ok(r#"{"success":true,"session_id":"s-1","reused":true}"#),
            ],
            files,
            0,
            7,
        ),
        // covers: agent hire --json
        case(
            "under the hire root as JSON",
            &["hire", "--template", "qa-bot", "--json", "--launch"],
            &[],
            vec![
                probe("/srv/hire-root/qa-bot"),
                not_found(),
                ok(TPL_APPLIED),
                ok(SET_OK),
                ok(r#"{"success":true,"session_id":"s-1"}"#),
            ],
            files,
            0,
            5,
        ),
        case(
            "an older daemon: the hello after the launch",
            &["hire", "--template", "qa-bot", "--launch"],
            &[],
            vec![
                probe("/srv/hire-root/qa-bot"),
                not_found(),
                ok(TPL_OLD),
                ok(SET_OK),
                ok(r#"{"success":true,"session_id":"s-2"}"#),
                ok(r#"{"success":true}"#),
            ],
            files,
            0,
            6,
        ),
        case(
            "no launch: the hello waits",
            &["hire", "hire-me", "--template", "qa-bot"],
            &[],
            vec![not_found(), ok(TPL_OLD), ok(SET_OK)],
            files,
            0,
            3,
        ),
        case(
            "the plan, and the switch the real hire needs",
            &["hire", "--template", "qa-bot", "--dry-run"],
            &[],
            vec![probe("/srv/hire-root/qa-bot"), not_found(), ok(TPL_DRY)],
            files,
            0,
            3,
        ),
        // covers: agent hire --json
        case(
            "the plan as JSON",
            &["hire", "--template", "qa-bot", "--dry-run", "--json"],
            &[],
            vec![probe("/srv/hire-root/qa-bot"), not_found(), ok(TPL_DRY)],
            files,
            0,
            3,
        ),
        case(
            "the plan refused: the template's exit",
            &["hire", "hire-me", "--template", "qa-bot", "--dry-run"],
            &[],
            vec![
                not_found(),
                Reply::Answer(
                    409,
                    r#"{"error":{"code":"template_conflict","hint":"another template"}}"#
                        .to_string(),
                ),
            ],
            files,
            3,
            2,
        ),
        case(
            "an unknown template",
            &["hire", "--template", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"unknown_template","hint":"no template 'nope'"}}"#.to_string(),
            )],
            files,
            4,
            1,
        ),
        case(
            "the hire refused before the folder: the card",
            &["hire", "--template", "qa-bot"],
            &[],
            vec![Reply::Answer(403, REFUSED_HIRE.to_string())],
            files,
            3,
            1,
        ),
        case(
            "the folder not named",
            &["hire", "--template", "qa-bot"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            files,
            1,
            1,
        ),
        case(
            "the template core fails",
            &["hire", "hire-me", "--template", "qa-bot", "--json"],
            &[],
            vec![
                not_found(),
                Reply::Answer(
                    400,
                    r#"{"error":{"code":"bad_template","hint":"template.toml: missing id"}}"#
                        .to_string(),
                ),
            ],
            files,
            2,
            2,
        ),
        case(
            "the hello not delivered",
            &[
                "hire",
                "hire-me",
                "--template",
                "qa-bot",
                "--launch",
                "--onboard",
                "go",
            ],
            &[],
            vec![
                not_found(),
                ok(TPL_OLD),
                ok(SET_OK),
                ok(r#"{"success":true,"session_id":"s-3"}"#),
                ok(r#"{"success":false,"message":"not ready"}"#),
                ok_(),
            ],
            files,
            1,
            6,
        ),
    ];
    check(&rig, v);
}

#[test]
fn hire_the_shell_s_cases() {
    let rig = Rig::new(VERB, "hireshell");
    let c = case(
        "a session with a passport: the shell's (the owner token to the new workspace)",
        &["hire", "newbie", "--dry-run"],
        &[],
        vec![not_found(), ok(r#"{"ok":true}"#)],
        files,
        0,
        2,
    );
    rig.check(&Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
        ..with(c, Compare::Shell)
    });
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "hirebrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want) in [
        (
            &["hire", "--bogus"][..],
            format!("{{\"error\": {{\"code\": \"bad_usage\", \"hint\": \"unknown flag '--bogus' \\u2014 see: {cli} agent hire --help\"}}}}\n"),
        ),
        (
            &["hire", "--agent"],
            format!("{{\"error\": {{\"code\": \"bad_usage\", \"hint\": \"--agent requires a preset id/label (see: {cli} preset list)\"}}}}\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], files, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
    let c = case(
        "brand",
        &["hire", "d", "--agent", "zz"],
        &[],
        vec![ok("[]")],
        files,
        4,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("{{\"error\": {{\"code\": \"not_found\", \"hint\": \"no agent preset matches 'zz' \\u2014 run `{cli} preset list` (id, label, or command token)\"}}}}\n")
    );
}
