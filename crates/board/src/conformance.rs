//! Runs the conformance vectors in `vectors/` against this crate. Any reader can run the same
//! files; `vectors/index.json` describes them.

use crate::hash::Sha256;
use crate::json::{self, Value};
use crate::limits::Limits;
use crate::model::Board;
use crate::ops::{apply_group, Group, Op};
use crate::package::{write, Package};
use crate::refusal::{Refusal, RefusalCode};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

/// The vectors' folder in this repo, wherever the repo is vendored.
pub const VECTORS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors");

/// What a run found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Cases that met their expectation.
    pub passed: usize,
    /// Cases that didn't, by name, with what happened.
    pub failed: Vec<(String, String)>,
    /// Refusal codes the vectors exercised.
    pub codes_seen: BTreeSet<String>,
}

fn text<'v>(case: &'v json::Fields, key: &str) -> Option<&'v str> {
    match case.get(key) {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}

fn limits_of(case: &json::Fields) -> Limits {
    let mut l = Limits::V1;
    if let Some(Value::Obj(over)) = case.get("limits") {
        for (k, v) in over {
            let Value::Num(n) = v else { continue };
            let n = *n as u64;
            match k.as_str() {
                "entries" => l.entries = n,
                "manifest_bytes" => l.manifest_bytes = n,
                "board_json_bytes" => l.board_json_bytes = n,
                "part_bytes" => l.part_bytes = n,
                "total_bytes" => l.total_bytes = n,
                "ratio" => l.ratio = n,
                "ratio_floor" => l.ratio_floor = n,
                "objects" => l.objects = n,
                "people" => l.people = n,
                "depth" => l.depth = n,
                "values" => l.values = n,
                _ => {}
            }
        }
    }
    l
}

/// A case's outcome: the canonical bytes and any hashes, or a refusal.
struct Outcome {
    canonical: Vec<u8>,
    guard: Option<Sha256>,
    missing: Vec<Sha256>,
}

fn run_canon(input: &[u8], l: &Limits) -> Result<Outcome, Refusal> {
    let v = json::parse(input, l)?;
    Ok(Outcome {
        canonical: json::to_canonical(&v)?,
        guard: None,
        missing: Vec::new(),
    })
}

fn run_json(input: &[u8], l: &Limits) -> Result<Outcome, Refusal> {
    let b = Board::from_json(input, l)?;
    Ok(Outcome {
        canonical: b.to_canonical_json()?,
        guard: None,
        missing: Vec::new(),
    })
}

fn run_zip(input: &[u8], l: &Limits) -> Result<Outcome, Refusal> {
    let mut p = Package::open(Cursor::new(input.to_vec()), l)?;
    p.verify_parts()?;
    Ok(Outcome {
        canonical: p.board().to_canonical_json()?,
        guard: Some(p.package_guard()),
        missing: p.missing_blobs(),
    })
}

fn check_revs(input: &[u8], case: &json::Fields, l: &Limits) -> Result<(), String> {
    let board = Board::from_json(input, l).map_err(|r| r.to_string())?;
    let Some(Value::Obj(revs)) = case.get("revs") else {
        return Err("no revs".to_string());
    };
    if revs.len() != board.objects.len() {
        return Err("revs doesn't list every object".to_string());
    }
    for o in &board.objects {
        let got = o.rev().map_err(|r| r.to_string())?.to_hex();
        match revs.get(o.id.as_str()) {
            Some(Value::Str(want)) if *want == got => {}
            want => return Err(format!("rev of {}: wanted {want:?}, got {got}", o.id)),
        }
    }
    let got = board.settings_rev().map_err(|r| r.to_string())?.to_hex();
    if text(case, "settings_rev") != Some(got.as_str()) {
        return Err(format!("settings_rev: got {got}"));
    }
    Ok(())
}

/// The ids a refusal names, against a case's expected list.
fn check_bad_ids(r: &Refusal, case: &json::Fields, dir: &Path) -> Result<(), String> {
    let Some(name) = text(case, "bad_ids") else {
        return Ok(());
    };
    let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    let want = json::parse(&bytes, &Limits::V1).map_err(|e| format!("{name}: {e}"))?;
    let got = Value::Obj(
        [
            (
                "ids".to_string(),
                Value::Arr(r.bad_ids.iter().cloned().map(Value::Str).collect()),
            ),
            ("total".to_string(), Value::Num(r.bad_ids_total as f64)),
            ("truncated".to_string(), Value::Bool(r.bad_ids_truncated)),
        ]
        .into_iter()
        .collect(),
    );
    if got == want {
        Ok(())
    } else {
        let got = json::to_canonical(&got).unwrap_or_default();
        let got = String::from_utf8_lossy(&got);
        Err(format!(
            "bad ids: got {}",
            got.chars().take(300).collect::<String>()
        ))
    }
}

/// A conversion pair: this crate has no converter, so it checks that the file side is a valid board,
/// and that the pair says what doesn't survive. The other side is for whoever converts.
fn check_widget_pair(input: &[u8], l: &Limits) -> Result<(), String> {
    let Ok(Value::Obj(pair)) = json::parse(input, l) else {
        return Err("not a JSON object".to_string());
    };
    if !matches!(pair.get("lossy"), Some(Value::Str(s)) if !s.is_empty()) {
        return Err("no lossy statement".to_string());
    }
    if !matches!(pair.get("lossy_direction"), Some(Value::Str(s))
        if ["both", "widget_to_file", "file_to_widget"].contains(&s.as_str()))
    {
        return Err("no lossy_direction: both, widget_to_file or file_to_widget".to_string());
    }
    if let Some(code) = pair.get("file_refused") {
        return check_refused_pair(&pair, code, l);
    }
    let file = pair.get("file").ok_or("no file side")?;
    if !pair.contains_key("widget") {
        return Err("no widget side".to_string());
    }
    let bytes = json::to_canonical(file).map_err(|r| r.to_string())?;
    Board::from_json(&bytes, l).map_err(|r| format!("the file side: {r}"))?;
    Ok(())
}

/// A value the widget's store takes and this format refuses: the file side has to be refused, with
/// exactly this code.
fn check_refused_pair(pair: &json::Fields, code: &Value, l: &Limits) -> Result<(), String> {
    let Value::Str(code) = code else {
        return Err("file_refused must be a code".to_string());
    };
    if pair.contains_key("file") || pair.contains_key("widget") {
        return Err("a refused pair has file_text and widget_text only".to_string());
    }
    if !matches!(pair.get("widget_text"), Some(Value::Str(_))) {
        return Err("no widget_text".to_string());
    }
    let Some(Value::Str(file)) = pair.get("file_text") else {
        return Err("no file_text".to_string());
    };
    match Board::from_json(file.as_bytes(), l) {
        Ok(_) => Err(format!("wanted refusal {code}, but it was accepted")),
        Err(r) if r.code.as_str() == code => Ok(()),
        Err(r) => Err(format!("wanted refusal {code}, got {r}")),
    }
}

/// A file with a dangling reference opens and reports it; a write refuses it; the clearing op then
/// writes, and the file has nothing missing.
fn check_missing(
    input: &[u8],
    case: &json::Fields,
    dir: &Path,
    l: &Limits,
    report: &mut Report,
) -> Result<(), String> {
    let mut p = Package::open(Cursor::new(input.to_vec()), l).map_err(|r| r.to_string())?;
    let meta = p.manifest().write_meta();
    let mut board = p.board().clone();
    let missing = p.missing_blobs();
    let mut out = Vec::new();
    match write(&mut out, &meta, &board, &mut p, l) {
        Err(r) if r.code == RefusalCode::BlobMissing => {
            report.codes_seen.insert(r.code.as_str().to_string());
        }
        Err(r) => return Err(format!("wanted blob_missing from write, got {r}")),
        Ok(_) => return Err("write kept a dangling reference".to_string()),
    }
    let group = Group {
        id: "clear".to_string(),
        by: None,
        ops: missing.iter().map(|h| Op::ClearBlob { hash: *h }).collect(),
    };
    apply_group(&mut board, &group).map_err(|c| format!("the clearing op: {c:?}"))?;
    let want_name = text(case, "cleared").ok_or("no cleared")?;
    let want = std::fs::read(dir.join(want_name)).map_err(|e| format!("{want_name}: {e}"))?;
    let got = board.to_canonical_json().map_err(|r| r.to_string())?;
    if got != want {
        return Err(format!(
            "cleared board: got {}",
            String::from_utf8_lossy(&got)
        ));
    }
    let mut out = Vec::new();
    write(&mut out, &meta, &board, &mut p, l).map_err(|r| format!("after clearing: {r}"))?;
    let again = Package::open(Cursor::new(out), l).map_err(|r| format!("reopen: {r}"))?;
    if !again.missing_blobs().is_empty() {
        return Err("still missing after clearing".to_string());
    }
    Ok(())
}

fn check_case(case: &json::Fields, dir: &Path, report: &mut Report) -> Result<(), String> {
    let layer = text(case, "layer").ok_or("no layer")?;
    let input_name = text(case, "input").ok_or("no input")?;
    let input = std::fs::read(dir.join(input_name)).map_err(|e| format!("{input_name}: {e}"))?;
    let l = limits_of(case);
    if layer == "rev" {
        return check_revs(&input, case, &l);
    }
    if layer == "widget" {
        return check_widget_pair(&input, &l);
    }
    let got = match layer {
        "canon" => run_canon(&input, &l),
        "json" | "fidelity" => run_json(&input, &l),
        "zip" => run_zip(&input, &l),
        other => return Err(format!("unknown layer {other}")),
    };
    match (got, text(case, "refusal")) {
        (Err(r), Some(want)) => {
            report.codes_seen.insert(r.code.as_str().to_string());
            if r.code.as_str() == want {
                check_bad_ids(&r, case, dir)
            } else {
                Err(format!("wanted refusal {want}, got {r}"))
            }
        }
        (Err(r), None) => Err(format!("wanted a board, got {r}")),
        (Ok(_), Some(want)) => Err(format!("wanted refusal {want}, but it was accepted")),
        (Ok(o), None) => {
            let want_name = text(case, "canonical").ok_or("no canonical")?;
            let want =
                std::fs::read(dir.join(want_name)).map_err(|e| format!("{want_name}: {e}"))?;
            if o.canonical != want {
                return Err(format!(
                    "canonical bytes differ: got {}",
                    String::from_utf8_lossy(&o.canonical)
                ));
            }
            if let Some(hash) = text(case, "sha256") {
                let got = Sha256::of(&o.canonical).to_hex();
                if got != hash {
                    return Err(format!("sha256 wanted {hash}, got {got}"));
                }
            }
            if let Some(guard) = text(case, "guard") {
                let got = o.guard.map(|g| g.to_hex()).unwrap_or_default();
                if got != guard {
                    return Err(format!("guard wanted {guard}, got {got}"));
                }
            }
            let want_missing: Vec<&str> = match case.get("missing") {
                Some(Value::Arr(a)) => a
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let got_missing: Vec<String> = o.missing.iter().map(Sha256::to_hex).collect();
            if got_missing != want_missing {
                return Err(format!(
                    "missing blobs: wanted {want_missing:?}, got {got_missing:?}"
                ));
            }
            if !want_missing.is_empty() {
                return check_missing(&input, case, dir, &l, report);
            }
            if layer == "zip" {
                round_trip(&input, &l, case)?;
            }
            Ok(())
        }
    }
}

/// An accepted file survives open and write: same board, every part, and the guard holds when the
/// file was canonical to start with.
fn round_trip(input: &[u8], l: &Limits, case: &json::Fields) -> Result<(), String> {
    let mut p = Package::open(Cursor::new(input.to_vec()), l).map_err(|r| r.to_string())?;
    let meta = p.manifest().write_meta();
    let board = p.board().clone();
    let guard = p.package_guard();
    let before: Vec<_> = crate::parts::PartSource::list(&p);
    let mut out = Vec::new();
    let w = write(&mut out, &meta, &board, &mut p, l).map_err(|r| format!("rewrite: {r}"))?;
    if matches!(case.get("stable_guard"), Some(Value::Bool(true))) && w.package_guard != guard {
        return Err("the guard moved on an unchanged open and write".to_string());
    }
    let mut again = Package::open(Cursor::new(out), l).map_err(|r| format!("reopen: {r}"))?;
    again.verify_parts().map_err(|r| format!("reopen: {r}"))?;
    if again.board() != &board {
        return Err("the board changed on rewrite".to_string());
    }
    let (m, n) = (&meta, again.manifest());
    if m.lineage != n.lineage || m.base != n.base || m.extra != n.extra {
        return Err("the manifest changed on rewrite".to_string());
    }
    if n.version != m.version || n.min_reader != m.min_reader {
        return Err(format!(
            "a rewrite moved the versions: {} and {} became {} and {}",
            m.version, m.min_reader, n.version, n.min_reader
        ));
    }
    if crate::parts::PartSource::list(&again) != before {
        return Err("a part changed on rewrite".to_string());
    }
    Ok(())
}

/// Runs every case in `dir`.
pub fn run(dir: &Path) -> Report {
    let mut report = Report::default();
    let index = match std::fs::read(dir.join("index.json")) {
        Ok(b) => b,
        Err(e) => {
            report.failed.push(("index.json".into(), e.to_string()));
            return report;
        }
    };
    let cases = match json::parse(&index, &Limits::V1) {
        Ok(Value::Obj(f)) => match f.get("cases") {
            Some(Value::Arr(a)) => a.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    if cases.is_empty() {
        report.failed.push(("index.json".into(), "no cases".into()));
    }
    for c in &cases {
        let Value::Obj(case) = c else { continue };
        let name = text(case, "name").unwrap_or("?").to_string();
        // A reader that stops without an answer fails that case by name, and the rest still run.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_case(case, dir, &mut report)
        }))
        .unwrap_or_else(|_| Err("the reader stopped without a refusal".to_string()));
        match outcome {
            Ok(()) => report.passed += 1,
            Err(why) => report.failed.push((name, why)),
        }
    }
    report
}

/// Refusal codes that no file can trigger: they come only from the API.
pub const API_ONLY_CODES: &[RefusalCode] = &[
    RefusalCode::LimitRaised,
    RefusalCode::IoError,
    RefusalCode::ObjectLocked,
];
