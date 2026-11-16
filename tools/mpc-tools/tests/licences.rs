//! `licences` against the old copies, from tests/data/licences (gen_licences_table.py).
//!
//! The allow-list now ruled is the `licence-gate.py` copies' (`pack-k` and `pack-l`), so every
//! expression row must judge as the `pack-k` copy did: its first verdict for a shipped package,
//! its second for one in a fuzz lock (`died`, an expression the old parser died on, refuses). The
//! other copies' verdicts stay in the table as the record of where they disagreed.

mod common;
use common::*;

const EXPRESSIONS: &str = include_str!("data/licences/expressions.tsv");
const METADATA: &str = include_str!("data/licences/metadata.tsv");
const COPIES: usize = 13;
const RULED: usize = 11;

fn verdict(e: &str, fuzz: bool) -> (i32, String) {
    let arg = format!("x={e}");
    let mut args = vec!["licences"];
    if fuzz {
        args.push("--fuzz");
    }
    args.push("--expr");
    args.push(arg.as_str());
    let out = run(&args, b"", &[], &data("licences"));
    (out.code, text(&out.stdout))
}

#[test]
fn every_expression_judges_as_the_ruled_copy() {
    let rows = rows(EXPRESSIONS);
    let mut wrong = Vec::new();
    for r in &rows {
        assert_eq!(r.len(), 1 + COPIES, "a row with {} fields: {r:?}", r.len());
        let e = unescape(&r[0]);
        let (ship, fuzz) = r[1 + RULED].split_once(',').expect("SHIPPED,FUZZ");
        let mut differs = Vec::new();
        for (fz, want) in [(false, ship), (true, fuzz)] {
            let want_ok = want == "ok";
            let (code, out) = verdict(&e, fz);
            let got_ok = code == 0;
            if got_ok != want_ok || !matches!(code, 0 | 1) {
                differs.push(format!(
                    "fuzz={fz}: want {want}, got exit {code} {:?}",
                    out.lines().next().unwrap_or("")
                ));
            }
        }
        if !differs.is_empty() {
            wrong.push(format!("{e:?}: {}", differs.join("; ")));
        }
    }
    report("expressions.tsv", rows.len(), 700, wrong);
}

#[test]
fn every_metadata_fixture_flags_as_the_old_kit_check_and_the_ruling() {
    let rows = rows(METADATA);
    let mut wrong = Vec::new();
    for r in &rows {
        let (name, old_code, flagged, ruled) = (&r[0], &r[1], &r[2], &r[3]);
        let mut want: Vec<String> = if flagged == "-" {
            Vec::new()
        } else {
            flagged.split(" | ").map(str::to_string).collect()
        };
        if let Some(extra) = ruled.strip_prefix('+') {
            want.push(extra.to_string());
        }
        want.sort();
        let fixture = format!("metadata/{name}.json");
        let out = run(
            &["--json", "licences", "--metadata", fixture.as_str()],
            b"",
            &[],
            &data("licences"),
        );
        let doc: serde_json::Value = match serde_json::from_slice(&out.stdout) {
            Ok(d) => d,
            Err(e) => {
                wrong.push(format!(
                    "{name}: no JSON on stdout ({e}); exit {}",
                    out.code
                ));
                continue;
            }
        };
        let mut got: Vec<String> = doc["locks"][0]["refused"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|p| {
                        format!(
                            "{} {}: {}",
                            p["name"].as_str().unwrap_or(""),
                            p["version"].as_str().unwrap_or(""),
                            p["licence"].as_str().unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        got.sort();
        let want_code = if want.is_empty() { 0 } else { 1 };
        if got != want || out.code != want_code {
            wrong.push(format!(
                "{name}: old exit {old_code}, ruled {ruled}: want {want:?} exit {want_code}, got {got:?} exit {}",
                out.code
            ));
        }
    }
    report("metadata.tsv", rows.len(), 10, wrong);
}

#[test]
fn expr_mode_prints_as_before() {
    let out = run(
        &["licences", "--expr", "a=MIT", "b=Unlicense", "c="],
        b"",
        &[],
        &data("licences"),
    );
    assert_eq!(out.code, 1, "Unlicense alone and no licence refuse");
    assert_eq!(
        text(&out.stdout),
        "Licences outside the allow-list:\nb: Unlicense\nc: NONE\n"
    );
    let out = run(
        &[
            "licences",
            "--expr",
            "a=MIT OR Apache-2.0",
            "b=Unlicense OR MIT",
        ],
        b"",
        &[],
        &data("licences"),
    );
    assert_eq!(out.code, 0);
    assert_eq!(
        text(&out.stdout),
        "licences ok (2: a (MIT OR Apache-2.0), b (Unlicense OR MIT))\n"
    );
}

#[test]
fn ncsa_only_in_a_fuzz_lock() {
    let fixture = "metadata/ncsa-in-main.json";
    let main = run(
        &["licences", "--metadata", fixture],
        b"",
        &[],
        &data("licences"),
    );
    assert_eq!(
        main.code,
        1,
        "NCSA outside a fuzz lock refuses: {}",
        text(&main.stdout)
    );
    let fuzz = run(
        &["licences", "--fuzz", "--metadata", fixture],
        b"",
        &[],
        &data("licences"),
    );
    assert_eq!(
        fuzz.code,
        0,
        "NCSA in a fuzz lock passes: {}",
        text(&fuzz.stdout)
    );
}
