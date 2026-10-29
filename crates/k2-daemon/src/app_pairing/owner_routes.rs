//! `/cli/app/pairing*`: the owner's, Admins' and the App agent's pairing
//! controls (`prd-app-phone-pairing-v1` §5, CP22–CP24, vs-live V2, V6,
//! V22, V23; S3; `k2 app pairing …`). Not App-facing: an App pass, an API
//! key or an API session is refused.
//!
//! ```text
//! GET  /cli/app/pairing        ?app=   Admin; the App's agent ("Build and run Apps"): config, state, hosts, counts
//! POST /cli/app/pairing/set    {app, ios?, android?, hosts?, nativeOrigins?, linkPaths?, store?, confirmToken}
//!                                      the owner in person + a fresh confirm (vault action app-pairing-apps)
//! POST /cli/app/pairing/off    {app}   Admin, or the App's agent ("Build and run Apps"): lower only
//! POST /cli/app/pairing/check  {app}   the owner: fetches the link files from each host over the internet
//! GET  /cli/app/pairing/log    ?app=   Admin; the App's agent with its App-user switch (it names people)
//! ```
//!
//! The phone app ids decide which phone app receives sign-in codes by
//! universal link, so setting them needs the owner in person and a fresh
//! confirm bound to the App (Touch ID, the login or the vault password):
//! the owner token alone is readable by agents (CP22, V22). An agent asks
//! with a permission card. Values are checked before the confirm is spent.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use k2_core::skin::{app_login, app_pairing as store};
use serde_json::{json, Value as J};

use crate::app_push::owner_routes::{admin_human, app_agent_at, fresh_confirm, owner_in_person};
use crate::apps::grace::{target, Target};
use crate::apps::routes::{actor, json_body, need_credential, no, respond, s, Refusal, Who, R};
use crate::cli_response::CliResponse;

pub const STATUS: &str = "/cli/app/pairing";
pub const SET: &str = "/cli/app/pairing/set";
pub const OFF: &str = "/cli/app/pairing/off";
pub const CHECK: &str = "/cli/app/pairing/check";
pub const LOG: &str = "/cli/app/pairing/log";

pub const ROUTES: &[&str] = &[STATUS, SET, OFF, CHECK, LOG];
pub const POST_ROUTES: &[&str] = &[SET, OFF, CHECK];
/// Request bodies here are at most this many bytes.
pub const MAX_BODY: usize = 32 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

pub fn too_large() -> CliResponse {
    respond(Err(no("too_large", 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024))))
}

/// The App a call names, checked against the caller (a passport sees only
/// its own workspace's Apps; host never pairs).
fn pairing_target(who: &Who, raw: Option<&str>) -> R<Target> {
    need_credential(who)?;
    if raw.map(str::trim) == Some(k2_core::skin::HOST_APP_ID) {
        return Err(no("not_for_host", 400, "pairing is for a published App's own users, never the box login (host)"));
    }
    target(who, raw)
}

fn store_err(e: String) -> crate::apps::routes::Refusal {
    no("store", 500, e)
}

/// The last `check` per App (in memory; status shows it).
fn last_checks() -> &'static Mutex<HashMap<String, (i64, J)>> {
    static M: OnceLock<Mutex<HashMap<String, (i64, J)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// GET `/cli/app/pairing?app=`.
fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = pairing_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !app_agent_at(who, &t, crate::apps::access::Level::Build) {
        return Err(no("forbidden", 403, "pairing status is the owner's, Admins' and this App's own agent's (\"Build and run Apps\" on)"));
    }
    status_json(&t)
}

pub(crate) fn status_json(t: &Target) -> R<J> {
    // A21: daemon-DB reads first.
    let all = super::pairing_hosts(&t.service_id);
    let served = super::front_rewrites(&t.service_id);
    let cfg = store::config(&t.service_id).map_err(store_err)?;
    let state = super::state(&t.service_id);
    let stay = app_login::settings(&t.service_id).map_err(store_err)?.stay_signed_in_days;
    let (live, last_day) = store::counts(&t.service_id, now() - 86_400).map_err(store_err)?;
    let on_hosts: Vec<String> = cfg.as_ref().map(|c| super::served_hosts(c, &all)).unwrap_or_default();
    let hosts: Vec<J> = all.iter().map(|h| json!({ "host": h, "pairs": on_hosts.contains(h) && state.is_ok() && served.is_ok() })).collect();
    let last = last_checks().lock().unwrap_or_else(|e| e.into_inner()).get(&t.service_id).cloned();
    let mut out = json!({
        "ok": true,
        "app": t.service_id,
        "name": t.name,
        "state": if state.is_ok() && served.is_ok() { "on" } else { "off" },
        "off": state.as_ref().err().map(|o| o.as_str()),
        "served": served.is_ok(),
        "notServed": served.err().map(super::NotServed::hint),
        "staySignedInDays": stay,
        "hosts": hosts,
        "ios": cfg.as_ref().map(|c| json!(c.ios)).unwrap_or(json!([])),
        "android": cfg.as_ref().map(|c| json!(c.android)).unwrap_or(json!([])),
        "hostsChosen": cfg.as_ref().and_then(|c| c.hosts.clone()),
        "nativeOrigins": cfg.as_ref().map(|c| json!(c.native_origins)).unwrap_or(json!([])),
        "linkPaths": cfg.as_ref().map(|c| json!(c.link_paths)).unwrap_or(json!([])),
        "store": {
            "ios": cfg.as_ref().and_then(|c| c.store_ios.clone()),
            "android": cfg.as_ref().and_then(|c| c.store_android.clone()),
        },
        "updatedAt": cfg.as_ref().map(|c| c.updated_at),
        "updatedBy": cfg.as_ref().and_then(|c| c.updated_by.clone()),
        "pairedPhones": live,
        "pairingsLastDay": last_day,
        "outstanding": store::outstanding_for(&t.service_id),
    });
    if let Some((at, results)) = last {
        out["lastCheck"] = json!({ "at": at, "hosts": results });
    }
    Ok(out)
}

/// POST `/cli/app/pairing/set`.
fn set(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = pairing_target(who, s(&v, "app"))?;
    if !owner_in_person(who) {
        return Err(no(
            "owner_only",
            403,
            "which phone apps open an App's sign-in codes is the owner's choice, in person (k2 app pairing setup from a terminal outside K2). \
             An agent asks with a permission card.",
        ));
    }
    let input = store::parse_input(&v).map_err(|e| no("usage", 400, e))?;
    if let Err(why) = super::front_rewrites(&t.service_id) {
        return Err(no("pairing_needs_k2_front", 409, why.hint()));
    }
    let all = super::pairing_hosts(&t.service_id);
    if all.is_empty() {
        return Err(no(
            "no_public_host",
            409,
            "this App has no public k2.dev host to pair on (it needs K2 Connect and its own label: k2 app domain add, or a tunnelled deploy)",
        ));
    }
    if let Some(h) = input.hosts.as_ref().and_then(|hs| hs.iter().find(|h| !all.contains(h))) {
        return Err(Refusal {
            extra: Some(json!({ "hosts": all })),
            ..no(
                "usage",
                400,
                format!(
                    "hosts: {h} isn't one of this App's k2.dev hosts. v1 pairs on the App's own labels; a custom domain works when you front it to the label yourself"
                ),
            )
        });
    }
    fresh_confirm(
        who,
        &t,
        &v,
        super::CONFIRM_ACTION,
        "the phone apps that open this App's sign-in codes need a fresh owner confirm (Touch ID, your login or vault password); \
         k2 app pairing setup asks for it from a terminal outside K2",
        "confirm again: a confirm works once, for this App's phone apps, within 60 seconds",
    )?;
    let by = actor(who);
    let before = store::config(&t.service_id).map_err(store_err)?;
    let cfg = store::set_config(&t.service_id, &input, &by).map_err(|e| no("usage", 400, e))?;
    super::refresh_links(&t.service_id);
    let ids: Vec<String> = cfg.ios.iter().cloned().chain(cfg.android.iter().map(|a| a.package.clone())).collect();
    super::audit("app-pairing-set", &by, "ok", "-", "cli", &format!("app={} apps={}", t.service_id, ids.join(",")));
    crate::apps::alerts::note(
        &t.project_id,
        "Phone pairing set up",
        &format!("The owner set the phone apps for {} ({by}): {}. People can now pair their phone from the web App.", t.name, ids.join(", ")),
    );
    let mut out = status_json(&t)?;
    let served: Vec<String> = super::served_hosts(&cfg, &all);
    let mut warnings = vec![format!(
        "while pairing is on, K2 itself answers /.well-known/apple-app-site-association and /.well-known/assetlinks.json on {}: \
         the App's own files there are replaced; list its other universal-link paths with --link-path",
        served.join(", ")
    )];
    if before.as_ref().is_some_and(|b| b.ios != cfg.ios || b.android != cfg.android) {
        warnings.push(
            "phones that already checked the old app ids may keep opening the old app for up to about a week (Apple's CDN and Android re-check on their own schedule)"
                .into(),
        );
    }
    warnings.push("list each host exactly in your app (applinks:<host>), never a wildcard: a wildcard makes iOS read k2.dev's own file".into());
    out["warnings"] = json!(warnings);
    Ok(out)
}


/// POST `/cli/app/pairing/off {app}`.
fn off(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = pairing_target(who, s(&v, "app"))?;
    if !admin_human(who) && !app_agent_at(who, &t, crate::apps::access::Level::Build) {
        return Err(no("forbidden", 403, "turning phone pairing off is for the owner, Admins and this App's own agent"));
    }
    let by = actor(who);
    let was = store::set_off(&t.service_id, &by).map_err(store_err)?;
    super::refresh_links(&t.service_id);
    if was {
        super::audit("app-pairing-off", &by, "ok", "-", "cli", &format!("app={}", t.service_id));
        crate::apps::alerts::note(
            &t.project_id,
            "Phone pairing turned off",
            &format!("Phone pairing for {} is off (by {by}). Phones already paired stay signed in; the owner ends them in People.", t.name),
        );
    }
    let mut out = status_json(&t)?;
    out["wasOn"] = json!(was);
    Ok(out)
}

/// The fetcher `check` uses (tests swap in a fake; never the internet).
fn fetcher() -> &'static Mutex<Option<Box<dyn super::links::Fetcher + Send>>> {
    static F: OnceLock<Mutex<Option<Box<dyn super::links::Fetcher + Send>>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(None))
}

/// Test seam: use `f` for `check` instead of the internet.
#[cfg(test)]
pub(crate) fn set_fetcher_for_tests(f: Option<Box<dyn super::links::Fetcher + Send>>) {
    *fetcher().lock().unwrap_or_else(|e| e.into_inner()) = f;
}

/// POST `/cli/app/pairing/check {app}`: the owner only (it calls the
/// internet). Never run by tests against the real internet.
fn check(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let t = pairing_target(who, s(&v, "app"))?;
    if !owner_in_person(who) && !matches!(who, Who::Shell { .. }) {
        return Err(no("owner_only", 403, "checking an App's phone link files fetches them over the internet: the owner runs k2 app pairing check"));
    }
    if crate::push_routes::airgap_blocks() {
        return Err(no("airgap", 503, "air-gap is on: nothing leaves this server"));
    }
    let Some(app_id) = t.app_id.clone() else { return Err(no("pairing_needs_k2_front", 409, super::NotServed::NotRuntime.hint())) };
    super::refresh_links(&t.service_id);
    let Some(set) = super::links::get(&app_id) else {
        return Err(no("pairing_off", 409, "this App's pairing is off, so K2 serves no link files to check (k2 app pairing setup)"));
    };
    // What K2 serves now, with the App's link-file setting merged in (0.46.7).
    let files = super::links::get_files(&app_id);
    let results = {
        let mut f = fetcher().lock().unwrap_or_else(|e| e.into_inner());
        match f.as_mut() {
            Some(fake) => super::links::check_merged(fake.as_ref(), Some(&set), files.as_deref()),
            None => super::links::check_merged(&super::links::NetFetcher, Some(&set), files.as_deref()),
        }
    };
    let hosts: Vec<J> = results
        .iter()
        .map(|h| json!({ "host": h.host, "aasa": h.aasa, "assetlinks": h.assetlinks, "appleCdn": h.apple_cdn }))
        .collect();
    last_checks().lock().unwrap_or_else(|e| e.into_inner()).insert(t.service_id.clone(), (now(), json!(hosts)));
    super::audit("app-pairing-check", &actor(who), "ok", "-", "cli", &format!("app={}", t.service_id));
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "hosts": hosts }))
}

/// May the App's agent read the pairing log? Its App-user switch (R5,
/// V23): the log names people and their phones.
fn agent_reads_log(who: &Who, t: &Target) -> bool {
    match who {
        Who::Agent { project_id, .. } if *project_id == t.project_id => crate::workspace_msg::resolve_workspace(project_id)
            .is_some_and(|p| crate::skin_routes::agent_may_manage_app_users(&p)),
        _ => false,
    }
}

/// GET `/cli/app/pairing/log?app=`: the App's paired phones, newest
/// first: when, person, device name, host, live or why it ended. Never a
/// code, a pass or a token.
fn log(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let t = pairing_target(who, params.get("app").map(String::as_str))?;
    if !admin_human(who) && !agent_reads_log(who, &t) {
        return Err(no(
            "forbidden",
            403,
            "the pairing log names people and their phones: the owner's, Admins', and this App's own agent's when it may manage the App's users",
        ));
    }
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(100).clamp(1, 500);
    let rows = store::pairings_for_app(&t.service_id, limit).map_err(store_err)?;
    Ok(json!({ "ok": true, "app": t.service_id, "name": t.name, "pairings": rows }))
}

pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if is_post_route(path) {
        if let Err(e) = crate::apps::access::member_read_only(&who) {
            return respond(Err(e));
        }
    }
    let r = match path {
        STATUS => status(&who, params),
        SET => set(&who, body),
        OFF => off(&who, body),
        CHECK => check(&who, body),
        LOG => log(&who, params),
        _ => Err(no("not_found", 404, "no such App pairing route")),
    };
    respond(r)
}

#[cfg(test)]
#[path = "owner_routes_tests.rs"]
mod tests;
