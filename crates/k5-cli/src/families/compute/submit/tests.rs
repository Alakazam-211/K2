//! The submit verbs' words, bodies and readers, against what the shell's
//! `cmd_compute_run`, `cmd_compute_test` and `cmd_compute_prewarm` and
//! their Python do with them.

use super::super::writes::Words;
use super::{bytes, distro, prewarm, run, set, test};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

fn ask<T: std::fmt::Debug>(w: Option<Words<T>>) -> T {
    match w {
        Some(Words::Ask(a)) => a,
        other => panic!("expected an ask, got {other:?}"),
    }
}

fn usage<T: std::fmt::Debug>(w: Option<Words<T>>) -> String {
    match w {
        Some(Words::Usage(l)) => l,
        other => panic!("expected a usage line, got {other:?}"),
    }
}

#[test]
fn run_words() {
    let a = ask(run::parse(&[
        "mini", "--cpus", "4+8", "--mem", "2G", "--", "make", "-j",
    ]));
    assert_eq!(a.node, "mini");
    assert!(!a.any);
    assert_eq!(a.cpus, run::Num(Some("4".into())));
    assert_eq!(a.cpus_burst, run::Num(Some("8".into())));
    assert_eq!(a.mem, "2147483648");
    assert_eq!(a.argv, vec!["make", "-j"]);
    let a = ask(run::parse(&["--", "true"]));
    assert!(a.any);
    assert_eq!(usage(run::parse(&["mini"])), run::USAGE);
    assert_eq!(
        usage(run::parse(&["--any", "mini", "--", "x"])),
        run::AFTER_DASHES
    );
    assert_eq!(
        usage(run::parse(&["--bogus", "--", "x"])),
        format!("usage: unknown flag '--bogus' ({CLI_NAME} compute --help)")
    );
    assert!(matches!(run::parse(&["--os"]), Some(Words::ShiftFailed)));
    assert_eq!(
        usage(run::parse(&["--timeout", "1x", "--", "x"])),
        "usage: bad duration '1x' (use 2h, 90m, 1h30m, 300s)"
    );
    assert_eq!(
        usage(run::parse(&["--label", "nokey", "--", "x"])),
        "usage: --label key=value"
    );
    assert_eq!(
        usage(run::parse(&["--wait", "--detach", "--", "x"])),
        "usage: --wait and --detach don't mix (--wait blocks until the end; --detach returns at once)"
    );
    // --failures is the shell's; a --cpus that is no plain number too.
    assert!(run::parse(&["--wait", "--failures", "--", "x"]).is_none());
    assert!(run::parse(&["--cpus", "x4", "--", "x"]).is_none());
    // The batch tier is detached.
    let a = ask(run::parse(&["--tier", "batch", "--", "x"]));
    assert!(a.detach);
    // --gpu with a flag after it is any.
    let a = ask(run::parse(&["--gpu", "--detach", "--", "x"]));
    assert_eq!(a.gpu, "any");
    assert_eq!(
        usage(run::parse(&["--gpu", "nvidia", "--", "x"])),
        "usage: --gpu nvidia: NVIDIA GPU jobs aren't supported yet; use --gpu any, amd or intel"
    );
}

#[test]
fn run_body() {
    let a = ask(run::parse(&[
        "mini",
        "--label",
        "a=1",
        "--label",
        "b=2",
        "--label",
        "a=3",
        "--needs",
        "rust@stable, cmake@3",
        "--cpus",
        "2.50",
        "--retries",
        "007",
        "--detach",
        "--",
        "echo",
        "hé",
    ]));
    let body = run::body(&a, "sub", "/repo", "acme", "c0ffee");
    assert_eq!(
        body,
        r#"{"node": "mini", "labels": {"a": "3", "b": "2"}, "env": {}, "argv": ["echo", "h\u00e9"], "needs": ["rust@stable", "cmake@3"], "retries": 7, "plan": false, "detach": true, "dirty": false, "noSource": false, "cpus": 2.5, "exclusive": false, "cwd": "sub", "repoDir": "/repo", "workspace": "acme", "clientJobId": "c0ffee"}"#
    );
    let a = ask(run::parse(&["--wait", "--", "true"]));
    let body = run::body(&a, "", "", "", "c");
    assert_eq!(
        body,
        r#"{"any": true, "labels": {}, "env": {}, "argv": ["true"], "plan": false, "detach": false, "dirty": false, "noSource": false, "exclusive": false, "retryInterrupted": true, "clientJobId": "c"}"#
    );
    let a = ask(run::parse(&[
        "--strict", "--detach", "--", "sh", "-c", "x | y",
    ]));
    assert!(a.strict);
    let body = run::body(&a, "", "", "", "c");
    assert_eq!(
        body,
        r#"{"any": true, "labels": {}, "env": {}, "argv": ["sh", "-c", "x | y"], "plan": false, "detach": true, "dirty": false, "noSource": false, "exclusive": false, "clientJobId": "c", "strict": true}"#
    );
    assert_eq!(run::client_job_id().len(), 32);
}

#[test]
fn sizes_and_distros() {
    assert_eq!(bytes::<()>("16G"), Some(Ok("17179869184".into())));
    assert_eq!(bytes::<()>("512m"), Some(Ok("536870912".into())));
    assert_eq!(bytes::<()>("0010"), Some(Ok("10".into())));
    assert!(matches!(bytes::<()>("1T"), Some(Err(Words::Usage(_)))));
    assert!(matches!(bytes::<()>(""), Some(Err(Words::Usage(_)))));
    assert_eq!(bytes::<()>("99999999999999999999G"), None);
    assert_eq!(
        distro::<()>("ubuntu@24.04"),
        Some(Ok("ubuntu@24.04".into()))
    );
    assert_eq!(distro::<()>("debian"), Some(Ok("debian".into())));
    assert!(matches!(distro::<()>("Ubuntu"), Some(Err(Words::Usage(_)))));
    assert!(matches!(
        distro::<()>("ubuntu@"),
        Some(Err(Words::Usage(_)))
    ));
}

#[test]
fn shlex_as_python() {
    assert_eq!(
        test::shlex_split("cargo  nextest run").unwrap(),
        vec!["cargo", "nextest", "run"]
    );
    assert_eq!(
        test::shlex_split(r#"a "b c" 'd e' f\ g "" h"\"i""#).unwrap(),
        vec!["a", "b c", "d e", "f g", "", "h\"i"]
    );
    assert_eq!(test::shlex_split(r#""a\b""#).unwrap(), vec!["a\\b"]);
    assert!(test::shlex_split("'open").is_none());
    assert!(test::shlex_split("end\\").is_none());
    assert_eq!(test::shlex_quote(""), "''");
    assert_eq!(test::shlex_quote("a-b/c.d"), "a-b/c.d");
    assert_eq!(test::shlex_quote("it's"), "'it'\"'\"'s'");
}

#[test]
fn test_script_and_words() {
    let script = test::script(
        "cargo nextest run",
        "",
        true,
        "count",
        &["-p".into(), "a b".into()],
    )
    .unwrap();
    assert!(script.ends_with(
        "exec cargo nextest run --config-file \"$cfg\" --profile k2 --partition count:{shard}/{shards} --no-tests=pass -p 'a b'"
    ));
    let a = ask(test::parse(&["--shards", "2", "--json", "--", "-p", "x"]));
    assert!(a.any);
    assert_eq!(a.nshards, "2");
    assert!(a.as_json);
    assert_eq!(a.argv[..2], ["sh".to_string(), "-c".to_string()]);
    assert_eq!(
        usage(test::parse(&["mini", "--shards", "3", "--", "x"])),
        format!("usage: --shards picks a node for each shard itself (no node: {PRODUCT_NAME} picks); drop the node name")
    );
    assert_eq!(
        usage(test::parse(&["--wait"])),
        format!("usage: unknown flag '--wait' ({CLI_NAME} compute test --help)")
    );
}

#[test]
fn prewarm_words_and_body() {
    let a = ask(prewarm::parse(&[
        "--needs", "a,b", "--node", "n1", "--plan",
    ]));
    assert_eq!(
        prewarm::body(&a, "/r", "acme"),
        r#"{"needs": ["a", "b"], "nodes": ["n1"], "plan": true, "repoDir": "/r", "workspace": "acme"}"#
    );
    assert_eq!(
        usage(prewarm::parse(&["--plan", "--wait"])),
        "usage: --plan submits nothing, so there is nothing to --wait for"
    );
    assert!(matches!(
        prewarm::parse(&["--tag"]),
        Some(Words::ShiftFailed)
    ));
}

#[test]
fn the_info_line() {
    let i = run::info(
        br#"{"job":{"id":"abcd1234-0000","short":"abcd1234","node":"mini","state":"queued","srcCommit":"0123456789"},"queue":{"position":2,"reason":"busy"},"placement":{"mode":"auto","node":"mini","why":"most free"}}"#,
    )
    .unwrap();
    assert_eq!(i.job, "abcd1234-0000");
    assert_eq!(i.kind, "one");
    assert_eq!(
        i.line,
        format!("[{CLI_NAME} compute] job abcd1234 on mini \u{b7} commit 01234567 \u{b7} queued (position 2: busy)")
    );
    assert_eq!(
        i.placed,
        format!("[{CLI_NAME} compute] placed on mini (auto): most free")
    );
}

#[test]
fn a_split_run_poll() {
    let mut cx = super::super::render::Cx::new(0);
    let mut seen = vec![];
    let mut err = String::new();
    let st = set::poll(
        &mut cx,
        br#"{"state":"done","short":"abcd1234","result":{"exit":1,"durationSecs":125,"shards":[{"index":1,"state":"done","exit":0,"node":"a"},{"index":2,"state":"done","exit":1,"attempt":2,"tests":{"passed":3,"failed":1,"errors":0}}],"tests":{"passed":5,"failed":1,"errors":0}}}"#,
        &mut seen,
        &mut err,
    )
    .unwrap();
    assert!(st.end);
    assert_eq!(st.code, "1");
    assert_eq!(
        err,
        format!(
            "[{CLI_NAME} compute] shard 1/2 on a: exit 0\n[{CLI_NAME} compute] shard 2/2 on ? attempt 2: exit 1 \u{b7} 3 passed, 1 failed\n[{CLI_NAME} compute] 2 shards: exit 1 after 2m05s \u{b7} tests 5 passed, 1 failed \u{b7} {CLI_NAME} compute logs abcd1234 --failures\n"
        )
    );
}
