//! `k2 app status <app>`: one answer to "how is my App?"
//! (prd-tauri-core-0467-v1 TC-15).
//!
//! `GET /cli/app/status?app=[&noChecks=1][&public=1]` composes what the
//! other App routes already know, as the same caller (each part keeps its
//! own access rule): the deploy, health, the App's own `host.checks`
//! (TC-16), errors in the last hour, resources, the door (egress denials,
//! held approvals), hosts, phone (pairing, link files, push), the update
//! feed, and **next**: one exact command per red or yellow item.
//!
//! - `verdict`: `green` (CLI exit 0), `yellow` (2: a deploy in progress, a
//!   held approval, an upload pending), `red` (1: health failing, a check
//!   failed, crash looping, the App down). Not your App: the refusal (3).
//! - `schema: 1`. A part another route refuses for this caller shows
//!   `{unavailable: <code>}`; it never fails the whole answer.
//! - App output never appears raw (vs-live TA-20): checks say matched or
//!   not; deployment errors are K2's own words, control characters
//!   stripped, as in `k2 app errors`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use k2_core::deploy::{self, store};
use serde_json::{json, Value as J};

use super::routes::{app_for, db_err, need_credential, R};
use crate::compute_routes::Who;

pub const ROUTE: &str = "/cli/app/status";
pub const ROUTES: &[&str] = &[ROUTE];
/// The answer's shape version.
pub const SCHEMA: u32 = 1;
/// "Errors" and "door" look back this far.
pub const WINDOW_SECS: i64 = 3600;

fn flag(params: &HashMap<String, String>, k: &str) -> bool {
    params.get(k).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
}

/// A part's answer, or what refused it for this caller.
fn part(r: R<J>) -> J {
    match r {
        Ok(v) => v,
        Err(e) => json!({ "unavailable": e.code }),
    }
}

/// `GET /cli/app/status?app=`.
pub(crate) fn route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let run_checks = !flag(params, "noChecks");
    let public = flag(params, "public");
    let mut next: Vec<J> = Vec::new();
    let mut red = false;
    let mut yellow = false;

    let dep = deploy_part(&app)?;
    let last_result = dep.pointer("/last/result").and_then(J::as_str).unwrap_or("").to_string();
    let live = app.live_deployment.is_some() && super::supervisor::live_port(&app.app_id).is_some();
    match last_result.as_str() {
        store::RESULT_PENDING | store::RESULT_STARTING => {
            yellow = true;
            next.push(item("yellow", "deploy", "a deploy is in progress", format!("k2 app deployment {}", app.app_id)));
        }
        store::RESULT_FAILED | store::RESULT_STALLED | store::RESULT_ROLLED_BACK if !live => {
            red = true;
            next.push(item("red", "deploy", "the last deploy didn't go live", format!("k2 app errors {}", app.app_id)));
        }
        store::RESULT_FAILED | store::RESULT_STALLED | store::RESULT_ROLLED_BACK => {
            yellow = true;
            next.push(item("yellow", "deploy", "the last deploy didn't go live; the previous Seed still serves", format!("k2 app errors {}", app.app_id)));
        }
        _ => {}
    }

    let health = health_part(&app);
    if app.state == store::STATE_CRASHLOOPING {
        red = true;
        next.push(item("red", "health", "the App is crash looping", format!("k2 app errors {} --kind crash", app.app_id)));
    } else if health.get("ok") == Some(&J::Bool(false)) {
        red = true;
        next.push(item("red", "health", "the health check isn't answering 2xx", format!("k2 app logs {}", app.app_id)));
    } else if !live && dep["last"].is_null() && app.live_seed.is_none() {
        yellow = true;
        next.push(item("yellow", "deploy", "nothing deployed yet", format!("k2 app deploy {} <folder>", app.app_id)));
    } else if !live && (app.state == store::STATE_STOPPED || app.state == store::STATE_PARKED) {
        next.push(item("info", "health", "the App is stopped", format!("k2 app start {}", app.app_id)));
    } else if !live && !yellow {
        red = true;
        next.push(item("red", "health", "nothing is live", format!("k2 app start {}", app.app_id)));
    }

    let checks = checks_part(&app, run_checks, public);
    if let Some(failed) = checks.get("failed").and_then(J::as_u64).filter(|n| *n > 0) {
        red = true;
        next.push(item("red", "checks", &format!("{failed} of the App's host.checks failed"), format!("k2 app status {} --json", app.app_id)));
    }

    let errors = errors_part(&app);
    if errors.get("total").and_then(J::as_u64).is_some_and(|n| n > 0) {
        yellow = true;
        next.push(item("yellow", "errors", "failures in the last hour", format!("k2 app errors {} --since 1h", app.app_id)));
    }

    let resources = resources_part(who, &app);
    if resources.get("level").and_then(J::as_str).is_some_and(|l| l != "ok") {
        yellow = true;
        next.push(item("yellow", "resources", "a disk is past the guard's line", format!("k2 app resources {}", app.app_id)));
    }

    let door = door_part(who, &app);
    if let Some(n) = door.get("heldApprovals").and_then(J::as_u64).filter(|n| *n > 0) {
        yellow = true;
        next.push(item("yellow", "door", &format!("{n} request(s) wait for the owner"), format!("k2 app approvals {}", app.app_id)));
    }
    if door.get("egressDenied").and_then(J::as_u64).is_some_and(|n| n > 0) {
        next.push(item("info", "door", "the App was refused an outside host", format!("k2 app egress log {}", app.app_id)));
    }

    let hosts = hosts_part(who, &app);
    let phone = phone_part(&app);
    let updates = updates_part(&app);

    let verdict = if red {
        "red"
    } else if yellow {
        "yellow"
    } else {
        "green"
    };
    Ok(json!({
        "ok": true,
        "schema": SCHEMA,
        "app": app.app_id,
        "verdict": verdict,
        "isolation": isolation_words(&app),
        "deploy": dep,
        "health": health,
        "checks": checks,
        "errors": errors,
        "resources": resources,
        "door": door,
        "hosts": hosts,
        "phone": phone,
        "updates": updates,
        "next": next,
    }))
}

fn item(level: &str, part: &str, what: &str, run: String) -> J {
    json!({ "level": level, "part": part, "what": what, "run": run })
}

/// How the App runs, in the words `inspect` uses (static Apps: never a
/// weaker wall, TC-2).
fn isolation_words(app: &store::AppRow) -> String {
    match app.isolation.as_str() {
        "container" => "walled (container)".into(),
        "none" if app.path == "static" => "none (static: no App code)".into(),
        other => other.to_string(),
    }
}

/// 1. Deploy: the live Seed (who, commit, when) and the last deployment.
fn deploy_part(app: &store::AppRow) -> R<J> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let seed = match app.live_seed.as_deref() {
        Some(h) => store::get_seed(&conn, h).map_err(db_err)?,
        None => None,
    };
    let last = store::list_deployments(&conn, &app.app_id, 1).map_err(db_err)?.into_iter().next();
    let live_dep = match app.live_deployment.as_deref() {
        Some(d) => store::get_deployment(&conn, d).map_err(db_err)?,
        None => None,
    };
    Ok(json!({
        "state": app.state,
        "stateReason": app.state_reason.as_deref().map(deploy::framing::strip_controls),
        "seed": seed.map(|s| json!({ "hash": s.hash, "version": s.version, "sourceRef": s.source_ref, "builtAt": s.created_at, "sizeBytes": s.size_bytes })),
        "live": live_dep.map(|d| json!({ "id": d.id, "actor": d.actor, "commit": d.commit_ref, "liveAt": d.live_at })),
        "last": last.map(|d| json!({
            "id": d.id, "kind": d.kind, "result": d.result, "actor": d.actor, "startedAt": d.started_at, "finishedAt": d.finished_at,
            "errorCode": d.error_code, "error": d.error.as_deref().map(deploy::framing::strip_controls),
        })),
    }))
}

/// 2. Health: one fresh probe of the health path (its latency), uptime,
/// restarts and backoff.
fn health_part(app: &store::AppRow) -> J {
    let rt = super::supervisor::runtime_json(&app.app_id);
    let port = super::supervisor::live_port(&app.app_id);
    let (path, tcp) = health_target(app);
    let probe = port.map(|p| {
        let t0 = Instant::now();
        let r = deploy::health::probe_as(tcp, p, &path, &[], Duration::from_secs(5));
        let ms = t0.elapsed().as_millis() as u64;
        match r {
            deploy::health::Probe::Status(s) => json!({ "status": s, "ms": ms }),
            deploy::health::Probe::NoAnswer(_) => json!({ "status": null, "ms": ms, "answer": "none" }),
        }
    });
    let ok = probe.as_ref().map(|p| p["status"].as_u64().is_some_and(|s| (200..300).contains(&s)));
    let up_since = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        app.live_deployment.as_deref().and_then(|d| store::get_deployment(&conn, d).ok().flatten()).and_then(|d| d.live_at)
    };
    json!({
        "ok": ok,
        "path": if tcp { "(port open)".to_string() } else { path },
        "probe": probe,
        "upSince": up_since,
        "uptimeSecs": up_since.map(|t| (deploy::now() - t).max(0)),
        "recentRestarts": rt.get("recentRestarts"),
        "restartAt": rt.get("restartAt"),
        "busy": rt.get("busy"),
        "watchUntilSecs": rt.get("watchUntilSecs"),
    })
}

/// The live Seed's health path (`host.health`), and whether it is the
/// port-open probe.
fn health_target(app: &store::AppRow) -> (String, bool) {
    let manifest = app.live_seed.as_deref().and_then(|h| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_seed(&conn, h).ok().flatten().map(|s| s.manifest)
    });
    let h = manifest.as_ref().and_then(|m| m.pointer("/host/health").cloned()).unwrap_or(J::Null);
    let path = h.get("path").and_then(J::as_str).unwrap_or("/").to_string();
    let tcp = h.get("tcp").and_then(J::as_bool).unwrap_or(false);
    (path, tcp)
}

/// 3. Checks (TC-16): fresh, or the run recorded after the last switch.
fn checks_part(app: &store::AppRow, fresh: bool, public: bool) -> J {
    let checks = match super::checks::of_live_seed(app) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e }),
    };
    if checks.is_empty() {
        return json!({ "declared": 0, "results": [], "note": "no host.checks in the live Seed's manifest" });
    }
    if !fresh {
        let rec = app.live_deployment.as_deref().and_then(|d| super::checks::recorded(&app.app_id, d));
        let failed = rec.as_ref().and_then(|r| r["results"].as_array()).map(|a| a.iter().filter(|x| x["ok"] != J::Bool(true)).count());
        return json!({ "declared": checks.len(), "source": "recorded", "recorded": rec, "failed": failed });
    }
    let base = if public {
        match public_base(app) {
            Some(b) => b,
            None => return json!({ "declared": checks.len(), "error": "no public host for this App (k2 app domains)" }),
        }
    } else {
        match super::supervisor::live_port(&app.app_id) {
            Some(p) => format!("http://127.0.0.1:{p}"),
            None => return json!({ "declared": checks.len(), "error": "nothing is live to check" }),
        }
    };
    let results = super::checks::run(&app.app_id, &base, &checks);
    let failed = results.iter().filter(|r| !r.ok).count();
    json!({
        "declared": checks.len(),
        "source": if public { "public" } else { "loopback" },
        "results": results.iter().map(super::checks::Outcome::to_json).collect::<Vec<_>>(),
        "failed": failed,
    })
}

/// `https://<the App's label host>` for `--public`.
fn public_base(app: &store::AppRow) -> Option<String> {
    super::routes::url_for(app).map(|u| u.trim_end_matches('/').to_string())
}

/// 4. Errors in the last hour by kind (crash, health, build, deploy).
fn errors_part(app: &store::AppRow) -> J {
    let conn = match k2_core::app_audit::open(&k2_core::app_audit::db_path()) {
        Ok(c) => c,
        Err(_) => return json!({ "unavailable": "audit_unavailable" }),
    };
    let since_ms = (deploy::now() - WINDOW_SECS) * 1000;
    let q = k2_core::app_audit::FailureQuery { app_id: &app.app_id, since_ms: Some(since_ms), kind: None, limit: 500 };
    let rows = match k2_core::app_audit::list_failures(&conn, &q) {
        Ok(r) => r,
        Err(_) => return json!({ "unavailable": "audit_unavailable" }),
    };
    let mut by: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for r in &rows {
        *by.entry(r.failure.kind.clone()).or_default() += 1;
    }
    json!({ "windowSecs": WINDOW_SECS, "total": rows.len(), "byKind": by, "last": rows.first().map(|r| json!({ "kind": r.failure.kind, "at": r.failure.at_ms / 1000, "reason": r.failure.reason })) })
}

/// 5. Resources: the App's row in the box view and the worst disk level.
fn resources_part(who: &Who, app: &store::AppRow) -> J {
    let mut p = HashMap::new();
    p.insert("app".to_string(), app.app_id.clone());
    let v = part(super::resources::handle("/cli/app/resources", who, b"", &p));
    if v.get("unavailable").is_some() {
        return v;
    }
    let row = v["consumers"].as_array().and_then(|a| a.first()).cloned().unwrap_or(J::Null);
    let level = v["disks"].as_array().map(|d| {
        let levels: Vec<&str> = d.iter().filter_map(|x| x["level"].as_str()).collect();
        ["critical", "refuse", "warn"].into_iter().find(|l| levels.contains(l)).unwrap_or("ok")
    });
    json!({ "app": row, "level": level, "guard": v.get("guard") })
}

/// 6. Door: egress refused in the last hour, held approvals.
fn door_part(who: &Who, app: &store::AppRow) -> J {
    let cutoff = deploy::now() - WINDOW_SECS;
    let denied = super::egress_proxy::tail(&app.app_id, 2000)
        .iter()
        .filter(|l| l.get("allowed") == Some(&J::Bool(false)) && l.get("at").and_then(J::as_i64).is_some_and(|t| t >= cutoff))
        .count();
    let mut p = HashMap::new();
    p.insert("app".to_string(), app.app_id.clone());
    let held = part(super::approvals::handle("/cli/app/approvals", who, b"", &p));
    let held_n = held.get("requests").and_then(J::as_array).map(|a| a.len());
    json!({ "windowSecs": WINDOW_SECS, "egressDenied": denied, "heldApprovals": held_n, "approvals": if held_n.is_none() { held } else { J::Null } })
}

/// 7. Hosts: the label and URL, custom domains and their certificate state.
fn hosts_part(who: &Who, app: &store::AppRow) -> J {
    let lab = super::open::label_of(app);
    let mut p = HashMap::new();
    p.insert("app".to_string(), app.app_id.clone());
    let domains = part(super::domains::handle("/cli/app/domains", who, b"", &p));
    json!({ "label": lab.text, "url": super::routes::url_for(app), "domains": domains })
}

/// 8. Phone: pairing, link files, push keys (yes/no) and devices.
fn phone_part(app: &store::AppRow) -> J {
    let svc = app.service_id.as_str();
    let pairing = match crate::app_pairing::state(svc) {
        Ok(c) => json!({ "on": true, "linkPaths": c.link_paths.len() }),
        Err(off) => json!({ "on": false, "why": off.as_str() }),
    };
    let push = match k2_core::skin::app_push::config(svc) {
        Ok(cfg) => {
            let devices: i64 = k2_core::skin::app_push::device_counts(svc).map(|v| v.iter().map(|(_, n, _)| *n).sum()).unwrap_or(0);
            json!({
                "on": crate::app_push::state(svc).is_ok(),
                "keys": { "apns": cfg.apns.is_some(), "fcm": cfg.fcm.is_some() },
                "agentReplies": cfg.agent_replies,
                "devices": devices,
            })
        }
        Err(_) => json!({ "on": false }),
    };
    json!({ "pairing": pairing, "push": push })
}

/// 9. Updates: the update feed (prd-tauri-core-0467-v1 TC-6…TC-12) when
/// this server has it.
fn updates_part(_app: &store::AppRow) -> J {
    json!({ "feed": "off", "why": "not_built" })
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
