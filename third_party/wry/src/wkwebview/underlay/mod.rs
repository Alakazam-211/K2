// Copyright 2026 Alakazam Labs
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! K2 fork, macOS only: the browser **underlay**.
//!
//! K2's browser pages are child `WryWebView`s. In underlay they sit *below*
//! the window's UI webview (the content webview, `is_child == false`), and
//! the UI webview's layer is masked open where a live page shows, except
//! where K2 claims the area (menus, toasts, the dock). Input that lands on
//! the UI view is routed to the page that owns the point:
//!
//! - `hitTest:` passes through to the owning page (clicks, drags);
//! - scroll and gestures are forwarded, latched per gesture;
//! - hover: the view that doesn't own the pointer gets no moves. The default
//!   route is public API only (its tracking areas are lifted while it isn't
//!   the owner); `hover_private_webkit_fallback` swaps WebKit's private
//!   `WKMouseTrackingObserver` instead, off unless switched on.
//!
//! K2 (src-tauri `browser_webviews.rs`) hands in one whole set per window
//! with [`set_window`] and drops it with [`clear_window`]. A window with no
//! set behaves exactly as stock wry. Everything here runs on the main
//! thread; state is thread-local so no AppKit object crosses threads.
//!
//! Patch plan: k2-browser-lab wiki "Patch plan - Tauri underlay for K2's fork".

pub mod logic;
pub mod private_hover;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, MainThreadMarker, Message};
use objc2_app_kit::{
  NSEvent, NSEventType, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindowOrderingMode,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{ns_string, NSComparisonResult, NSPoint};

pub use logic::{Route, Shape};
use logic::{Latch, PageAt};

/// One browser page in a window's set.
pub struct PageSpec {
  /// The page's WKWebView (a child `WryWebView`).
  pub view: Retained<NSView>,
  /// Its corner radii / chamfers (position and size are read from the
  /// view's real frame).
  pub corners: Shape,
  /// K2 things drawn above this page, in page px.
  pub minus: Vec<Shape>,
  /// `live` (input and pixels are the page's) or `still` (K2 keeps both).
  pub live: bool,
}

/// The whole set for one window.
pub struct WindowSpec {
  /// The window's UI (content) webview.
  pub ui: Retained<NSView>,
  /// Front to back.
  pub pages: Vec<PageSpec>,
  /// K2 things that own input over pages (menus, the dock, toasts), page px.
  pub claims: Vec<Shape>,
}

struct Win {
  ui: Retained<NSView>,
  pages: Vec<PageSpec>,
  claims: Vec<Shape>,
  latch: Latch<usize>,
  hover_owner: Option<usize>,
}

/// Masks on UI views: (container, holes layer, claims layer).
struct Mask {
  container: Retained<AnyObject>,
  holes: Retained<AnyObject>,
  claims: Retained<AnyObject>,
}

thread_local! {
  /// Registered windows, keyed by NSWindow pointer.
  static WINS: RefCell<HashMap<usize, Win>> = RefCell::new(HashMap::new());
  /// Windows whose new child webviews are inserted below the UI view: the
  /// window's UI view, by NSWindow pointer.
  static BELOW: RefCell<HashMap<usize, Retained<NSView>>> = RefCell::new(HashMap::new());
  /// Tracking areas lifted off a view while it isn't the pointer's owner.
  static GATED: RefCell<HashMap<usize, Vec<Retained<NSTrackingArea>>>> = RefCell::new(HashMap::new());
  static MASKS: RefCell<HashMap<usize, Mask>> = RefCell::new(HashMap::new());
  /// Set while an event is being forwarded, so the receiver runs it as is.
  static IN_FWD: Cell<bool> = const { Cell::new(false) };
  /// Set while this module adds/removes tracking areas itself.
  static IN_GATE: Cell<bool> = const { Cell::new(false) };
  /// Logged once: overlapping holes were merged.
  static MERGE_LOGGED: Cell<bool> = const { Cell::new(false) };
  /// Set while a mask is being rebuilt (AppKit re-entering setFrameSize:).
  static REBUILDING: Cell<bool> = const { Cell::new(false) };
}

/// First click on a page in an inactive window: off (Design, matching
/// Safari): that click only activates the window. One switch.
static PAGE_FIRST_MOUSE: AtomicBool = AtomicBool::new(false);
/// Hover route: false = public (tracking areas lifted), true = the private
/// `WKMouseTrackingObserver` fallback.
static HOVER_PRIVATE: AtomicBool = AtomicBool::new(false);

fn ptr<T: ?Sized>(v: &T) -> usize {
  v as *const T as *const c_void as usize
}

fn window_key(view: &NSView) -> Option<usize> {
  view.window().map(|w| Retained::as_ptr(&w) as usize)
}

/// The view's backing layer (msg_send: wry's AppKit features leave out
/// QuartzCore).
fn layer_of(v: &NSView) -> Option<Retained<AnyObject>> {
  unsafe { msg_send![v, layer] }
}

fn mtm() -> Result<MainThreadMarker, String> {
  MainThreadMarker::new().ok_or_else(|| "underlay: not on the main thread".to_string())
}

// ---------------------------------------------------------------------------
// Public API (K2's src-tauri)
// ---------------------------------------------------------------------------

/// Install or replace a window's whole set: routing, the UI view's mask
/// (built from the pages' real frames) and the hover gates. Main thread.
pub fn set_window(spec: WindowSpec) -> Result<(), String> {
  mtm()?;
  let key = window_key(&spec.ui).ok_or("underlay: the UI view has no window")?;
  if layer_of(&spec.ui).is_none() {
    spec.ui.setWantsLayer(true);
  }
  if layer_of(&spec.ui).is_none() {
    return Err("underlay: the UI view has no layer to mask".into());
  }
  let (old_hover, dropped) = WINS.with(|w| {
    let mut w = w.borrow_mut();
    let prev = w.remove(&key);
    let (latch, hover, old_pages) = match prev {
      Some(p) => (p.latch, p.hover_owner, p.pages),
      None => (Latch::default(), None, Vec::new()),
    };
    let still_here: HashSet<usize> = spec.pages.iter().map(|p| ptr(&*p.view)).collect();
    let dropped: Vec<Retained<NSView>> =
      old_pages.into_iter().map(|p| p.view).filter(|v| !still_here.contains(&ptr(&**v))).collect();
    w.insert(key, Win { ui: spec.ui, pages: spec.pages, claims: spec.claims, latch, hover_owner: hover });
    (hover, dropped)
  });
  // A page that left the set gets its pointer tracking back.
  for v in &dropped {
    ungate(v);
  }
  rebuild_mask(key)?;
  // Keep the gates in step with the owner we last saw.
  if let Some(o) = old_hover {
    apply_gates(key, o);
  }
  Ok(())
}

/// Drop a window's set: no mask, every tracking area back, stock routing.
pub fn clear_window(ui: &NSView) {
  if MainThreadMarker::new().is_none() {
    return;
  }
  let Some(key) = window_key(ui) else { return };
  BELOW.with(|b| b.borrow_mut().remove(&key));
  let win = WINS.with(|w| w.borrow_mut().remove(&key));
  if let Some(win) = win {
    ungate(&win.ui);
    for p in &win.pages {
      ungate(&p.view);
    }
  }
  ungate(ui);
  remove_mask(ui);
}

/// A page view is about to close: give it its tracking areas back and stop
/// routing to it. Main thread.
pub fn forget_view(view: &NSView) {
  if MainThreadMarker::new().is_none() {
    return;
  }
  let p = ptr(view);
  ungate(view);
  let keys: Vec<usize> = WINS.with(|w| {
    let mut w = w.borrow_mut();
    let mut keys = Vec::new();
    for (k, win) in w.iter_mut() {
      let before = win.pages.len();
      win.pages.retain(|pg| ptr(&*pg.view) != p);
      if win.pages.len() != before {
        keys.push(*k);
        if win.hover_owner == Some(p) {
          win.hover_owner = None;
        }
        if win.latch.scroll == Some(p) {
          win.latch.scroll = None;
        }
        for g in win.latch.gesture.iter_mut() {
          if *g == Some(p) {
            *g = None;
          }
        }
      }
    }
    keys
  });
  for k in keys {
    let _ = rebuild_mask(k);
  }
}

/// New child webviews of this UI view's window are inserted below the UI
/// view (never added on top and then moved). Main thread.
pub fn set_children_below(ui: &NSView, on: bool) {
  if MainThreadMarker::new().is_none() {
    return;
  }
  if let Some(key) = window_key(ui) {
    BELOW.with(|b| {
      let mut b = b.borrow_mut();
      if on {
        b.insert(key, ui.retain());
      } else {
        b.remove(&key);
      }
    });
  }
}

/// Order sibling views in place (no remove and re-add, so first responder
/// stays): `pages` (back to front) go just below the UI view when `below`,
/// just above it otherwise. Other siblings keep their order. Main thread.
pub fn order(ui: &NSView, pages: &[Retained<NSView>], below: bool) -> Result<(), String> {
  mtm()?;
  let parent = unsafe { ui.superview() }.ok_or("underlay: the UI view has no superview")?;
  let subs = parent.subviews();
  let current: Vec<usize> = (0..subs.count()).map(|i| Retained::as_ptr(&subs.objectAtIndex(i)) as usize).collect();
  let page_ptrs: Vec<usize> = pages.iter().map(|v| ptr(&**v)).filter(|p| current.contains(p)).collect();
  let ui_p = ptr(ui);
  let mut desired: Vec<usize> = current.iter().copied().filter(|p| !page_ptrs.contains(p)).collect();
  let Some(at) = desired.iter().position(|p| *p == ui_p) else {
    return Err("underlay: the UI view is not a sibling of the pages".into());
  };
  let at = if below { at } else { at + 1 };
  for (i, p) in page_ptrs.iter().enumerate() {
    desired.insert(at + i, *p);
  }
  if desired == current {
    return Ok(());
  }
  let rank: HashMap<usize, usize> = desired.iter().enumerate().map(|(i, p)| (*p, i)).collect();
  unsafe extern "C-unwind" fn cmp(a: NonNull<NSView>, b: NonNull<NSView>, ctx: *mut c_void) -> NSComparisonResult {
    // SAFETY: ctx is the &HashMap below, alive for the whole sort.
    let rank = unsafe { &*(ctx as *const HashMap<usize, usize>) };
    let ra = rank.get(&(a.as_ptr() as usize)).copied().unwrap_or(usize::MAX);
    let rb = rank.get(&(b.as_ptr() as usize)).copied().unwrap_or(usize::MAX);
    ra.cmp(&rb).into()
  }
  unsafe {
    parent.sortSubviewsUsingFunction_context(cmp, &rank as *const HashMap<usize, usize> as *mut c_void);
  }
  Ok(())
}

/// First click on a page in an inactive window (default off).
pub fn set_page_first_mouse(on: bool) {
  PAGE_FIRST_MOUSE.store(on, Ordering::Relaxed);
}

/// Pick the hover route. `private_fallback` installs the named private
/// WebKit fallback (`hover_private_webkit_fallback`); a missing class is
/// logged and the public route stays. Returns a line for the log.
pub fn set_hover_route(private_fallback: bool) -> String {
  if private_fallback {
    match private_hover::install() {
      Ok(line) => {
        HOVER_PRIVATE.store(true, Ordering::Relaxed);
        format!("hover route: hover_private_webkit_fallback in use ({line})")
      }
      Err(line) => {
        HOVER_PRIVATE.store(false, Ordering::Relaxed);
        format!("hover route: public (hover_private_webkit_fallback asked for, not in use: {line})")
      }
    }
  } else {
    HOVER_PRIVATE.store(false, Ordering::Relaxed);
    format!(
      "hover route: public (hover_private_webkit_fallback off; WKMouseTrackingObserver {})",
      if private_hover::class_present() { "present" } else { "absent" }
    )
  }
}

// ---------------------------------------------------------------------------
// Coordinates and the owner rule
// ---------------------------------------------------------------------------

/// A window point in `view`'s own top-left coordinates.
fn local_top_left(view: &NSView, wp: NSPoint) -> (f64, f64) {
  let p = view.convertPoint_fromView(wp, None);
  let b = view.bounds();
  let x = p.x - b.origin.x;
  let y = if view.isFlipped() { p.y - b.origin.y } else { b.size.height - (p.y - b.origin.y) };
  (x, y)
}

fn shown_in(view: &NSView, key: usize) -> bool {
  !view.isHiddenOrHasHiddenAncestor() && view.alphaValue() > 0.0 && window_key(view) == Some(key)
}

/// The owner of a window point: a page view pointer, or `None` for K2.
fn owner_in(win: &Win, key: usize, wp: NSPoint) -> Option<usize> {
  let (ux, uy) = local_top_left(&win.ui, wp);
  let pages: Vec<PageAt<usize>> = win
    .pages
    .iter()
    .map(|p| {
      let local = if shown_in(&p.view, key) {
        let (lx, ly) = local_top_left(&p.view, wp);
        let b = p.view.bounds();
        Some((lx, ly, b.size.width, b.size.height))
      } else {
        None
      };
      PageAt { id: ptr(&*p.view), local, corners: p.corners, minus: p.minus.clone(), live: p.live }
    })
    .collect();
  logic::owner(&pages, &win.claims, ux, uy)
}

/// The event's location in its window's base coordinates.
fn event_point(view: &NSView, ev: &NSEvent) -> Option<NSPoint> {
  let win = view.window()?;
  let p = ev.locationInWindow();
  let n = ev.windowNumber();
  if n == win.windowNumber() {
    Some(p)
  } else if n == 0 {
    // A window-less event reports screen coordinates.
    Some(win.convertPointFromScreen(p))
  } else {
    None
  }
}

/// Called from `WryWebView`'s `hitTest:` after super answered non-nil:
/// true means "answer nil" (the point belongs to a view below).
pub(crate) fn hit_test_passes(view: &NSView, point_in_superview: NSPoint) -> bool {
  let Some(key) = window_key(view) else { return false };
  let Some(sv) = (unsafe { view.superview() }) else { return false };
  let wp = sv.convertPoint_toView(point_in_superview, None);
  let me = ptr(view);
  WINS.with(|w| {
    let w = w.borrow();
    let Some(win) = w.get(&key) else { return false };
    let is_ui = ptr(&*win.ui) == me;
    let is_page = win.pages.iter().any(|p| ptr(&*p.view) == me);
    if !is_ui && !is_page {
      return false;
    }
    let own = owner_in(win, key, wp);
    if is_ui {
      own.is_some()
    } else {
      own != Some(me)
    }
  })
}

/// Where a routed event goes.
pub(crate) enum Fwd {
  Super,
  To(Retained<NSView>),
}

/// Pick the target for an input event that reached `view`. `hover` also
/// keeps the pointer's owner (gates and one exit on a change).
pub(crate) fn route(view: &NSView, ev: &NSEvent, r: Route, hover: bool) -> Fwd {
  if IN_FWD.with(|c| c.get()) {
    return Fwd::Super;
  }
  let Some(key) = window_key(view) else { return Fwd::Super };
  if !WINS.with(|w| w.borrow().contains_key(&key)) {
    return Fwd::Super;
  }
  let Some(wp) = event_point(view, ev) else { return Fwd::Super };
  let ty = ev.r#type();
  // Only read fields that are valid for the event type (others raise).
  let (phase, mom) = if ty == NSEventType::ScrollWheel {
    (ev.phase().0 as usize, ev.momentumPhase().0 as usize)
  } else if ty == NSEventType::Magnify || ty == NSEventType::Rotate {
    (ev.phase().0 as usize, 0)
  } else {
    (0, 0)
  };
  let me = ptr(view);
  let (target, hover_change) = WINS.with(|w| {
    let mut w = w.borrow_mut();
    let Some(win) = w.get_mut(&key) else { return (None, None) };
    let ui = ptr(&*win.ui);
    let known = ui == me || win.pages.iter().any(|p| ptr(&*p.view) == me);
    if !known {
      return (None, None);
    }
    let own = owner_in(win, key, wp).unwrap_or(ui);
    let t = logic::decide(&mut win.latch, r, own, phase, mom);
    // A still (or vanished) page is never a target.
    let t_ok = t == ui || win.pages.iter().any(|p| ptr(&*p.view) == t && p.live && shown_in(&p.view, key));
    let target = if !t_ok || t == me {
      None
    } else if t == ui {
      Some(win.ui.clone())
    } else {
      win.pages.iter().find(|p| ptr(&*p.view) == t).map(|p| p.view.clone())
    };
    let change = if hover && win.hover_owner != Some(own) {
      let prev = win.hover_owner.replace(own);
      Some(prev)
    } else {
      None
    };
    (target, change.map(|prev| (prev, own)))
  });
  if let Some((prev, own)) = hover_change {
    if let Some(prev) = prev {
      send_exit(key, prev, ev);
    }
    apply_gates(key, own);
  }
  match target {
    Some(v) => Fwd::To(v),
    None => Fwd::Super,
  }
}

/// Run a forwarded send with the re-forward guard on.
pub(crate) fn forward(f: impl FnOnce()) {
  let was = IN_FWD.with(|c| c.replace(true));
  f();
  IN_FWD.with(|c| c.set(was));
}

/// `acceptsFirstMouse:` for a page in an underlay window; `None` keeps
/// wry's own answer.
pub(crate) fn page_first_mouse(view: &NSView) -> Option<bool> {
  let key = window_key(view)?;
  let me = ptr(view);
  WINS.with(|w| {
    let w = w.borrow();
    let win = w.get(&key)?;
    win.pages.iter().any(|p| ptr(&*p.view) == me).then(|| PAGE_FIRST_MOUSE.load(Ordering::Relaxed))
  })
}

/// For new child webviews: the UI view to insert below, when the window is
/// in underlay.
pub(crate) fn insert_below_target(parent: &NSView) -> Option<Retained<NSView>> {
  let key = window_key(parent)?;
  let ui = BELOW.with(|b| b.borrow().get(&key).cloned())?;
  // Only when the UI view is really a child of this parent.
  let sv = unsafe { ui.superview() }?;
  (ptr(&*sv) == ptr(parent)).then_some(ui)
}

// ---------------------------------------------------------------------------
// Hover: tracking-area gates (public route) and the exit on an owner change
// ---------------------------------------------------------------------------

/// The pointer's owner is `own` (a page or the UI view): every other view of
/// the window has its tracking areas lifted, the owner gets its back. On
/// the private route nothing is lifted (the observer swap drops moves).
fn apply_gates(key: usize, own: usize) {
  let views: Vec<Retained<NSView>> = WINS.with(|w| {
    w.borrow()
      .get(&key)
      .map(|win| std::iter::once(win.ui.clone()).chain(win.pages.iter().map(|p| p.view.clone())).collect())
      .unwrap_or_default()
  });
  let private = HOVER_PRIVATE.load(Ordering::Relaxed);
  for v in &views {
    if private || ptr(&**v) == own {
      ungate(v);
    } else {
      gate(v);
    }
  }
}

fn gate(v: &NSView) {
  let p = ptr(v);
  let areas = v.trackingAreas();
  let list: Vec<Retained<NSTrackingArea>> = (0..areas.count()).map(|i| areas.objectAtIndex(i)).collect();
  GATED.with(|g| {
    let mut g = g.borrow_mut();
    let e = g.entry(p).or_default();
    for a in &list {
      if !e.iter().any(|x| Retained::as_ptr(x) == Retained::as_ptr(a)) {
        e.push(a.clone());
      }
    }
  });
  let was = IN_GATE.with(|c| c.replace(true));
  for a in &list {
    v.removeTrackingArea(a);
  }
  IN_GATE.with(|c| c.set(was));
}

fn ungate(v: &NSView) {
  let Some(list) = GATED.with(|g| g.borrow_mut().remove(&ptr(v))) else { return };
  let was = IN_GATE.with(|c| c.replace(true));
  for a in &list {
    v.addTrackingArea(a);
  }
  IN_GATE.with(|c| c.set(was));
}

/// `addTrackingArea:` on a lifted view (WebKit re-adding its own): kept
/// aside until the view owns the pointer again. True = handled, skip super.
pub(crate) fn tracking_area_added(v: &NSView, area: &NSTrackingArea) -> bool {
  if IN_GATE.with(|c| c.get()) {
    return false;
  }
  GATED.with(|g| {
    let mut g = g.borrow_mut();
    let Some(e) = g.get_mut(&ptr(v)) else { return false };
    let ap = area as *const NSTrackingArea;
    if !e.iter().any(|x| Retained::as_ptr(x) == ap) {
      e.push(area.retain());
    }
    true
  })
}

/// `removeTrackingArea:` on a lifted view: WebKit dropped one of its own,
/// so it must not come back.
pub(crate) fn tracking_area_removed(v: &NSView, area: &NSTrackingArea) {
  if IN_GATE.with(|c| c.get()) {
    return;
  }
  GATED.with(|g| {
    if let Some(e) = g.borrow_mut().get_mut(&ptr(v)) {
      let ap = area as *const NSTrackingArea;
      e.retain(|x| Retained::as_ptr(x) != ap);
    }
  });
}

/// After `updateTrackingAreas` on a lifted view, lift whatever came back.
pub(crate) fn tracking_areas_updated(v: &NSView) {
  if IN_GATE.with(|c| c.get()) {
    return;
  }
  if GATED.with(|g| g.borrow().contains_key(&ptr(v))) {
    gate(v);
  }
}

/// One mouse-exit, far outside, to the view that lost the pointer: sent to
/// its tracking areas' owners the way AppKit sends a real one, so its page
/// drops its hover state and cursor.
fn send_exit(key: usize, prev: usize, src: &NSEvent) {
  let Some(view) = WINS.with(|w| {
    w.borrow().get(&key).and_then(|win| {
      if ptr(&*win.ui) == prev {
        Some(win.ui.clone())
      } else {
        win.pages.iter().find(|p| ptr(&*p.view) == prev).map(|p| p.view.clone())
      }
    })
  }) else {
    return;
  };
  // Areas on the view, or kept aside while it was lifted.
  let mut areas: Vec<Retained<NSTrackingArea>> = {
    let a = view.trackingAreas();
    (0..a.count()).map(|i| a.objectAtIndex(i)).collect()
  };
  GATED.with(|g| {
    if let Some(e) = g.borrow().get(&prev) {
      areas.extend(e.iter().cloned());
    }
  });
  for area in areas {
    if !area.options().contains(NSTrackingAreaOptions::MouseEnteredAndExited) {
      continue;
    }
    let Some(owner) = area.owner() else { continue };
    let responds: bool = unsafe { msg_send![&*owner, respondsToSelector: objc2::sel!(mouseExited:)] };
    if !responds {
      continue;
    }
    let ev = unsafe {
      NSEvent::enterExitEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_trackingNumber_userData(
        NSEventType::MouseExited,
        NSPoint::new(-32000.0, -32000.0),
        src.modifierFlags(),
        src.timestamp(),
        src.windowNumber(),
        None,
        0,
        Retained::as_ptr(&area) as isize,
        std::ptr::null_mut(),
      )
    };
    let Some(ev) = ev else { continue };
    forward(|| unsafe {
      let _: () = msg_send![&*owner, mouseExited: &*ev];
    });
  }
}

// ---------------------------------------------------------------------------
// The UI view's mask: open where a live page shows, closed under claims
// ---------------------------------------------------------------------------

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
  fn CGPathCreateMutable() -> *mut c_void;
  fn CGPathAddRect(path: *mut c_void, m: *const c_void, rect: CGRect);
  fn CGPathMoveToPoint(path: *mut c_void, m: *const c_void, x: f64, y: f64);
  fn CGPathAddLineToPoint(path: *mut c_void, m: *const c_void, x: f64, y: f64);
  fn CGPathAddArcToPoint(path: *mut c_void, m: *const c_void, x1: f64, y1: f64, x2: f64, y2: f64, r: f64);
  fn CGPathCloseSubpath(path: *mut c_void);
  fn CGPathRelease(path: *mut c_void);
}

/// Append `s` (top-left coords) as one closed subpath; `fy` maps to layer y.
unsafe fn add_shape(p: *mut c_void, s: &Shape, fy: &dyn Fn(f64) -> f64) {
  let n = std::ptr::null();
  let (x, y, w, h) = (s.x, s.y, s.w, s.h);
  if let Some([tl, tr, br, bl]) = s.ch {
    let pts = [
      (x + tl, y),
      (x + w - tr, y),
      (x + w, y + tr),
      (x + w, y + h - br),
      (x + w - br, y + h),
      (x + bl, y + h),
      (x, y + h - bl),
      (x, y + tl),
    ];
    CGPathMoveToPoint(p, n, pts[0].0, fy(pts[0].1));
    for &(px, py) in &pts[1..] {
      CGPathAddLineToPoint(p, n, px, fy(py));
    }
  } else {
    let [tl, tr, br, bl] = s.r;
    let corner = |cx: f64, cy: f64, ex: f64, ey: f64, r: f64| {
      if r > 0.0 {
        CGPathAddArcToPoint(p, n, cx, fy(cy), ex, fy(ey), r);
      } else {
        CGPathAddLineToPoint(p, n, cx, fy(cy));
      }
    };
    CGPathMoveToPoint(p, n, x + tl, fy(y));
    corner(x + w, y, x + w, y + tr, tr);
    corner(x + w, y + h, x + w - br, y + h, br);
    corner(x, y + h, x, y + h - bl, bl);
    corner(x, y, x + tl, y, tl);
  }
  CGPathCloseSubpath(p);
}

fn new_layer(class: &std::ffi::CStr) -> Option<Retained<AnyObject>> {
  let cls = AnyClass::get(class)?;
  let l: Retained<AnyObject> = unsafe { msg_send![cls, new] };
  Some(l)
}

fn with_no_animation(f: impl FnOnce()) {
  let tx = AnyClass::get(c"CATransaction");
  if let Some(tx) = tx {
    unsafe {
      let _: () = msg_send![tx, begin];
      let _: () = msg_send![tx, setDisableActions: Bool::YES];
    }
  }
  f();
  if let Some(tx) = tx {
    unsafe {
      let _: () = msg_send![tx, commit];
    }
  }
}

/// Rebuild the window's UI mask from the pages' real frames (so it follows
/// a resize in the same layout pass). No live page: no mask.
fn rebuild_mask(key: usize) -> Result<(), String> {
  if REBUILDING.with(|r| r.replace(true)) {
    return Ok(());
  }
  let r = rebuild_mask_inner(key);
  REBUILDING.with(|r| r.set(false));
  r
}

fn rebuild_mask_inner(key: usize) -> Result<(), String> {
  struct Geo {
    ui: Retained<NSView>,
    holes: Vec<Shape>,
    keep: Vec<Shape>,
  }
  let geo = WINS.with(|w| {
    let w = w.borrow();
    let win = w.get(&key)?;
    let ui = win.ui.clone();
    let ub = ui.bounds();
    let flipped = ui.isFlipped();
    let mut holes = Vec::new();
    let mut keep = win.claims.clone();
    // A shown page keeps its hole in either mode: `still` only moves input
    // to K2. The hole closes when the view hides (K2's freeze-frame then
    // shows), in the same pass, through `setHidden:`.
    for p in &win.pages {
      if !shown_in(&p.view, key) {
        continue;
      }
      let Some(sv) = (unsafe { p.view.superview() }) else { continue };
      let r = ui.convertRect_fromView(p.view.frame(), Some(&sv));
      let top = if flipped { r.origin.y - ub.origin.y } else { ub.size.height - (r.origin.y - ub.origin.y) - r.size.height };
      let corners = p.corners.local(r.size.width, r.size.height);
      if let Some(s) = Shape::new(r.origin.x - ub.origin.x, top, r.size.width, r.size.height, corners.r, corners.ch.unwrap_or([0.0; 4])) {
        holes.push(s);
      }
      keep.extend(p.minus.iter().copied());
    }
    Some(Geo { ui, holes, keep })
  });
  let Some(geo) = geo else { return Ok(()) };
  if geo.holes.is_empty() {
    remove_mask(&geo.ui);
    return Ok(());
  }
  let layer = layer_of(&geo.ui).ok_or("underlay: the UI view has no layer to mask")?;
  let scale = geo.ui.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
  let (holes, merged) = logic::disjoint_holes(geo.holes);
  if merged && !MERGE_LOGGED.with(|c| c.replace(true)) {
    eprintln!("wry underlay: overlapping pages; their mask holes were merged into one box");
  }
  let b = geo.ui.bounds();
  let (w, h) = (b.size.width, b.size.height);
  let flip = !geo.ui.isFlipped();
  let fy = move |y: f64| if flip { h - y } else { y };
  let up = ptr(&*geo.ui);
  let have = MASKS.with(|m| m.borrow().contains_key(&up));
  if !have {
    let (Some(c), Some(hl), Some(cl)) = (new_layer(c"CALayer"), new_layer(c"CAShapeLayer"), new_layer(c"CAShapeLayer")) else {
      return Err("underlay: CALayer / CAShapeLayer missing".into());
    };
    unsafe {
      let _: () = msg_send![&*hl, setFillRule: ns_string!("even-odd")];
      let _: () = msg_send![&*c, addSublayer: &*hl];
      let _: () = msg_send![&*c, addSublayer: &*cl];
    }
    MASKS.with(|m| m.borrow_mut().insert(up, Mask { container: c, holes: hl, claims: cl }));
  }
  let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w, h));
  let mut ok = true;
  with_no_animation(|| {
    MASKS.with(|m| {
      let m = m.borrow();
      let Some(mask) = m.get(&up) else {
        ok = false;
        return;
      };
      unsafe {
        let hp = CGPathCreateMutable();
        let kp = CGPathCreateMutable();
        if hp.is_null() || kp.is_null() {
          ok = false;
          return;
        }
        CGPathAddRect(hp, std::ptr::null(), frame);
        for s in &holes {
          add_shape(hp, s, &fy);
        }
        for s in &geo.keep {
          add_shape(kp, &s.snap_out(scale), &fy);
        }
        let _: () = msg_send![&*mask.container, setFrame: frame];
        let _: () = msg_send![&*mask.holes, setFrame: frame];
        let _: () = msg_send![&*mask.claims, setFrame: frame];
        let _: () = msg_send![&*mask.holes, setPath: hp as *const c_void];
        let _: () = msg_send![&*mask.claims, setPath: kp as *const c_void];
        CGPathRelease(hp);
        CGPathRelease(kp);
        let cur: *mut AnyObject = msg_send![&*layer, mask];
        if cur != Retained::as_ptr(&mask.container) as *mut AnyObject {
          let _: () = msg_send![&*layer, setMask: &*mask.container];
        }
        let now: *mut AnyObject = msg_send![&*layer, mask];
        ok = now == Retained::as_ptr(&mask.container) as *mut AnyObject;
      }
    })
  });
  if ok {
    Ok(())
  } else {
    remove_mask(&geo.ui);
    Err("underlay: the UI view's mask did not take".into())
  }
}

fn remove_mask(ui: &NSView) {
  let up = ptr(ui);
  let had = MASKS.with(|m| m.borrow_mut().remove(&up));
  if had.is_some() {
    if let Some(layer) = layer_of(ui) {
      with_no_animation(|| unsafe {
        let _: () = msg_send![&*layer, setMask: std::ptr::null_mut::<AnyObject>()];
      });
    }
  }
}

/// `setFrameSize:` / `setHidden:` on any WryWebView: if it's the UI view or
/// a page of an underlay window, rebuild that window's mask in this same
/// pass.
pub(crate) fn frame_changed(view: &NSView) {
  let Some(key) = window_key(view) else { return };
  let me = ptr(view);
  let hit = WINS.with(|w| {
    w.borrow()
      .get(&key)
      .map(|win| ptr(&*win.ui) == me || win.pages.iter().any(|p| ptr(&*p.view) == me))
      .unwrap_or(false)
  });
  if hit {
    if let Err(e) = rebuild_mask(key) {
      eprintln!("wry underlay: mask after a resize: {e}");
    }
  }
}

/// Is `view` the pointer's current non-owner in an underlay window? (For
/// the private observer swap: drop its moves and enters.)
pub(crate) fn drops_hover_for(view_ptr: usize, key: usize) -> bool {
  WINS.with(|w| {
    let w = w.borrow();
    let Some(win) = w.get(&key) else { return false };
    let known = ptr(&*win.ui) == view_ptr || win.pages.iter().any(|p| ptr(&*p.view) == view_ptr);
    known && win.hover_owner.is_some_and(|o| o != view_ptr)
  })
}

/// The registered views of every underlay window: (window key, view).
pub(crate) fn all_views() -> Vec<(usize, Retained<NSView>)> {
  WINS.with(|w| {
    w.borrow()
      .iter()
      .flat_map(|(k, win)| std::iter::once((*k, win.ui.clone())).chain(win.pages.iter().map(move |p| (*k, p.view.clone()))))
      .collect()
  })
}

/// `NSWindowOrderingMode::Below`, re-exported for wry's child insert.
pub(crate) const BELOW_MODE: NSWindowOrderingMode = NSWindowOrderingMode::Below;
