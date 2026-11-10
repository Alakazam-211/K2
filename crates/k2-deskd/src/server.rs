//! deskd's side of the link, platform-neutral: answers requests, sends
//! events, and runs the 5 s TCC re-check and the local-hold watcher. The
//! OS end is a [`Host`] (TCC, displays, idle, banner) plus the
//! [`Injector`]'s [`crate::inject::Poster`].
//!
//! One link at a time. When it closes (the daemon quit, restarted or
//! crashed) deskd releases everything held and hides the banner.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use k2_deskd_proto::msg::{codes, Display, Event, FramesVia, Hello, Op, ReleaseReason, Request, Tcc};
use k2_deskd_proto::wire::{self, Message};
use k2_deskd_proto::{PROTOCOL_VERSION, ROLE, VERSION};

use crate::hold::{Edge, HoldEdges, LocalHold};
use crate::inject::{Injector, Outcome};

/// How often deskd re-reads its TCC grants (§3.2 step 3).
pub const TCC_RECHECK: Duration = Duration::from_secs(5);
/// How often the hold watcher looks for a hold starting or ending.
pub const HOLD_POLL: Duration = Duration::from_millis(50);

/// The OS end of deskd, apart from posting input.
pub trait Host: Send + Sync + 'static {
    fn os(&self) -> &'static str;
    /// `macos`, `x11`, `wayland` or `none`.
    fn backend(&self) -> String;
    fn frames_via(&self) -> FramesVia;
    fn tcc(&self) -> Tcc;
    /// Fire the OS prompts (and open the Settings panes) for what's missing.
    fn request_tcc(&self) -> Tcc;
    fn displays(&self) -> Result<Vec<Display>, String>;
    /// The OS's idle clock (HID idle on macOS, XScreenSaver on X11). Note:
    /// on X11 XTEST input resets it (vs-live R11); `local_idle_ms` doesn't.
    fn hid_idle_ms(&self) -> Option<u64>;
    /// `None` hides it.
    fn banner(&self, text: Option<String>);
}

/// Where events and replies go: the current link, if any.
#[derive(Default)]
pub struct Outbox {
    w: Mutex<Option<Box<dyn Write + Send>>>,
}

impl Outbox {
    pub fn attach(&self, w: Box<dyn Write + Send>) {
        *self.w.lock().unwrap() = Some(w);
    }

    pub fn detach(&self) {
        *self.w.lock().unwrap() = None;
    }

    pub fn attached(&self) -> bool {
        self.w.lock().unwrap().is_some()
    }

    /// Sends one JSON message; `false` when there is no link or the write
    /// failed (then the link is dropped here; the reader sees it close).
    pub fn send(&self, v: &impl serde::Serialize) -> bool {
        let mut g = self.w.lock().unwrap();
        let Some(w) = g.as_mut() else { return false };
        if wire::write_json(w, v).is_err() {
            *g = None;
            return false;
        }
        true
    }

    pub fn event(&self, e: &Event) -> bool {
        self.send(e)
    }
}

pub struct Deskd<H: Host> {
    pub host: Arc<H>,
    pub injector: Mutex<Injector>,
    pub hold: Arc<LocalHold>,
    pub out: Outbox,
    /// The frames pipe's write end for the current link (D5/D8b write
    /// encoded units here; D3 only plumbs it).
    frames: Mutex<Option<Box<dyn Write + Send>>>,
}

fn reply_ok(id: u64, mut extra: J) -> J {
    if !extra.is_object() {
        extra = json!({});
    }
    extra["id"] = json!(id);
    extra["ok"] = json!(true);
    extra
}

fn reply_err(id: u64, code: &str, error: &str) -> J {
    json!({"id": id, "ok": false, "code": code, "error": error})
}

impl<H: Host> Deskd<H> {
    pub fn new(host: Arc<H>, injector: Injector, hold: Arc<LocalHold>) -> Self {
        Deskd { host, injector: Mutex::new(injector), hold, out: Outbox::default(), frames: Mutex::new(None) }
    }

    pub fn hello(&self) -> Event {
        Event::Hello(Hello {
            version: PROTOCOL_VERSION,
            build: VERSION.to_string(),
            role: ROLE.to_string(),
            os: self.host.os().to_string(),
            backend: self.host.backend(),
            tcc: self.host.tcc(),
            pid: std::process::id(),
            frames: self.host.frames_via(),
        })
    }

    /// Answers one request. `None` for a fire-and-forget (`nr`) request,
    /// whose failure goes out as an `input_error` event instead.
    pub fn handle(&self, req: Request) -> Option<J> {
        let id = req.id;
        let op_name = op_name(&req.op);
        let result: Result<J, (&'static str, String)> = match req.op {
            Op::Ping => Ok(json!({})),
            Op::TccStatus => Ok(json!({"tcc": self.host.tcc()})),
            Op::TccRequest => Ok(json!({"tcc": self.host.request_tcc()})),
            Op::Displays => self.host.displays().map(|d| json!({"displays": d})).map_err(|e| (codes::NO_DISPLAY, e)),
            Op::Idle => {
                let now = Instant::now();
                Ok(json!({
                    "hid_idle_ms": self.host.hid_idle_ms(),
                    "local_idle_ms": self.hold.local_idle_at(now).map(|d| d.as_millis() as u64),
                    "hold": self.hold.held_at(now),
                }))
            }
            Op::Banner { text } => {
                self.host.banner(text);
                Ok(json!({}))
            }
            Op::ReleaseAll { reason: _ } => Ok(json!({"released": self.injector.lock().unwrap().release_all()})),
            Op::Inject { ev } => match self.injector.lock().unwrap().inject(&ev, &self.hold, Instant::now()) {
                Ok(Outcome::Posted) | Ok(Outcome::Ignored) => Ok(json!({})),
                Ok(Outcome::DroppedForHold) => Ok(json!({"dropped": codes::LOCAL_HOLD})),
                Err(e) => Err((e.code, e.message)),
            },
        };
        match (id, req.nr, result) {
            (Some(id), false, Ok(v)) => Some(reply_ok(id, v)),
            (Some(id), false, Err((code, msg))) => Some(reply_err(id, code, &msg)),
            (_, _, Ok(_)) => None,
            (_, _, Err((code, msg))) => {
                self.out.event(&Event::InputError { op: op_name.into(), code: code.into(), error: msg });
                None
            }
        }
    }

    /// Serves one link: `w` is attached as the outbox (hello must already
    /// be written), then requests are read off `r` until it closes.
    pub fn serve(&self, mut r: impl Read, w: Box<dyn Write + Send>) -> io::Result<()> {
        self.out.attach(w);
        let res = loop {
            match wire::read_message(&mut r) {
                Ok(None) => break Ok(()),
                Ok(Some(Message::Json(v))) => {
                    let id = v.get("id").and_then(J::as_u64);
                    match serde_json::from_value::<Request>(v) {
                        Ok(req) => {
                            if let Some(rep) = self.handle(req) {
                                self.out.send(&rep);
                            }
                        }
                        Err(e) => match id {
                            Some(id) => {
                                self.out.send(&reply_err(id, codes::BAD_REQUEST, &e.to_string()));
                            }
                            None => {
                                self.out.event(&Event::InputError { op: "?".into(), code: codes::BAD_REQUEST.into(), error: e.to_string() });
                            }
                        },
                    }
                }
                Ok(Some(Message::Frame(_))) => break Err(io::Error::new(io::ErrorKind::InvalidData, "the daemon sent a frame")),
                Err(e) => break Err(e),
            }
        };
        self.link_closed();
        res
    }

    pub fn attach_frames(&self, w: Box<dyn Write + Send>) {
        *self.frames.lock().unwrap() = Some(w);
    }

    /// Closes the frames pipe (the daemon's reader sees end of stream).
    pub fn detach_frames(&self) {
        *self.frames.lock().unwrap() = None;
    }

    /// Writes one encoded unit to the frames pipe; `false` with no link or
    /// when the daemon stopped reading (then the pipe is dropped).
    pub fn send_frame(&self, h: &wire::FrameHeader, data: &[u8]) -> bool {
        let mut g = self.frames.lock().unwrap();
        let Some(w) = g.as_mut() else { return false };
        if wire::write_frame(w, h, data).is_err() {
            *g = None;
            return false;
        }
        true
    }

    /// The daemon went away: let go of everything and hide the banner.
    pub fn link_closed(&self) {
        self.out.detach();
        self.injector.lock().unwrap().release_all();
        self.host.banner(None);
    }

    /// The person at the machine clicked Disconnect.
    pub fn banner_disconnect(&self) {
        self.injector.lock().unwrap().release_all();
        self.out.event(&Event::BannerDisconnect);
    }

    /// One step of the hold watcher.
    pub fn tick_hold(&self, edges: &mut HoldEdges, now: Instant) {
        match edges.poll(self.hold.held_at(now)) {
            Some(Edge::Started) => {
                self.injector.lock().unwrap().release_all();
                self.out.event(&Event::LocalInput { hold: true });
            }
            Some(Edge::Ended) => {
                self.out.event(&Event::LocalInput { hold: false });
            }
            None => {}
        }
    }

    /// One step of the TCC re-check: an event when the grants changed.
    pub fn tick_tcc(&self, last: &mut Tcc) {
        let now = self.host.tcc();
        if now != *last {
            *last = now;
            self.out.event(&Event::TccChanged { tcc: now });
        }
    }

    /// Starts the hold watcher and the TCC re-check threads.
    pub fn spawn_watchers(self: &Arc<Self>) {
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("k2-deskd-hold".into())
            .spawn(move || {
                let mut edges = HoldEdges::default();
                loop {
                    me.tick_hold(&mut edges, Instant::now());
                    std::thread::sleep(HOLD_POLL);
                }
            })
            .expect("spawn hold watcher");
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("k2-deskd-tcc".into())
            .spawn(move || {
                let mut last = me.host.tcc();
                loop {
                    std::thread::sleep(TCC_RECHECK);
                    me.tick_tcc(&mut last);
                }
            })
            .expect("spawn tcc re-check");
    }

    /// SIGTERM, a fatal error: release, then go.
    pub fn shutdown(&self, _reason: ReleaseReason) {
        self.injector.lock().unwrap().release_all();
        self.host.banner(None);
    }
}

fn op_name(op: &Op) -> &'static str {
    match op {
        Op::TccStatus => "tcc_status",
        Op::TccRequest => "tcc_request",
        Op::Inject { .. } => "inject",
        Op::ReleaseAll { .. } => "release_all",
        Op::Banner { .. } => "banner",
        Op::Idle => "idle",
        Op::Displays => "displays",
        Op::Ping => "ping",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::inject::tests::Recorder;
    use std::os::unix::net::UnixStream;

    #[derive(Default)]
    struct FakeHost {
        tcc: Mutex<Tcc>,
        banner: Mutex<Vec<Option<String>>>,
        requested: Mutex<u32>,
    }

    impl Host for FakeHost {
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
            *self.tcc.lock().unwrap()
        }
        fn request_tcc(&self) -> Tcc {
            *self.requested.lock().unwrap() += 1;
            self.tcc()
        }
        fn displays(&self) -> Result<Vec<Display>, String> {
            Ok(vec![Display { id: 1, name: "Built-in".into(), x: 0.0, y: 0.0, width_pt: 1512.0, height_pt: 982.0, width_px: 3024, height_px: 1964, scale: 2.0, main: true }])
        }
        fn hid_idle_ms(&self) -> Option<u64> {
            Some(1234)
        }
        fn banner(&self, text: Option<String>) {
            self.banner.lock().unwrap().push(text);
        }
    }

    fn deskd() -> (Arc<Deskd<FakeHost>>, Recorder) {
        let rec = Recorder::default();
        let host = Arc::new(FakeHost { tcc: Mutex::new(Tcc { required: true, screen: false, accessibility: false }), ..Default::default() });
        let d = Deskd::new(host, Injector::new(Box::new(rec.clone())), Arc::new(LocalHold::default()));
        (Arc::new(d), rec)
    }

    /// Runs `serve` on one end of a socket pair; returns the daemon's end.
    fn link(d: &Arc<Deskd<FakeHost>>) -> (UnixStream, std::thread::JoinHandle<io::Result<()>>) {
        let (daemon, deskd_end) = UnixStream::pair().unwrap();
        // A missing message fails the test instead of hanging it.
        daemon.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let d = Arc::clone(d);
        let w = deskd_end.try_clone().unwrap();
        let h = std::thread::spawn(move || d.serve(deskd_end, Box::new(w)));
        (daemon, h)
    }

    fn call(s: &mut UnixStream, v: J) -> J {
        wire::write_json(s, &v).unwrap();
        loop {
            match wire::read_message(s).unwrap().expect("a reply") {
                Message::Json(r) if r.get("id").is_some() => return r,
                Message::Json(_) => continue, // an event
                Message::Frame(_) => panic!("frame on the control link"),
            }
        }
    }

    #[test]
    fn hello_carries_version_role_and_tcc() {
        let (d, _) = deskd();
        match d.hello() {
            Event::Hello(h) => {
                assert_eq!(h.version, PROTOCOL_VERSION);
                assert_eq!(h.role, "desktop");
                assert_eq!(h.tcc, Tcc { required: true, screen: false, accessibility: false });
                assert_eq!(h.frames, FramesVia::Fd);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn requests_get_replies_with_their_id() {
        let (d, _) = deskd();
        let (mut s, h) = link(&d);
        assert_eq!(call(&mut s, json!({"id": 1, "op": "ping"})), json!({"id": 1, "ok": true}));
        let r = call(&mut s, json!({"id": 2, "op": "tcc_request"}));
        assert_eq!(r["tcc"]["screen"], false);
        assert_eq!(*d.host.requested.lock().unwrap(), 1);
        let r = call(&mut s, json!({"id": 3, "op": "displays"}));
        assert_eq!(r["displays"][0]["scale"], 2.0);
        let r = call(&mut s, json!({"id": 4, "op": "idle"}));
        assert_eq!(r["hid_idle_ms"], 1234);
        assert_eq!(r["local_idle_ms"], J::Null);
        let r = call(&mut s, json!({"id": 5, "op": "no_such_op"}));
        assert_eq!(r["ok"], false);
        assert_eq!(r["code"], "bad_request");
        drop(s);
        h.join().unwrap().unwrap();
    }

    #[test]
    fn an_nr_inject_failure_comes_back_as_an_input_error_event() {
        let (d, _) = deskd();
        let (mut s, _h) = link(&d);
        wire::write_json(&mut s, &json!({"nr": true, "op": "inject", "ev": {"t": "key", "code": "Bogus", "down": true}})).unwrap();
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => {
                assert_eq!(v["event"], "input_error");
                assert_eq!(v["op"], "inject");
                assert_eq!(v["code"], "unknown_key");
            }
            Message::Frame(_) => panic!("frame"),
        }
    }

    #[test]
    fn a_dropped_link_releases_held_keys_and_hides_the_banner() {
        let (d, rec) = deskd();
        let (mut s, h) = link(&d);
        call(&mut s, json!({"id": 1, "op": "banner", "text": "Rosson is controlling this screen"}));
        call(&mut s, json!({"id": 2, "op": "inject", "ev": {"t": "key", "code": "MetaLeft", "down": true}}));
        rec.take();
        drop(s);
        h.join().unwrap().unwrap();
        assert_eq!(rec.take(), vec!["key MetaLeft up mods=[]"]);
        assert_eq!(*d.host.banner.lock().unwrap(), vec![Some("Rosson is controlling this screen".to_string()), None]);
        assert!(!d.out.attached());
    }

    #[test]
    fn release_all_reports_how_many() {
        let (d, _rec) = deskd();
        let (mut s, _h) = link(&d);
        call(&mut s, json!({"id": 1, "op": "inject", "ev": {"t": "button", "button": "left", "down": true}}));
        call(&mut s, json!({"id": 2, "op": "inject", "ev": {"t": "key", "code": "ShiftLeft", "down": true}}));
        let r = call(&mut s, json!({"id": 3, "op": "release_all", "reason": "hand_back"}));
        assert_eq!(r["released"], 2);
    }

    #[test]
    fn a_local_hold_start_releases_and_tells_the_daemon_and_drops_input() {
        let (d, rec) = deskd();
        let (mut s, _h) = link(&d);
        call(&mut s, json!({"id": 1, "op": "inject", "ev": {"t": "key", "code": "ControlLeft", "down": true}}));
        rec.take();
        let mut edges = HoldEdges::default();
        d.hold.note_local();
        d.tick_hold(&mut edges, Instant::now());
        assert_eq!(rec.take(), vec!["key ControlLeft up mods=[]"]);
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, json!({"event": "local_input", "hold": true})),
            Message::Frame(_) => panic!(),
        }
        let r = call(&mut s, json!({"id": 2, "op": "inject", "ev": {"t": "text", "text": "hello"}}));
        assert_eq!(r["dropped"], "local_hold");
        assert!(rec.take().is_empty());
        // The hold ends: one event.
        d.tick_hold(&mut edges, Instant::now() + Duration::from_secs(4));
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, json!({"event": "local_input", "hold": false})),
            Message::Frame(_) => panic!(),
        }
    }

    #[test]
    fn tcc_change_sends_one_event() {
        let (d, _) = deskd();
        let (mut s, _h) = link(&d);
        // The link is attached once a request is answered.
        call(&mut s, json!({"id": 1, "op": "ping"}));
        let mut last = d.host.tcc();
        d.tick_tcc(&mut last);
        *d.host.tcc.lock().unwrap() = Tcc { required: true, screen: true, accessibility: true };
        d.tick_tcc(&mut last);
        d.tick_tcc(&mut last);
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => {
                assert_eq!(v["event"], "tcc_changed");
                assert_eq!(v["tcc"]["accessibility"], true);
            }
            Message::Frame(_) => panic!(),
        }
        // Exactly one: the next message is the ping's reply.
        assert_eq!(call(&mut s, json!({"id": 9, "op": "ping"}))["id"], 9);
    }

    #[test]
    fn banner_disconnect_releases_and_sends_the_event() {
        let (d, rec) = deskd();
        let (mut s, _h) = link(&d);
        call(&mut s, json!({"id": 1, "op": "inject", "ev": {"t": "button", "button": "right", "down": true}}));
        rec.take();
        d.banner_disconnect();
        assert_eq!(rec.take(), vec!["button Right up"]);
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, json!({"event": "banner_disconnect"})),
            Message::Frame(_) => panic!(),
        }
    }

    #[test]
    fn a_frame_from_the_daemon_closes_the_link() {
        let (d, _) = deskd();
        let (mut s, h) = link(&d);
        let hdr = wire::FrameHeader { codec: wire::Codec::H264, key: true, display: 1, width: 1, height: 1, seq: 0, captured_ns: 0, emitted_ns: 0 };
        wire::write_frame(&mut s, &hdr, b"x").unwrap();
        assert!(h.join().unwrap().is_err());
    }
}
