//! `/cli/desktop/stream` (D6), headless: the §4 gate on its own ([`admit`]),
//! and whole viewers on real loopback sockets against a fake k2-deskd that
//! writes protocol-4 units into a pipe read by the engine's `LinkSource`.
//! Nothing here captures or encodes: the units are made-up bytes. Each
//! test is its own process under nextest (the hub, the registry and the
//! lease are process-wide).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use k2_core::connect_users::{self, Role};
use k2_core::desktop::wire::{Display as WireDisplay, Encoder, HostOs, Mode};
use k2_core::desktop::{grants, store, ticket, viewers, MachineState, CONSOLE};
use k2_core::sim::stream::{Codec, VideoHeader, ViewerPath};
use k2_core::sim::video::link::{write_unit, Codec as LinkCodec, LayerSpec, Request, UnitHeader};
use k2_core::stream::link_source::{DisplaySize, LinkClock, LinkFeed, LinkSource, SendRequest};
use k2_core::stream::EncodedSource;
use k2_deskd_proto::msg::{Display as DeskdDisplay, InputEvent, ReleaseReason};

use super::*;
use crate::desktop::deskd::{self, Deskd, NotBuilt};

const OWNER: &str = "owner-token-desktop-stream";

// ── a fake k2-deskd ───────────────────────────────────────────────────

#[derive(Default)]
struct Fake {
    display: Arc<AtomicU32>,
    injects: Mutex<Vec<InputEvent>>,
    releases: Mutex<Vec<ReleaseReason>>,
    selects: Mutex<Vec<u32>>,
    banners: Mutex<Vec<Option<String>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    events: Mutex<Option<std::sync::mpsc::Sender<StreamEvent>>>,
    sources: AtomicU32,
    failed: Mutex<Vec<(u8, String)>>,
    feed: Mutex<Option<LinkFeed>>,
}

fn deskd_display(id: u32, w: f64, h: f64, main: bool) -> DeskdDisplay {
    DeskdDisplay { id, name: format!("Display {id}"), x: 0.0, y: 0.0, width_pt: w, height_pt: h, width_px: w as u32, height_px: h as u32, scale: 1.0, main }
}

fn size_of(d: u32) -> (u32, u32) {
    if d == 2 {
        (1920, 1080)
    } else {
        (1280, 720)
    }
}

impl Fake {
    fn new() -> Arc<Fake> {
        let f = Fake::default();
        f.display.store(1, Ordering::SeqCst);
        Arc::new(f)
    }

    fn event(&self, e: StreamEvent) {
        if let Some(tx) = self.events.lock().unwrap().as_ref() {
            let _ = tx.send(e);
        }
    }
}

/// Writes units for the layers it was asked for (every 20 ms), key and
/// source start first and after a display change, until the source drops.
/// Like a Mac's deskd it makes no JPEG: a JPEG layer fails at once.
fn produce(mut w: std::io::PipeWriter, rx: std::sync::mpsc::Receiver<Request>, display: Arc<AtomicU32>, log: Arc<Mutex<Vec<Request>>>, feed: LinkFeed) {
    let t0 = Instant::now();
    let mut layers: Vec<LayerSpec> = Vec::new();
    let mut st: HashMap<u8, (u64, bool, u32)> = HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(r) => {
                log.lock().unwrap().push(r.clone());
                match r {
                    Request::Layers { layers: l } => {
                        for j in l.iter().filter(|l| l.on && l.codec == LinkCodec::Jpeg) {
                            feed.fail_layer(j.id, "no JPEG encoder on this Mac");
                        }
                        layers = l.into_iter().filter(|l| l.on && l.codec != LinkCodec::Jpeg).collect();
                        st.retain(|id, _| layers.iter().any(|l| l.id == *id));
                        for l in &layers {
                            st.entry(l.id).or_insert((0, true, 0));
                        }
                    }
                    Request::Keyframe { layer, .. } => {
                        if let Some(s) = st.get_mut(&layer) {
                            s.1 = true;
                        }
                    }
                    Request::Rate { .. } => {}
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let d = display.load(Ordering::SeqCst);
        for l in &layers {
            let Some(s) = st.get_mut(&l.id) else { continue };
            let start = s.2 != d;
            if start {
                *s = (0, true, d);
            }
            s.0 += 1;
            let (width, height) = size_of(d);
            let ns = t0.elapsed().as_nanos() as u64;
            let h = UnitHeader {
                codec: l.codec,
                key: s.1 || l.codec == LinkCodec::Jpeg,
                source_start: start,
                discardable: false,
                layer: l.id,
                source: d,
                width,
                height,
                seq: s.0,
                captured_ns: ns,
                emitted_ns: ns,
            };
            s.1 = false;
            if write_unit(&mut w, &h, l.codec.as_str().as_bytes()).is_err() {
                return;
            }
        }
    }
}

impl StreamDeskd for Fake {
    fn os(&self) -> HostOs {
        HostOs::Macos
    }
    fn encoder(&self) -> Encoder {
        Encoder::Hardware
    }
    fn codecs(&self) -> Vec<Codec> {
        vec![Codec::H264]
    }
    fn local_wins(&self) -> bool {
        true
    }
    /// A Mac's deskd: H.264 only.
    fn jpeg(&self) -> bool {
        false
    }
    fn fail_layer(&self, layer: u8, why: &str) -> bool {
        self.failed.lock().unwrap().push((layer, why.to_string()));
        match self.feed.lock().unwrap().as_ref() {
            Some(f) => {
                f.fail_layer(layer, why);
                true
            }
            None => false,
        }
    }
    fn displays(&self) -> Result<Vec<DeskdDisplay>, String> {
        Ok(vec![deskd_display(1, 1280.0, 720.0, true), deskd_display(2, 1920.0, 1080.0, false)])
    }
    fn source(&self, display: u32, _displays: &[DeskdDisplay]) -> Result<Box<dyn EncodedSource>, String> {
        self.sources.fetch_add(1, Ordering::SeqCst);
        let (rx, tx) = std::io::pipe().map_err(|e| e.to_string())?;
        let feed = LinkFeed::spawn(rx).map_err(|e| e.to_string())?;
        *self.feed.lock().unwrap() = Some(feed.clone());
        let (req_tx, req_rx) = std::sync::mpsc::channel::<Request>();
        let (d, log, f) = (self.display.clone(), self.requests.clone(), feed.clone());
        std::thread::spawn(move || produce(tx, req_rx, d, log, f));
        let send: SendRequest = Box::new(move |r: &Request| req_tx.send(r.clone()).map_err(|_| std::io::Error::other("producer gone")));
        let sizes = Box::new(|d: u32| Some(DisplaySize { points: size_of(d), native_px: Some(size_of(d)) }));
        Ok(Box::new(LinkSource::attach(&feed, LinkClock::new(0, Instant::now()), display, sizes, send)))
    }
    fn select_display(&self, display: u32) -> Result<(), String> {
        self.selects.lock().unwrap().push(display);
        self.display.store(display, Ordering::SeqCst);
        Ok(())
    }
    fn inject(&self, ev: InputEvent) -> Result<(), String> {
        self.injects.lock().unwrap().push(ev);
        Ok(())
    }
    fn release_all(&self, why: ReleaseReason) {
        self.releases.lock().unwrap().push(why);
    }
    fn clipboard_get(&self) -> Result<(Option<String>, Option<String>), String> {
        Ok((Some("from the mac".into()), None))
    }
    fn clipboard_set(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn banner(&self, text: Option<String>) {
        self.banners.lock().unwrap().push(text);
    }
    fn events(&self) -> std::sync::mpsc::Receiver<StreamEvent> {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.events.lock().unwrap() = Some(tx);
        rx
    }
}

/// The routes' side: ready, with the fake as its stream.
struct Backend(Arc<Fake>);

impl Deskd for Backend {
    fn backend(&self) -> &'static str {
        "macos"
    }
    fn state(&self) -> MachineState {
        MachineState::Ready
    }
    fn enable(&self) -> Result<(), String> {
        Ok(())
    }
    fn disable(&self) -> Result<(), String> {
        Ok(())
    }
    fn displays(&self) -> Vec<WireDisplay> {
        Vec::new()
    }
    fn stream(&self) -> Option<Arc<dyn StreamDeskd>> {
        Some(self.0.clone() as Arc<dyn StreamDeskd>)
    }
}

struct Env {
    _home: k2_core::test_env::TempHome,
    _deskd: deskd::TestDeskd,
    fake: Arc<Fake>,
}

fn env() -> Env {
    let home = k2_core::test_env::TempHome::new();
    k2_core::db::init_for_tests();
    let fake = Fake::new();
    let guard = deskd::set_for_test(Arc::new(Backend(fake.clone())));
    store::enable(CONSOLE, true, "owner").expect("switch on");
    Env { _home: home, _deskd: guard, fake }
}

fn login(name: &str, role: Role) -> String {
    connect_users::add_user(name, "password123").unwrap_or_else(|e| panic!("add_user {name}: {e}"));
    if role != Role::Member {
        connect_users::set_role(name, role).unwrap_or_else(|e| panic!("set_role {name}: {e}"));
    }
    connect_users::create_session(name)
}

fn ticket_for(name: &str, session: &str) -> String {
    ticket::mint(ticket::Claim { desktop: CONSOLE.into(), username: name.into(), session_token: session.into(), display: None })
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

fn tk(t: &str) -> HashMap<String, String> {
    params(&[("ticket", t)])
}

fn refused(r: Result<Admitted, Refused>) -> (u16, &'static str) {
    match r {
        Ok(a) => panic!("let in: {}", a.username),
        Err(r) => (r.status, r.code),
    }
}

// ── the gate ─────────────────────────────────────────────────────────

/// PRD §4: only a ticket opens the stream (the owner token, a shell, an
/// agent or an App pass has none); a ticket works once; Owners and Admins
/// watch and drive, a Member only with a grant and only watches, Voice &
/// Chat only never; nobody watches from this machine; the switch and a
/// working K2 Screen come first.
#[test]
fn the_gate_lets_in_only_a_ticketed_login_that_may_watch() {
    let _e = env();
    let olive = login("olive", Role::Owner);
    let ada = login("ada", Role::Admin);
    let mel = login("mel", Role::Member);
    let vic = login("vic", Role::VoiceChat);

    // No ticket: the owner token, a passport or an App pass in the query
    // never upgrades.
    for creds in [&[("token", OWNER)][..], &[("token", "sid-1.secret")][..], &[("token", "k2skn_app_pass")][..], &[][..]] {
        assert_eq!(refused(admit(&params(creds), "lan")), (403, "desktop_logins_only"), "{creds:?}");
    }
    // A session token where the ticket goes isn't a ticket.
    assert_eq!(refused(admit(&tk(&olive), "lan")), (401, "ticket_invalid"));

    let t = ticket_for("olive", &olive);
    let a = admit(&tk(&t), "lan").expect("an Owner is let in");
    assert!(a.may_control && a.role == Role::Owner && a.username == "olive");
    assert_eq!(refused(admit(&tk(&t), "lan")), (401, "ticket_invalid"), "a ticket works once");

    let a = admit(&tk(&ticket_for("ada", &ada)), "tunnel").expect("an Admin needs no grant");
    assert!(a.may_control);

    assert_eq!(refused(admit(&tk(&ticket_for("mel", &mel)), "lan")), (403, "not_allowed"), "a Member without a grant");
    grants::grant(CONSOLE, "mel", Mode::View, "owner").expect("grant");
    let a = admit(&tk(&ticket_for("mel", &mel)), "lan").expect("a Member with a grant watches");
    assert!(!a.may_control, "a Member never drives");

    assert_eq!(refused(admit(&tk(&ticket_for("vic", &vic)), "lan")), (403, "not_allowed"), "Voice & Chat only never");

    // Same machine: refused, no frame (R19).
    for ingress in ["loopback", "cell"] {
        assert_eq!(refused(admit(&tk(&ticket_for("olive", &olive)), ingress)), (403, "same_machine"), "{ingress}");
    }
    assert_eq!(refused(admit(&tk(&ticket_for("olive", &olive)), "app")), (403, "desktop_people_only"));

    // A ticket whose login signed out since.
    let gone = ticket_for("ada", &ada);
    connect_users::revoke_user_sessions("ada");
    assert_eq!(refused(admit(&tk(&gone), "lan")), (401, "auth_revoked"));

    // The switch, then K2 Screen.
    store::enable(CONSOLE, false, "owner").expect("off");
    assert_eq!(refused(admit(&tk(&ticket_for("olive", &olive)), "lan")), (409, "desktop_off"));
    store::enable(CONSOLE, true, "owner").expect("on");
    let _none = deskd::set_for_test(Arc::new(NotBuilt));
    let r = admit(&tk(&ticket_for("olive", &olive)), "lan");
    let Err(r) = r else { panic!("let in with no K2 Screen") };
    assert_eq!((r.status, r.code, r.state), (409, "desktop_unavailable", Some("unavailable")));
}

#[test]
fn the_engine_tick_is_five_seconds_and_s8_limits_are_the_prds() {
    assert_eq!(AUTH_TICK, Duration::from_secs(5));
    let l = Limits::default();
    assert_eq!((l.tick, l.idle, l.max), (AUTH_TICK, Duration::from_secs(1800), Duration::from_secs(8 * 3600)));
    assert_eq!(CONTROL_CACHE, Duration::from_millis(250));
}

#[test]
fn the_banner_says_who_watches_and_who_drives() {
    assert_eq!(banner_text(&[]), None);
    assert_eq!(banner_text(&[("olive".into(), Mode::Control)]).as_deref(), Some("olive is controlling this screen through K2"));
    assert_eq!(
        banner_text(&[("olive".into(), Mode::Control), ("mel".into(), Mode::View), ("ann".into(), Mode::View)]).as_deref(),
        Some("olive is controlling and ann, mel are viewing this screen through K2")
    );
}

// ── whole viewers ────────────────────────────────────────────────────

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("rt")
}

fn fast() -> Limits {
    Limits { tick: Duration::from_millis(150), ..Limits::default() }
}

/// One viewer over a real socket (`ingress` is the label the dispatcher
/// would give; the socket itself is loopback). `Err` = the HTTP status of
/// a refusal before the upgrade.
async fn open(t: &str, ingress: &str, extra: &[(&str, &str)], limits: Limits) -> Result<(Client, tokio::task::JoinHandle<()>), u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let mut p = params(&[("ticket", t), ("codecs", "h264,jpeg"), ("win", "3")]);
    p.extend(params(extra));
    let ingress = ingress.to_string();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        let req = StreamReq { params: p, ingress, path: ViewerPath::Lan, ip: "203.0.113.9".into(), owner_token: OWNER.into() };
        serve_with(&mut s, req, limits).await;
    });
    match tokio_tungstenite::connect_async(format!("ws://{addr}/cli/desktop/stream")).await {
        Ok((c, _)) => Ok((c, server)),
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
        Err(e) => panic!("connect: {e}"),
    }
}

async fn viewer(name: &str, session: &str, extra: &[(&str, &str)], limits: Limits) -> Client {
    open(&ticket_for(name, session), "lan", extra, limits).await.unwrap_or_else(|s| panic!("{name} refused with {s}")).0
}

/// What arrives next: a text message, a frame (acked, so the window never
/// fills), or the close.
enum Got {
    Text(Value),
    Frame(VideoHeader),
    Closed(Option<(u16, String)>),
}

async fn next(c: &mut Client, within: Duration) -> Option<Got> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let m = match tokio::time::timeout_at(deadline, c.next()).await {
            Err(_) => return None,
            Ok(None) | Ok(Some(Err(_))) => return Some(Got::Closed(None)),
            Ok(Some(Ok(m))) => m,
        };
        return Some(match m {
            Message::Text(t) => Got::Text(serde_json::from_str(&t).expect("JSON")),
            Message::Binary(b) => {
                let (h, _) = VideoHeader::decode(&b).expect("K2SV");
                let _ = c.send(Message::Text(format!(r#"{{"t":"ack","seq":{}}}"#, h.seq))).await;
                Got::Frame(h)
            }
            Message::Close(f) => Got::Closed(f.map(|f| (u16::from(f.code), f.reason.to_string()))),
            _ => continue,
        });
    }
}

/// The first text message `pred` takes, within 5 s.
async fn text_where(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match next(c, left).await {
            Some(Got::Text(v)) if pred(&v) => return v,
            Some(Got::Text(_)) | Some(Got::Frame(_)) => continue,
            Some(Got::Closed(f)) => panic!("closed ({f:?}) waiting for {what}"),
            None => panic!("no {what} within 5 s"),
        }
    }
}

async fn t_is(c: &mut Client, t: &str) -> Value {
    text_where(c, t, |v| v["t"] == t).await
}

async fn frame(c: &mut Client) -> VideoHeader {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match next(c, deadline.saturating_duration_since(Instant::now())).await {
            Some(Got::Frame(h)) => return h,
            Some(Got::Text(_)) => continue,
            other => panic!("no frame: {}", matches!(other, Some(Got::Closed(_)))),
        }
    }
}

/// The close code and reason, within `within`.
async fn closed(c: &mut Client, within: Duration) -> (u16, String) {
    let deadline = Instant::now() + within;
    loop {
        match next(c, deadline.saturating_duration_since(Instant::now())).await {
            Some(Got::Closed(Some(f))) => return f,
            Some(Got::Closed(None)) => panic!("closed with no code"),
            Some(_) => continue,
            None => panic!("still open after {within:?}"),
        }
    }
}

async fn say(c: &mut Client, v: Value) {
    c.send(Message::Text(v.to_string())).await.expect("send");
}

async fn wait_for(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "{what} within 5 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The server has let `n` closed controllers go: each one's lease is
/// dropped, then its held input released. The close reaches the client
/// before that, so a viewer opened at once can find the lease still held.
async fn controller_left(fake: &Arc<Fake>, n: usize) {
    wait_for("release when the closed controller left", || {
        fake.releases.lock().unwrap().iter().filter(|r| **r == ReleaseReason::ViewerClosed).count() >= n
    })
    .await;
}

fn audit_events() -> Vec<(String, String)> {
    k2_core::desktop::audit::read(None, 1000)
        .expect("audit")
        .into_iter()
        .map(|l| (l["event"].as_str().unwrap_or_default().to_string(), l["who"].as_str().unwrap_or_default().to_string()))
        .collect()
}

/// An Owner opens in control and drives; a Member with a grant gets the
/// same frames, but every input (and a take) is refused per message and
/// logged, and nothing reaches deskd.
#[test]
fn the_owner_drives_and_a_granted_member_only_watches() {
    let e = env();
    let olive = login("olive", Role::Owner);
    let mel = login("mel", Role::Member);
    grants::grant(CONSOLE, "mel", Mode::View, "owner").expect("grant");
    rt().block_on(async {
        let mut o = viewer("olive", &olive, &[], fast()).await;
        let hello = t_is(&mut o, "hello").await;
        assert_eq!((hello["desktop"].as_str(), hello["display"].as_str(), hello["codec"].as_str(), hello["mode"].as_str()), (Some("console"), Some("1"), Some("h264"), Some("control")));
        assert_eq!((hello["os"].as_str(), hello["encoder"].as_str(), hello["lane"].as_str(), hello["sameMachine"].as_bool()), (Some("macos"), Some("hardware"), Some("main"), Some(false)));
        assert_eq!(hello["displays"].as_array().map(Vec::len), Some(2));
        let ctl = t_is(&mut o, "control").await;
        assert_eq!((ctl["controller"].as_str(), ctl["you"].as_bool()), (Some("olive"), Some(true)));
        let h = frame(&mut o).await;
        assert!(h.key, "key first");
        say(&mut o, json!({"t": "pointer", "x": 10, "y": 20})).await;
        let fake = e.fake.clone();
        wait_for("the pointer at deskd", || fake.injects.lock().unwrap().iter().any(|i| *i == InputEvent::Pointer { display: 1, x: 10.0, y: 20.0 })).await;

        // The controller's clipboard: read and write reach deskd, and the
        // access log says only that they happened, never the text.
        say(&mut o, json!({"t": "clipboard-get"})).await;
        let c = t_is(&mut o, "clipboard").await;
        assert_eq!(c["text"], "from the mac");
        // Reads and writes share one clipboard use a second (input.rs).
        tokio::time::sleep(Duration::from_millis(1100)).await;
        say(&mut o, json!({"t": "clipboard-set", "text": "s3cret paste"})).await;
        wait_for("the clipboard lines", || {
            let ev = audit_events();
            ev.iter().any(|(e, w)| e == "clipboard_read" && w == "login:olive") && ev.iter().any(|(e, w)| e == "clipboard_write" && w == "login:olive")
        })
        .await;
        let raw = k2_core::desktop::audit::read(None, 1000).expect("audit");
        let clip: Vec<_> = raw.iter().filter(|l| l["event"].as_str().is_some_and(|e| e.starts_with("clipboard_"))).collect();
        assert!(clip.iter().all(|l| l["outcome"] == "ok"), "{clip:?}");
        let all = serde_json::to_string(&raw).unwrap();
        assert!(!all.contains("from the mac") && !all.contains("s3cret"), "no clipboard text in the access log");

        let mut m = viewer("mel", &mel, &[], fast()).await;
        let hello = t_is(&mut m, "hello").await;
        assert_eq!((hello["mode"].as_str(), hello["controller"].as_str()), (Some("view"), Some("olive")));
        let _ = frame(&mut m).await;
        let before = e.fake.injects.lock().unwrap().len();
        for input in [json!({"t": "pointer", "x": 1, "y": 1}), json!({"t": "key", "code": "KeyA", "down": true}), json!({"t": "text", "text": "hi"}), json!({"t": "clipboard-get"}), json!({"t": "take-control"})] {
            say(&mut m, input.clone()).await;
            let err = t_is(&mut m, "error").await;
            assert_eq!(err["code"], "desktop_watch_only", "{input}");
        }
        let _ = frame(&mut m).await;
        assert_eq!(e.fake.injects.lock().unwrap().len(), before, "nothing of the Member's reached deskd");
        assert!(audit_events().iter().any(|(ev, who)| ev == "refused" && who == "login:mel"), "the refusal is in the access log");
        let banners = e.fake.banners.clone_last();
        assert!(banners.as_deref().is_some_and(|b| b.contains("olive is controlling")), "{banners:?}");
        assert_eq!(viewers::count(CONSOLE), 2);
        assert_eq!(e.fake.sources.load(Ordering::SeqCst), 1, "one capture for both viewers");
        drop(m);
        drop(o);
        wait_for("both gone", || viewers::count(CONSOLE) == 0).await;
        let ev = audit_events();
        assert_eq!(ev.iter().filter(|(e, _)| e == "connect").count(), 2);
        wait_for("two disconnect lines", || audit_events().iter().filter(|(e, _)| e == "disconnect").count() == 2).await;
    });
}

trait LastOf {
    fn clone_last(&self) -> Option<String>;
}

impl LastOf for Mutex<Vec<Option<String>>> {
    fn clone_last(&self) -> Option<String> {
        self.lock().unwrap().last().cloned().flatten()
    }
}

/// A revoked grant, a downgrade to Member with no grant and a sign-out each
/// close with 4401 on the next tick (5 s in the daemon; 150 ms here).
#[test]
fn a_revoked_grant_a_downgrade_and_a_sign_out_close_on_the_tick() {
    let _e = env();
    let mel = login("mel", Role::Member);
    let olive = login("olive", Role::Owner);
    let ada = login("ada", Role::Admin);
    grants::grant(CONSOLE, "mel", Mode::View, "owner").expect("grant");
    rt().block_on(async {
        let mut m = viewer("mel", &mel, &[], fast()).await;
        let _ = frame(&mut m).await;
        grants::revoke(CONSOLE, "mel").expect("revoke");
        let t0 = Instant::now();
        assert_eq!(closed(&mut m, Duration::from_secs(3)).await, (4401, "grant_gone".into()));
        assert!(t0.elapsed() < Duration::from_secs(3));

        let mut o = viewer("olive", &olive, &[], fast()).await;
        let _ = frame(&mut o).await;
        connect_users::set_role("olive", Role::Member).expect("downgrade");
        // A role change ends the login's session too: the next acked frame
        // meets the L7 check (4013) or the tick does (4401), whichever is
        // first; either way within the tick.
        let (code, _) = closed(&mut o, Duration::from_secs(3)).await;
        assert!(code == 4401 || code == 4013, "an Owner moved to Member with no grant closes; got {code}");

        let mut a = viewer("ada", &ada, &[], fast()).await;
        let _ = frame(&mut a).await;
        connect_users::revoke_user_sessions("ada");
        // The next acked frame meets the L7 check (4013) or the tick does
        // (4401), whichever comes first.
        let (code, reason) = closed(&mut a, Duration::from_secs(3)).await;
        assert!((code, reason.as_str()) == (4401, "auth_revoked") || (code, reason.as_str()) == (4013, "access_lost"), "a sign-out closes; got {code} {reason}");
    });
}

/// The lease: one controller; another is told who drives; hand-back
/// releases held input and frees it; the next take works; a controller's
/// socket closing frees it too.
#[test]
fn take_and_hand_back_control() {
    let e = env();
    let ada = login("ada", Role::Admin);
    let olive = login("olive", Role::Owner);
    rt().block_on(async {
        let mut a = viewer("ada", &ada, &[], fast()).await;
        assert_eq!(t_is(&mut a, "hello").await["mode"], "control");
        let mut o = viewer("olive", &olive, &[("view", "1")], fast()).await;
        let hello = t_is(&mut o, "hello").await;
        assert_eq!((hello["mode"].as_str(), hello["controller"].as_str()), (Some("view"), Some("ada")));
        say(&mut o, json!({"t": "take-control"})).await;
        let err = t_is(&mut o, "error").await;
        assert_eq!(err["code"], "desktop_controlled");
        assert!(err["message"].as_str().unwrap_or_default().contains("ada"), "{err}");
        say(&mut o, json!({"t": "pointer", "x": 3, "y": 4})).await;
        assert_eq!(t_is(&mut o, "error").await["code"], "desktop_controlled");

        say(&mut a, json!({"t": "hand-back"})).await;
        let c = text_where(&mut o, "control: nobody", |v| v["t"] == "control" && v["controller"].is_null()).await;
        assert_eq!(c["you"], false);
        let fake = e.fake.clone();
        wait_for("release on hand-back", || fake.releases.lock().unwrap().contains(&ReleaseReason::HandBack)).await;
        say(&mut a, json!({"t": "pointer", "x": 1, "y": 1})).await;
        assert_eq!(t_is(&mut a, "error").await["code"], "desktop_not_controller");

        say(&mut o, json!({"t": "take-control"})).await;
        let c = text_where(&mut o, "control: olive", |v| v["t"] == "control" && v["controller"] == "olive").await;
        assert_eq!(c["you"], true);
        say(&mut o, json!({"t": "pointer", "x": 7, "y": 8})).await;
        wait_for("olive's pointer", || fake.injects.lock().unwrap().iter().any(|i| *i == InputEvent::Pointer { display: 1, x: 7.0, y: 8.0 })).await;
        say(&mut a, json!({"t": "key", "code": "KeyA", "down": true})).await;
        assert_eq!(t_is(&mut a, "error").await["code"], "desktop_controlled");

        drop(o);
        let c = text_where(&mut a, "control freed", |v| v["t"] == "control" && v["controller"].is_null()).await;
        assert_eq!(c["you"], false);
        wait_for("release on close", || fake.releases.lock().unwrap().contains(&ReleaseReason::ViewerClosed)).await;
        let ev = audit_events();
        assert!(ev.iter().any(|(e, w)| e == "control_taken" && w == "login:olive"), "{ev:?}");
        assert!(ev.iter().any(|(e, w)| e == "control_handed_back" && w == "login:ada"), "{ev:?}");
    });
}

/// Only the controller switches display, and the switch moves every
/// viewer (one capture per desktop): both get `displays` naming it, and
/// frames from the new display.
#[test]
fn only_the_controller_switches_display_and_it_moves_everyone() {
    let e = env();
    let ada = login("ada", Role::Admin);
    let olive = login("olive", Role::Owner);
    rt().block_on(async {
        let mut a = viewer("ada", &ada, &[], fast()).await;
        let _ = frame(&mut a).await;
        let mut o = viewer("olive", &olive, &[("view", "1"), ("display", "2")], fast()).await;
        assert_eq!(t_is(&mut o, "hello").await["display"], "1", "a joiner gets the display the desktop shows");
        say(&mut o, json!({"t": "display", "id": "2"})).await;
        assert_eq!(t_is(&mut o, "error").await["code"], "desktop_controlled");
        assert!(e.fake.selects.lock().unwrap().is_empty());

        say(&mut a, json!({"t": "display", "id": "9"})).await;
        assert_eq!(t_is(&mut a, "error").await["code"], "desktop_no_such_display");
        say(&mut a, json!({"t": "display", "id": "2"})).await;
        for c in [&mut a, &mut o] {
            let d = text_where(c, "displays", |v| v["t"] == "displays").await;
            assert_eq!(d["display"], "2");
        }
        assert_eq!(*e.fake.selects.lock().unwrap(), vec![2]);
        assert!(e.fake.releases.lock().unwrap().contains(&ReleaseReason::DisplaySwitch));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let h = frame(&mut o).await;
            if (h.width_px, h.height_px) == (1920, 1080) {
                assert!(h.key, "the new display starts on a key");
                break;
            }
            assert!(Instant::now() < deadline, "frames from display 2");
        }
        say(&mut a, json!({"t": "pointer", "x": 5, "y": 6})).await;
        let fake = e.fake.clone();
        wait_for("a pointer on display 2", || fake.injects.lock().unwrap().iter().any(|i| *i == InputEvent::Pointer { display: 2, x: 5.0, y: 6.0 })).await;
    });
}

/// S8: a hidden tab (`fps 0`) with no input closes after the idle limit;
/// every viewer closes after the session limit (30 min and 8 h in the
/// daemon; short here).
#[test]
fn a_hidden_idle_viewer_and_a_long_session_close() {
    let _e = env();
    let olive = login("olive", Role::Owner);
    rt().block_on(async {
        let idle = Limits { tick: Duration::from_millis(100), idle: Duration::from_millis(400), max: Duration::from_secs(3600) };
        let mut shown = viewer("olive", &olive, &[("view", "1")], idle).await;
        let mut hidden = viewer("olive", &olive, &[("view", "1")], idle).await;
        let _ = frame(&mut hidden).await;
        say(&mut hidden, json!({"t": "fps", "fps": 0})).await;
        assert_eq!(closed(&mut hidden, Duration::from_secs(3)).await, (4403, "idle".into()));
        // The shown tab, with no input either, stays.
        let _ = frame(&mut shown).await;
        assert!(viewers::count(CONSOLE) >= 1);

        let short = Limits { tick: Duration::from_millis(100), idle: Duration::from_secs(3600), max: Duration::from_millis(400) };
        let mut long = viewer("olive", &olive, &[("view", "1")], short).await;
        assert_eq!(closed(&mut long, Duration::from_secs(3)).await, (4403, "session_limit".into()));
    });
}

/// `kick`, `revoke` and `disable` reach an open viewer through the
/// registry; Disconnect on the banner closes every viewer.
#[test]
fn kick_disable_and_disconnect_at_the_screen_close_viewers() {
    let e = env();
    let olive = login("olive", Role::Owner);
    let ada = login("ada", Role::Admin);
    rt().block_on(async {
        let mut a = viewer("ada", &ada, &[], fast()).await;
        let _ = frame(&mut a).await;
        assert_eq!(viewers::close_user(CONSOLE, "", "ada", 4403, "kicked"), 1);
        assert_eq!(closed(&mut a, Duration::from_secs(3)).await, (4403, "kicked".into()));
        controller_left(&e.fake, 1).await;

        let mut o = viewer("olive", &olive, &[], fast()).await;
        let _ = frame(&mut o).await;
        e.fake.event(StreamEvent::BannerDisconnect);
        assert_eq!(closed(&mut o, Duration::from_secs(3)).await, (4403, "disconnected_at_screen".into()));
        // Olive drove: until the server lets that viewer go, the next one
        // would open watching, not driving.
        controller_left(&e.fake, 2).await;

        let mut o = viewer("olive", &olive, &[], fast()).await;
        let _ = frame(&mut o).await;
        e.fake.event(StreamEvent::LocalInput { hold: true });
        let c = text_where(&mut o, "control with a local hold", |v| v["t"] == "control" && v["localHold"] == true).await;
        assert_eq!(c["you"], true, "a local hold pauses, it doesn't take the lease");
        say(&mut o, json!({"t": "pointer", "x": 2, "y": 2})).await;
        assert_eq!(t_is(&mut o, "error").await["code"], "desktop_local_hold");
        e.fake.event(StreamEvent::CursorPos { x: 4.0, y: 5.0, visible: true });
        let p = t_is(&mut o, "cursor-pos").await;
        assert_eq!((p["x"].as_f64(), p["y"].as_f64()), (Some(4.0), Some(5.0)));

        // The disable route closes every viewer with 4410.
        let n = viewers::close_all(CONSOLE, k2_core::desktop::wire::CLOSE_DESKTOP_OFF, "desktop_off");
        assert_eq!(n, 1);
        assert_eq!(closed(&mut o, Duration::from_secs(3)).await, (4410, "desktop_off".into()));
        // A refusal before the upgrade over the socket: a spent ticket.
        let t = ticket_for("olive", &olive);
        let (c, _s) = open(&t, "lan", &[], fast()).await.expect("first use");
        drop(c);
        assert_eq!(open(&t, "lan", &[], fast()).await.err(), Some(401));
        assert_eq!(open(&ticket_for("olive", &olive), "loopback", &[], fast()).await.err(), Some(403), "same machine");
    });
}

/// A Mac's K2 Screen sends H.264 only: a viewer that can't decode it is
/// refused before the upgrade (no audit line, no viewer). A viewer whose
/// H.264 layer fails moves to JPEG, the JPEG layer fails too, and the viewer
/// ends on a clear card with the reason (4410).
#[test]
fn a_mac_s_h264_only_desktop_refuses_jpeg_viewers_and_ends_on_a_failed_layer() {
    let e = env();
    let olive = login("olive", Role::Owner);
    rt().block_on(async {
        let t = ticket_for("olive", &olive);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.expect("accept");
            let req = StreamReq { params: params(&[("ticket", t.as_str()), ("codecs", "jpeg")]), ingress: "lan".into(), path: ViewerPath::Lan, ip: "-".into(), owner_token: OWNER.into() };
            serve_with(&mut s, req, fast()).await;
        });
        match tokio_tungstenite::connect_async(format!("ws://{addr}/cli/desktop/stream")).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), 409),
            Ok(_) => panic!("a JPEG-only viewer was let in to an H.264-only desktop"),
            Err(e) => panic!("connect: {e}"),
        }
        let _ = server.await;
        assert_eq!(viewers::count(CONSOLE), 0);
        assert!(!audit_events().iter().any(|(ev, _)| ev == "connect"));

        let mut o = viewer("olive", &olive, &[], fast()).await;
        let _ = frame(&mut o).await;
        e.fake.event(StreamEvent::LayerFailed { layer: 0, why: "VideoToolbox wouldn't open a session".into() });
        assert_eq!(closed(&mut o, Duration::from_secs(3)).await, (4410, "deskd_ended".into()));
        assert_eq!(e.fake.failed.lock().unwrap().len(), 1, "deskd's failure reached the hub once");
        wait_for("the viewer gone", || viewers::count(CONSOLE) == 0).await;
        let req = e.fake.requests.lock().unwrap().clone();
        assert!(
            req.iter().any(|r| matches!(r, Request::Layers { layers } if layers.iter().any(|l| l.on && l.codec == LinkCodec::Jpeg))),
            "the hub tried JPEG after H.264 failed: {req:?}"
        );
    });
}

/// The viewer of a failed H.264 layer on a Mac ends on the reason's card,
/// after the move to JPEG (the engine sends a let-go viewer its `Ended`
/// reason before the 4410 close).
#[test]
fn a_mac_viewer_whose_picture_fails_ends_on_the_reason_card() {
    let e = env();
    let olive = login("olive", Role::Owner);
    rt().block_on(async {
        let mut o = viewer("olive", &olive, &[], fast()).await;
        let _ = frame(&mut o).await;
        e.fake.event(StreamEvent::LayerFailed { layer: 0, why: "VideoToolbox wouldn't open a session".into() });
        // The move to JPEG and its failure come within a millisecond; the
        // viewer may or may not see the `codec` line in between, but it
        // always ends on the reason.
        let st = t_is(&mut o, "state").await;
        assert_eq!(st["message"], "The picture stopped: this source can't send JPEG: no JPEG encoder on this Mac. Try again.");
        assert_eq!(closed(&mut o, Duration::from_secs(3)).await, (4410, "deskd_ended".into()));
    });
}

/// The rate row is picked with the display's height (K2 Render's L2 note):
/// a 720p desktop's VP8 runs at 30, a 1080p one at 15, a 4K one (shrunk
/// to 1920 wide, 1080 lines) at 15.
#[test]
fn the_vp8_rate_follows_the_displays_height() {
    use crate::stream::rates;
    use k2_core::sim::stream::ViewerPath;
    let list = vec![deskd_display(1, 1280.0, 720.0, true), deskd_display(2, 1920.0, 1080.0, false), deskd_display(3, 3840.0, 2160.0, false)];
    assert_eq!(layer_height(&list, 1), Some(720));
    assert_eq!(layer_height(&list, 3), Some(1080), "after the 1920-wide cap");
    assert_eq!(layer_height(&list, 9), None);
    let cap = |id| rates::cap_sized(SourceKind::Desktop, k2_core::stream::Codec::Vp8, ViewerPath::Tunnel, layer_height(&list, id));
    assert_eq!((cap(1), cap(2), cap(3)), (30, 15, 15));
    let offer = crate::stream::viewer::Offer::from_params(&[("codecs".to_string(), "vp8,jpeg".to_string())].into_iter().collect());
    let neg = crate::stream::viewer::negotiate(SourceKind::Desktop, &offer, &[Codec::Vp8], ViewerPath::Tunnel, layer_height(&list, 1));
    assert_eq!((neg.codec, neg.fps), (Codec::Vp8, 30), "720p VP8 isn't held to the 1080p row");
}

// ── live, on Linux X11 (L2 / D8b) ────────────────────────────────────

#[cfg(target_os = "linux")]
#[path = "stream_live_tests.rs"]
mod live;
