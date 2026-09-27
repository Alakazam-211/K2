//! Cmd+L address focus.
//!
//! `Webview::set_focus` ends in `wry`'s `focus()`, which calls
//! `-[NSWindow makeFirstResponder:]` and drops the BOOL (it always
//! returns `Ok(())`). A refused resign must not select the address.
//! This command is the only caller that returns that BOOL. The
//! renderer selects only when it is YES.

/// Make the invoking UI webview first responder and return whether
/// AppKit accepted it. Off macOS there is no BOOL; selecting is allowed.
#[tauri::command]
pub async fn ui_webview_make_first_responder(webview: tauri::Webview) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        webview
            .with_webview(move |platform| {
                let yes = make_first_responder_yes(&platform);
                let _ = tx.send(yes);
            })
            .map_err(|e| e.to_string())?;
        // The oneshot fires only after `makeFirstResponder:` has returned.
        // Do not wait on `Webview::set_focus`; that promise is not the BOOL.
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(yes)) => Ok(yes),
            Ok(Err(_)) => Err("webview closed before makeFirstResponder".into()),
            Err(_) => Err("makeFirstResponder timed out".into()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = webview;
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn make_first_responder_yes(webview: &tauri::webview::PlatformWebview) -> bool {
    use cocoa::base::{id, nil, BOOL, YES};

    let ns_window = webview.ns_window() as id;
    let wk = webview.inner() as id;
    if ns_window == nil || wk == nil {
        return false;
    }
    // Same call as wry `focus()`, but keep the BOOL. A NO is not a resign.
    let accepted: BOOL = unsafe { msg_send![ns_window, makeFirstResponder: wk] };
    accepted == YES
}
