//! Link files as an App setting (prd-tauri-core-0467-v1 TC-20…TC-22):
//! `k2 app links`. The App's iOS and Android app ids and its universal-link
//! paths; K2 serves the AASA and `assetlinks.json` on every host of the
//! App **with or without phone pairing**. When pairing is on too, both feed
//! one per-App snapshot (`app_pairing::links`): one AASA, pairing's path
//! added, as today; a pairing-only App's files stay byte for byte the same.
//!
//! ```text
//! GET  /cli/app/links        ?app=                                  Admin; the App's agent ("Build and run Apps")
//! POST /cli/app/links/set    {app, ios?, android?, paths?, drop?}   the owner, an Admin, or the App's agent ("Build and run Apps")
//! POST /cli/app/links/off    {app}                                  the same
//! POST /cli/app/links/check  {app}                                  the owner: fetches the files from each host over the internet
//! ```
//!
//! - Paths may carry a query pattern (`/messages?addr=*`, TA-21), kept
//!   parsed: the `components` form gets `"/"` and `"?"`, the classic `paths`
//!   form the path alone.
//! - **Frozen paths:** a path that shipped stays until it is dropped on
//!   purpose (`drop` names it, `--drop` in the CLI): a set that leaves one
//!   out is refused `link_path_dropped`, since installed apps and Apple's
//!   CDN keep the old file for days.
//! - Raw Apps (`host.clientIp: false`) are refused `needs_k2_front`: K2
//!   can't answer on their hosts (TA-10).
//! - Every change goes into the App's deploy audit.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use k2_core::skin::app_pairing as store;
use serde_json::{json, Value as J};

use super::grace::{target, Target};
use super::routes::{audit_human, json_body, need_credential, no, Refusal, Who, R};
use crate::app_pairing::links::{self, LinkFiles};
use crate::app_push::owner_routes::{admin_human, app_agent_at, owner_in_person};

pub const STATUS: &str = "/cli/app/links";
pub const SET: &str = "/cli/app/links/set";
pub const OFF: &str = "/cli/app/links/off";
pub const CHECK: &str = "/cli/app/links/check";

pub const ROUTES: &[&str] = &[STATUS, SET, OFF, CHECK];
pub const POST_ROUTES: &[&str] = &[SET, OFF, CHECK];

/// The refusal for an App K2 can't answer in front of (raw, or not a
/// runtime App); the shared id for pairing's `pairing_needs_k2_front`.
pub const NEEDS_K2_FRONT: &str = "needs_k2_front";
/// A set left out paths that shipped.
pub const LINK_PATH_DROPPED: &str = "link_path_dropped";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn store_err(e: String) -> Refusal {
    no("store", 500, e)
}

fn links_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "link files are for a published App, never the box login (host)"));
    }
    target(who, raw)
}

/// The owner, an Admin, or the App's own agent with "Build and run Apps".
fn may_change(who: &Who, t: &Target) -> R<()> {
    if admin_human(who) || app_agent_at(who, t, super::access::Level::Build) {
        Ok(())
    } else {
        Err(no(
            "forbidden",
            403,
            "an App's link files are the owner's, Admins' and this App's own agent's to change (\"Build and run Apps\" on)",
        ))
    }
}

/// Rebuild one App's link-file part of the snapshot from its setting and
/// hosts (set, off, boot, every deploy via `app_pairing::refresh_links`).
pub fn refresh(service_id: &str) {
    let Some(app) = crate::app_pairing::runtime_app(service_id) else { return };
    let cfg = match store::links_config(service_id) {
        Ok(c) => c.filter(|c| c.is_on()),
        Err(e) => {
            k2_core::log_debug!("[app-links] {service_id}: {e}");
            None
        }
    };
    let files = match (cfg, crate::app_pairing::front_rewrites(service_id)) {
        (Some(c), Ok(_)) => Some(LinkFiles { ios: c.ios, android: c.android, paths: c.paths, hosts: crate::app_pairing::pairing_hosts(service_id) }),
        _ => None,
    };
    // Most Apps never set link files: no snapshot write for them.
    if files.is_some() || links::get_files(&app.app_id).is_some() {
        links::set_files(&app.app_id, files);
    }
}

/// Every App's link-file part (boot).
pub fn refresh_all() {
    match store::all_links_configs() {
        Ok(all) => {
            for c in all {
                refresh(&c.app);
            }
        }
        Err(e) => k2_core::log_debug!("[app-links] WARN boot snapshot: {e}"),
    }
}

/// The last `check` per App (in memory; status shows it).
fn last_checks() -> &'static Mutex<HashMap<String, (i64, J)>> {
    static M: OnceLock<Mutex<HashMap<String, (i64, J)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn status_json(t: &Target) -> R<J> {
    let cfg = store::links_config(&t.service_id).map_err(store_err)?;
    let served = crate::app_pairing::front_rewrites(&t.service_id);
    let hosts = crate::app_pairing::pairing_hosts(&t.service_id);
    let pairing_on = crate::app_pairing::state(&t.service_id).is_ok();
    let on = cfg.as_ref().is_some_and(|c| c.is_on()) && served.is_ok();
    let paths: Vec<J> = cfg
        .as_ref()
        .map(|c| c.paths.iter().map(|p| json!({ "spec": p.spec(), "path": p.path, "query": p.query, "since": p.since })).collect())
        .unwrap_or_default();
    let mut out = json!({
        "ok": true,
        "app": t.service_id,
        "appId": t.app_id,
        "name": t.name,
        "state": if on { "on" } else { "off" },
        "set": cfg.is_some(),
        "served": served.is_ok(),
        "notServed": served.err().map(crate::app_pairing::NotServed::hint),
        "hosts": hosts,
        "ios": cfg.as_ref().map(|c| json!(c.ios)).unwrap_or(json!([])),
        "android": cfg.as_ref().map(|c| json!(c.android)).unwrap_or(json!([])),
        "paths": paths,
        "pairing": pairing_on,
        "updatedAt": cfg.as_ref().map(|c| c.updated_at),
        "updatedBy": cfg.as_ref().and_then(|c| c.updated_by.clone()),
    });
    if let Some((at, results)) = last_checks().lock().unwrap_or_else(|e| e.into_inner()).get(&t.service_id).cloned() {
        out["lastCheck"] = json!({ "at": at, "hosts": results });
    }
    Ok(out)
}

/// GET `/cli/app/links?app=`.
fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = links_target(who, params.get("app").map(String::as_str))?;
    may_change(who, &t)?;
    status_json(&t)
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    super::routes::s(v, k)
}

/// The runtime App behind the target, or `needs_k2_front`.
fn served_app(t: &Target) -> R<String> {
    crate::app_pairing::front_rewrites(&t.service_id).map_err(|why| no(NEEDS_K2_FRONT, 409, why.hint()))
}

/// POST `/cli/app/links/set`.
fn set(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = links_target(who, s(&v, "app"))?;
    may_change(who, &t)?;
    let input = store::parse_links_input(&v).map_err(|e| no("usage", 400, e))?;
    let app_id = served_app(&t)?;
    let by = super::routes::actor(who);
    let before = store::links_config(&t.service_id).map_err(store_err)?;
    let cfg = match store::set_links(&t.service_id, &input, &by, now()) {
        Ok(c) => c,
        Err(store::LinksError::Dropped(gone)) => {
            return Err(Refusal {
                extra: Some(json!({ "dropped": gone })),
                ..no(
                    LINK_PATH_DROPPED,
                    409,
                    format!(
                        "this set leaves out {} that already shipped. Installed apps and Apple's CDN keep the old file for days, so a shipped \
                         path is only removed on purpose: list it again, or drop it with {}",
                        gone.join(", "),
                        gone.iter().map(|g| format!("--drop '{g}'")).collect::<Vec<_>>().join(" ")
                    ),
                )
            })
        }
        Err(store::LinksError::Store(e)) => return Err(no("usage", 400, e)),
    };
    refresh(&t.service_id);
    let specs: Vec<String> = cfg.paths.iter().map(|p| p.spec()).collect();
    audit_human(
        &app_id,
        "links_set",
        who,
        json!({ "ios": cfg.ios, "android": cfg.android.iter().map(|a| a.package.clone()).collect::<Vec<_>>(), "paths": specs, "dropped": input.drop }),
    );
    let mut out = status_json(&t)?;
    let mut warnings = vec![format!(
        "K2 itself now answers /.well-known/apple-app-site-association and /.well-known/assetlinks.json on {}: the App's own files there are replaced",
        out["hosts"].as_array().map(|h| h.iter().filter_map(J::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
    )];
    if before.as_ref().is_some_and(|b| b.ios != cfg.ios || b.android != cfg.android) {
        warnings.push("phones that already checked the old app ids may keep the old ones for up to about a week (Apple's CDN and Android re-check on their own schedule)".into());
    }
    if cfg.paths.iter().any(|p| !p.query.is_empty()) {
        warnings.push("a query pattern (?name=*) is matched by iOS 14 and newer; older iOS matches the path alone".into());
    }
    warnings.push("list each host exactly in your app (applinks:<host>), never a wildcard: a wildcard makes iOS read k2.dev's own file".into());
    out["warnings"] = json!(warnings);
    Ok(out)
}

/// POST `/cli/app/links/off {app}`.
fn off(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = links_target(who, s(&v, "app"))?;
    may_change(who, &t)?;
    let by = super::routes::actor(who);
    let was = store::links_off(&t.service_id, &by, now()).map_err(store_err)?;
    refresh(&t.service_id);
    if was {
        if let Some(app_id) = t.app_id.as_deref() {
            audit_human(app_id, "links_off", who, json!({}));
        }
    }
    let mut out = status_json(&t)?;
    out["wasOn"] = json!(was);
    Ok(out)
}

/// The fetcher `check` uses (tests swap in a fake; never the internet).
fn fetcher() -> &'static Mutex<Option<Box<dyn links::Fetcher + Send>>> {
    static F: OnceLock<Mutex<Option<Box<dyn links::Fetcher + Send>>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(None))
}

/// Test seam: use `f` for `check` instead of the internet.
#[cfg(test)]
pub(crate) fn set_fetcher_for_tests(f: Option<Box<dyn links::Fetcher + Send>>) {
    *fetcher().lock().unwrap_or_else(|e| e.into_inner()) = f;
}

/// POST `/cli/app/links/check {app}`: the owner only (it calls the
/// internet; pairing's public fetch and Apple's CDN copy).
fn check(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = links_target(who, s(&v, "app"))?;
    if !owner_in_person(who) && !matches!(who, Who::Shell { .. }) {
        return Err(no("owner_only", 403, "checking an App's link files fetches them over the internet: the owner runs k2 app links check"));
    }
    if crate::push_routes::airgap_blocks() {
        return Err(no("airgap", 503, "air-gap is on: nothing leaves this server"));
    }
    let app_id = served_app(&t)?;
    // Both parts of the snapshot, as they are now.
    crate::app_pairing::refresh_links(&t.service_id);
    let files = links::get_files(&app_id);
    let pair = links::get(&app_id).filter(|_| crate::app_pairing::box_enabled());
    if files.is_none() && pair.is_none() {
        return Err(no("links_off", 409, "K2 serves no link files for this App (k2 app links set)"));
    }
    let results = {
        let mut f = fetcher().lock().unwrap_or_else(|e| e.into_inner());
        match f.as_mut() {
            Some(fake) => links::check_merged(fake.as_ref(), pair.as_deref(), files.as_deref()),
            None => links::check_merged(&links::NetFetcher, pair.as_deref(), files.as_deref()),
        }
    };
    let hosts: Vec<J> = results
        .iter()
        .map(|h| json!({ "host": h.host, "aasa": h.aasa, "assetlinks": h.assetlinks, "appleCdn": h.apple_cdn }))
        .collect();
    last_checks().lock().unwrap_or_else(|e| e.into_inner()).insert(t.service_id.clone(), (now(), json!(hosts)));
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "hosts": hosts }))
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        STATUS => status(who, params),
        SET => set(who, body),
        OFF => off(who, body),
        CHECK => check(who, body),
        _ => Err(no("not_found", 404, "no such App links route")),
    }
}

#[cfg(test)]
#[path = "link_files_tests.rs"]
mod tests;
