//! The engine's viewer loop with a second producer (S5): a fake k2-deskd
//! writing protocol-4 units into an in-memory pipe, read by a
//! [`LinkSource`], served through [`serve`] on real loopback sockets with
//! desktop-style hooks (a short auth tick, view-only per message, a
//! recording input sink, [`DesktopRule`]). Linux is enough: nothing here
//! encodes; the units are made-up bytes.

use super::*;
use k2_core::sim::stream::CLOSE_AUTH_REVOKED;
use k2_core::stream::link_source::{DisplaySize, LinkClock, LinkSource, SendRequest, SizesFn};
use k2_core::sim::video::link::{write_unit, Codec as LinkCodec, LayerSpec, Request, UnitHeader};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// A desktop-like viewer message (D1 owns the real one).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
enum DeskMsg {
    Ack { seq: u32 },
    Fps { fps: u32 },
    Keyframe,
    Pointer { x: f64, y: f64 },
    Button { button: String, down: bool },
    Wheel { dx: f64, dy: f64, mode: String },
    Key { code: String, down: bool },
    Text { text: String },
}

impl StreamMsg for DeskMsg {
    fn engine(&self) -> Option<EngineMsg> {
        match self {
            DeskMsg::Ack { seq } => Some(EngineMsg::Ack { seq: *seq }),
            DeskMsg::Fps { fps } => Some(EngineMsg::Fps { fps: *fps }),
            DeskMsg::Keyframe => Some(EngineMsg::Keyframe),
            _ => None,
        }
    }
}

impl PointerWheel for DeskMsg {
    fn is_pointer(&self) -> bool {
        matches!(self, DeskMsg::Pointer { .. })
    }

    fn wheel(&self) -> Option<Wheel> {
        match self {
            DeskMsg::Wheel { dx, dy, mode } => Some(Wheel { dx: *dx, dy: *dy, lines: mode == "line" }),
            _ => None,
        }
    }

    fn set_wheel(&mut self, w: Wheel) {
        if let DeskMsg::Wheel { dx, dy, .. } = self {
            *dx = w.dx;
            *dy = w.dy;
        }
    }
}

fn pointer(x: f64) -> DeskMsg {
    DeskMsg::Pointer { x, y: 5.0 }
}

fn wheel(dy: f64, mode: &str) -> DeskMsg {
    DeskMsg::Wheel { dx: 0.0, dy, mode: mode.into() }
}

fn key(down: bool) -> DeskMsg {
    DeskMsg::Key { code: "KeyA".into(), down }
}

fn button(down: bool) -> DeskMsg {
    DeskMsg::Button { button: "left".into(), down }
}

/// PRD §6: pointer moves collapse to the last, wheel steps of one mode
/// sum, and nothing merges across a button, key or text. Each output keeps
/// the index of the last message it stands for.
#[test]
fn the_desktop_rule_collapses_pointers_and_sums_wheels_but_never_across_a_press() {
    let batch = vec![
        pointer(1.0),
        pointer(2.0),
        pointer(3.0),
        button(true),
        pointer(4.0),
        wheel(1.0, "pixel"),
        wheel(2.0, "pixel"),
        wheel(5.0, "line"),
        DeskMsg::Text { text: "a".into() },
        wheel(3.0, "line"),
        wheel(4.0, "line"),
        key(true),
        key(true),
        button(false),
        button(false),
    ];
    let out = DesktopRule.coalesce(batch);
    let want = vec![
        (2, pointer(3.0)),
        (3, button(true)),
        (4, pointer(4.0)),
        (6, wheel(3.0, "pixel")),
        (7, wheel(5.0, "line")),
        (8, DeskMsg::Text { text: "a".into() }),
        (10, wheel(7.0, "line")),
        (11, key(true)),
        (12, key(true)),
        (13, button(false)),
        (14, button(false)),
    ];
    assert_eq!(out, want);
    assert!(DesktopRule.coalesce(Vec::<DeskMsg>::new()).is_empty());
    assert_eq!(KeepAll.coalesce(vec![pointer(1.0), pointer(2.0)]).len(), 2);
}

// ── a fake k2-deskd ───────────────────────────────────────────────────

/// What the test sees of the fake producer.
#[derive(Default)]
struct Ctl {
    /// The display it captures (a change restarts every layer on it with
    /// a key + source start and seq 1).
    display: AtomicU32,
    requests: Mutex<Vec<Request>>,
}

impl Ctl {
    fn new() -> Arc<Ctl> {
        let c = Ctl::default();
        c.display.store(1, Ordering::SeqCst);
        Arc::new(c)
    }
}

fn size_of(display: u32) -> (u32, u32) {
    if display == 2 {
        (1920, 1080)
    } else {
        (1280, 720)
    }
}

fn sizes() -> SizesFn {
    Box::new(|d| Some(DisplaySize { points: size_of(d), native_px: Some(size_of(d)) }))
}

struct LayerState {
    seq: u64,
    key: bool,
    start: bool,
    next: Instant,
    display: u32,
}

/// Writes protocol-4 units for the layers it was asked for, each at its
/// fps, until its request channel closes (the source dropped).
fn produce(mut w: std::io::PipeWriter, rx: std::sync::mpsc::Receiver<Request>, ctl: Arc<Ctl>) {
    let t0 = Instant::now();
    let mut layers: Vec<LayerSpec> = Vec::new();
    let mut st: HashMap<u8, LayerState> = HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(r) => {
                ctl.requests.lock().unwrap().push(r.clone());
                match r {
                    Request::Layers { layers: l } => {
                        layers = l.into_iter().filter(|l| l.on).collect();
                        st.retain(|id, _| layers.iter().any(|l| l.id == *id));
                        for l in &layers {
                            let display = ctl.display.load(Ordering::SeqCst);
                            st.entry(l.id).or_insert(LayerState { seq: 0, key: true, start: true, next: Instant::now(), display });
                        }
                    }
                    Request::Keyframe { layer, .. } => {
                        if let Some(s) = st.get_mut(&layer) {
                            s.key = true;
                        }
                    }
                    Request::Rate { .. } => {}
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        let display = ctl.display.load(Ordering::SeqCst);
        for l in &layers {
            let Some(s) = st.get_mut(&l.id) else { continue };
            if now < s.next {
                continue;
            }
            s.next = now + Duration::from_millis(1000 / l.fps.max(1) as u64);
            if s.display != display {
                // deskd keys the new display's first frame; this one also
                // starts its seqs over.
                *s = LayerState { seq: 0, key: true, start: true, next: s.next, display };
            }
            s.seq += 1;
            let (width, height) = size_of(display);
            let ns = t0.elapsed().as_nanos() as u64;
            let h = UnitHeader {
                codec: l.codec,
                key: s.key || l.codec == LinkCodec::Jpeg,
                source_start: s.start,
                discardable: false,
                layer: l.id,
                source: display,
                width,
                height,
                seq: s.seq,
                captured_ns: ns,
                emitted_ns: ns,
            };
            s.key = false;
            s.start = false;
            if write_unit(&mut w, &h, l.codec.as_str().as_bytes()).is_err() {
                return;
            }
        }
    }
}

/// A desktop source over the fake producer. `name` keeps each test's hub
/// apart.
fn desktop_source(name: &str, gives: Vec<Codec>, ctl: Arc<Ctl>) -> Source<String> {
    Source {
        key: SourceKey::Desktop { desktop: format!("test-{name}"), display: 1 },
        kind: SourceKind::Desktop,
        open: Box::new(move || {
            let (rx, tx) = std::io::pipe().map_err(|e| e.to_string())?;
            let (req_tx, req_rx) = std::sync::mpsc::channel::<Request>();
            std::thread::spawn(move || produce(tx, req_rx, ctl));
            let send: SendRequest = Box::new(move |r: &Request| req_tx.send(r.clone()).map_err(|_| std::io::Error::other("producer gone")));
            let src = LinkSource::new(rx, LinkClock::new(0, Instant::now()), 1, sizes(), send).map_err(|e| e.to_string())?;
            Ok(Box::new(src) as Box<dyn EncodedSource>)
        }),
        gives,
    }
}

/// The input sink: records what reaches deskd, and can be held (busy on
/// one input, so the rest back up and coalesce).
#[derive(Clone, Default)]
struct Rec {
    got: Arc<Mutex<Vec<DeskMsg>>>,
    hold: Arc<AtomicBool>,
}

impl InputSink<DeskMsg> for Rec {
    fn send(&mut self, m: DeskMsg) -> Result<(), Refusal> {
        while self.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.got.lock().unwrap().push(m);
        Ok(())
    }
}

/// Desktop-style hooks: the re-check every 100 ms closes with 4401 once
/// `revoked` is set; a view-only viewer's input is refused per message.
fn hooks(revoked: Arc<AtomicBool>, view_only: bool, sink: Rec) -> Hooks<DeskMsg> {
    Hooks {
        auth: Auth {
            every: Duration::from_millis(100),
            recheck: Box::new(move || {
                if revoked.load(Ordering::SeqCst) {
                    Verdict::Close { code: CLOSE_AUTH_REVOKED, reason: "grant revoked".into() }
                } else {
                    Verdict::Keep
                }
            }),
            frame: Box::new(|_: &WsFrame| Verdict::Keep),
            may_input: Box::new(move |_: &DeskMsg| if view_only { Err(Refusal::new("desktop_view_only", "You can watch this desktop but not use it.")) } else { Ok(()) }),
        },
        limits: Box::new(|_: &DeskMsg, _: Instant| Ok(())),
        sink: Box::new(sink),
        coalesce: Box::new(DesktopRule),
        hello: Box::new(|_: &Negotiated| {
            let mut m = Map::new();
            m.insert("desktop".into(), "console".into());
            m.insert("fps".into(), 999.into()); // the engine's own field wins
            m
        }),
        push: None,
        busy: Refusal::new("desktop_busy", "Too much input at once; slow down."),
        ended: Box::new(|_: &str| None),
        ended_reason: "desktop off".into(),
        tag: "desktop-test",
    }
}

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn open(source: Source<String>, query: &[(&str, &str)], hooks: Hooks<DeskMsg>) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let params: HashMap<String, String> = query.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        serve(&mut s, source, Offer::from_params(&params), ViewerPath::Local, hooks).await.expect("upgraded");
    });
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/cli/desktop/stream")).await.expect("connect");
    (client, server)
}

async fn next_text(c: &mut Client) -> serde_json::Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), c.next()).await.expect("a message within 5 s").expect("open").expect("ok");
        match m {
            Message::Text(t) => return serde_json::from_str(&t).expect("JSON"),
            Message::Binary(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The next text message, acking the frames before it (so the window
/// never fills while a test waits for a reply).
async fn next_text_acking(c: &mut Client) -> serde_json::Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), c.next()).await.expect("a message within 5 s").expect("open").expect("ok");
        match m {
            Message::Text(t) => return serde_json::from_str(&t).expect("JSON"),
            Message::Binary(b) => {
                let seq = VideoHeader::decode(&b).map(|(h, _)| h.seq).or_else(|| FrameHeader::decode(&b).map(|(h, _)| h.seq)).expect("a frame");
                ack(c, seq).await;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn next_binary(c: &mut Client, within: Duration) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let m = match tokio::time::timeout_at(deadline, c.next()).await {
            Err(_) => return None,
            Ok(m) => m.expect("open").expect("ok"),
        };
        match m {
            Message::Binary(b) => return Some(b),
            Message::Text(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn ack(c: &mut Client, seq: u32) {
    c.send(Message::Text(format!(r#"{{"t":"ack","seq":{seq}}}"#))).await.expect("ack");
}

/// The next K2SV frame, acked.
async fn next_video(c: &mut Client) -> (VideoHeader, Vec<u8>) {
    let m = next_binary(c, Duration::from_secs(5)).await.expect("a frame within 5 s");
    let (h, data) = VideoHeader::decode(&m).expect("K2SV");
    ack(c, h.seq).await;
    (h, data.to_vec())
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt")
}

async fn ends(server: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("the server ends when the viewer leaves").expect("join");
}

/// (a) A viewer of a protocol-4 source gets its units through `serve`,
/// key first, then the chain; `hello` has the engine's fields (the
/// desktop's rate row: local H.264 at 60) over the route's.
#[test]
fn a_viewer_gets_a_second_producers_units_key_first() {
    rt().block_on(async {
        let ctl = Ctl::new();
        let (mut c, server) = open(desktop_source("a", vec![Codec::H264], ctl.clone()), &[("codecs", "h264,jpeg"), ("win", "3")], hooks(Default::default(), false, Rec::default())).await;
        let hello = next_text(&mut c).await;
        assert_eq!(hello["t"], "hello");
        assert_eq!((hello["codec"].as_str(), hello["window"].as_u64(), hello["fps"].as_u64(), hello["path"].as_str()), (Some("h264"), Some(3), Some(60), Some("local")));
        assert_eq!(hello["desktop"], "console", "the route's field");
        let mut heads = Vec::new();
        for _ in 0..4 {
            let (h, data) = next_video(&mut c).await;
            assert_eq!(h.codec, k2_core::sim::stream::VIDEO_CODEC_H264);
            assert_eq!(data, b"h264", "the producer's bytes, relayed");
            assert_eq!((h.width_px, h.height_px, h.width_pt, h.height_pt), (1280, 720, 1280, 720));
            heads.push(h);
        }
        assert!(heads[0].key, "key first");
        assert!(heads[1..].iter().all(|h| !h.key), "then deltas");
        assert!(heads.windows(2).all(|w| w[1].seq == w[0].seq + 1));
        let asked: Vec<Request> = ctl.requests.lock().unwrap().clone();
        assert!(
            asked.iter().any(|r| matches!(r, Request::Layers { layers } if layers.iter().any(|l| l.on && l.codec == LinkCodec::H264 && l.fps == 60 && l.max_width == 1280))),
            "one H.264 layer at 60, the points width: {asked:?}"
        );
        drop(c);
        ends(server).await;
    });
}

/// (b) The auth hook flipping to revoked closes the socket with 4401 on
/// the next tick.
#[test]
fn a_revoked_grant_closes_with_4401_within_the_tick() {
    rt().block_on(async {
        let revoked = Arc::new(AtomicBool::new(false));
        let (mut c, server) = open(desktop_source("b", vec![Codec::H264], Ctl::new()), &[("codecs", "h264"), ("win", "3")], hooks(revoked.clone(), false, Rec::default())).await;
        let _ = next_text(&mut c).await;
        let _ = next_video(&mut c).await;
        revoked.store(true, Ordering::SeqCst);
        let t0 = Instant::now();
        let code = loop {
            let m = tokio::time::timeout(Duration::from_secs(2), c.next()).await.expect("closed within 2 s").expect("open").expect("ok");
            if let Message::Close(f) = m {
                break f.map(|f| u16::from(f.code));
            }
        };
        assert_eq!(code, Some(CLOSE_AUTH_REVOKED));
        assert!(t0.elapsed() < Duration::from_millis(600), "within the 100 ms tick (and some): {:?}", t0.elapsed());
        ends(server).await;
    });
}

/// (c) A view-only viewer's input is refused per message, nothing reaches
/// the sink, and frames keep coming.
#[test]
fn a_view_only_viewers_input_is_refused_per_message_while_frames_flow() {
    rt().block_on(async {
        let rec = Rec::default();
        let (mut c, server) = open(desktop_source("c", vec![Codec::H264], Ctl::new()), &[("codecs", "h264"), ("win", "3")], hooks(Default::default(), true, rec.clone())).await;
        let _ = next_text(&mut c).await;
        let _ = next_video(&mut c).await;
        for m in [r#"{"t":"pointer","x":1,"y":2}"#, r#"{"t":"key","code":"KeyA","down":true}"#] {
            c.send(Message::Text(m.into())).await.expect("send");
            let e = next_text_acking(&mut c).await;
            assert_eq!((e["t"].as_str(), e["code"].as_str()), (Some("error"), Some("desktop_view_only")), "{e}");
        }
        for _ in 0..3 {
            let _ = next_video(&mut c).await;
        }
        assert!(rec.got.lock().unwrap().is_empty(), "nothing reached deskd");
        drop(c);
        ends(server).await;
    });
}

/// (d) Input that backs up while the sink is busy reaches it coalesced by
/// the desktop rule: pointer moves to the last, wheel steps summed, never
/// across a button or key.
#[test]
fn the_input_sink_gets_coalesced_pointers_and_summed_wheels_never_across_a_button() {
    rt().block_on(async {
        let rec = Rec::default();
        let (mut c, server) = open(desktop_source("d", vec![Codec::H264], Ctl::new()), &[("codecs", "h264"), ("win", "3")], hooks(Default::default(), false, rec.clone())).await;
        let _ = next_text(&mut c).await;
        rec.hold.store(true, Ordering::SeqCst);
        c.send(Message::Text(r#"{"t":"key","code":"KeyA","down":true}"#.into())).await.expect("first");
        tokio::time::sleep(Duration::from_millis(200)).await; // the input thread takes it and waits in the sink
        let mut sends: Vec<String> = Vec::new();
        sends.extend((1..=10).map(|x| format!(r#"{{"t":"pointer","x":{x},"y":5}}"#)));
        sends.extend([1, 2, 3].iter().map(|dy| format!(r#"{{"t":"wheel","dx":0,"dy":{dy},"mode":"pixel"}}"#)));
        sends.push(r#"{"t":"wheel","dx":0,"dy":1,"mode":"line"}"#.into());
        sends.push(r#"{"t":"button","button":"left","down":true}"#.into());
        sends.extend((11..=15).map(|x| format!(r#"{{"t":"pointer","x":{x},"y":5}}"#)));
        sends.extend([10, 20].iter().map(|dy| format!(r#"{{"t":"wheel","dx":0,"dy":{dy},"mode":"pixel"}}"#)));
        sends.push(r#"{"t":"key","code":"KeyA","down":false}"#.into());
        for m in sends {
            c.send(Message::Text(m)).await.expect("send");
        }
        tokio::time::sleep(Duration::from_millis(300)).await; // all queued behind the held one
        rec.hold.store(false, Ordering::SeqCst);
        let t0 = Instant::now();
        while !rec.got.lock().unwrap().last().is_some_and(|m| *m == key(false)) {
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", rec.got.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let got = rec.got.lock().unwrap().clone();
        assert_eq!(got, vec![key(true), pointer(10.0), wheel(6.0, "pixel"), wheel(1.0, "line"), button(true), pointer(15.0), wheel(30.0, "pixel"), key(false)]);
        drop(c);
        ends(server).await;
    });
}

/// (e) A VP8 source: a VP8 viewer gets K2SV with codec byte 2; an
/// H.264-only viewer is negotiated to JPEG (K2SF) and never gets a VP8
/// unit; the producer is never asked for H.264.
#[test]
fn vp8_units_reach_a_vp8_viewer_as_codec_2_and_never_an_h264_only_viewer() {
    rt().block_on(async {
        let ctl = Ctl::new();
        let gives = vec![Codec::Vp8];
        let (mut v, s1) = open(desktop_source("e", gives.clone(), ctl.clone()), &[("codecs", "vp8,jpeg"), ("win", "3")], hooks(Default::default(), false, Rec::default())).await;
        let (mut h, s2) = open(desktop_source("e", gives, ctl.clone()), &[("codecs", "h264,jpeg"), ("win", "3")], hooks(Default::default(), false, Rec::default())).await;
        let hv = next_text(&mut v).await;
        assert_eq!((hv["codec"].as_str(), hv["fps"].as_u64()), (Some("vp8"), Some(15)), "VP8 with no height known: the 1080p row");
        let hh = next_text(&mut h).await;
        assert_eq!((hh["codec"].as_str(), hh["fps"].as_u64()), (Some("jpeg"), Some(15)), "local desktop JPEG");
        for i in 0..3 {
            let (vh, data) = next_video(&mut v).await;
            assert_eq!(vh.codec, k2_core::sim::stream::VIDEO_CODEC_VP8);
            assert_eq!(vh.codec, 2);
            assert_eq!(data, b"vp8");
            assert_eq!(vh.key, i == 0, "key first, then deltas");
        }
        for _ in 0..3 {
            let m = next_binary(&mut h, Duration::from_secs(5)).await.expect("JPEG frames");
            assert!(VideoHeader::decode(&m).is_none(), "never K2SV to an H.264-only viewer");
            let (fh, jpeg) = FrameHeader::decode(&m).expect("K2SF");
            assert_eq!(jpeg, b"jpeg");
            ack(&mut h, fh.seq).await;
        }
        let asked = ctl.requests.lock().unwrap().clone();
        let codecs: std::collections::BTreeSet<&str> = asked.iter().flat_map(|r| match r {
            Request::Layers { layers } => layers.iter().filter(|l| l.on).map(|l| l.codec.as_str()).collect::<Vec<_>>(),
            _ => Vec::new(),
        }).collect();
        assert_eq!(codecs.into_iter().collect::<Vec<_>>(), vec!["jpeg", "vp8"], "{asked:?}");
        drop((v, h));
        ends(s1).await;
        ends(s2).await;
    });
}

/// (f) deskd switches display: the new display's first unit is a key +
/// source start (its seqs start over); the viewer's gate takes it, the
/// wire seq keeps rising, deltas follow it, and wpt / hpt follow the new
/// display.
#[test]
fn a_source_change_arrives_as_a_source_start_key_and_the_gate_resyncs() {
    rt().block_on(async {
        let ctl = Ctl::new();
        let (mut c, server) = open(desktop_source("f", vec![Codec::Vp8], ctl.clone()), &[("codecs", "vp8"), ("win", "3")], hooks(Default::default(), false, Rec::default())).await;
        let _ = next_text(&mut c).await;
        let mut last = 0;
        for _ in 0..3 {
            let (h, _) = next_video(&mut c).await;
            assert_eq!(h.width_pt, 1280);
            last = h.seq;
        }
        ctl.display.store(2, Ordering::SeqCst);
        let switched = loop {
            let (h, _) = next_video(&mut c).await;
            assert!(h.seq > last, "wire seqs rise: {} after {last}", h.seq);
            last = h.seq;
            if h.width_pt == 1920 {
                break h;
            }
        };
        assert!(switched.key, "the new display starts on a key");
        assert_eq!((switched.width_px, switched.height_px, switched.height_pt), (1920, 1080, 1080));
        let mut prev = switched.seq;
        for _ in 0..3 {
            let (h, _) = next_video(&mut c).await;
            assert!(!h.key && h.seq == prev + 1 && h.width_pt == 1920, "deltas on the new chain: {h:?} after {prev}");
            prev = h.seq;
        }
        drop(c);
        ends(server).await;
    });
}
