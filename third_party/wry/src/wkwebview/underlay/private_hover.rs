// Copyright 2026 Alakazam Labs
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! `hover_private_webkit_fallback`: the fallback hover route. OFF unless K2
//! switches it on (a setting), for a macOS where the public route fails.
//!
//! WebKit gets pointer moves through its tracking area, whose owner is a
//! private `WKMouseTrackingObserver` that calls into WebKit directly and
//! ignores the view stacked on top. This swaps the observer's `mouseMoved:`
//! and `mouseEntered:` (class-wide, this process only) so the view that
//! doesn't own the pointer drops them. A missing class is logged and
//! nothing is installed; it never crashes. Test both routes on each new
//! macOS: a change in WebKit's class shows as hover on both layers again,
//! or cursor flicker.

use std::cell::RefCell;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSView;

const CLASS: &std::ffi::CStr = c"WKMouseTrackingObserver";

/// (selector, original IMP). Installed once per process.
static HOOKS: Mutex<Vec<(Sel, Imp)>> = Mutex::new(Vec::new());

thread_local! {
  /// observer pointer -> (window key, view pointer)
  static OWNERS: RefCell<Vec<(usize, usize, usize)>> = const { RefCell::new(Vec::new()) };
}

type EvFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);

pub fn class_present() -> bool {
  AnyClass::get(CLASS).is_some()
}

/// Swap the observer's move and enter methods. Ok(report) when in use,
/// Err(why) when not (the public route stays).
pub(super) fn install() -> Result<String, String> {
  let Some(cls) = AnyClass::get(CLASS) else {
    return Err("WKMouseTrackingObserver absent".into());
  };
  let mut hooks = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
  if !hooks.is_empty() {
    return Ok("WKMouseTrackingObserver present, already installed".into());
  }
  let imp: Imp = unsafe { std::mem::transmute::<EvFn, Imp>(obs_imp) };
  let mut done = Vec::new();
  for name in [c"mouseMoved:", c"mouseEntered:"] {
    let sel = Sel::register(name);
    let responds: bool = unsafe { msg_send![cls, instancesRespondToSelector: sel] };
    if !responds {
      continue;
    }
    let Some(m) = cls.instance_method(sel) else { continue };
    let cur = m.implementation();
    let inherited = cls
      .superclass()
      .and_then(|s| s.instance_method(sel))
      .map(|sm| sm.implementation() as usize)
      == Some(cur as usize);
    // Register the original first: an event may arrive as soon as ours is in.
    hooks.push((sel, cur));
    if inherited {
      let ok = unsafe { objc2::ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, sel, imp, c"v@:@".as_ptr()) }.as_bool();
      if !ok {
        hooks.retain(|h| h.0 != sel);
        continue;
      }
    } else {
      let old = unsafe { m.set_implementation(imp) };
      if let Some(h) = hooks.iter_mut().find(|h| h.0 == sel) {
        h.1 = old;
      }
    }
    done.push(name.to_string_lossy().into_owned());
  }
  if done.is_empty() {
    Err("WKMouseTrackingObserver present but has no mouseMoved:/mouseEntered:".into())
  } else {
    Ok(format!("WKMouseTrackingObserver present, swapped {}", done.join(" ")))
  }
}

/// Which registered view an observer belongs to (rescans on a miss).
fn owner_of(obs: usize) -> Option<(usize, usize)> {
  let find = || OWNERS.with(|o| o.borrow().iter().find(|t| t.0 == obs).map(|t| (t.1, t.2)));
  find().or_else(|| {
    let mut map = Vec::new();
    for (key, v) in super::all_views() {
      let areas = v.trackingAreas();
      for i in 0..areas.count() {
        let a = areas.objectAtIndex(i);
        if let Some(owner) = a.owner() {
          map.push((Retained::as_ptr(&owner) as usize, key, &*v as *const NSView as usize));
        }
      }
    }
    OWNERS.with(|o| *o.borrow_mut() = map);
    find()
  })
}

unsafe extern "C-unwind" fn obs_imp(this: *mut AnyObject, cmd: Sel, event: *mut AnyObject) {
  let orig = HOOKS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|h| h.0 == cmd).map(|h| h.1);
  let Some(orig) = orig else { return };
  let orig: EvFn = unsafe { std::mem::transmute::<Imp, EvFn>(orig) };
  let in_fwd = super::IN_FWD.with(|c| c.get());
  if !event.is_null() && !in_fwd && (cmd == sel!(mouseMoved:) || cmd == sel!(mouseEntered:)) {
    if let Some((key, view)) = owner_of(this as usize) {
      if super::drops_hover_for(view, key) {
        return;
      }
    }
  }
  unsafe { orig(this, cmd, event) }
}
