// Copyright 2026 Alakazam Labs
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The pure half of the macOS underlay (K2 fork): shapes, the owner rule
//! and the per-gesture latch. No AppKit here, so K2's src-tauri tests
//! include this file by path and run it on any host.
//!
//! Coordinates are top-left, in points. "UI px" are the UI webview's own
//! coordinates (K2's window CSS px from the content area's top-left).

/// A rect with per-corner radii, or a chamfered rect (45° cuts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
  pub x: f64,
  pub y: f64,
  pub w: f64,
  pub h: f64,
  /// Top-left, top-right, bottom-right, bottom-left radii.
  pub r: [f64; 4],
  /// Chamfer sizes in the same order; wins over `r` when set.
  pub ch: Option<[f64; 4]>,
}

impl Shape {
  /// A shape from the wire values. `None` when it has no area. Radii and
  /// chamfers are clamped to half the short side; negatives are 0.
  pub fn new(x: f64, y: f64, w: f64, h: f64, radius: [f64; 4], chamfer: [f64; 4]) -> Option<Shape> {
    if !(w > 0.0 && h > 0.0) || !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
      return None;
    }
    let lim = w.min(h) / 2.0;
    let clamp = |a: [f64; 4]| a.map(|v| if v.is_finite() { v.max(0.0).min(lim) } else { 0.0 });
    let r = clamp(radius);
    let ch = clamp(chamfer);
    Some(Shape {
      x,
      y,
      w,
      h,
      r,
      ch: if ch.iter().any(|v| *v > 0.0) { Some(ch) } else { None },
    })
  }

  /// The same corners on a box at (0, 0) of the given size: a page's corner
  /// shape tested against its view's real bounds.
  pub fn local(&self, w: f64, h: f64) -> Shape {
    let lim = w.min(h).max(0.0) / 2.0;
    let clamp = |a: [f64; 4]| a.map(|v| v.min(lim));
    Shape {
      x: 0.0,
      y: 0.0,
      w,
      h,
      r: clamp(self.r),
      ch: self.ch.map(clamp),
    }
  }

  pub fn is_square(&self) -> bool {
    self.ch.map(|c| c.iter().all(|v| *v <= 0.0)).unwrap_or(true) && self.r.iter().all(|v| *v <= 0.0)
  }

  /// Exact containment: per-corner rounded rect or chamfered polygon.
  /// Edges count as inside.
  pub fn contains(&self, px: f64, py: f64) -> bool {
    let (x0, y0, x1, y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
    if !(px >= x0 && py >= y0 && px <= x1 && py <= y1) {
      return false;
    }
    if let Some([tl, tr, br, bl]) = self.ch {
      return (px - x0) + (py - y0) >= tl
        && (x1 - px) + (py - y0) >= tr
        && (x1 - px) + (y1 - py) >= br
        && (px - x0) + (y1 - py) >= bl;
    }
    let [tl, tr, br, bl] = self.r;
    let out = |cx: f64, cy: f64, r: f64| (px - cx).powi(2) + (py - cy).powi(2) > r * r;
    if tl > 0.0 && px < x0 + tl && py < y0 + tl && out(x0 + tl, y0 + tl, tl) {
      return false;
    }
    if tr > 0.0 && px > x1 - tr && py < y0 + tr && out(x1 - tr, y0 + tr, tr) {
      return false;
    }
    if br > 0.0 && px > x1 - br && py > y1 - br && out(x1 - br, y1 - br, br) {
      return false;
    }
    if bl > 0.0 && px < x0 + bl && py > y1 - bl && out(x0 + bl, y1 - bl, bl) {
      return false;
    }
    true
  }

  /// Bounding boxes overlap with positive area.
  pub fn overlaps(&self, o: &Shape) -> bool {
    self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
  }

  /// Grown outward to whole device pixels (`scale` px per point), corners kept.
  pub fn snap_out(&self, scale: f64) -> Shape {
    if !(scale > 0.0) {
      return *self;
    }
    let x0 = (self.x * scale).floor() / scale;
    let y0 = (self.y * scale).floor() / scale;
    let x1 = ((self.x + self.w) * scale).ceil() / scale;
    let y1 = ((self.y + self.h) * scale).ceil() / scale;
    Shape { x: x0, y: y0, w: x1 - x0, h: y1 - y0, ..*self }
  }
}

/// Holes for a mask must never overlap: an even-odd fill re-opens the area
/// two holes share (the "overlapping holes" trap). Overlapping holes are
/// merged into one square box covering both. Disjoint holes (today's split
/// panes) pass through unchanged. Returns (holes, merged any).
pub fn disjoint_holes(mut holes: Vec<Shape>) -> (Vec<Shape>, bool) {
  let mut merged_any = false;
  loop {
    let mut merged = false;
    'outer: for i in 0..holes.len() {
      for j in (i + 1)..holes.len() {
        if holes[i].overlaps(&holes[j]) {
          let (a, b) = (holes[i], holes[j]);
          let x0 = a.x.min(b.x);
          let y0 = a.y.min(b.y);
          let x1 = (a.x + a.w).max(b.x + b.w);
          let y1 = (a.y + a.h).max(b.y + b.h);
          holes[i] = Shape { x: x0, y: y0, w: x1 - x0, h: y1 - y0, r: [0.0; 4], ch: None };
          holes.remove(j);
          merged = true;
          merged_any = true;
          break 'outer;
        }
      }
    }
    if !merged {
      return (holes, merged_any);
    }
  }
}

/// One page as the owner rule sees it at one point.
#[derive(Clone, Debug)]
pub struct PageAt<'a, Id> {
  pub id: Id,
  /// The point in the page view's own top-left coordinates, and the view's
  /// bounds size. `None` when the view is hidden or not in the window.
  pub local: Option<(f64, f64, f64, f64)>,
  /// The page's corners (only the radii/chamfers are read).
  pub corners: Shape,
  /// K2 things drawn above this page, in UI px.
  pub minus: &'a [Shape],
  pub live: bool,
}

/// Who owns a point: the frontmost page (front first in `pages`) whose real
/// frame and corner shape contain it, unless a claim, that page's `minus`
/// or `still` gives the point to K2 (`None`). `(ux, uy)` is the point in
/// UI px.
pub fn owner<Id: Copy>(pages: &[PageAt<'_, Id>], claims: &[Shape], ux: f64, uy: f64) -> Option<Id> {
  if claims.iter().any(|c| c.contains(ux, uy)) {
    return None;
  }
  for p in pages {
    let Some((lx, ly, w, h)) = p.local else { continue };
    if p.corners.local(w, h).contains(lx, ly) {
      if !p.live || p.minus.iter().any(|m| m.contains(ux, uy)) {
        return None;
      }
      return Some(p.id);
    }
  }
  None
}

/// NSEventPhase bits (also momentumPhase).
pub const PH_BEGAN: usize = 1;
pub const PH_ENDED: usize = 8;
pub const PH_CANCELLED: usize = 16;
pub const PH_MAYBEGIN: usize = 32;

/// How an event method picks its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
  /// `scrollWheel:`: latched per gesture (phase, then momentum). A legacy
  /// wheel event (no phase, no momentum) is decided on its own.
  Scroll,
  /// magnify (0) / rotate (1): latched from Began to Ended.
  Gesture(usize),
  /// Smart magnify, swipe, begin/end gesture, right mouse down, moves: by
  /// location, per event.
  Location,
}

/// The per-window gesture latch.
#[derive(Clone, Debug, PartialEq)]
pub struct Latch<Id> {
  pub scroll: Option<Id>,
  pub maybegin: bool,
  pub gesture: [Option<Id>; 2],
}

impl<Id> Default for Latch<Id> {
  fn default() -> Self {
    Latch { scroll: None, maybegin: false, gesture: [None, None] }
  }
}

/// Pick the target for one event and update the latch. `owner` is the owner
/// of the event's point right now.
pub fn decide<Id: Copy>(l: &mut Latch<Id>, route: Route, owner: Id, phase: usize, mom: usize) -> Id {
  match route {
    Route::Location => owner,
    Route::Gesture(i) => {
      let i = i.min(1);
      if phase & PH_BEGAN != 0 {
        l.gesture[i] = Some(owner);
        owner
      } else if phase != 0 {
        let t = l.gesture[i].unwrap_or(owner);
        if phase & (PH_ENDED | PH_CANCELLED) != 0 {
          l.gesture[i] = None;
        }
        t
      } else {
        owner
      }
    }
    Route::Scroll => {
      if phase & PH_MAYBEGIN != 0 {
        l.scroll = Some(owner);
        l.maybegin = true;
        owner
      } else if phase & PH_BEGAN != 0 {
        let t = match (l.maybegin, l.scroll) {
          (true, Some(t)) => t,
          _ => {
            l.scroll = Some(owner);
            owner
          }
        };
        l.maybegin = false;
        t
      } else if phase != 0 {
        let t = l.scroll.unwrap_or(owner);
        // A touch that never became a scroll (MayBegin, then Cancelled).
        if phase & PH_CANCELLED != 0 && l.maybegin {
          l.scroll = None;
        }
        l.maybegin = false;
        t
      } else if mom != 0 {
        let t = l.scroll.unwrap_or(owner);
        if mom & (PH_ENDED | PH_CANCELLED) != 0 {
          l.scroll = None;
        }
        t
      } else {
        // A mouse wheel: per event.
        owner
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sq(x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape::new(x, y, w, h, [0.0; 4], [0.0; 4]).unwrap()
  }

  #[test]
  fn rounded_corner_excludes_the_cut_and_keeps_the_edge() {
    let s = Shape::new(0.0, 0.0, 100.0, 100.0, [16.0; 4], [0.0; 4]).unwrap();
    assert!(!s.contains(1.0, 1.0), "outside the top-left arc");
    assert!(s.contains(16.0, 0.0), "top edge after the arc");
    assert!(s.contains(50.0, 50.0));
    assert!(!s.contains(99.0, 99.0), "outside the bottom-right arc");
    assert!(!s.contains(101.0, 50.0));
  }

  #[test]
  fn chamfer_is_a_45_degree_cut() {
    let s = Shape::new(0.0, 0.0, 100.0, 100.0, [0.0; 4], [10.0, 0.0, 0.0, 0.0]).unwrap();
    assert!(!s.contains(4.0, 4.0));
    assert!(s.contains(5.0, 5.0), "on the cut line");
    assert!(s.contains(99.0, 1.0), "the other corners are square");
  }

  #[test]
  fn radii_clamp_to_half_the_short_side_and_empty_shapes_are_none() {
    let s = Shape::new(0.0, 0.0, 20.0, 100.0, [50.0; 4], [0.0; 4]).unwrap();
    assert_eq!(s.r, [10.0; 4]);
    assert!(Shape::new(0.0, 0.0, 0.0, 10.0, [0.0; 4], [0.0; 4]).is_none());
    assert!(Shape::new(f64::NAN, 0.0, 10.0, 10.0, [0.0; 4], [0.0; 4]).is_none());
  }

  fn page(id: u8, x: f64, y: f64, w: f64, h: f64, ux: f64, uy: f64, live: bool, minus: &[Shape]) -> PageAt<'_, u8> {
    // Point in the view's own coordinates = UI px minus the view origin.
    PageAt { id, local: Some((ux - x, uy - y, w, h)), corners: Shape::new(x, y, w, h, [16.0; 4], [0.0; 4]).unwrap(), minus, live }
  }

  #[test]
  fn owner_is_the_frontmost_page_unless_k2_claims_the_point() {
    // B (front) overlaps A (back) on x 150..200.
    let at = |ux: f64, uy: f64| {
      vec![page(2, 150.0, 0.0, 200.0, 200.0, ux, uy, true, &[]), page(1, 0.0, 0.0, 200.0, 200.0, ux, uy, true, &[])]
    };
    assert_eq!(owner(&at(50.0, 100.0), &[], 50.0, 100.0), Some(1));
    assert_eq!(owner(&at(175.0, 100.0), &[], 175.0, 100.0), Some(2), "B is in front");
    assert_eq!(owner(&at(400.0, 100.0), &[], 400.0, 100.0), None, "no page: K2");
    let menu = [sq(40.0, 90.0, 30.0, 30.0)];
    assert_eq!(owner(&at(50.0, 100.0), &menu, 50.0, 100.0), None, "a claim keeps the point");
  }

  #[test]
  fn minus_and_still_give_the_point_to_k2() {
    let chat = sq(20.0, 20.0, 40.0, 40.0);
    let minus = [chat];
    let p = vec![page(1, 0.0, 0.0, 200.0, 200.0, 30.0, 30.0, true, &minus)];
    assert_eq!(owner(&p, &[], 30.0, 30.0), None);
    let p = vec![page(1, 0.0, 0.0, 200.0, 200.0, 100.0, 100.0, false, &[])];
    assert_eq!(owner(&p, &[], 100.0, 100.0), None, "still keeps input with K2");
  }

  #[test]
  fn a_rounded_page_corner_falls_through_to_the_page_below() {
    let p = vec![page(2, 100.0, 100.0, 100.0, 100.0, 101.0, 101.0, true, &[]), page(1, 0.0, 0.0, 300.0, 300.0, 101.0, 101.0, true, &[])];
    assert_eq!(owner(&p, &[], 101.0, 101.0), Some(1));
  }

  #[test]
  fn a_hidden_view_passes_nothing_through() {
    let mut p = vec![page(1, 0.0, 0.0, 200.0, 200.0, 50.0, 50.0, true, &[])];
    p[0].local = None;
    assert_eq!(owner(&p, &[], 50.0, 50.0), None);
  }

  #[test]
  fn the_real_frame_wins_over_the_page_rect() {
    // The set said x=0, but the view now sits at x=100 (mid-move): routing
    // follows the view.
    let mut p = vec![page(1, 0.0, 0.0, 200.0, 200.0, 50.0, 50.0, true, &[])];
    p[0].local = Some((50.0 - 100.0, 50.0, 200.0, 200.0));
    assert_eq!(owner(&p, &[], 50.0, 50.0), None);
  }

  #[test]
  fn scroll_latches_from_maybegin_through_momentum() {
    let mut l = Latch::default();
    assert_eq!(decide(&mut l, Route::Scroll, 1u8, PH_MAYBEGIN, 0), 1);
    assert_eq!(decide(&mut l, Route::Scroll, 0, PH_BEGAN, 0), 1, "Began keeps the MayBegin owner");
    assert_eq!(decide(&mut l, Route::Scroll, 0, 4, 0), 1, "Changed over a toast stays on the page");
    assert_eq!(decide(&mut l, Route::Scroll, 0, PH_ENDED, 0), 1);
    assert_eq!(decide(&mut l, Route::Scroll, 0, 0, PH_BEGAN), 1, "momentum stays latched");
    assert_eq!(decide(&mut l, Route::Scroll, 0, 0, 4), 1);
    assert_eq!(decide(&mut l, Route::Scroll, 0, 0, PH_ENDED), 1);
    assert_eq!(l.scroll, None, "released at momentum end");
    assert_eq!(decide(&mut l, Route::Scroll, 0, 0, 0), 0, "a wheel is per event");
  }

  #[test]
  fn scroll_began_without_maybegin_latches_there() {
    let mut l = Latch::default();
    assert_eq!(decide(&mut l, Route::Scroll, 2u8, PH_BEGAN, 0), 2);
    assert_eq!(decide(&mut l, Route::Scroll, 0, 4, 0), 2);
    let mut l = Latch::default();
    decide(&mut l, Route::Scroll, 2u8, PH_MAYBEGIN, 0);
    decide(&mut l, Route::Scroll, 0, PH_CANCELLED, 0);
    assert_eq!(l.scroll, None, "a cancelled touch drops the latch");
  }

  #[test]
  fn magnify_latches_began_to_ended() {
    let mut l = Latch::default();
    assert_eq!(decide(&mut l, Route::Gesture(0), 3u8, PH_BEGAN, 0), 3);
    assert_eq!(decide(&mut l, Route::Gesture(0), 0, 4, 0), 3);
    assert_eq!(decide(&mut l, Route::Gesture(1), 0, PH_BEGAN, 0), 0, "rotate has its own slot");
    assert_eq!(decide(&mut l, Route::Gesture(0), 0, PH_ENDED, 0), 3);
    assert_eq!(l.gesture[0], None);
  }

  #[test]
  fn overlapping_holes_merge_and_disjoint_holes_stay() {
    let (h, m) = disjoint_holes(vec![sq(0.0, 0.0, 10.0, 10.0), sq(20.0, 0.0, 10.0, 10.0)]);
    assert_eq!(h.len(), 2);
    assert!(!m);
    let (h, m) = disjoint_holes(vec![sq(0.0, 0.0, 10.0, 10.0), sq(5.0, 5.0, 10.0, 10.0), sq(40.0, 0.0, 1.0, 1.0)]);
    assert!(m);
    assert_eq!(h.len(), 2);
    assert_eq!((h[0].x, h[0].y, h[0].w, h[0].h), (0.0, 0.0, 15.0, 15.0));
  }

  #[test]
  fn snap_out_grows_to_device_pixels() {
    // 10.3..15.6 at 2 px per point: out to 10.0..16.0.
    let s = sq(10.3, 10.3, 5.3, 5.3).snap_out(2.0);
    assert_eq!((s.x, s.y), (10.0, 10.0));
    assert_eq!((s.x + s.w, s.y + s.h), (16.0, 16.0));
    // Already on the grid: unchanged.
    let s = sq(10.0, 10.0, 5.5, 5.5).snap_out(2.0);
    assert_eq!((s.x, s.y, s.w, s.h), (10.0, 10.0, 5.5, 5.5));
  }
}
