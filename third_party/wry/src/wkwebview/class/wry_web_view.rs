// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{collections::HashMap, sync::Mutex};

#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
use objc2::{define_class, rc::Retained, runtime::Bool, DeclaredClass};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSDraggingDestination, NSEvent, NSTrackingArea, NSView};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSSize};
use objc2_foundation::{NSObjectProtocol, NSUUID};

#[cfg(target_os = "ios")]
use crate::wkwebview::ios::WKWebView::WKWebView;
#[cfg(target_os = "macos")]
use crate::{
  wkwebview::{
    drag_drop, synthetic_mouse_events,
    underlay::{self, Fwd, Route},
  },
  DragDropEvent,
};
#[cfg(target_os = "ios")]
use objc2_ui_kit::UIEvent as NSEvent;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

pub struct WryWebViewIvars {
  pub(crate) is_child: bool,
  #[cfg(target_os = "macos")]
  pub(crate) drag_drop_handler: Box<dyn Fn(DragDropEvent) -> bool>,
  #[cfg(target_os = "macos")]
  pub(crate) accept_first_mouse: objc2::runtime::Bool,
  #[cfg(target_os = "ios")]
  pub(crate) input_accessory_view_builder: Option<Box<crate::InputAccessoryViewBuilder>>,
  pub(crate) custom_protocol_task_ids: Mutex<HashMap<usize, Retained<NSUUID>>>,
}

define_class!(
  #[unsafe(super(WKWebView))]
  #[ivars = WryWebViewIvars]
  pub struct WryWebView;

  /// Overridden NSView methods.
  impl WryWebView {
    #[unsafe(method(performKeyEquivalent:))]
    fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
      // This is a temporary workaround for https://github.com/tauri-apps/tauri/issues/9426
      // FIXME: When the webview is a child webview, performKeyEquivalent always return YES
      // and stop propagating the event to the window, hence the menu shortcut won't be
      // triggered. However, overriding this method also means the cmd+key event won't be
      // handled in webview, which means the key cannot be listened by JavaScript.
      if self.ivars().is_child {
        Bool::NO
      } else {
        unsafe { objc2::msg_send![super(self), performKeyEquivalent: event] }
      }
    }

    #[cfg(target_os = "macos")]
    #[unsafe(method(acceptsFirstMouse:))]
    fn accept_first_mouse(&self, _event: &NSEvent) -> Bool {
      // K2 underlay: a page's first click in an inactive window only
      // activates it (one switch, default off).
      if self.ivars().is_child {
        if let Some(on) = underlay::page_first_mouse(self.as_view()) {
          return Bool::new(on);
        }
      }
      self.ivars().accept_first_mouse
    }

    #[cfg(target_os = "ios")]
    #[unsafe(method_id(inputAccessoryView))]
    fn input_accessory_view(&self) -> Option<Retained<objc2_ui_kit::UIView>> {
      if let Some(builder) = &self.ivars().input_accessory_view_builder {
        builder(self)
      } else {
        unsafe { objc2::msg_send![super(self), inputAccessoryView] }
      }
    }
  }
  unsafe impl NSObjectProtocol for WryWebView {}

  // Drag & Drop
  #[cfg(target_os = "macos")]
  unsafe impl NSDraggingDestination for WryWebView {
    #[unsafe(method(draggingEntered:))]
    fn dragging_entered(
      &self,
      drag_info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>,
    ) -> objc2_app_kit::NSDragOperation {
      drag_drop::dragging_entered(self, drag_info)
    }

    #[unsafe(method(draggingUpdated:))]
    fn dragging_updated(
      &self,
      drag_info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>,
    ) -> objc2_app_kit::NSDragOperation {
      drag_drop::dragging_updated(self, drag_info)
    }

    #[unsafe(method(performDragOperation:))]
    fn perform_drag_operation(
      &self,
      drag_info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>,
    ) -> Bool {
      drag_drop::perform_drag_operation(self, drag_info)
    }

    #[unsafe(method(draggingExited:))]
    fn dragging_exited(&self, drag_info: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>) {
      drag_drop::dragging_exited(self, drag_info)
    }
  }

  // K2 underlay (macOS): routing to the page under the UI view. A window
  // with no underlay set takes the stock path (super) at the first check.
  #[cfg(target_os = "macos")]
  impl WryWebView {
    #[unsafe(method(hitTest:))]
    fn hit_test(&self, point: NSPoint) -> *mut NSView {
      let sup: *mut NSView = unsafe { objc2::msg_send![super(self), hitTest: point] };
      if !sup.is_null() && underlay::hit_test_passes(self.as_view(), point) {
        return std::ptr::null_mut();
      }
      sup
    }

    #[unsafe(method(scrollWheel:))]
    fn scroll_wheel(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Scroll, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), scrollWheel: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, scrollWheel: event] }),
      }
    }

    #[unsafe(method(magnifyWithEvent:))]
    fn magnify_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Gesture(0), false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), magnifyWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, magnifyWithEvent: event] }),
      }
    }

    #[unsafe(method(rotateWithEvent:))]
    fn rotate_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Gesture(1), false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), rotateWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, rotateWithEvent: event] }),
      }
    }

    #[unsafe(method(smartMagnifyWithEvent:))]
    fn smart_magnify_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), smartMagnifyWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, smartMagnifyWithEvent: event] }),
      }
    }

    #[unsafe(method(swipeWithEvent:))]
    fn swipe_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), swipeWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, swipeWithEvent: event] }),
      }
    }

    #[unsafe(method(beginGestureWithEvent:))]
    fn begin_gesture_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), beginGestureWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, beginGestureWithEvent: event] }),
      }
    }

    #[unsafe(method(endGestureWithEvent:))]
    fn end_gesture_with_event(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), endGestureWithEvent: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, endGestureWithEvent: event] }),
      }
    }

    #[unsafe(method(mouseMoved:))]
    fn mouse_moved(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, true) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), mouseMoved: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, mouseMoved: event] }),
      }
    }

    #[unsafe(method(rightMouseDown:))]
    fn right_mouse_down(&self, event: &NSEvent) {
      match underlay::route(self.as_view(), event, Route::Location, false) {
        Fwd::Super => unsafe { objc2::msg_send![super(self), rightMouseDown: event] },
        Fwd::To(v) => underlay::forward(|| unsafe { objc2::msg_send![&*v, rightMouseDown: event] }),
      }
    }

    // Hover, public route: while this view doesn't own the pointer its
    // tracking areas are lifted; WebKit re-adding one is kept aside too.
    #[unsafe(method(addTrackingArea:))]
    fn add_tracking_area(&self, area: &NSTrackingArea) {
      if !underlay::tracking_area_added(self.as_view(), area) {
        unsafe { objc2::msg_send![super(self), addTrackingArea: area] }
      }
    }

    #[unsafe(method(removeTrackingArea:))]
    fn remove_tracking_area(&self, area: &NSTrackingArea) {
      underlay::tracking_area_removed(self.as_view(), area);
      unsafe { objc2::msg_send![super(self), removeTrackingArea: area] }
    }

    #[unsafe(method(updateTrackingAreas))]
    fn update_tracking_areas(&self) {
      unsafe { objc2::msg_send![super(self), updateTrackingAreas] }
      underlay::tracking_areas_updated(self.as_view());
    }

    // Same-pass resize: the UI view's mask follows the new frames in this
    // layout pass, never a frame later.
    #[unsafe(method(setFrameSize:))]
    fn set_frame_size(&self, size: NSSize) {
      unsafe { objc2::msg_send![super(self), setFrameSize: size] }
      underlay::frame_changed(self.as_view());
    }

    // A page hiding (K2's freeze-frame) closes its hole in the same pass.
    #[unsafe(method(setHidden:))]
    fn set_hidden(&self, hidden: bool) {
      unsafe { objc2::msg_send![super(self), setHidden: hidden] }
      underlay::frame_changed(self.as_view());
    }
  }

  // Synthetic mouse events
  #[cfg(target_os = "macos")]
  impl WryWebView {
    #[unsafe(method(otherMouseDown:))]
    fn other_mouse_down(&self, event: &NSEvent) {
      synthetic_mouse_events::other_mouse_down(self, event)
    }

    #[unsafe(method(otherMouseUp:))]
    fn other_mouse_up(&self, event: &NSEvent) {
      synthetic_mouse_events::other_mouse_up(self, event)
    }
  }
);

#[cfg(target_os = "macos")]
impl WryWebView {
  fn as_view(&self) -> &NSView {
    // WryWebView -> WKWebView -> NSView.
    self
  }
}

// Custom Protocol Task Checker
impl WryWebView {
  pub(crate) fn add_custom_task_key(&self, task_id: usize) -> Retained<NSUUID> {
    let task_uuid = NSUUID::new();
    self
      .ivars()
      .custom_protocol_task_ids
      .lock()
      .unwrap()
      .insert(task_id, task_uuid.clone());
    task_uuid
  }
  pub(crate) fn remove_custom_task_key(&self, task_id: usize) {
    self
      .ivars()
      .custom_protocol_task_ids
      .lock()
      .unwrap()
      .remove(&task_id);
  }
  pub(crate) fn get_custom_task_uuid(&self, task_id: usize) -> Option<Retained<NSUUID>> {
    self
      .ivars()
      .custom_protocol_task_ids
      .lock()
      .unwrap()
      .get(&task_id)
      .cloned()
  }
}
