//! X11 capture against a real X server. Each test starts its own Xvfb
//! (`-displayfd`, so they can run side by side) and drives it as an X
//! client. They are ignored by default because they need the Xvfb binary;
//! run them with
//!
//!   cargo test -p kessel-video --lib -- --include-ignored linux::x11::
//!
//! (`xrandr` is needed for the size change.) The pure parts at the bottom
//! always run.

use super::*;
use crate::api::no_wake;
use crate::kit::wait_for;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use x11rb::protocol::xproto::{CreateGCAux, CreateWindowAux, Rectangle, Screen, WindowClass};
use x11rb::wrapper::ConnectionExt as _;

/// An Xvfb of its own, killed on drop.
struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    fn start(w: u32, h: u32) -> Xvfb {
        let (mut rx, tx) = std::io::pipe().expect("a pipe");
        let fd = tx.as_raw_fd();
        let mut cmd = Command::new("Xvfb");
        cmd.args(["-displayfd", &fd.to_string(), "-screen", "0", &format!("{w}x{h}x24"), "-nolisten", "tcp"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only fcntl on the inherited write end, in the child.
        unsafe {
            cmd.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("Xvfb (apt install xvfb)");
        drop(tx);
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        while rx.read(&mut b).unwrap_or(0) == 1 && b[0] != b'\n' {
            line.push(b[0]);
        }
        let n = String::from_utf8_lossy(&line).trim().to_string();
        assert!(!n.is_empty(), "Xvfb didn't report a display");
        Xvfb { child, display: format!(":{n}") }
    }

    fn d(&self) -> Option<&str> {
        Some(&self.display)
    }

    fn client(&self) -> (RustConnection, Screen) {
        let (conn, screen) = x11rb::connect(self.d()).expect("connect to Xvfb");
        let s = conn.setup().roots[screen].clone();
        (conn, s)
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn constraints(cursor: bool) -> CaptureConstraints {
    let mut c = CaptureConstraints::new(1920, 1080, 30);
    c.cursor = cursor;
    c
}

fn next_frame(src: &mut Box<dyn FrameSource>, within: Duration) -> Option<VideoFrame> {
    wait_for(within, || src.try_recv())
}

/// The frame's (B, G, R) at `x`, `y`.
fn pixel(f: &VideoFrame, x: u32, y: u32) -> [u8; 3] {
    let p = f.planes().expect("CPU planes");
    assert_eq!(p.format, PixelFormat::Bgra);
    let at = y as usize * p.strides[0] as usize + x as usize * 4;
    [p.data[at], p.data[at + 1], p.data[at + 2]]
}

const RED: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];

/// A mapped window with a solid background (TrueColor pixel `rgb`).
fn window(conn: &RustConnection, s: &Screen, rect: (i16, i16, u16, u16), rgb: u32) -> Window {
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(rgb).override_redirect(1);
    conn.create_window(s.root_depth, win, s.root, rect.0, rect.1, rect.2, rect.3, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.map_window(win).unwrap();
    conn.sync().unwrap();
    win
}

/// A solid 16x16 cursor of `rgb` (16-bit channels) with its hot spot at
/// the centre.
fn solid_cursor(conn: &RustConnection, s: &Screen, rgb: (u16, u16, u16)) -> u32 {
    let pm = conn.generate_id().unwrap();
    conn.create_pixmap(1, pm, s.root, 16, 16).unwrap();
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, pm, &CreateGCAux::new().foreground(1)).unwrap();
    conn.poly_fill_rectangle(pm, gc, &[Rectangle { x: 0, y: 0, width: 16, height: 16 }]).unwrap();
    let cur = conn.generate_id().unwrap();
    conn.create_cursor(cur, pm, pm, rgb.0, rgb.1, rgb.2, 0, 0, 0, 8, 8).unwrap();
    conn.sync().unwrap();
    cur
}

fn warp(conn: &RustConnection, s: &Screen, x: i16, y: i16) {
    conn.warp_pointer(x11rb::NONE, s.root, 0, 0, 0, 0, x, y).unwrap();
    conn.sync().unwrap();
}

#[test]
#[ignore = "needs Xvfb: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn the_screen_is_listed_and_opens_without_a_picker() {
    let x = Xvfb::start(1280, 720);
    let list = sources_on(x.d());
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!((list[0].id.as_str(), list[0].kind, list[0].width, list[0].height), ("x11:0", ScreenKind::Display, 1280, 720));
    let mut src = open_on(x.d(), "x11:0", constraints(false), no_wake()).expect("opens with no picker");
    let f = next_frame(&mut src, Duration::from_secs(5)).expect("the first frame comes at once");
    assert_eq!((f.width, f.height, f.format()), (1280, 720, PixelFormat::Bgra));
    assert_eq!(src.try_event(), Some(SourceEvent::Started { width: 1280, height: 720, fps: 30 }));
    for bad in ["x11:3", "x11:", "x11:zero", "display:0"] {
        assert_eq!(open_on(x.d(), bad, constraints(false), no_wake()).err(), Some(VideoError::NotFound), "{bad}");
    }
    assert!(sources_on(Some(":4242")).is_empty(), "no server there: nothing listed");
}

#[test]
#[ignore = "needs Xvfb: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn no_frame_without_damage() {
    let x = Xvfb::start(1280, 720);
    let mut src = open_on(x.d(), "x11:0", constraints(false), no_wake()).expect("open");
    next_frame(&mut src, Duration::from_secs(5)).expect("the first frame");
    // Nothing draws on a bare Xvfb: no damage, no frame.
    assert!(next_frame(&mut src, Duration::from_millis(1500)).is_none(), "a frame with nothing changed");
    assert_eq!(src.stats().frames, 1);
}

#[test]
#[ignore = "needs Xvfb: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn a_frame_after_a_window_maps() {
    let x = Xvfb::start(1280, 720);
    let mut src = open_on(x.d(), "x11:0", constraints(false), no_wake()).expect("open");
    let first = next_frame(&mut src, Duration::from_secs(5)).expect("the first frame");
    assert_ne!(pixel(&first, 150, 150), RED);
    assert!(next_frame(&mut src, Duration::from_millis(300)).is_none(), "quiet before the window");
    let (conn, s) = x.client();
    window(&conn, &s, (100, 100, 200, 200), 0xff_0000);
    let f = wait_for(Duration::from_secs(5), || src.try_recv().filter(|f| pixel(f, 150, 150) == RED)).expect("a frame showing the window");
    assert_eq!(pixel(&f, 50, 50), pixel(&first, 50, 50), "outside the window nothing changed");
    assert_eq!(pixel(&f, 299, 299), RED);
}

#[test]
#[ignore = "needs Xvfb: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn the_cursor_image_is_read_and_drawn_where_the_pointer_is() {
    let x = Xvfb::start(1280, 720);
    let (conn, s) = x.client();
    let win = window(&conn, &s, (0, 0, 1280, 720), 0);
    let cur = solid_cursor(&conn, &s, (0, 0xffff, 0));
    conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().cursor(cur)).unwrap();
    warp(&conn, &s, 400, 300);

    let mut g = Grabber::connect(x.d(), 0).expect("grabber");
    let img = wait_for(Duration::from_secs(5), || {
        g.cursor_stale = true;
        g.cursor().filter(|c| (c.width, c.height) == (16, 16)).cloned()
    })
    .expect("XFIXES gives the window's cursor");
    assert_eq!((img.xhot, img.yhot), (8, 8));
    assert!(img.pixels.iter().all(|p| *p == 0xff00_ff00), "solid opaque green");

    // Not in the picture without the cursor.
    let plain = g.frame(false).expect("a frame");
    assert_eq!(pixel(&plain, 400, 300), BLACK);

    let mut src = open_on(x.d(), "x11:0", constraints(true), no_wake()).expect("open");
    let f = next_frame(&mut src, Duration::from_secs(5)).expect("the first frame");
    assert_eq!(pixel(&f, 400, 300), GREEN, "drawn at the hot spot");
    assert_eq!(pixel(&f, 392, 292), GREEN, "its top-left corner");
    assert_eq!(pixel(&f, 391, 300), BLACK, "and no wider");

    // A pointer move damages nothing, but the picture changed.
    warp(&conn, &s, 600, 300);
    let f = wait_for(Duration::from_secs(5), || src.try_recv().filter(|f| pixel(f, 600, 300) == GREEN)).expect("a frame for the move");
    assert_eq!(pixel(&f, 400, 300), BLACK, "gone from where it was");
}

fn xrandr(display: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("xrandr").arg("-d").arg(display).args(args).output().map_err(|e| format!("xrandr (apt install x11-xserver-utils): {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("xrandr {args:?}: {text}"))
    }
}

/// Resizes the X screen to `w` x `h` with RandR: the framebuffer alone,
/// else with the output switched off first (Xvfb keeps its one mode at
/// the start size, so its CRTC must let go before the screen shrinks; it
/// can't grow past the start size, and it refuses new modes). The error
/// says what this X server refused.
fn resize(display: &str, w: u32, h: u32) -> Result<(), String> {
    let fb = format!("{w}x{h}");
    let Err(first) = xrandr(display, &["--fb", &fb]) else { return Ok(()) };
    let q = xrandr(display, &["-q"])?;
    let output = q.lines().find(|l| l.contains(" connected")).and_then(|l| l.split_whitespace().next()).ok_or(format!("no connected output in: {q}"))?.to_string();
    xrandr(display, &["--output", &output, "--off", "--fb", &fb]).map(|_| ()).map_err(|e| format!("--fb alone: {first}\nwith the output off: {e}\nxrandr -q: {q}"))
}

#[test]
#[ignore = "needs Xvfb and xrandr: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn a_size_change_is_reported() {
    let x = Xvfb::start(1280, 720);
    let mut src = open_on(x.d(), "x11:0", constraints(false), no_wake()).expect("open");
    next_frame(&mut src, Duration::from_secs(5)).expect("the first frame");
    assert_eq!(src.try_event(), Some(SourceEvent::Started { width: 1280, height: 720, fps: 30 }));
    // Smaller, then back to the start size (Xvfb's largest).
    for (w, h) in [(1024, 576), (1280, 720)] {
        if let Err(e) = resize(&x.display, w, h) {
            panic!("this X server can't change size to {w}x{h}: {e}");
        }
        let f = wait_for(Duration::from_secs(5), || src.try_recv().filter(|f| f.width == w)).unwrap_or_else(|| panic!("a frame at {w}x{h}"));
        assert_eq!((f.width, f.height), (w, h));
        let ev = wait_for(Duration::from_secs(1), || src.try_event()).expect("an event");
        assert_eq!(ev, SourceEvent::Started { width: w, height: h, fps: 30 });
        assert_eq!((sources_on(x.d())[0].width, sources_on(x.d())[0].height), (w, h), "the list has the new size too");
        eprintln!("[x11] resized to {w}x{h}: reported");
    }
}

#[test]
#[ignore = "needs Xvfb: cargo test -p kessel-video --lib -- --include-ignored linux::x11::"]
fn the_rate_is_kept_while_the_screen_keeps_changing() {
    let x = Xvfb::start(640, 360);
    let mut c = constraints(false);
    c.fps = Some(10);
    let mut src = open_on(x.d(), "x11:0", c, no_wake()).expect("open");
    next_frame(&mut src, Duration::from_secs(5)).expect("the first frame");
    let (conn, s) = x.client();
    let t0 = std::time::Instant::now();
    let mut frames = 0;
    let mut i = 0u32;
    while t0.elapsed() < Duration::from_secs(2) {
        // A new colour every ~5 ms: far more changes than frames.
        window(&conn, &s, (0, 0, 64, 64), (i * 0x01_03_07) & 0xff_ffff);
        i += 1;
        while src.try_recv().is_some() {
            frames += 1;
        }
        let spin = std::time::Instant::now();
        while spin.elapsed() < Duration::from_millis(5) {
            std::hint::spin_loop();
        }
    }
    assert!((15..=22).contains(&frames), "{frames} frames in 2 s at 10 fps");
}

#[test]
fn ids_are_parsed_strictly() {
    assert_eq!(parse_id("x11:0"), Some(0));
    assert_eq!(parse_id("x11:2"), Some(2));
    assert_eq!(parse_id("x11:"), None);
    assert_eq!(parse_id("display:0"), None);
    assert!(is_x11_id("x11:0") && !is_x11_id("portal"));
}

#[test]
fn the_cursor_is_blended_and_clipped() {
    let (w, h) = (4u32, 3u32);
    let mut data = vec![100u8; (w * h * 4) as usize];
    // 2x2: opaque red, half-transparent white (premultiplied), clear, opaque blue.
    let img = CursorImage { width: 2, height: 2, xhot: 1, yhot: 1, serial: 1, pixels: vec![0xffff_0000, 0x8080_8080, 0, 0xff00_00ff] };
    draw_cursor(&mut data, w, h, (1, 1), &img);
    let px = |x: u32, y: u32| {
        let at = ((y * w + x) * 4) as usize;
        [data[at], data[at + 1], data[at + 2]]
    };
    assert_eq!(px(0, 0), [0, 0, 255], "opaque red as BGR");
    assert_eq!(px(1, 0), [178, 178, 178], "0x80 + 100 * 127 / 255");
    assert_eq!(px(0, 1), [100, 100, 100], "a clear pixel leaves the screen");
    assert_eq!(px(1, 1), [255, 0, 0], "opaque blue as BGR");
    // Hanging off the top-left corner: only the inside part is drawn.
    let mut data2 = vec![0u8; (w * h * 4) as usize];
    draw_cursor(&mut data2, w, h, (0, 0), &img);
    assert_eq!(&data2[..3], &[255, 0, 0], "only the blue corner lands at 0,0");
    draw_cursor(&mut data2, w, h, (100, -5), &img);
}
