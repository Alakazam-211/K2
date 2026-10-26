//! Phone pairing for custom Apps (`prd-app-phone-pairing-v1` + vs-live,
//! 0.46.5).
//!
//! A person signed in to a custom App on the web presses "Connect your
//! phone"; the web App asks K2 for a one-time code and shows it as a QR;
//! the phone's camera opens the App's own phone app (iOS universal link /
//! Android App Link on the App's own host); the phone app redeems the code
//! for a device sign-in of that App and that person, and can register for
//! the App's push in the same call. No password on the phone.
//!
//! - [`guest_routes`]: `pairing.start|status|cancel` (a guest's own fresh
//!   web session pass) and `pairing.check|redeem` (Public: the code is the
//!   credential).
//! - [`owner_routes`]: `/cli/app/pairing*` (`k2 app pairing …`): the
//!   owner's phone-app ids (Owner in person + a fresh confirm), off (Admin
//!   or the App's agent), status, the public check, the log.
//! - [`links`]: the iOS / Android link files and the landing page, served
//!   by the daemon on the App's own k2.dev labels while its pairing is on.
//! - Storage: `k2_core::skin::app_pairing` (codes in memory, config and
//!   pairing rows in skin.db).
//!
//! **State.** An App's pairing is on once the owner saved at least one iOS
//! app id or Android package with a fresh confirm, and nobody turned it
//! off. `K2_APP_PAIRING=off` in the daemon's environment turns every App
//! off: start, check, redeem and the link files (Rosson's switch rule).
//! v1 pairs on the App's own k2.dev labels only (R1): a custom domain works
//! only when its owner fronts it to the label.

pub mod guest_routes;
pub mod links;
pub mod owner_routes;

use k2_core::skin::{self, app_pairing as store};

/// The vault confirm action for setting an App's phone apps (`secretId` =
/// the App's service id).
pub const CONFIRM_ACTION: &str = "app-pairing-apps";

/// The box-wide switch: `off` (or `0`/`false`/`no`) stops every App.
pub const SWITCH_ENV: &str = "K2_APP_PAIRING";

/// The `/boot-status` feature key.
pub const FEATURE: &str = "app-phone-pairing-v1";

/// Is App pairing allowed on this box at all (default yes)?
pub fn box_enabled() -> bool {
    !crate::app_push::switch_off(std::env::var(SWITCH_ENV).ok().as_deref())
}

/// The text the person approves on the confirm sheet, built here from the
/// App's real name and service id, never from a client's label (the push
/// pattern, review A-L1). `None` = not a pairing action.
pub fn confirm_reason(action: &str, service_id: Option<&str>) -> Option<Result<String, &'static str>> {
    if action != CONFIRM_ACTION {
        return None;
    }
    let Some(id) = service_id.map(str::trim).filter(|s| !s.is_empty()) else { return Some(Err("secretId (the App's service id) is required")) };
    let Some(row) = published(id) else { return Some(Err("no published App by that service id")) };
    let name: String = row.name.chars().filter(|c| !c.is_control()).take(64).collect();
    Some(Ok(format!("choose which phone apps open sign-in codes for App {name} ({})", row.id)))
}

/// Why an App's pairing is off (`Ok` = on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Off {
    /// `host` (the box login) never pairs.
    Host,
    /// No published App by that id.
    NoApp,
    /// `K2_APP_PAIRING=off`.
    Box,
    /// The owner never set phone apps.
    NotSetUp,
    /// The owner (an Admin, the App's agent) turned it off.
    Owner,
}

impl Off {
    pub fn as_str(self) -> &'static str {
        match self {
            Off::Host => "host",
            Off::NoApp => "no_app",
            Off::Box => "box_off",
            Off::NotSetUp => "not_set_up",
            Off::Owner => "owner_off",
        }
    }
}

/// The published service row of `app` (a service id), if any.
pub(crate) fn published(app: &str) -> Option<k2_core::published_services::PublishedService> {
    crate::app_push::published(app)
}

/// The runtime App behind a service id, if it is one.
pub(crate) fn runtime_app(service_id: &str) -> Option<k2_core::deploy::store::AppRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::deploy::store::get_app_by_service(&conn, service_id).ok().flatten()
}

/// An App's pairing state and config (`Ok` = on).
pub fn state(app: &str) -> Result<store::Config, Off> {
    let app = app.trim();
    if app.is_empty() || app == skin::HOST_APP_ID {
        return Err(Off::Host);
    }
    if !box_enabled() {
        return Err(Off::Box);
    }
    if published(app).is_none() {
        return Err(Off::NoApp);
    }
    match store::config(app).map_err(|_| Off::NoApp)? {
        None => Err(Off::NotSetUp),
        Some(c) if !c.enabled => Err(Off::Owner),
        Some(c) if !c.is_on() => Err(Off::NotSetUp),
        Some(c) => Ok(c),
    }
}

/// The k2.dev hosts an App answers on (V2: its own label, its taken and
/// attached labels; the supervisor's union), lowercase, no scheme. R1:
/// v1 pairs on these only, never a custom domain.
pub fn pairing_hosts(service_id: &str) -> Vec<String> {
    let app = runtime_app(service_id);
    let name = match &app {
        Some(a) => a.app_id.clone(),
        None => match published(service_id) {
            Some(r) => r.name,
            None => return Vec::new(),
        },
    };
    let mut origins = crate::apps::origins::for_service(service_id, &name);
    if let Some(a) = &app {
        origins = crate::apps::origins::with_taken_labels(origins, &crate::apps::labels::taken(&a.app_id));
    }
    let sub = crate::apps::origins::box_subdomain();
    if sub.is_empty() {
        return Vec::new();
    }
    let suffix = format!(".{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST);
    let mut out: Vec<String> = Vec::new();
    for o in origins {
        if let Some(h) = o.strip_prefix("https://").map(|h| h.trim_end_matches('/').to_ascii_lowercase()) {
            if h.ends_with(&suffix) && !out.contains(&h) {
                out.push(h);
            }
        }
    }
    out
}

/// The hosts pairing serves on for `cfg`: its `hosts` (when set) that are
/// still the App's, else every pairing host.
pub fn served_hosts(cfg: &store::Config, all: &[String]) -> Vec<String> {
    match &cfg.hosts {
        Some(h) => h.iter().filter(|x| all.contains(x)).cloned().collect(),
        None => all.to_vec(),
    }
}

/// The default host a QR names (V1): the App's own label, else the first
/// served host.
pub fn default_host(service_id: &str, served: &[String]) -> Option<String> {
    let own = runtime_app(service_id).map(|a| {
        format!("{}.{}.{}", a.app_id, crate::apps::origins::box_subdomain(), k2_core::tunnel::config::SUBDOMAIN_HOST).to_ascii_lowercase()
    });
    match own {
        Some(o) if served.contains(&o) => Some(o),
        _ => served.first().cloned(),
    }
}

/// Why K2 can't serve an App's link files (V6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotServed {
    /// A `--cmd` or `--skin` service, not a runtime App.
    NotRuntime,
    /// A runtime App whose front is a raw splice (`host.clientIp: false`).
    Raw,
}

impl NotServed {
    pub fn hint(self) -> &'static str {
        match self {
            NotServed::NotRuntime => {
                "pairing needs a runtime App (k2 app deploy): K2 serves the phone link files on its own front, which a --cmd or --skin service doesn't pass through"
            }
            NotServed::Raw => {
                "this App's manifest says host.clientIp: false (a raw splice), so K2 can't answer on its host; turn the client-IP front back on to pair phones"
            }
        }
    }
}

/// Does K2 serve link files for this service now? `Ok(runtime App id)`.
pub fn front_rewrites(service_id: &str) -> Result<String, NotServed> {
    let Some(app) = runtime_app(service_id) else { return Err(NotServed::NotRuntime) };
    let rewrites = match crate::apps::router::live(&app.app_id) {
        Some(be) => be.client_ip,
        None => manifest_wants_client_ip(&app),
    };
    if rewrites || k2_core::tunnel::client_ip_labels::enabled(&app.app_id) {
        Ok(app.app_id)
    } else {
        Err(NotServed::Raw)
    }
}

fn manifest_wants_client_ip(app: &k2_core::deploy::store::AppRow) -> bool {
    let Some(seed) = app.live_seed.as_deref() else { return true };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::deploy::store::get_seed(&conn, seed) {
        Ok(Some(row)) => serde_json::from_value::<k2_core::deploy::manifest::Manifest>(row.manifest).map(|m| m.wants_client_ip()).unwrap_or(true),
        _ => true,
    }
}

fn last_served() -> &'static std::sync::Mutex<std::collections::HashMap<String, bool>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Rebuild one App's link-file snapshot from its config and hosts now (on
/// set, off, boot and each deploy). When an App that served its files no
/// longer can (a deploy turned its client-IP front off, V6), its
/// workspace gets an inbox note.
pub fn refresh_links(service_id: &str) {
    let Some(app) = runtime_app(service_id) else { return };
    // Most Apps never pair: one skin.db read and nothing else (each read
    // stands alone, never one store's lock inside the other's: A21).
    if links::get(&app.app_id).is_none() && store::config(service_id).ok().flatten().is_none() {
        return;
    }
    let hosts = pairing_hosts(service_id);
    let name = published(service_id).map(|r| r.name).unwrap_or_else(|| app.app_id.clone());
    let served = front_rewrites(service_id);
    let cfg = store::config(service_id).ok().flatten().filter(|c| c.is_on());
    let on = cfg.is_some() && box_enabled();
    match (cfg, &served) {
        (Some(c), Ok(_)) => {
            let set = links::LinkSet {
                service_id: service_id.to_string(),
                app_name: name.clone(),
                ios: c.ios.clone(),
                android: c.android.clone(),
                link_paths: c.link_paths.clone(),
                hosts: served_hosts(&c, &hosts),
                store_ios: c.store_ios.clone(),
                store_android: c.store_android.clone(),
            };
            links::set(&app.app_id, Some(set));
        }
        _ => links::set(&app.app_id, None),
    }
    let now_served = on && served.is_ok();
    let before = last_served().lock().unwrap_or_else(|e| e.into_inner()).insert(service_id.to_string(), now_served);
    if before == Some(true) && on && !now_served {
        let why = served.err().map(NotServed::hint).unwrap_or("");
        crate::apps::alerts::note(
            &app.owner_workspace,
            "Phone pairing stopped",
            &format!("K2 no longer serves {name}'s phone link files, so phones can't pair: {why}. Starts answer pairing_not_served until it's fixed."),
        );
    }
}

/// Rebuild every App's snapshot (boot).
pub fn refresh_all_links() {
    match store::all_configs() {
        Ok(all) => {
            for c in all {
                refresh_links(&c.app);
            }
        }
        Err(e) => k2_core::log_debug!("[app-pairing] WARN boot snapshot: {e}"),
    }
}

/// An App went away (`publish rm`, `k2 app uninstall`): its config and
/// codes go, and its link files stop.
pub fn forget_app(service_id: &str, app_id: Option<&str>) {
    if let Some(a) = app_id {
        links::set(a, None);
    }
    match store::purge_app(service_id) {
        Ok(n) if n > 0 => k2_core::log_debug!("[app-pairing] {service_id} removed: its pairing config went"),
        Ok(_) => {}
        Err(e) => k2_core::log_debug!("[app-pairing] {service_id} removed: WARN {e}"),
    }
    last_served().lock().unwrap_or_else(|e| e.into_inner()).remove(service_id);
}

/// One Access Audit line (`auth-audit.jsonl`). Never a code, a pass, a
/// refresh token or a push token: callers pass the pairingId.
pub(crate) fn audit(event: &str, actor: &str, outcome: &str, ip: &str, door: &str, line: &str) {
    let ev = k2_core::auth_audit::AuditEvent::new(event, actor, format!("{outcome} {line}"), door, ip, "app-pairing");
    k2_core::auth_audit::record(&ev);
}

/// The JSON refusal shape every App pairing route uses.
pub(crate) fn refusal(status: &'static str, code: &str, hint: &str) -> crate::cli_response::CliResponse {
    crate::app_push::refusal(status, code, hint)
}

/// [`refusal`] with extra fields inside `error`.
pub(crate) fn refusal_with(
    status: &'static str,
    code: &str,
    hint: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> crate::cli_response::CliResponse {
    crate::app_push::refusal_with(status, code, hint, extra)
}

/// The refusal for a state that is off.
pub(crate) fn off_response(off: Off) -> crate::cli_response::CliResponse {
    match off {
        Off::Host => refusal("400 Bad Request", "not_for_host", "pairing is for a published App's own users, never the box login (host)"),
        Off::NoApp => refusal("401 Unauthorized", "pass_invalid", "this App is gone: the pass no longer works"),
        Off::Box => {
            let mut extra = serde_json::Map::new();
            extra.insert("reason".into(), serde_json::json!("box_off"));
            refusal_with("403 Forbidden", "pairing_off", "phone pairing is off on this server (K2_APP_PAIRING=off)", extra)
        }
        Off::NotSetUp | Off::Owner => {
            refusal("403 Forbidden", "pairing_off", "this App doesn't pair phones (its owner turns it on: k2 app pairing setup)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_never_pairs_and_the_reason_strings_hold() {
        assert_eq!(state(skin::HOST_APP_ID).err(), Some(Off::Host));
        assert_eq!(state("  ").err(), Some(Off::Host));
        assert_eq!(Off::Box.as_str(), "box_off");
        assert!(confirm_reason("app-push-on", Some("x")).is_none());
        assert_eq!(confirm_reason(CONFIRM_ACTION, None), Some(Err("secretId (the App's service id) is required")));
    }

    #[test]
    fn served_hosts_keep_only_the_apps_own() {
        let all = vec!["jobs.box.k2.dev".to_string(), "careers.box.k2.dev".to_string()];
        let mut c = store::Config {
            app: "svc".into(),
            enabled: true,
            ios: vec!["ABCDE12345.com.you.app".into()],
            android: vec![],
            hosts: None,
            native_origins: vec![],
            link_paths: vec![],
            store_ios: None,
            store_android: None,
            updated_at: 0,
            updated_by: None,
        };
        assert_eq!(served_hosts(&c, &all), all);
        c.hosts = Some(vec!["careers.box.k2.dev".into(), "gone.box.k2.dev".into()]);
        assert_eq!(served_hosts(&c, &all), vec!["careers.box.k2.dev".to_string()]);
    }
}
