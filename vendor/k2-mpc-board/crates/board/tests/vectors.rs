//! The vectors, the schema file and the spec's tables all match what generates them, and every
//! vector and every refusal code holds.

mod support;

use k2_mpc_board::{conformance, schema, schema_json, RefusalCode};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
    for e in std::fs::read_dir(dir).expect("read_dir") {
        let p = e.expect("entry").path();
        if p.is_dir() {
            walk(&p, base, out);
        } else {
            let rel = p
                .strip_prefix(base)
                .expect("under base")
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel);
        }
    }
}

#[test]
fn vector_tree_matches_its_generator() {
    let root = repo().join("vectors");
    let files = support::cases::files();
    let mut on_disk = BTreeSet::new();
    walk(&root, &root, &mut on_disk);
    on_disk.remove("check.mjs");
    let want: BTreeSet<String> = files.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        on_disk, want,
        "the files in vectors/ differ from the generator's list"
    );
    for (path, bytes) in &files {
        let got = std::fs::read(root.join(path)).expect("read a vector");
        assert!(&got == bytes, "vectors/{path} differs from its generator");
    }
}

#[test]
fn every_vector_holds() {
    let report = conformance::run(&repo().join("vectors"));
    assert!(
        report.failed.is_empty(),
        "failed vectors: {:#?}",
        report.failed
    );
    assert_eq!(
        report.passed,
        support::cases::all().len(),
        "not every vector ran"
    );
}

#[test]
fn vectors_dir_constant_finds_the_vectors() {
    let report = conformance::run(Path::new(conformance::VECTORS_DIR));
    assert!(
        report.passed > 0,
        "VECTORS_DIR found no cases: {:?}",
        report.failed
    );
}

#[test]
fn every_refusal_code_has_a_vector_or_is_api_only() {
    let report = conformance::run(&repo().join("vectors"));
    let mut missing = Vec::new();
    for c in RefusalCode::ALL {
        if !report.codes_seen.contains(c.as_str()) && !conformance::API_ONLY_CODES.contains(c) {
            missing.push(c.as_str());
        }
    }
    assert!(
        missing.is_empty(),
        "refusal codes with no vector: {missing:?}"
    );
}

/// Runs one planted case through the runner.
fn planted(tag: &str, input: &[u8], case: &str) -> usize {
    let dir = std::env::temp_dir().join(format!("k5board-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("canon")).expect("mkdir");
    std::fs::write(dir.join("canon/a.json"), input).expect("write");
    std::fs::write(dir.join("canon/a.out.json"), b"[1]").expect("write");
    let index = format!(
        "{{\"cases\":[{{\"input\":\"canon/a.json\",\"layer\":\"canon\",\"name\":\"a\",{case}}}]}}"
    );
    std::fs::write(dir.join("index.json"), index).expect("write");
    let report = conformance::run(&dir);
    std::fs::remove_dir_all(&dir).ok();
    report.failed.len()
}

fn planted_pair(tag: &str, pair: &str) -> usize {
    let case = format!("{{\"input\":\"widget/x.json\",\"layer\":\"widget\",\"name\":\"{tag}\"}}");
    let dir = std::env::temp_dir().join(format!("board-pair-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("widget")).expect("a folder");
    std::fs::write(dir.join("widget/x.json"), pair).expect("a pair");
    std::fs::write(
        dir.join("index.json"),
        format!("{{\"cases\":[{case}],\"format_version\":1}}"),
    )
    .expect("an index");
    let report = conformance::run(&dir);
    std::fs::remove_dir_all(&dir).expect("clean up");
    report.failed.len()
}

const OK_BOARD: &str = r#"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}"#;

fn refused_pair(code: &str, file_text: &str) -> String {
    format!(
        "{{\"file_refused\":\"{code}\",\"file_text\":\"{file_text}\",\"lossy\":\"x\",\"lossy_direction\":\"widget_to_file\",\"people_map\":{{}},\"widget_text\":\"{{}}\"}}"
    )
}

#[test]
fn a_refused_pair_has_to_be_refused_with_its_code() {
    let bad = r#"{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"voting\":{\"active\":true,\"perPerson\":0}}}"#;
    assert_eq!(planted_pair("right", &refused_pair("field_value", bad)), 0);
    assert_eq!(
        planted_pair("accepted", &refused_pair("field_value", OK_BOARD)),
        1,
        "a file side that's accepted has to fail"
    );
    assert_eq!(
        planted_pair("other-code", &refused_pair("field_kind", bad)),
        1,
        "another code has to fail"
    );
}

#[test]
fn a_pair_has_to_say_its_direction() {
    let pair = |direction: &str| {
        format!(
            "{{\"file\":{{\"frames\":[],\"objects\":[],\"people\":{{}},\"settings\":{{}}}},\"lossy\":\"x\",{direction}\"people_map\":{{}},\"widget\":{{}}}}"
        )
    };
    assert_eq!(
        planted_pair("good", &pair("\"lossy_direction\":\"both\",")),
        0
    );
    assert_eq!(
        planted_pair("none", &pair("")),
        1,
        "no direction has to fail"
    );
    assert_eq!(
        planted_pair("bad", &pair("\"lossy_direction\":\"sideways\",")),
        1,
        "an unknown direction has to fail"
    );
}

#[test]
fn the_runner_reports_a_wrong_expectation() {
    let ok = "\"canonical\":\"canon/a.out.json\"";
    assert_eq!(planted("ok", b"[1]", ok), 0, "a correct case was reported");
    assert_eq!(
        planted("code", b"[01]", "\"refusal\":\"json_utf8\""),
        1,
        "a wrong code"
    );
    assert_eq!(
        planted("acc", b"[1]", "\"refusal\":\"json_utf8\""),
        1,
        "accepted, wanted a refusal"
    );
    assert_eq!(planted("ref", b"[01]", ok), 1, "refused, wanted a board");
    assert_eq!(planted("can", b"[2]", ok), 1, "wrong canonical bytes");
    let hash = "\"canonical\":\"canon/a.out.json\",\"sha256\":\"00\"";
    assert_eq!(planted("sha", b"[1]", hash), 1, "a wrong hash");
}

/// A zip case on a copy of a real vector, with an index of the caller's.
fn planted_zip(tag: &str, vector: &str, case: &str) -> usize {
    let dir = std::env::temp_dir().join(format!("k5board-zip-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("zip")).expect("mkdir");
    let vectors = repo().join("vectors/zip");
    for name in [
        format!("{vector}.k5board"),
        format!("{vector}.out.json"),
        format!("{vector}.cleared.json"),
    ] {
        if vectors.join(&name).exists() {
            std::fs::copy(vectors.join(&name), dir.join("zip").join(&name)).expect("copy");
        }
    }
    let index = format!(
        "{{\"cases\":[{{\"canonical\":\"zip/{vector}.out.json\",\"input\":\"zip/{vector}.k5board\",\"layer\":\"zip\",\"name\":\"a\",{case}}}]}}"
    );
    std::fs::write(dir.join("index.json"), index).expect("write");
    let report = conformance::run(&dir);
    std::fs::remove_dir_all(&dir).ok();
    report.failed.len()
}

#[test]
fn the_runner_reports_a_wrong_guard_or_a_guard_that_moves() {
    assert_eq!(
        planted_zip("ok", "minimal", "\"stable_guard\":true"),
        0,
        "a correct case"
    );
    assert_eq!(
        planted_zip("guard", "minimal", "\"guard\":\"00\""),
        1,
        "a wrong guard"
    );
    assert_eq!(
        planted_zip("miss", "minimal", "\"missing\":[\"00\"]"),
        1,
        "a missing blob this file doesn't have"
    );
    let wrong = "\"cleared\":\"zip/dangling.cleared.json\",\"missing\":[\"00\"]";
    assert_eq!(
        planted_zip("miss2", "dangling", wrong),
        1,
        "the wrong blob named as missing"
    );
    assert_eq!(
        planted_zip("moves", "foreign", "\"stable_guard\":true"),
        1,
        "a guard that moves: this file isn't canonical, so a rewrite gives it a new one"
    );
}

#[test]
fn the_central_directory_size_is_capped() {
    use support::forge::{Forge, E, STORE};
    let mut entries = Vec::new();
    for i in 0..1100 {
        let mut e = E::new(&format!("e{i}"), b"", STORE);
        e.extra_central = vec![9, 0, 0xFB, 0xFF];
        e.extra_central.resize(65_535, 0);
        entries.push(e);
    }
    let file = Forge::new(entries).build();
    let r = k2_mpc_board::Package::open(std::io::Cursor::new(file), &k2_mpc_board::Limits::V1)
        .err()
        .expect("refused");
    assert_eq!(r.code, RefusalCode::LimitEntries);
}

#[test]
fn schema_file_matches_the_tables() {
    let on_disk = std::fs::read_to_string(repo().join("schema/board-schema.json")).expect("read");
    assert!(
        on_disk == schema_json(),
        "schema/board-schema.json differs from schema_json(): regenerate"
    );
}

#[test]
fn spec_tables_match_the_tables() {
    let doc = std::fs::read_to_string(repo().join("docs/FORMAT.md")).expect("read");
    let tables = schema::format_tables();
    assert!(
        doc.contains(&tables),
        "docs/FORMAT.md's generated tables differ: regenerate"
    );
}

#[test]
fn schema_lists_every_refusal_and_type() {
    let text = schema_json();
    for c in RefusalCode::ALL {
        assert!(
            text.contains(&format!("\"{}\"", c.as_str())),
            "{} isn't in the schema",
            c.as_str()
        );
    }
    for (t, _) in schema::TYPES {
        assert!(
            text.contains(&format!("\"{t}\":[")),
            "{t} isn't in the schema"
        );
    }
}

#[test]
fn vector_names_stay_short() {
    // The tree has to check out inside a host's own tree on Windows.
    for (path, _) in support::cases::files() {
        assert!(path.len() <= 60, "{path} is over 60 bytes");
    }
}
