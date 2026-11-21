//! `k5-cli remote-session` against the shell CLI's `cli/k2
//! remote-session`, on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code for every verb of
//! `cmd_remote_session`.
//!
//! - Each verb: `status` (and `--json`), `enable`, `disable`, `grant`
//!   (its `--ttl` forms), `grants` (and `--json`, one above 256 KiB),
//!   `revoke`, `shell`, `write`, `read`; no verb, `help`, an unknown verb.
//! - The error printer: each taught code, a refusal, a usage error, an
//!   answer it can't read, an empty one.
//! - A grant token on stdin and on the command line, both, an empty one.
//! - A value flag as the last word: exit 1, no line (a bash bug, copied).
//! - The gates and the locked tool's session check; no answer.
//! - Times: Python 3.12's `utcfromtimestamp` warning, once per line;
//!   `grants` with a grant raises in the shell's reader (a bash bug,
//!   copied).
//! - Help with `-h` or `--help`: the shell's, and it sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "remote-session";

fn reply(body: &str) -> Reply {
    ok(body)
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

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
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

#[test]
fn remote_session_verbs() {
    let rig = Rig::new(VERB, "verbs");
    let v = vec![
        // covers: remote-session
        case("no verb: the help page", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        // covers: remote-session --json
        case(
            "--json is no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: remote-session status
        case(
            "status",
            &["status"],
            &[],
            vec![reply(
                r#"{"ok":true,"enabled":false,"activeSessions":[{"sessionId":"s1","grantId":"g1"},{}],"recentDenials":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: remote-session status --json
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![reply(r#"{"ok":true,"enabled":true,"n":"é","l":[1,{"a":[]}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: off, taught",
            &["status"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"REMOTE_SESSIONS_DISABLED","hint":"Remote Sessions are off on this server"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "status: a flag it doesn't take, and the page",
            &["status", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: remote-session enable
        case(
            "enable",
            &["enable"],
            &[],
            vec![reply(r#"{"ok":true,"enabled":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remote-session disable
        case(
            "disable",
            &["disable"],
            &[],
            vec![reply(r#"{"ok":true,"enabled":false,"killedSessions":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "disable refused",
            &["disable"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"Invalid or missing auth token"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "enable: a usage error",
            &["enable"],
            &[],
            vec![Reply::Answer(400, r#"{"ok":false,"code":"bad_request","message":"no"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "enable: not JSON",
            &["enable"],
            &[],
            vec![reply("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "enable: an empty answer",
            &["enable"],
            &[],
            vec![reply("\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "enable: a list",
            &["enable"],
            &[],
            vec![reply("[1]")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "enable: a code that is a number",
            &["enable"],
            &[],
            vec![reply(r#"{"error":{"code":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "enable: a word it doesn't take",
            &["enable", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: remote-session revoke
        case(
            "revoke",
            &["revoke", "g1"],
            &[],
            vec![reply(r#"{"ok":true,"grant":{"id":"g1"},"killedSessions":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "revoke: no grant, taught",
            &["revoke", "g9"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"NO_GRANT"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case("revoke: no id", &["revoke"], &[], vec![], owner_and_port, 2, 0),
        case(
            "revoke: two ids",
            &["revoke", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: remote-session shell
        case(
            "shell",
            &["shell", "--cwd", "/tmp/x y"],
            &[],
            vec![reply(r#"{"ok":true,"sessionId":"s1","grantId":"g1","ready":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "shell: no cwd",
            &["shell"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: remote-session write
        case(
            "write",
            &["write", "s 1", "ls -l é"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "write: an empty answer",
            &["write", "s1", "x"],
            &[],
            vec![reply("")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "write: not JSON",
            &["write", "s1", "x"],
            &[],
            vec![reply("done")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "write: lines only",
            &["write", "s1", "x"],
            &[],
            vec![reply(r#"{"lines":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "write: expired, taught",
            &["write", "s1", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"GRANT_EXPIRED","hint":"Grant expired. Mint a new one: k2 remote-session grant [--ttl 30m]"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case("write: no text", &["write", "s1"], &[], vec![], owner_and_port, 2, 0),
        // covers: remote-session read
        case(
            "read",
            &["read", "s1", "--lines", "3"],
            &[],
            vec![reply(r#"{"lines":["a","b é",3]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read: an error",
            &["read", "s1"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"not_found","hint":"no session"}}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "read: lines that are a number",
            &["read", "s1"],
            &[],
            vec![reply(r#"{"lines":5}"#)],
            owner_and_port,
            1,
            1,
        ),
        case("read: no id", &["read"], &[], vec![], owner_and_port, 2, 0),
        case(
            "an unknown verb: the line and the page",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a value flag as the last word: exit 1, no line",
            &["status", "--host"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "grant: --ttl as the last word",
            &["grant", "--ttl"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn remote_session_grants_and_times() {
    let rig = Rig::new(VERB, "grants");
    let v = vec![
        // covers: remote-session grant
        case(
            "grant",
            &[
                "grant", "--ttl", "1H", "--label", "  pair ", "--scope", "shell",
            ],
            &[],
            vec![reply(
                r#"{"ok":true,"token":"k2rs_abc","grant":{"id":"g1","expiresAt":1763600000,"label":"pair"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "grant: no time, an empty scope",
            &["grant", "--scope", ""],
            &[],
            vec![reply(r#"{"ok":true,"grant":{"expires_at":"soon"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "grant: a ttl it refuses",
            &["grant", "--ttl", "1d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: remote-session grants
        case(
            "grants: none",
            &["grants"],
            &[],
            vec![reply(r#"{"ok":true,"grants":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "grants: the reader raises with a grant (bash bug)",
            &["grants"],
            &[],
            vec![reply(
                r#"{"ok":true,"grants":[{"id":"g1","expiresAt":60},{"id":"g2","expiresAt":120,"revokedAt":1}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // covers: remote-session grants --json
        case(
            "grants --json",
            &["grants", "--json"],
            &[],
            vec![reply(r#"{"ok":true,"grants":[{"id":"g1","label":"é"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "status: denials with times",
            &["status"],
            &[],
            vec![reply(
                r#"{"ok":true,"enabled":true,"activeGrants":1,"recentDenials":[{"createdAt":1763600000,"code":"NO_GRANT","principalLabel":"bo","payload":"0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890"},{"createdAt":"x"},{},3,{},{},{},{},{},{},{"code":"eleventh"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"id":"grant-{i:05}","scope":"shell","expiresAt":{},"label":"{}","issuedBy":"ann"}}"#,
                1_763_600_000 + i,
                "\u{e9}x".repeat(i % 30)
            )
        })
        .collect();
    let big: &'static str =
        Box::leak(format!(r#"{{"ok":true,"grants":[{}]}}"#, rows.join(",")).into_boxed_str());
    assert!(big.len() > 256 * 1024);
    rig.check(&case(
        "grants --json: above 256 KiB",
        &["grants", "--json"],
        &[],
        vec![reply(big)],
        owner_and_port,
        0,
        1,
    ));
    rig.check(&traceback(case(
        "grants: above 256 KiB",
        &["grants"],
        &[],
        vec![reply(big)],
        owner_and_port,
        1,
        1,
    )));
}

#[test]
fn remote_session_tokens_gates_and_no_answer() {
    let rig = Rig::new(VERB, "tokens");
    let v = vec![
        stdin(
            b"k2rs_fromstdin\nrest\n",
            case(
                "a grant token on stdin",
                &["read", "s1", "--token-stdin"],
                &[],
                vec![reply(r#"{"lines":["x"]}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        case(
            "a grant token on the command line",
            &["write", "s1", "x", "--token", "k2rs_argv"],
            &[],
            vec![reply(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "both",
            &["status", "--token", "k2rs_a", "--token-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no token on stdin",
            &["status", "--token-stdin"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no port or token: the gate",
            &["status"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "a session with no passport: the locked tool",
            &["status"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        waited(case(
            "status: an empty reply",
            &["status"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        )),
        waited(case(
            "enable: an empty reply",
            &["enable"],
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
    let dead = vec![("K2_PORT", dead_port())];
    for c in [
        case(
            "status: nothing listening",
            &["status"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "write: nothing listening reads as written",
            &["write", "s1", "x"],
            &[],
            vec![],
            owner_only,
            0,
            0,
        ),
    ] {
        rig.check(&with(dead.clone(), c));
    }
}

/// `-h` or `--help`: the shell's (the binary hands the line over); its
/// router prints the help page and sends nothing.
#[test]
fn remote_session_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for args in [
        &["enable", "--help"][..],
        &["disable", "-h"],
        &["grant", "--ttl", "1h", "-h"],
        &["revoke", "g1", "--help"],
        &["shell", "-h"],
        &["write", "s1", "x", "--help"],
        &["-h"],
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        rig.check(&shell(case(
            "help acts on nothing",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )));
    }
}

/// `c` on the rig, then the binary alone once more: it answered the line
/// itself, so it never wrote the shell CLI out to hand it over.
fn answered_here(rig: &Rig, c: &Case) {
    rig.check(c);
    rig.binary(c, false);
    assert!(
        !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "{}: the binary handed the line to the shell",
        c.name
    );
}

/// `--host`: the host's token (`--token`, `--token-stdin`, then
/// `K2_REMOTE_TOKEN`; none is two lines, exit 1), then every verb's call
/// to that server over TCP. The other server is the side's own stub,
/// named as `http://127.0.0.1:<port>`; plain http to a name that is
/// looked up, and `--host` with a session passport, are the shell's.
#[test]
fn remote_session_on_another_server() {
    let rig = Rig::new(VERB, "host");
    const H: &str = "http://127.0.0.1:{port}";
    let one =
        |name, args, body: &str| case(name, args, &[], vec![reply(body)], owner_and_port, 0, 1);
    let remote = |c: Case| with(vec![("K2_REMOTE_TOKEN", "remote-tok-9".to_string())], c);
    for c in [
        // native: remote-session status --host
        remote(one(
            "status",
            &["status", "--host", H],
            r#"{"ok":true,"enabled":true,"activeGrants":1,"activeSessions":[]}"#,
        )),
        remote(one(
            "status --json",
            &["status", "--json", "--host", "http://127.0.0.1:{port}/"],
            r#"{"ok":true,"enabled":false}"#,
        )),
        // native: remote-session enable --host
        remote(one(
            "enable",
            &["enable", "--host", H],
            r#"{"ok":true,"enabled":true}"#,
        )),
        remote(one(
            "disable",
            &["disable", "--host", H],
            r#"{"ok":true,"enabled":false}"#,
        )),
        // native: remote-session grant --host
        one(
            "grant, the token as --token",
            &[
                "grant",
                "--ttl",
                "30m",
                "--label",
                "ci",
                "--host",
                H,
                "--token",
                "remote-tok-1",
            ],
            r#"{"ok":true,"token":"k2rs_abc","grant":{"id":"g1","expiresAt":1763600000,"label":"ci"}}"#,
        ),
        // native: remote-session grants --host
        remote(one(
            "grants --json",
            &["grants", "--json", "--host", H],
            r#"{"ok":true,"grants":[]}"#,
        )),
        // native: remote-session revoke --host
        remote(one(
            "revoke",
            &["revoke", "g1", "--host", H],
            r#"{"ok":true,"grant":{"id":"g1"},"killedSessions":0}"#,
        )),
        // native: remote-session shell --host
        remote(one(
            "shell",
            &["shell", "--cwd", "/tmp", "--host", H],
            r#"{"ok":true,"sessionId":"s9","grantId":"g1","ready":true}"#,
        )),
        // native: remote-session write --host
        stdin(
            b"remote-tok-2\n",
            one(
                "write, the token on stdin",
                &["write", "s9", "ls -la", "--host", H, "--token-stdin"],
                r#"{"ok":true}"#,
            ),
        ),
        // native: remote-session read --host
        remote(one(
            "read",
            &["read", "s9", "--lines", "20", "--host", H],
            r#"{"lines":["a","b"]}"#,
        )),
        remote(case(
            "the other server's refusal",
            &["status", "--host", H],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"REMOTE_SESSIONS_DISABLED","hint":"off"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        )),
        case(
            "no token for the host",
            &["status", "--host", H],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "the connection gate first",
            &["status", "--host", H],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
    for c in [
        remote(shell(one(
            "plain http to a name",
            &["status", "--host", "http://localhost:{port}"],
            r#"{"ok":true}"#,
        ))),
        with(
            with_passport(&[("K2_REMOTE_TOKEN", "remote-tok-9")]),
            shell(case(
                "with a session passport",
                &["status", "--host", H],
                &[],
                vec![reply(r#"{"ok":true}"#)],
                port_only,
                0,
                1,
            )),
        ),
    ] {
        rig.check(&c);
    }
}
