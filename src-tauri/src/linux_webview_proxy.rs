//! The Linux web view's network session and the person's proxy settings.
//!
//! Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
//! HTTPS to a remote K2 may use the person's proxy (a CONNECT tunnel hides
//! the request; a company proxy that intercepts TLS is the organisation's
//! own policy).
//!
//! WebKitGTK sends every request through the system proxy, loopback
//! included, unless the session says otherwise. So when the person has a
//! proxy, the web view's session gets custom settings that keep it: for
//! each scheme, the proxy GIO's own resolver picks for a remote host (the
//! environment's or GNOME's, whichever the system uses), the person's own
//! ignore hosts, and the loopback hosts added to them. With no proxy, the
//! session is left at its default. Connection only: no product logic.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use webkit2gtk::gio;
use webkit2gtk::gio::prelude::*;
use webkit2gtk::{WebContextExt, WebViewExt, WebsiteDataManagerExt};

/// Added to the person's ignore hosts: loopback never uses a proxy.
pub const LOOPBACK_IGNORE: [&str; 5] = ["localhost", "*.localhost", "127.0.0.0/8", "::1", "[::1]"];

/// The session's schemes and the scheme of the remote URL whose proxy each
/// one takes (a WebSocket goes where its http or https twin goes).
const SCHEMES: [(&str, &str); 4] = [("http", "http"), ("https", "https"), ("ws", "http"), ("wss", "https")];

/// A host no one owns: what the system's resolver would do for a remote
/// host, without a request.
const PROBE_HOST: &str = "k2-proxy-probe.invalid";

/// Custom proxy settings for the session: a proxy per scheme and the hosts
/// that never use one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub per_scheme: Vec<(String, String)>,
    pub ignore_hosts: Vec<String>,
}

/// The plan from `lookup` (the proxies a resolver gives for a URL, in
/// GIO's form: `direct://` or `http://host:port`), the person's own ignore
/// hosts and `loopback`. `None` when no scheme has a proxy, or the person's
/// ignore list holds `*`: then nothing changes.
pub fn plan(lookup: impl Fn(&str) -> Vec<String>, person_ignore: &[String], loopback: &[&str]) -> Option<Plan> {
    if person_ignore.iter().any(|h| h.trim() == "*") {
        return None;
    }
    let mut per_scheme = Vec::new();
    for (scheme, remote_scheme) in SCHEMES {
        let uri = format!("{remote_scheme}://{PROBE_HOST}/");
        if let Some(proxy) = lookup(&uri).into_iter().find(|p| !p.is_empty() && !p.starts_with("direct://")) {
            per_scheme.push((scheme.to_string(), proxy));
        }
    }
    if per_scheme.is_empty() {
        return None;
    }
    let mut ignore_hosts: Vec<String> = Vec::new();
    for host in person_ignore.iter().map(|h| h.trim().to_string()).chain(loopback.iter().map(|h| h.to_string())) {
        if !host.is_empty() && !ignore_hosts.contains(&host) {
            ignore_hosts.push(host);
        }
    }
    Some(Plan { per_scheme, ignore_hosts })
}

/// The person's own ignore hosts: `no_proxy` / `NO_PROXY`, and GNOME's
/// `ignore-hosts` when its proxy mode is not `none`.
pub fn person_ignore_hosts() -> Vec<String> {
    let mut out = Vec::new();
    for var in ["no_proxy", "NO_PROXY"] {
        if let Ok(v) = std::env::var(var) {
            out.extend(v.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()));
        }
    }
    let gnome = gio::SettingsSchemaSource::default().and_then(|s| s.lookup("org.gnome.system.proxy", true));
    if gnome.is_some() {
        let settings = gio::Settings::new("org.gnome.system.proxy");
        if settings.string("mode").as_str() != "none" {
            out.extend(settings.strv("ignore-hosts").iter().map(|h| h.to_string()));
        }
    }
    out
}

/// The plan for this system, with `loopback` added: GIO's default resolver
/// (the one WebKit's network process uses) and the person's ignore hosts.
pub fn system_plan_with(loopback: &[&str]) -> Option<Plan> {
    let resolver = gio::ProxyResolver::default();
    let lookup = |uri: &str| -> Vec<String> {
        match resolver.lookup(uri, None::<&gio::Cancellable>) {
            Ok(proxies) => proxies.iter().map(|p| p.to_string()).collect(),
            Err(e) => {
                k2_client::log_debug!("[webview-proxy] lookup {uri}: {e}");
                Vec::new()
            }
        }
    };
    plan(lookup, &person_ignore_hosts(), loopback)
}

/// Give `view`'s session `plan`: the per-scheme proxies and the ignore
/// hosts, in WebKit's custom mode.
pub fn apply_plan(view: &webkit2gtk::WebView, plan: &Plan) -> Result<(), String> {
    let manager = view
        .context()
        .and_then(|c| c.website_data_manager())
        .ok_or_else(|| "the web view has no website data manager".to_string())?;
    let hosts: Vec<&str> = plan.ignore_hosts.iter().map(String::as_str).collect();
    let mut settings = webkit2gtk::NetworkProxySettings::new(None, &hosts);
    for (scheme, proxy) in &plan.per_scheme {
        settings.add_proxy_for_scheme(scheme, proxy);
    }
    manager.set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Custom, Some(&mut settings));
    Ok(())
}

/// Once per session (website data manager): the system plan with the
/// loopback hosts, or nothing when the person has no proxy. Call it where
/// a K2 web view is built, before its page makes requests.
pub fn apply_once(view: &webkit2gtk::WebView) {
    static PLAN: OnceLock<Option<Plan>> = OnceLock::new();
    static DONE: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    let Some(plan) = PLAN.get_or_init(|| system_plan_with(&LOOPBACK_IGNORE)) else {
        return;
    };
    let Some(manager) = view.context().and_then(|c| c.website_data_manager()) else {
        k2_client::log_debug!("[webview-proxy] no website data manager; proxy settings unchanged");
        return;
    };
    let key = manager.as_ptr() as usize;
    let mut done = DONE.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
    if !done.insert(key) {
        return;
    }
    if let Err(e) = apply_plan(view, plan) {
        k2_client::log_debug!("[webview-proxy] {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_table(rows: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Vec<String> {
        move |uri| rows.iter().filter(|(u, _)| *u == uri).map(|(_, p)| p.to_string()).collect()
    }

    #[test]
    fn a_person_with_a_proxy_keeps_it_and_loopback_is_added_to_their_ignore_list() {
        let got = plan(
            lookup_table(&[
                ("http://k2-proxy-probe.invalid/", "http://proxy.acme.test:3128"),
                ("https://k2-proxy-probe.invalid/", "http://proxy.acme.test:3129"),
            ]),
            &["intranet.acme.test".to_string(), "localhost".to_string()],
            &LOOPBACK_IGNORE,
        )
        .expect("a plan");
        assert_eq!(
            got.per_scheme,
            vec![
                ("http".to_string(), "http://proxy.acme.test:3128".to_string()),
                ("https".to_string(), "http://proxy.acme.test:3129".to_string()),
                ("ws".to_string(), "http://proxy.acme.test:3128".to_string()),
                ("wss".to_string(), "http://proxy.acme.test:3129".to_string()),
            ]
        );
        assert_eq!(got.ignore_hosts, vec!["intranet.acme.test", "localhost", "*.localhost", "127.0.0.0/8", "::1", "[::1]"]);
    }

    #[test]
    fn no_proxy_or_an_ignore_everything_list_changes_nothing() {
        assert_eq!(plan(lookup_table(&[("http://k2-proxy-probe.invalid/", "direct://")]), &[], &LOOPBACK_IGNORE), None);
        assert_eq!(plan(lookup_table(&[]), &[], &LOOPBACK_IGNORE), None);
        assert_eq!(
            plan(lookup_table(&[("http://k2-proxy-probe.invalid/", "http://p:1")]), &["*".to_string()], &LOOPBACK_IGNORE),
            None
        );
    }

    #[test]
    fn a_socks_proxy_from_all_proxy_is_kept_as_is() {
        let got = plan(
            lookup_table(&[
                ("http://k2-proxy-probe.invalid/", "socks5://127.0.0.1:1080"),
                ("https://k2-proxy-probe.invalid/", "socks5://127.0.0.1:1080"),
            ]),
            &[],
            &LOOPBACK_IGNORE,
        )
        .expect("a plan");
        assert!(got.per_scheme.iter().all(|(_, p)| p == "socks5://127.0.0.1:1080"), "{:?}", got.per_scheme);
    }
}
