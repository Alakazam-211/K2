//! A steady stream of conversions allocates no pixel memory: `convert`'s
//! output comes from a pool that gets each buffer back when its frame
//! drops. Counted with this binary's own allocator, in a file of its own
//! so no other test allocates meanwhile.

use kessel_video::{convert, PixelFormat, Planes, VideoFrame};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static BYTES: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator unchanged, counting on the way.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's layout, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: memory from `alloc` above, with its layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        BYTES.fetch_add(new_size, Ordering::Relaxed);
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: memory from `alloc` above, with its layout.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn frame(format: PixelFormat, w: u32, h: u32) -> VideoFrame {
    let len = format.frame_len(w, h);
    let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    VideoFrame::from_planes(Planes::tight(format, w, h, data).expect("planes"), w, h, 0)
        .expect("frame")
}

#[test]
fn steady_conversions_allocate_no_pixel_memory() {
    // Camera frames to the VP8 backup's sizes, landscape and portrait.
    let cases = [
        (
            PixelFormat::Nv12,
            (1280, 720),
            (640, 360),
            PixelFormat::I420,
        ),
        (
            PixelFormat::I420,
            (1280, 720),
            (320, 180),
            PixelFormat::I420,
        ),
        (
            PixelFormat::Nv12,
            (720, 1280),
            (360, 640),
            PixelFormat::I420,
        ),
        (
            PixelFormat::Bgra,
            (1280, 720),
            (640, 360),
            PixelFormat::I420,
        ),
        (
            PixelFormat::Nv12,
            (1280, 720),
            (1280, 720),
            PixelFormat::I420,
        ),
    ];
    for (from, (sw, sh), (dw, dh), to) in cases {
        let src = frame(from, sw, sh);
        // Warm up: the pool's first buffers (and the RGB scratch).
        for _ in 0..3 {
            drop(convert(&src, dw, dh, to).expect("convert"));
        }
        const FRAMES: usize = 100;
        let (b0, c0) = (BYTES.load(Ordering::SeqCst), CALLS.load(Ordering::SeqCst));
        for _ in 0..FRAMES {
            let out = convert(&src, dw, dh, to).expect("convert");
            assert_eq!((out.width, out.height), (dw, dh));
            drop(out);
        }
        let bytes = (BYTES.load(Ordering::SeqCst) - b0) / FRAMES;
        let calls = (CALLS.load(Ordering::SeqCst) - c0) / FRAMES;
        let frame_bytes = to.frame_len(dw, dh);
        eprintln!(
            "{from:?} {sw}x{sh} -> {to:?} {dw}x{dh}: {bytes} bytes in {calls} allocations a frame \
             (the frame is {frame_bytes} bytes)"
        );
        // Only the frame's small shared handle: never pixel memory.
        assert!(
            bytes <= 256 && calls <= 2,
            "{from:?} {sw}x{sh} -> {dw}x{dh}: {bytes} bytes in {calls} allocations a frame"
        );
    }
}
