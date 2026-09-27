//! Embedded Browser Tab (PRD .k2/prds/prd-browser-pane-v1.md) — S1 spike.
//!
//! Rust-side lifecycle for CHILD webviews docked inside the *invoking*
//! Tauri window (tauri `unstable` multiwebview). Creation/positioning lives
//! HERE, not in the renderer, so `core:webview:allow-create-webview` is never
//! granted to renderer code. Default capability lists `webviews` (`main`,
//! `window-*`, `focus-*`) and omits `windows` — a window glob would still
//! IPC `browser-*` children of that window (§6.5 security seam).
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

/// Page history for one browser child. Wire shape is `{ canBack, canForward }`.
/// No map entry is both flags false — not an error, and not the red strip.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserHistoryState {
    pub can_back: bool,
    pub can_forward: bool,
}

#[cfg(feature = "browser-pane")]
mod real {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};

    use tauri::{
        AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl,
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
            return Err(format!(
                "scheme '{}' is not allowed in a browser pane",
                url.scheme()
            ));
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
        views(app).0.lock().unwrap().insert(key.to_string(), view);
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

    /// macOS only. Reads WKWebView's own agent on the main thread and adds
    /// the Safari product token it leaves off. Cached. Windows and Linux
    /// do not call this: WebView2 already says Edge, WebKitGTK already
    /// names itself.
    #[cfg(target_os = "macos")]
    fn macos_browser_user_agent(app: &AppHandle) -> Option<String> {
        static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        if let Some(hit) = CACHED.get() {
            return if hit.is_empty() {
                None
            } else {
                Some(hit.clone())
            };
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let queued = app.run_on_main_thread(move || {
            let _ = tx.send(macos_default_webkit_agent());
        });
        let base = if queued.is_err() {
            None
        } else {
            rx.recv().ok().flatten()
        };
        let declared = base
            .as_deref()
            .map(|ua| {
                let (major, minor) = macos_product_version();
                super::declare_browser_engine(ua, major, minor)
            })
            .filter(|ua| ua.contains("Safari/"));
        let stored = declared.clone().unwrap_or_default();
        let _ = CACHED.set(stored);
        declared
    }

    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    fn macos_default_webkit_agent() -> Option<String> {
        use cocoa::base::{id, nil};
        use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        use std::ffi::CStr;

        unsafe {
            let config: id = msg_send![class!(WKWebViewConfiguration), new];
            if config == nil {
                return None;
            }
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            let alloc: id = msg_send![class!(WKWebView), alloc];
            let view: id = msg_send![alloc, initWithFrame: frame configuration: config];
            if view == nil {
                return None;
            }
            let key = NSString::alloc(nil).init_str("userAgent");
            let ua_obj: *mut Object = msg_send![view, valueForKey: key];
            if ua_obj.is_null() {
                return None;
            }
            let bytes: *const i8 = msg_send![ua_obj, UTF8String];
            if bytes.is_null() {
                return None;
            }
            CStr::from_ptr(bytes).to_str().ok().map(str::to_string)
        }
    }

    #[cfg(target_os = "macos")]
    fn macos_product_version() -> (i64, i64) {
        let Ok(out) = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
        else {
            return (0, 0);
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut parts = text.trim().split('.');
        let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor)
    }

    /// Shell `img-src` has no general `https:`. Bytes are read in the child
    /// and only a `data:image/…` URL is emitted. Sync XHR: `eval_with_callback`
    /// does not await a Promise on Windows or Linux. Empty means keep the globe.
    const BROWSER_FAVICON_SCRIPT: &str = r#"(function () {
  var MAX = 65536;
  function sniff(bytes) {
    if (bytes.length >= 4 && bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) return 'image/png';
    if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return 'image/jpeg';
    if (bytes.length >= 4 && bytes[0] === 0x47 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x38) return 'image/gif';
    if (bytes.length >= 4 && bytes[0] === 0x00 && bytes[1] === 0x00 && bytes[2] === 0x01 && bytes[3] === 0x00) return 'image/x-icon';
    if (bytes.length >= 12 && bytes[0] === 0x52 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x46 && bytes[8] === 0x57 && bytes[9] === 0x45 && bytes[10] === 0x42 && bytes[11] === 0x50) return 'image/webp';
    var n = bytes.length < 180 ? bytes.length : 180;
    var head = '';
    for (var i = 0; i < n; i++) head += String.fromCharCode(bytes[i]);
    var t = head.replace(/^\s+/, '').toLowerCase();
    if (t.indexOf('<svg') === 0 || t.indexOf('<?xml') === 0) return 'image/svg+xml';
    return '';
  }
  function fromBytes(bytes, headerMime) {
    if (!bytes || bytes.length === 0 || bytes.length > MAX) return '';
    var mime = sniff(bytes);
    if (!mime && headerMime && headerMime.indexOf('image/') === 0) mime = headerMime.split(';')[0].trim();
    if (!mime || mime.indexOf('image/') !== 0) return '';
    var raw = '';
    for (var i = 0; i < bytes.length; i++) raw += String.fromCharCode(bytes[i]);
    try { return 'data:' + mime + ';base64,' + btoa(raw); } catch (e) { return ''; }
  }
  function readUrl(url) {
    if (!url) return '';
    if (url.indexOf('data:image/') === 0) return url.length > 120000 ? '' : url;
    if (url.indexOf('http:') !== 0 && url.indexOf('https:') !== 0) return '';
    try {
      var xhr = new XMLHttpRequest();
      xhr.open('GET', url, false);
      xhr.overrideMimeType('text/plain; charset=x-user-defined');
      xhr.send(null);
      if (xhr.status < 200 || xhr.status >= 300) return '';
      var text = xhr.responseText || '';
      var bytes = new Uint8Array(text.length);
      for (var i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i) & 255;
      var header = '';
      try { header = xhr.getResponseHeader('Content-Type') || ''; } catch (e2) { header = ''; }
      return fromBytes(bytes, header);
    } catch (e3) {
      return '';
    }
  }
  function pickLink() {
    var nodes = document.querySelectorAll('link[rel]');
    var best = '';
    var bestRank = 99;
    for (var i = 0; i < nodes.length; i++) {
      var rel = (nodes[i].getAttribute('rel') || '').toLowerCase().split(/\s+/);
      if (rel.indexOf('icon') === -1) continue;
      var href = nodes[i].href || '';
      if (!href) continue;
      var sizes = (nodes[i].getAttribute('sizes') || '').toLowerCase();
      var rank = 3;
      if (sizes.indexOf('32x32') !== -1) rank = 0;
      else if (sizes.indexOf('16x16') !== -1) rank = 1;
      else if (!sizes || sizes === 'any') rank = 2;
      if (rank < bestRank) { bestRank = rank; best = href; }
    }
    return best;
  }
  var link = pickLink();
  var icon = link ? readUrl(link) : '';
  if (icon) return icon;
  try { return readUrl(new URL('/favicon.ico', location.origin).href); } catch (e4) { return ''; }
})()"#;

    const FAVICON_DATA_URL_MAX: usize = 120_000;

    /// Callback payload is a JSON string. Only `data:image/` crosses into the shell.
    fn parse_favicon_result(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "null" || trimmed == "undefined" {
            return String::new();
        }
        let value = serde_json::from_str::<String>(trimmed).unwrap_or_else(|_| trimmed.to_string());
        let value = value.trim();
        if value.len() > FAVICON_DATA_URL_MAX || !value.starts_with("data:image/") {
            return String::new();
        }
        value.to_string()
    }

    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct BrowserPageMeta {
        item_id: String,
        parent: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    }

    /// Tab label only. The callback's `Webview` is the child, not a window —
    /// the shell title stays `K2 | <server>`.
    fn emit_browser_page_meta(webview: &Webview, parent: &str, meta: BrowserPageMeta) {
        if let Some(window) = webview.get_webview_window(parent) {
            let _ = window.emit("browser:page-meta", &meta);
            return;
        }
        let _ = webview
            .app_handle()
            .emit_to(parent, "browser:page-meta", &meta);
    }

    fn read_browser_favicon(webview: &Webview, item_id: &str, parent: &str) {
        let item_id = item_id.to_string();
        let parent = parent.to_string();
        let emit_view = webview.clone();
        let item_for_cb = item_id.clone();
        let parent_for_cb = parent.clone();
        if webview
            .eval_with_callback(BROWSER_FAVICON_SCRIPT, move |raw| {
                emit_browser_page_meta(
                    &emit_view,
                    &parent_for_cb,
                    BrowserPageMeta {
                        item_id: item_for_cb.clone(),
                        parent: parent_for_cb.clone(),
                        title: None,
                        icon: Some(parse_favicon_result(&raw)),
                    },
                );
            })
            .is_err()
        {
            emit_browser_page_meta(
                webview,
                &parent,
                BrowserPageMeta {
                    item_id,
                    parent: parent.clone(),
                    title: None,
                    icon: Some(String::new()),
                },
            );
        }
    }

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
        // Loopback is ordinary http here (Gmail OAuth redirects to this Mac).
        // on_new_window: Deny — never `window.open` into a window labeled `main`.
        // Title and favicon stay on the tab. The child is not a window.
        let make_builder = |u: Url| {
            let title_item = item_id.clone();
            let title_parent = parent.clone();
            let icon_item = item_id.clone();
            let icon_parent = parent.clone();
            let builder = WebviewBuilder::new(&label, WebviewUrl::External(u))
                .on_navigation(|url| matches!(url.scheme(), "http" | "https"))
                .on_new_window(|_url, _features| NewWindowResponse::Deny)
                .on_document_title_changed(move |webview, title| {
                    emit_browser_page_meta(
                        &webview,
                        &title_parent,
                        BrowserPageMeta {
                            item_id: title_item.clone(),
                            parent: title_parent.clone(),
                            title: Some(title),
                            icon: None,
                        },
                    );
                })
                .on_page_load(move |webview, payload| {
                    if payload.event() != PageLoadEvent::Finished {
                        return;
                    }
                    if !matches!(payload.url().scheme(), "http" | "https") {
                        return;
                    }
                    read_browser_favicon(&webview, &icon_item, &icon_parent);
                })
                .focused(false);
            // macOS WKWebView omits Safari. Windows WebView2 already says
            // Edge, and Linux WebKitGTK already names itself, so only the
            // Mac pane amends the engine's own string.
            #[cfg(target_os = "macos")]
            let builder = match macos_browser_user_agent(&app) {
                Some(agent) => builder.user_agent(&agent),
                None => builder,
            };
            builder
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

    #[derive(Clone, Copy)]
    enum HistoryStep {
        Back,
        Forward,
    }

    fn browser_view(
        app: &AppHandle,
        item_id: &str,
        parent_window: Option<&str>,
    ) -> Option<Webview> {
        let parent = resolve_parent(parent_window);
        let key = registry_key(parent, item_id);
        views(app).0.lock().unwrap().get(&key).cloned()
    }

    /// `with_webview` posts to the main thread and returns before the
    /// closure runs. Bound the wait so a dead child cannot hang the command.
    async fn with_platform_webview<T: Send + 'static>(
        view: &Webview,
        f: impl FnOnce(tauri::webview::PlatformWebview) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |webview| {
            let _ = tx.send(f(webview));
        })
        .map_err(|e| e.to_string())?;
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("browser view closed before history".into()),
            Err(_) => Err("browser history timed out".into()),
        }
    }

    fn history_on_platform(
        webview: tauri::webview::PlatformWebview,
        step: Option<HistoryStep>,
    ) -> Result<super::BrowserHistoryState, String> {
        #[cfg(target_os = "macos")]
        let result = macos_history(webview, step);
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let result = gtk_history(webview, step);
        #[cfg(windows)]
        let result = windows_history(webview, step);
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        let result = {
            let _ = (webview, step);
            Ok(super::BrowserHistoryState {
                can_back: false,
                can_forward: false,
            })
        };
        result
    }

    #[cfg(target_os = "macos")]
    fn macos_history(
        webview: tauri::webview::PlatformWebview,
        step: Option<HistoryStep>,
    ) -> Result<super::BrowserHistoryState, String> {
        // PlatformWebview::inner() is the WKWebView tauri already owns.
        // Do not add wry::WebView::go_back — that method is not on the child.
        unsafe {
            let view: &objc2_web_kit::WKWebView = &*webview.inner().cast();
            match step {
                Some(HistoryStep::Back) => {
                    let _ = view.goBack();
                }
                Some(HistoryStep::Forward) => {
                    let _ = view.goForward();
                }
                None => {}
            }
            Ok(super::BrowserHistoryState {
                can_back: view.canGoBack(),
                can_forward: view.canGoForward(),
            })
        }
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    fn gtk_history(
        webview: tauri::webview::PlatformWebview,
        step: Option<HistoryStep>,
    ) -> Result<super::BrowserHistoryState, String> {
        use webkit2gtk::WebViewExt;
        let view = webview.inner();
        match step {
            Some(HistoryStep::Back) => view.go_back(),
            Some(HistoryStep::Forward) => view.go_forward(),
            None => {}
        }
        Ok(super::BrowserHistoryState {
            can_back: view.can_go_back(),
            can_forward: view.can_go_forward(),
        })
    }

    #[cfg(windows)]
    fn windows_history(
        webview: tauri::webview::PlatformWebview,
        step: Option<HistoryStep>,
    ) -> Result<super::BrowserHistoryState, String> {
        // History lives on the controller's ICoreWebView2, not a second webview.
        unsafe {
            let core = webview
                .controller()
                .CoreWebView2()
                .map_err(|e| e.to_string())?;
            match step {
                Some(HistoryStep::Back) => {
                    core.GoBack().map_err(|e| e.to_string())?;
                }
                Some(HistoryStep::Forward) => {
                    core.GoForward().map_err(|e| e.to_string())?;
                }
                None => {}
            }
            let mut can_back = windows_core::BOOL::default();
            let mut can_forward = windows_core::BOOL::default();
            core.CanGoBack(&mut can_back).map_err(|e| e.to_string())?;
            core.CanGoForward(&mut can_forward)
                .map_err(|e| e.to_string())?;
            Ok(super::BrowserHistoryState {
                can_back: can_back.as_bool(),
                can_forward: can_forward.as_bool(),
            })
        }
    }

    fn disabled_history() -> super::BrowserHistoryState {
        super::BrowserHistoryState {
            can_back: false,
            can_forward: false,
        }
    }

    /// Walk this child's back-forward list. No view is `Ok(())` — the
    /// renderer must not paint that as the red strip.
    #[tauri::command]
    pub async fn browser_back(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let Some(view) = browser_view(&app, &item_id, parent_window.as_deref()) else {
            return Ok(());
        };
        with_platform_webview(&view, |webview| {
            history_on_platform(webview, Some(HistoryStep::Back))
        })
        .await?;
        Ok(())
    }

    /// Walk this child's back-forward list forward. No view is `Ok(())`.
    #[tauri::command]
    pub async fn browser_forward(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<(), String> {
        let Some(view) = browser_view(&app, &item_id, parent_window.as_deref()) else {
            return Ok(());
        };
        with_platform_webview(&view, |webview| {
            history_on_platform(webview, Some(HistoryStep::Forward))
        })
        .await?;
        Ok(())
    }

    /// `{ canBack, canForward }` for this child. No map entry is both false,
    /// not `Err`, so the address chrome does not use the red strip.
    #[tauri::command]
    pub async fn browser_history_state(
        app: AppHandle,
        item_id: String,
        parent_window: Option<String>,
    ) -> Result<super::BrowserHistoryState, String> {
        let Some(view) = browser_view(&app, &item_id, parent_window.as_deref()) else {
            return Ok(disabled_history());
        };
        match with_platform_webview(&view, |webview| history_on_platform(webview, None)).await {
            Ok(state) => Ok(state),
            Err(_) => Ok(disabled_history()),
        }
    }

    #[cfg(test)]
    mod favicon_parse_tests {
        use super::parse_favicon_result;

        #[test]
        fn data_image_url_round_trips() {
            let raw = serde_json::to_string("data:image/png;base64,AAAA").unwrap();
            assert_eq!(parse_favicon_result(&raw), "data:image/png;base64,AAAA");
        }

        #[test]
        fn https_blob_and_empty_stay_empty() {
            assert_eq!(
                parse_favicon_result("\"https://example.com/favicon.ico\""),
                ""
            );
            assert_eq!(
                parse_favicon_result(
                    "\"blob:https://example.com/11111111-1111-1111-1111-111111111111\""
                ),
                ""
            );
            assert_eq!(parse_favicon_result("null"), "");
            assert_eq!(parse_favicon_result(""), "");
            assert_eq!(parse_favicon_result("undefined"), "");
        }
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
    #[tauri::command]
    pub async fn browser_back(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_forward(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<(), String> {
        Err(OFF.into())
    }
    #[tauri::command]
    pub async fn browser_history_state(
        _app: AppHandle,
        _item_id: String,
        _parent_window: Option<String>,
    ) -> Result<super::BrowserHistoryState, String> {
        Err(OFF.into())
    }
}

#[cfg(not(feature = "browser-pane"))]
pub use stub::*;

/// Keep each engine's own agent. Apple WebKit omits a browser product, so
/// add Safari using the WebKit version already in the string and the
/// running OS for `Version/`. An agent that already names Safari, Edge,
/// or Chrome is left alone. macOS calls this. Other platforms only compile
/// it for the unit tests.
#[cfg(any(target_os = "macos", test))]
fn declare_browser_engine(default_ua: &str, os_major: i64, os_minor: i64) -> String {
    if default_ua.contains("Safari/")
        || default_ua.contains("Edg/")
        || default_ua.contains("Chrome/")
    {
        return default_ua.to_string();
    }
    let Some(webkit) = default_ua
        .split("AppleWebKit/")
        .nth(1)
        .and_then(|rest| rest.split([' ', ';']).next())
        .filter(|ver| !ver.is_empty())
    else {
        return default_ua.to_string();
    };
    if os_major <= 0 {
        return format!("{default_ua} Safari/{webkit}");
    }
    format!(
        "{default_ua} Version/{os_major}.{minor} Safari/{webkit}",
        minor = os_minor.max(0)
    )
}

#[cfg(test)]
mod history_wire {
    use super::BrowserHistoryState;

    #[test]
    fn history_state_serializes_camel_case_disabled() {
        let v = serde_json::to_value(BrowserHistoryState {
            can_back: false,
            can_forward: false,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "canBack": false, "canForward": false })
        );
    }

    #[test]
    fn webkit_without_a_product_gains_safari_from_its_own_version() {
        let ua = super::declare_browser_engine(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
            26,
            1,
        );
        assert!(ua.ends_with(" Version/26.1 Safari/605.1.15"));
        assert!(ua.contains("AppleWebKit/605.1.15"));
    }

    #[test]
    fn edge_and_existing_safari_are_not_rewritten() {
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36 Edg/128.0.0.0";
        assert_eq!(super::declare_browser_engine(edge, 11, 0), edge);
        let safari =
            "Mozilla/5.0 AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Safari/605.1.15";
        assert_eq!(super::declare_browser_engine(safari, 26, 0), safari);
    }
}
