//! Linux X11: XTEST input, XInput2 local-input detection, XScreenSaver
//! idle, the root window as display 0, its capture (kessel-video's X11
//! source, `capture`) and the CLIPBOARD selection (`clipboard`, L7).
//!
//! Honest limit (§3.2): on X11 any process with the session cookie can
//! already read and drive the display, so deskd is a tidy code path here,
//! not a boundary.

pub mod capture;
pub mod clipboard;
pub mod inject;
pub mod watch;

use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

use k2_deskd_proto::msg::Display;

pub struct X11 {
    pub conn: Arc<RustConnection>,
    pub root: Window,
    /// The screen `DISPLAY` names (kessel-video's `x11:<screen>`).
    pub screen: usize,
    pub width: u16,
    pub height: u16,
}

impl X11 {
    pub fn connect() -> Result<X11, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
        let s = &conn.setup().roots[screen];
        let (root, width, height) = (s.root, s.width_in_pixels, s.height_in_pixels);
        Ok(X11 { conn: Arc::new(conn), root, screen, width, height })
    }

    /// The root window's size now (a RandR change shows here), else the
    /// size at connect.
    pub fn size(&self) -> (u16, u16) {
        self.conn.get_geometry(self.root).ok().and_then(|c| c.reply().ok()).map_or((self.width, self.height), |g| (g.width, g.height))
    }

    pub fn displays(&self) -> Vec<Display> {
        let (width, height) = self.size();
        vec![Display {
            id: 0,
            name: "X11 screen".into(),
            x: 0.0,
            y: 0.0,
            width_pt: width as f64,
            height_pt: height as f64,
            width_px: width as u32,
            height_px: height as u32,
            scale: 1.0,
            main: true,
        }]
    }

    /// XScreenSaver's idle (XTEST input resets it; see watch.rs).
    pub fn idle_ms(&self) -> Option<u64> {
        let r = self.conn.screensaver_query_info(self.root).ok()?.reply().ok()?;
        Some(r.ms_since_user_input as u64)
    }
}
