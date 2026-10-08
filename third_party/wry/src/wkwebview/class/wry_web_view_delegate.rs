// Copyright 2020-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{ffi::CStr, panic::AssertUnwindSafe};

use http::Request;
use objc2::{
  define_class, msg_send,
  rc::Retained,
  runtime::{NSObject, ProtocolObject},
  DeclaredClass, MainThreadOnly,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSObjectProtocol, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController};

pub const IPC_MESSAGE_HANDLER_NAME: &str = "ipc";

/// K2 patch (UW34): the IPC handler only hears the main frame.
#[inline]
pub(crate) fn accept_ipc_from_frame(is_main_frame: bool) -> bool {
  is_main_frame
}

pub struct WryWebViewDelegateIvars {
  pub controller: Retained<WKUserContentController>,
  pub ipc_handler: Box<dyn Fn(Request<String>)>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[thread_kind = MainThreadOnly]
  #[ivars = WryWebViewDelegateIvars]
  pub struct WryWebViewDelegate;

  unsafe impl NSObjectProtocol for WryWebViewDelegate {}

  unsafe impl WKScriptMessageHandler for WryWebViewDelegate {
    // Function for ipc handler
    #[unsafe(method(userContentController:didReceiveScriptMessage:))]
    fn did_receive(
      this: &WryWebViewDelegate,
      _controller: &WKUserContentController,
      msg: &WKScriptMessage,
    ) {
      // Safety: objc runtime calls are unsafe
      unsafe {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!(parent: None, "wry::ipc::handle").entered();

        let ipc_handler = &this.ivars().ipc_handler;
        let frame_info = msg.frameInfo();
        // K2 patch (prd-zen-user-widgets-v2 UW34 / prd-html-frame-csp F12):
        // IPC comes from the main frame only. WKUserContentController
        // exposes `window.webkit.messageHandlers.ipc` to EVERY frame, so a
        // sealed widget or HTML-file frame could post here, and the request
        // URI below would be its frame's URL. Drop anything a subframe sends
        // before it reaches the handler.
        if !accept_ipc_from_frame(frame_info.isMainFrame()) {
          #[cfg(feature = "tracing")]
          tracing::warn!("WebView dropped an IPC message from a subframe.");
          return;
        }
        let body = msg.body();
        if let Ok(body) = body.downcast::<NSString>() {
          let js_utf8 = body.UTF8String();

          let request = frame_info.request();
          // about:srcdoc and some macOS 27 callbacks hand a nil NSURL.
          // Unwrapping aborts the main thread inside didReceiveScriptMessage.
          if let Some(abs) = request.URL().and_then(|u| u.absoluteString()) {
            let url_utf8 = abs.UTF8String();
            if let (Ok(url), Ok(js)) = (
              CStr::from_ptr(url_utf8).to_str(),
              CStr::from_ptr(js_utf8).to_str(),
            ) {
              if let Ok(r) = Request::builder().uri(url).body(js.to_string()) {
                ipc_handler(r);
              } else {
                #[cfg(feature = "tracing")]
                tracing::warn!("WebView received invalid IPC request: {}", js);
              }
              return;
            }
          }
        }

        #[cfg(feature = "tracing")]
        tracing::warn!("WebView received invalid IPC call.");
      }
    }
  }
);

impl WryWebViewDelegate {
  pub fn new(
    controller: Retained<WKUserContentController>,
    ipc_handler: Box<dyn Fn(Request<String>)>,
    mtm: MainThreadMarker,
  ) -> Retained<Self> {
    let delegate = mtm
      .alloc::<WryWebViewDelegate>()
      .set_ivars(WryWebViewDelegateIvars {
        ipc_handler,
        controller,
      });

    let delegate: Retained<Self> = unsafe { msg_send![super(delegate), init] };

    let proto_delegate = ProtocolObject::from_ref(&*delegate);
    unsafe {
      // this will increase the retain count of the delegate
      let _res = objc2::exception::catch(AssertUnwindSafe(|| {
        delegate
          .ivars()
          .controller
          .addScriptMessageHandler_name(proto_delegate, ns_string!(IPC_MESSAGE_HANDLER_NAME));
      }));
    }

    delegate
  }
}
