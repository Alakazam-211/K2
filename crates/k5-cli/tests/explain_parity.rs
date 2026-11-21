//! `k5-cli explain` against the shell CLI's `cli/k2 explain`, on the
//! parity rig ([`common::parity`]): nothing sent, the same stdout, stderr
//! and exit code.
//!
//! - The registry the binary carries is `cli/k2`'s `_k2_refusals_json`.
//! - Every id in it, as text, and every tenth as `--json`; an alias; an id
//!   with spaces round it and in capitals; an unknown id, text and JSON.
//! - The list, text and JSON. The words: the usage (`--help`, `-h`, after
//!   an id), an unknown flag (before `--help` too), a second id, an empty
//!   word.
//!   A line with `-h` or `--help` that is no help page goes to the shell
//!   (the front's rule): those rows compare as the shell's.
//! - The settings: an owner's terminal, an agent on its cell socket and
//!   over TCP, `HOOK_SOCK` with no passport, no token and no port (the
//!   gate is skipped), the virtual desktop's guard (a help word lets it
//!   by), a `CODEX_` setting.
//! - A Markdown kit id: the kit's explain on the daemon's own read,
//!   `GET /cli/kit/explain?id=<id>[&json=1]` (it answers with Board files
//!   off), one GET with no path, beside a decoy file of the id's name; the
//!   kit's page (and one above 256 KiB), its JSON, its row, a refusal, an
//!   older daemon without the route (404 or 405: one line saying to update
//!   it, exit 1), an answer that is not JSON.

#![cfg(unix)]

mod common;

use common::parity::*;
use k5_cli::out::json::{self, Value};

const VERB: &str = "explain";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// A line the front hands to the shell: one with `-h` or `--help` in it
/// that is no help page (`explain --help` and `explain -h` are).
fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

/// Every row's id and aliases, in the registry's order.
fn rows() -> Vec<(String, Vec<String>)> {
    let reg = json::parse(k5_cli::families::explain::registry()).expect("the registry is JSON");
    let Value::Object(top) = reg else {
        panic!("the registry is not an object")
    };
    let rows = top
        .iter()
        .find(|(k, _)| k.as_str() == Some("rows"))
        .map(|(_, v)| v.clone())
        .expect("the registry has rows");
    let Value::Array(rows) = rows else {
        panic!("rows is not a list")
    };
    rows.iter()
        .map(|r| {
            let Value::Object(pairs) = r else {
                panic!("a row is not an object: {r:?}")
            };
            let field = |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| n.as_str() == Some(k))
                    .map(|(_, v)| v)
            };
            let Some(Value::Str(id)) = field("id") else {
                panic!("a row with no id: {r:?}")
            };
            let aliases = match field("aliases") {
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|v| match v {
                        Value::Str(t) => t.as_str().expect("an alias is UTF-8").to_string(),
                        other => panic!("an alias is not a string: {other:?}"),
                    })
                    .collect(),
                other => panic!("aliases is not a list: {other:?}"),
            };
            (id.as_str().expect("an id is UTF-8").to_string(), aliases)
        })
        .collect()
}

#[test]
fn the_registry_is_cli_k2_s() {
    const OPEN: &str = "\n_k2_refusals_json() {\n    cat <<'EOF'\n";
    let script = std::fs::read_to_string(cli_k2()).expect("read cli/k2");
    let start = script.find(OPEN).expect("cli/k2 defines _k2_refusals_json") + OPEN.len();
    let end = script[start..].find("\nEOF\n").expect("the heredoc's end");
    assert_eq!(
        k5_cli::families::explain::registry(),
        &script[start..start + end]
    );
    assert!(rows().len() > 100);
}

#[test]
fn every_id() {
    let rig = Rig::new(VERB, "ids");
    let all = rows();
    for (n, (id, aliases)) in all.iter().enumerate() {
        let id = leak(id.clone());
        rig.check(&case(
            "an id",
            words(vec![id]),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
        if n % 10 == 0 {
            rig.check(&case(
                "an id as JSON",
                words(vec![id, "--json"]),
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ));
        }
        for a in aliases {
            rig.check(&case(
                "an alias",
                words(vec![leak(a.clone())]),
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ));
        }
    }
    assert!(
        all.iter().any(|(_, a)| !a.is_empty()),
        "no row has an alias: the alias case proves nothing"
    );
    let first = leak(format!("  {}\t", all[0].0.to_uppercase()));
    rig.check(&case(
        "spaces round it, in capitals",
        words(vec![first]),
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    ));
}

#[test]
fn the_list_and_the_words() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        // covers: explain
        case("the list", &[], &[], vec![], owner_and_port, 0, 0),
        case(
            "an empty id is the list",
            &[""],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an id of spaces is the list",
            &["   "],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: explain --json
        case(
            "the list as JSON",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown id",
            &["no_such_thing"],
            &[],
            vec![],
            owner_and_port,
            4,
            0,
        ),
        case(
            "an unknown id as JSON",
            &["--json", "Odd 'id' é"],
            &[],
            vec![],
            owner_and_port,
            4,
            0,
        ),
        case("--help", &["--help"], &[], vec![], owner_and_port, 0, 0),
        shell(case(
            "-h after an id",
            &["x", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        case(
            "an unknown flag",
            &["--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        shell(case(
            "an unknown flag before --help",
            &["--bogus", "--help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        case("a lone dash", &["-"], &[], vec![], owner_and_port, 2, 0),
        case(
            "a second id",
            &["a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty second word",
            &["a", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        shell(case(
            "--help after a second id",
            &["a", "b", "--help"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn the_settings() {
    let rig = Rig::new(VERB, "env");
    let id = leak(rows()[0].0.clone());
    let one = words(vec![id]);
    let row = |name, setup, code| case(name, one, &[], vec![], setup, code, 0);
    let v = vec![
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            row("an agent on its cell socket: nothing sent", nothing, 0),
        ),
        with(with_passport(&[]), row("an agent over TCP", nothing, 0)),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            row(
                "HOOK_SOCK set and no passport: no governed tool",
                owner_and_port,
                0,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            row("the socket gone: nothing asked", nothing, 0),
        ),
        row("no token: the gate is skipped", port_only, 0),
        row("no port: the gate is skipped", owner_only, 0),
        row("neither", nothing, 0),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            row("the virtual desktop's guard", owner_and_port, 3),
        ),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            case(
                "a help word lets the guard by",
                &["--help"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            shell(case(
                "a help word after an unknown flag",
                &["--x", "-h"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )),
        ),
        with(
            with_passport(&[("K2_NO_OWNER_FALLBACK", "1")]),
            row("the guard lets a passport by", nothing, 0),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}"), ("CODEX_THREAD_ID", "t1")]),
            row("a CODEX_ setting", nothing, 0),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// What the kit's explain answers for a kit id (`{exit, stdout}`).
const KIT: &str = r#"{"exit":0,"stdout":"MPC-E001  a fence was left open\nfix: close it\n"}"#;
/// The same as JSON (`json=1`: the kit's JSON page).
const KIT_JSON: &str = r#"{"exit":0,"stdout":"{\"id\":\"MPC-E001\",\"fix\":\"close it\"}\n"}"#;
/// The kit's row for an id it doesn't know.
const KIT_ROW: &str = r#"{"id":"MPC-E110","code":"unknown_id","message":"no error id MPC-E404","next":"explain lists every id","exit":4,"on":{"path":""}}"#;

/// A decoy file named like each id the rows ask, where the command runs.
fn decoys(r: &Rig) {
    owner_and_port(r);
    for id in ["MPC-E001", "mpc-e001", "MPCE-E002", "mpce-1", "MPC-E404"] {
        std::fs::write(r.proj.join(id), "a decoy, never read\n").unwrap();
    }
}

/// The kit's page above 256 KiB.
fn big_page() -> &'static str {
    let line = "MPC-E001  a line of the kit's page, long enough to add up\n";
    let page: String = line.repeat(300 * 1024 / line.len() + 1);
    let body = format!(
        "{{\"exit\":0,\"stdout\":{}}}",
        k5_cli::out::json::to_string(
            &Value::Str(page.as_str().into()),
            &json::Options {
                compact: true,
                ..json::Options::default()
            }
        )
        .unwrap()
    );
    Box::leak(body.into_boxed_str())
}

/// A Markdown kit id is the kit's own explain (0.46.17): one
/// `GET /cli/kit/explain?id=<id>[&json=1]`, no path in it, then
/// `_explain_kit`'s reader. Before 0.46.17 it was `cmd_doc explain <id>`,
/// which took the id as a doc's path: each row runs beside a decoy file of
/// the id's name, and no request names it. The route answers with Board
/// files off; it was the board route's meta verb, which doesn't.
#[test]
fn a_kit_id_is_the_kit_s_explain() {
    let rig = Rig::new(VERB, "kit");
    let one = |name, args, body: &str, code| case(name, args, &[], vec![ok(body)], decoys, code, 1);
    let answer = |name, args, status, body: &str, code| {
        case(
            name,
            args,
            &[],
            vec![common::stub::Reply::Answer(status, body.into())],
            decoys,
            code,
            1,
        )
    };
    let native = vec![
        // covers: explain
        one("a kit id", &["MPC-E001"], KIT, 0),
        one("in lower case", &["mpc-e001"], KIT, 0),
        one("an MPCE id", &["MPCE-E002"], KIT, 0),
        // covers: explain --json
        one("as JSON", &["MPC-E001", "--json"], KIT_JSON, 0),
        one("--json first", &["--json", "mpce-1"], KIT_JSON, 0),
        one("a quote and a backslash in the id", &["MPC-\"x\\y"], KIT, 0),
        one(
            "the kit's exit",
            &["MPC-E001"],
            r#"{"exit":5,"stdout":"held\n"}"#,
            5,
        ),
        one("a page above 256 KiB", &["MPC-E001"], big_page(), 0),
        answer("the kit's row", &["MPC-E404"], 422, KIT_ROW, 4),
        answer(
            "the kit's row as JSON",
            &["MPC-E404", "--json"],
            422,
            KIT_ROW,
            4,
        ),
        answer(
            "a refusal (exit 3)",
            &["MPC-E001"],
            403,
            r#"{"error":{"code":"forbidden","hint":"not for this caller"}}"#,
            3,
        ),
        answer(
            "a refusal, as JSON",
            &["MPC-E001", "--json"],
            403,
            r#"{"error":{"code":"forbidden","hint":"not for this caller"}}"#,
            3,
        ),
        answer(
            "an older daemon without the route",
            &["MPC-E001"],
            404,
            r#"{"error":"not found"}"#,
            1,
        ),
        answer(
            "an older daemon without the route, as JSON",
            &["MPC-E001", "--json"],
            404,
            r#"{"error":"not found"}"#,
            1,
        ),
        answer(
            "an older daemon without the route: a 405",
            &["MPC-E001"],
            405,
            r#"{"error":"GET or POST only"}"#,
            1,
        ),
        one("an id curl must escape", &["MPC-E 1&x=%"], KIT, 0),
        answer("an answer that is not JSON", &["MPC-E001"], 500, "oops", 1),
        answer(
            "an empty answer, as JSON",
            &["MPC-E001", "--json"],
            502,
            "",
            1,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "an agent on its cell socket",
                &["MPC-E001"],
                &[],
                vec![ok(KIT)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in native {
        rig.check(&c);
        let (out, sent) = rig.binary(&c, false);
        // K2's ruling: an older daemon (no such route) is one line, exit 1.
        if c.name.starts_with("an older daemon") {
            let line = format!("{}\n", k5_cli::families::explain::OLDER_SERVER);
            assert_eq!(out.stderr, line, "{}", c.name);
            assert_eq!((out.stdout.as_str(), out.code), ("", Some(1)), "{}", c.name);
        }
        assert!(
            !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
            "{}: the binary handed the line to the shell",
            c.name
        );
        assert_eq!(sent.len(), 1, "{}: {sent:?}", c.name);
        let (head, body) = sent[0].split_once("\n\n").unwrap();
        let line = head
            .lines()
            .find(|l| l.starts_with("GET ") || l.starts_with("POST "))
            .unwrap_or_else(|| panic!("{}: no request line: {head}", c.name));
        let (target, _) = line["GET ".len()..]
            .rsplit_once(' ')
            .unwrap_or_else(|| panic!("{}: {line}", c.name));
        let query = target
            .strip_prefix("/cli/kit/explain?")
            .unwrap_or_else(|| panic!("{}: one GET of the kit's explain: {line}", c.name));
        let keys: Vec<_> = query
            .split('&')
            .map(|kv| kv.split('=').next().unwrap())
            .collect();
        let want: &[&str] = if c.args.contains(&"--json") {
            &["id", "json"]
        } else {
            &["id"]
        };
        assert_eq!(keys, want, "{}: no path or file is sent: {line}", c.name);
        assert_eq!(body, "", "{}: a GET has no body", c.name);
        assert!(
            !head.contains("decoy") && !head.contains(&*rig.proj.to_string_lossy()),
            "{}: {head}",
            c.name
        );
    }
    let guard = with(
        vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
        case(
            "the virtual desktop's guard",
            &["MPC-E001"],
            &[],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    );
    rig.check(&guard);
}

/// The lines name the brand (in a build with `K2_BRAND_DIR=brand/xy` they
/// say `xy` and `XY`): the usage of a usage error, a kit id's error line,
/// and the older server's line.
#[test]
fn explain_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("two ids", &["a", "b"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("usage: {cli} explain [<id>] [--json]\n")
    );
    assert!(log.is_empty(), "{log:?}");
    let c = case(
        "a kit id's row",
        &["MPC-E404"],
        &[],
        vec![common::stub::Reply::Answer(422, KIT_ROW.into())],
        decoys,
        4,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(out.code, Some(4), "{out:?}");
    assert!(
        !out.stderr.contains("k5 ") && !out.stderr.contains("k2 "),
        "{}",
        out.stderr
    );
    assert_eq!(log.len(), 1, "{log:?}");
    let product = k2_client::brand::PRODUCT_NAME;
    let c = case(
        "an older daemon",
        &["MPC-E001"],
        &[],
        vec![common::stub::Reply::Answer(
            404,
            r#"{"error":"not found"}"#.into(),
        )],
        decoys,
        1,
        1,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!(
            "This {product} server is older and can't explain kit codes yet; update it ({cli} update).\n"
        )
    );
    assert_eq!((out.code, log.len()), (Some(1), 1), "{out:?}");
}
