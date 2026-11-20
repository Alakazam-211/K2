//! `k5-cli thread export|import|imports` against the shell CLI's, on the
//! parity rig ([`common::parity`]): the words, the person guard, the
//! three `_compute_call`s and their readers, a failing status, a reader
//! that raises, a large imports table, and help that never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "thread";
const EXPORTED: &str =
    r#"{"workspace":"ws","summary":{"conversations":2,"docs":10},"path":"/x/ws-threads.tar"}"#;
const IMPORTED: &str = r#"{"workspace":"ws2","report":{"docsImported":9,"conversations":["a","b"],"docsSkipped":1,"pending":true}}"#;

fn export_file(rig: &Rig) {
    owner_only(rig);
    std::fs::write(rig.proj.join("ws-threads.tar"), "tar").unwrap();
}

fn plain(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    requests: usize,
) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        ..case(name, args, &[], replies, export_file, code, requests)
    }
}

fn traced(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn thread_transfer_as_the_shell_does_it() {
    let rig = Rig::new(VERB, "transfer");
    let rows = (0..3000)
        .map(|i| {
            format!(
                r#"{{"localConversationId":"c-{i:06}","role":"imported-reader","docCount":{i},"sourceHandle":"ada","sourceHost":"box.example","name":"chat {i}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let big = leak(format!(r#"{{"imports":[{rows}]}}"#));
    let v = vec![
        // covers: thread
        plain("export", &["export", "ws", "--since-days", "007"], vec![ok(EXPORTED)], 0, 1),
        plain("export --json", &["export", "ws", "--json"], vec![ok(EXPORTED)], 0, 1),
        plain("export, no workspace", &["export"], vec![], 2, 0),
        plain("export, --since-days last", &["export", "ws", "--since-days"], vec![], 1, 0),
        plain("an unknown flag", &["export", "ws", "--zap"], vec![], 2, 0),
        plain(
            "export refused",
            &["export", "ws"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
            1,
        ),
        plain(
            "export, a summary that is no object",
            &["export", "ws"],
            vec![ok(r#"{"workspace":"ws","summary":0}"#)],
            0,
            1,
        ),
        traced(plain(
            "export, an answer that is not JSON",
            &["export", "ws"],
            vec![ok("not json")],
            1,
            1,
        )),
        plain(
            "import",
            &["import", "ws-threads.tar", "--to", "ws2"],
            vec![ok(IMPORTED)],
            0,
            1,
        ),
        plain(
            "import from a folder that isn't there",
            &["import", "nowhere/ws-threads.tar", "--to", "ws2", "--json"],
            vec![ok(IMPORTED)],
            0,
            1,
        ),
        plain("import, no --to", &["import", "ws-threads.tar"], vec![], 2, 0),
        plain(
            "imports",
            &["imports", "ws"],
            vec![ok(r#"{"imports":[{"localConversationId":"c1","role":"reader","docCount":3,"sourceHandle":"ada","sourceHost":"box.example"},{"localConversationId":"c2","role":"reader","docCount":12345,"sourceHandle":"bob","sourceHost":"h","name":"notes"}]}"#)],
            0,
            1,
        ),
        plain("no imports", &["imports", "ws"], vec![ok(r#"{"imports":[]}"#)], 0, 1),
        traced(plain(
            "imports, a row without a field",
            &["imports", "ws"],
            vec![ok(r#"{"imports":[{"localConversationId":"c1","role":"r","docCount":1,"sourceHandle":"a","sourceHost":"h"},{"role":"r"}]}"#)],
            1,
            1,
        )),
        plain("imports, a large table", &["imports", "ws"], vec![ok(big)], 0, 1),
        plain("imports, no workspace", &["imports"], vec![], 2, 0),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn thread_transfer_is_for_people() {
    let rig = Rig::new(VERB, "guard");
    for (name, args) in [
        ("export in a session", &["export", "ws"][..]),
        ("import in a session", &["import", "ws-threads.tar", "--to", "ws2"]),
    ] {
        rig.check(&Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            setup: nothing,
            ..plain(name, args, vec![], 3, 0)
        });
    }
}

#[test]
fn thread_transfer_help_never_acts() {
    let rig = Rig::new(VERB, "transferhelp");
    for (name, args) in [
        ("export -h", &["export", "ws", "-h"][..]),
        ("import --help", &["import", "f", "--to", "ws2", "--help"]),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..plain(name, args, vec![], 0, 0)
        });
    }
}
