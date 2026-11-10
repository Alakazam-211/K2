//! `/cli/sim/stream` tests: the stream engine's hub on the fake lane
//! (layers, keyframes, pane widths; the fake H.264 encoder, so they run on
//! Linux), and real sockets for the old JPEG wire, the negotiated video
//! wire, keyframes, the ack window, skips to keyframes and the JPEG
//! fallback. The pixel adapter's own tests (schedule, sizes, encoder keys
//! and failure) are in `k2_core::stream::pixel`.

use super::*;
use k2_core::sim::video::annexb::is_keyframe;
use k2_core::sim::video::fake;
use k2_core::sim::video::{EncoderConfig, VideoEncoder};
use std::sync::Arc;
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

fn want(codec: UnitCodec, fps: u32) -> Want {
    Want { codec, fps, path: ViewerPath::Local, pw: None, stats: false }
}

/// The next unit off a subscription (5 s at most).
fn next_unit(sub: &mut Subscription) -> EncodedUnit {
    let t0 = Instant::now();
    loop {
        match sub.units.try_recv() {
            Ok(u) => return u,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                assert!(t0.elapsed() < Duration::from_secs(5), "a unit within 5 s");
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("{e:?}"),
        }
    }
}

/// The next key unit, skipping deltas.
fn next_key(sub: &mut Subscription) -> EncodedUnit {
    loop {
        let u = next_unit(sub);
        if u.key {
            return u;
        }
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Two JPEG viewers at one rate share one hub and one layer: each unit is
/// encoded once and both get the same bytes (one shared buffer). A viewer
/// that never reads holds nobody back (its own queue just fills).
#[test]
fn one_encode_per_layer_serves_every_viewer() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("hub");
    let (sim_a, mut a) = join(&b.ws, want(UnitCodec::Jpeg, 30)).expect("join a");
    let (sim_b, mut c) = join(&b.ws, want(UnitCodec::Jpeg, 30)).expect("join b");
    assert_eq!(sim_a, sim_b);
    assert!(Arc::ptr_eq(a.hub(), c.hub()), "one hub per device");
    assert_eq!(a.layer(), c.layer(), "one layer");
    let got: Vec<EncodedUnit> = (0..20).map(|_| next_unit(&mut a)).collect();
    assert!(got.windows(2).all(|w| w[1].seq == w[0].seq + 1), "a reader gets every unit");
    assert!(got.iter().all(|u| u.key && u.codec == UnitCodec::Jpeg && (u.width, u.height, u.width_pt, u.height_pt) == (4, 8, 4, 8)));
    assert_eq!(&got[0].data[..2], &[0xff, 0xd8]);
    // The other viewer never read: its queue holds the first units only.
    let mut theirs = Vec::new();
    while let Ok(u) = c.units.try_recv() {
        theirs.push(u);
    }
    assert!(!theirs.is_empty() && theirs.len() <= crate::stream::VIEWER_QUEUE, "{}", theirs.len());
    let mut shared = 0;
    for u in &theirs {
        if let Some(m) = got.iter().find(|g| g.seq == u.seq) {
            assert_eq!(m.data.as_ptr(), u.data.as_ptr(), "the same encode, not a copy");
            shared += 1;
        }
    }
    assert!(shared > 0);
    assert_eq!(a.hub().layer_specs().len(), 1);
    drop((a, c));
    shut_down(b);
}

/// Fix 1a: the hub asks the lane for points-sized frames, and retunes the
/// source once each time the fastest layer's rate changes: up when a
/// faster viewer joins, back down when it leaves.
#[test]
fn the_hub_asks_for_points_and_retunes_to_the_fastest_layer() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("retune");
    let opts = b.fake.frame_opts.clone();
    let wait_for = |n: usize| wait_until("retunes", || opts.lock().len() >= n);
    let (_, slow) = join(&b.ws, want(UnitCodec::Jpeg, 15)).expect("join");
    wait_for(2);
    assert_eq!(opts.lock()[0], ("open", FrameOpts::points(LOCAL_MAX_FPS)), "opened at points");
    let (_, fast) = join(&b.ws, want(UnitCodec::Jpeg, 30)).expect("join");
    wait_for(3);
    std::thread::sleep(Duration::from_millis(200));
    drop(fast);
    wait_for(4);
    std::thread::sleep(Duration::from_millis(200));
    let got: Vec<(&str, u32, FrameSize)> = opts.lock().iter().map(|(k, o)| (*k, o.max_fps, o.size)).collect();
    assert_eq!(
        got,
        vec![("open", 30, FrameSize::Points), ("retune", 15, FrameSize::Points), ("retune", 30, FrameSize::Points), ("retune", 15, FrameSize::Points)],
        "one retune per change of the fastest layer"
    );
    drop(slow);
    shut_down(b);
}

/// (a) A viewer joining a running layer gets a key unit first, and its
/// keyframe ask is never throttled, even right after another viewer's
/// ask (which is: a second ask inside 250 ms is merged).
#[test]
fn a_joiner_gets_a_key_first_and_its_ask_is_never_throttled() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("joiner");
    let (_, mut a) = join(&b.ws, want(UnitCodec::H264, 30)).expect("join a");
    let first = next_unit(&mut a);
    assert!(first.key && first.source_start, "a new layer starts with a key unit");
    assert!(!next_unit(&mut a).key && !next_unit(&mut a).key, "then deltas");
    let hub = a.hub().clone();
    let asks = hub.key_asks();
    assert!(a.ask_keyframe(), "a viewer's ask");
    assert!(!a.ask_keyframe(), "a second ask inside 250 ms is merged");
    let (_, mut j) = join(&b.ws, want(UnitCodec::H264, 30)).expect("join mid-stream");
    assert_eq!(j.layer(), a.layer(), "the running layer");
    assert_eq!(hub.key_asks(), asks + 2, "the joiner's ask went through right after the other one");
    // Units already on their way may come first (the viewer's gate drops
    // them); the key is the next encode once the ask reaches the source.
    let mut before = 0;
    let mine = loop {
        let u = next_unit(&mut j);
        if u.key {
            break u;
        }
        before += 1;
    };
    assert!(before <= 2, "{before} deltas before the joiner's key");
    // The viewer already there gets that key too, in its order.
    let k = next_key(&mut a);
    assert!(k.seq <= mine.seq, "{} > {}", k.seq, mine.seq);
    fake::uninstall();
    drop((a, j));
    shut_down(b);
}

/// Count what a socket viewer gets for `dur`, acking each frame at once.
async fn take_video(c: &mut Client, dur: Duration) -> Vec<VideoHeader> {
    let deadline = tokio::time::Instant::now() + dur;
    let mut heads = Vec::new();
    loop {
        let m = match tokio::time::timeout_at(deadline, c.next()).await {
            Err(_) => return heads,
            Ok(m) => m.expect("open").expect("ok"),
        };
        if let Message::Binary(bin) = m {
            let (h, au) = VideoHeader::decode(&bin).expect("K2SV");
            assert_eq!(h.key, is_keyframe(au));
            ack(c, h.seq).await;
            heads.push(h);
        }
    }
}

/// (b) Two H.264 viewers at 60 and 15 fps: two layers, two encoders, each
/// viewer at about its tier. Three viewers at 60: one encoder.
#[test]
fn two_rates_are_two_layers_and_three_viewers_at_one_rate_are_one_encoder() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let log = fake::install();
    let b = boot_fake("tiers");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut fast, s1) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3"), ("fps", "60")]).await;
        let (mut slow, s2) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3"), ("fps", "15")]).await;
        assert_eq!(next_text(&mut fast).await["fps"], 60);
        assert_eq!(next_text(&mut slow).await["fps"], 15);
        let (f, s) = tokio::join!(take_video(&mut fast, Duration::from_secs(2)), take_video(&mut slow, Duration::from_secs(2)));
        assert!((90..=130).contains(&f.len()), "60 fps viewer: {} in 2 s", f.len());
        assert!((22..=36).contains(&s.len()), "15 fps viewer: {} in 2 s", s.len());
        for hs in [&f, &s] {
            assert!(hs[0].key, "each starts on a key unit");
            assert!(hs.windows(2).all(|w| w[1].key || w[1].seq == w[0].seq + 1), "no delta after a gap");
        }
        drop((fast, slow));
        for s in [s1, s2] {
            tokio::time::timeout(Duration::from_secs(5), s).await.expect("server ends").expect("join");
        }
    });
    assert_eq!(log.lock().expect("log").opened, vec![(4, 8), (4, 8)], "one encoder per layer");
    let log = fake::install();
    let mut subs: Vec<Subscription> = (0..3).map(|_| join(&b.ws, want(UnitCodec::H264, 60)).expect("join").1).collect();
    assert!(subs.windows(2).all(|w| w[0].layer() == w[1].layer()));
    for s in subs.iter_mut() {
        let _ = next_key(s);
    }
    assert_eq!(log.lock().expect("log").opened.len(), 1, "three viewers at 60: one encoder");
    fake::uninstall();
    drop(subs);
    shut_down(b);
}

/// (c) A JPEG viewer and an H.264 viewer at once: two layers, K2SF to one
/// and K2SV to the other.
#[test]
fn a_jpeg_viewer_and_a_video_viewer_watch_at_once() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("mixed");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut v, s1) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3")]).await;
        let (mut j, s2) = open(&b, &[]).await;
        let hello = next_text(&mut v).await;
        assert_eq!(hello["codec"], "h264");
        assert_eq!(next_text(&mut j).await["codec"], "jpeg");
        for _ in 0..3 {
            let m = next_binary(&mut v, Duration::from_secs(5)).await.expect("video");
            let (h, _) = VideoHeader::decode(&m).expect("K2SV");
            ack(&mut v, h.seq).await;
            let m = next_binary(&mut j, Duration::from_secs(5)).await.expect("jpeg");
            let (h, jpeg) = FrameHeader::decode(&m).expect("K2SF");
            assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
            ack(&mut j, h.seq).await;
        }
        let id: i64 = hello["sim"].as_str().and_then(|s| s.strip_prefix('s')).and_then(|n| n.parse().ok()).expect("sim id");
        let hub = crate::stream::hub_for(&SourceKey::Sim(id)).expect("hub");
        let mut codecs: Vec<UnitCodec> = hub.layer_specs().iter().map(|l| l.codec).collect();
        codecs.sort_by_key(|c| *c as u8);
        assert_eq!(codecs, vec![UnitCodec::H264, UnitCodec::Jpeg]);
        drop((v, j));
        for s in [s1, s2] {
            tokio::time::timeout(Duration::from_secs(5), s).await.expect("server ends").expect("join");
        }
    });
    fake::uninstall();
    shut_down(b);
}

/// (d) A viewer leaving pauses its layer: the list the source gets drops it.
#[test]
fn a_viewer_leaving_pauses_its_layer() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("pause");
    let (_, mut a) = join(&b.ws, want(UnitCodec::H264, 60)).expect("join a");
    let (_, mut c) = join(&b.ws, want(UnitCodec::H264, 15)).expect("join b");
    let _ = (next_unit(&mut a), next_unit(&mut c));
    let hub = a.hub().clone();
    wait_until("two layers", || hub.layer_specs().len() == 2);
    let gone = c.layer();
    drop(c);
    wait_until("the layer leaves the list", || hub.layer_specs().len() == 1);
    let left = hub.layer_specs();
    assert_eq!((left[0].id, left[0].fps), (a.layer(), 60));
    assert!(left.iter().all(|l| l.id != gone));
    // Nothing more is encoded for it: every unit now is the 60 fps layer's.
    for _ in 0..10 {
        assert_eq!(next_unit(&mut a).layer, a.layer());
    }
    fake::uninstall();
    drop(a);
    shut_down(b);
}

/// (e) Per layer, every delta a viewer gets is the one after the unit it
/// got before, else a key unit: a viewer that acks slowly now and then
/// skips to keyframes, and a fast one on the same layer keeps its rate.
#[test]
fn a_slow_acker_skips_to_keyframes_and_never_gets_a_delta_after_a_gap() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("skips");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut slow, s1) = open(&b, &[("codecs", "h264,jpeg"), ("win", "1"), ("fps", "60")]).await;
        let (mut fast, s2) = open(&b, &[("codecs", "h264,jpeg"), ("win", "3"), ("fps", "60")]).await;
        let _ = (next_text(&mut slow).await, next_text(&mut fast).await);
        let slow_run = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let mut heads = Vec::new();
            let delays = [5u64, 5, 5, 60];
            while let Ok(m) = tokio::time::timeout_at(deadline, slow.next()).await {
                if let Message::Binary(bin) = m.expect("open").expect("ok") {
                    let (h, _) = VideoHeader::decode(&bin).expect("K2SV");
                    tokio::time::sleep(Duration::from_millis(delays[heads.len() % delays.len()])).await;
                    ack(&mut slow, h.seq).await;
                    heads.push(h);
                }
            }
            heads
        };
        let (s, f) = tokio::join!(slow_run, take_video(&mut fast, Duration::from_secs(3)));
        for (who, hs) in [("slow", &s), ("fast", &f)] {
            assert!(hs[0].key, "{who} starts on a key unit");
            for w in hs.windows(2) {
                assert!(w[1].key || w[1].seq == w[0].seq + 1, "{who}: delta {} after {}", w[1].seq, w[0].seq);
            }
        }
        assert!(s.iter().skip(1).any(|h| h.key), "the slow viewer skipped to a keyframe at least once");
        assert!(s.windows(2).any(|w| !w[1].key), "and took deltas while it kept up");
        assert!(f.len() >= 120, "the fast viewer keeps its rate: {} in 3 s", f.len());
        drop((slow, fast));
        for s in [s1, s2] {
            tokio::time::timeout(Duration::from_secs(5), s).await.expect("server ends").expect("join");
        }
    });
    fake::uninstall();
    shut_down(b);
}

/// (f) At most three layers: a fourth rate joins the nearest higher-fps
/// layer of its codec and remote-ness. Only a codec with no layer at all
/// opens one past the cap.
#[test]
fn a_source_runs_at_most_three_layers() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _log = fake::install();
    let b = boot_fake("cap");
    let subs: Vec<Subscription> = [60, 30, 15].iter().map(|f| join(&b.ws, want(UnitCodec::H264, *f)).expect("join").1).collect();
    let hub = subs[0].hub().clone();
    let fps_of = |s: &Subscription| s.layer_spec().expect("spec").fps;
    assert_eq!(subs.iter().map(fps_of).collect::<Vec<_>>(), vec![60, 30, 15]);
    let (_, eight) = join(&b.ws, want(UnitCodec::H264, 8)).expect("join 8");
    assert_eq!(eight.layer(), subs[2].layer(), "8 fps joins the 15 fps layer");
    let (_, twenty) = join(&b.ws, want(UnitCodec::H264, 20)).expect("join 20");
    assert_eq!(twenty.layer(), subs[1].layer(), "20 rounds up to 30, which runs");
    wait_until("three layers", || hub.layer_specs().len() == 3);
    let (_, jpeg) = join(&b.ws, want(UnitCodec::Jpeg, 30)).expect("join jpeg");
    assert_eq!(jpeg.layer_spec().map(|l| l.codec), Some(UnitCodec::Jpeg), "never a codec it didn't negotiate");
    wait_until("a JPEG layer past the cap", || hub.layer_specs().len() == 4);
    fake::uninstall();
    drop((subs, eight, twenty, jpeg));
    shut_down(b);
}

/// (g) `pw`: a viewer's pane width makes its layer native pixels up to that
/// width, clamped to the device's own width (here 8 px; points are 4), with
/// a key + source start at the new size. A narrower pane on another layer
/// is scaled down. On the wire, K2SF keeps the points size beside it.
#[test]
fn a_pane_width_sizes_the_layer_up_to_native() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("pw");
    let (_, mut a) = join(&b.ws, want(UnitCodec::Jpeg, 30)).expect("join");
    assert_eq!((next_unit(&mut a).width, a.layer_spec().map(|l| l.max_width)), (4, Some(4)), "the points width until a viewer says");
    let (_, wide) = join(&b.ws, Want { pw: Some(99), ..want(UnitCodec::Jpeg, 30) }).expect("join pw");
    assert_eq!(wide.layer(), a.layer());
    let u = loop {
        let u = next_unit(&mut a);
        if u.width != 4 {
            break u;
        }
    };
    assert_eq!((u.width, u.height, u.width_pt, u.height_pt), (8, 16, 4, 8), "native, never past it");
    assert!(u.key && u.source_start, "a new size starts with a source start");
    let hub = a.hub().clone();
    wait_until("pw clamped to the native width", || hub.layer_specs().iter().any(|l| l.max_width == 8));
    let (_, mut narrow) = join(&b.ws, Want { pw: Some(6), ..want(UnitCodec::Jpeg, 15) }).expect("join narrow");
    let n = next_unit(&mut narrow);
    assert_eq!((n.width, n.height, n.source_start), (6, 12, true), "scaled down to its pane");
    drop((a, wide, narrow));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[("pw", "99")]).await;
        let _hello = next_text(&mut c).await;
        let mut sizes = Vec::new();
        for _ in 0..6 {
            let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("a frame");
            let (h, _) = FrameHeader::decode(&m).expect("K2SF");
            sizes.push((h.width_px, h.width_pt));
            ack(&mut c, h.seq).await;
        }
        assert_eq!(sizes.last(), Some(&(8, 4)), "pixels and points side by side: {sizes:?}");
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    shut_down(b);
}
/// TCP_NODELAY on the stream socket (M-1): a loopback socket through the
/// stream's setup has Nagle off; with the switch forced off it keeps Nagle
/// on, as before.
#[test]
fn the_stream_socket_has_nagle_off_unless_switched_off() {
    use crate::tunnel_tls_listener::{nodelay_test_lock, set_nodelay_switch_for_test};
    let _nodelay = nodelay_test_lock();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("rt");
    rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = l.local_addr().expect("addr");
        let _client = TcpStream::connect(addr).await.expect("connect");
        let (s, _) = l.accept().await.expect("accept");
        assert!(!s.nodelay().expect("read"), "the main listener's sockets start with Nagle on");
        set_nodelay_switch_for_test(true);
        stream_socket_setup(&s);
        assert!(s.nodelay().expect("read"), "switch on: Nagle off");
        s.set_nodelay(false).expect("reset");
        set_nodelay_switch_for_test(false);
        stream_socket_setup(&s);
        assert!(!s.nodelay().expect("read"), "switch off: Nagle stays on");
        set_nodelay_switch_for_test(true);
    });
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
                    assert!(st["skips"]["window"].is_u64() && st["skips"]["age"].is_u64() && st["skips"]["send"].is_u64() && st["catchUps"].is_u64(), "S4's skip counts: {st}");
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
/// out before an ack. A delta that finds the window full is dropped (the
/// layer is shared, so it can't wait), and the viewer skips to a keyframe:
/// the next frame after the ack is one. A keyframe ask brings one.
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
            if h.key {
                assert_eq!(k2_core::sim::video::annexb::codec_string(au).as_deref(), Some("avc1.42E01F"), "SPS in band");
            }
            heads.push(h);
        }
        assert!(heads[0].key, "the first frame is a keyframe");
        assert!(!heads[1].key && !heads[2].key, "then deltas");
        assert!(heads.windows(2).all(|w| w[1].seq == w[0].seq + 1));
        assert!(next_binary(&mut c, Duration::from_millis(400)).await.is_none(), "the window is full: nothing until an ack");
        // One cumulative ack frees the window: the deltas it missed are
        // gone, so it goes on from a keyframe.
        ack(&mut c, heads[2].seq).await;
        let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("a frame after the ack");
        let (h4, _) = VideoHeader::decode(&m).expect("K2SV");
        assert!(h4.key, "a keyframe after the skip");
        assert!(h4.seq > heads[2].seq + 1, "the wire seq shows the gap");
        ack(&mut c, h4.seq).await;
        let mut keyed = false;
        let mut asked = false;
        for _ in 0..40 {
            let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("frames keep coming");
            let (h, _) = VideoHeader::decode(&m).expect("K2SV");
            ack(&mut c, h.seq).await;
            if asked && h.key {
                keyed = true;
                break;
            }
            if !asked && !h.key {
                c.send(Message::Text(r#"{"t":"keyframe"}"#.into())).await.expect("ask");
                asked = true;
            }
        }
        assert!(keyed, "a keyframe after the ask");
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    fake::uninstall();
    let l = log.lock().expect("log").clone();
    assert_eq!(l.opened, vec![(4, 8)], "one encoder for this layer");
    assert!(l.frames.iter().filter(|(_, k)| *k).count() >= 3, "start + skip + ask: {:?}", l.frames);
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

/// Fix 3: a viewer's input runs on its own thread, in order. A drag's
/// moves reach the lane after its down and before its up, ending at the
/// last point (moves that backed up may collapse to the newest, never
/// reorder).
#[test]
fn a_viewers_moves_reach_the_lane_in_order_on_its_input_thread() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("order");
    b.fake.actions.lock().clear();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[]).await;
        let _hello = next_text(&mut c).await;
        let send = |m: String| Message::Text(m);
        c.send(send(r#"{"t":"touch","phase":"down","x":0,"y":20}"#.into())).await.expect("down");
        for x in 1..=40 {
            c.send(send(format!(r#"{{"t":"touch","phase":"move","x":{x},"y":20}}"#))).await.expect("move");
        }
        c.send(send(r#"{"t":"touch","phase":"up","x":40,"y":20}"#.into())).await.expect("up");
        let t0 = Instant::now();
        while !b.fake.actions.lock().iter().any(|a| a.starts_with("touch Up")) {
            assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", b.fake.actions.lock());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    let acts: Vec<String> = b.fake.actions.lock().iter().filter(|a| a.starts_with("touch")).cloned().collect();
    assert_eq!(acts.first().map(String::as_str), Some("touch Down 0,20"), "{acts:?}");
    assert_eq!(acts.last().map(String::as_str), Some("touch Up 40,20"), "{acts:?}");
    let xs: Vec<u32> = acts[1..acts.len() - 1]
        .iter()
        .map(|a| a.strip_prefix("touch Move ").and_then(|r| r.split(',').next()).and_then(|x| x.parse().ok()).expect("a move"))
        .collect();
    assert!(!xs.is_empty() && xs.windows(2).all(|w| w[1] > w[0]), "in order: {xs:?}");
    assert_eq!(xs.last(), Some(&40), "the drag ends at its last point");
    shut_down(b);
}

/// Fix 3 keeps the queue rule: with the device busy on one input, 64 more
/// queue, and the next is refused with `sim_busy`. Taps never merge, so
/// every queued one reaches the lane once the device is free.
#[test]
fn a_full_input_queue_still_answers_sim_busy() {
    let _serial = crate::sim_routes::sim_test_lock();
    let b = boot_fake("busy");
    b.fake.actions.lock().clear();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    rt.block_on(async {
        let (mut c, server) = open(&b, &[]).await;
        let _hello = next_text(&mut c).await;
        b.fake.hold_act.store(true, std::sync::atomic::Ordering::SeqCst);
        c.send(Message::Text(r#"{"t":"tap","x":1,"y":1}"#.into())).await.expect("first");
        tokio::time::sleep(Duration::from_millis(300)).await; // the input thread takes it and waits in the lane
        for i in 2..=66 {
            c.send(Message::Text(format!(r#"{{"t":"tap","x":{i},"y":1}}"#))).await.expect("tap");
        }
        let mut busy = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        while let Ok(Some(Ok(m))) = tokio::time::timeout_at(deadline, c.next()).await {
            if let Message::Text(t) = m {
                let v: serde_json::Value = serde_json::from_str(&t).expect("JSON");
                if v["t"] == "error" {
                    assert_eq!(v["code"], "sim_busy", "{v}");
                    busy += 1;
                }
            }
        }
        assert_eq!(busy, 1, "only the 66th (one past the queue of 64) is refused");
        b.fake.hold_act.store(false, std::sync::atomic::Ordering::SeqCst);
        let t0 = Instant::now();
        while b.fake.actions.lock().iter().filter(|a| a.starts_with("tap")).count() < 65 {
            assert!(t0.elapsed() < Duration::from_secs(5), "{} taps", b.fake.actions.lock().len());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
    });
    let taps = b.fake.actions.lock().iter().filter(|a| a.starts_with("tap")).count();
    assert_eq!(taps, 65, "the first and the 64 queued");
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

/// A protocol-4 stand-in (S3b): the "device" encodes itself, as the iOS
/// helper does. Each on H.264 layer gets a unit every 1/fps s, a key unit
/// (SPS + PPS + IDR) first and on request, else a delta, each with its own
/// bytes; it records what it gave the hub.
struct NativeUnits {
    /// (spec, last seq, next due, key wanted)
    layers: Vec<(k2_core::stream::LayerSpec, u64, Instant, bool)>,
    gave: Arc<std::sync::Mutex<Vec<EncodedUnit>>>,
}

fn native_payload(layer: u8, seq: u64, key: bool) -> Vec<u8> {
    let mut v = if key { vec![0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65] } else { vec![0, 0, 0, 1, 0x41] };
    v.extend_from_slice(format!("native layer {layer} unit {seq}").as_bytes());
    v
}

impl EncodedSource for NativeUnits {
    fn next_unit(&mut self, wait: Duration) -> Result<Option<EncodedUnit>, k2_core::stream::SourceError> {
        let Some(i) = (0..self.layers.len()).min_by_key(|i| self.layers[*i].2) else {
            std::thread::sleep(wait);
            return Ok(None);
        };
        let due = self.layers[i].2;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(wait.min(due - now));
            if Instant::now() < due {
                return Ok(None);
            }
        }
        let (spec, seq, next, want_key) = &mut self.layers[i];
        *seq += 1;
        let key = std::mem::take(want_key) || *seq == 1;
        *next = due + Duration::from_secs_f64(1.0 / spec.fps.max(1) as f64);
        let w = if spec.max_width > 0 { spec.max_width } else { 8 };
        let at = Instant::now();
        let u = EncodedUnit {
            codec: UnitCodec::H264,
            key,
            source_start: *seq == 1,
            layer: spec.id,
            width: w,
            height: w * 2,
            width_pt: 4,
            height_pt: 8,
            seq: *seq,
            captured_at: at,
            stamps: Default::default(),
            pulled_at: at,
            encode: None,
            data: k2_core::stream::Bytes::from(native_payload(spec.id, *seq, key)),
        };
        self.gave.lock().expect("gave").push(u.clone());
        Ok(Some(u))
    }

    fn set_layers(&mut self, layers: &[k2_core::stream::LayerSpec]) {
        let mut next = Vec::new();
        for s in layers.iter().filter(|s| s.on && s.codec == UnitCodec::H264) {
            match self.layers.iter().find(|l| l.0.id == s.id) {
                Some(l) => next.push((*s, l.1, l.2, l.3)),
                None => next.push((*s, 0, Instant::now(), true)),
            }
        }
        self.layers = next;
    }

    fn request_keyframe(&mut self, layer: u8, _since: Option<Instant>) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.0.id == layer) {
            l.3 = true;
        }
    }

    fn sizes(&self) -> Option<k2_core::stream::SourceSizes> {
        Some(k2_core::stream::SourceSizes { points: 4, native_px: Some(8) })
    }
}

/// S3b: a lane whose device encodes itself (the iOS helper on protocol 4)
/// is the hub's source as it is. A K2SV viewer gets each unit's own bytes,
/// byte for byte, with the source's seq, key flag and size, and the daemon
/// encodes nothing: no encoder opens, no frame source opens.
#[test]
fn a_video_viewer_on_a_protocol_4_source_gets_its_bytes_with_no_daemon_encode() {
    let _serial = crate::sim_routes::sim_test_lock();
    let _f = fake::FACTORY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let log = fake::install(); // the host "has H.264"; a daemon encode would show here
    let b = boot_fake("native");
    let gave = Arc::new(std::sync::Mutex::new(Vec::new()));
    let g2 = gave.clone();
    *b.fake.native.lock() = Some(Box::new(move || -> Box<dyn EncodedSource> { Box::new(NativeUnits { layers: vec![], gave: g2.clone() }) }));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("rt");
    let got = rt.block_on(async {
        let (mut c, server) = open(&b, &[("codecs", "h264,jpeg"), ("win", "8"), ("fps", "60"), ("pw", "6")]).await;
        let hello = next_text(&mut c).await;
        assert_eq!((hello["codec"].as_str(), hello["fps"].as_u64()), (Some("h264"), Some(60)));
        let mut got = Vec::new();
        while got.len() < 30 {
            let m = next_binary(&mut c, Duration::from_secs(5)).await.expect("units keep coming");
            let (h, au) = VideoHeader::decode(&m).expect("K2SV");
            ack(&mut c, h.seq).await;
            got.push((h, au.to_vec()));
        }
        drop(c);
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("server ends").expect("join");
        got
    });
    let gave = gave.lock().expect("gave").clone();
    assert!(got[0].0.key, "a key unit first");
    for (h, au) in &got {
        let u = gave.iter().find(|u| u.seq as u32 == h.seq).unwrap_or_else(|| panic!("wire seq {} is a source unit", h.seq));
        assert_eq!(&au[..], &u.data[..], "unit {}: the source's bytes, unchanged", u.seq);
        assert_eq!((h.key, h.width_px, h.height_px, h.width_pt, h.height_pt), (u.key, 6, 12, 4, 8), "unit {}", u.seq);
        assert!(u.encode.is_none());
    }
    assert!(got.windows(2).all(|w| w[1].0.seq > w[0].0.seq), "in order");
    fake::uninstall();
    let l = log.lock().expect("log").clone();
    assert!(l.opened.is_empty() && l.frames.is_empty(), "no daemon encode: {l:?}");
    assert!(b.fake.frame_opts.lock().is_empty(), "no frame source opened");
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
