//! Linux: deskd as the daemon's child over pipes (§3.2), the k2-simhelper
//! way. Requests on stdin, replies and events on stdout, frames on fd 3.
//! The daemon passes the session's `DISPLAY`, `XAUTHORITY`,
//! `WAYLAND_DISPLAY` and `DBUS_SESSION_BUS_ADDRESS`. No TCC.
//!
//! The clipboard (L7) is the X11 CLIPBOARD selection, text only.
//!
//! On X11 (D8b) deskd captures and encodes: kessel-video's X11 source into
//! software VP8, and JPEG for viewers that decode no video
//! ([`capture::VP8_JPEG`], listed in `hello.codecs`). Wayland waits for
//! the portal slice (D9): there every layer fails as unsupported.
//!
//! stdout is the link: nothing else in deskd may print to it.

use std::os::fd::FromRawFd;
use std::sync::{Arc, OnceLock};

use k2_deskd_proto::msg::{codes, Display, FramesVia, ReleaseReason, Tcc};
use k2_deskd_proto::wire;

use crate::capture;
use crate::hold::LocalHold;
use crate::inject::{Injector, Post, PostError, Poster, Pressed};
use crate::notify::NotifyBanner;
use crate::server::{Deskd, Host};
use crate::x11::X11;

/// Which backend the session offers. Pure, for tests.
pub fn pick_backend(session_type: Option<&str>, display: Option<&str>, wayland: Option<&str>) -> &'static str {
    let has = |v: Option<&str>| v.is_some_and(|s| !s.is_empty());
    if session_type == Some("wayland") || (has(wayland) && !has(display)) {
        // XTEST into XWayland would drive X clients only; the portal comes
        // with D9.
        "wayland"
    } else if has(display) {
        "x11"
    } else {
        "none"
    }
}

/// Input where deskd can't post (Wayland before D9, no display).
struct NoInput(&'static str, &'static str);

impl Poster for NoInput {
    fn post(&mut self, _p: Post<'_>, _pressed: Pressed<'_>) -> Result<(), PostError> {
        Err(PostError::new(self.0, self.1))
    }
}

pub struct LinuxHost {
    backend: &'static str,
    x: Option<X11>,
    /// Text through the CLIPBOARD selection (L7); `None` without X11.
    clip: Option<crate::x11::clipboard::X11Clipboard>,
    banner: Arc<NotifyBanner>,
}

impl Host for LinuxHost {
    fn os(&self) -> &'static str {
        "linux"
    }
    fn backend(&self) -> String {
        self.backend.into()
    }
    fn frames_via(&self) -> FramesVia {
        FramesVia::Fd3
    }
    fn tcc(&self) -> Tcc {
        Tcc::NOT_REQUIRED
    }
    fn request_tcc(&self) -> Tcc {
        Tcc::NOT_REQUIRED
    }
    fn displays(&self) -> Result<Vec<Display>, String> {
        self.x.as_ref().map(X11::displays).ok_or_else(|| format!("no X11 display (backend {})", self.backend))
    }
    fn hid_idle_ms(&self) -> Option<u64> {
        self.x.as_ref().and_then(X11::idle_ms)
    }
    fn banner(&self, text: Option<String>) {
        self.banner.set(text);
    }
    fn clipboard_get(&self) -> crate::clipboard::ClipRead {
        self.clip.as_ref().map_or(crate::clipboard::ClipRead::Unsupported, |c| c.read())
    }
    fn clipboard_set(&self, text: &str) -> Result<(), (&'static str, String)> {
        match &self.clip {
            Some(c) => c.write(text),
            None => Err((codes::UNSUPPORTED, format!("no X11 clipboard (backend {})", self.backend))),
        }
    }
}

/// kessel-video's backend (its X11 source and VP8) behind the engine.
/// Without it deskd still serves input, and every layer fails as
/// unsupported.
fn start_x11_capture(deskd: &Deskd<LinuxHost>, x: &X11) {
    match kessel_video::system(kessel_video::SystemConfig::default()) {
        Ok(backend) => {
            let encoder = capture::probe_encoder_for(&*backend, k2_deskd_proto::link::Codec::Vp8);
            let opener = crate::x11::capture::X11Opener::new(Arc::clone(&backend), Arc::clone(&x.conn), x.root, x.screen);
            if let Err(e) = deskd.start_capture_with(opener, backend, encoder, capture::VP8_JPEG) {
                eprintln!("k2-deskd: capture: {e}");
            }
        }
        Err(e) => eprintln!("k2-deskd: no video backend: {e}"),
    }
}

fn fd3_open() -> bool {
    // SAFETY: F_GETFD only reads the descriptor's flags.
    unsafe { libc::fcntl(3, libc::F_GETFD) >= 0 }
}

pub fn run() -> ! {
    let env = |k: &str| std::env::var(k).ok();
    let backend = pick_backend(env("XDG_SESSION_TYPE").as_deref(), env("DISPLAY").as_deref(), env("WAYLAND_DISPLAY").as_deref());
    let hold = Arc::new(LocalHold::default());
    let (x, poster): (Option<X11>, Box<dyn Poster>) = match backend {
        "x11" => match X11::connect() {
            Ok(x) => {
                let p = crate::x11::inject::X11Poster::new(Arc::clone(&x.conn), x.root, x.width, x.height);
                if let Err(e) = crate::x11::watch::spawn(Arc::clone(&hold)) {
                    // Can't tell local input apart: count as active is the
                    // daemon's call; deskd says so on stderr.
                    eprintln!("k2-deskd: no XInput2 local-input detection: {e}");
                }
                (Some(x), Box::new(p))
            }
            Err(e) => {
                eprintln!("k2-deskd: {e}");
                (None, Box::new(NoInput(codes::NO_DISPLAY, "deskd could not open the X display")))
            }
        },
        "wayland" => (None, Box::new(NoInput(codes::UNSUPPORTED, "input on Wayland comes with the RemoteDesktop portal (D9)"))),
        _ => (None, Box::new(NoInput(codes::NO_DISPLAY, "no display in this session"))),
    };
    let backend = if backend == "x11" && x.is_none() { "none" } else { backend };
    let mut injector = Injector::new(poster);
    if x.is_some() {
        injector.release_at_startup();
    }

    let cell: Arc<OnceLock<Arc<Deskd<LinuxHost>>>> = Arc::new(OnceLock::new());
    let c2 = Arc::clone(&cell);
    let banner = NotifyBanner::new(Arc::new(move || {
        if let Some(d) = c2.get() {
            d.banner_disconnect();
        }
    }));
    // The clipboard has its own connection (it waits on selection events).
    let clip = if x.is_some() {
        crate::x11::clipboard::X11Clipboard::connect().map_err(|e| eprintln!("k2-deskd: no clipboard: {e}")).ok()
    } else {
        None
    };
    let host = Arc::new(LinuxHost { backend, x, clip, banner });
    let deskd = Arc::new(Deskd::new(host, injector, hold));
    let _ = cell.set(Arc::clone(&deskd));
    // D8b: X11 capture into VP8 and JPEG, before hello (it lists them).
    if let Some(x) = deskd.host.x.as_ref() {
        start_x11_capture(&deskd, x);
    }
    deskd.spawn_watchers();
    let d = Arc::clone(&deskd);
    crate::signals::on_sigterm(move || d.shutdown(ReleaseReason::Sigterm));

    if fd3_open() {
        // SAFETY: fd 3 is the frames pipe the daemon set up; we own it now.
        deskd.attach_frames(Box::new(unsafe { std::fs::File::from_raw_fd(3) }));
    }
    let mut out = std::io::stdout();
    if let Err(e) = wire::write_json(&mut out, &deskd.hello()) {
        eprintln!("k2-deskd: cannot write hello: {e}");
        std::process::exit(1);
    }
    // stdin closes when the daemon goes away.
    let res = deskd.serve(std::io::stdin(), Box::new(std::io::stdout()));
    deskd.shutdown(ReleaseReason::LinkClosed);
    if let Err(e) = res {
        eprintln!("k2-deskd: link ended: {e}");
        std::process::exit(1);
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_choice() {
        assert_eq!(pick_backend(Some("x11"), Some(":0"), None), "x11");
        assert_eq!(pick_backend(None, Some(":99"), None), "x11");
        assert_eq!(pick_backend(Some("wayland"), Some(":0"), Some("wayland-0")), "wayland");
        assert_eq!(pick_backend(None, None, Some("wayland-0")), "wayland");
        assert_eq!(pick_backend(Some("tty"), None, None), "none");
        assert_eq!(pick_backend(None, Some(""), None), "none");
    }
}
