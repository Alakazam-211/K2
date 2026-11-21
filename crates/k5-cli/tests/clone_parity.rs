//! `k5-cli clone` against the shell CLI's `cli/k2 clone`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for the proposal verbs (`proposals`, `propose`, `withdraw`,
//! `deny`) and the help. The engine's verbs (`preview`, `to`, `from`,
//! `here`, `approve`) are `tests/clone_engine_parity.rs`'s.
//!
//! - Each verb's answer, `--json`, the daemon's refusals (`_compute_fail`
//!   lines and exits), Python's raise on an odd answer.
//! - The usage stops (`_clone_die`'s JSON line), a value flag as the last
//!   word on each verb (`_need_value`'s line, exit 2), the person guard.
//! - The cell socket in a session, `deny` over TCP; no answer.
//! - One large list (above 256 KiB).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "clone";

const LIST: &str = r#"{"proposals":[{"id":"cp-1","status":"open","summary":"clone ws-a to here","proposedBy":"agent:kessel","why":"a backup é"},{"id":"cp-2","status":"denied","summary":"clone ws-b to acme.example","proposedBy":"agent:x","decidedBy":"ann"}]}"#;
const HELD: &str = r#"{"proposal":{"id":"cp-3","summary":"clone ws-a to here","status":"open"},"hint":"a person approves: k2 clone approve cp-3"}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn clone_proposals_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        case("the help", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        // covers: clone
        case(
            "proposals",
            &["proposals"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        // covers: clone --json
        case(
            "proposal --all --json",
            &["proposal", "--all", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "proposals: none",
            &["proposals", "x", "--mine"],
            &[],
            vec![ok(r#"{"proposals":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "proposals: refused",
            &["proposals"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"people only"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "proposals: a card",
            &["proposals"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "proposals: a bad request",
            &["proposals"],
            &[],
            vec![Reply::Answer(400, "nope".into())],
            owner_and_port,
            2,
            1,
        ),
        traceback(case(
            "proposals: a row with no id",
            &["proposals"],
            &[],
            vec![ok(r#"{"proposals":[{"status":"open"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "proposals: not JSON",
            &["proposals"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        )),
        // covers: clone propose
        case(
            "propose",
            &[
                "propose",
                "--to",
                "here",
                "--why",
                "a copy",
                "--copy",
                "a, b",
                "--skip",
                "c",
                "--no-threads",
                "--threads-since",
                "007",
                "--dest",
                "/srv/w",
                "--workspace",
                "ws-a",
            ],
            &[],
            vec![ok(HELD)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "propose --json",
            &["propose", "--to", "acme.example", "--live-only", "--json"],
            &[],
            vec![ok(HELD)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "propose: no --to",
            &["propose", "--why", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "propose: a word",
            &["propose", "ws"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "propose: an unknown flag",
            &["propose", "--frob"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "proposals: --why last",
            &["proposals", "--why"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "propose: --since-days last",
            &["propose", "--to", "here", "--since-days"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "withdraw: --reason last",
            &["withdraw", "cp-1", "--reason"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "deny: --workspace last",
            &["deny", "cp-1", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "preview: --host last",
            &["preview", "ws1", "--host"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "to: --token last",
            &["to", "https://h.example", "ws1", "--token"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "from: --skip last",
            &["from", "h.example:ws1", "--copy", "a", "--skip"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "here: --dest last",
            &["here", "ws1", "--dest"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "approve: --to last",
            &["approve", "cp-1", "--to"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a bare workspace: --blank last",
            &["ws1", "--blank"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "propose: days that are not a number",
            &["propose", "--to", "here", "--since-days", "2d"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "propose: refused",
            &["propose", "--to", "here"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"clone_not_allowed","message":"off here"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "withdraw",
            &["withdraw", "cp-1"],
            &[],
            vec![ok(r#"{"proposal":{"id":"cp-1","status":"withdrawn"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "withdraw: no id",
            &["withdraw"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "deny",
            &["deny", "cp-1", "--reason", "not now"],
            &[],
            vec![ok(r#"{"proposal":{"id":"cp-1","status":"denied"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "deny: two ids",
            &["deny", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        with(
            with_passport(&[]),
            case(
                "deny with a passport: the person guard",
                &["deny", "cp-1"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "propose on the cell socket",
                &["propose", "--to", "here"],
                &[],
                vec![ok(HELD)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "proposals on the cell socket",
                &["proposals"],
                &[],
                vec![ok(LIST)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "no answer",
            &["proposals"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn clone_proposals_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"id":"cp-{i:05}","status":"open","summary":"clone workspace-{i} to acme.example","proposedBy":"agent:a{i}","why":"weekly copy {i}"}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"proposals":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "a large list",
        &["proposals"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn clone_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["withdraw"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("{{\"error\": \"usage\", \"hint\": \"usage: {cli} clone withdraw <id>\"}}\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
    let c = case(
        "brand: an engine verb",
        &["to", "http://h.example", "w", "--token", "t"],
        &[],
        vec![],
        owner_and_port,
        3,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    let product = k2_client::brand::PRODUCT_NAME;
    assert_eq!(
        out.stderr,
        format!("note: --token is visible to every account on this computer while the command runs; use --token-stdin\nrefused: h.example isn't https, so {product} won't send your token or password there; use https://h.example\n")
    );
    assert_eq!(out.code, Some(3));
    assert!(log.is_empty());
    let c = case("brand: here", &["here"], &[], vec![], owner_and_port, 2, 0);
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "{{\"error\": \"usage\", \"hint\": \"usage: {cli} clone here <ws> [--dest DIR]\"}}\n"
        )
    );
}

/// Help never acts.
#[test]
fn clone_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("propose -h", &["propose", "--to", "here", "-h"]),
        shell("deny --help", &["deny", "cp-1", "--help"]),
        shell("withdraw -h", &["withdraw", "-h", "cp-1"]),
        shell("to -h", &["to", "https://h.example", "ws1", "-h"]),
        shell("from --help", &["from", "h.example:ws1", "--help"]),
        shell("here -h", &["here", "ws1", "-h"]),
        shell("approve --help", &["approve", "cp-1", "--help"]),
        shell("preview -h", &["preview", "ws1", "-h"]),
        shell("a bare workspace -h", &["ws1", "--dest", "/d", "-h"]),
    ] {
        rig.check(&c);
    }
}
