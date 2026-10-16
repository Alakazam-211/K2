// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Linux: the container child webviews live in.
//!
//! Child webviews (`WebviewKind::WindowChild`, the `unstable` multiwebview)
//! used to be packed into the window's vertical `gtk::Box`. A box stacks
//! its children and ignores positions, so `add_child(position, size)` and
//! `set_bounds` did nothing and two webviews split the window in halves
//! (tauri#10420, #13071, #15656, #16132).
//!
//! Each window now gets one of these, created with the window and packed
//! into the window's `default_vbox` (under any menu bar), and every child
//! webview is built into it. wry places webviews in a `gtk::Fixed` with
//! `Fixed::put` / `Fixed::move_` and a size request.
//!
//! It is a `gtk::Fixed` that requests no size of its own. A plain
//! `gtk::Fixed` requests the bounding box of its children's size requests,
//! and GTK turns a window's minimum request into its minimum size: with a
//! webview filling the window, the user could never make it smaller.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
  use super::*;

  #[derive(Default)]
  pub struct ChildWebviewContainer;

  #[glib::object_subclass]
  impl ObjectSubclass for ChildWebviewContainer {
    const NAME: &'static str = "TauriChildWebviewContainer";
    type Type = super::ChildWebviewContainer;
    type ParentType = gtk::Fixed;
  }

  impl ObjectImpl for ChildWebviewContainer {}

  impl WidgetImpl for ChildWebviewContainer {
    // Children are still allocated at their own size requests (that is
    // `gtk::Fixed`'s `size_allocate`); only the container's own request is
    // zero, so it never forces a minimum window size.
    fn preferred_width(&self) -> (i32, i32) {
      (0, 0)
    }
    fn preferred_height(&self) -> (i32, i32) {
      (0, 0)
    }
    fn preferred_width_for_height(&self, _height: i32) -> (i32, i32) {
      (0, 0)
    }
    fn preferred_height_for_width(&self, _width: i32) -> (i32, i32) {
      (0, 0)
    }
  }

  impl ContainerImpl for ChildWebviewContainer {}
  impl FixedImpl for ChildWebviewContainer {}
}

glib::wrapper! {
  pub struct ChildWebviewContainer(ObjectSubclass<imp::ChildWebviewContainer>)
    @extends gtk::Fixed, gtk::Container, gtk::Widget;
}

impl Default for ChildWebviewContainer {
  fn default() -> Self {
    glib::Object::new()
  }
}

/// The container for `window`'s child webviews, packed below anything
/// already in its `default_vbox` (a menu bar), filling the rest.
pub fn new_for(window: &tao::window::Window) -> Option<gtk::Fixed> {
  use tao::platform::unix::WindowExtUnix;
  let vbox = window.default_vbox()?;
  let container = ChildWebviewContainer::default();
  vbox.pack_start(&container, true, true, 0);
  container.show();
  Some(container.upcast())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Needs a display (GTK init): on a headless box run it under `xvfb-run`.
  /// Without one it fails, loudly, rather than skipping.
  #[test]
  fn requests_no_size_but_allocates_children_at_their_requests() {
    if gtk::init().is_err() {
      panic!("GTK could not initialise: run this test with a display (xvfb-run)");
    }
    let container = ChildWebviewContainer::default();
    let fixed: &gtk::Fixed = container.upcast_ref();
    let child = gtk::DrawingArea::new();
    child.set_size_request(800, 600);
    fixed.put(&child, 40, 30);
    container.show_all();

    // The container asks for nothing, whatever its children ask for.
    let (min_w, nat_w) = container.preferred_width();
    let (min_h, nat_h) = container.preferred_height();
    assert_eq!((min_w, nat_w, min_h, nat_h), (0, 0, 0, 0));

    // A plain gtk::Fixed would ask for the children's box (the old lock).
    let plain = gtk::Fixed::new();
    let other = gtk::DrawingArea::new();
    other.set_size_request(800, 600);
    plain.put(&other, 40, 30);
    plain.show_all();
    assert_eq!(plain.preferred_width().0, 840);

    // Children still land at their position and size request.
    container.size_allocate(&gtk::Allocation::new(0, 0, 300, 200));
    let a = child.allocation();
    assert_eq!((a.x(), a.y(), a.width(), a.height()), (40, 30, 800, 600));

    // move_ is what wry's set_bounds uses; it persists across allocations.
    fixed.move_(&child, 10, 20);
    container.size_allocate(&gtk::Allocation::new(0, 0, 300, 200));
    let a = child.allocation();
    assert_eq!((a.x(), a.y()), (10, 20));
  }
}
