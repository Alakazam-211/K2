//! Browser underlay, macOS (0.46.17): `browser_set_page_layers`.
//!
//! Beside today's path, behind the client setting `browserLayering`
//! (`underlay` | `legacy`, default underlay on macOS). In underlay the
//! browser pages are native views BELOW K2's UI webview; the UI view's
//! layer is masked open where a page shows and closed under K2's own
//! menus, toasts and the dock (`claims`), and input over a page is routed to
//! it (wry's `underlay` module, K2's fork). Windows and Linux keep today's
//! path; the command says so.
//!
//! One whole `PageLayers` set per K2 (OS) window per change, from the
//! renderer (`src/renderer/lib/page-layers.ts`). Native side:
//! - sets with a generation at or below the last one for that window are
//!   dropped;
//! - a burst is coalesced: one main-thread apply takes the newest set;
//! - one apply = one main-thread turn in one CATransaction: place each live
//!   page's frame directly (not Tauri's deferred `set_size`), order the
//!   pages below the UI view in place, hand routing and the mask to wry;
//! - the viewport guard: a set measured at a UI size that isn't the
//!   current one moves nothing (the autoresizing anchors hold the pages
//!   until the renderer's next set);
//! - a page in `still` (drag, motion) is never resized;
//! - a page's size changes at most once per frame (a trailing apply takes
//!   the rest), which keeps WebKit's backing-store churn down in a resize
//!   storm.
//!
//! If any of that fails the command errors, and the renderer switches the
//! window back to the legacy path (`browser_underlay_clear`) and logs it.
//! `browser_set_bounds` keeps working for the legacy path.
//!
//! Patch plan: k2-browser-lab wiki "Patch plan - Tauri underlay for K2's fork".

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

/// A number or `[tl, tr, br, bl]`.
#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(untagged)]
pub enum Corners {
    One(f64),
    Four([f64; 4]),
}

pub fn corners(c: Option<Corners>) -> [f64; 4] {
    match c {
        Some(Corners::One(v)) => [v; 4],
        Some(Corners::Four(a)) => a,
        None => [0.0; 4],
    }
}

/// Window CSS px from the top-left of the K2 window's content area.
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WireShape {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub radius: Option<Corners>,
    #[serde(default)]
    pub chamfer: Option<Corners>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WireLayer {
    /// The browser view: K2's item id (the registry key with the parent).
    pub window_id: String,
    pub z: f64,
    pub rect: WireShape,
    #[serde(default)]
    pub minus: Vec<WireShape>,
    /// `live` | `still`. Anything else reads as still (K2 keeps input).
    pub mode: String,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WireClaim {
    #[serde(flatten)]
    pub shape: WireShape,
    #[serde(default)]
    pub id: Option<f64>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Native frost under this claim: a later spike; ignored here.
    #[serde(default)]
    pub frost: Option<bool>,
}

/// K2 Design's PageLayers (patch plan §2f).
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PageLayers {
    #[serde(default)]
    pub parent_window: Option<String>,
    pub generation: u64,
    #[serde(default)]
    pub dpr: Option<f64>,
    /// The UI page size the rects were measured at.
    #[serde(default)]
    pub viewport: Option<[f64; 2]>,
    /// Back to front.
    pub layers: Vec<WireLayer>,
    #[serde(default)]
    pub claims: Vec<WireClaim>,
}

/// K2's own switches that ride with a set (not part of Design's shape).
#[derive(serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnderlayOptions {
    /// The private `WKMouseTrackingObserver` hover fallback. Off by default.
    #[serde(default)]
    pub hover_private_webkit_fallback: bool,
}

/// Is this set measured at a size other than the UI view's current one?
pub fn viewport_stale(viewport: Option<[f64; 2]>, current: (f64, f64)) -> bool {
    match viewport {
        Some([w, h]) => (w - current.0).abs() > 1.0 || (h - current.1).abs() > 1.0,
        None => false,
    }
}

/// Layers back to front by `z` (stable for equal z: wire order).
pub fn back_to_front(layers: &[WireLayer]) -> Vec<&WireLayer> {
    let mut v: Vec<&WireLayer> = layers.iter().collect();
    v.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Generations per window, and the newest set waiting for the main thread.
#[derive(Default)]
pub struct Queue {
    last_gen: HashMap<String, u64>,
    pending: HashMap<String, (PageLayers, UnderlayOptions)>,
    scheduled: HashSet<String>,
    /// UI views (by window label) and pages (by registry key) already kept
    /// on the main thread.
    known: HashSet<String>,
}

/// What `offer` decided for one incoming set.
#[derive(Debug, PartialEq, Eq)]
pub enum Offer {
    /// Older than (or the same as) one already seen: ignored.
    Stale,
    /// Folded into an apply that is already queued.
    Coalesced,
    /// Queue a main-thread apply for this window.
    Schedule,
}

impl Queue {
    pub fn offer(&mut self, parent: &str, set: PageLayers, opts: UnderlayOptions) -> Offer {
        if let Some(last) = self.last_gen.get(parent) {
            if set.generation <= *last {
                return Offer::Stale;
            }
        }
        self.last_gen.insert(parent.to_string(), set.generation);
        self.pending.insert(parent.to_string(), (set, opts));
        if self.scheduled.insert(parent.to_string()) {
            Offer::Schedule
        } else {
            Offer::Coalesced
        }
    }

    /// The newest set for the window, taken by the apply.
    pub fn take(&mut self, parent: &str) -> Option<(PageLayers, UnderlayOptions)> {
        self.scheduled.remove(parent);
        self.pending.remove(parent)
    }

    /// Back to legacy: forget the window (a later underlay starts afresh).
    pub fn reset(&mut self, parent: &str) {
        self.last_gen.remove(parent);
        self.pending.remove(parent);
        self.scheduled.remove(parent);
    }
}

pub struct PageLayersQueue(pub Mutex<Queue>);

pub fn init(app: &AppHandle) {
    app.manage(PageLayersQueue(Mutex::new(Queue::default())));
}

fn queue(app: &AppHandle) -> tauri::State<'_, PageLayersQueue> {
    app.state::<PageLayersQueue>()
}

#[cfg(target_os = "macos")]
pub mod mac {
    //! Main-thread state and the apply. Views are kept as retained
    //! `NSView`s in thread-locals: nothing AppKit crosses threads.
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use tauri::{AppHandle, Manager, Webview};
    use wry::underlay::{self, PageSpec, Shape, WindowSpec};

    use super::{back_to_front, corners, viewport_stale, PageLayers, UnderlayOptions, WireShape};

    thread_local! {
        /// K2 window label -> its UI (content) webview.
        static UIS: RefCell<HashMap<String, Retained<NSView>>> = RefCell::new(HashMap::new());
        /// Registry key (`parent\0item`) -> the page's WKWebView.
        static PAGES: RefCell<HashMap<String, Retained<NSView>>> = RefCell::new(HashMap::new());
        /// Page -> when its size last changed (one size change per frame).
        static SIZED: RefCell<HashMap<usize, Instant>> = RefCell::new(HashMap::new());
        /// The hover route last installed (None = not yet).
        static HOVER: RefCell<Option<bool>> = const { RefCell::new(None) };
    }

    /// One frame at 60 Hz, a little under so a steady 60 fps stream passes.
    const FRAME: Duration = Duration::from_millis(14);

    fn shape(s: &WireShape) -> Option<Shape> {
        Shape::new(s.x, s.y, s.width, s.height, corners(s.radius), corners(s.chamfer))
    }

    /// Keep a webview's NSView on the main thread under `slot`. Awaited
    /// (bounded) so a set right after create finds it.
    async fn cache(view: &Webview, f: impl FnOnce(Retained<NSView>) + Send + 'static) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        view.with_webview(move |pw| {
            let p = pw.inner() as *mut NSView;
            // SAFETY: inner() is the live WKWebView tauri owns; main thread.
            if let Some(v) = unsafe { Retained::retain(p) } {
                f(v);
            }
            let _ = tx.send(());
        })
        .map_err(|e| e.to_string())?;
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(())) => Ok(()),
            _ => Err("underlay: the webview did not answer".into()),
        }
    }

    pub async fn cache_ui(app: &AppHandle, parent: &str) -> Result<(), String> {
        let ui = app
            .get_webview(parent)
            .ok_or_else(|| format!("underlay: no UI webview for window {parent:?}"))?;
        let label = parent.to_string();
        cache(&ui, move |v| UIS.with(|m| {
            m.borrow_mut().insert(label, v);
        }))
        .await
    }

    pub async fn cache_page(view: &Webview, key: &str) -> Result<(), String> {
        let k = key.to_string();
        cache(view, move |v| PAGES.with(|m| {
            m.borrow_mut().insert(k, v);
        }))
        .await?;
        super::queue(view.app_handle()).0.lock().unwrap().known.insert(key.to_string());
        Ok(())
    }

    /// Before a page closes: give it back its tracking areas, stop routing
    /// to it and drop our reference. Posted ahead of the close.
    pub fn forget_page(app: &AppHandle, key: &str) {
        super::queue(app).0.lock().unwrap().known.remove(key);
        let key = key.to_string();
        let _ = app.run_on_main_thread(move || {
            if let Some(v) = PAGES.with(|m| m.borrow_mut().remove(&key)) {
                underlay::forget_view(&v);
                SIZED.with(|s| s.borrow_mut().remove(&(Retained::as_ptr(&v) as usize)));
            }
        });
    }

    /// New pages of this window go in below the UI view (underlay) or on
    /// top (legacy). Main thread.
    pub fn set_children_below(parent: &str, on: bool) -> Result<(), String> {
        let ui = UIS.with(|m| m.borrow().get(parent).cloned()).ok_or("underlay: UI view not known")?;
        underlay::set_children_below(&ui, on);
        Ok(())
    }

    fn pages_of(parent: &str) -> Vec<Retained<NSView>> {
        let prefix = format!("{parent}\0");
        PAGES.with(|m| m.borrow().iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| v.clone()).collect())
    }

    /// Back to legacy for one window: no mask, stock routing, pages on top.
    pub fn clear(parent: &str) -> Result<(), String> {
        let ui = UIS.with(|m| m.borrow().get(parent).cloned()).ok_or("underlay: UI view not known")?;
        underlay::clear_window(&ui);
        underlay::order(&ui, &pages_of(parent), false)
    }

    fn set_hover_route(private: bool) {
        let changed = HOVER.with(|h| {
            let mut h = h.borrow_mut();
            if *h == Some(private) {
                false
            } else {
                *h = Some(private);
                true
            }
        });
        if changed {
            log::info!("[browser-underlay] {}", underlay::set_hover_route(private));
        }
    }

    fn transaction(begin: bool) {
        let Some(tx) = AnyClass::get(c"CATransaction") else { return };
        unsafe {
            if begin {
                let _: () = msg_send![tx, begin];
                let _: () = msg_send![tx, setDisableActions: true];
            } else {
                let _: () = msg_send![tx, commit];
            }
        }
    }

    /// One set, one main-thread turn. Returns true when a page's size
    /// change was held back to the next frame (the caller re-applies).
    pub fn apply(parent: &str, set: &PageLayers, opts: UnderlayOptions) -> Result<bool, String> {
        if MainThreadMarker::new().is_none() {
            return Err("underlay: apply off the main thread".into());
        }
        set_hover_route(opts.hover_private_webkit_fallback);
        let ui = UIS.with(|m| m.borrow().get(parent).cloned()).ok_or("underlay: UI view not known")?;
        // SAFETY: AppKit getter on the main thread.
        let sv = unsafe { ui.superview() }.ok_or("underlay: the UI view has no superview")?;
        let ub = ui.bounds();
        let (uw, uh) = (ub.size.width, ub.size.height);
        let stale = viewport_stale(set.viewport, (uw, uh));
        let now = Instant::now();
        let mut held = false;
        transaction(true);
        let result = (|| {
            let mut back_first: Vec<Retained<NSView>> = Vec::new();
            let mut specs: Vec<PageSpec> = Vec::new();
            for layer in back_to_front(&set.layers) {
                let key = format!("{parent}\0{}", layer.window_id);
                let Some(view) = PAGES.with(|m| m.borrow().get(&key).cloned()) else { continue };
                let live = layer.mode == "live";
                let Some(rect) = shape(&layer.rect) else { continue };
                // Still (drag, motion): never resized. Stale: the anchors hold.
                if live && !stale {
                    held |= place(&view, &ui, &sv, &rect, (uw, uh), now);
                }
                back_first.push(view.clone());
                specs.push(PageSpec {
                    view,
                    corners: rect,
                    minus: layer.minus.iter().filter_map(shape).collect(),
                    live,
                });
            }
            underlay::order(&ui, &back_first, true)?;
            specs.reverse(); // wry wants front first
            underlay::set_window(WindowSpec {
                ui: ui.clone(),
                pages: specs,
                claims: set.claims.iter().filter_map(|c| shape(&c.shape)).collect(),
            })
        })();
        transaction(false);
        result.map(|()| held)
    }

    /// Frame a page at `rect` (page px) in its superview. Returns true when
    /// the size change waits for the next frame.
    fn place(view: &NSView, ui: &NSView, sv: &NSView, rect: &Shape, ui_size: (f64, f64), now: Instant) -> bool {
        let top = if ui.isFlipped() { rect.y } else { ui_size.1 - rect.y - rect.h };
        let in_ui = NSRect::new(NSPoint::new(rect.x, top), NSSize::new(rect.w, rect.h));
        let want = sv.convertRect_fromView(in_ui, Some(ui));
        let cur = view.frame();
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        let same_size = near(cur.size.width, want.size.width) && near(cur.size.height, want.size.height);
        let same_origin = near(cur.origin.x, want.origin.x) && near(cur.origin.y, want.origin.y);
        let key = view as *const NSView as usize;
        let mut held = false;
        if !same_size {
            let recent = SIZED.with(|s| s.borrow().get(&key).is_some_and(|t| now.duration_since(*t) < FRAME));
            if recent {
                held = true;
                if !same_origin {
                    view.setFrameOrigin(want.origin);
                }
            } else {
                SIZED.with(|s| s.borrow_mut().insert(key, now));
                view.setFrame(want);
            }
        } else if !same_origin {
            view.setFrameOrigin(want.origin);
        }
        // Follow the window's right/bottom edge during a live resize until
        // the renderer's next set (same rule as browser_set_bounds).
        let right = (ui_size.0 - (rect.x + rect.w)).abs() < 1.0;
        let bottom = (ui_size.1 - (rect.y + rect.h)).abs() < 1.0;
        let flipped = sv.isFlipped();
        let mut mask = if right {
            NSAutoresizingMaskOptions::ViewWidthSizable
        } else {
            NSAutoresizingMaskOptions::ViewMaxXMargin
        };
        mask |= if bottom {
            NSAutoresizingMaskOptions::ViewHeightSizable
        } else if flipped {
            NSAutoresizingMaskOptions::ViewMaxYMargin
        } else {
            NSAutoresizingMaskOptions::ViewMinYMargin
        };
        if view.autoresizingMask() != mask {
            view.setAutoresizingMask(mask);
        }
        held
    }
}

/// Apply the newest set for `parent` on the main thread; a size change
/// held back for a frame schedules one trailing apply of the same set.
#[cfg(target_os = "macos")]
fn schedule_apply(app: &AppHandle, parent: String, tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>) {
    let app2 = app.clone();
    let queued = app.run_on_main_thread(move || {
        let taken = queue(&app2).0.lock().unwrap().take(&parent);
        let Some((set, opts)) = taken else {
            if let Some(tx) = tx {
                let _ = tx.send(Ok(()));
            }
            return;
        };
        let r = mac::apply(&parent, &set, opts);
        let held = matches!(r, Ok(true));
        if let Err(e) = &r {
            log::warn!("[browser-underlay] apply failed for {parent:?}: {e}");
        }
        if held {
            // Put the set back unless a newer one arrived, and re-apply
            // after a frame.
            let again = {
                let q = queue(&app2);
                let mut q = q.0.lock().unwrap();
                let newer = q.pending.contains_key(&parent);
                if !newer {
                    q.pending.insert(parent.clone(), (set, opts));
                }
                q.scheduled.insert(parent.clone())
            };
            if again {
                let app3 = app2.clone();
                let p = parent.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                    schedule_apply(&app3, p, None);
                });
            }
        }
        if let Some(tx) = tx {
            let _ = tx.send(r.map(|_| ()));
        }
    });
    if let Err(e) = queued {
        log::warn!("[browser-underlay] could not reach the main thread: {e}");
    }
}

/// The whole layer set for one K2 window (macOS underlay).
#[tauri::command]
pub async fn browser_set_page_layers(
    app: AppHandle,
    layers: PageLayers,
    options: Option<UnderlayOptions>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let parent = match layers.parent_window.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "main".to_string(),
        };
        if !queue(&app).0.lock().unwrap().known.contains(&parent) {
            mac::cache_ui(&app, &parent).await?;
            queue(&app).0.lock().unwrap().known.insert(parent.clone());
        }
        // Pages created before underlay came on (a setting flip) or adopted.
        for l in &layers.layers {
            let key = format!("{parent}\0{}", l.window_id);
            if queue(&app).0.lock().unwrap().known.contains(&key) {
                continue;
            }
            if let Some(view) = super::browser_webviews::view_by_key(&app, &key) {
                if let Err(e) = mac::cache_page(&view, &key).await {
                    log::warn!("[browser-underlay] {key:?}: {e}");
                }
            }
        }
        let offer = queue(&app).0.lock().unwrap().offer(&parent, layers, options.unwrap_or_default());
        match offer {
            Offer::Stale | Offer::Coalesced => Ok(()),
            Offer::Schedule => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                schedule_apply(&app, parent, Some(tx));
                match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
                    Ok(Ok(r)) => r,
                    _ => Err("underlay: the main thread did not answer".into()),
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, layers, options);
        Err("the browser underlay is macOS only; this system keeps the current browser path".into())
    }
}

/// Back to the legacy path for one K2 window: no mask, pages on top.
#[tauri::command]
pub async fn browser_underlay_clear(app: AppHandle, parent_window: Option<String>) -> Result<(), String> {
    let parent = match parent_window.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "main".to_string(),
    };
    queue(&app).0.lock().unwrap().reset(&parent);
    #[cfg(target_os = "macos")]
    {
        mac::cache_ui(&app, &parent).await?;
        queue(&app).0.lock().unwrap().known.insert(parent.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let p = parent.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(mac::clear(&p));
        })
        .map_err(|e| e.to_string())?;
        let r = match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(r)) => r,
            _ => Err("underlay: the main thread did not answer".into()),
        };
        log::info!("[browser-underlay] {parent:?} back to the legacy path: {r:?}");
        r
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(gen: u64) -> PageLayers {
        serde_json::from_value(serde_json::json!({
            "parentWindow": "main",
            "generation": gen,
            "dpr": 2,
            "viewport": [1200, 800],
            "layers": [
                {"windowId": "b", "z": 2, "rect": {"x": 600, "y": 40, "width": 600, "height": 760, "radius": [0, 0, 16, 0]}, "minus": [], "mode": "live"},
                {"windowId": "a", "z": 1, "rect": {"x": 0, "y": 40, "width": 600, "height": 760, "radius": 16}, "mode": "still"}
            ],
            "claims": [{"id": 3, "kind": "menu", "x": 10, "y": 50, "width": 100, "height": 200, "chamfer": 6}]
        }))
        .expect("PageLayers parses")
    }

    #[test]
    fn page_layers_parse_design_shape() {
        let s = set(1);
        assert_eq!(s.layers.len(), 2);
        assert_eq!(corners(s.layers[0].rect.radius), [0.0, 0.0, 16.0, 0.0]);
        assert_eq!(corners(s.layers[1].rect.radius), [16.0; 4]);
        assert_eq!(corners(s.claims[0].shape.chamfer), [6.0; 4]);
        assert_eq!(s.claims[0].kind.as_deref(), Some("menu"));
        assert_eq!(s.viewport, Some([1200.0, 800.0]));
    }

    #[test]
    fn layers_sort_back_to_front_by_z() {
        let s = set(1);
        let ids: Vec<&str> = back_to_front(&s.layers).iter().map(|l| l.window_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn older_generations_drop_and_bursts_coalesce_per_window() {
        let mut q = Queue::default();
        let o = UnderlayOptions::default();
        assert_eq!(q.offer("main", set(5), o), Offer::Schedule);
        assert_eq!(q.offer("main", set(6), o), Offer::Coalesced, "one apply takes the newest");
        assert_eq!(q.offer("main", set(6), o), Offer::Stale, "same generation again");
        assert_eq!(q.offer("main", set(4), o), Offer::Stale, "older");
        assert_eq!(q.offer("window-2", set(1), o), Offer::Schedule, "generations are per window");
        let (taken, _) = q.take("main").expect("pending set");
        assert_eq!(taken.generation, 6);
        assert!(q.take("main").is_none());
        assert_eq!(q.offer("main", set(7), o), Offer::Schedule);
        q.reset("main");
        assert_eq!(q.offer("main", set(1), o), Offer::Schedule, "after legacy, a fresh start is accepted");
    }

    #[test]
    fn viewport_guard() {
        assert!(!viewport_stale(Some([1200.0, 800.0]), (1200.0, 800.5)));
        assert!(viewport_stale(Some([1200.0, 800.0]), (1180.0, 800.0)));
        assert!(!viewport_stale(None, (1.0, 1.0)));
    }

    #[test]
    fn options_default_to_the_public_hover_route() {
        let o: UnderlayOptions = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!o.hover_private_webkit_fallback);
    }
}
