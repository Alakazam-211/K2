//! `K2_APP_ORIGIN` / `K2_APP_ORIGINS` (0.46.1): every App process learns
//! the public https origins it is served on, so it can check `Origin` on
//! CSRF-sensitive requests and WebSocket upgrades.
//!
//! - **Sources** (the daemon's own bindings, read at each start):
//!   the custom hostnames bound to the App (`k2 app domain add <app>
//!   <host>`, role `app:<app>`, on an attached apex) **that are `live`**,
//!   i.e. actually served (0.46.6, vs-live CD-A4: a bound name nothing
//!   serves yet is no origin), the reserved `meet` label when it is
//!   bound to the App (`k2 hostchat meet bind <app>`, K-M2), and the nested
//!   label `https://<app>.<sub>.k2.dev` when the App is tunnelled and the
//!   box has a K2 Connect subdomain.
//! - `K2_APP_ORIGIN` = the primary origin: the custom name the owner or the
//!   App's agent marked primary (`k2 app domain primary <app> <host>`)
//!   once it is live, else the App's K2 label, else (a local-only App) the
//!   first live custom name. Never just the alphabetically first bound
//!   name. `K2_APP_ORIGINS` = every origin, comma-separated, primary first.
//! - No public URL (local-only App, no Connect subdomain, no custom
//!   domain): **neither variable is set**. An App must then treat a missing
//!   value as "no public origin" (loopback only), never as "allow any".
//! - The spec is rebuilt on every deploy and restart (crash restarts and
//!   `k2 app restart` included), so a changed binding reaches the App on
//!   its next start; a running process keeps the values it started with.

/// The primary public origin.
pub const ORIGIN_ENV: &str = "K2_APP_ORIGIN";
/// Every public origin, comma-separated, primary first.
pub const ORIGINS_ENV: &str = "K2_APP_ORIGINS";

/// Pure: the origins in order, deduplicated, each `https://<host>`
/// (lowercase, no trailing dot, no path):
/// 1. `primary`, when it is one of the live `custom` names;
/// 2. the k2.dev labels in the order given (`meet` if bound, the App's own);
/// 3. the other live custom names, sorted.
///
/// `custom` must already be the LIVE names only (the caller filters).
pub fn ordered(labels: Vec<String>, custom: &[String], primary: Option<&str>) -> Vec<String> {
    let norm = |h: &str| h.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut hosts: Vec<String> = custom.iter().map(|h| norm(h)).filter(|h| !h.is_empty()).collect();
    hosts.sort();
    let mut out: Vec<String> = Vec::new();
    let push = |o: String, out: &mut Vec<String>| {
        if !out.contains(&o) {
            out.push(o);
        }
    };
    if let Some(p) = primary.map(norm).filter(|p| hosts.contains(p)) {
        push(format!("https://{p}"), &mut out);
    }
    for n in labels.into_iter().map(|n| n.trim().trim_end_matches('/').to_ascii_lowercase()).filter(|n| n.starts_with("https://")) {
        push(n, &mut out);
    }
    for h in hosts {
        push(format!("https://{h}"), &mut out);
    }
    out
}

/// The two variables for `origins` (empty = none: nothing is set).
pub fn env_pairs(origins: &[String]) -> Vec<(&'static str, String)> {
    match origins.first() {
        Some(primary) => vec![(ORIGIN_ENV, primary.clone()), (ORIGINS_ENV, origins.join(","))],
        None => Vec::new(),
    }
}

/// `https://<name>.<sub>.k2.dev` when `expose` is the tunnel and the box
/// has a subdomain (`sub` empty = none).
pub fn nested_origin(name: &str, expose: &str, sub: &str) -> Option<String> {
    let name = name.trim();
    let sub = sub.trim();
    if expose != k2_core::published_services::EXPOSE_TUNNEL || name.is_empty() || sub.is_empty() {
        return None;
    }
    Some(format!("https://{name}.{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST))
}

/// The box's K2 Connect subdomain (`""` = none).
/// The https origins of labels an App took over from its workspace's
/// legacy services (`k2 app label take`, 0.46.3), after its own.
pub fn with_taken_labels(mut origins: Vec<String>, labels: &[String]) -> Vec<String> {
    let sub = box_subdomain();
    for l in labels {
        if let Some(o) = nested_origin(l, k2_core::published_services::EXPOSE_TUNNEL, &sub) {
            if !origins.contains(&o) {
                origins.push(o);
            }
        }
    }
    origins
}

pub(crate) fn box_subdomain() -> String {
    k2_core::tunnel::config::load()
        .ok()
        .map(|c| c.subdomain.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| k2_core::tunnel::subdomains::current().primary.trim().to_ascii_lowercase())
}

/// The public origins of the published service `service_id` named `name`
/// (an App's name is its id), read from the daemon's bindings now.
pub fn for_service(service_id: &str, name: &str) -> Vec<String> {
    for_service_on(service_id, name, &box_subdomain(), crate::sew::meet::bound_live())
}

fn for_service_on(service_id: &str, name: &str, sub: &str, meet: Option<(String, Option<String>)>) -> Vec<String> {
    let db = k2_core::db::shared();
    let (expose, custom, primary) = {
        let conn = db.lock();
        let expose = k2_core::published_services::get_by_id(&conn, service_id)
            .ok()
            .flatten()
            .map(|r| r.expose)
            .unwrap_or_default();
        // Live names only (CD-A4): a bound name that nothing serves yet is
        // not an origin, so invite pages and the gateway's form check never
        // trust it either.
        let (custom, primary) = if name.is_empty() {
            (Vec::new(), None)
        } else {
            let live = k2_core::domains::live_hostnames_for_app(&conn, name).unwrap_or_else(|e| {
                k2_core::log_debug!("[apps] origins for {name}: {e}");
                Vec::new()
            });
            (live, k2_core::domains::primary_hostname_for_app(&conn, name).ok().flatten())
        };
        (expose, custom, primary)
    };
    let mut labels: Vec<String> = Vec::new();
    if let Some((app, Some(url))) = meet {
        if !name.is_empty() && app == name {
            labels.push(url);
        }
    }
    labels.extend(nested_origin(name, &expose, sub));
    ordered(labels, &custom, primary.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_label_only() {
        let o = ordered(nested_origin("hearth", "tunnel", "sewlab").into_iter().collect(), &[], None);
        assert_eq!(o, vec!["https://hearth.sewlab.k2.dev".to_string()]);
        let env = env_pairs(&o);
        assert_eq!(
            env,
            vec![(ORIGIN_ENV, "https://hearth.sewlab.k2.dev".to_string()), (ORIGINS_ENV, "https://hearth.sewlab.k2.dev".to_string())]
        );
    }

    /// CD-A4: the primary is the name the owner picked (once live), never
    /// the alphabetically first; without a pick the App's K2 label leads.
    #[test]
    fn the_picked_custom_domain_is_primary_and_the_label_stays_listed() {
        let live = ["Hearth.Example.com.".to_string(), "chat.example.com".to_string(), "hearth.example.com".to_string()];
        let label = || nested_origin("hearth", "tunnel", "sewlab").into_iter().collect::<Vec<_>>();
        let o = ordered(label(), &live, Some("hearth.example.com"));
        assert_eq!(
            o,
            vec![
                "https://hearth.example.com".to_string(),
                "https://hearth.sewlab.k2.dev".to_string(),
                "https://chat.example.com".to_string(),
            ]
        );
        let env = env_pairs(&o);
        assert_eq!(env[0], (ORIGIN_ENV, "https://hearth.example.com".to_string()));
        assert_eq!(env[1], (ORIGINS_ENV, "https://hearth.example.com,https://hearth.sewlab.k2.dev,https://chat.example.com".to_string()));
        // No pick: the K2 label is primary, NOT `chat.` (first by name).
        let o = ordered(label(), &live, None);
        assert_eq!(o[0], "https://hearth.sewlab.k2.dev");
        // A pick that is not among the live names is ignored.
        let o = ordered(label(), &["chat.example.com".to_string()], Some("hearth.example.com"));
        assert_eq!(o, vec!["https://hearth.sewlab.k2.dev".to_string(), "https://chat.example.com".to_string()]);
        // A local-only App with a live custom domain: only the custom origin.
        let local = ordered(nested_origin("hearth", "local", "sewlab").into_iter().collect(), &["hearth.example.com".to_string()], None);
        assert_eq!(local, vec!["https://hearth.example.com".to_string()]);
    }

    /// Read from the daemon's bindings: the service row's expose and the
    /// `app:<name>` hostnames on attached apexes. A binding change shows
    /// on the next read (the next start).
    #[test]
    fn for_service_reads_the_publish_and_domain_bindings() {
        let _db = k2_core::db::scoped_for_test();
        let (tunnel_id, local_id) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let t = k2_core::published_services::insert(&conn, "ws-o", "hearth", "app", "/x", 0, "tunnel", "running", "cmd", "").unwrap();
            let l = k2_core::published_services::insert(&conn, "ws-o", "quiet", "app", "/x", 0, "local", "running", "cmd", "").unwrap();
            (t.id, l.id)
        };
        assert_eq!(for_service_on(&tunnel_id, "hearth", "sewlab", None), vec!["https://hearth.sewlab.k2.dev".to_string()]);
        assert!(for_service_on(&tunnel_id, "hearth", "", None).is_empty(), "no Connect subdomain");
        assert!(for_service_on(&local_id, "quiet", "sewlab", None).is_empty(), "local-only, no custom domain");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::upsert_binding(&conn, "example.com", None, false).unwrap();
            k2_core::domains::upsert_name(&conn, "hearth.example.com", "example.com", "app:hearth").unwrap();
            k2_core::domains::upsert_name(&conn, "aaa.example.com", "example.com", "app:hearth").unwrap();
        }
        // CD-A4: bound but not served yet → not an origin at all.
        assert_eq!(for_service_on(&tunnel_id, "hearth", "sewlab", None), vec!["https://hearth.sewlab.k2.dev".to_string()]);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::set_app_state(&conn, "hearth.example.com", k2_core::domains::STATE_LIVE, None).unwrap();
        }
        // Live: an origin, after the K2 label until it is picked as primary.
        assert_eq!(
            for_service_on(&tunnel_id, "hearth", "sewlab", None),
            vec!["https://hearth.sewlab.k2.dev".to_string(), "https://hearth.example.com".to_string()]
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            assert!(k2_core::domains::set_primary(&conn, "hearth", "hearth.example.com").unwrap());
        }
        assert_eq!(
            for_service_on(&tunnel_id, "hearth", "sewlab", None),
            vec!["https://hearth.example.com".to_string(), "https://hearth.sewlab.k2.dev".to_string()]
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::remove_name(&conn, "hearth.example.com").unwrap();
        }
        assert_eq!(for_service_on(&tunnel_id, "hearth", "sewlab", None), vec!["https://hearth.sewlab.k2.dev".to_string()]);
        // The `meet` label bound to this App (K-M2) comes before its own;
        // bound to another App, it isn't this one's.
        let meet = |app: &str| Some((app.to_string(), Some("https://meet.sewlab.k2.dev".to_string())));
        assert_eq!(
            for_service_on(&tunnel_id, "hearth", "sewlab", meet("hearth")),
            vec!["https://meet.sewlab.k2.dev".to_string(), "https://hearth.sewlab.k2.dev".to_string()]
        );
        assert_eq!(for_service_on(&tunnel_id, "hearth", "sewlab", meet("other")), vec!["https://hearth.sewlab.k2.dev".to_string()]);
    }

    /// NSI (Vanilla, 2026-10-27): with a live custom primary, the App's
    /// k2.dev names (its own label and the labels it took or attached:
    /// agents.nsi, agent.nsi, dannon.nsi, dannon1.nsi) all stay in
    /// K2_APP_ORIGINS. Live-only filtering applies to custom names only.
    #[test]
    fn k2_dev_labels_stay_origins_next_to_a_live_custom_primary() {
        let _db = k2_core::db::scoped_for_test();
        let _home = crate::test_support::TempHome::new();
        let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
        cfg.subdomain = "nsi".into();
        k2_core::tunnel::config::save(&cfg).unwrap();
        let svc = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let s = k2_core::published_services::insert(&conn, "ws-n", "agentnsi", "app", "/x", 0, "tunnel", "running", "cmd", "").unwrap();
            k2_core::domains::upsert_binding(&conn, "agentnsi.com", Some("z1"), true).unwrap();
            k2_core::domains::upsert_name(&conn, "nsi.agentnsi.com", "agentnsi.com", "app:agentnsi").unwrap();
            s.id
        };
        let taken = ["agents", "agent", "dannon", "dannon1"].map(String::from);
        // Not served yet: every k2.dev name is there, the custom one isn't.
        let o = with_taken_labels(for_service(&svc, "agentnsi"), &taken);
        assert_eq!(
            o,
            [
                "https://agentnsi.nsi.k2.dev",
                "https://agents.nsi.k2.dev",
                "https://agent.nsi.k2.dev",
                "https://dannon.nsi.k2.dev",
                "https://dannon1.nsi.k2.dev"
            ]
            .map(String::from)
            .to_vec()
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::domains::set_app_state(&conn, "nsi.agentnsi.com", k2_core::domains::STATE_LIVE, None).unwrap();
            k2_core::domains::set_primary(&conn, "agentnsi", "nsi.agentnsi.com").unwrap();
        }
        let o = with_taken_labels(for_service(&svc, "agentnsi"), &taken);
        assert_eq!(o[0], "https://nsi.agentnsi.com", "the live primary leads");
        for l in ["agentnsi", "agents", "agent", "dannon", "dannon1"] {
            assert!(o.contains(&format!("https://{l}.nsi.k2.dev")), "{l} dropped: {o:?}");
        }
        assert_eq!(o.len(), 6);
    }

    #[test]
    fn no_public_url_sets_nothing() {
        assert_eq!(nested_origin("hearth", "local", "sewlab"), None, "local-only");
        assert_eq!(nested_origin("hearth", "tunnel", ""), None, "no Connect subdomain");
        let o = ordered(nested_origin("hearth", "tunnel", "").into_iter().collect(), &[], None);
        assert!(o.is_empty());
        assert!(env_pairs(&o).is_empty(), "unset, not empty strings");
    }
}
