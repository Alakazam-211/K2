//! macOS: deskd as a LaunchAgent in `K2 Screen.app`.
//!
//! `run()` wires it: the CGEvent poster, the event tap, the TCC re-check,
//! capture and encode (ScreenCaptureKit → VideoToolbox, [`capture`]), the
//! socket thread, SIGTERM, and AppKit's main loop for the banner.

pub mod capture;
pub mod ffi;
pub mod inject;
pub mod peer;
pub mod tap;
pub mod tcc;
pub mod ui;

use std::sync::{Arc, OnceLock};

use k2_deskd_proto::msg::{Display, FramesVia, ReleaseReason, Tcc};

use crate::inject::Injector;
use crate::server::{Deskd, Host};

pub struct MacHost {
    banner: Arc<ui::BannerShared>,
}

impl Host for MacHost {
    fn os(&self) -> &'static str {
        "macos"
    }
    fn backend(&self) -> String {
        "macos".into()
    }
    fn frames_via(&self) -> FramesVia {
        FramesVia::Fd
    }
    fn tcc(&self) -> Tcc {
        tcc::current()
    }
    fn request_tcc(&self) -> Tcc {
        tcc::request()
    }
    fn displays(&self) -> Result<Vec<Display>, String> {
        displays()
    }
    fn hid_idle_ms(&self) -> Option<u64> {
        tap::hid_idle_ms()
    }
    fn banner(&self, text: Option<String>) {
        self.banner.set(text);
    }
}

/// The active displays, main first as CG lists them.
pub fn displays() -> Result<Vec<Display>, String> {
    use ffi::*;
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: ids holds 16 entries; CG writes at most that many.
    let rc = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    if rc != 0 {
        return Err(format!("CGGetActiveDisplayList failed ({rc})"));
    }
    if n == 0 {
        return Err("no active display (a headless Mac with no framebuffer)".into());
    }
    // SAFETY: plain CG calls on ids CG just gave us.
    let main = unsafe { CGMainDisplayID() };
    Ok(ids[..n as usize]
        .iter()
        .map(|&id| unsafe {
            let b = CGDisplayBounds(id);
            let mode = CGDisplayCopyDisplayMode(id);
            let (pw, ph) = if mode.is_null() {
                (b.size.width as u32, b.size.height as u32)
            } else {
                let r = (CGDisplayModeGetPixelWidth(mode) as u32, CGDisplayModeGetPixelHeight(mode) as u32);
                CGDisplayModeRelease(mode);
                r
            };
            let scale = if b.size.width > 0.0 { pw as f64 / b.size.width } else { 1.0 };
            let name = if CGDisplayIsBuiltin(id) != 0 { "Built-in display".to_string() } else { format!("Display {id}") };
            Display { id, name, x: b.origin.x, y: b.origin.y, width_pt: b.size.width, height_pt: b.size.height, width_px: pw, height_px: ph, scale, main: id == main }
        })
        .collect())
}

/// deskd's socket: `$HOME/.k2/run/deskd.sock` (the daemon's `~/.k2`).
pub fn socket_path() -> std::path::PathBuf {
    crate::home_dir().join(".k2").join("run").join(k2_deskd_proto::SOCKET_NAME)
}

pub fn run() -> ! {
    let hold = Arc::new(crate::hold::LocalHold::default());
    let poster = match inject::MacPoster::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("k2-deskd: {}", e.message);
            std::process::exit(1);
        }
    };
    let mut injector = Injector::new(Box::new(poster));
    // vs-live R3: covers a crash that left a key or button down. Posting
    // needs Accessibility; without it this does nothing.
    injector.release_at_startup();

    let cell: Arc<OnceLock<Arc<Deskd<MacHost>>>> = Arc::new(OnceLock::new());
    let c2 = Arc::clone(&cell);
    let banner = ui::BannerShared::new(Box::new(move || {
        if let Some(d) = c2.get() {
            d.banner_disconnect();
        }
    }));
    let host = Arc::new(MacHost { banner: Arc::clone(&banner) });
    let deskd = Arc::new(Deskd::new(host, injector, Arc::clone(&hold)));
    let _ = cell.set(Arc::clone(&deskd));

    // D5: ScreenCaptureKit → VideoToolbox. The probe session tells hello
    // what the encoder runs on (R9). Without kessel-video's backend deskd
    // still serves input; every layer then fails as unsupported.
    match kessel_video::system(kessel_video::SystemConfig::default()) {
        Ok(backend) => {
            let encoder = crate::capture::probe_encoder(&*backend);
            if let Err(e) = deskd.start_capture(capture::MacOpener::new(Arc::clone(&backend)), backend, encoder) {
                eprintln!("k2-deskd: capture: {e}");
            }
        }
        Err(e) => eprintln!("k2-deskd: no video backend: {e}"),
    }

    tap::spawn(hold);
    deskd.spawn_watchers();
    let d = Arc::clone(&deskd);
    crate::signals::on_sigterm(move || d.shutdown(ReleaseReason::Sigterm));

    let d = Arc::clone(&deskd);
    std::thread::Builder::new()
        .name("k2-deskd-socket".into())
        .spawn(move || {
            let path = socket_path();
            let req = peer::requirement();
            let check: &'static crate::sock::PeerCheck = Box::leak(Box::new(move |s: &std::os::unix::net::UnixStream| peer::check_peer(s, &req)));
            if let Err(e) = crate::sock::serve_socket(&path, check, &d) {
                eprintln!("k2-deskd: socket {}: {e}", path.display());
                std::process::exit(1);
            }
        })
        .expect("spawn socket thread");

    ui::run_main_loop(banner)
}
