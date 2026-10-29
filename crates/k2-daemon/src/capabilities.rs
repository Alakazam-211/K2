//! `GET /cli/capabilities`: what this server can do for Apps, before an
//! agent tries (prd-tauri-core-0467-v1 TC-17).
//!
//! Rows `{cap, ok, detail, missing?: {reason, fix, code}}`, with the reason
//! words `k2 mail capabilities` uses (`not_built`, `owner_disabled`,
//! `level`) plus `not_prepared` and `hardware`. A "no" row names the fix
//! and, when a route would refuse, the refusal id (a row of
//! `k2_core::refusals`, so `k2 explain <code>` answers it; a test holds
//! that). Nothing secret: push keys are set or not, never a key.
//!
//! - **This box:** Apps on, the engines (walled, open, static), protocols,
//!   public ports, custom domains, phone simulators, Postgres majors, mail,
//!   push keys per App the caller sees, the update feed, limits.
//! - **Compute:** each node the caller may use (`k2 compute nodes`' own
//!   rule: a passport sees its workspace's granted nodes).
//! - **doctor:** `k2 app doctor`'s verdict, included, not redone.
//!
//! Any credential but an App pass; a passport reads its own workspace's
//! Apps and nodes.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const ROUTE: &str = "/cli/capabilities";
/// The answer's shape version.
pub const SCHEMA: u32 = 1;

/// The reason words.
pub const NOT_BUILT: &str = "not_built";
pub const OWNER_DISABLED: &str = "owner_disabled";
pub const LEVEL: &str = "level";
pub const NOT_PREPARED: &str = "not_prepared";
pub const HARDWARE: &str = "hardware";

/// One row.
pub fn row(cap: &str, ok: bool, detail: impl Into<String>) -> J {
    json!({ "cap": cap, "ok": ok, "detail": detail.into() })
}

/// A "no" row: why, the fix and the refusal id a route would answer
/// (`None`: no route refuses it, K2 just doesn't do it).
pub fn no_row(cap: &str, detail: impl Into<String>, reason: &str, fix: &str, code: Option<&str>) -> J {
    json!({ "cap": cap, "ok": false, "detail": detail.into(), "missing": { "reason": reason, "fix": fix, "code": code } })
}

fn feature_on(f: &str) -> bool {
    crate::boot_status::features().iter().any(|x| *x == f)
}

/// `GET /cli/capabilities[?node=]`.
pub fn handle(who: Who, params: &HashMap<String, String>) -> CliResponse {
    match who {
        Who::Owner | Who::Login { .. } | Who::Agent { .. } | Who::Shell { .. } => {}
        Who::Wrong => {
            return crate::apps::routes::refusal_response(&crate::apps::routes::no(
                k2_core::deploy::codes::WRONG_CREDENTIAL,
                403,
                "capabilities are for the owner, a login or an agent's passport, never an App pass or an API key",
            ))
        }
        Who::Invalid => return crate::apps::routes::refusal_response(&crate::apps::routes::no(k2_core::deploy::codes::FORBIDDEN, 403, "send a valid token")),
    }
    let node = params.get("node").map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let (doctor, doctor_json) = doctor_part(&who);
    let box_rows = if node.is_some() { Vec::new() } else { box_rows(&who, &doctor_json) };
    let compute = compute_rows(&who, node.as_deref());
    if let Some(n) = &node {
        if compute.is_empty() {
            return crate::apps::routes::refusal_response(&crate::apps::routes::no(
                "not_found",
                404,
                format!("no compute node '{n}' this workspace may use (k2 compute nodes)"),
            ));
        }
    }
    CliResponse::ok_json(
        json!({ "ok": true, "schema": SCHEMA, "box": box_rows, "compute": compute, "doctor": if node.is_some() { J::Null } else { doctor } }).to_string(),
    )
}

/// `k2 app doctor`'s verdict (ready, the failing check ids) and its JSON.
fn doctor_part(who: &Who) -> (J, J) {
    if !k2_core::deploy::enabled() {
        return (json!({ "ready": false, "apps": false }), J::Null);
    }
    match crate::apps::box_prep::box_route(who) {
        Ok(v) => {
            let failing: Vec<J> = v["checks"]
                .as_array()
                .map(|a| a.iter().filter(|c| c["status"] == "fail").map(|c| c["id"].clone()).collect())
                .unwrap_or_default();
            (json!({ "ready": v["ready"], "linux": v["linux"], "failing": failing }), v)
        }
        Err(e) => (json!({ "unavailable": e.code }), J::Null),
    }
}

fn box_rows(who: &Who, doctor: &J) -> Vec<J> {
    let mut out = Vec::new();
    let apps_on = k2_core::deploy::enabled();
    out.push(if apps_on {
        row("apps", true, "Apps (beta) are on")
    } else {
        no_row("apps", "Apps (beta) are off on this server", OWNER_DISABLED, "the owner turns them on in Settings → Apps → This server", Some("apps_disabled"))
    });
    let linux = cfg!(target_os = "linux");
    let ready = doctor["ready"].as_bool().unwrap_or(false);
    out.push(if !linux {
        no_row("engine.walled", "walled Apps run on Linux boxes only", HARDWARE, "deploy walled Apps to a Linux K2 server", Some("linux_only"))
    } else if ready {
        row("engine.walled", true, format!("ready (runtime {})", doctor["runtime"].as_str().unwrap_or("crun")))
    } else {
        no_row("engine.walled", "the box isn't set up for walled Apps yet", NOT_PREPARED, "the owner runs k2 app box prepare (k2 app doctor lists what is missing)", Some("engine_unavailable"))
    });
    out.push(if doctor["processEngine"].as_bool().unwrap_or(!linux) {
        row("engine.open", true, "the open (process) path: Apps run unwalled")
    } else {
        no_row("engine.open", "the open path is off here", OWNER_DISABLED, "deploy walled Apps, or the owner opens the path for one App (k2 app exception)", Some("open_path_only"))
    });
    out.push(if feature_on("app-static-v1") {
        row("engine.static", true, "static Apps: a built folder served with no process (host.mode \"static\")")
    } else {
        no_row("engine.static", "this K2 doesn't serve static Apps on the runtime yet", NOT_BUILT, "serve the folder with k2 publish, or update K2 (k2 update --app)", Some("static_not_on_runtime"))
    });
    out.push(row("protocols", true, "HTTP/1.1 and WebSocket on the App's labels and custom domains"));
    out.push(no_row(
        "protocols.other",
        "HTTP/2 to the App, gRPC and UDP",
        NOT_BUILT,
        "speak HTTP/1.1 or WebSocket to K2's front; K2 terminates TLS and HTTP/2 for browsers",
        None,
    ));
    out.push(no_row("ports.public", "an App can't open its own public TCP port", NOT_BUILT, "serve on the App's label (HTTPS) or a custom domain", None));
    out.push(custom_domains_row());
    out.push(if crate::sim_routes::boot_status_key().is_some() {
        row("sim", true, "a phone simulator or emulator lane can run here (k2 sim status)")
    } else {
        no_row("sim", "no phone simulator lane can run here", HARDWARE, "run phone builds on a compute node with Xcode or the Android SDK (k2 compute nodes)", None)
    });
    let majors: Vec<String> = k2_root_proto::pg_major::SUPPORTED.iter().map(|m| m.to_string()).collect();
    out.push(row("postgres", true, format!("majors {}", majors.join(", "))));
    out.push(if crate::email_notify::box_enabled() {
        row("mail", true, "an App can send email to its own users once its owner grants a sender (k2 app email on)")
    } else {
        no_row("mail", "email from Apps is off on this server", OWNER_DISABLED, "the owner turns it on (Settings → Notifications → Email)", Some("email_off"))
    });
    out.extend(push_rows(who));
    out.push(if feature_on("app-updates-v1") {
        row("updates", true, "a desktop app's update feed (k2 app updates)")
    } else {
        no_row("updates", "this K2 has no update feed for desktop apps yet", NOT_BUILT, "update K2 (k2 update --app); until then host latest.json yourself", Some("updates_host_unavailable"))
    });
    let q = crate::apps::quotas::DEFAULTS;
    out.push(row(
        "limits",
        true,
        format!(
            "Seed {} GiB and {}k entries; request body {} MiB, upload part {} MiB; disk guard lines: k2 app resources",
            k2_core::deploy::seed::MAX_BYTES >> 30,
            k2_core::deploy::seed::MAX_ENTRIES / 1000,
            q.body_bytes >> 20,
            q.upload_bytes >> 20,
        ),
    ));
    out
}

fn custom_domains_row() -> J {
    row("domains.custom", true, "an App answers on its owner's own domain once its DNS points here (k2 app domain add)")
}

/// Push keys per App the caller sees: set or not (never the key).
fn push_rows(who: &Who) -> Vec<J> {
    let apps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::list_apps(&conn, None).unwrap_or_default()
    };
    let mine = |a: &k2_core::deploy::store::AppRow| match who.passport() {
        Some((ws, _)) => a.owner_workspace == ws,
        None => true,
    };
    apps.iter()
        .filter(|a| mine(a))
        .map(|a| match k2_core::skin::app_push::config(&a.service_id) {
            Ok(cfg) => {
                let (apns, fcm) = (cfg.apns.is_some(), cfg.fcm.is_some());
                let detail = format!("{}: APNs key {}, FCM key {}", a.app_id, if apns { "set" } else { "not set" }, if fcm { "set" } else { "not set" });
                if apns || fcm {
                    row(&format!("push.{}", a.app_id), true, detail)
                } else {
                    no_row(&format!("push.{}", a.app_id), detail, NOT_PREPARED, "the App's owner adds a key: k2 app push setup <app>", Some("push_not_configured"))
                }
            }
            Err(_) => no_row(&format!("push.{}", a.app_id), format!("{}: no push settings", a.app_id), NOT_PREPARED, "k2 app push setup <app>", Some("push_not_configured")),
        })
        .collect()
}

/// Compute nodes the caller may use (`k2 compute nodes`' rule).
fn compute_rows(who: &Who, only: Option<&str>) -> Vec<J> {
    if !k2_core::compute::enabled() {
        return Vec::new();
    }
    let r = crate::compute_routes::handle("/cli/compute/nodes", who.clone(), b"", &HashMap::new());
    if !r.status.starts_with("200") {
        return Vec::new();
    }
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    v["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter(|n| only.is_none_or(|o| n["name"].as_str() == Some(o) || n["id"].as_str() == Some(o)))
                .map(node_row)
                .collect()
        })
        .unwrap_or_default()
}

/// One node: OS and CPU, free slots, Xcode, the Android SDK, tools.
pub fn node_row(n: &J) -> J {
    let name = n["name"].as_str().unwrap_or("?");
    let os = n["os"].as_str().unwrap_or("?");
    let arch = n["arch"].as_str().unwrap_or("?");
    let online = n["online"].as_bool().unwrap_or(false);
    let offer = &n["offer"];
    let tools_text = offer["tools"].to_string().to_ascii_lowercase();
    let xcode = tools_text.contains("xcode");
    let android = tools_text.contains("android");
    let slots = offer["parallelMax"].as_u64();
    let running = n["running"].as_u64().unwrap_or(0);
    let free = slots.map(|s| s.saturating_sub(running));
    let detail = format!(
        "{os}-{arch}, {}, {} free slot(s), Xcode {}, Android SDK {}",
        if online { "online" } else { "offline" },
        free.map(|f| f.to_string()).unwrap_or_else(|| "?".into()),
        if xcode { "yes" } else { "no" },
        if android { "yes" } else { "no" },
    );
    let mut r = if online {
        row(&format!("node.{name}"), true, detail)
    } else {
        no_row(&format!("node.{name}"), detail, NOT_PREPARED, "the node's machine is off or offline (k2 compute status)", None)
    };
    r["node"] = json!({ "name": name, "os": os, "arch": arch, "online": online, "freeSlots": free, "xcode": xcode, "androidSdk": android, "tools": offer["tools"] });
    r
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
