//! `k5-cli workspace <verb>` for the verbs a first run doesn't use,
//! against the shell CLI's `cli/k2 workspace <verb>`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `workspace` alone and an unknown verb: the usage line, exit 1.
//! - The switches (`api-guest-policy`, `api-skip-permissions`,
//!   `default-model`, `force-model-on-resume`, `host-session-cell-cap`,
//!   `design-tab`, `mobile-tab`): get and set, each value, a skipped set,
//!   the daemon's error, `ok: false`, not JSON, Python's raise, and the
//!   stops; an agent switching `api-skip-permissions` on (exit 3).
//! - The named verbs (`cleanup`, `resume-chat-args`, `agent-name`,
//!   `set-agent-name`, `set-handle`, `api-key`, `swap-canonical`): the
//!   lookup (a path, none), the answers, Python's `Error: <e>` lines, the
//!   stops, a session's passport.
//! - `trust`: status (all, one, `--json`), request, reset, forget, a
//!   decision with and without a fingerprint, a passport (not yours), the
//!   card refusal, the stops.
//! - `triage`, `remove`, `resources` (and one list above 256 KiB),
//!   `preview`.
//! - No answer: to the lookup (nothing on stderr, then SIGPIPE), to a request
//!   on the left of a pipe (the line, then SIGPIPE), to one at the top
//!   (the line, exit 1); nothing listening (curl's 7).
//! - Help never acts: `-h` and `--help` on every verb that changes
//!   something print help and send nothing, on both sides (the router
//!   takes them before any verb runs).
//! - What the binary hands to the shell: the help forms,
//!   `set-agent-identity`, `remove --apps`.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "workspace";
const RESOLVED: &str = r#"{"path":"/srv/ws1","name":"ws1"}"#;

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

/// A request on the left of a pipe into the shell's reader, with no
/// answer: the binary's line, then SIGPIPE. macOS's `/bin/bash` 3.2 runs
/// the reader on the empty input, which prints `stdout` and `stderr`, and
/// exits 1 (`scripts/cli-parity.sh`'s named difference
/// `bash32-pipe-reader`, through [`Compare::Bash32PipeReader`]).
fn in_pipe(c: Case, stdout: &'static str, stderr: &'static str) -> Case {
    Case {
        compare: Compare::Bash32PipeReader { stdout, stderr },
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// The owner's case: the lookup's answer, then `answers`.
fn o(name: &'static str, args: &'static [&'static str], answers: &[&str], code: i32) -> Case {
    let mut replies = vec![ok(RESOLVED)];
    replies.extend(answers.iter().map(|a| ok(a)));
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

/// A stop: nothing sent.
fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("policy.txt"), "Line one\nline two é\n\n").unwrap();
    std::fs::write(r.proj.join("notes.md"), "handoff\r\nline two\rthree\n").unwrap();
    std::fs::write(r.proj.join("CLAUDE.md"), "my own notes\n").unwrap();
    std::fs::write(r.proj.join("GEMINI.md"), "x\nk2so_generated: true\n").unwrap();
    std::fs::write(r.proj.join("AGENTS.md"), "agents\n").unwrap();
    std::fs::write(r.proj.join(".aider.conf.yml"), "read: [SKILL.md]\n").unwrap();
    let link = r.proj.join("AGENT.md");
    if !link.exists() {
        std::os::unix::fs::symlink("CLAUDE.md", &link).unwrap();
    }
}

#[test]
fn workspace_alone_and_unknown_verbs() {
    let rig = Rig::new(VERB, "bare");
    for c in [
        // covers: workspace
        stop("workspace alone", &[], 1),
        stop("an unknown verb", &["frobnicate", "x"], 1),
    ] {
        rig.check(&c);
    }
}

#[test]
fn switches_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "switch");
    let set_ok = |field: &str, value: &str, status: &str| {
        format!(
            r#"{{"ok":true,"actions":[{{"field":"other","value":"x"}},{{"field":"{field}","value":{value},"status":"{status}"}}]}}"#
        )
    };
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let v = vec![
        o(
            "api-guest-policy get",
            &["api-guest-policy", "get", "ws1"],
            &[r#"{"apiGuestPolicy":"Be kind.\nAlways."}"#],
            0,
        ),
        case(
            "api-guest-policy get: the lookup names none",
            &["api-guest-policy", "get", "ws9"],
            &[],
            vec![ok("{}"), ok(r#"{"apiGuestPolicy":"x\n"}"#)],
            owner_and_port,
            0,
            2,
        ),
        o(
            "api-guest-policy get: missing",
            &["api-guest-policy", "get", "ws1"],
            &["{}"],
            1,
        ),
        o(
            "api-guest-policy get: the daemon's error",
            &["api-guest-policy", "get", "ws1"],
            &[r#"{"error":{"code":"not_found","hint":"No workspace ws1."}}"#],
            1,
        ),
        o(
            "api-guest-policy get: not JSON",
            &["api-guest-policy", "get", "ws1"],
            &["<html>"],
            1,
        ),
        traceback(o(
            "api-guest-policy get: a list",
            &["api-guest-policy", "get", "ws1"],
            &["[1]"],
            1,
        )),
        traceback(o(
            "api-guest-policy get: a number",
            &["api-guest-policy", "get", "ws1"],
            &[r#"{"apiGuestPolicy":5}"#],
            1,
        )),
        o(
            "api-guest-policy set: words",
            &["api-guest-policy", "set", "ws1", "Be", "kind é."],
            &[leak(set_ok("api_guest_policy", r#""Be kind é.""#, "set"))],
            0,
        ),
        o(
            "api-guest-policy set --default",
            &["api-guest-policy", "set", "ws1", "--default"],
            &[leak(set_ok("api_guest_policy", r#""""#, "set"))],
            0,
        ),
        o(
            "api-guest-policy set: skipped",
            &["api-guest-policy", "set", "ws1", "--clear"],
            &[leak(set_ok("api_guest_policy", "null", "skipped"))],
            0,
        ),
        case(
            "api-guest-policy set --file",
            &["api-guest-policy", "set", "ws1", "--file", "policy.txt"],
            &[],
            vec![
                ok(RESOLVED),
                ok(&set_ok("api_guest_policy", r#""Line one""#, "set")),
            ],
            files,
            0,
            2,
        ),
        o(
            "api-guest-policy set --file - (nothing on stdin)",
            &["api-guest-policy", "set", "ws1", "--file", "-"],
            &[r#"{"ok":true}"#],
            0,
        ),
        o(
            "api-guest-policy set: ok false",
            &["api-guest-policy", "set", "ws1", "x"],
            &[r#"{"ok":false}"#],
            1,
        ),
        case(
            "api-guest-policy set: refused",
            &["api-guest-policy", "set", "ws1", "x"],
            &[],
            vec![
                ok(RESOLVED),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"forbidden","hint":"Owner only."}}"#.into(),
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        traceback(o(
            "api-guest-policy set: a value that is a number",
            &["api-guest-policy", "set", "ws1", "x"],
            &[leak(set_ok("api_guest_policy", "7", "set"))],
            1,
        )),
        stop(
            "api-guest-policy get: no workspace",
            &["api-guest-policy", "get"],
            2,
        ),
        stop(
            "api-guest-policy set: no text",
            &["api-guest-policy", "set", "ws1"],
            2,
        ),
        stop(
            "api-guest-policy set --file: none",
            &["api-guest-policy", "set", "ws1", "--file", "nope.txt"],
            1,
        ),
        stop(
            "api-guest-policy: a verb it doesn't know",
            &["api-guest-policy", "put"],
            2,
        ),
        o(
            "api-skip-permissions get: true",
            &["api-skip-permissions", "get", "ws1"],
            &[r#"{"apiSkipPermissions":true}"#],
            0,
        ),
        o(
            "api-skip-permissions get: snake case OFF",
            &["api-skip-permissions", "get", "ws1"],
            &[r#"{"api_skip_permissions":"OFF"}"#],
            0,
        ),
        o(
            "api-skip-permissions get: absent",
            &["api-skip-permissions", "get", "ws1"],
            &["{}"],
            0,
        ),
        o(
            "api-skip-permissions get: null",
            &["api-skip-permissions", "get", "ws1"],
            &[r#"{"apiSkipPermissions":null}"#],
            0,
        ),
        o(
            "api-skip-permissions get: 2",
            &["api-skip-permissions", "get", "ws1"],
            &[r#"{"apiSkipPermissions":2}"#],
            0,
        ),
        o(
            "api-skip-permissions set on",
            &["api-skip-permissions", "set", "ws1", "ON"],
            &[leak(set_ok("api_skip_permissions", r#""1""#, "set"))],
            0,
        ),
        o(
            "api-skip-permissions set off: skipped",
            &["api-skip-permissions", "set", "ws1", "no"],
            &[leak(set_ok("api_skip_permissions", "0", "skipped"))],
            0,
        ),
        stop(
            "api-skip-permissions set: a word that is no switch",
            &["api-skip-permissions", "set", "ws1", "maybe"],
            2,
        ),
        stop(
            "api-skip-permissions set: no value",
            &["api-skip-permissions", "set", "ws1"],
            2,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "api-skip-permissions set on from an agent's session",
                &["api-skip-permissions", "set", "ws1", "on"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        o(
            "default-model get",
            &["default-model", "get", "ws1"],
            &[r#"{"defaultModel":"opus"}"#],
            0,
        ),
        o(
            "default-model get: blank",
            &["default-model", "get", "ws1"],
            &[r#"{"defaultModel":null,"default_model":"  "}"#],
            0,
        ),
        o(
            "default-model set",
            &["default-model", "set", "ws1", "opus"],
            &[leak(set_ok("default_model", r#""opus""#, "set"))],
            0,
        ),
        o(
            "default-model set clear",
            &["default-model", "set", "ws1", "CLEAR"],
            &[leak(set_ok("default_model", r#""""#, "set"))],
            0,
        ),
        o(
            "default-model set: skipped",
            &["default-model", "set", "ws1", "opus"],
            &[leak(set_ok("default_model", r#""opus""#, "skipped"))],
            0,
        ),
        o(
            "force-model-on-resume get: absent",
            &["force-model-on-resume", "get", "ws1"],
            &["{}"],
            0,
        ),
        o(
            "force-model-on-resume set off",
            &["force-model-on-resume", "set", "ws1", "off"],
            &[leak(set_ok("force_model_on_resume", r#""0""#, "set"))],
            0,
        ),
        o(
            "host-session-cell-cap get: a float",
            &["host-session-cell-cap", "get", "ws1"],
            &[r#"{"hostSessionCellCap":32.7}"#],
            0,
        ),
        o(
            "api-host-session-cap get: digits in a string",
            &["api-host-session-cap", "get", "ws1"],
            &[r#"{"host_session_cell_cap":" 12 "}"#],
            0,
        ),
        o(
            "host-session-cell-cap get: null",
            &["host-session-cell-cap", "get", "ws1"],
            &[r#"{"hostSessionCellCap":null}"#],
            0,
        ),
        o(
            "host-session-cell-cap set default",
            &["host-session-cell-cap", "set", "ws1", "Default"],
            &[leak(set_ok("host_session_cell_cap", r#""default""#, "set"))],
            0,
        ),
        o(
            "host-session-cell-cap set 20",
            &["host-session-cell-cap", "set", "ws1", "20"],
            &[leak(set_ok("host_session_cell_cap", "20", "set"))],
            0,
        ),
        o(
            "host-session-cell-cap set: no action",
            &["host-session-cell-cap", "set", "ws1", "20"],
            &[r#"{"ok":true}"#],
            0,
        ),
        stop(
            "host-session-cell-cap set 600",
            &["host-session-cell-cap", "set", "ws1", "600"],
            2,
        ),
        stop(
            "host-session-cell-cap set 0",
            &["host-session-cell-cap", "set", "ws1", "0"],
            2,
        ),
        o(
            "design-tab get",
            &["design-tab", "get", "ws1"],
            &[r#"{"designModeEnabled":true}"#],
            0,
        ),
        o(
            "design-mode get: an older daemon",
            &["design-mode", "get", "ws1"],
            &["{}"],
            1,
        ),
        o(
            "design-mode set on",
            &["design-mode", "set", "ws1", "yes"],
            &[r#"{"ok":true,"actions":[]}"#],
            0,
        ),
        case(
            "design-tab set: auto is no Design word (after the lookup)",
            &["design-tab", "set", "ws1", "auto"],
            &[],
            vec![ok(RESOLVED)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "mobile-tab set: no word (after the lookup)",
            &["mobile-tab", "set", "ws1"],
            &[],
            vec![ok(RESOLVED)],
            owner_and_port,
            2,
            1,
        ),
        o(
            "mobile-tab get: not shown",
            &["mobile-tab", "get", "ws1"],
            &[r#"{"mobileTab":"auto","mobileTabShown":false}"#],
            0,
        ),
        o(
            "mobile-tab get: shown",
            &["mobile-tab", "get", "ws1"],
            &[r#"{"mobileTab":"on","mobileTabShown":true}"#],
            0,
        ),
        o(
            "mobile-tab set auto: skipped",
            &["mobile-tab", "set", "ws1", "default"],
            &[r#"{"ok":true,"actions":[{"field":"mobile_tab_enabled","status":"skipped"}]}"#],
            0,
        ),
        o(
            "mobile-tab set: an empty error dict",
            &["mobile-tab", "set", "ws1", "on"],
            &[r#"{"ok":false,"error":{}}"#],
            1,
        ),
        o(
            "design-tab get: an error that is a string",
            &["design-tab", "get", "ws1"],
            &[r#"{"error":"boom"}"#],
            1,
        ),
        stop("design-tab: no workspace", &["design-tab", "get"], 2),
        stop(
            "mobile-tab: a verb it doesn't know",
            &["mobile-tab", "flip", "ws1"],
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn named_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "named");
    const SWAPPED: &str = r#"{"swapped":true,"provider":"codex","model":"gpt","fromProvider":"claude","sessionId":"s1","handoffPath":"/srv/ws1/.k2/handoff.md","messageDelivered":false,"messageReason":"busy","messageHint":"Paste it."}"#;
    let v = vec![
        case(
            "cleanup",
            &["cleanup", "ignored"],
            &[],
            vec![ok("{\"removed\":3}\n")],
            owner_and_port,
            0,
            1,
        ),
        o(
            "resume-chat-args",
            &["resume-chat-args", "ws1"],
            &[r#"{"command":"claude","args":["--resume","x"]}"#],
            0,
        ),
        stop("resume-chat-args: no workspace", &["resume-chat-args"], 1),
        o("agent-name", &["agent-name", "ws1"], &[r#"{"display_name":"Ada"}"#], 0),
        o(
            "agent-name: none",
            &["agent-name", "ws1"],
            &["{}"],
            0,
        ),
        o(
            "agent-name: the daemon's error",
            &["agent-name", "ws1"],
            &[r#"{"error":"no such workspace"}"#],
            1,
        ),
        o("agent-name: not JSON", &["agent-name", "ws1"], &["<html>"], 1),
        o("agent-name: extra data", &["agent-name", "ws1"], &["{} x"], 1),
        o("agent-name: a list", &["agent-name", "ws1"], &["[1, 2]"], 1),
        o("agent-name: a number", &["agent-name", "ws1"], &["5"], 1),
        o(
            "agent-name: a string with error in it",
            &["agent-name", "ws1"],
            &[r#""an error""#],
            1,
        ),
        o(
            "set-agent-name",
            &["set-agent-name", "ws1", "New Name & co"],
            &[r#"{"display_name":"New Name & co"}"#],
            0,
        ),
        stop("set-agent-name: no name", &["set-agent-name", "ws1"], 1),
        stop("set-handle: not confirmed", &["set-handle", "ws1", "ada"], 1),
        stop(
            "set-handle: no handle",
            &["set-handle", "ws1", "--i-know-this-breaks-federation"],
            1,
        ),
        o(
            "set-handle",
            &["set-handle", "--i-know-this-breaks-federation", "ws1", "ada"],
            &[r#"{"handle":"ada"}"#],
            0,
        ),
        o(
            "set-handle: refused",
            &["set-handle", "ws1", "ada", "--i-know-this-breaks-federation"],
            &[r#"{"error":{"code":"taken","hint":"ada is taken."}}"#],
            1,
        ),
        o("api-key: clear", &["api-key", "ws1"], &[r#"{"keySet":false}"#], 0),
        o(
            "api-key: on the command line (deprecated)",
            &["api-key", "ws1", " sk-test "],
            &[r#"{"keySet":true}"#],
            0,
        ),
        o(
            "api-key: the daemon's error",
            &["api-key", "--", "ws1"],
            &[r#"{"error":"owner only"}"#],
            1,
        ),
        stop("api-key --key-stdin: nothing on stdin", &["api-key", "ws1", "--key-stdin"], 2),
        stop("api-key: too many words", &["api-key", "a", "b", "c"], 1),
        stop("swap-canonical: no harness", &["swap-canonical", "ws1"], 2),
        stop(
            "swap-canonical: a mode it doesn't know",
            &["swap-canonical", "ws1", "--agent", "codex", "--mode", "half"],
            2,
        ),
        stop(
            "swap-canonical: no notes file",
            &["swap-canonical", "ws1", "--agent", "codex", "--notes-file", "gone.md"],
            1,
        ),
        stop(
            "swap-canonical: an unknown flag",
            &["swap-canonical", "ws1", "--fast"],
            2,
        ),
        o(
            "swap-canonical",
            &["swap-canonical", "ws1", "--provider", "codex", "--full", "--model", "gpt"],
            &[SWAPPED],
            0,
        ),
        case(
            "swap-canonical with notes",
            &["swap-canonical", "ws1", "--agent", "codex", "--notes-file", "notes.md"],
            &[],
            vec![
                ok(RESOLVED),
                ok(r#"{"swapped":true,"provider":"codex","messageDelivered":true}"#),
            ],
            files,
            0,
            2,
        ),
        case(
            "swap-canonical: refused",
            &["swap-canonical", "ws1", "--agent", "codex"],
            &[],
            vec![
                ok(RESOLVED),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"harness_needs_person","hint":"Ask a person."},"handoffPath":"/h.md"}"#
                        .into(),
                ),
            ],
            owner_and_port,
            3,
            2,
        ),
        o(
            "swap-canonical: not swapped",
            &["swap-canonical", "ws1", "--agent", "codex"],
            &[r#"{"ok":true}"#],
            1,
        ),
        case(
            "swap-canonical: the lookup fails, the name as typed",
            &["swap-canonical", "ws1", "--agent", "codex"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec()), ok(SWAPPED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "swap-canonical: no answer to the POST",
            &["swap-canonical", "ws1", "--agent", "codex"],
            &[],
            vec![ok(RESOLVED), Reply::Close],
            owner_and_port,
            1,
            2,
        ),
        case(
            "swap-canonical: a session with no passport",
            &["swap-canonical", "ws1", "--agent", "codex"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "swap-canonical: the session's passport",
                &["swap-canonical", "ws1", "--agent", "codex"],
                &[],
                vec![ok(RESOLVED), ok(SWAPPED)],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn trust_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "trust");
    const ONE: &str = r#"{"path":"/srv/ws1","state":"changed","decided_by":"ann","findings":[{"file":".claude/settings.json","detail":"runs a hook"}],"more":2,"card":"c1","ticket":"t1","scope":"/srv","fingerprint":"fp1","trusted_by":["claude"],"forget":"k2 workspace trust forget /srv/ws1","decide":"k2 workspace trust /srv/ws1 --yes"}"#;
    let v = vec![
        case(
            "status: all",
            &["trust", "status"],
            &[],
            vec![ok(
                r#"{"folders":[{"state":"trusted","path":"/a"},{"path":"/b"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: workspace trust status
        case(
            "status: one",
            &["trust", "status", "."],
            &[],
            vec![ok(ONE)],
            owner_and_port,
            0,
            1,
        ),
        // covers: workspace trust status --json
        case(
            "status --json",
            &["trust", "status", "--json", "sub/x"],
            &[],
            vec![ok(ONE)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status --json: the daemon's error",
            &["trust", "status", "--json"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: not JSON",
            &["trust", "status"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: the card refusal",
            &["trust", "status"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"action_card_person_only","message":"Ask the owner."}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "status: more that is not a number",
            &["trust", "status"],
            &[],
            vec![ok(r#"{"path":"/a","more":"x"}"#)],
            owner_and_port,
            1,
            1,
        )),
        // covers: workspace trust request
        case(
            "request: held",
            &["trust", "request", ".", "--why", "to build"],
            &[],
            vec![ok(
                r#"{"state":"held","card":"c1","findings":[{"file":"a","detail":"b"}],"leftover":["/x"],"decide_in_app":"Settings → Agents → ada"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "request: denied",
            &["trust", "request", "."],
            &[],
            vec![ok(r#"{"state":"denied"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "request: not gated",
            &["trust", "request", "."],
            &[],
            vec![ok(r#"{"state":"open","reason":"no settings"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "reset",
            &["trust", "reset", "."],
            &[],
            vec![ok(
                r#"{"path":"/a","state":"undecided","decide":"k2 workspace trust /a --yes"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "forget",
            &["trust", "forget", "."],
            &[],
            vec![ok(
                r#"{"removed":["claude"],"status":{"trusted_by":["codex"]}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "decide with a fingerprint",
            &["trust", ".", "--always", "--fingerprint", "fp1"],
            &[],
            vec![ok(r#"{"message":"Trusted.","leftover":["/y"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "decide: the card refusal names the request",
            &["trust", ".", "--yes", "--fingerprint", "fp1"],
            &[],
            vec![ok(
                r#"{"error":{"code":"action_card_person_only","hint":"People only."}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "decide with no fingerprint: risky, not at a terminal",
            &["trust", ".", "--yes"],
            &[],
            vec![ok(r#"{"path":"/a","state":"pending","fingerprint":"fp9"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "decide with no fingerprint: none to read",
            &["trust", ".", "--no"],
            &[],
            vec![
                ok(r#"{"path":"/a","state":"clean","scope":"/"}"#),
                ok(r#"{"message":"Not trusted."}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        with(
            with_passport(&[]),
            case(
                "decide with a passport: not yours",
                &["trust", ".", "--yes"],
                &[],
                vec![ok(r#"{"decide_in_app":"Settings → Agents → ada"}"#)],
                port_only,
                3,
                1,
            ),
        ),
        stop("status: two folders", &["trust", "status", "a", "b"], 2),
        stop("status: an unknown flag", &["trust", "status", "--all"], 2),
        stop("request: no folder", &["trust", "request", "--why", "x"], 2),
        stop("decide: no decision", &["trust", "."], 2),
        stop("reset --yes", &["trust", "reset", ".", "--yes"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn triage_remove_resources_preview() {
    let rig = Rig::new(VERB, "other");
    let many = {
        let rows: Vec<String> = (0..3000)
            .map(|i| {
                format!(
                    r#"{{"fileName":"doc-{i}-{}.md","filePath":"/srv/ws1/docs/doc-{i}.md"}}"#,
                    "é".repeat(i % 40)
                )
            })
            .collect();
        format!(r#"{{"ok":true,"docs":[{}]}}"#, rows.join(","))
    };
    assert!(many.len() > 256 * 1024);
    let v = vec![
        case(
            "triage",
            &["triage"],
            &[],
            vec![ok(r#"[{"id":"a1","folder":"projects","priority":"high","title":"Ship"}]"#)],
            owner_and_port,
            0,
            1,
        ),
        stop("triage: a word", &["triage", "x"], 2),
        case(
            "triage: a session with no passport",
            &["triage"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "triage: the cell socket",
                &["triage"],
                &[],
                vec![ok("[]")],
                port_only,
                0,
                1,
            ),
        ),
        o(
            "remove",
            &["remove", "ws1"],
            &[r#"{"ok":true,"removed":"/srv/ws1"}"#],
            0,
        ),
        o(
            "remove --mode",
            &["remove", "--mode=keep-current", "ws1", "extra"],
            &[r#"{"ok":true}"#],
            0,
        ),
        case(
            "remove: not found",
            &["remove", "ws9"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            4,
            1,
        ),
        case(
            "remove: --drop-access alone",
            &["remove", "ws1", "--drop-access"],
            &[],
            vec![ok(RESOLVED)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "remove: refused (a project's PoC)",
            &["remove", "ws1"],
            &[],
            vec![
                ok(RESOLVED),
                Reply::Answer(
                    409,
                    r#"{"error":{"code":"poc_of_projects","hint":"ws1 is the Point of Contact for: A."}}"#
                        .into(),
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        stop("remove: no path", &["remove"], 1),
        stop("remove: an unknown flag", &["remove", "ws1", "-f"], 1),
        stop("remove: --apps with no value", &["remove", "ws1", "--apps"], 1),
        case(
            "resources list",
            &["resources", "list"],
            &[],
            vec![ok(r#"{"ok":true,"docs":[{"fileName":"a.md","filePath":"/p/a.md"},{"fileName":5}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resources list: many",
            &["resources", "list", "--workspace", "ws1"],
            &[],
            vec![ok(&many)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resources list --json",
            &["resources", "list", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"docs":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resources add",
            &["resources", "add", "./sub/../notes.md", "--project", "ws1"],
            &[],
            vec![ok(r#"{"ok":true,"filePath":"/srv/ws1/notes.md"}"#)],
            files,
            0,
            1,
        ),
        case(
            "resources remove: refused",
            &["resources", "remove", "--", "-odd.md"],
            &[],
            vec![ok(r#"{"ok":false,"error":{"code":"not_a_file","hint":"not_a_file: -odd.md"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "resources list: not JSON",
            &["resources", "list"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        stop("resources: no verb", &["resources"], 2),
        stop("resources: a verb it doesn't know", &["resources", "move"], 2),
        stop("resources list: a file", &["resources", "list", "a.md"], 2),
        stop("resources add: no file", &["resources", "add"], 2),
        stop("resources: --workspace with no value", &["resources", "add", "--workspace"], 2),
        stop("resources: two files", &["resources", "add", "a", "--", "b"], 2),
        case("preview", &["preview"], &[], vec![], files, 0, 0),
        case("preview: a folder that isn't there", &["preview", "/nonexistent-k5-x"], &[], vec![], owner_and_port, 0, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_verbs_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let port = dead_port();
    let v = vec![
        in_pipe(
            case(
                "the lookup: no answer (nothing on stderr, then SIGPIPE)",
                &["agent-name", "ws1"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            "",
        ),
        in_pipe(
            case(
                "agent-name: no answer on the left of the pipe",
                &["agent-name", "ws1"],
                &[],
                vec![ok(RESOLVED), Reply::Close],
                owner_and_port,
                141,
                2,
            ),
            "",
            "Error: Expecting value: line 1 column 1 (char 0)\n",
        ),
        waited(case(
            "resume-chat-args: no answer at the top",
            &["resume-chat-args", "ws1"],
            &[],
            vec![ok(RESOLVED), Reply::Close],
            owner_and_port,
            1,
            2,
        )),
        in_pipe(
            case(
                "remove: the lookup's no-answer line, then SIGPIPE",
                &["remove", "ws1"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            ),
            "",
            "",
        ),
        case(
            "set-handle: a reset",
            &[
                "set-handle",
                "ws1",
                "ada",
                "--i-know-this-breaks-federation",
            ],
            &[],
            vec![ok(RESOLVED), Reply::Reset],
            owner_and_port,
            1,
            2,
        ),
        case(
            "agent-name: nothing listens",
            &["agent-name", "ws1"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            7,
            0,
        ),
        waited(case(
            "trust status: no answer",
            &["trust", "status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn workspace_verbs_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let v = vec![
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                o(
                    "agent-name: the owner on disk (PRD 13.9 #4)",
                    &["agent-name", "ws1"],
                    &[r#"{"display_name":"Ada"}"#],
                    0,
                ),
            )
        },
        with(
            with_passport(&[]),
            case(
                "default-model get: a passport and no owner",
                &["default-model", "get", "ws1"],
                &[],
                vec![ok(RESOLVED), ok(r#"{"defaultModel":"opus"}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a refused passport heals once",
                &["cleanup"],
                &[],
                vec![heal(&rig), ok("{}")],
                port_only,
                0,
                2,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn help_never_acts_and_the_shell_keeps_its_forms() {
    let rig = Rig::new(VERB, "shell");
    let help = |name: &'static str, args: &'static [&'static str]| {
        shell(case(name, args, &[], vec![], owner_and_port, 0, 0))
    };
    let v = vec![
        help(
            "set-handle --help",
            &[
                "set-handle",
                "ws1",
                "ada",
                "--i-know-this-breaks-federation",
                "--help",
            ],
        ),
        help("set-agent-name -h", &["set-agent-name", "ws1", "-h", "x"]),
        help("api-key -h", &["api-key", "ws1", "sk", "-h"]),
        help(
            "swap-canonical --help",
            &["swap-canonical", "ws1", "--agent", "codex", "--help"],
        ),
        help("remove -h", &["remove", "ws1", "-h"]),
        help("cleanup --help", &["cleanup", "--help"]),
        help("resources add -h", &["resources", "add", "a.md", "-h"]),
        help(
            "trust decide --help",
            &["trust", ".", "--yes", "--fingerprint", "f", "--help"],
        ),
        help("trust request -h", &["trust", "request", ".", "-h"]),
        help(
            "api-guest-policy set -h",
            &["api-guest-policy", "set", "ws1", "x", "-h"],
        ),
        help(
            "api-skip-permissions set --help",
            &["api-skip-permissions", "set", "ws1", "on", "--help"],
        ),
        help(
            "default-model set -h",
            &["default-model", "set", "ws1", "x", "-h"],
        ),
        help(
            "force-model-on-resume set -h",
            &["force-model-on-resume", "set", "ws1", "on", "-h"],
        ),
        help(
            "host-session-cell-cap set -h",
            &["host-session-cell-cap", "set", "ws1", "5", "-h"],
        ),
        help(
            "design-tab set -h",
            &["design-tab", "set", "ws1", "on", "-h"],
        ),
        help(
            "mobile-tab set --help",
            &["mobile-tab", "set", "ws1", "auto", "--help"],
        ),
        help("api-guest-policy alone", &["api-guest-policy"]),
        help("trust alone", &["trust"]),
        help("design-tab help", &["design-tab", "help"]),
        shell(case(
            "set-agent-identity",
            &["set-agent-identity"],
            &[],
            vec![ok(r#"{"handle":"ada","displayName":"Ada"}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "remove --apps",
            &["remove", "ws9", "--apps", "park"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            4,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
