//! `k5-cli compute provenance|verify ...` against the shell CLI's
//! `cli/k2 compute ...`, on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code, and for `provenance --out` the
//! same file.
//!
//! - `provenance`: the line as it is, `--json`, `--out` (the file and what
//!   it names), a raise before and after the file is made.
//! - `verify`: a job, a statement file (one line, several, not JSON),
//!   `--file` (its sha256 and name in the body), `--owner-key`, `--json`,
//!   and exit 1 when the daemon says not verified.
//! - The words: usage lines (exit 2); nothing is sent after one.
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "compute";

fn raises(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn one(name: &'static str, args: &'static [&'static str], body: &'static str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn words(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

const PROV: &str = r#"{"line":"{\"payloadType\":\"application/vnd.in-toto+json\",\"payload\":\"e30=\"}\n","short":"abcd1234","node":"mini","subjects":[{"name":"out/k5","sha256":"aa11"},{"name":"out/k5.sig","sha256":"bb22"}]}"#;

#[test]
fn compute_provenance() {
    let rig = Rig::new(VERB, "prov");
    let v = vec![
        // covers: compute provenance
        one("the line", &["provenance", "abcd1234"], PROV, 0),
        one("a line with no newline", &["provenance", "abcd"], r#"{"line":"x é"}"#, 0),
        raises(one("no line", &["provenance", "abcd"], r#"{"short":"a"}"#, 1)),
        raises(one("a line that is a number", &["provenance", "abcd"], r#"{"line":5}"#, 1)),
        // covers: compute provenance --json
        one("--json", &["provenance", "--json", "abcd1234"], PROV, 0),
        one("--out", &["provenance", "abcd1234", "--out", "s.intoto.jsonl"], PROV, 0),
        one("--out, no node, no subjects", &["provenance", "abcd1234", "--out", "s.jsonl"], r#"{"line":"L","short":"ab"}"#, 0),
        raises(one("--out, no short", &["provenance", "abcd1234", "--out", "s.jsonl"], r#"{"line":"L"}"#, 1)),
        raises(one("--out, subjects that are a number", &["provenance", "a", "--out", "s.jsonl"], r#"{"line":"L","short":"ab","subjects":3}"#, 1)),
        raises(one("--out into no folder", &["provenance", "a", "--out", "no/such/s.jsonl"], PROV, 1)),
        case(
            "the daemon's error",
            &["provenance", "abcd"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"no_statement","hint":"none kept"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        words("no job", &["provenance"], 2),
        words("--file", &["provenance", "a", "--file", "x"], 2),
        words("--owner-key", &["provenance", "a", "--owner-key", "fp"], 2),
        words("--out with nothing", &["provenance", "a", "--out"], 2),
        words("--out empty", &["provenance", "a", "--out", ""], 2),
        words("two jobs", &["provenance", "a", "b"], 2),
        words("an unknown flag", &["provenance", "a", "--frob"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `--out`: each side writes the same file.
#[test]
fn compute_provenance_out_file() {
    let rig = Rig::new(VERB, "provout");
    let path = rig.proj.join("s.intoto.jsonl");
    for (args, body) in [
        (&["provenance", "abcd1234", "--out", "s.intoto.jsonl"][..], PROV),
        (&["provenance", "abcd1234", "--out", "s.intoto.jsonl"][..], r#"{"short":"ab"}"#),
    ] {
        let c = case("out", args, &[], vec![ok(body)], owner_and_port, 0, 1);
        let mut files = vec![];
        for bash in [true, false] {
            let _ = std::fs::remove_file(&path);
            rig.run(bash, &c);
            files.push(std::fs::read(&path).ok());
        }
        assert_eq!(files[0], files[1], "{args:?}: the file");
        assert!(files[0].is_some(), "{args:?}: bash made the file");
    }
}

const VERIFIED: &str = r#"{"short":"abcd1234","node":"mini","verified":true,"checks":[{"ok":true,"check":"signature","detail":"node key fp1"},{"ok":false,"check":"subject","detail":"no file given"}],"notChecked":"the roster"}"#;
const NOT_VERIFIED: &str = r#"{"short":"abcd1234","verified":false,"failed":"signature","checks":[],"notChecked":"x"}"#;

fn files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("one.jsonl"), "\n{\"payload\":\"e30=\",\"n\":1.5,\"u\":\"é\"}\r\n  \n").unwrap();
    std::fs::write(r.proj.join("two.jsonl"), "{}\n{}\n").unwrap();
    std::fs::write(r.proj.join("bad.jsonl"), "{nope\n").unwrap();
    std::fs::write(r.proj.join("latin.jsonl"), b"\xff{}\n").unwrap();
    std::fs::write(r.proj.join("k5.bin"), b"some bytes\n").unwrap();
    std::fs::create_dir_all(r.proj.join("dir")).unwrap();
}

fn with_files(c: Case) -> Case {
    Case { setup: files, ..c }
}

#[test]
fn compute_verify() {
    let rig = Rig::new(VERB, "verify");
    let v = vec![
        // covers: compute verify
        one("a job, verified", &["verify", "abcd1234"], VERIFIED, 0),
        one("a job, not verified", &["verify", "abcd1234"], NOT_VERIFIED, 1),
        with_files(one("a statement file and --file", &["verify", "one.jsonl", "--file", "k5.bin", "--owner-key", "fp9"], VERIFIED, 0)),
        with_files(one("--file in a folder", &["verify", "abcd", "--file", "./k5.bin"], VERIFIED, 0)),
        // covers: compute verify --json
        one("--json", &["verify", "abcd1234", "--json"], VERIFIED, 0),
        one("--json, not verified", &["verify", "--json", "abcd1234"], NOT_VERIFIED, 1),
        raises(one("--json, no verified", &["verify", "--json", "abcd1234"], r#"{"short":"a"}"#, 1)),
        raises(one("no checks", &["verify", "abcd1234"], r#"{"short":"a","verified":true}"#, 1)),
        raises(one("notChecked a number", &["verify", "abcd1234"], r#"{"short":"a","verified":true,"checks":[],"notChecked":1}"#, 1)),
        with_files(words("two lines", &["verify", "two.jsonl"], 2)),
        with_files(words("not JSON", &["verify", "bad.jsonl"], 2)),
        with_files(raises(words("not UTF-8", &["verify", "latin.jsonl"], 2))),
        with_files(words("--file not there", &["verify", "abcd", "--file", "none.bin"], 2)),
        with_files(words("--file a folder", &["verify", "abcd", "--file", "dir"], 2)),
        with_files(one("a folder as the target is a job", &["verify", "dir"], VERIFIED, 0)),
        case(
            "the daemon's error",
            &["verify", "abcd"],
            &[],
            vec![Reply::Answer(400, r#"{"error":{"code":"bad_envelope","hint":"no"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case("an empty reply", &["verify", "abcd"], &[], vec![Reply::Close], owner_and_port, 3, 1),
        words("no target", &["verify"], 2),
        words("--out", &["verify", "a", "--out", "x"], 2),
        words("--file with nothing", &["verify", "a", "--file"], 2),
        words("--owner-key with nothing", &["verify", "a", "--owner-key"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name.
#[test]
fn the_provenance_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "provbrand");
    for (args, want) in [
        (
            &["provenance"][..],
            format!("usage: {cli} compute provenance <job> [--out FILE] [--json]\n"),
        ),
        (
            &["verify", "a", "--frob"],
            format!("{cli} compute verify: unknown flag --frob ({cli} compute --help)\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// Help never acts.
#[test]
fn compute_provenance_help_never_acts() {
    let rig = Rig::new(VERB, "provhelp");
    let lines: &[&[&str]] = &[
        &["provenance", "a", "--out", "x", "-h"],
        &["verify", "a", "--help"],
        &["verify", "-h", "a", "--file", "x"],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..case("help", args, &[], vec![], owner_and_port, 0, 0)
        });
    }
}
