//! K2 app-window builder: pin the React origin and deny `window.open`.
//!
//! Every K2 UI webview (`main`, `window-{uuid}`, `focus-{project_id}`) must
//! stay on the app origin. A top-level navigation to `http://127.0.0.1:<port>`
//! (or any other origin) blanks the chrome. Child Browser panes are a
//! different builder (`browser_webviews.rs`) and keep their http(s) scheme
//! gate — this module is parent windows only.

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

/// True when the URL host is a loopback **IP** (`127.0.0.1` / `::1`).
/// `localhost` is **not** included — the Vite `devUrl` is `http://localhost:5173`.
pub fn is_loopback_ip_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("127.0.0.1") | Some("::1") | Some("[::1]")
    )
}

/// True when the URL host is loopback including the `localhost` name.
pub fn is_loopback_host(url: &Url) -> bool {
    is_loopback_ip_host(url) || url.host_str() == Some("localhost")
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn is_tauri_prod_origin(url: &Url) -> bool {
    match (url.scheme(), url.host_str()) {
        ("http", Some("tauri.localhost")) => true,
        ("tauri", Some("localhost")) => true,
        _ => false,
    }
}

/// Parent-window `on_navigation` predicate (C1 / C16).
///
/// Allow: spawn-time `app_url` origin, prod `http://tauri.localhost` +
/// `tauri://localhost`, and (in dev) the exact configured `devUrl` origin
/// including path/query on that origin.
///
/// Never allow `http://127.0.0.1` (even on the Vite port). Never allow
/// `http://localhost` except that exact `devUrl` origin (or spawn `app_url`).
pub fn allow_k2_app_navigation(
    url: &Url,
    app_url: Option<&Url>,
    dev_url: Option<&Url>,
    is_dev: bool,
) -> bool {
    if is_loopback_ip_host(url) {
        return false;
    }
    if is_tauri_prod_origin(url) {
        return true;
    }
    if let Some(app) = app_url {
        if !is_loopback_ip_host(app) && same_origin(url, app) {
            return true;
        }
    }
    if is_dev {
        if let Some(dev) = dev_url {
            if !is_loopback_ip_host(dev) && same_origin(url, dev) {
                return true;
            }
        }
    }
    false
}

fn url_is_usable_app_page(url: &Url) -> bool {
    let s = url.as_str();
    !s.is_empty() && s != "about:blank"
}

/// Watchdog `app_url` capture (C20): reject loopback hosts unless the URL
/// is the exact `devUrl` origin in `is_dev`. Never store `127.0.0.1:8788`
/// as the recovery target.
pub fn watchdog_capture_app_url(
    current: Option<&Url>,
    dev_url: Option<&Url>,
    is_dev: bool,
) -> Option<Url> {
    if let Some(u) = current {
        if url_is_usable_app_page(u) && watchdog_app_url_allowed(u, dev_url, is_dev) {
            return Some(u.clone());
        }
    }
    if is_dev {
        if let Some(d) = dev_url {
            if watchdog_app_url_allowed(d, dev_url, is_dev) {
                return Some(d.clone());
            }
        }
    }
    "http://tauri.localhost"
        .parse()
        .ok()
        .or_else(|| "tauri://localhost".parse().ok())
}

fn watchdog_app_url_allowed(url: &Url, dev_url: Option<&Url>, is_dev: bool) -> bool {
    if !is_loopback_host(url) {
        return true;
    }
    is_dev && dev_url.is_some_and(|d| same_origin(url, d))
}

/// Dev: configured `devUrl` (optional `#fragment`). Prod: `index.html`.
pub fn k2_app_webview_url<R: Runtime>(app: &AppHandle<R>, fragment: Option<&str>) -> WebviewUrl {
    if tauri::is_dev() {
        if let Some(mut url) = app.config().build.dev_url.clone() {
            if let Some(frag) = fragment {
                url.set_fragment(Some(frag));
            }
            return WebviewUrl::External(url);
        }
    }
    let path = match fragment {
        Some(frag) => format!("index.html#{frag}"),
        None => "index.html".to_string(),
    };
    WebviewUrl::App(path.into())
}

fn attach_k2_app_window_guards<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
    app: &AppHandle<R>,
) -> WebviewWindowBuilder<'a, R, M> {
    let is_dev = tauri::is_dev();
    let dev_url = app.config().build.dev_url.clone();
    let app_url = if is_dev {
        dev_url.clone()
    } else {
        "http://tauri.localhost".parse().ok()
    };
    builder
        .on_navigation(move |url| {
            allow_k2_app_navigation(url, app_url.as_ref(), dev_url.as_ref(), is_dev)
        })
        .on_new_window(|_url, _features| NewWindowResponse::Deny)
}

/// Extra K2 UI windows (`window-*`, `focus-*`). Attaches C1 + C15.
pub fn k2_app_window_builder<'a, R: Runtime, M: Manager<R>>(
    manager: &'a M,
    label: impl Into<String>,
    url: WebviewUrl,
) -> WebviewWindowBuilder<'a, R, M> {
    let builder = WebviewWindowBuilder::new(manager, label, url);
    attach_k2_app_window_guards(builder, manager.app_handle())
}

/// Conf `main` via `create: false` + `from_config` so `on_navigation` can
/// attach (builder-only API).
pub fn k2_app_window_from_config<'a, R: Runtime, M: Manager<R>>(
    manager: &'a M,
    config: &tauri::utils::config::WindowConfig,
) -> tauri::Result<WebviewWindowBuilder<'a, R, M>> {
    let builder = WebviewWindowBuilder::from_config(manager, config)?;
    Ok(attach_k2_app_window_guards(builder, manager.app_handle()))
}

/// Build the primary `main` window from `tauri.conf.json` `app.windows[0]`.
pub fn create_main_k2_window<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<WebviewWindow<R>, Box<dyn std::error::Error>> {
    let config = app.config().app.windows.first().cloned().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "tauri.conf.json app.windows[0] is missing",
        )
    })?;
    Ok(k2_app_window_from_config(app, &config)?.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(raw: &str) -> Url {
        raw.parse()
            .unwrap_or_else(|e| panic!("test url {raw:?} must parse: {e}"))
    }

    #[test]
    fn allow_k2_app_navigation_vetoes_loopback_ip_and_off_origin() {
        let dev = u("http://localhost:5173");
        let app = u("http://localhost:5173");
        assert!(
            !allow_k2_app_navigation(&u("http://127.0.0.1:9/"), Some(&app), Some(&dev), true),
            "127.0.0.1 must never navigate the parent"
        );
        assert!(
            !allow_k2_app_navigation(&u("http://example.com/"), Some(&app), Some(&dev), true),
            "off-origin http must be vetoed"
        );
        assert!(
            !allow_k2_app_navigation(&u("http://localhost:8788"), Some(&app), Some(&dev), true),
            "localhost on a non-devUrl port must be vetoed"
        );
        assert!(
            !allow_k2_app_navigation(&u("http://127.0.0.1:5173"), Some(&app), Some(&dev), true),
            "127.0.0.1 on the Vite port is still not devUrl"
        );
        assert!(
            !allow_k2_app_navigation(&u("https://localhost:5173"), Some(&app), Some(&dev), true),
            "https://localhost is not the exact http devUrl"
        );
    }

    #[test]
    fn allow_k2_app_navigation_allows_exact_devurl_and_tauri_origin() {
        let dev = u("http://localhost:5173");
        let app = u("http://localhost:5173");
        assert!(allow_k2_app_navigation(
            &u("http://localhost:5173"),
            Some(&app),
            Some(&dev),
            true
        ));
        assert!(allow_k2_app_navigation(
            &u("http://localhost:5173/"),
            Some(&app),
            Some(&dev),
            true
        ));
        assert!(allow_k2_app_navigation(
            &u("http://localhost:5173/foo?bar=1"),
            Some(&app),
            Some(&dev),
            true
        ));
        assert!(allow_k2_app_navigation(
            &u("http://tauri.localhost/"),
            None,
            None,
            false
        ));
        assert!(allow_k2_app_navigation(
            &u("tauri://localhost/"),
            None,
            None,
            false
        ));
        assert!(allow_k2_app_navigation(
            &u("http://tauri.localhost/index.html"),
            None,
            None,
            false
        ));
    }

    #[test]
    fn allow_k2_app_navigation_prod_does_not_treat_localhost_5173_as_app() {
        let stale = u("http://localhost:5173");
        assert!(!allow_k2_app_navigation(
            &stale,
            Some(&u("http://tauri.localhost/")),
            Some(&stale),
            false
        ));
    }

    #[test]
    fn watchdog_capture_rejects_loopback_unless_exact_devurl() {
        let dev = u("http://localhost:5173");
        let captured =
            watchdog_capture_app_url(Some(&u("http://127.0.0.1:8788/")), Some(&dev), true);
        let captured = captured.expect("fallback app_url");
        assert_ne!(captured.host_str(), Some("127.0.0.1"));
        assert!(
            same_origin(&captured, &dev) || is_tauri_prod_origin(&captured),
            "recovery URL must be devUrl or tauri origin, got {captured}"
        );

        let ok = watchdog_capture_app_url(Some(&dev), Some(&dev), true).expect("devUrl");
        assert!(same_origin(&ok, &dev));

        let prod = watchdog_capture_app_url(Some(&u("http://127.0.0.1:8788/")), Some(&dev), false)
            .expect("prod fallback");
        assert!(
            is_tauri_prod_origin(&prod),
            "prod must not recover to loopback, got {prod}"
        );

        let prod_stale = watchdog_capture_app_url(Some(&dev), Some(&dev), false).expect("prod");
        assert!(
            is_tauri_prod_origin(&prod_stale),
            "prod must not keep localhost:5173 as app_url, got {prod_stale}"
        );
    }

    #[test]
    fn default_capability_uses_webviews_not_windows_glob() {
        let raw = include_str!("../capabilities/default.json");
        let v: serde_json::Value =
            serde_json::from_str(raw).expect("capabilities/default.json must parse");

        match v.get("windows") {
            None => {}
            Some(windows) => {
                let arr = windows
                    .as_array()
                    .expect("windows, if present, must be an array");
                assert!(
                    arr.is_empty(),
                    "windows must be omitted or empty so child webviews do not inherit IPC; got {arr:?}"
                );
            }
        }

        let webviews = v
            .get("webviews")
            .and_then(|x| x.as_array())
            .expect("capability key must be webviews");
        let labels: Vec<&str> = webviews
            .iter()
            .map(|x| x.as_str().expect("webview label must be a string"))
            .collect();
        assert_eq!(labels, ["main", "window-*", "focus-*"]);
        assert!(
            !labels
                .iter()
                .any(|l| *l == "*" || *l == "main*" || l.contains("browser")),
            "webviews must not glob browser children: {labels:?}"
        );
        assert!(
            !labels.contains(&"browser-main-foo"),
            "synthetic browser-main-foo must not be listed"
        );
        assert!(
            !raw.contains("browser-main-foo"),
            "browser-main-foo must not appear in the capability file"
        );
        assert!(
            v.get("remote").is_none(),
            "default capability must not set remote.urls"
        );
    }
}
