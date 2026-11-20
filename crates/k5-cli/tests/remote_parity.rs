//! `k5-cli remote` against the shell CLI's `cli/k2 remote`, on the parity
//! rig ([`common::parity`]): the usage, `help` and an unknown verb, with
//! and without a local daemon (`remote` skips the connection gate), and
//! the locked tool's checks; and `logout --host H` to the stub as the
//! other server (plain http to this computer), with the token from stdin
//! or `$K2_REMOTE_TOKEN`, each answer kind, and every usage stop.
//! tests/cli/remote_logout.sh runs the saved-copy removal on both sides.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "remote";

#[test]
fn remote_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: remote
        case(
            "alone: the usage, exit 2",
            &[],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown verb",
            &["login", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no daemon: still the usage",
            &[],
            &[],
            vec![],
            nothing,
            2,
            0,
        ),
        case(
            "HOOK_SOCK set and no passport",
            &["help"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

const HOST: &str = "http://127.0.0.1:{port}";

fn on_stdin(mut c: Case, input: &'static [u8]) -> Case {
    c.stdin = Some(input);
    c
}

#[test]
fn remote_logout_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "logout");
    let dead: &'static str =
        Box::leak(format!("http://127.0.0.1:{}", dead_port()).into_boxed_str());
    let dead_args: &'static [&'static str] =
        Box::leak(vec!["logout", "--host", dead, "--token-stdin"].into_boxed_slice());
    let env_tok = [("K2_REMOTE_TOKEN", "k2rt_env_token_1")];
    let v = vec![
        // covers: remote logout
        on_stdin(
            case(
                "--token-stdin: signed out, nothing saved here",
                &["logout", "--host", HOST, "--token-stdin"],
                &[],
                vec![ok(r#"{"success":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            b"k2rt_stdin_token_1\r\n",
        ),
        case(
            "$K2_REMOTE_TOKEN: signed out",
            &["logout", "--host=http://127.0.0.1:{port}/"],
            &env_tok,
            vec![ok("{}")],
            nothing,
            0,
            1,
        ),
        case(
            "401: refused, nothing removed",
            &["logout", "--host", HOST],
            &env_tok,
            vec![Reply::Answer(401, r#"{"error":"unauthorized"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "403: refused",
            &["logout", "--host", HOST],
            &env_tok,
            vec![Reply::Answer(403, "no\nway".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "500 with a long body: its first 300 characters",
            &["logout", "--host", HOST],
            &env_tok,
            vec![Reply::Answer(500, "\u{e9}".repeat(400))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty reply: no answer",
            &["logout", "--host", HOST],
            &env_tok,
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        on_stdin(
            case(
                "no server there: no answer, one connect",
                dead_args,
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ),
            b"k2rt_dead_token\n",
        ),
        on_stdin(
            case(
                "--token-stdin with nothing on stdin",
                &["logout", "--host", HOST, "--token-stdin"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
            b"",
        ),
        case(
            "no saved token",
            &["logout", "--host", HOST],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no --host", &["logout"], &[], vec![], owner_and_port, 2, 0),
        case(
            "--host with no value",
            &["logout", "--host"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--host with an empty value",
            &["logout", "--host", "", "--token-stdin"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--token on the command line",
            &["logout", "--host", HOST, "--token", "k2rt_on_argv"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--token=",
            &["logout", "--host", HOST, "--token=k2rt_on_argv"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an extra word",
            &["logout", "--host", HOST, "extra"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "plain http to another computer",
            &["logout", "--host", "http://example.invalid"],
            &env_tok,
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "-h: help, nothing sent",
            &["logout", "--host", HOST, "-h"],
            &env_tok,
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "--help first: help, nothing sent",
            &["logout", "--help", "--host", HOST],
            &env_tok,
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "HOOK_SOCK set and no passport",
            &["logout", "--host", HOST],
            &[
                ("K2_HOOK_SOCK", "{sock}"),
                ("K2_REMOTE_TOKEN", "k2rt_env_token_1"),
            ],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn remote_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["nope"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert!(
        out.stderr.starts_with(&format!(
            "{cli} remote: unknown verb 'nope'\n{cli} remote \u{2014}"
        )),
        "{}",
        out.stderr
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
    let c = case(
        "brand: logout's usage",
        &["logout"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: {cli} remote logout --host HOST [--token-stdin]\n")
    );
    assert!(log.is_empty());
    let c = case(
        "brand: logout's --token line",
        &["logout", "--host", "h", "--token", "t"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, _) = rig.binary(&c, false);
    assert!(
        out.stderr
            .starts_with(&format!("{cli} remote logout: no --token")),
        "{}",
        out.stderr
    );
}
