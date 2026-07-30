//! Embedded Browser Tab (PRD .k2/prds/prd-browser-pane-v1.md) — S1 spike.
//!
//! Rust-side lifecycle for CHILD webviews docked inside the main window
//! (tauri `unstable` multiwebview). Creation/positioning lives HERE, not in
//! the renderer, so `core:webview:allow-create-webview` is never granted to
//! renderer code — the browsed page's webview label appears in NO capability
//! and therefore has zero Tauri IPC surface (§6.5 security seam).
//!
//! The renderer drives these commands through a bounds-bridge (ResizeObserver
//! → rAF-throttled `browser_set_bounds`) and an overlay registry that calls
//! `browser_set_visible(false)` whenever any DOM overlay (modal, palette,
//! dropdown, drag) must render above pane content — native child views float
//! over the DOM unconditionally, so over-hiding is the only correct bias.

#![allow(clippy::module_inception)]

#[cfg(feature = "browser-pane")]
mod real {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use tauri::webview::WebviewBuilder;
    use tauri::{
        AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl,
    };

    /// Registry of live browser webviews keyed by our pane-item id (the tabs
    /// store's item id, NOT the tauri label — label = `browser-<item id>`).
    /// Mutex, not RwLock: every op is a short critical section on the main
    /// thread's command handlers.
    struct BrowserViews(Mutex<HashMap<String, Webview>>);

    fn views(app: &AppHandle) -> tauri::State<'_, BrowserViews> {
        app.state::<BrowserViews>()
    }

    /// Install the registry at app setup. Called once from lib.rs.
    pub fn init(app: &AppHandle) {
        app.manage(BrowserViews(Mutex::new(HashMap::new())));
    }

    /// Only http(s) may load in a browser pane (§6.5): never `file:`, `tauri:`,
    /// `asset:`, or custom schemes — a hostile page redirecting to a local
    /// scheme must dead-end. localhost/127.0.0.1 are ordinary http here.
    fn scheme_allowed(url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
    }

    fn parse_pane_url(raw: &str) -> Result<Url, String> {
        let url: Url = raw
            .parse()
            .map_err(|e| format!("invalid url {raw:?}: {e}"))?;
        if !scheme_allowed(&url) {
            return Err(format!("scheme '{}' is not allowed in a browser pane", url.scheme()));
        }
        Ok(url)
    }

    #[derive(serde::Deserialize)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
    }

    /// Create (or replace) the child webview for a pane item and dock it at
    /// `rect` (logical px, main-window coordinate space). Idempotent per item:
    /// an existing view for the id is closed first — reactivation after the
    /// 30-min lifecycle destroy (§6.4) goes through here again.
    ///
    /// Label uniqueness is enforced by **Tauri's registry** (`app.get_webview`),
    /// not only our in-memory `views` map. The map can desync after a renderer
    /// reload, a missed `browser_close`, or a panic that drops the map entry
    /// without closing the native child — then `add_child` collides on the
    /// deterministic label `browser-<item_id>` ("a webview with label …
    /// already exists"). Same reconcile pattern as focus windows
    /// (`projects.rs` + `get_webview_window`).
    #[tauri::command]
    pub async fn browser_create(
        app: AppHandle,
        item_id: String,
        url: String,
        rect: Rect,
    ) -> Result<(), String> {
        let parsed = parse_pane_url(&url)?;
        let window = app
            .get_window("main")
            .ok_or_else(|| "main window not found".to_string())?;

        let label = format!("browser-{item_id}");

        // (1) Our map — close + drop if we still track this item.
        if let Some(old) = views(&app).0.lock().unwrap().remove(&item_id) {
            let _ = old.close();
        }
        // (2) Tauri is authoritative for label uniqueness. If a native
        // webview still holds `browser-<id>` after a map desync, close it
        // before add_child so re-open never hard-collides.
        if let Some(existing) = app.get_webview(&label) {
            let _ = existing.close();
        }

        // on_navigation: scheme gate for EVERY in-page navigation, not just our
        // own `navigate` calls — the return bool vetoes the load (§6.5).
        let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed))
            .on_navigation(|url| matches!(url.scheme(), "http" | "https"))
            // Hidden panes get WKWebView timer throttling for free; explicit
            // background throttling policy is a follow-up knob (§6.4).
            .focused(false);

        let view = window
            .add_child(
                builder,
                LogicalPosition::new(rect.x, rect.y),
                LogicalSize::new(rect.width, rect.height),
            )
            .map_err(|e| format!("add_child failed: {e}"))?;

        views(&app).0.lock().unwrap().insert(item_id, view);
        Ok(())
    }

    /// Bounds re-assert from the renderer bridge. Also called unconditionally
    /// on window resize/restore — tauri #10131/#14843 both manifest as stale
    /// child bounds, so callers re-assert on a settle timer (§6.1).
    #[tauri::command]
    pub async fn browser_set_bounds(
        app: AppHandle,
        item_id: String,
        rect: Rect,
    ) -> Result<(), String> {
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&item_id).ok_or("no such browser view")?;
        view.set_position(LogicalPosition::new(rect.x, rect.y))
            .map_err(|e| e.to_string())?;
        view.set_size(LogicalSize::new(rect.width, rect.height))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Overlay-registry visibility flip (§6.2). Hide is also the retained-view
    /// rule's tool: a browser item that isn't the active item of a visible pane
    /// stays hidden exactly like `display:none` DOM panes.
    #[tauri::command]
    pub async fn browser_set_visible(
        app: AppHandle,
        item_id: String,
        visible: bool,
    ) -> Result<(), String> {
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&item_id).ok_or("no such browser view")?;
        if visible { view.show().map_err(|e| e.to_string())? } else { view.hide().map_err(|e| e.to_string())? }
        Ok(())
    }

    /// Navigate an existing pane. Same scheme gate as creation.
    #[tauri::command]
    pub async fn browser_navigate(
        app: AppHandle,
        item_id: String,
        url: String,
    ) -> Result<(), String> {
        let parsed = parse_pane_url(&url)?;
        let state = views(&app);
        let mut guard = state.0.lock().unwrap();
        let view = guard.get_mut(&item_id).ok_or("no such browser view")?;
        view.navigate(parsed).map_err(|e| e.to_string())
    }

    /// Current URL (address-bar sync after in-page navigation).
    #[tauri::command]
    pub async fn browser_current_url(app: AppHandle, item_id: String) -> Result<String, String> {
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&item_id).ok_or("no such browser view")?;
        view.url().map(|u| u.to_string()).map_err(|e| e.to_string())
    }

    /// Destroy the child view (tab close / 30-min hidden lifecycle). URL is
    /// retained renderer-side; reactivation re-creates.
    ///
    /// Closes both our map entry and any Tauri-registered webview for the
    /// deterministic label — so a desynced map still frees the label for a
    /// later `browser_create`.
    #[tauri::command]
    pub async fn browser_close(app: AppHandle, item_id: String) -> Result<(), String> {
        let label = format!("browser-{item_id}");
        if let Some(view) = views(&app).0.lock().unwrap().remove(&item_id) {
            view.close().map_err(|e| e.to_string())?;
        }
        if let Some(existing) = app.get_webview(&label) {
            let _ = existing.close();
        }
        Ok(())
    }

    /// S1 spike probe: report whether `window.__TAURI__` leaked into the
    /// browsed page (§6.5 acceptance — must be absent on external URLs).
    /// eval has no return channel; the probe writes into document.title which
    /// the spike reads back via `browser_eval_title_probe` → `url()`+title.
    #[tauri::command]
    pub async fn browser_devtools(app: AppHandle, item_id: String) -> Result<(), String> {
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&item_id).ok_or("no such browser view")?;
        #[cfg(debug_assertions)]
        view.open_devtools();
        #[cfg(not(debug_assertions))]
        let _ = view;
        Ok(())
    }
}

#[cfg(feature = "browser-pane")]
pub use real::*;

#[cfg(not(feature = "browser-pane"))]
mod stub {
    //! Same command surface, inert: default builds ship WITHOUT tauri's
    //! `unstable` feature (whole-app side effects — see Cargo.toml), so the
    //! renderer gets a uniform "not enabled" error instead of a missing
    //! command. init() is a no-op.
    use tauri::AppHandle;

    const OFF: &str = "browser pane is not enabled in this build";

    pub fn init(_app: &AppHandle) {}

    // Wire-shape parity with `real::Rect`: the stub must DESERIALIZE the
    // same JSON the renderer always sends, so the fields exist but are
    // (deliberately) never read in the browser-pane-off build.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
    }

    #[tauri::command]
    pub async fn browser_create(_app: AppHandle, _item_id: String, _url: String, _rect: Rect) -> Result<(), String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_set_bounds(_app: AppHandle, _item_id: String, _rect: Rect) -> Result<(), String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_set_visible(_app: AppHandle, _item_id: String, _visible: bool) -> Result<(), String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_navigate(_app: AppHandle, _item_id: String, _url: String) -> Result<(), String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_current_url(_app: AppHandle, _item_id: String) -> Result<String, String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_close(_app: AppHandle, _item_id: String) -> Result<(), String> { Err(OFF.into()) }
    #[tauri::command]
    pub async fn browser_devtools(_app: AppHandle, _item_id: String) -> Result<(), String> { Err(OFF.into()) }
}

#[cfg(not(feature = "browser-pane"))]
pub use stub::*;
