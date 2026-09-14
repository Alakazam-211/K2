//! K2 app-window builder: pin the React origin and deny `window.open`.
//!
//! Every K2 UI webview (`main`, `window-{uuid}`, `focus-{project_id}`) must
//! stay on the app origin. A top-level navigation to `http://127.0.0.1:<port>`
//! (or any other origin) blanks the chrome. Child Browser panes are a
//! different builder (`browser_webviews.rs`) and keep their http(s) scheme
//! gate — this module is parent windows only.
//!
//! wry (macOS WKWebView + Linux WebKitGTK) runs the `on_navigation`
//! predicate for **every** frame, subframes included, with no main-frame
//! flag. The renderer draws HTML file tabs, dashboard htmlDoc panes and
//! Inbox HTML mail inside `<iframe srcDoc>` → a subframe navigation to
//! `about:srcdoc`. So the predicate also allows `about:` (always) and
//! `blob:` whose inner URL is an allowed app origin (C24 of
//! `prd-tauri-loopback-navigation-crash-v1`, per
//! `prd-html-tab-srcdoc-navigation-guard-v1`). Neither leaves the app;
//! the loopback-IP / off-origin vetoes are unchanged, including for an
//! `<iframe src="http://127.0.0.1:…">` inside a user document.

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

/// Parent-window `on_navigation` predicate (C1 / C16 / C24).
///
/// Allow: `about:` (always — `about:srcdoc` is every `<iframe srcDoc>`
/// subframe, `about:blank` cannot reach loopback or an off-origin host),
/// spawn-time `app_url` origin, prod `http://tauri.localhost` +
/// `tauri://localhost`, (in dev) the exact configured `devUrl` origin
/// including path/query on that origin, and `blob:` whose inner URL
/// (`Url::parse(url.path())`) is `http`/`https`/`tauri` on one of those
/// same allowed origins (`blob:tauri://localhost/…` mac/Linux prod,
/// `blob:http://tauri.localhost/…` Windows prod, `blob:http://localhost:5173/…`
/// dev only).
///
/// Never allow `http://127.0.0.1` (even on the Vite port), including as a
/// blob inner URL. Never allow `http://localhost` except that exact `devUrl`
/// origin (or spawn `app_url`). `data:`, `blob:null/…`, `blob:about:blank`
/// and every other scheme/origin stay vetoed.
pub fn allow_k2_app_navigation(
    url: &Url,
    app_url: Option<&Url>,
    dev_url: Option<&Url>,
    is_dev: bool,
) -> bool {
    // G1: `about:` never leaves the app (no host, nothing to reach).
    if url.scheme() == "about" {
        return true;
    }
    if is_loopback_ip_host(url) {
        return false;
    }
    if url.scheme() == "blob" {
        // G2/G15: explicit, non-recursive, scheme-gated inner check. The
        // outer `blob:` URL has no host, so the veto above never fires for
        // it — the inner loopback check below is the veto for blobs.
        let Ok(inner) = Url::parse(url.path()) else {
            return false;
        };
        if !matches!(inner.scheme(), "http" | "https" | "tauri") {
            return false;
        }
        return allow_app_origin(&inner, app_url, dev_url, is_dev);
    }
    allow_app_origin(url, app_url, dev_url, is_dev)
}

/// Origin allow-list shared by the top-level and blob-inner checks: not a
/// loopback IP, and (prod tauri origin | spawn `app_url` same-origin with a
/// non-loopback app | dev exact `devUrl` same-origin).
fn allow_app_origin(
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

/// A page the watchdog may reload to. `about:` / `blob:` / `data:` are
/// never reloadable app pages (G6 / G16(a)): only `http`, `https`, `tauri`.
fn url_is_usable_app_page(url: &Url) -> bool {
    let s = url.as_str();
    !s.is_empty()
        && s != "about:blank"
        && matches!(url.scheme(), "http" | "https" | "tauri")
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

    // ── PRD html-tab-srcdoc-navigation-guard G7 / G21 ─────────────────────

    fn dev_args() -> (Url, Url) {
        (u("http://localhost:5173"), u("http://localhost:5173"))
    }

    /// G21(a): `about:srcdoc` (every `<iframe srcDoc>` subframe) is allowed
    /// with prod args and with dev args.
    #[test]
    fn allow_k2_app_navigation_allows_about_srcdoc_prod_and_dev() {
        let (app, dev) = dev_args();
        assert!(
            allow_k2_app_navigation(&u("about:srcdoc"), None, None, false),
            "about:srcdoc must be allowed in prod"
        );
        assert!(
            allow_k2_app_navigation(&u("about:srcdoc"), Some(&app), Some(&dev), true),
            "about:srcdoc must be allowed in dev"
        );
    }

    /// G21(b): `about:blank` allowed, both.
    #[test]
    fn allow_k2_app_navigation_allows_about_blank_prod_and_dev() {
        let (app, dev) = dev_args();
        assert!(
            allow_k2_app_navigation(&u("about:blank"), None, None, false),
            "about:blank must be allowed in prod"
        );
        assert!(
            allow_k2_app_navigation(&u("about:blank"), Some(&app), Some(&dev), true),
            "about:blank must be allowed in dev"
        );
    }

    /// G21(c): app-origin blobs — mac/Linux prod `blob:tauri://localhost/x`,
    /// Windows prod `blob:http://tauri.localhost/x`, dev
    /// `blob:http://localhost:5173/x` (allowed in dev, vetoed in prod —
    /// stale-dev sibling of `..._prod_does_not_treat_localhost_5173_as_app`).
    #[test]
    fn allow_k2_app_navigation_allows_app_origin_blobs() {
        let (app, dev) = dev_args();
        let prod_app = u("http://tauri.localhost/");
        assert!(
            allow_k2_app_navigation(&u("blob:tauri://localhost/x"), None, None, false),
            "blob:tauri://localhost (macOS/Linux prod) must be allowed"
        );
        assert!(
            allow_k2_app_navigation(
                &u("blob:tauri://localhost/x"),
                Some(&prod_app),
                Some(&dev),
                false
            ),
            "blob:tauri://localhost must be allowed with the spawn-time prod app_url"
        );
        assert!(
            allow_k2_app_navigation(&u("blob:http://tauri.localhost/x"), None, None, false),
            "blob:http://tauri.localhost (Windows prod) must be allowed"
        );
        assert!(
            allow_k2_app_navigation(
                &u("blob:http://localhost:5173/x"),
                Some(&app),
                Some(&dev),
                true
            ),
            "blob:http://localhost:5173 must be allowed in dev"
        );
        assert!(
            !allow_k2_app_navigation(
                &u("blob:http://localhost:5173/x"),
                Some(&prod_app),
                Some(&dev),
                false
            ),
            "blob:http://localhost:5173 must be vetoed in prod (stale devUrl)"
        );
    }

    /// G21(d): loopback-IP blob inners are vetoed both ways — the outer
    /// `blob:` URL has no host, so only the inner check can veto them.
    #[test]
    fn allow_k2_app_navigation_vetoes_loopback_ip_blobs() {
        let (app, dev) = dev_args();
        assert!(
            !allow_k2_app_navigation(&u("blob:http://127.0.0.1:8788/x"), None, None, false),
            "blob:http://127.0.0.1 must be vetoed in prod"
        );
        assert!(
            !allow_k2_app_navigation(
                &u("blob:http://127.0.0.1:8788/x"),
                Some(&app),
                Some(&dev),
                true
            ),
            "blob:http://127.0.0.1 must be vetoed in dev"
        );
        assert!(
            !allow_k2_app_navigation(&u("blob:http://[::1]:8788/x"), None, None, false),
            "blob:http://[::1] must be vetoed in prod"
        );
        assert!(
            !allow_k2_app_navigation(
                &u("blob:http://[::1]:8788/x"),
                Some(&app),
                Some(&dev),
                true
            ),
            "blob:http://[::1] must be vetoed in dev"
        );
        assert!(
            !allow_k2_app_navigation(&u("blob:http://example.com/x"), None, None, false),
            "off-origin blob inner must be vetoed"
        );
    }

    /// G21(e): `data:` is not on the allow list (G3), both.
    #[test]
    fn allow_k2_app_navigation_vetoes_data_urls() {
        let (app, dev) = dev_args();
        assert!(
            !allow_k2_app_navigation(&u("data:text/html,x"), None, None, false),
            "data: must be vetoed in prod"
        );
        assert!(
            !allow_k2_app_navigation(&u("data:text/html,x"), Some(&app), Some(&dev), true),
            "data: must be vetoed in dev"
        );
    }

    /// G21(e′): opaque / non-http blob inners are vetoed (G15: explicit,
    /// non-recursive inner check).
    #[test]
    fn allow_k2_app_navigation_vetoes_opaque_and_nested_blobs() {
        let (app, dev) = dev_args();
        for raw in ["blob:null/x", "blob:about:blank", "blob:blob:tauri://localhost/x"] {
            assert!(
                !allow_k2_app_navigation(&u(raw), None, None, false),
                "{raw} must be vetoed in prod"
            );
            assert!(
                !allow_k2_app_navigation(&u(raw), Some(&app), Some(&dev), true),
                "{raw} must be vetoed in dev"
            );
        }
    }

    /// G21(g) / G16(a): the watchdog never stores `about:` or `blob:` as the
    /// recovery target.
    #[test]
    fn watchdog_capture_never_stores_about_or_blob() {
        let dev = u("http://localhost:5173");
        for raw in [
            "about:srcdoc",
            "blob:tauri://localhost/x",
            "blob:http://127.0.0.1:8788/x",
        ] {
            let current = u(raw);
            let prod = watchdog_capture_app_url(Some(&current), None, false)
                .expect("prod fallback must exist");
            assert_ne!(prod, current, "prod watchdog must not capture {raw}");
            assert!(
                is_tauri_prod_origin(&prod),
                "prod watchdog must fall back to the tauri origin for {raw}, got {prod}"
            );

            let devc = watchdog_capture_app_url(Some(&current), Some(&dev), true)
                .expect("dev fallback must exist");
            assert_ne!(devc, current, "dev watchdog must not capture {raw}");
            assert!(
                same_origin(&devc, &dev),
                "dev watchdog must fall back to devUrl for {raw}, got {devc}"
            );
        }
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
