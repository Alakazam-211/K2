//! An X11 client that keeps the screen changing, for the live capture
//! tests (Linux, D8b): a window over the whole screen (or a part of it)
//! redrawn with moving stripes and a moving block, `--fps` times a second,
//! until it is killed. Paced by spinning, not sleeping: sleeps are
//! coalesced in compute jobs (a 16 ms sleep can take ~55 ms).
//!
//!   DISPLAY=:99 x11_animate [--fps 30] [--size WxH]
//!
//! Prints `ready` on stdout once its window is mapped.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("x11_animate runs on Linux only");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    use std::io::Write;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ChangeGCAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, Rectangle, WindowClass};
    use x11rb::protocol::Event;
    use x11rb::wrapper::ConnectionExt as _;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let fps: u64 = arg("--fps").and_then(|v| v.parse().ok()).unwrap_or(30).clamp(1, 240);
    let (conn, screen) = x11rb::connect(None).expect("DISPLAY names no X server");
    let s = conn.setup().roots[screen].clone();
    let (w, h) = arg("--size")
        .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse::<u16>().ok()?, b.parse::<u16>().ok()?))))
        .unwrap_or((s.width_in_pixels, s.height_in_pixels));
    let win = conn.generate_id().unwrap();
    conn.create_window(s.root_depth, win, s.root, 0, 0, w, h, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().background_pixel(0).event_mask(EventMask::STRUCTURE_NOTIFY).override_redirect(1))
        .unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    loop {
        if let Event::MapNotify(_) = conn.wait_for_event().unwrap() {
            break;
        }
    }
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, win, &CreateGCAux::new().foreground(0)).unwrap();
    println!("ready");
    let _ = std::io::stdout().flush();
    let period = Duration::from_nanos(1_000_000_000 / fps);
    let mut next = Instant::now();
    let mut n: u32 = 0;
    loop {
        // Stripes 64 px wide that slide 8 px a frame, and a block that moves.
        let shift = (n * 8 % 128) as i16;
        for (i, x) in (-128i16..w as i16).step_by(64).enumerate() {
            let colour = if i % 2 == 0 { 0x20_4080 + (n % 64) * 0x0101 } else { 0xd0_c0a0 };
            conn.change_gc(gc, &ChangeGCAux::new().foreground(colour)).unwrap();
            conn.poly_fill_rectangle(win, gc, &[Rectangle { x: x + shift, y: 0, width: 64, height: h }]).unwrap();
        }
        conn.change_gc(gc, &ChangeGCAux::new().foreground(0xff_ffff)).unwrap();
        let bx = ((n * 13) % (w.max(200) as u32 - 100)) as i16;
        let by = ((n * 7) % (h.max(200) as u32 - 100)) as i16;
        conn.poly_fill_rectangle(win, gc, &[Rectangle { x: bx, y: by, width: 100, height: 100 }]).unwrap();
        if conn.sync().is_err() {
            return;
        }
        n = n.wrapping_add(1);
        next += period;
        while Instant::now() < next {
            std::hint::spin_loop();
        }
        if Instant::now() > next + period {
            next = Instant::now();
        }
    }
}
