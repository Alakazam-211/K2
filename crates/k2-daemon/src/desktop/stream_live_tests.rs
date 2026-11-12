//! Live: the remote desktop stream on Linux X11 (Linux virtual desktop
//! L2, RD D8b) through this daemon's own code. A real Xvfb, the real
//! k2-deskd binary spawned by the daemon's [`ChildTransport`] (as on a
//! server: a child over pipes, frames on fd 3, a clean environment), the
//! real [`ManagedDeskd`], and whole viewers on loopback WebSockets through
//! [`serve_with`]. Nothing is faked.
//!
//! Ignored by default: they need Xvfb, a built k2-deskd and its
//! `x11_animate` example. `scripts/test/deskd-checks.sh live` builds both
//! and runs these one at a time:
//!
//!   DESKD_LIVE_BIN=target/release/k2-deskd \
//!   DESKD_LIVE_ANIMATE=target/release/examples/x11_animate \
//!   cargo nextest run -p k2-daemon --lib --run-ignored only \
//!       -E 'test(/desktop::stream::tests::live::/)' -j 1
//!
//! Each prints `LIVE …` lines with its numbers.

use std::io::BufRead;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use k2_core::sim::stream::FrameHeader;

use super::*;
use crate::desktop::deskd_link::{ChildTransport, DeskdManager, DeskdState, SessionEnv};
use crate::desktop::managed::ManagedDeskd;

/// A child killed (and reaped) on drop.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The CPUs this process may use.
fn allowed_cpus() -> Vec<usize> {
    // SAFETY: a zeroed cpu_set_t filled by the kernel for this process.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return vec![0];
        }
        (0..libc::CPU_SETSIZE as usize).filter(|&c| libc::CPU_ISSET(c, &set)).collect()
    }
}

/// Pins every thread of `pid` to `cpu`.
fn pin(pid: u32, cpu: usize) {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for t in tasks.flatten() {
        let Ok(tid) = t.file_name().to_string_lossy().parse::<i32>() else { continue };
        // SAFETY: a cpu_set_t with one CPU, for a thread of our own child.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &set);
        }
    }
}

/// CPU seconds `pid` has used (user + system, all threads).
fn cpu_seconds(pid: u32) -> f64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let after = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let f: Vec<u64> = after.split_whitespace().map(|v| v.parse().unwrap_or(0)).collect();
    // After the name: state is [0], utime [11], stime [12].
    // SAFETY: plain sysconf.
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    (f.get(11).copied().unwrap_or(0) + f.get(12).copied().unwrap_or(0)) as f64 / tck
}

fn xvfb(w: u32, h: u32) -> (Proc, String) {
    let (rx, tx) = std::io::pipe().expect("a pipe");
    let fd = std::os::fd::AsRawFd::as_raw_fd(&tx);
    let mut cmd = Command::new("Xvfb");
    cmd.args(["-displayfd", &fd.to_string(), "-screen", "0", &format!("{w}x{h}x24"), "-nolisten", "tcp"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: fcntl on the inherited write end, in the child.
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = Proc(cmd.spawn().expect("Xvfb (apt install xvfb)"));
    drop(tx);
    let mut line = String::new();
    std::io::BufReader::new(rx).read_line(&mut line).expect("Xvfb's display number");
    let n = line.trim().to_string();
    assert!(!n.is_empty(), "Xvfb didn't report a display");
    (child, format!(":{n}"))
}

fn animate(display: &str, fps: u32) -> Proc {
    let bin = std::env::var("DESKD_LIVE_ANIMATE").expect("DESKD_LIVE_ANIMATE: the built x11_animate example");
    let mut c = Command::new(bin).args(["--fps", &fps.to_string()]).env("DISPLAY", display).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().expect("x11_animate");
    let mut line = String::new();
    std::io::BufReader::new(c.stdout.take().unwrap()).read_line(&mut line).expect("x11_animate is ready");
    assert_eq!(line.trim(), "ready");
    Proc(c)
}

struct Live {
    _home: k2_core::test_env::TempHome,
    _deskd: deskd::TestDeskd,
    m: Arc<DeskdManager>,
    xvfb: Proc,
    display: String,
    olive: String,
}

/// Xvfb at `w` x `h`, and K2 Screen on it the way the daemon starts it on a
/// server, ready, with the switch on and an Owner signed in.
fn live(w: u32, h: u32) -> Live {
    let bin = std::env::var("DESKD_LIVE_BIN").expect("DESKD_LIVE_BIN: a built k2-deskd");
    let home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let (xvfb, display) = xvfb(w, h);
    let (d, bus) = (display.clone(), std::env::var("DBUS_SESSION_BUS_ADDRESS").ok());
    let env = move || {
        let mut e = SessionEnv::default();
        e.vars.insert("DISPLAY".into(), d.clone());
        if let Some(b) = &bus {
            e.vars.insert("DBUS_SESSION_BUS_ADDRESS".into(), b.clone());
        }
        Some(e)
    };
    let m = DeskdManager::new(Arc::new(ChildTransport { bin: bin.into(), env: Box::new(env) }));
    let st = m.enable(Duration::from_secs(20));
    assert!(matches!(st, DeskdState::Ready { .. }), "K2 Screen didn't come up: {st:?}");
    let guard = deskd::set_for_test(Arc::new(ManagedDeskd::new(m.clone())));
    store::enable(CONSOLE, true, "owner").expect("switch on");
    let olive = login("olive", Role::Owner);
    eprintln!("LIVE display {display}, bus {}", std::env::var("DBUS_SESSION_BUS_ADDRESS").map_or("none", |_| "a session bus, no notification server"));
    Live { _home: home, _deskd: guard, m, xvfb, display, olive }
}

impl Live {
    fn deskd_pid(&self) -> u32 {
        self.m.hello().expect("a hello").pid
    }
}

/// A frame of either wire: (codec name, key, width, height, payload).
#[derive(Debug, Clone)]
struct Pic {
    codec: &'static str,
    key: bool,
    width: u16,
    height: u16,
    data: Vec<u8>,
}

enum Msg {
    Text(Value),
    Pic(Pic),
    Closed(Option<(u16, String)>),
}

/// The next message within `within`; frames are acked at once.
async fn next_msg(c: &mut Client, within: Duration) -> Option<Msg> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let m = match tokio::time::timeout_at(deadline, c.next()).await {
            Err(_) => return None,
            Ok(None) | Ok(Some(Err(_))) => return Some(Msg::Closed(None)),
            Ok(Some(Ok(m))) => m,
        };
        return Some(match m {
            Message::Text(t) => Msg::Text(serde_json::from_str(&t).expect("JSON")),
            Message::Binary(b) => {
                let (seq, pic) = if let Some((h, data)) = VideoHeader::decode(&b) {
                    let codec = match h.codec {
                        1 => "h264",
                        2 => "vp8",
                        _ => "?",
                    };
                    (h.seq, Pic { codec, key: h.key, width: h.width_px, height: h.height_px, data: data.to_vec() })
                } else if let Some((h, data)) = FrameHeader::decode(&b) {
                    (h.seq, Pic { codec: "jpeg", key: true, width: h.width_px, height: h.height_px, data: data.to_vec() })
                } else {
                    panic!("a binary message that is neither K2SV nor K2SF");
                };
                let _ = c.send(Message::Text(format!(r#"{{"t":"ack","seq":{seq}}}"#))).await;
                Msg::Pic(pic)
            }
            Message::Close(f) => Msg::Closed(f.map(|f| (u16::from(f.code), f.reason.to_string()))),
            _ => continue,
        });
    }
}

/// The next picture within `within`; a close fails the test with its
/// reason, and texts are collected into `texts`.
async fn next_pic(c: &mut Client, within: Duration, texts: &mut Vec<Value>) -> Option<Pic> {
    let deadline = Instant::now() + within;
    loop {
        match next_msg(c, deadline.saturating_duration_since(Instant::now())).await {
            Some(Msg::Pic(p)) => return Some(p),
            Some(Msg::Text(v)) => {
                assert_ne!(v["t"], "layer_failed", "{v}");
                texts.push(v);
            }
            Some(Msg::Closed(f)) => panic!("the stream closed: {f:?}; texts {texts:?}"),
            None => return None,
        }
    }
}

async fn live_viewer(l: &Live, codecs: &str) -> Client {
    viewer("olive", &l.olive, &[("codecs", codecs), ("win", "3")], fast()).await
}

/// `layers` asks K2 Screen for VP8, and VP8 units come back (never
/// `layer_failed`): the viewer's hello says vp8, the first unit is a key,
/// and while the screen changes they keep coming.
#[test]
#[ignore = "live: Xvfb and a built k2-deskd; scripts/test/deskd-checks.sh live"]
fn live_vp8_units_through_the_daemon() {
    let l = live(1280, 720);
    let hello = l.m.hello().expect("hello");
    let codecs: Vec<&str> = hello.codecs.iter().map(|c| c.as_str()).collect();
    assert_eq!(codecs, vec!["vp8", "jpeg"], "hello lists vp8 and jpeg");
    assert_eq!((hello.os.as_str(), hello.backend.as_str()), ("linux", "x11"));
    rt().block_on(async {
        let mut o = live_viewer(&l, "vp8").await;
        let hv = t_is(&mut o, "hello").await;
        assert_eq!(hv["codec"], "vp8", "{hv}");
        let mut texts = Vec::new();
        let t0 = Instant::now();
        let first = next_pic(&mut o, Duration::from_secs(5), &mut texts).await.expect("a first picture within 5 s");
        eprintln!("LIVE vp8 first unit after {:?}: {}x{} key {} {} bytes", t0.elapsed(), first.width, first.height, first.key, first.data.len());
        assert_eq!((first.codec, first.key, first.width, first.height), ("vp8", true, 1280, 720));
        // A VP8 keyframe starts with the frame tag's key bit clear and the
        // start code 9d 01 2a.
        assert_eq!(first.data[0] & 1, 0, "a VP8 key frame");
        assert_eq!(&first.data[3..6], &[0x9d, 0x01, 0x2a]);
        let _anim = animate(&l.display, 30);
        let mut n = 0;
        let mut deltas = 0;
        let t1 = Instant::now();
        while t1.elapsed() < Duration::from_secs(4) {
            if let Some(p) = next_pic(&mut o, Duration::from_secs(2), &mut texts).await {
                assert_eq!(p.codec, "vp8");
                n += 1;
                deltas += usize::from(!p.key);
            }
        }
        eprintln!("LIVE vp8 720p: {n} units in 4 s ({deltas} deltas)");
        assert!(n >= 40, "{n} VP8 units in 4 s of a changing 720p screen (30 fps asked)");
        assert!(deltas >= n - 2, "deltas, not a key each time");
    });
}

/// A viewer that offers only JPEG (a webview with no VP8 decoder) gets
/// JPEG pictures that decode, made by K2 Screen.
#[test]
#[ignore = "live: Xvfb and a built k2-deskd; scripts/test/deskd-checks.sh live"]
fn live_a_jpeg_only_viewer_gets_jpeg() {
    let l = live(1280, 720);
    rt().block_on(async {
        let mut o = live_viewer(&l, "jpeg").await;
        let hv = t_is(&mut o, "hello").await;
        assert_eq!(hv["codec"], "jpeg", "{hv}");
        let mut texts = Vec::new();
        let p = next_pic(&mut o, Duration::from_secs(5), &mut texts).await.expect("a JPEG within 5 s");
        assert_eq!(p.codec, "jpeg");
        assert!(p.data.starts_with(&[0xff, 0xd8]) && p.data.ends_with(&[0xff, 0xd9]), "a whole JPEG");
        let img = image::load_from_memory_with_format(&p.data, image::ImageFormat::Jpeg).expect("decodes");
        assert_eq!((img.width(), img.height()), (p.width as u32, p.height as u32));
        let _anim = animate(&l.display, 30);
        let t1 = Instant::now();
        let mut n = 0;
        while t1.elapsed() < Duration::from_secs(3) {
            if let Some(p) = next_pic(&mut o, Duration::from_secs(2), &mut texts).await {
                assert_eq!(p.codec, "jpeg");
                n += 1;
            }
        }
        eprintln!("LIVE jpeg {}x{}: {n} pictures in 3 s, last {} bytes", p.width, p.height, p.data.len());
        assert!(n >= 6, "{n} JPEG pictures in 3 s");
    });
}

/// 1080p at 15 fps on one core: K2 Screen pinned to a single CPU (every
/// thread: capture, VP8, the link), the whole screen changing at 30 fps
/// (the animator and Xvfb on other CPUs), one VP8 viewer for 10 s.
#[test]
#[ignore = "live: Xvfb and a built k2-deskd; scripts/test/deskd-checks.sh live"]
fn live_1080p15_holds_on_one_core() {
    let l = live(1920, 1080);
    let cpus = allowed_cpus();
    assert!(cpus.len() >= 3, "needs 3 CPUs to keep the animator and Xvfb off K2 Screen's: {cpus:?}");
    let (deskd_cpu, anim_cpu, x_cpu) = (cpus[cpus.len() - 1], cpus[cpus.len() - 2], cpus[cpus.len() - 3]);
    let pid = l.deskd_pid();
    pin(pid, deskd_cpu);
    pin(l.xvfb.0.id(), x_cpu);
    let anim = animate(&l.display, 30);
    pin(anim.0.id(), anim_cpu);
    rt().block_on(async {
        let mut o = live_viewer(&l, "vp8").await;
        let hv = t_is(&mut o, "hello").await;
        assert_eq!(hv["codec"], "vp8", "{hv}");
        let mut texts = Vec::new();
        // Warm up: the encoder's first frames, and pin the threads it made.
        let t0 = Instant::now();
        let mut first = None;
        while t0.elapsed() < Duration::from_secs(3) {
            if let Some(p) = next_pic(&mut o, Duration::from_secs(2), &mut texts).await {
                first.get_or_insert(p);
            }
            pin(pid, deskd_cpu);
        }
        let first = first.expect("pictures during warm-up");
        assert_eq!((first.width, first.height), (1920, 1080));
        pin(pid, deskd_cpu);
        let (cpu0, t1) = (cpu_seconds(pid), Instant::now());
        let (mut n, mut bytes, mut keys) = (0usize, 0usize, 0usize);
        while t1.elapsed() < Duration::from_secs(10) {
            if let Some(p) = next_pic(&mut o, Duration::from_secs(2), &mut texts).await {
                assert_eq!((p.codec, p.width, p.height), ("vp8", 1920, 1080));
                n += 1;
                bytes += p.data.len();
                keys += usize::from(p.key);
            }
        }
        let secs = t1.elapsed().as_secs_f64();
        let cpu = (cpu_seconds(pid) - cpu0) / secs;
        let fps = n as f64 / secs;
        let mbps = bytes as f64 * 8.0 / secs / 1e6;
        eprintln!("LIVE 1080p15 on one core (cpu {deskd_cpu}): {fps:.2} fps, K2 Screen {:.0}% of a core, {mbps:.2} Mb/s, {keys} keys in {n} units", cpu * 100.0);
        assert!(fps >= 14.0, "{fps:.2} fps at 1080p on one core (15 asked)");
        assert!(cpu <= 1.0, "pinned to one CPU: {cpu:.2}");
    });
    drop(anim);
}

/// LD-7: a virtual desktop has no notification server (and K2 Screen may
/// have no session bus at all). The banner calls come back at once and
/// the link stays up: K2 Screen ready, pictures still coming.
#[test]
#[ignore = "live: Xvfb and a built k2-deskd; scripts/test/deskd-checks.sh live"]
fn live_the_banner_with_no_notification_server_doesnt_fail_the_link() {
    let l = live(1280, 720);
    rt().block_on(async {
        // A viewer joining sends the banner (who is watching) on its own.
        let mut o = live_viewer(&l, "vp8").await;
        let mut texts = Vec::new();
        next_pic(&mut o, Duration::from_secs(5), &mut texts).await.expect("a first picture");
        let m = l.m.clone();
        let took = tokio::task::spawn_blocking(move || {
            let mut took = Vec::new();
            for text in [Some("olive is controlling this screen through K2".to_string()), Some("olive is viewing this screen through K2".to_string()), None] {
                let t0 = Instant::now();
                m.banner(text).expect("the banner call answers ok");
                took.push(t0.elapsed());
            }
            took
        })
        .await
        .unwrap();
        eprintln!("LIVE banner calls took {took:?}");
        assert!(took.iter().all(|t| *t < Duration::from_secs(2)), "{took:?}");
        assert!(matches!(l.m.state(), DeskdState::Ready { .. }), "{:?}", l.m.state());
        assert_eq!(l.m.displays().expect("displays still answer").len(), 1);
        let _anim = animate(&l.display, 30);
        next_pic(&mut o, Duration::from_secs(3), &mut texts).await.expect("pictures keep coming after the banner calls");
    });
}

/// The sleep and wake of a virtual desktop (PRD §3.4) as K2 Screen sees
/// it: the link closed, the display frozen (SIGSTOP stands in for the
/// cgroup freezer), thawed, and K2 Screen started again on it. Nothing
/// draws after the thaw, so no damage comes; the first unit must still be
/// a keyframe, within the 2 s target.
#[test]
#[ignore = "live: Xvfb and a built k2-deskd; scripts/test/deskd-checks.sh live"]
fn live_a_just_thawed_display_sends_a_keyframe() {
    let l = live(1280, 720);
    rt().block_on(async {
        let mut o = live_viewer(&l, "vp8").await;
        let mut texts = Vec::new();
        {
            let _anim = animate(&l.display, 30);
            next_pic(&mut o, Duration::from_secs(5), &mut texts).await.expect("a picture before the sleep");
        }
        drop(o);
        // Sleep: K2 Screen's link closes, then the display freezes.
        let d = deskd::current();
        d.disable().expect("K2 Screen off");
        let xpid = l.xvfb.0.id() as i32;
        // SAFETY: signalling our own Xvfb child.
        unsafe { libc::kill(xpid, libc::SIGSTOP) };
        tokio::time::sleep(Duration::from_secs(2)).await;
        // Wake: thaw, then K2 Screen again.
        unsafe { libc::kill(xpid, libc::SIGCONT) };
        let t0 = Instant::now();
        d.enable().expect("K2 Screen on");
        assert!(matches!(l.m.wait_settled(Duration::from_secs(20)), DeskdState::Ready { .. }), "{:?}", l.m.state());
        let mut o = live_viewer(&l, "vp8").await;
        let p = next_pic(&mut o, Duration::from_secs(5), &mut texts).await.expect("a picture after the wake");
        eprintln!("LIVE wake: first unit {:?} after K2 Screen was started again, key {}", t0.elapsed(), p.key);
        assert!(p.key, "the first unit after a wake is a keyframe");
        assert!(t0.elapsed() < Duration::from_secs(2), "a picture within 2 s of the wake: {:?}", t0.elapsed());
    });
}
