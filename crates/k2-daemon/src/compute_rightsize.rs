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
use k2_core::cli_name;

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

/// `--cpus N+M` in a run body (slice R2, §4.4.4, BK-13): `cpus: N` and a
/// separate `cpusBurst: M`, so an older daemon keeps N and ignores M.
/// `Ok(None)`: not asked. `Ok(Some(millis))`: M in thousandths of a core,
/// above N (which it needs), at most 1024 CPUs.
pub(crate) fn cpus_burst_from(v: &Value, cpu_millis: Option<u64>) -> R<Option<u64>> {
    let Some(x) = v.get("cpusBurst").filter(|x| !x.is_null()) else { return Ok(None) };
    ci_gate("--cpus N+M")?;
    let b = x
        .as_f64()
        .filter(|b| *b > 0.0 && *b <= 1024.0)
        .ok_or_else(|| no(BAD, 400, "--cpus N+M: M is the most CPUs it may use, a number up to 1024 (--cpus 4+8)"))?;
    let b = (b * 1000.0) as u64;
    k2_core::compute::proto::gens::check_cpus_burst(cpu_millis, Some(b)).map_err(|_| {
        no(
            BAD,
            400,
            match cpu_millis {
                None => "--cpus N+M needs N, the CPUs it is guaranteed (--cpus 4+8: 4 guaranteed, up to 8 while cores are free)",
                Some(_) => "--cpus N+M: M, the most it may use, must be above N, its guarantee (--cpus 4+8)",
            },
        )
    })?;
    Ok(Some(b))
}

/// `N+M` for `effective.cpus` and notes (whole numbers without a dot).
pub(crate) fn cpus_words(cpu_millis: u64, burst_millis: u64) -> String {
    let w = |m: u64| if m % 1000 == 0 { (m / 1000).to_string() } else { format!("{}", m as f64 / 1000.0) };
    format!("{}+{}", w(cpu_millis), w(burst_millis))
}

/// The run's note when its node won't use idle cores (an older k2-node, or
/// its owner turned `cpu_burst` off): it runs held to N there.
pub(crate) fn burst_note(node: &str, features: &[String], cpu_millis: u64, burst_millis: u64) -> Option<String> {
    if features.iter().any(|f| f == k2_core::compute::proto::gens::FEATURE_CPU_BURST) {
        return None;
    }
    Some(format!(
        "--cpus {}: {node} doesn't take idle cores now (its k2-node is older than protocol 10, or its owner set cpu_burst = false): it runs held to {} there",
        cpus_words(cpu_millis, burst_millis),
        cpus_words(cpu_millis, cpu_millis).split('+').next().unwrap_or_default()
    ))
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
    ci_gate(concat!(cli_name!(), " compute rightsize"))?;
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
    let mut out = json!({
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
    });
    // The lines `k2 compute rightsize` prints (the CLI only prints them).
    out["text"] = json!(text(&out));
    Ok(out)
}

/// `k2 compute rightsize`'s plain answer, from the route's JSON.
pub(crate) fn text(v: &Value) -> Vec<String> {
    let g = |b: &Value| match b.as_f64() {
        None => "?".to_string(),
        Some(b) => {
            let x = b / (1u64 << 30) as f64;
            if (x - x.round()).abs() < 0.05 {
                format!("{}G", x.round() as i64)
            } else {
                format!("{x:.1}G")
            }
        }
    };
    let s = |x: &Value| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string());
    let (k, m, c) = (&v["key"], &v["mem"], &v["cpus"]);
    let what = match k["kind"].as_str() {
        Some("argv") => format!("command: {}", k["value"].as_array().map(|a| a.iter().map(s).collect::<Vec<_>>().join(" ")).unwrap_or_default()),
        Some("check") => format!("CI check {}", s(&k["value"])),
        _ => format!("tag {}", s(&k["value"])),
    };
    let what: String = what.chars().take(160).collect();
    let n = |x: &Value| x.as_u64().unwrap_or(0);
    let mut out = vec![format!(
        "rightsize · {} · {what} · last {} days · {} runs ({} finished) · confidence {}",
        s(&v["workspace"]),
        n(&v["days"]),
        n(&v["runs"]),
        n(&v["finished"]),
        s(&v["confidence"])
    )];
    if n(&v["runs"]) == 0 {
        out.push("  no finished runs of it in that window: run it once with a generous --mem, then ask again;".into());
        out.push("  or submit with --mem auto:<size> (that size until it has history)".into());
        return out;
    }
    let mut line = if n(&m["samples"]) > 0 {
        format!("memory: peak p50 {} · p95 {} · max {}", g(&m["p50Bytes"]), g(&m["p95Bytes"]), g(&m["maxBytes"]))
    } else {
        "memory: no peak reported (an older k2-node, or no finished run)".to_string()
    };
    if n(&m["askedBytes"]) > 0 {
        line += &format!(" · asked {}", g(&m["askedBytes"]));
    }
    if n(&m["memCapEnds"]) > 0 {
        line += &format!(" · {} stopped at their limit (mem_cap; last peak {})", n(&m["memCapEnds"]), g(&m["lastMemCapPeakBytes"]));
    }
    out.push(line);
    if let Some(sug) = m["suggest"].as_str() {
        let floor = if n(&m["floorBytes"]) == 0 { "" } else { "; never below 1.5 × the last mem_cap peak" };
        out.push(format!("  suggest: {sug}   (p95 × 1.25, rounded up{floor})"));
        if m["auto"].is_object() {
            out.push(format!("  or: --mem auto   (it would ask {} now: {})", s(&m["auto"]["value"]), s(&m["auto"]["from"])));
        }
    }
    if n(&c["samples"]) > 0 {
        let avg = c["avgUsed"].as_f64().unwrap_or(0.0);
        let mut line = format!("cpus: used {avg:.1} on average (p95 {:.1})", c["p95Used"].as_f64().unwrap_or(avg));
        line += &match c["asked"].as_f64() {
            Some(a) => format!(" of --cpus {a}"),
            None => " (no --cpus asked)".to_string(),
        };
        if n(&c["pinnedRuns"]) > 0 {
            line += &format!(" · held at its limit in {} of {} runs", n(&c["pinnedRuns"]), n(&c["samples"]));
        }
        out.push(line);
        if let Some(sug) = c["suggest"].as_f64() {
            out.push(format!("  suggest: --cpus {sug}   ({})", c["why"].as_str().unwrap_or_default()));
        }
    } else {
        out.push("cpus: no CPU time reported".into());
    }
    if v["confidence"] == "low" {
        out.push("  (fewer than 5 finished runs: a first guess; it firms up as runs finish)".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain answer the CLI prints (it was rendered in `cli/k2`
    /// before; the CLI now only prints these lines).
    #[test]
    fn rightsize_text_lines() {
        let g = 1u64 << 30;
        let v = json!({"workspace": "proj", "key": {"kind": "check", "value": "unit", "tag": "ci:unit"},
            "days": 30, "runs": 6, "finished": 5, "confidence": "medium",
            "mem": {"samples": 5, "p50Bytes": 9 * g, "p95Bytes": 10 * g, "maxBytes": 10 * g, "memCapEnds": 1, "lastMemCapPeakBytes": 16 * g,
                    "floorBytes": 24 * g, "askedBytes": 16 * g, "suggestBytes": 24 * g, "suggest": "--mem 24G",
                    "auto": {"bytes": 24 * g, "value": "24G", "from": "auto (p95 10G of 5 runs)"}},
            "cpus": {"samples": 5, "avgUsed": 3.9, "p95Used": 4.0, "asked": 4.0, "pinnedRuns": 5, "suggest": 8.0, "why": "speed: held"}});
        assert_eq!(
            text(&v),
            vec![
                "rightsize · proj · CI check unit · last 30 days · 6 runs (5 finished) · confidence medium",
                "memory: peak p50 9G · p95 10G · max 10G · asked 16G · 1 stopped at their limit (mem_cap; last peak 16G)",
                "  suggest: --mem 24G   (p95 × 1.25, rounded up; never below 1.5 × the last mem_cap peak)",
                "  or: --mem auto   (it would ask 24G now: auto (p95 10G of 5 runs))",
                "cpus: used 3.9 on average (p95 4.0) of --cpus 4 · held at its limit in 5 of 5 runs",
                "  suggest: --cpus 8   (speed: held)",
            ]
        );
        let argv = json!({"workspace": "proj", "key": {"kind": "argv", "value": ["cargo", "test"]}, "days": 14, "runs": 0, "finished": 0, "confidence": "low", "mem": {}, "cpus": {}});
        let t = text(&argv);
        assert_eq!(t[0], "rightsize · proj · command: cargo test · last 14 days · 0 runs (0 finished) · confidence low");
        assert!(t[1].starts_with("  no finished runs of it"), "{t:?}");
        let low = json!({"workspace": "p", "key": {"kind": "tag", "value": "fuzz"}, "days": 14, "runs": 2, "finished": 2, "confidence": "low",
            "mem": {"samples": 2, "p50Bytes": 1_610_612_736u64, "p95Bytes": g, "maxBytes": g, "memCapEnds": 0}, "cpus": {"samples": 0}});
        let t = text(&low);
        assert_eq!(t[1], "memory: peak p50 1.5G · p95 1G · max 1G");
        assert_eq!(t[2], "cpus: no CPU time reported");
        assert_eq!(t[3], "  (fewer than 5 finished runs: a first guess; it firms up as runs finish)");
    }

    /// R2 (§4.4.4, BK-13): `cpusBurst` is above `cpus`, which it needs;
    /// behind the CI preview like the rest of the arc.
    #[test]
    fn cpus_burst_body_rules() {
        let _env = k2_core::test_env::lock();
        let _c = k2_core::test_env::EnvVar::set("K2_COMPUTE", "1");
        let _ci = k2_core::test_env::EnvVar::set(k2_core::compute::CI_ENV, "1");
        assert_eq!(cpus_burst_from(&json!({"cpus": 4}), Some(4000)).unwrap(), None);
        assert_eq!(cpus_burst_from(&json!({"cpus": 4, "cpusBurst": 8}), Some(4000)).unwrap(), Some(8000));
        assert_eq!(cpus_burst_from(&json!({"cpus": 0.5, "cpusBurst": 1.5}), Some(500)).unwrap(), Some(1500));
        let e = cpus_burst_from(&json!({"cpusBurst": 8}), None).unwrap_err();
        assert!(e.code == BAD && e.hint.contains("needs N"), "{e:?}");
        let e = cpus_burst_from(&json!({"cpus": 8, "cpusBurst": 8}), Some(8000)).unwrap_err();
        assert!(e.hint.contains("must be above N"), "{e:?}");
        assert_eq!(cpus_burst_from(&json!({"cpus": 4, "cpusBurst": 2000}), Some(4000)).unwrap_err().code, BAD);
        assert_eq!(cpus_burst_from(&json!({"cpus": 4, "cpusBurst": "8"}), Some(4000)).unwrap_err().code, BAD);
        assert_eq!(cpus_words(4000, 8000), "4+8");
        assert_eq!(cpus_words(500, 1500), "0.5+1.5");
        let old = vec!["stall".to_string()];
        assert!(burst_note("rig7", &old, 4000, 8000).unwrap().contains("held to 4"));
        assert_eq!(burst_note("rig7", &[k2_core::compute::proto::gens::FEATURE_CPU_BURST.to_string()], 4000, 8000), None);
        let _off = k2_core::test_env::EnvVar::set(k2_core::compute::CI_ENV, "off");
        assert_eq!(cpus_burst_from(&json!({"cpus": 4, "cpusBurst": 8}), Some(4000)).unwrap_err().code, k2_core::compute::CI_PREVIEW_OFF);
        assert_eq!(cpus_burst_from(&json!({"cpus": 4}), Some(4000)).unwrap(), None, "a plain --cpus is untouched");
    }

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
