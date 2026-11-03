// K2 patch, third_party/upstream/wry-0005-empty-uri-path-guard.patch.
// REMOVE ON THE TAURI 2.12.5 MOVE (0.46.9): delete this file, its `mod` line in
// lib.rs and the two call sites (wkwebview/class/url_scheme_handler.rs,
// webkitgtk/web_context.rs). Tauri 2.12.4 has the real fix (tauri#16197).

//! A custom-protocol request whose URI has an empty path never reaches its
//! handler.
//!
//! Tauri 2.12.0's IPC and asset handlers run `&uri.path().as_bytes()[1..]`, and
//! `ipc:foo` parses as authority-form, whose path is `""`, so a page that POSTs
//! to it panics the handler. On macOS that handler runs under an `extern "C"`
//! callback with no `catch_unwind`, so the panic aborts the app. Real URLs such
//! as `tauri://localhost` and `ipc://localhost/cmd` have a path of at least `/`.

/// The request has no URI path at all (`ipc:foo`, `ipc:`, `localhost:80`).
pub(crate) fn has_empty_uri_path<T>(request: &http::Request<T>) -> bool {
  request.uri().path().is_empty()
}

#[cfg(test)]
mod tests {
  use super::has_empty_uri_path;
  use http::Request;

  /// Tauri 2.12.0 `src/ipc/protocol.rs:442` and `src/protocol/asset.rs:40`: the
  /// slice the guard stands in front of, copied as it ships.
  fn tauri_2_12_0_handler(request: Request<Vec<u8>>) -> String {
    let (parts, _body) = request.into_parts();
    String::from_utf8_lossy(&parts.uri.path().as_bytes()[1..]).to_string()
  }

  /// Both call sites in wry: refuse, or call the handler.
  fn through_the_guard(uri: &str) -> Result<String, &'static str> {
    let request = Request::builder()
      .uri(uri)
      .method("POST")
      .body(Vec::new())
      .unwrap_or_else(|e| panic!("{uri} must build a request: {e}"));
    if has_empty_uri_path(&request) {
      return Err("refused");
    }
    Ok(tauri_2_12_0_handler(request))
  }

  #[test]
  fn an_empty_uri_path_never_reaches_the_handler() {
    for uri in ["ipc:foo", "ipc:", "asset:foo", "localhost:80"] {
      assert_eq!(through_the_guard(uri), Err("refused"), "{uri}");
    }
  }

  /// The slice really does panic on this URI, so the guard is what stands
  /// between it and the handler. Fails if `http` ever changes the parse, which
  /// would mean the guard can go.
  #[test]
  #[should_panic(expected = "out of range")]
  fn the_unguarded_handler_panics_on_ipc_foo() {
    let request = Request::builder()
      .uri("ipc:foo")
      .method("POST")
      .body(Vec::new())
      .expect("ipc:foo builds a request");
    tauri_2_12_0_handler(request);
  }

  #[test]
  fn real_urls_still_reach_the_handler() {
    for (uri, command) in [
      ("tauri://localhost", ""),
      ("tauri://localhost/", ""),
      ("ipc://localhost/plugin%3Aupdater%7Ccheck", "plugin%3Aupdater%7Ccheck"),
      ("http://ipc.localhost/cmd", "cmd"),
      ("asset://localhost/%2Ftmp%2Fa.png", "%2Ftmp%2Fa.png"),
    ] {
      assert_eq!(through_the_guard(uri), Ok(command.to_string()), "{uri}");
    }
  }
}
