//! `/cli/sim/stream` tests: the shared hub, the per-viewer H.264 pipe
//! (with the fake encoder, so they run on Linux), and real sockets for the
//! old JPEG wire, the negotiated video wire, keyframes, the ack window and
//! the JPEG fallback.

use super::*;
use k2_core::sim::lane::{FrameData, Orientation};
use k2_core::sim::video::annexb::{codec_string, is_keyframe};
use k2_core::sim::video::fake;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The rate table, by codec and path (fix 2, M-4): JPEG ≤ 30 here and
/// 2–15 elsewhere; H.264 ≤ 60 here, 2–60 on the LAN (default 60), 2–30
/// over Connect (default 24). `K2_SIM_VIDEO_MAX_FPS=30` puts local and LAN
/// H.264 back to 30.
#[test]
fn fps_caps_follow_codec_and_local_or_remote() {
    use ViewerPath::{Lan, Local, Tunnel};
    assert_eq!(clamp_fps(Codec::Jpeg, Local, None), 30);
    assert_eq!(clamp_fps(Codec::Jpeg, Local, Some(60)), 30, "the tab asks 60; JPEG gets 30");
    for p in [Lan, Tunnel] {
        assert_eq!(clamp_fps(Codec::Jpeg, p, None), 8);
        assert_eq!(clamp_fps(Codec::Jpeg, p, Some(1)), 2);
        assert_eq!(clamp_fps(Codec::Jpeg, p, Some(30)), 15);
    }
    assert_eq!(clamp_fps(Codec::H264, Local, Some(60)), 60, "local video at 60");
    assert_eq!(clamp_fps(Codec::H264, Local, None), 60);
    assert_eq!(clamp_fps(Codec::H264, Local, Some(99)), 60);
    assert_eq!(clamp_fps(Codec::H264, Lan, None), 60, "LAN video defaults to 60");
    assert_eq!(clamp_fps(Codec::H264, Lan, Some(60)), 60);
    assert_eq!(clamp_fps(Codec::H264, Lan, Some(1)), 2);
    assert_eq!(clamp_fps(Codec::H264, Tunnel, None), 24);
    assert_eq!(clamp_fps(Codec::H264, Tunnel, Some(30)), 30, "video may go faster remotely");
    assert_eq!(clamp_fps(Codec::H264, Tunnel, Some(60)), 30, "Connect stays at 30");
    // The rollback cap.
    assert_eq!(clamp_fps_with(Codec::H264, Local, Some(60), 30), 30);
    assert_eq!(clamp_fps_with(Codec::H264, Lan, None, 30), 30);
    assert_eq!(clamp_fps_with(Codec::H264, Tunnel, None, 30), 24);
}

#[test]
fn viewer_messages_map_to_point_actions_and_bad_ones_are_dropped() {
    assert!(matches!(to_action(&ViewerMsg::Tap { x: 1.0, y: 2.0 }), Some(Action::Tap { .. })));
    assert!(to_action(&ViewerMsg::OpenUrl { url: "no scheme".into() }).is_none());
    assert!(to_action(&ViewerMsg::Text { text: "x".repeat(5000) }).is_none());
    assert!(to_action(&ViewerMsg::Text { text: "x".repeat(TEXT_MAX_CHARS + 1) }).is_none());
    assert!(to_action(&ViewerMsg::Paste { text: "x".repeat(PASTE_MAX_BYTES + 1) }).is_none());
    assert!(to_action(&ViewerMsg::Ack { seq: 1 }).is_none());
    assert!(to_action(&ViewerMsg::Keyframe).is_none(), "a keyframe ask is not device input");
}

fn frame(w: u32, h: u32, shade: u8) -> Frame {
    Frame {
        width_px: w,
        height_px: h,
        scale: 1.0,
        orientation: Orientation::Portrait,
        seq: 1,
        captured_at: Instant::now(),
        stamps: Default::default(),
        data: FrameData::Rgb888(vec![shade; (w * h * 3) as usize]),
    }
}

fn job(seq: u32, f: Frame, force_key: bool) -> VideoJob {
    VideoJob { shared: Arc::new(Shared::new(seq, f)), force_key, fps: 30, bitrate: 500_000, ts_ms: seq * 33, stats: false }
}

/// The pipe opens an encoder on the first frame (a keyframe), sends deltas,
/// forces a keyframe on request, and reopens (keyframe) when the size
/// changes, as a rotation does. Every message is a K2SV header + Annex B.
#[test]
fn the_video_pipe_keys_on_join_on_request_and_on_rotation() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let log = fake::install();
    let mut enc: Option<Box<dyn VideoEncoder>> = None;
    let mut out = Vec::new();
    for (i, (w, h, key)) in [(4, 8, false), (4, 8, false), (4, 8, true), (4, 8, false), (8, 4, false), (8, 4, false)].into_iter().enumerate() {
        let (k, msg) = encode_job(&mut enc, &job(i as u32 + 1, frame(w, h, i as u8), key)).expect("encode").expect("not dropped");
        let (hd, au) = VideoHeader::decode(&msg).expect("a K2SV message");
        assert_eq!(hd.key, k);
        assert_eq!(hd.key, is_keyframe(au), "the flag matches the bitstream");
        assert_eq!((hd.seq, hd.width_px, hd.height_px, hd.width_pt, hd.height_pt), (i as u32 + 1, w as u16, h as u16, w as u16, h as u16));
        assert_eq!(hd.codec, VIDEO_CODEC_H264);
        assert_eq!(hd.ts_ms, (i as u32 + 1) * 33);
        if hd.key {
            assert_eq!(codec_string(au).as_deref(), Some("avc1.42E01F"), "SPS in band");
        }
        out.push(hd.key);
    }
    fake::uninstall();
    assert_eq!(out, vec![true, false, true, false, true, false], "join, request, rotation");
    assert_eq!(log.lock().expect("log").opened, vec![(4, 8), (8, 4)], "one encoder per size");
}

/// A failing encoder ends the worker after one `Failed` (the viewer falls
/// back to JPEG); rate-control drops come back as `Dropped`.
#[test]
fn the_video_worker_reports_failure_once_and_stops() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fn failing(_: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
        Err("no encoder here (test)".into())
    }
    video::set_h264_factory(Some(failing));
    let (jobs_tx, jobs_rx) = std::sync::mpsc::sync_channel::<VideoJob>(1);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<VideoOut>(2);
    let worker = std::thread::spawn(move || video_worker(jobs_rx, out_tx));
    jobs_tx.send(job(7, frame(4, 8, 1), false)).expect("job");
    match out_rx.blocking_recv() {
        Some(VideoOut::Failed { seq, why }) => {
            assert_eq!(seq, 7);
            assert!(why.contains("no encoder here"), "{why}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    worker.join().expect("worker ends after a failure");
    assert!(out_rx.blocking_recv().is_none(), "nothing after the failure");
    video::set_h264_factory(None);
}

struct Booted {
    ws: Ws,
    tmp: std::path::PathBuf,
    fake: Arc<k2_core::sim::fake::FakeLane>,
}

fn boot_fake(tag: &str) -> Booted {
    let tmp = std::env::temp_dir().join(format!("k2-sim-stream-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
    let fake_lane = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
    k2_core::sim::register_lane(fake_lane.clone());
    let m = manager::manager();
    m.refresh_probes();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, format!("stream-{tag}"), tmp.join("ws").display().to_string()])
            .expect("project");
    }
    let ws = Ws { id: id.clone(), handle: format!("stream-{tag}"), path: tmp.join("ws") };
    m.boot(&ws, &manager::Actor::Person { name: "owner".into() }, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
    Booted { ws, tmp, fake: fake_lane }
}

fn shut_down(b: Booted) {
    manager::manager().stop(&b.ws, &manager::Actor::Person { name: "owner".into() }, "test").expect("stop");
    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&b.tmp);
}

/// Two viewers share one hub. JPEG is encoded lazily (fix 2, M-3): the hub
/// encodes nothing; the first viewer that sends a frame encodes it, and a
/// second viewer sending the same frame reuses it (one encode per frame
/// however many viewers); a frame no viewer sends is never encoded. A
/// viewer that never reads holds nobody back (the channel keeps only the
/// newest).
#[test]
fn one_encoder_serves_every_viewer() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("hub");
    let (sim_a, hub_a) = join(&b.ws).expect("join a");
    let (sim_b, hub_b) = join(&b.ws).expect("join b");
    assert_eq!(sim_a, sim_b);
    assert!(Arc::ptr_eq(&hub_a, &hub_b), "one hub per device");
    hub_a.want_fps.lock().expect("fps").insert(1, 30);
    let mut fast = hub_a.rx.clone();
    let _slow = hub_b.rx.clone(); // never reads
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("rt");
    let mut got: Vec<Arc<Shared>> = Vec::new();
    rt.block_on(async {
        while got.len() < 5 {
            tokio::time::timeout(Duration::from_secs(5), fast.changed()).await.expect("a frame within 5 s").expect("hub alive");
            if let Pub::Frame(f) = &*fast.borrow_and_update() {
                got.push(f.clone());
            }
        }
    });
    let seqs: Vec<u32> = got.iter().map(|f| f.seq).collect();
    assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
    assert!(got.iter().all(|f| f.jpeg_ready().is_none()), "the hub encodes nothing itself");
    assert_eq!(encodes(sim_a), Some(0));
    // Two viewers send the same frame: one encode, the same bytes.
    let first = got[2].jpeg_message(&hub_a.encodes).expect("encode").to_vec();
    let second = got[2].jpeg_message(&hub_b.encodes).expect("reuse").to_vec();
    assert_eq!(first, second);
    let (h, jpeg) = FrameHeader::decode(&first).expect("header");
    assert_eq!((h.width_px, h.height_px, h.width_pt, h.height_pt), (4, 8, 4, 8));
    assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    assert_eq!(encodes(sim_a), Some(1), "one encode for two viewers");
    // The frames nobody sent were never encoded.
    assert!(got.iter().enumerate().all(|(i, f)| i == 2 || f.jpeg_ready().is_none()), "a frame no viewer sends is never encoded");
    assert_eq!(encodes(sim_a), Some(1));
    drop(fast);
    drop(_slow);
    drop(hub_a);
    drop(hub_b);
    shut_down(b);
}

/// Fix 1a: the hub asks the lane for points-sized frames, and retunes the
/// source once each time the fastest viewer's rate changes: up when a
/// faster viewer joins, back down when it leaves.
#[test]
fn the_hub_asks_for_points_and_retunes_to_the_fastest_viewer() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("retune");
    let (_, hub) = join(&b.ws).expect("join");
    let opts = b.fake.frame_opts.clone();
    let rx = hub.rx.clone(); // a viewer, so the hub keeps running
    let wait_for = |n: usize| {
        let t0 = Instant::now();
        while opts.lock().len() < n {
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", opts.lock());
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait_for(1);
    assert_eq!(opts.lock()[0], ("open", FrameOpts::points(LOCAL_MAX_FPS)), "opened at points");
    hub.want_fps.lock().expect("fps").insert(1, 15);
    wait_for(2);
    hub.want_fps.lock().expect("fps").insert(2, 30);
    wait_for(3);
    std::thread::sleep(Duration::from_millis(200));
    hub.want_fps.lock().expect("fps").remove(&2);
    wait_for(4);
    std::thread::sleep(Duration::from_millis(200));
    let got: Vec<(&str, u32, FrameSize)> = opts.lock().iter().map(|(k, o)| (*k, o.max_fps, o.size)).collect();
    assert_eq!(
        got,
        vec![("open", 30, FrameSize::Points), ("retune", 15, FrameSize::Points), ("retune", 30, FrameSize::Points), ("retune", 15, FrameSize::Points)],
        "one retune per change of the fastest viewer"
    );
    drop(rx);
    drop(hub);
    shut_down(b);
}

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Serve one viewer on a real loopback socket (as the dispatcher would,
/// with the owner token) and connect a client to it.
async fn open(b: &Booted, extra: &[(&str, &str)]) -> (Client, tokio::task::JoinHandle<()>) {
    open_as(b, ViewerPath::Local, extra).await
}

/// [`open`] as a viewer on `path` (the dispatcher's ingress).
async fn open_as(b: &Booted, path: ViewerPath, extra: &[(&str, &str)]) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let mut params: HashMap<String, String> = HashMap::new();
    params.insert("workspace".into(), b.ws.path.display().to_string());
    params.insert("fps".into(), "30".into());
    for (k, v) in extra {
        params.insert((*k).into(), (*v).into());
    }
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        serve(&mut s, params, Viewer { caller: Caller::Owner, token: "owner-tok".into(), path }, "owner-tok".into()).await;
    });
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/cli/sim/stream")).await.expect("connect");
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

/// A viewer that offers nothing (every client before 0.46.7) gets the old
/// wire: hello says JPEG and a window of 1, frames are K2SF, and the next
/// frame waits for the ack.
#[test]
fn an_old_viewer_gets_jpeg_with_one_frame_in_flight() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("old");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[]).await;
        let hello = next_text(&mut c).await;
        assert_eq!(hello["t"], "hello");
        assert_eq!(hello["codec"], "jpeg");
        assert_eq!(hello["window"], 1);
        let first = next_binary(&mut c, Duration::from_secs(5)).await.expect("a first frame");
        let (h1, jpeg) = FrameHeader::decode(&first).expect("K2SF");
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        assert!(next_binary(&mut c, Duration::from_millis(400)).await.is_none(), "no second frame before the ack");
        ack(&mut c, h1.seq).await;
        let second = next_binary(&mut c, Duration::from_secs(5)).await.expect("the next frame after the ack");
        let (h2, _) = FrameHeader::decode(&second).expect("K2SF");
        assert!(h2.seq > h1.seq, "the newest frame, {} > {}", h2.seq, h1.seq);
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("the server ends when the viewer leaves").expect("join");
    });
    shut_down(b);
}

/// M0: a viewer that asks with `stats=1` gets a `stats` message about once
/// a second with its frames' stage times and the daemon-side time of each
/// touch that carried an id. Nothing else on the wire changes.
#[test]
fn a_stats_viewer_gets_stage_times_and_its_probed_touches() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("stats");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("stats", "1")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!(hello["t"], "hello");
        c.send(Message::Text(r#"{"t":"touch","phase":"down","x":10,"y":20,"id":41}"#.into())).await.expect("down");
        c.send(Message::Text(r#"{"t":"touch","phase":"up","x":10,"y":20,"id":42}"#.into())).await.expect("up");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let (mut frames, mut ids, mut stages) = (0u64, Vec::new(), std::collections::BTreeSet::new());
        while tokio::time::Instant::now() < deadline && (frames == 0 || ids.len() < 2 || !stages.contains("total")) {
            let m = tokio::time::timeout_at(deadline, c.next()).await.expect("messages keep coming").expect("open").expect("ok");
            match m {
                Message::Binary(bin) => {
                    let (h, _) = FrameHeader::decode(&bin).expect("K2SF");
                    ack(&mut c, h.seq).await;
                }
                Message::Text(t) => {
                    let v: serde_json::Value = serde_json::from_str(&t).expect("JSON");
                    if v["t"] != "stats" {
                        continue;
                    }
                    let st = &v["stats"];
                    frames += st["frames"].as_u64().expect("frames");
                    assert!(st["hubCpuMs"].is_number() && st["encoderCpuMs"].is_number(), "{st}");
                    for s in st["stages"].as_array().expect("stages") {
                        let (p50, p95, max) = (s["p50"].as_f64().expect("p50"), s["p95"].as_f64().expect("p95"), s["max"].as_f64().expect("max"));
                        assert!(p50 <= p95 && p95 <= max, "{s}");
                        stages.insert(s["stage"].as_str().expect("name").to_string());
                    }
                    for i in st["inputs"].as_array().expect("inputs") {
                        assert!(i["laneMs"].as_f64().expect("laneMs") >= 0.0);
                        ids.push(i["id"].as_u64().expect("id"));
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(frames > 0, "stats count the frames sent");
        assert_eq!(ids, vec![41, 42], "each probed touch, in order");
        for want in ["receiveHub", "hubGate", "encode", "send", "total"] {
            assert!(stages.contains(want), "{want} in {stages:?}");
        }
        assert!(!stages.contains("captureEmit"), "the fake lane has no helper times: {stages:?}");
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    shut_down(b);
}

/// A viewer that offers H.264 on a host that has it: hello says h264 and the
/// window; frames are K2SV, the first a keyframe with SPS + PPS; up to 3 go
/// out before an ack; a keyframe ask brings one.
#[test]
fn a_video_viewer_gets_keyframe_first_a_window_and_keyframes_on_request() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let log = fake::install();
    let b = boot_fake("video");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["codec"].as_str(), hello["window"].as_u64(), hello["fps"].as_u64()), (Some("h264"), Some(3), Some(30)));
        let mut heads = Vec::new();
        for _ in 0..3 {
            let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("a frame inside the window");
            let (h, au) = VideoHeader::decode(&m).expect("K2SV");
            assert_eq!(h.key, is_keyframe(au));
            heads.push(h);
        }
        assert!(heads[0].key, "the first frame is a keyframe");
        assert!(!heads[1].key && !heads[2].key, "then deltas");
        assert!(heads.windows(2).all(|w| w[1].seq > w[0].seq));
        assert!(next_binary(&mut c, Duration::from_millis(400)).await.is_none(), "the window is full: nothing until an ack");
        // One cumulative ack frees the window.
        ack(&mut c, heads[2].seq).await;
        let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("a frame after the ack");
        let (h4, _) = VideoHeader::decode(&m).expect("K2SV");
        assert!(!h4.key);
        ack(&mut c, h4.seq).await;
        c.send(Message::Text(r#"{"t":"keyframe"}"#.into())).await.expect("ask");
        let mut keyed = false;
        for _ in 0..10 {
            let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("frames keep coming");
            let (h, _) = VideoHeader::decode(&m).expect("K2SV");
            ack(&mut c, h.seq).await;
            if h.key {
                keyed = true;
                break;
            }
        }
        assert!(keyed, "a keyframe after the ask");
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    fake::uninstall();
    let l = log.lock().expect("log").clone();
    assert_eq!(l.opened, vec![(4, 8)], "one encoder for this viewer");
    assert_eq!(l.frames.iter().filter(|(_, k)| *k).count(), 2, "join + ask: {:?}", l.frames);
    shut_down(b);
}

/// Fix 2: the tab asks 60. A local H.264 viewer gets 60; a JPEG one gets 30
/// and is told so in `hello.fps`; a LAN H.264 viewer that asks nothing gets
/// 60 and `path: lan` (still `remote`, so the tab keeps its remote UI).
#[test]
fn local_and_lan_video_get_60_fps_and_jpeg_gets_30() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("rates");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3"), ("fps", "60")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["codec"].as_str(), hello["fps"].as_u64(), hello["path"].as_str()), (Some("h264"), Some(60), Some("local")));
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");

        let (mut c, server) = open(&b, &[("fps", "60")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["codec"].as_str(), hello["fps"].as_u64()), (Some("jpeg"), Some(30)));
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");

        // `open` sends fps=30 unless told otherwise; an unreadable fps is no ask.
        let (mut c, server) = open_as(&b, ViewerPath::Lan, &[("codecs", "h264,jpeg"), ("win", "3"), ("fps", "")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["fps"].as_u64(), hello["path"].as_str(), hello["remote"].as_bool()), (Some(60), Some("lan"), Some(true)));
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    fake::uninstall();
    shut_down(b);
}

/// When the encoder fails the same socket says `codec: jpeg` and carries on
/// with K2SF frames.
#[test]
fn a_failed_encoder_falls_back_to_jpeg_on_the_same_socket() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fn failing(_: &EncoderConfig) -> Result<Box<dyn VideoEncoder>, String> {
        Err("no encoder here (test)".into())
    }
    video::set_h264_factory(Some(failing));
    let b = boot_fake("fallback");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("codecs", "h264,jpeg"), ("win", "2")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!(hello["codec"], "h264");
        let switched = next_text(&mut c).await;
        assert_eq!(switched["t"], "codec");
        assert_eq!(switched["codec"], "jpeg");
        assert!(switched["message"].as_str().is_some_and(|m| m.contains("no encoder here")), "{switched}");
        let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("JPEG frames follow");
        assert!(FrameHeader::decode(&m).is_some(), "K2SF after the fallback");
        assert!(VideoHeader::decode(&m).is_none());
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    video::set_h264_factory(None);
    shut_down(b);
}

/// A host with no H.264 (Linux today) answers a video offer with JPEG.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_host_without_h264_answers_a_video_offer_with_jpeg() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    video::set_h264_factory(None);
    let b = boot_fake("nohw");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["codec"].as_str(), hello["window"].as_u64()), (Some("jpeg"), Some(3)));
        let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("a frame");
        assert!(FrameHeader::decode(&m).is_some());
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    shut_down(b);
}

/// A person's keyboard: typing is key events (no read-back), ⌘V is a
/// paste, Shift/Option ride the key, ⌘A/⌘Z are edit chords.
#[test]
fn keyboard_messages_map_to_keyboard_actions() {
    use k2_core::sim::lane::{EditOp, Key};
    match to_action(&ViewerMsg::Text { text: "日本".into() }) {
        Some(Action::Keystrokes { text }) => assert_eq!(text.expose(), "日本"),
        other => panic!("text → keystrokes, got {other:?}"),
    }
    match to_action(&ViewerMsg::Paste { text: "a\nb".into() }) {
        Some(Action::Paste { text }) => assert_eq!(text.expose(), "a\nb"),
        other => panic!("paste → paste, got {other:?}"),
    }
    assert_eq!(
        to_action(&ViewerMsg::Key { key: Key::ArrowLeft, shift: true, alt: false }),
        Some(Action::Key { key: Key::ArrowLeft, mods: KeyMods { shift: true, alt: false } })
    );
    assert_eq!(to_action(&ViewerMsg::Edit { op: EditOp::Undo }), Some(Action::Edit { op: EditOp::Undo }));
    assert!(to_action(&ViewerMsg::Text { text: "a\u{1b}".into() }).is_none(), "control characters are refused");
}

/// Keyboard actions reach the device lane the same way touch does (the
/// fake lane records them, never the text itself).
#[test]
fn keyboard_input_reaches_the_lane_through_person_input() {
    use k2_core::sim::lane::{EditOp, Key};
    let _serial = crate::sim_routes::sim_test_lock();
    let tmp = std::env::temp_dir().join(format!("k2-sim-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
    let fake = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
    k2_core::sim::register_lane(fake.clone());
    let m = manager::manager();
    m.refresh_probes();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'keys', ?2)", rusqlite::params![id, tmp.join("ws").display().to_string()])
            .expect("project");
    }
    let ws = Ws { id: id.clone(), handle: "keys".into(), path: tmp.join("ws") };
    m.boot(&ws, &manager::Actor::Person { name: "owner".into() }, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
    let owner = Caller::Owner;
    person_input(&ws, "owner", &owner, ViewerMsg::Text { text: "hé".into() }).expect("text");
    person_input(&ws, "owner", &owner, ViewerMsg::Key { key: Key::ArrowRight, shift: true, alt: false }).expect("key");
    person_input(&ws, "owner", &owner, ViewerMsg::Edit { op: EditOp::SelectAll }).expect("edit");
    person_input(&ws, "owner", &owner, ViewerMsg::Paste { text: "pasted secret".into() }).expect("paste");
    let e = person_input(&ws, "owner", &owner, ViewerMsg::Paste { text: "x".repeat(PASTE_MAX_BYTES + 1) }).expect_err("too big");
    assert_eq!(e.code, "sim_invalid");
    let acts = fake.actions.lock().clone();
    let tail: Vec<&str> = acts.iter().rev().take(4).rev().map(String::as_str).collect();
    assert_eq!(tail, vec!["keys 2", "key ArrowRight+shift", "edit SelectAll", "paste 13"]);
    assert!(!acts.iter().any(|a| a.contains("secret")), "{acts:?}");
    m.stop(&ws, &manager::Actor::Person { name: "owner".into() }, "test").expect("stop");
    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}

fn touch(phase: TouchPhase, x2: Option<f32>, y2: Option<f32>) -> ViewerMsg {
    ViewerMsg::Touch { phase, x: 10.0, y: 20.0, x2, y2, vx: None, vy: None, id: None }
}

#[test]
fn a_second_finger_is_a_two_finger_touch_and_half_a_pair_is_dropped() {
    assert_eq!(
        to_action(&touch(TouchPhase::Move, Some(30.0), Some(40.0))),
        Some(Action::TwoTouch { phase: TouchPhase::Move, a: Point { x: 10.0, y: 20.0 }, b: Point { x: 30.0, y: 40.0 } })
    );
    assert_eq!(to_action(&touch(TouchPhase::Down, None, None)), Some(Action::Touch { phase: TouchPhase::Down, at: Point { x: 10.0, y: 20.0 } }));
    assert!(to_action(&touch(TouchPhase::Move, Some(30.0), None)).is_none(), "never one finger for a pinch");
}

/// The fake lane end to end: a flick's `up` with a velocity replays the
/// tail of moves before lifting; a pinch reaches an Android lane as two
/// fingers and is refused (not sent as one) on iOS; hello reports it.
#[test]
fn fling_tail_and_pinch_reach_the_fake_lane() {
    let _serial = crate::sim_routes::sim_test_lock();
    let tmp = std::env::temp_dir().join(format!("k2-sim-gesture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).expect("ws");
    k2_core::sim::set_base_for_tests(tmp.join("home"));
    k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
    let fake = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
    k2_core::sim::register_lane(fake.clone());
    let m = manager::manager();
    m.refresh_probes();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'gesture', ?2)", rusqlite::params![id, tmp.join("ws").display().to_string()])
            .expect("project");
    }
    let ws = Ws { id: id.clone(), handle: "gesture".into(), path: tmp.join("ws") };
    let owner = manager::Actor::Person { name: "owner".into() };
    m.boot(&ws, &owner, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
    assert_eq!(m.input_caps(k2_core::sim::LaneKind::Android), InputCaps { multi_touch: true, fling: true });
    fake.actions.lock().clear();

    let fling = ViewerMsg::Touch { phase: TouchPhase::Up, x: 200.0, y: 600.0, x2: None, y2: None, vx: Some(0.0), vy: Some(-2000.0), id: None };
    person_input(&ws, "owner", &Caller::Owner, fling).expect("fling");
    let got = fake.actions.lock().clone();
    assert_eq!(got.len(), 5, "{got:?}");
    assert!(got[..4].iter().all(|a| a.starts_with("touch Move 200,")), "{got:?}");
    assert!(got[4].starts_with("touch Up 200,53"), "lifts at the tail's end: {got:?}");

    fake.actions.lock().clear();
    let slow = ViewerMsg::Touch { phase: TouchPhase::Up, x: 200.0, y: 600.0, x2: None, y2: None, vx: Some(0.0), vy: Some(-5.0), id: None };
    person_input(&ws, "owner", &Caller::Owner, slow).expect("slow");
    assert_eq!(fake.actions.lock().clone(), vec!["touch Up 200,600".to_string()]);

    fake.actions.lock().clear();
    let pinch = ViewerMsg::Touch { phase: TouchPhase::Down, x: 150.0, y: 400.0, x2: Some(250.0), y2: Some(400.0), vx: None, vy: None, id: None };
    person_input(&ws, "owner", &Caller::Owner, pinch).expect("pinch");
    assert_eq!(fake.actions.lock().clone(), vec!["two-touch Down 150,400 250,400".to_string()]);

    m.stop(&ws, &owner, "test").expect("stop");
    crate::sim_routes::clear_lanes_for_tests();

    // iOS takes one finger only: the pinch is refused, nothing is sent.
    let ios = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Ios));
    k2_core::sim::register_lane(ios.clone());
    m.refresh_probes();
    m.boot(&ws, &owner, &k2_core::sim::api::BootReq { platform: Some("ios".into()), no_tab: true, ..Default::default() }).expect("boot ios");
    assert!(!m.input_caps(k2_core::sim::LaneKind::Ios).multi_touch);
    ios.actions.lock().clear();
    let pinch = ViewerMsg::Touch { phase: TouchPhase::Down, x: 150.0, y: 400.0, x2: Some(250.0), y2: Some(400.0), vx: None, vy: None, id: None };
    let err = person_input(&ws, "owner", &Caller::Owner, pinch).expect_err("iOS refuses a pinch");
    assert!(err.message.contains("Pinch isn't available"), "{}", err.message);
    assert!(ios.actions.lock().is_empty(), "nothing reached the device");
    m.stop(&ws, &owner, "test").expect("stop ios");
    crate::sim_routes::clear_lanes_for_tests();
    let _ = std::fs::remove_dir_all(&tmp);
}
