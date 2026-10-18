//! `K2_APP_ORIGIN` / `K2_APP_ORIGINS` (0.46.1): every App process learns
//! the public https origins it is served on, so it can check `Origin` on
//! CSRF-sensitive requests and WebSocket upgrades.
//!
//! - **Sources** (the daemon's own bindings, read at each start):
//!   the bound custom hostnames (`k2 domain name add <host> --role
//!   app:<app>`, on an attached apex) and the nested label
//!   `https://<app>.<sub>.k2.dev` when the App is tunnelled and the box has
//!   a K2 Connect subdomain.
//! - `K2_APP_ORIGIN` = the primary origin: the first bound custom domain
//!   (sorted), else the nested label. `K2_APP_ORIGINS` = every origin,
//!   comma-separated, primary first.
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

/// Pure: the origins in order (custom domains sorted first, then the
/// k2.dev labels in the order given: `meet` if bound, the App's own),
/// deduplicated, each `https://<host>` (lowercase, no trailing dot, no
/// path).
pub fn ordered(labels: Vec<String>, custom: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = custom
        .iter()
        .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    hosts.sort();
    let mut out: Vec<String> = Vec::new();
    for h in hosts {
        let o = format!("https://{h}");
        if !out.contains(&o) {
            out.push(o);
        }
    }
    for n in labels.into_iter().map(|n| n.trim().trim_end_matches('/').to_ascii_lowercase()).filter(|n| n.starts_with("https://")) {
        if !out.contains(&n) {
            out.push(n);
        }
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
fn box_subdomain() -> String {
    k2_core::tunnel::config::load()
        .ok()
        .map(|c| c.subdomain.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| k2_core::tunnel::subdomains::current().primary.trim().to_ascii_lowercase())
}

/// The public origins of the published service `service_id` named `name`
/// (an App's name is its id), read from the daemon's bindings now.
pub fn for_service(service_id: &str, name: &str) -> Vec<String> {
    for_service_on(service_id, name, &box_subdomain(), None)
}

fn for_service_on(service_id: &str, name: &str, sub: &str, meet: Option<(String, Option<String>)>) -> Vec<String> {
    let db = k2_core::db::shared();
    let (expose, custom) = {
        let conn = db.lock();
        let expose = k2_core::published_services::get_by_id(&conn, service_id)
            .ok()
            .flatten()
            .map(|r| r.expose)
            .unwrap_or_default();
        let custom = if name.is_empty() {
            Vec::new()
        } else {
            k2_core::domains::hostnames_for_app(&conn, name).unwrap_or_else(|e| {
                k2_core::log_debug!("[apps] origins for {name}: {e}");
                Vec::new()
            })
        };
        (expose, custom)
    };
    let mut labels: Vec<String> = Vec::new();
    if let Some((app, Some(url))) = meet {
        if !name.is_empty() && app == name {
            labels.push(url);
        }
    }
    labels.extend(nested_origin(name, &expose, sub));
    ordered(labels, &custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_label_only() {
        let o = ordered(nested_origin("hearth", "tunnel", "sewlab").into_iter().collect(), &[]);
        assert_eq!(o, vec!["https://hearth.sewlab.k2.dev".to_string()]);
        let env = env_pairs(&o);
        assert_eq!(
            env,
            vec![(ORIGIN_ENV, "https://hearth.sewlab.k2.dev".to_string()), (ORIGINS_ENV, "https://hearth.sewlab.k2.dev".to_string())]
        );
    }

    #[test]
    fn a_custom_domain_is_primary_and_the_label_stays_listed() {
        let o = ordered(
            nested_origin("hearth", "tunnel", "sewlab").into_iter().collect(),
            &["Hearth.Example.com.".to_string(), "chat.example.com".to_string(), "hearth.example.com".to_string()],
        );
        assert_eq!(
            o,
            vec![
                "https://chat.example.com".to_string(),
                "https://hearth.example.com".to_string(),
                "https://hearth.sewlab.k2.dev".to_string(),
            ]
        );
        let env = env_pairs(&o);
        assert_eq!(env[0], (ORIGIN_ENV, "https://chat.example.com".to_string()));
        assert_eq!(env[1], (ORIGINS_ENV, "https://chat.example.com,https://hearth.example.com,https://hearth.sewlab.k2.dev".to_string()));
        // A local-only App with a custom domain: only the custom origin.
        let local = ordered(nested_origin("hearth", "local", "sewlab").into_iter().collect(), &["hearth.example.com".to_string()]);
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
    }

    #[test]
    fn no_public_url_sets_nothing() {
        assert_eq!(nested_origin("hearth", "local", "sewlab"), None, "local-only");
        assert_eq!(nested_origin("hearth", "tunnel", ""), None, "no Connect subdomain");
        let o = ordered(nested_origin("hearth", "tunnel", "").into_iter().collect(), &[]);
        assert!(o.is_empty());
        assert!(env_pairs(&o).is_empty(), "unset, not empty strings");
    }
}
