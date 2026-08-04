//! Embedded Browser Tab (PRD .k2/prds/prd-browser-pane-v1.md) — S1 spike.
//!
//! Rust-side lifecycle for CHILD webviews docked inside the *invoking*
//! Tauri window (tauri `unstable` multiwebview). Creation/positioning lives
//! HERE, not in the renderer, so `core:webview:allow-create-webview` is never
//! granted to renderer code — the browsed page's webview label appears in NO
//! capability and therefore has zero Tauri IPC surface (§6.5 security seam).
//!
//! Multi-window: each browser child is parented via `parent_window` (the
//! caller's window label — `main` or `window-{uuid}`). Labels and registry
//! keys include the parent so the same item id in two windows cannot collide,
//! and so second windows no longer dock onto hard-coded `"main"`.
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

    /// Registry of live browser webviews. Keyed by composite
    /// `"parent_label\0item_id"` (NOT the tauri label).
    /// Tauri label = `browser-{sanitized_parent}-{item_id}`.
    /// Mutex, not RwLock: every op is a short critical section on the main
    /// thread's command handlers.
    struct BrowserViews(Mutex<HashMap<String, Webview>>);

    /// Serializes all create/close/reap for child webviews. Concurrent
    /// `browser_create` (visibility + ResizeObserver, or Cancel→Start OAuth)
    /// otherwise both pass "label free" and race `add_child`.
    struct BrowserCreateLock(tokio::sync::Mutex<()>);

    fn views(app: &AppHandle) -> tauri::State<'_, BrowserViews> {
        app.state::<BrowserViews>()
    }

    fn create_lock(app: &AppHandle) -> tauri::State<'_, BrowserCreateLock> {
        app.state::<BrowserCreateLock>()
    }

    /// Install the registry at app setup. Called once from lib.rs.
    pub fn init(app: &AppHandle) {
        app.manage(BrowserViews(Mutex::new(HashMap::new())));
        app.manage(BrowserCreateLock(tokio::sync::Mutex::new(())));
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

    /// Resolve parent window label: empty/missing → `"main"` for back-compat.
    fn resolve_parent(parent_window: Option<&str>) -> &str {
        match parent_window {
            Some(s) if !s.is_empty() => s,
            _ => "main",
        }
    }

    /// Composite registry key so the same item id can live in two windows.
    fn registry_key(parent: &str, item_id: &str) -> String {
        format!("{parent}\0{item_id}")
    }

    /// Sanitize a window label for use inside a Tauri webview label
    /// (alphanumeric only; everything else → `-`).
    fn sanitize_label_part(s: &str) -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    /// Deterministic Tauri webview label including parent so labels never
    /// collide across windows for the same item id.
    fn tauri_label(parent: &str, item_id: &str) -> String {
        format!("browser-{}-{}", sanitize_label_part(parent), item_id)
    }

    #[derive(serde::Deserialize)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
    }

    /// Close any live webview for the composite key (map + Tauri label).
    /// `Webview::close` is asynchronous on WKWebView — a bare close +
    /// immediate `add_child` races and surfaces `add_child failed: a webview
    /// with label … already exists` (Email Link Gmail OAuth re-open, double
    /// create from ResizeObserver).
    fn reap_browser_label(app: &AppHandle, key: &str, label: &str) {
        if let Some(old) = views(app).0.lock().unwrap().remove(key) {
            let _ = old.close();
        }
        if let Some(existing) = app.get_webview(label) {
            let _ = existing.close();
        }
    }

    /// Poll until Tauri no longer lists `label` (or give up after a short
    /// budget). Must run on the async command so we can yield without
    /// blocking the main thread forever.
    async fn wait_label_free(app: &AppHandle, label: &str) -> bool {
        // ~500ms worst case — WKWebView teardown is often slower than 250ms.
        for _ in 0..20 {
            if app.get_webview(label).is_none() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        app.get_webview(label).is_none()
    }

    /// Dock an existing webview (re-open path when the label never freed).
    /// Prefer this over failing `add_child` with "already exists".
    fn adopt_existing(
        app: &AppHandle,
        key: &str,
        label: &str,
        parsed: &Url,
        rect: &Rect,
    ) -> Result<(), String> {
        let view = app
            .get_webview(label)
            .ok_or_else(|| format!("expected existing webview {label}"))?;
        view.set_position(LogicalPosition::new(rect.x, rect.y))
            .map_err(|e| e.to_string())?;
        view.set_size(LogicalSize::new(rect.width, rect.height))
            .map_err(|e| e.to_string())?;
        let _ = view.show();
        view.navigate(parsed.clone()).map_err(|e| e.to_string())?;
        views(app)
            .0
            .lock()
            .unwrap()
            .insert(key.to_string(), view);
        Ok(())
    }

    /// Create (or replace) the child webview for a pane item and dock it at
    /// `rect` (logical px, parent-window coordinate space). Idempotent per
    /// (parent, item): an existing view for the key is re-used or reaped —
    /// reactivation after the 30-min lifecycle destroy (§6.4) goes through
    /// here again.
    ///
    /// Label uniqueness is enforced by **Tauri's registry** (`app.get_webview`),
    /// not only our in-memory `views` map. The map can desync after a renderer
    /// reload, a missed `browser_close`, or a panic that drops the map entry
    /// without closing the native child — then `add_child` collides on the
    /// deterministic label `browser-{parent}-{item_id}` ("a webview with label
    /// … already exists"). Same reconcile pattern as focus windows
    /// (`projects.rs` + `get_webview_window`).
    ///
    /// If the label still exists after reap+wait (WKWebView slow close, or
    /// concurrent creates), we **adopt** the existing view (navigate + bounds)
    /// instead of failing — Email Link Gmail OAuth re-open must never hard-error.
    ///
    /// `parent_window`: Tauri window label of the invoking window (`main` or
    /// `window-{uuid}`). Empty/missing falls back to `"main"` for back-compat.
    #[tauri::command]
    pub async fn browser_create(
        app: AppHandle,
        item_id: String,
        url: String,
        rect: Rect,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        // Serialize all creates so two concurrent creates for the same label
        // cannot both pass "label free" and race add_child.
        let create_gate = create_lock(&app);
        let _gate = create_gate.0.lock().await;

        let parent = resolve_parent(parent_window.as_deref()).to_string();
        let key = registry_key(&parent, &item_id);
        let label = tauri_label(&parent, &item_id);

        let parsed = parse_pane_url(&url)?;
        let window = app
            .get_window(&parent)
            .ok_or_else(|| format!("window {parent:?} not found"))?;

        // Fast path: we already track this key — navigate + re-dock.
        // Avoids tear-down thrash when visibility + RO both call create.
        let mut recreate_after_dead = false;
        {
            let state = views(&app);
            let mut map = state.0.lock().unwrap();
            if let Some(view) = map.get(&key) {
                let _ = view.set_position(LogicalPosition::new(rect.x, rect.y));
                let _ = view.set_size(LogicalSize::new(rect.width, rect.height));
                let _ = view.show();
                if view.navigate(parsed.clone()).is_err() {
                    // View may be half-dead; drop and fall through to recreate.
                    let _ = map.remove(&key);
                    recreate_after_dead = true;
                } else {
                    return Ok(());
                }
            }
        }
        if recreate_after_dead {
            reap_browser_label(&app, &key, &label);
            let _ = wait_label_free(&app, &label).await;
        }

        // Reap map + Tauri registry, then wait for the label to free.
        reap_browser_label(&app, &key, &label);
        let free = wait_label_free(&app, &label).await;
        if !free {
            // Native view still registered — adopt rather than collide.
            if app.get_webview(&label).is_some() {
                return adopt_existing(&app, &key, &label, &parsed, &rect);
            }
        }

        // on_navigation: scheme gate for EVERY in-page navigation, not just our
        // own `navigate` calls — the return bool vetoes the load (§6.5).
        let make_builder = |u: Url| {
            WebviewBuilder::new(&label, WebviewUrl::External(u))
                .on_navigation(|url| matches!(url.scheme(), "http" | "https"))
                .focused(false)
        };

        let view = match window.add_child(
            make_builder(parsed.clone()),
            LogicalPosition::new(rect.x, rect.y),
            LogicalSize::new(rect.width, rect.height),
        ) {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("already exists") {
                    // Adopt the survivor instead of failing the OAuth UI.
                    if app.get_webview(&label).is_some() {
                        return adopt_existing(&app, &key, &label, &parsed, &rect);
                    }
                    reap_browser_label(&app, &key, &label);
                    let _ = wait_label_free(&app, &label).await;
                    if app.get_webview(&label).is_some() {
                        return adopt_existing(&app, &key, &label, &parsed, &rect);
                    }
                    window
                        .add_child(
                            make_builder(parsed),
                            LogicalPosition::new(rect.x, rect.y),
                            LogicalSize::new(rect.width, rect.height),
                        )
                        .map_err(|e2| format!("add_child failed: {e2}"))?
                } else {
                    return Err(format!("add_child failed: {msg}"));
                }
            }
        };

        views(&app).0.lock().unwrap().insert(key, view);
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
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&key).ok_or("no such browser view")?;
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
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&key).ok_or("no such browser view")?;
        if visible {
            view.show().map_err(|e| e.to_string())?
        } else {
            view.hide().map_err(|e| e.to_string())?
        }
        Ok(())
    }

    /// Navigate an existing pane. Same scheme gate as creation.
    #[tauri::command]
    pub async fn browser_navigate(
        app: AppHandle,
        item_id: String,
        url: String,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let parsed = parse_pane_url(&url)?;
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let state = views(&app);
        let mut guard = state.0.lock().unwrap();
        let view = guard.get_mut(&key).ok_or("no such browser view")?;
        view.navigate(parsed).map_err(|e| e.to_string())
    }

    /// Current URL (address-bar sync after in-page navigation).
    #[tauri::command]
    pub async fn browser_current_url(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<String, String> {
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&key).ok_or("no such browser view")?;
        view.url().map(|u| u.to_string()).map_err(|e| e.to_string())
    }

    /// Destroy the child view (tab close / 30-min hidden lifecycle). URL is
    /// retained renderer-side; reactivation re-creates.
    ///
    /// Closes both our map entry and any Tauri-registered webview for the
    /// deterministic label — so a desynced map still frees the label for a
    /// later `browser_create`. Waits briefly so a same-label re-create
    /// (Email Link Cancel → Start again, or React StrictMode remount)
    /// does not race `add_child`.
    #[tauri::command]
    pub async fn browser_close(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let create_gate = create_lock(&app);
        let _gate = create_gate.0.lock().await;
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let label = tauri_label(parent, &item_id);
        reap_browser_label(&app, &key, &label);
        let _ = wait_label_free(&app, &label).await;
        Ok(())
    }

    /// S1 spike probe: report whether `window.__TAURI__` leaked into the
    /// browsed page (§6.5 acceptance — must be absent on external URLs).
    /// eval has no return channel; the probe writes into document.title which
    /// the spike reads back via `browser_eval_title_probe` → `url()`+title.
    #[tauri::command]
    pub async fn browser_devtools(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let parent = resolve_parent(parent_window.as_deref());
        let key = registry_key(parent, &item_id);
        let state = views(&app);
        let guard = state.0.lock().unwrap();
        let view = guard.get(&key).ok_or("no such browser view")?;
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
    pub async fn browser_create(
        _app: AppHandle,
        _item_id: String,
        _url: String,
        _rect: Rect,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_set_bounds(
        _app: AppHandle,
        _item_id: String,
        _rect: Rect,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_set_visible(
        _app: AppHandle,
        _item_id: String,
        _visible: bool,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_navigate(
        _app: AppHandle,
        _item_id: String,
        _url: String,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_current_url(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<String, String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_close(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_devtools(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
}

#[cfg(not(feature = "browser-pane"))]
pub use stub::*;
