//! Calls in this shell's webview. `k2 app init --template tauri --voice` wrote
//! this file; it is yours to change.
//!
//! Your page runs Sew's browser voice kit (WebRTC, an AudioWorklet and
//! WebAssembly) inside the webview: WKWebView on macOS, WebView2 on Windows.
//! This file decides who may use the mic and camera: your App's own pages
//! get them without a second prompt, any other page is refused.
//!
//! Wire it in src-tauri/src/lib.rs (needs Tauri 2.12 or newer):
//!
//!     mod k2_voice;
//!
//!     tauri::Builder::default()
//!         .on_permission_request(k2_voice::on_permission_request)
//!
//! A window that your Rust code builds (not one listed in tauri.conf.json):
//! pass its builder through `k2_voice::voice_window`.
//!
//! macOS still asks once for the microphone (the system prompt, with
//! NSMicrophoneUsageDescription from Info.plist). Windows needs Settings >
//! Privacy > Microphone > "Let desktop apps access your microphone" on.

use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{Manager, Runtime, Url, Webview, WebviewWindowBuilder};

/// The origins whose pages may use the mic and camera: `https://` plus a
/// host, no path. Add each custom domain the window may load.
pub const APP_ORIGINS: &[&str] = &["https://<your-app-host>"];

/// WebView2's browser arguments for calls (tauri.conf.json carries the same
/// line as additionalBrowserArgs). Setting any replaces wry's defaults, so
/// its `--disable-features` line comes first. Every window that shares a
/// data directory must use the same line.
pub const WEBVIEW2_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows";

/// For `tauri::Builder::on_permission_request`.
pub fn on_permission_request<R: Runtime>(webview: Webview<R>, kind: PermissionKind) -> PermissionResponse {
    decide(webview.url().ok().as_ref(), kind, APP_ORIGINS)
}

/// The mic and camera: allowed when the window's page is on one of
/// `origins`, refused anywhere else. Other permissions: the webview decides.
pub fn decide(page: Option<&Url>, kind: PermissionKind, origins: &[&str]) -> PermissionResponse {
    match kind {
        PermissionKind::Microphone | PermissionKind::Camera => {
            if page.is_some_and(|u| is_app_origin(u, origins)) {
                PermissionResponse::Allow
            } else {
                PermissionResponse::Deny
            }
        }
        _ => PermissionResponse::Default,
    }
}

/// `page`'s origin (scheme, host and port) is one of `origins`.
pub fn is_app_origin(page: &Url, origins: &[&str]) -> bool {
    let origin = page.origin().ascii_serialization();
    origins.iter().any(|o| o.trim_end_matches('/').eq_ignore_ascii_case(&origin))
}

/// A window built in Rust: the same settings tauri.conf.json gives the
/// first window (no slowing while hidden on macOS 14+, the WebView2 line).
#[allow(dead_code)]
pub fn voice_window<'a, R: Runtime, M: Manager<R>>(b: WebviewWindowBuilder<'a, R, M>) -> WebviewWindowBuilder<'a, R, M> {
    b.background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .additional_browser_args(WEBVIEW2_ARGS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_own_pages_get_the_mic() {
        let own = Url::parse("https://acme.example.com/room/1").unwrap();
        let other = Url::parse("https://evil.example.net/").unwrap();
        let origins = &["https://acme.example.com"];
        assert_eq!(decide(Some(&own), PermissionKind::Microphone, origins), PermissionResponse::Allow);
        assert_eq!(decide(Some(&own), PermissionKind::Camera, origins), PermissionResponse::Allow);
        assert_eq!(decide(Some(&other), PermissionKind::Microphone, origins), PermissionResponse::Deny);
        assert_eq!(decide(None, PermissionKind::Microphone, origins), PermissionResponse::Deny);
        assert_eq!(decide(Some(&own), PermissionKind::Geolocation, origins), PermissionResponse::Default);
        let http = Url::parse("http://acme.example.com/").unwrap();
        assert_eq!(decide(Some(&http), PermissionKind::Microphone, origins), PermissionResponse::Deny, "the scheme counts");
        let port = Url::parse("https://acme.example.com:8443/").unwrap();
        assert_eq!(decide(Some(&port), PermissionKind::Microphone, origins), PermissionResponse::Deny, "the port counts");
    }
}
