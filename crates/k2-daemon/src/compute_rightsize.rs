//! `k2 compute rightsize` and `--mem auto[:G]` (prd-compute-blacksmith-class-v1
//! §4.4.2–§4.4.3, slice R1). Daemon only, no node update; behind the CI
//! preview (`ciPreview`) like the rest of the slice.
//!
//! The numbers come from `k2_core::compute::rightsize` (this workspace's
//! ended runs of the same command, tag or CI check). `--mem auto` resolves
//! at submit, so the plan, its digest, the receipt and the node only ever
//! see a plain number; `effective.mem` says where it came from.

use std::collections::HashMap;

use k2_core::compute::rightsize::{self as rs, Key};
use serde_json::{json, Value};

use crate::compute_routes::{ci_gate, n, need_credential, no, passport_gate, target_ws, Who, R};

/// `GET /cli/compute/rightsize`.
pub const PATH: &str = "/cli/compute/rightsize";

/// `mem_auto_no_history` (exit 2).
pub use k2_core::compute::proto::gens::codes::MEM_AUTO_NO_HISTORY;

/// Default and largest `--days`.
pub const DEFAULT_DAYS: i64 = 14;
pub const MAX_DAYS: i64 = 90;

const BAD: &str = k2_core::compute::codes::BAD_REQUEST;

/// `--mem auto` in a run body: `memAuto: true`, with the fallback
/// (`--mem auto:8G`, a preset's `mem = "auto:8G"`, `ci.toml`'s `mem_gb`) as
/// `memAutoFallbackBytes`. `Ok(None)`: not asked. `Ok(Some(fallback))`.
pub(crate) fn mem_auto_from(v: &Value) -> R<Option<Option<u64>>> {
    match v.get("memAuto") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {
            if v.get("memAutoFallbackBytes").is_some_and(|x| !x.is_null()) {
                return Err(no(BAD, 400, "memAutoFallbackBytes goes with memAuto: --mem auto:8G"));
            }
            Ok(None)
        }
        Some(Value::Bool(true)) => {
            ci_gate("--mem auto")?;
            if v.get("memBytes").is_some_and(|x| !x.is_null()) {
                return Err(no(BAD, 400, "--mem is either a size (--mem 8G) or auto (--mem auto, --mem auto:8G), not both"));
            }
            let fb = match n(v, "memAutoFallbackBytes") {
                None => None,
                Some(b) if b > 0 => Some(b as u64),
                Some(_) => return Err(no(BAD, 400, "--mem auto:<size>: the fallback must be positive (--mem auto:8G)")),
            };
            Ok(Some(fb))
        }
        Some(_) => Err(no(BAD, 400, "memAuto is true or absent (--mem auto[:8G])")),
    }
}

/// The history a run body's `--mem auto` reads: a CI check's (`tag`
/// `ci:<check>`), else the exact command.
pub(crate) fn auto_key(v: &Value, argv: &[String]) -> Key {
    match v.get("tag").and_then(Value::as_str).filter(|t| t.starts_with("ci:") && t.len() > 3) {
        Some(t) => Key::Tag(t.to_string()),
        None => Key::Argv(argv.to_vec()),
    }
}

/// Resolve `--mem auto` for workspace `ws_id`: the number and where it came
/// from. `mem_auto_no_history` (exit 2) when there is neither history nor
/// a fallback.
pub(crate) fn resolve_auto(ws_id: &str, key: &Key, fallback: Option<u64>) -> R<(u64, String)> {
    let runs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        rs::runs(&conn, ws_id, key, 0, rs::AUTO_RUNS)
    };
    rs::auto_mem(&runs, fallback).ok_or_else(|| {
        no(
            MEM_AUTO_NO_HISTORY,
            400,
            "--mem auto: this command has no finished run to learn from yet. Give a fallback for the first runs: --mem auto:8G (a preset: mem = \"auto:8G\"; .k2/ci.toml: mem_gb = 8 with mem_auto = true)",
        )
    })
}

/// `GET /cli/compute/rightsize?workspace=&check=|tag=|argv=<json>&days=`:
/// what one command used and what to ask for (§4.4.2). A reader verb for
/// the caller's own workspace.
pub(crate) fn handle(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    need_credential(who)?;
    passport_gate(who)?;
    ci_gate("k2 compute rightsize")?;
    let ws = target_ws(who, params.get("workspace").map(String::as_str))?;
    let get = |k: &str| params.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    let picked = [get("check").is_some(), get("tag").is_some(), get("argv").is_some()].iter().filter(|x| **x).count();
    if picked != 1 {
        return Err(no(BAD, 400, "name one command: --check NAME (a .k2/ci.toml check), --tag NAME, or -- <command…> (the exact argv you run)"));
    }
    let key = if let Some(c) = get("check") {
        if !k2_core::compute::proto::ci::is_name(c) {
            return Err(no(BAD, 400, format!("--check {c:?}: a check name ([a-z0-9._-])")));
        }
        Key::check(c)
    } else if let Some(t) = get("tag") {
        if !crate::compute_routes::valid_tag(t) {
            return Err(no(BAD, 400, "--tag is 1–64 of A-Z a-z 0-9 . _ : / -"));
        }
        Key::Tag(t.to_string())
    } else {
        let a: Vec<String> = get("argv")
            .and_then(|t| serde_json::from_str(t).ok())
            .filter(|a: &Vec<String>| !a.is_empty() && a.len() <= 256)
            .ok_or_else(|| no(BAD, 400, "argv is the command as a JSON list of strings (the CLI sends what follows --)"))?;
        Key::Argv(a)
    };
    let days = match get("days") {
        None => DEFAULT_DAYS,
        Some(d) => d.trim_end_matches('d').parse::<i64>().ok().filter(|d| (1..=MAX_DAYS).contains(d)).ok_or_else(|| no(BAD, 400, format!("--days: 1-{MAX_DAYS}")))?,
    };
    let since = k2_core::compute::now() - days * 86_400;
    let runs = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        rs::runs(&conn, &ws.id, &key, since, rs::MAX_RUNS)
    };
    let mem = rs::mem_facts(&runs);
    let cpus = rs::cpu_facts(&runs);
    let auto = rs::auto_mem(&runs.iter().take(rs::AUTO_RUNS as usize).cloned().collect::<Vec<_>>(), None);
    Ok(json!({
        "workspace": ws.handle,
        "key": key.json(),
        "days": days,
        "runs": runs.len(),
        "finished": runs.iter().filter(|r| r.is_done()).count(),
        "confidence": rs::confidence(mem.samples),
        "mem": {
            "samples": mem.samples,
            "p50Bytes": mem.p50_bytes,
            "p95Bytes": mem.p95_bytes,
            "maxBytes": mem.max_bytes,
            "memCapEnds": mem.mem_cap_ends,
            "lastMemCapPeakBytes": mem.last_mem_cap_peak_bytes,
            "floorBytes": mem.floor_bytes,
            "askedBytes": mem.asked_bytes,
            "suggestBytes": mem.suggest_bytes,
            "suggest": mem.suggest_bytes.map(|b| format!("--mem {}", rs::gib_words(b))),
            "auto": auto.map(|(b, from)| json!({"bytes": b, "value": rs::gib_words(b), "from": from})),
        },
        "cpus": cpus,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_reads_a_check_by_its_tag_else_the_command() {
        let argv = vec!["cargo".to_string(), "test".to_string()];
        assert_eq!(auto_key(&json!({"tag": "ci:unit"}), &argv), Key::Tag("ci:unit".into()));
        assert_eq!(auto_key(&json!({"tag": "fuzz:p3"}), &argv), Key::Argv(argv.clone()));
        assert_eq!(auto_key(&json!({}), &argv), Key::Argv(argv.clone()));
    }

    #[test]
    fn mem_auto_body_shapes() {
        let _env = k2_core::test_env::lock();
        let _c = k2_core::test_env::EnvVar::set("K2_COMPUTE", "1");
        let _ci = k2_core::test_env::EnvVar::set(k2_core::compute::CI_ENV, "1");
        assert_eq!(mem_auto_from(&json!({})).unwrap(), None);
        assert_eq!(mem_auto_from(&json!({"memAuto": true})).unwrap(), Some(None));
        assert_eq!(mem_auto_from(&json!({"memAuto": true, "memAutoFallbackBytes": 8u64 << 30})).unwrap(), Some(Some(8 << 30)));
        assert_eq!(mem_auto_from(&json!({"memAuto": true, "memBytes": 1})).unwrap_err().code, BAD);
        assert_eq!(mem_auto_from(&json!({"memAutoFallbackBytes": 1})).unwrap_err().code, BAD);
        assert_eq!(mem_auto_from(&json!({"memAuto": true, "memAutoFallbackBytes": 0})).unwrap_err().code, BAD);
        // Off with the CI preview off.
        let _off = k2_core::test_env::EnvVar::set(k2_core::compute::CI_ENV, "off");
        assert_eq!(mem_auto_from(&json!({"memAuto": true})).unwrap_err().code, k2_core::compute::CI_PREVIEW_OFF);
    }
}
