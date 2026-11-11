//! Live test of D5's capture half on a real Mac: ScreenCaptureKit display
//! capture through deskd's own opener (`mac::capture::MacOpener`: native
//! buffers, `minimumFrameInterval` from the layer rate, deskd's windows
//! left out), deskd's capture engine, VideoToolbox, protocol-4 units on a
//! real pipe, read back with the engine's reader. The screen changes
//! because an animated window (a separate process, compiled here from
//! Swift with `xcrun swiftc`) covers most of the display.
//!
//! ScreenCaptureKit needs a logged-in GUI session and Screen Recording for
//! the process that captures, so this is an example run as an app with
//! `open` in the console user's session, not a cargo test in a compute job
//! (those run as the node user, with no GUI). `scripts/test/deskd-sck-live.sh`
//! bundles the binary as `K2 Screen Live Test.app` (ad hoc signed,
//! `dev.k2.test.screen-live`) and opens it. Without Screen Recording it
//! writes `{"state": "needs_permission"}` and, with `--request`, shows the
//! macOS prompt at the Mac. A person has to allow it once in System
//! Settings → Privacy & Security → Screen & System Audio Recording (again
//! after each rebuild: an ad hoc signature changes with the binary).
//!
//!   sck_live --out <file.json> [--seconds 8] [--request]
//!
//! Prints `LIVE …` lines and writes the numbers to `--out`. Exit 0 on a
//! pass, 1 on a failed check, 3 when Screen Recording is missing.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("sck_live runs on macOS only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::PipeReader;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use k2_deskd::capture::{self, Capture, Out};
    use k2_deskd::mac::capture::MacOpener;
    use k2_deskd_proto::link::{self, Codec, LayerSpec, Message, UnitHeader};
    use k2_deskd_proto::msg::Event;
    use kessel_video::Permission;
    use serde_json::{json, Value};

    const ANIMATOR: &str = r#"
import AppKit
let seconds = Double(CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "10") ?? 10
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screen = NSScreen.main!.frame
let rect = NSRect(x: screen.minX + 40, y: screen.minY + 40, width: screen.width - 80, height: screen.height - 120)
let w = NSWindow(contentRect: rect, styleMask: [.borderless], backing: .buffered, defer: false)
w.level = .floating
w.isOpaque = true
let content = NSView(frame: NSRect(origin: .zero, size: rect.size))
content.wantsLayer = true
w.contentView = content
let bar = NSView(frame: NSRect(x: 0, y: 0, width: rect.width / 10, height: rect.height))
bar.wantsLayer = true
bar.layer?.backgroundColor = NSColor.black.cgColor
content.addSubview(bar)
w.orderFrontRegardless()
var n = 0
let t = Timer(timeInterval: 1.0 / 60.0, repeats: true) { _ in
    n += 1
    content.layer?.backgroundColor = NSColor(hue: CGFloat(n % 360) / 360.0, saturation: 0.6, brightness: 0.9, alpha: 1).cgColor
    var f = bar.frame
    f.origin.x = CGFloat((n * 14) % Int(max(1, rect.width - f.width)))
    bar.frame = f
}
RunLoop.main.add(t, forMode: .common)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()
"#;

    struct Opts {
        out: PathBuf,
        seconds: u64,
        request: bool,
    }

    fn opts() -> Opts {
        let mut o = Opts { out: PathBuf::from("/tmp/k2-sck-live.json"), seconds: 8, request: false };
        let mut a = std::env::args().skip(1);
        while let Some(x) = a.next() {
            match x.as_str() {
                "--out" => o.out = a.next().expect("--out <file>").into(),
                "--seconds" => o.seconds = a.next().and_then(|s| s.parse().ok()).expect("--seconds <n>"),
                "--request" => o.request = true,
                other => panic!("unknown argument {other}"),
            }
        }
        o
    }

    fn write(out: &Path, v: &Value) {
        std::fs::write(out, serde_json::to_vec_pretty(v).unwrap()).expect("write the results");
    }

    struct PipeOut {
        w: Mutex<std::io::PipeWriter>,
        events: Mutex<Vec<Event>>,
    }

    impl Out for PipeOut {
        fn unit(&self, h: &UnitHeader, data: &[u8]) -> bool {
            link::write_unit(&mut *self.w.lock().unwrap(), h, data).is_ok()
        }
        fn event(&self, e: &Event) {
            self.events.lock().unwrap().push(e.clone());
        }
    }

    struct Got {
        h: UnitHeader,
        bytes: usize,
        read_us: u64,
    }

    fn reader(mut r: PipeReader) -> Arc<Mutex<Vec<Got>>> {
        let got: Arc<Mutex<Vec<Got>>> = Arc::default();
        let g = got.clone();
        std::thread::spawn(move || {
            while let Ok(Some(m)) = link::read_message(&mut r) {
                if let Message::Unit(u) = m {
                    g.lock().unwrap().push(Got { h: u.header, bytes: u.data.len(), read_us: capture::now_us() });
                }
            }
        });
        got
    }

    fn take(got: &Arc<Mutex<Vec<Got>>>) -> Vec<Got> {
        std::mem::take(&mut *got.lock().unwrap())
    }

    fn pct(mut v: Vec<f64>, p: f64) -> f64 {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f64 * p).round() as usize]
    }

    fn cpu_s() -> f64 {
        // SAFETY: getrusage fills the struct it's given.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
        t(ru.ru_utime) + t(ru.ru_stime)
    }

    fn animator(seconds: u64) -> Child {
        let dir = std::env::temp_dir().join(format!("k2-sck-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("animate.swift");
        std::fs::write(&src, ANIMATOR).unwrap();
        let bin = dir.join("k2-animate");
        let st = Command::new("xcrun").args(["swiftc", "-O", "-o"]).arg(&bin).arg(&src).status().expect("xcrun must run: the animator needs the Swift compiler");
        assert!(st.success(), "building the animator failed");
        Command::new(&bin).arg(seconds.to_string()).spawn().expect("start the animator")
    }

    fn layer_stats(us: &[&Got], wall: f64) -> Value {
        if us.len() < 2 {
            return json!({"units": us.len()});
        }
        let span = (us[us.len() - 1].h.captured_ns - us[0].h.captured_ns) as f64 / 1e9;
        let gaps: Vec<f64> = us.windows(2).map(|p| (p[1].h.captured_ns - p[0].h.captured_ns) as f64 / 1e6).collect();
        let enc: Vec<f64> = us.iter().map(|u| (u.h.emitted_ns - u.h.captured_ns) as f64 / 1e6).collect();
        let read: Vec<f64> = us.iter().map(|u| (u.read_us * 1000).saturating_sub(u.h.captured_ns) as f64 / 1e6).collect();
        json!({
            "units": us.len(),
            "size": [us[0].h.width, us[0].h.height],
            "fps": (us.len() - 1) as f64 / span,
            "interval_p50_ms": pct(gaps.clone(), 0.5),
            "interval_p95_ms": pct(gaps, 0.95),
            "encode_p50_ms": pct(enc.clone(), 0.5),
            "encode_p95_ms": pct(enc, 0.95),
            "capture_to_read_p50_ms": pct(read.clone(), 0.5),
            "capture_to_read_p95_ms": pct(read, 0.95),
            "keys": us.iter().filter(|u| u.h.key).count(),
            "first_is_start_key": us[0].h.key && us[0].h.source_start,
            "mbps": us.iter().map(|u| u.bytes).sum::<usize>() as f64 * 8.0 / 1e6 / wall,
        })
    }

    pub fn main() {
        let o = opts();
        let backend = kessel_video::system(kessel_video::SystemConfig::default()).expect("kessel-video's Apple backend");
        let perm = backend.screen_permission();
        if perm != Permission::Granted {
            if o.request {
                let (tx, rx) = std::sync::mpsc::channel();
                backend.request_screen_permission(Box::new(move |p| {
                    let _ = tx.send(p);
                }));
                let _ = rx.recv_timeout(Duration::from_secs(5));
            }
            let v = json!({"state": "needs_permission", "permission": format!("{perm:?}"), "asked": o.request});
            println!("LIVE needs Screen Recording: {v}");
            write(&o.out, &v);
            std::process::exit(3);
        }
        let displays = k2_deskd::mac::displays().expect("the active displays");
        let main = displays.iter().find(|d| d.main).unwrap_or(&displays[0]).clone();
        let encoder = capture::probe_encoder(&*backend);
        let mut anim = animator(o.seconds + 4);
        std::thread::sleep(Duration::from_millis(1500));

        let (r, w) = std::io::pipe().expect("a pipe");
        let got = reader(r);
        let out = Arc::new(PipeOut { w: Mutex::new(w), events: Mutex::default() });
        let cap = Capture::spawn(MacOpener::new(backend.clone()), backend, out.clone(), encoder).expect("the capture thread");
        // As the hub asks: the full layer at the display's own width, 60 fps,
        // 12 Mb/s; the remote one at most 1920 wide, 30 fps, 4 Mb/s.
        let full = LayerSpec { id: 0, codec: Codec::H264, max_width: main.width_px, fps: 60, bitrate: 12_000_000, quality: None, on: true };
        let remote = LayerSpec { id: 1, codec: Codec::H264, max_width: main.width_px.min(1920), fps: 30, bitrate: 4_000_000, quality: None, on: true };
        let t_open = Instant::now();
        cap.layers(vec![full, remote]);
        let t0 = Instant::now();
        while got.lock().unwrap().is_empty() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let first_ms = t_open.elapsed().as_secs_f64() * 1e3;
        let (cpu0, w0) = (cpu_s(), Instant::now());
        std::thread::sleep(Duration::from_secs(o.seconds));
        let wall = w0.elapsed().as_secs_f64();
        let cpu = cpu_s() - cpu0;
        let units = take(&got);
        let of = |l: u8| units.iter().filter(|u| u.h.layer == l).collect::<Vec<_>>();
        let (s0, s1) = (layer_stats(&of(0), wall), layer_stats(&of(1), wall));

        // Still: the animator ends, the screen stops.
        let _ = anim.kill();
        let _ = anim.wait();
        std::thread::sleep(Duration::from_millis(800));
        take(&got);
        std::thread::sleep(Duration::from_secs(1));
        let still = take(&got).len();
        // A joiner's keyframe on the still screen.
        let t_k = Instant::now();
        cap.keyframe(1, Some(capture::clock_ns()));
        while got.lock().unwrap().is_empty() && t_k.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let key_ms = t_k.elapsed().as_secs_f64() * 1e3;
        let k = take(&got);
        cap.layers(vec![]);
        let stats = cap.stats();
        let events: Vec<String> = out.events.lock().unwrap().iter().map(|e| format!("{e:?}")).collect();

        let v = json!({
            "state": "ran",
            "display": {"id": main.id, "px": [main.width_px, main.height_px], "pt": [main.width_pt, main.height_pt]},
            "encoder": format!("{encoder:?}"),
            "first_unit_ms": first_ms,
            "cpu_pct_of_one_core": cpu / wall * 100.0,
            "full": s0,
            "remote": s1,
            "still_units": still,
            "keyframe_on_still": {"ms": key_ms, "units": k.len(), "layer": k.first().map(|u| u.h.layer), "key": k.first().map(|u| u.h.key)},
            "engine": format!("{stats:?}"),
            "events": events,
        });
        println!("LIVE {}", serde_json::to_string(&v).unwrap());
        write(&o.out, &v);
        let fps0 = s0["fps"].as_f64().unwrap_or(0.0);
        let p50 = s0["interval_p50_ms"].as_f64().unwrap_or(f64::MAX);
        let ok = s0["first_is_start_key"] == json!(true) && s1["first_is_start_key"] == json!(true) && still == 0 && k.len() == 1 && p50 <= 18.5 && fps0 >= 55.0 && events.is_empty();
        println!("LIVE {}", if ok { "PASS" } else { "FAIL" });
        std::process::exit(if ok { 0 } else { 1 });
    }
}
