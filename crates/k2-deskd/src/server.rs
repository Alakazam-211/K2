//! deskd's side of the link, platform-neutral: answers requests, sends
//! events, and runs the 5 s TCC re-check and the local-hold watcher. The
//! OS end is a [`Host`] (TCC, displays, idle, banner) plus the
//! [`Injector`]'s [`crate::inject::Poster`], and, where capture is built
//! (macOS, D5), a [`Capture`] that answers protocol 4's `layers`,
//! `keyframe` and `rate` and writes units to the frames pipe.
//!
//! One link at a time. When it closes (the daemon quit, restarted or
//! crashed) deskd releases everything held, hides the banner and stops
//! capturing.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use k2_deskd_proto::link::{Encoder as LinkEncoder, UnitHeader};
use k2_deskd_proto::msg::{codes, Display, Event, FramesVia, Hello, Op, ReleaseReason, Request, Tcc};
use k2_deskd_proto::wire::{self, Message};
use k2_deskd_proto::{PROTOCOL_VERSION, ROLE, VERSION};

use crate::capture::{self, Capture, Opener};
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

/// The frames pipe's write end for the current link: the capture writes
/// encoded units here.
#[derive(Default)]
pub struct FramesPipe {
    w: Mutex<Option<Box<dyn Write + Send>>>,
}

impl FramesPipe {
    pub fn attach(&self, w: Box<dyn Write + Send>) {
        *self.w.lock().unwrap() = Some(w);
    }

    pub fn detach(&self) {
        *self.w.lock().unwrap() = None;
    }

    /// Writes one unit; `false` with no link or when the daemon stopped
    /// reading (then the pipe is dropped).
    pub fn send(&self, h: &UnitHeader, data: &[u8]) -> bool {
        let mut g = self.w.lock().unwrap();
        let Some(w) = g.as_mut() else { return false };
        if wire::write_unit(w, h, data).is_err() {
            *g = None;
            return false;
        }
        true
    }
}

/// The capture's view of the link: units to the frames pipe, events to
/// the control link.
struct LinkOut {
    out: Arc<Outbox>,
    frames: Arc<FramesPipe>,
}

impl capture::Out for LinkOut {
    fn unit(&self, h: &UnitHeader, data: &[u8]) -> bool {
        self.frames.send(h, data)
    }
    fn event(&self, e: &Event) {
        self.out.event(e);
    }
}

pub struct Deskd<H: Host> {
    pub host: Arc<H>,
    pub injector: Mutex<Injector>,
    pub hold: Arc<LocalHold>,
    pub out: Arc<Outbox>,
    frames: Arc<FramesPipe>,
    /// Capture and encode, where built ([`Deskd::start_capture`]).
    capture: OnceLock<Capture>,
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
        Deskd { host, injector: Mutex::new(injector), hold, out: Arc::default(), frames: Arc::default(), capture: OnceLock::new() }
    }

    /// Starts capture and encode on `opener` with `backend`'s encoders.
    /// `encoder` is what [`capture::probe_encoder`] found (`hello.encoder`).
    pub fn start_capture<O: Opener>(&self, opener: O, backend: Arc<dyn kessel_video::VideoBackend>, encoder: LinkEncoder) -> io::Result<()> {
        let out = Arc::new(LinkOut { out: Arc::clone(&self.out), frames: Arc::clone(&self.frames) });
        let c = Capture::spawn(opener, backend, out, encoder)?;
        self.capture.set(c).map_err(|_| io::Error::other("capture already started"))
    }

    pub fn capture(&self) -> Option<&Capture> {
        self.capture.get()
    }

    pub fn hello(&self) -> Event {
        let cap = self.capture.get();
        Event::Hello(Hello {
            version: PROTOCOL_VERSION,
            build: VERSION.to_string(),
            role: ROLE.to_string(),
            os: self.host.os().to_string(),
            backend: self.host.backend(),
            tcc: self.host.tcc(),
            pid: std::process::id(),
            frames: self.host.frames_via(),
            accepts: vec![PROTOCOL_VERSION],
            codecs: cap.map(Capture::codecs).unwrap_or_default(),
            encoder: cap.map_or(LinkEncoder::Unknown, Capture::encoder),
            clock_ns: capture::clock_ns(),
        })
    }

    /// Protocol 4's `layers` where nothing captures (Linux until D8b):
    /// every layer that's on fails at once, so the daemon's hub moves its
    /// viewers instead of waiting.
    fn no_capture(&self, layers: &[k2_deskd_proto::link::LayerSpec]) {
        for l in layers.iter().filter(|l| l.on) {
            self.out.event(&Event::LayerFailed { layer: l.id, code: codes::UNSUPPORTED.into(), error: "K2 Screen doesn't capture on this computer yet".into() });
        }
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
            Op::Layers { layers } => {
                match self.capture.get() {
                    Some(c) => c.layers(layers),
                    None => self.no_capture(&layers),
                }
                Ok(json!({}))
            }
            Op::Keyframe { layer, since_ns } => {
                if let Some(c) = self.capture.get() {
                    c.keyframe(layer, since_ns);
                }
                Ok(json!({}))
            }
            Op::Rate { layer, fps, bitrate } => {
                if let Some(c) = self.capture.get() {
                    c.rate(layer, fps, bitrate);
                }
                Ok(json!({}))
            }
            Op::Display { display } => match self.capture.get() {
                Some(c) => c.display(display).map(|d| json!({"display": d})),
                None => Err((codes::UNSUPPORTED, "K2 Screen doesn't capture on this computer yet".into())),
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
                Ok(Some(Message::Unit(_))) => break Err(io::Error::new(io::ErrorKind::InvalidData, "the daemon sent a unit")),
                Err(e) => break Err(e),
            }
        };
        self.link_closed();
        res
    }

    pub fn attach_frames(&self, w: Box<dyn Write + Send>) {
        self.frames.attach(w);
    }

    /// Closes the frames pipe (the daemon's reader sees end of stream).
    pub fn detach_frames(&self) {
        self.frames.detach();
    }

    /// Writes one encoded unit to the frames pipe; `false` with no link or
    /// when the daemon stopped reading (then the pipe is dropped).
    pub fn send_unit(&self, h: &UnitHeader, data: &[u8]) -> bool {
        self.frames.send(h, data)
    }

    /// The daemon went away: let go of everything, hide the banner and stop
    /// capturing (the next daemon sends its own layer list).
    pub fn link_closed(&self) {
        self.out.detach();
        if let Some(c) = self.capture.get() {
            c.reset();
        }
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
        Op::Layers { .. } => "layers",
        Op::Keyframe { .. } => "keyframe",
        Op::Rate { .. } => "rate",
        Op::Display { .. } => "display",
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
                Message::Unit(_) => panic!("frame on the control link"),
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
            Message::Unit(_) => panic!("frame"),
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
            Message::Unit(_) => panic!(),
        }
        let r = call(&mut s, json!({"id": 2, "op": "inject", "ev": {"t": "text", "text": "hello"}}));
        assert_eq!(r["dropped"], "local_hold");
        assert!(rec.take().is_empty());
        // The hold ends: one event.
        d.tick_hold(&mut edges, Instant::now() + Duration::from_secs(4));
        match wire::read_message(&mut s).unwrap().unwrap() {
            Message::Json(v) => assert_eq!(v, json!({"event": "local_input", "hold": false})),
            Message::Unit(_) => panic!(),
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
            Message::Unit(_) => panic!(),
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
            Message::Unit(_) => panic!(),
        }
    }

    #[test]
    fn a_unit_from_the_daemon_closes_the_link() {
        let (d, _) = deskd();
        let (mut s, h) = link(&d);
        let hdr = UnitHeader { codec: wire::Codec::H264, key: true, source_start: true, discardable: false, layer: 0, source: 1, width: 16, height: 16, seq: 0, captured_ns: 0, emitted_ns: 0 };
        wire::write_unit(&mut s, &hdr, b"x").unwrap();
        assert!(h.join().unwrap().is_err());
    }

    fn next_event(s: &mut UnixStream) -> J {
        match wire::read_message(s).unwrap().expect("an event") {
            Message::Json(v) => v,
            Message::Unit(_) => panic!("a unit on the control link"),
        }
    }

    #[test]
    fn hello_carries_protocol_4s_fields() {
        let (d, _) = deskd();
        let Event::Hello(h) = d.hello() else { panic!("a hello") };
        assert_eq!(h.accepts, vec![PROTOCOL_VERSION]);
        assert!(h.codecs.is_empty(), "no capture started: nothing to encode");
        assert_eq!(h.encoder, LinkEncoder::Unknown);
        let before = capture::clock_ns();
        let Event::Hello(h2) = d.hello() else { panic!("a hello") };
        assert!(h2.clock_ns >= before && h2.clock_ns >= h.clock_ns, "the clock of every unit's times, read now");
    }

    /// Where nothing captures (Linux until D8b), each layer that's on fails
    /// at once instead of leaving the hub's viewers waiting.
    #[test]
    fn layers_without_capture_fail_each_layer_thats_on() {
        let (d, _) = deskd();
        let (mut s, _h) = link(&d);
        let on = k2_deskd_proto::link::LayerSpec { id: 3, codec: wire::Codec::H264, max_width: 1920, fps: 60, bitrate: 12_000_000, quality: None, on: true };
        let off = k2_deskd_proto::link::LayerSpec { id: 4, on: false, ..on };
        let req = k2_deskd_proto::link::Request::Layers { layers: vec![on, off] };
        let mut v = serde_json::to_value(&req).unwrap();
        v["nr"] = json!(true);
        wire::write_json(&mut s, &v).unwrap();
        let e = next_event(&mut s);
        assert_eq!((e["event"].as_str(), e["layer"].as_u64(), e["code"].as_str()), (Some("layer_failed"), Some(3), Some("unsupported")));
        // Exactly one: the next message is the ping's reply.
        assert_eq!(call(&mut s, json!({"id": 1, "op": "ping"})), json!({"id": 1, "ok": true}));
        let r = call(&mut s, json!({"id": 2, "op": "display", "display": 1}));
        assert_eq!((r["ok"].as_bool(), r["code"].as_str()), (Some(false), Some("unsupported")));
    }

    /// The whole path on one machine: the engine's `layers` request on the
    /// control link, a frame from the (fake) capture, a protocol-4 unit on
    /// the frames pipe; the link closing stops the capture.
    #[test]
    fn layers_reach_the_capture_and_units_come_out_on_the_frames_pipe() {
        use crate::capture::tests::{fake_opener, push_frame};
        let (d, _) = deskd();
        let (opener, script) = fake_opener(1280, 720);
        let backend: Arc<dyn kessel_video::VideoBackend> = kessel_video::fake::FakeBackend::new(kessel_video::fake::FakeConfig::default());
        d.start_capture(opener, backend, LinkEncoder::Hardware).unwrap();
        let Event::Hello(h) = d.hello() else { panic!("a hello") };
        assert_eq!((h.codecs.as_slice(), h.encoder), (&[wire::Codec::H264][..], LinkEncoder::Hardware));
        let (mut frames_rx, frames_tx) = std::io::pipe().unwrap();
        d.attach_frames(Box::new(frames_tx));
        let (mut s, h) = link(&d);
        let layer = k2_deskd_proto::link::LayerSpec { id: 2, codec: wire::Codec::H264, max_width: 640, fps: 30, bitrate: 1_000_000, quality: None, on: true };
        let mut v = serde_json::to_value(k2_deskd_proto::link::Request::Layers { layers: vec![layer] }).unwrap();
        v["nr"] = json!(true);
        wire::write_json(&mut s, &v).unwrap();
        push_frame(&script, 1280, 720);
        match wire::read_message(&mut frames_rx).unwrap().expect("a unit") {
            Message::Unit(u) => {
                let uh = u.header;
                assert!(uh.key && uh.source_start, "{uh:?}");
                assert_eq!((uh.layer, uh.source, uh.width, uh.height, uh.seq), (2, 1, 640, 360, 0));
                assert!(u.data.starts_with(&[0, 0, 0, 1]), "Annex B");
            }
            Message::Json(_) => panic!("JSON on the frames pipe"),
        }
        drop(s);
        h.join().unwrap().unwrap();
        // The link closed: the capture stops (the fake counts the close).
        let t0 = Instant::now();
        while *script.lock().unwrap().closed.lock().unwrap() == 0 {
            assert!(t0.elapsed() < Duration::from_secs(5), "the capture didn't stop with the link");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
