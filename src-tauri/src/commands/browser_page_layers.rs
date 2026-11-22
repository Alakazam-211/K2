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

/// `id`, `kind` and `frost` are Design's wire fields: routing and the mask
/// read the shape only (frost is a later spike).
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
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
    /// K2's own field: one id per renderer page load (and per return to
    /// underlay). `generation` counts within it; a new epoch starts afresh,
    /// so a reloaded UI is never stuck behind its old generation.
    #[serde(default)]
    pub epoch: Option<String>,
    pub generation: u64,
    /// Sent by Design's shape; native placement works in points.
    #[serde(default)]
    #[allow(dead_code)]
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
    /// The first click on a page in an inactive window also reaches the
    /// page. Off by default (Design: it only activates the window).
    #[serde(default)]
    pub page_first_click: bool,
}

/// Caps on what the renderer may send (it is not trusted to be sane).
pub const MAX_LAYERS: usize = 32;
pub const MAX_CLAIMS: usize = 256;
pub const MAX_MINUS: usize = 64;
/// Shapes may reach this far past the UI view's edges.
pub const CLAMP_MARGIN: f64 = 64.0;
/// This many stale sets in a row (same epoch, older generation) and the
/// window falls back to legacy: the renderer and K2 disagree.
pub const STALE_LIMIT: u32 = 8;
/// This many 5 s main-thread timeouts in a row and the window falls back.
pub const TIMEOUT_LIMIT: u32 = 3;
/// This many sets in a row at a viewport other than the UI view's size is
/// more than a resize frame: logged once.
pub const VIEWPORT_MISS_LIMIT: u32 = 30;

/// Cut a set down to the caps. True when anything was dropped.
pub fn cap_counts(set: &mut PageLayers) -> bool {
    let mut cut = false;
    if set.layers.len() > MAX_LAYERS {
        // Keep the FRONT ones (highest z): those are what a person sees.
        set.layers.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
        let extra = set.layers.len() - MAX_LAYERS;
        set.layers.drain(..extra);
        cut = true;
    }
    if set.claims.len() > MAX_CLAIMS {
        set.claims.truncate(MAX_CLAIMS);
        cut = true;
    }
    for l in &mut set.layers {
        if l.minus.len() > MAX_MINUS {
            l.minus.truncate(MAX_MINUS);
            cut = true;
        }
    }
    cut
}

/// A shape clipped to the UI view plus a margin; None when it is not
/// finite or nothing is left.
pub fn clamp_shape(s: &WireShape, ui: (f64, f64)) -> Option<WireShape> {
    if ![s.x, s.y, s.width, s.height].iter().all(|v| v.is_finite()) {
        return None;
    }
    let x0 = s.x.max(-CLAMP_MARGIN);
    let y0 = s.y.max(-CLAMP_MARGIN);
    let x1 = (s.x + s.width).min(ui.0 + CLAMP_MARGIN);
    let y1 = (s.y + s.height).min(ui.1 + CLAMP_MARGIN);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(WireShape { x: x0, y: y0, width: x1 - x0, height: y1 - y0, radius: s.radius, chamfer: s.chamfer })
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

/// Epochs and generations per window, and the newest set waiting for the
/// main thread.
#[derive(Default)]
pub struct Queue {
    last_gen: HashMap<String, u64>,
    /// The renderer epoch each window's generations belong to.
    epoch: HashMap<String, String>,
    /// The epoch a window left underlay in: its late sets are ignored.
    cleared: HashMap<String, String>,
    /// Stale sets in a row, per window.
    stale: HashMap<String, u32>,
    /// Main-thread timeouts in a row, per window.
    timeouts: HashMap<String, u32>,
    pending: HashMap<String, (PageLayers, UnderlayOptions)>,
    /// Bumped each time a window label closes: work begun for the old
    /// window (an in-flight set, a webview cache) sees a different number
    /// and drops itself, so a closed window is never revived and a reused
    /// label starts clean.
    label_gen: HashMap<String, u64>,
    scheduled: HashSet<String>,
    /// UI views (by window label) and pages (by registry key) already kept
    /// on the main thread.
    known: HashSet<String>,
}

/// What `offer` decided for one incoming set.
#[derive(Debug, PartialEq, Eq)]
pub enum Offer {
    /// Older than (or the same as) one already seen in this epoch: ignored,
    /// and counted (the count in a row).
    Stale(u32),
    /// From an epoch that already went back to legacy (a set in flight
    /// when the renderer switched): ignored.
    Refused,
    /// Folded into an apply that is already queued.
    Coalesced,
    /// Queue a main-thread apply for this window.
    Schedule,
}

impl Queue {
    pub fn offer(&mut self, parent: &str, set: PageLayers, opts: UnderlayOptions) -> Offer {
        let ep = set.epoch.clone().unwrap_or_default();
        if self.cleared.get(parent) == Some(&ep) {
            return Offer::Refused;
        }
        if self.epoch.get(parent) != Some(&ep) {
            // A reload (or a return to underlay): its generations start over.
            self.epoch.insert(parent.to_string(), ep);
            self.last_gen.remove(parent);
            self.stale.remove(parent);
            self.cleared.remove(parent);
        }
        if let Some(last) = self.last_gen.get(parent) {
            if set.generation <= *last {
                let n = self.stale.entry(parent.to_string()).or_insert(0);
                *n += 1;
                return Offer::Stale(*n);
            }
        }
        self.stale.remove(parent);
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

    /// Back to legacy: forget the window's sets, and ignore any still in
    /// flight from `epoch` (default: the current one). A later underlay
    /// comes with a new epoch.
    pub fn clear(&mut self, parent: &str, epoch: Option<String>) {
        let ep = epoch.or_else(|| self.epoch.get(parent).cloned()).unwrap_or_default();
        self.cleared.insert(parent.to_string(), ep);
        self.epoch.remove(parent);
        self.last_gen.remove(parent);
        self.stale.remove(parent);
        self.timeouts.remove(parent);
        self.pending.remove(parent);
        self.scheduled.remove(parent);
    }

    /// The label's generation (see `label_gen`).
    pub fn label_gen(&self, label: &str) -> u64 {
        self.label_gen.get(label).copied().unwrap_or(0)
    }

    /// Is the window in underlay (a live epoch, not cleared)?
    pub fn underlay(&self, parent: &str) -> bool {
        self.epoch.contains_key(parent)
    }

    /// One more 5 s timeout in a row; returns the count.
    pub fn timed_out(&mut self, parent: &str) -> u32 {
        let n = self.timeouts.entry(parent.to_string()).or_insert(0);
        *n += 1;
        *n
    }

    pub fn answered(&mut self, parent: &str) {
        self.timeouts.remove(parent);
    }

    /// The K2 window closed: drop everything about it (a reused label then
    /// starts clean).
    pub fn forget_window(&mut self, parent: &str) {
        *self.label_gen.entry(parent.to_string()).or_insert(0) += 1;
        // Late sets of the closed window's epoch stay refused.
        match self.epoch.remove(parent) {
            Some(ep) => {
                self.cleared.insert(parent.to_string(), ep);
            }
            None => {
                self.cleared.remove(parent);
            }
        }
        self.last_gen.remove(parent);
        self.stale.remove(parent);
        self.timeouts.remove(parent);
        self.pending.remove(parent);
        self.scheduled.remove(parent);
        let prefix = format!("{parent}\0");
        self.known.retain(|k| k != parent && !k.starts_with(&prefix));
    }
}


pub struct PageLayersQueue(pub Mutex<Queue>);

pub fn init(app: &AppHandle) {
    app.manage(PageLayersQueue(Mutex::new(Queue::default())));
    // The hover route, logged at startup: public unless a set asks for the
    // named private fallback (then logged again).
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(mac::log_hover_route_at_startup);
    }
}

/// Is this K2 window in underlay right now? (browser_set_bounds then leaves
/// its pages to the layer sets: Tauri's deferred bounds could land after a
/// newer set.)
pub fn underlay_window(app: &AppHandle, parent: &str) -> bool {
    app.try_state::<PageLayersQueue>().is_some_and(|q| q.0.lock().unwrap().underlay(parent))
}

/// A K2 window closed: forget it here and on the main thread, so its UI
/// webview (and its WebContent process) can go.
pub fn window_destroyed(app: &AppHandle, label: &str) {
    if let Some(q) = app.try_state::<PageLayersQueue>() {
        q.0.lock().unwrap().forget_window(label);
    }
    #[cfg(target_os = "macos")]
    {
        let label = label.to_string();
        let _ = app.run_on_main_thread(move || mac::window_destroyed(&label));
    }
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

    use super::{back_to_front, clamp_shape, corners, viewport_stale, PageLayers, UnderlayOptions, WireShape};

    thread_local! {
        /// K2 window label -> its UI (content) webview.
        static UIS: RefCell<HashMap<String, Retained<NSView>>> = RefCell::new(HashMap::new());
        /// Registry key (`parent\0item`) -> the page's WKWebView.
        static PAGES: RefCell<HashMap<String, Retained<NSView>>> = RefCell::new(HashMap::new());
        /// Page -> when its size last changed (one size change per frame).
        static SIZED: RefCell<HashMap<usize, Instant>> = RefCell::new(HashMap::new());
        /// The hover route last installed (None = not yet).
        static HOVER: RefCell<Option<bool>> = const { RefCell::new(None) };
        /// Sets in a row measured at a viewport other than the UI view's
        /// size, per window, and whether that was logged.
        static VIEWPORT_MISS: RefCell<HashMap<String, (u32, bool)>> = RefCell::new(HashMap::new());
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

    /// The window label a key belongs to (`label\0item`, or the label).
    fn label_of(key: &str) -> &str {
        key.split('\0').next().unwrap_or(key)
    }

    /// Keep `v` under `key` unless its window closed since `gen` was read
    /// (then it would leak: the close already cleaned up).
    fn keep_if_open(app: &AppHandle, key: &str, gen: u64, keep: impl FnOnce()) -> bool {
        let open = super::queue(app).0.lock().unwrap().label_gen(label_of(key)) == gen;
        if open {
            keep();
        }
        open
    }

    pub async fn cache_ui(app: &AppHandle, parent: &str) -> Result<(), String> {
        let ui = app
            .get_webview(parent)
            .ok_or_else(|| format!("underlay: no UI webview for window {parent:?}"))?;
        let gen = super::queue(app).0.lock().unwrap().label_gen(parent);
        let label = parent.to_string();
        let app2 = app.clone();
        cache(&ui, move |v| {
            let k = label.clone();
            keep_if_open(&app2, &label, gen, move || UIS.with(|m| {
                m.borrow_mut().insert(k, v);
            }));
        })
        .await
    }

    pub async fn cache_page(view: &Webview, key: &str) -> Result<(), String> {
        let app = view.app_handle().clone();
        let gen = super::queue(&app).0.lock().unwrap().label_gen(label_of(key));
        let k = key.to_string();
        let app2 = app.clone();
        cache(view, move |v| {
            let kk = k.clone();
            keep_if_open(&app2, &k, gen, move || PAGES.with(|m| {
                m.borrow_mut().insert(kk, v);
            }));
        })
        .await?;
        let mut q = super::queue(&app).0.lock().unwrap();
        if q.label_gen(label_of(key)) == gen {
            q.known.insert(key.to_string());
        }
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

    /// Main thread, at startup: log the hover route in use.
    pub fn log_hover_route_at_startup() {
        set_hover_route(false);
    }

    /// The K2 window `label` closed (main thread): drop its UI view and
    /// pages from here and from wry's registry.
    pub fn window_destroyed(label: &str) {
        let prefix = format!("{label}\0");
        let pages: Vec<Retained<NSView>> = PAGES.with(|m| {
            let mut m = m.borrow_mut();
            let keys: Vec<String> = m.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            keys.into_iter().filter_map(|k| m.remove(&k)).collect()
        });
        SIZED.with(|s| {
            let mut s = s.borrow_mut();
            for v in &pages {
                s.remove(&(Retained::as_ptr(v) as usize));
            }
        });
        if let Some(ui) = UIS.with(|m| m.borrow_mut().remove(label)) {
            underlay::drop_ui(&ui);
        }
        log::info!("[browser-underlay] window {label:?} closed: its UI view and {} page(s) let go", pages.len());
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
        underlay::set_page_first_mouse(opts.page_first_click);
        let ui = UIS.with(|m| m.borrow().get(parent).cloned()).ok_or("underlay: UI view not known")?;
        // SAFETY: AppKit getter on the main thread.
        let sv = unsafe { ui.superview() }.ok_or("underlay: the UI view has no superview")?;
        let ub = ui.bounds();
        let (uw, uh) = (ub.size.width, ub.size.height);
        let stale = viewport_stale(set.viewport, (uw, uh));
        note_viewport(parent, stale, set.viewport, (uw, uh));
        let now = Instant::now();
        let mut held = false;
        transaction(true);
        let clip = |s: &WireShape| clamp_shape(s, (uw, uh)).and_then(|c| shape(&c));
        let result = (|| {
            let mut set_pages: Vec<Retained<NSView>> = Vec::new();
            let mut specs: Vec<PageSpec> = Vec::new();
            for layer in back_to_front(&set.layers) {
                let key = format!("{parent}\0{}", layer.window_id);
                let Some(view) = PAGES.with(|m| m.borrow().get(&key).cloned()) else { continue };
                let live = layer.mode == "live";
                let Some(rect) = clip(&layer.rect) else { continue };
                // Still (drag, motion): never resized. Stale: the anchors hold.
                if live && !stale {
                    held |= place(&view, &ui, &sv, &rect, (uw, uh), now);
                }
                set_pages.push(view.clone());
                specs.push(PageSpec {
                    view,
                    corners: rect,
                    minus: layer.minus.iter().filter_map(clip).collect(),
                    live,
                });
            }
            // Every page of this window goes below the UI view: the ones not
            // in the set (hidden tabs) furthest back, so one un-hiding never
            // flashes above K2 before its set arrives.
            let in_set: std::collections::HashSet<usize> = set_pages.iter().map(|v| Retained::as_ptr(v) as usize).collect();
            let mut back_first: Vec<Retained<NSView>> =
                pages_of(parent).into_iter().filter(|v| !in_set.contains(&(Retained::as_ptr(v) as usize))).collect();
            back_first.extend(set_pages);
            underlay::order(&ui, &back_first, true)?;
            specs.reverse(); // wry wants front first
            underlay::set_window(WindowSpec {
                ui: ui.clone(),
                pages: specs,
                claims: set.claims.iter().filter_map(|c| clip(&c.shape)).collect(),
            })
        })();
        transaction(false);
        result.map(|()| held)
    }

    /// A viewport that keeps disagreeing with the UI view (not one resize
    /// frame) means the renderer and K2 measure in different units: logged
    /// once per window until it agrees again.
    fn note_viewport(parent: &str, stale: bool, viewport: Option<[f64; 2]>, ui: (f64, f64)) {
        VIEWPORT_MISS.with(|m| {
            let mut m = m.borrow_mut();
            if !stale {
                m.remove(parent);
                return;
            }
            let e = m.entry(parent.to_string()).or_insert((0, false));
            e.0 += 1;
            if e.0 >= super::VIEWPORT_MISS_LIMIT && !e.1 {
                e.1 = true;
                log::warn!(
                    "[browser-underlay] {parent:?}: {} sets in a row measured at a viewport of {viewport:?}, but the UI view is {ui:?}: pages keep their frames until they agree",
                    e.0
                );
            }
        });
    }

    /// Frame a page at `rect` (UI px) in its superview. Returns true when
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
        let mut layers = layers;
        let parent = match layers.parent_window.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "main".to_string(),
        };
        // Only a K2 window's own UI webview, never a browser page's label.
        if app.get_webview_window(&parent).is_none() {
            return Err(format!("underlay: {parent:?} is not one of the app's own windows"));
        }
        let gen_at_start = queue(&app).0.lock().unwrap().label_gen(&parent);
        if cap_counts(&mut layers) {
            log::warn!(
                "[browser-underlay] {parent:?}: a set over the caps ({MAX_LAYERS} layers, {MAX_CLAIMS} claims, {MAX_MINUS} minus) was cut down"
            );
        }
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
        // The awaits above can outlive the window: a set for a window that
        // closed meanwhile must not bring its queue entries back.
        if app.get_webview_window(&parent).is_none() || queue(&app).0.lock().unwrap().label_gen(&parent) != gen_at_start {
            log::debug!("[browser-underlay] {parent:?} closed while its set was on the way; dropped");
            return Ok(());
        }
        let offer = queue(&app).0.lock().unwrap().offer(&parent, layers, options.unwrap_or_default());
        match offer {
            Offer::Coalesced => Ok(()),
            Offer::Refused => {
                log::debug!("[browser-underlay] {parent:?}: a set from before the switch to legacy, ignored");
                Ok(())
            }
            Offer::Stale(n) => {
                log::warn!("[browser-underlay] {parent:?}: a set older than one already applied ({n} in a row), ignored");
                if n >= STALE_LIMIT {
                    Err(format!("underlay: {n} sets in a row were older than one already applied"))
                } else {
                    Ok(())
                }
            }
            Offer::Schedule => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                schedule_apply(&app, parent.clone(), Some(tx));
                match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
                    Ok(Ok(r)) => {
                        queue(&app).0.lock().unwrap().answered(&parent);
                        r
                    }
                    // A long native menu, a modal or sleep can hold the main
                    // thread: the apply still runs when it frees. Only a run
                    // of these falls back.
                    _ => {
                        let n = queue(&app).0.lock().unwrap().timed_out(&parent);
                        log::warn!("[browser-underlay] {parent:?}: the main thread did not answer in 5 s ({n} in a row)");
                        if n >= TIMEOUT_LIMIT {
                            Err(format!("underlay: the main thread did not answer {n} times in a row"))
                        } else {
                            Ok(())
                        }
                    }
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
pub async fn browser_underlay_clear(
    app: AppHandle,
    parent_window: Option<String>,
    epoch: Option<String>,
) -> Result<(), String> {
    let parent = match parent_window.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "main".to_string(),
    };
    queue(&app).0.lock().unwrap().clear(&parent, epoch);
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
        set_in("load-1", gen)
    }

    fn set_in(epoch: &str, gen: u64) -> PageLayers {
        serde_json::from_value(serde_json::json!({
            "parentWindow": "main",
            "epoch": epoch,
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
        assert_eq!(q.offer("main", set(6), o), Offer::Stale(1), "same generation again");
        assert_eq!(q.offer("main", set(4), o), Offer::Stale(2), "older, counted in a row");
        assert_eq!(q.offer("window-2", set(1), o), Offer::Schedule, "generations are per window");
        let (taken, _) = q.take("main").expect("pending set");
        assert_eq!(taken.generation, 6);
        assert!(q.take("main").is_none());
        assert_eq!(q.offer("main", set(7), o), Offer::Schedule);
    }

    #[test]
    fn a_reloaded_ui_starts_a_new_epoch_and_its_sets_apply() {
        let mut q = Queue::default();
        let o = UnderlayOptions::default();
        assert_eq!(q.offer("main", set_in("load-1", 40), o), Offer::Schedule);
        q.take("main");
        // The UI reloads: its generation restarts at 1.
        assert_eq!(q.offer("main", set_in("load-2", 1), o), Offer::Schedule, "a new load is never stale");
        q.take("main");
        assert_eq!(q.offer("main", set_in("load-2", 2), o), Offer::Schedule);
    }

    #[test]
    fn stale_sets_count_in_a_row_and_a_fresh_one_resets_the_count() {
        let mut q = Queue::default();
        let o = UnderlayOptions::default();
        q.offer("main", set(10), o);
        q.take("main");
        for n in 1..=STALE_LIMIT {
            assert_eq!(q.offer("main", set(3), o), Offer::Stale(n));
        }
        assert_eq!(q.offer("main", set(11), o), Offer::Schedule);
        q.take("main");
        assert_eq!(q.offer("main", set(5), o), Offer::Stale(1), "the run starts over");
    }

    #[test]
    fn after_clear_late_sets_of_that_epoch_are_refused_and_a_new_epoch_is_accepted() {
        let mut q = Queue::default();
        let o = UnderlayOptions::default();
        q.offer("main", set_in("a", 5), o);
        q.take("main");
        assert!(q.underlay("main"));
        q.clear("main", Some("a".into()));
        assert!(!q.underlay("main"));
        assert_eq!(q.offer("main", set_in("a", 6), o), Offer::Refused, "in flight when the renderer switched");
        assert!(!q.underlay("main"));
        assert_eq!(q.offer("main", set_in("b", 1), o), Offer::Schedule, "back to underlay with a new epoch");
        assert!(q.underlay("main"));
    }

    #[test]
    fn timeouts_count_in_a_row_until_an_answer() {
        let mut q = Queue::default();
        assert_eq!(q.timed_out("main"), 1);
        assert_eq!(q.timed_out("main"), 2);
        q.answered("main");
        assert_eq!(q.timed_out("main"), 1);
    }

    #[test]
    fn a_closed_window_is_forgotten_and_a_reused_label_starts_clean() {
        let mut q = Queue::default();
        let o = UnderlayOptions::default();
        q.known.insert("window-2".into());
        q.known.insert("window-2\0tab".into());
        q.known.insert("main\0tab".into());
        q.offer("window-2", set_in("x", 9), o);
        let gen = q.label_gen("window-2");
        q.forget_window("window-2");
        assert!(!q.known.contains("window-2"));
        assert!(!q.known.contains("window-2\0tab"));
        assert!(q.known.contains("main\0tab"), "other windows keep theirs");
        assert!(!q.underlay("window-2"));
        assert!(q.take("window-2").is_none(), "nothing pending");
        assert_ne!(q.label_gen("window-2"), gen, "work begun for the old window can tell");
        assert_eq!(q.label_gen("main"), 0, "other labels untouched");
        // A set from the closed window, still in flight: not revived.
        assert_eq!(q.offer("window-2", set_in("x", 10), o), Offer::Refused);
        assert!(!q.underlay("window-2"));
        // A new window under the same label (a new renderer, a new epoch).
        assert_eq!(q.offer("window-2", set_in("y", 1), o), Offer::Schedule, "the reused label starts clean");
    }

    #[test]
    fn caps_keep_the_front_layers() {
        let mut s = set(1);
        let base = s.layers[0].clone();
        s.layers = (0..40)
            .map(|i| WireLayer { window_id: format!("p{i}"), z: f64::from(i), ..base.clone() })
            .collect();
        assert!(cap_counts(&mut s));
        assert_eq!(s.layers.len(), MAX_LAYERS);
        assert_eq!(s.layers.first().map(|l| l.window_id.as_str()), Some("p8"));
        assert_eq!(s.layers.last().map(|l| l.window_id.as_str()), Some("p39"), "the frontmost stays");
    }

    /// The real CoreGraphics path code the mask uses (macOS 13+).
    #[cfg(target_os = "macos")]
    #[test]
    fn mask_paths_handle_a_page_over_the_whole_ui_and_cut_in_z_order() {
        use wry::underlay::{visible_bounds, Shape};
        let sq = |x: f64, y: f64, w: f64, h: f64| Shape::new(x, y, w, h, [0.0; 4], [0.0; 4]).unwrap();
        // CoreGraphics' boolean operations may move an edge by a hair.
        let near = |got: Result<Option<(f64, f64, f64, f64)>, String>, want: (f64, f64, f64, f64)| {
            let g = got.expect("path operations").expect("something shows");
            let d = [g.0 - want.0, g.1 - want.1, g.2 - want.2, g.3 - want.3];
            assert!(d.iter().all(|v| v.abs() < 0.01), "bounds {g:?}, want {want:?}");
        };
        // A square page over the whole UI view: nothing of the UI shows,
        // and that is not a failure (CoreGraphics returns NULL here).
        assert_eq!(visible_bounds(800.0, 600.0, &[(sq(0.0, 0.0, 800.0, 600.0), vec![])], &[]), Ok(None));
        // ... and a menu over it shows exactly the menu.
        near(visible_bounds(800.0, 600.0, &[(sq(0.0, 0.0, 800.0, 600.0), vec![])], &[sq(10.0, 20.0, 100.0, 50.0)]), (10.0, 20.0, 100.0, 50.0));
        // A page's own minus shows only inside that page: a nested dialog
        // that pokes out past the page adds nothing outside it.
        near(visible_bounds(800.0, 600.0, &[(sq(0.0, 0.0, 800.0, 600.0), vec![sq(700.0, 500.0, 300.0, 300.0)])], &[]), (700.0, 500.0, 100.0, 100.0));
        // Overlapping pages: the union stays open (no box between them).
        let vis = visible_bounds(800.0, 600.0, &[(sq(0.0, 0.0, 400.0, 600.0), vec![]), (sq(300.0, 0.0, 500.0, 300.0), vec![])], &[]);
        near(vis, (400.0, 300.0, 400.0, 300.0)); // only the lower right stays UI
    }

    #[test]
    fn caps_cut_counts_and_clamp_shapes_to_the_ui() {
        let mut s = set(1);
        s.layers = (0..40).map(|_| s.layers[0].clone()).collect();
        s.layers[0].minus = (0..70).map(|_| s.layers[1].rect.clone()).collect();
        s.claims = (0..300).map(|_| s.claims[0].clone()).collect();
        assert!(cap_counts(&mut s));
        assert_eq!((s.layers.len(), s.claims.len(), s.layers[0].minus.len()), (MAX_LAYERS, MAX_CLAIMS, MAX_MINUS));
        assert!(!cap_counts(&mut s));
        let big = WireShape { x: -1e9, y: 10.0, width: 2e9, height: 1e12, radius: None, chamfer: None };
        let c = clamp_shape(&big, (1200.0, 800.0)).expect("clipped, not dropped");
        assert_eq!((c.x, c.y, c.width, c.height), (-CLAMP_MARGIN, 10.0, 1200.0 + 2.0 * CLAMP_MARGIN, 800.0 + CLAMP_MARGIN - 10.0));
        let off = WireShape { x: 5000.0, y: 0.0, width: 10.0, height: 10.0, radius: None, chamfer: None };
        assert!(clamp_shape(&off, (1200.0, 800.0)).is_none());
        let nan = WireShape { x: f64::NAN, y: 0.0, width: 10.0, height: 10.0, radius: None, chamfer: None };
        assert!(clamp_shape(&nan, (1200.0, 800.0)).is_none());
    }

    #[test]
    fn viewport_guard() {
        assert!(!viewport_stale(Some([1200.0, 800.0]), (1200.0, 800.5)));
        assert!(viewport_stale(Some([1200.0, 800.0]), (1180.0, 800.0)));
        assert!(!viewport_stale(None, (1.0, 1.0)));
    }

    #[test]
    fn options_default_to_the_public_hover_route_and_first_click_off() {
        let o: UnderlayOptions = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!o.hover_private_webkit_fallback);
        assert!(!o.page_first_click);
        let o: UnderlayOptions = serde_json::from_value(serde_json::json!({"pageFirstClick": true})).unwrap();
        assert!(o.page_first_click);
    }
}
