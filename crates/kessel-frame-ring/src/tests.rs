//! The ring between two processes (this test binary runs itself again as
//! the producer, through [`child_process_entry`]), a producer killed
//! mid-write, wraparound, files that don't match, the control words, and
//! the `extern "C"` surface.

use crate::layout::*;
use crate::*;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const CHILD_ROLE: &str = "KFR_TEST_CHILD";
const CHILD_PATH: &str = "KFR_TEST_PATH";

/// A path in the temp folder no other test uses.
fn temp_ring(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("kfr-{name}-{}-{n}", std::process::id()))
}

/// A small ring with no rate limit.
fn small() -> Layout {
    let mut l = Layout::new(64, 48);
    l.max_fps = 0;
    l
}

/// The byte every pixel of frame `n` has: the frame's number shows in its
/// content, so a torn copy (two frames mixed) can't pass as either.
fn shade(n: u64) -> u8 {
    (16 + (n * 37) % 220) as u8
}

/// Push a uniform `w` x `h` frame of `shade(n)`.
fn push_uniform(p: &mut Producer, w: u32, h: u32, n: u64) -> Pushed {
    let (y, uv) = (
        vec![shade(n); (w * h) as usize],
        vec![shade(n); nv12_len(w, h) - (w * h) as usize],
    );
    let uv_stride = 2 * (w as usize).div_ceil(2);
    let mut f = Nv12Frame::new(&y, w as usize, &uv, uv_stride, w, h);
    f.timestamp_ns = 1_000_000 * n;
    p.push(&f).expect("push")
}

/// Every byte is the shade of the frame's number.
fn check_uniform(info: &FrameInfo, buf: &[u8]) {
    assert_eq!(buf.len(), info.len());
    let want = shade(info.frame_no);
    assert!(
        buf.iter().all(|&b| b == want),
        "frame {} is torn or wrong: want {want}, found {:?}",
        info.frame_no,
        buf.iter().find(|&&b| b != want)
    );
}

/// Start this test binary again as a producer with `role`.
fn spawn_child(role: &str, path: &std::path::Path) -> Child {
    Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "tests::child_process_entry",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_ROLE, role)
        .env(CHILD_PATH, path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the producer process")
}

/// Wait for `line` on the child's stdout.
fn wait_line(child: &mut Child, line: &str) {
    let out = child.stdout.as_mut().expect("piped stdout");
    let mut r = BufReader::new(out);
    let mut l = String::new();
    loop {
        l.clear();
        let n = r.read_line(&mut l).expect("read the child");
        assert!(n > 0, "the child exited before saying {line}");
        // libtest prints the test's name before the test runs, on the
        // same line as the child's first output.
        if l.trim_end().ends_with(line) {
            return;
        }
    }
}

fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(t) = f() {
            return t;
        }
        assert!(Instant::now() < deadline, "nothing within {timeout:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Not a test of its own: the producer side of the two-process tests,
/// which run this binary again with [`CHILD_ROLE`] set. Without it, it
/// returns at once.
#[test]
fn child_process_entry() {
    let Ok(role) = std::env::var(CHILD_ROLE) else {
        return;
    };
    let path = PathBuf::from(std::env::var(CHILD_PATH).expect("a ring path"));
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    match role.as_str() {
        "produce" => {
            for n in 1..=60 {
                push_uniform(&mut p, 64, 48, n);
                std::thread::sleep(Duration::from_millis(2));
            }
            p.finish(EndReason::UserStopped);
        }
        "stress" => {
            // As fast as it goes, scaled down from twice the box.
            for n in 1..=3000 {
                push_uniform(&mut p, 128, 96, n);
            }
            p.finish(EndReason::SourceClosed);
        }
        "torn" => {
            assert!(matches!(
                push_uniform(&mut p, 64, 48, 1),
                Pushed::Written { .. }
            ));
            p.push_torn(0xEE);
            println!("TORN");
            std::thread::sleep(Duration::from_secs(60));
            unreachable!("the parent kills this process");
        }
        other => panic!("unknown child role {other}"),
    }
}

#[test]
fn two_processes_hand_over_frames_and_the_end() {
    let path = temp_ring("two");
    let mut c = Consumer::create(&path, small()).expect("create");
    assert_eq!(c.producer().expect("state"), ProducerState::Waiting);
    let mut child = spawn_child("produce", &path);
    let mut buf = Vec::new();
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(info) = c.try_read(&mut buf) {
            check_uniform(&info, &buf);
            assert_eq!((info.width, info.height), (64, 48));
            assert_eq!(
                info.timestamp_ns,
                1_000_000 * info.frame_no,
                "the timestamp travels"
            );
            seen.push(info.frame_no);
        }
        if let ProducerState::Ended(r) = c.producer().expect("state") {
            assert_eq!(r, EndReason::UserStopped);
            break;
        }
        assert!(Instant::now() < deadline, "no end within 30 s");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(child.wait().expect("wait").success(), "the producer passed");
    // The last frame is there after the end.
    if let Some(info) = c.try_read(&mut buf) {
        seen.push(info.frame_no);
    }
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "in order: {seen:?}");
    assert_eq!(seen.last(), Some(&60), "the last frame arrives");
    assert!(seen.len() >= 10, "most frames read: {}", seen.len());
    let st = c.stats();
    assert_eq!(st.written, 60);
    assert_eq!(st.missed, 60 - seen.len() as u64);
    assert_eq!(st.attaches, 1);
    drop(c);
    assert!(!path.exists(), "the consumer removes its file");
}

#[test]
fn a_racing_producer_never_hands_over_a_torn_frame() {
    let path = temp_ring("stress");
    let mut c = Consumer::create(&path, small()).expect("create");
    let mut child = spawn_child("stress", &path);
    let mut buf = Vec::new();
    let mut reads = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !matches!(c.producer().expect("state"), ProducerState::Ended(_)) {
        if let Some(info) = c.try_read(&mut buf) {
            // Scaled from 128 x 96 into the 64 x 48 box; uniform stays so.
            assert_eq!((info.width, info.height), (64, 48));
            check_uniform(&info, &buf);
            reads += 1;
        }
        assert!(Instant::now() < deadline, "no end within 60 s");
    }
    assert!(child.wait().expect("wait").success());
    let st = c.stats();
    assert_eq!(st.written, 3000);
    assert!(reads > 0);
    eprintln!(
        "[ring] {reads} reads of 3000 frames, {} torn retries given up",
        st.torn
    );
}

#[test]
fn a_killed_writer_leaves_no_torn_frame_and_the_next_producer_takes_over() {
    let path = temp_ring("killed");
    let mut c = Consumer::create(&path, small()).expect("create");
    let mut child = spawn_child("torn", &path);
    wait_line(&mut child, "TORN");
    assert_eq!(c.producer().expect("state"), ProducerState::Running);
    assert!(
        matches!(Producer::attach(&path), Err(RingError::Busy)),
        "one producer at a time"
    );
    let mut buf = Vec::new();
    let info = c.try_read(&mut buf).expect("the whole frame");
    assert_eq!(info.frame_no, 1);
    check_uniform(&info, &buf);

    child.kill().expect("kill the producer mid-write");
    child.wait().expect("wait");
    assert_eq!(
        wait_for(Duration::from_secs(5), || {
            let s = c.producer().expect("state");
            (s == ProducerState::Gone).then_some(s)
        }),
        ProducerState::Gone
    );
    assert!(
        c.try_read(&mut buf).is_none(),
        "the torn frame was never published"
    );
    assert_eq!(c.read_latest(&mut buf).map(|i| i.frame_no), Some(1));

    // The next producer attaches (the lock died with the process), numbers
    // on from the last frame, and rewrites the torn slot.
    let mut p = Producer::attach(&path).expect("attach after a kill");
    p.start();
    assert_eq!(c.producer().expect("state"), ProducerState::Running);
    for n in 2..=8 {
        let Pushed::Written { frame_no, .. } = push_uniform(&mut p, 64, 48, n) else {
            panic!("frame {n} not written");
        };
        assert_eq!(frame_no, n);
        let info = c.try_read(&mut buf).expect("each frame");
        assert_eq!(info.frame_no, n);
        check_uniform(&info, &buf);
    }
    assert_eq!(c.stats().attaches, 2);
    drop(p);
    assert_eq!(
        c.producer().expect("state"),
        ProducerState::Ended(EndReason::SourceClosed),
        "dropped without a reason"
    );
}

#[test]
fn the_ring_wraps_and_a_slow_reader_gets_the_newest() {
    let path = temp_ring("wrap");
    let mut c = Consumer::create(&path, small()).expect("create");
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    let mut buf = Vec::new();
    assert!(c.try_read(&mut buf).is_none(), "nothing yet");
    // Read after every push, around the 3 slots several times.
    for n in 1..=10 {
        push_uniform(&mut p, 64, 48, n);
        let info = c.try_read(&mut buf).expect("a frame");
        assert_eq!(info.frame_no, n);
        check_uniform(&info, &buf);
        assert!(c.try_read(&mut buf).is_none(), "each frame once");
    }
    // Seven pushes, no reads: the newest comes, six were missed.
    for n in 11..=17 {
        push_uniform(&mut p, 64, 48, n);
    }
    let info = c.try_read(&mut buf).expect("the newest");
    assert_eq!(info.frame_no, 17);
    check_uniform(&info, &buf);
    let st = c.stats();
    assert_eq!((st.written, st.missed, st.torn), (17, 6, 0));
    // Again on purpose (a keyframe for an unchanged screen).
    assert_eq!(c.read_latest(&mut buf).map(|i| i.frame_no), Some(17));
}

#[test]
fn a_mismatched_or_missing_ring_is_refused() {
    let path = temp_ring("mismatch");
    assert!(matches!(Producer::attach(&path), Err(RingError::NotFound)));
    let c = Consumer::create(&path, small()).expect("create");
    let len = small().file_len() as u64;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open");
    // One byte short, one byte long, under the header.
    for bad in [len - 1, len + 1, 100] {
        file.set_len(bad).expect("resize");
        let err = Producer::attach(&path).expect_err("refused");
        assert!(matches!(err, RingError::Mismatch(_)), "{bad} bytes: {err}");
        assert_eq!(err.code(), -4);
    }
    file.set_len(len).expect("restore");
    // Another version, or a header that disagrees with itself.
    let v = c.raw_map().u32(H_VERSION);
    v.store(VERSION + 1, Ordering::Relaxed);
    assert!(matches!(
        Producer::attach(&path),
        Err(RingError::Mismatch(_))
    ));
    v.store(VERSION, Ordering::Relaxed);
    let sb = c.raw_map().u64(H_SLOT_BYTES);
    let right = sb.load(Ordering::Relaxed);
    sb.store(right + 64, Ordering::Relaxed);
    assert!(matches!(
        Producer::attach(&path),
        Err(RingError::Mismatch(_))
    ));
    sb.store(right, Ordering::Relaxed);
    let magic = c.raw_map().u64(H_MAGIC);
    magic.store(0, Ordering::Relaxed);
    assert!(matches!(
        Producer::attach(&path),
        Err(RingError::Mismatch(_))
    ));
    magic.store(MAGIC, Ordering::Relaxed);
    let p = Producer::attach(&path).expect("whole again");
    assert!(matches!(Producer::attach(&path), Err(RingError::Busy)));
    drop(p);
    drop(c);
    // Bad layouts never make a file.
    for l in [
        Layout::new(15, 48),
        Layout::new(65, 48),
        Layout::new(8192, 48),
    ] {
        assert!(matches!(
            Consumer::create(&path, l),
            Err(RingError::Invalid(_))
        ));
    }
    let mut l = small();
    l.slots = 1;
    assert!(Consumer::create(&path, l).is_err());
    assert!(!path.exists());
}

#[test]
fn bad_frames_are_refused() {
    let path = temp_ring("bad");
    let _c = Consumer::create(&path, small()).expect("create");
    let mut p = Producer::attach(&path).expect("attach");
    let (y, uv) = (vec![0u8; 64 * 48], vec![0u8; 64 * 24]);
    let ok = Nv12Frame::new(&y, 64, &uv, 64, 64, 48);
    for (f, what) in [
        (Nv12Frame { rotation: 45, ..ok }, "a rotation of 45"),
        (
            Nv12Frame {
                y: &y[..64 * 47],
                ..ok
            },
            "a short Y plane",
        ),
        (
            Nv12Frame {
                uv: &uv[..64 * 23 + 10],
                ..ok
            },
            "a short UV plane",
        ),
        (
            Nv12Frame { y_stride: 63, ..ok },
            "a Y stride under the width",
        ),
        (
            Nv12Frame {
                uv_stride: 62,
                ..ok
            },
            "a UV stride under the width",
        ),
        (Nv12Frame { width: 0, ..ok }, "zero width"),
    ] {
        let err = p.push(&f).expect_err(what);
        assert!(matches!(err, RingError::Invalid(_)), "{what}: {err}");
    }
    assert!(matches!(p.push(&ok).expect("ok"), Pushed::Written { .. }));
}

#[test]
fn the_control_words_drop_frames_without_copying() {
    let path = temp_ring("control");
    let mut c = Consumer::create(&path, Layout::new(64, 48)).expect("create");
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    assert!(p.wants_frames() && p.consumer_present() && !p.stop_requested());
    // 15 fps: 10 ms later is too soon, 50 ms (3/4 of 67) is enough.
    let (y, uv) = (vec![9u8; 64 * 48], vec![9u8; 64 * 24]);
    let (y, uv) = (&y[..], &uv[..]);
    let at = move |ns: u64| {
        let mut f = Nv12Frame::new(y, 64, uv, 64, 64, 48);
        f.timestamp_ns = ns;
        f
    };
    assert!(matches!(
        p.push(&at(1_000_000_000)).expect("push"),
        Pushed::Written { .. }
    ));
    assert_eq!(p.push(&at(1_010_000_000)).expect("push"), Pushed::TooSoon);
    assert!(matches!(
        p.push(&at(1_050_000_000)).expect("push"),
        Pushed::Written { .. }
    ));
    c.set_max_fps(0).expect("no limit");
    assert_eq!(p.layout().max_fps, 0);
    assert!(matches!(
        p.push(&at(1_051_000_000)).expect("push"),
        Pushed::Written { .. }
    ));
    assert!(c.set_max_fps(241).is_err());

    c.set_wanted(false);
    assert!(!p.wants_frames());
    assert_eq!(p.push(&at(2_000_000_000)).expect("push"), Pushed::NotWanted);
    c.set_wanted(true);
    // A consumer that stops looking (the app suspended) counts as away.
    c.raw_map().u64(H_BEAT).store(0, Ordering::Relaxed);
    assert!(!p.consumer_present());
    assert_eq!(p.push(&at(3_000_000_000)).expect("push"), Pushed::NotWanted);
    let mut buf = Vec::new();
    assert!(
        c.try_read(&mut buf).is_some(),
        "a read marks it present again"
    );
    assert!(p.wants_frames());
    assert_eq!(c.stats().dropped, 3);

    c.request_stop();
    assert!(p.stop_requested());
    drop(c);
    assert!(
        !p.consumer_present() && !p.wants_frames(),
        "the consumer is gone"
    );
    assert_eq!(p.push(&at(4_000_000_000)).expect("push"), Pushed::NotWanted);
}

#[test]
fn rotation_colour_and_scaling_travel_with_each_frame() {
    let path = temp_ring("meta");
    let mut c = Consumer::create(&path, Layout::hd720()).expect("create");
    assert_eq!(c.layout(), Layout::hd720());
    assert!(Layout::hd720().file_len() < 4_500_000, "about 4 MB");
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    // An iPhone-sized portrait screen, left half 200 and right half 50,
    // with padded rows.
    let (w, h) = (1170u32, 2532u32);
    let ys = 1216;
    let mut y = vec![0u8; ys * h as usize];
    for row in y.chunks_mut(ys) {
        row[..585].fill(200);
        row[585..1170].fill(50);
    }
    let uv = vec![128u8; ys * (h as usize / 2)];
    let mut f = Nv12Frame::new(&y, ys, &uv, ys, w, h);
    f.rotation = 90;
    f.color = Color::Bt709Full;
    f.timestamp_ns = 42;
    assert_eq!(
        p.push(&f).expect("push"),
        Pushed::Written {
            frame_no: 1,
            width: 590,
            height: 1280
        }
    );
    let mut buf = Vec::new();
    let info = c.try_read(&mut buf).expect("frame");
    assert_eq!((info.width, info.height, info.rotation), (590, 1280, 90));
    assert_eq!((info.color, info.timestamp_ns), (Color::Bt709Full, 42));
    assert_eq!(
        (info.y_stride(), info.uv_offset(), info.uv_stride()),
        (590, 590 * 1280, 590)
    );
    // Each half keeps its shade away from the seam.
    let row = &buf[600 * 590..601 * 590];
    assert!(row[..290].iter().all(|&v| v == 200), "left half");
    assert!(row[300..].iter().all(|&v| v == 50), "right half");
    assert!(buf[info.uv_offset()..].iter().all(|&v| v == 128));
}

#[test]
fn the_c_surface_works_end_to_end() {
    use crate::ffi::*;
    let path = temp_ring("ffi");
    let mut c = Consumer::create(&path, small()).expect("create");
    let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("c path");
    let mut err = 99;
    // SAFETY: a NUL-terminated path and a valid error slot.
    let p = unsafe { kfr_producer_attach(cpath.as_ptr(), &mut err) };
    assert!(!p.is_null());
    assert_eq!(err, KFR_OK);
    let (y, uv) = (vec![shade(1); 64 * 48], vec![shade(1); 64 * 24]);
    // SAFETY: a live producer and planes of the lengths given.
    unsafe {
        assert_eq!(kfr_producer_start(p), KFR_OK);
        assert!(kfr_producer_wants_frames(p));
        assert!(!kfr_producer_stop_requested(p));
        let push = |rotation, color| {
            kfr_producer_push_nv12(
                p,
                y.as_ptr(),
                y.len(),
                64,
                uv.as_ptr(),
                uv.len(),
                64,
                64,
                48,
                rotation,
                color,
                0,
            )
        };
        assert_eq!(push(270, 1), KFR_OK);
        assert_eq!(push(45, 1), KFR_ERR_INVALID);
        assert_eq!(push(0, 7), KFR_ERR_INVALID);
        assert_eq!(
            kfr_producer_push_nv12(
                p,
                std::ptr::null(),
                0,
                64,
                uv.as_ptr(),
                uv.len(),
                64,
                64,
                48,
                0,
                0,
                0
            ),
            KFR_ERR_INVALID
        );
    }
    let mut buf = Vec::new();
    let info = c.try_read(&mut buf).expect("frame");
    check_uniform(&info, &buf);
    assert_eq!((info.rotation, info.color), (270, Color::Bt709Limited));
    assert!(info.timestamp_ns > 0 && info.timestamp_ns <= kfr_monotonic_ns());
    c.request_stop();
    // SAFETY: the same live producer, then finished once.
    unsafe {
        assert!(kfr_producer_stop_requested(p));
        kfr_producer_finish(p, EndReason::UserStopped as u32);
        kfr_producer_finish(std::ptr::null_mut(), 1);
    }
    assert_eq!(
        c.producer().expect("state"),
        ProducerState::Ended(EndReason::UserStopped)
    );

    // Errors come back as codes.
    let missing = std::ffi::CString::new(temp_ring("ffi-missing").as_os_str().as_encoded_bytes())
        .expect("c path");
    // SAFETY: NUL-terminated paths (or null) and valid error slots.
    unsafe {
        assert!(kfr_producer_attach(missing.as_ptr(), &mut err).is_null());
        assert_eq!(err, -2);
        assert!(kfr_producer_attach(std::ptr::null(), &mut err).is_null());
        assert_eq!(err, KFR_ERR_INVALID);
        assert!(kfr_producer_attach(missing.as_ptr(), std::ptr::null_mut()).is_null());
        assert!(!kfr_producer_wants_frames(std::ptr::null()));
        assert_eq!(kfr_producer_start(std::ptr::null_mut()), KFR_ERR_INVALID);
    }
}

/// The README says both sides are `Send` (and not `Sync`).
#[test]
fn both_sides_can_move_to_another_thread() {
    fn send<T: Send>() {}
    send::<Consumer>();
    send::<Producer>();
}
