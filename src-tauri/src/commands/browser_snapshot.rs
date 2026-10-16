//! Freeze-frame snapshots of a Browser tab's native page (0.46).
//!
//! When a K2 overlay covers the page, the renderer shows a picture of it in
//! place of the native view (see `src/renderer/components/BrowserPane/
//! browserFreeze.ts`). This module takes that picture with each engine's own
//! API — no screen capture, so no Screen Recording permission:
//!
//! - macOS: `WKWebView takeSnapshotWithConfiguration:completionHandler:`
//!   (the visible page, at backing scale); JPEG via ImageIO
//!   (`NSBitmapImageRep`) on a worker thread.
//! - Windows: WebView2 `CapturePreview` straight to a JPEG stream.
//! - Linux: WebKitGTK `webkit_web_view_get_snapshot` (visible region, a
//!   cairo image surface); JPEG via the `image` crate on a worker thread.
//!
//! Capture runs on the main thread (inside `with_webview`) and hands its
//! result to a `Send` callback; encoding runs off the main thread. The
//! callback shape mirrors wry PR #1674 (`WebView::screenshot(handler)`) so
//! this can move into wry unchanged later.

/// Byte order of one 32-bit pixel in memory. Which variants a build
/// constructs depends on the platform (and the target's endianness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PixelOrder {
    Bgra,
    Rgba,
    Argb,
    Abgr,
}

impl PixelOrder {
    /// Offsets of R, G and B inside one 4-byte pixel.
    fn rgb_offsets(self) -> [usize; 3] {
        match self {
            PixelOrder::Bgra => [2, 1, 0],
            PixelOrder::Rgba => [0, 1, 2],
            PixelOrder::Argb => [1, 2, 3],
            PixelOrder::Abgr => [3, 2, 1],
        }
    }
}

/// 32-bit pixels straight from the engine.
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row (may be padded past `width * 4`).
    pub stride: usize,
    pub order: PixelOrder,
    pub data: Vec<u8>,
}

/// What a platform capture hands back. `Send`: encoding runs elsewhere.
/// Each platform constructs one variant.
#[allow(dead_code)]
pub enum Captured {
    /// Already JPEG (WebView2).
    Jpeg(Vec<u8>),
    /// Raw pixels (WebKitGTK).
    Raw(RawFrame),
    /// A CGImage (WKWebView); ImageIO encodes it.
    #[cfg(target_os = "macos")]
    Cg(objc2::rc::Retained<objc2_core_graphics::CGImage>),
}

/// JPEG quality for frozen frames: sharp text, ~0.5–1 MB at 2× a large pane.
pub const JPEG_QUALITY: u8 = 80;

impl Captured {
    /// Encode to JPEG. Call off the main thread.
    pub fn into_jpeg(self) -> Result<Vec<u8>, String> {
        match self {
            Captured::Jpeg(bytes) if !bytes.is_empty() => Ok(bytes),
            Captured::Jpeg(_) => Err("snapshot stream was empty".into()),
            Captured::Raw(frame) => raw_to_jpeg(&frame, JPEG_QUALITY),
            #[cfg(target_os = "macos")]
            Captured::Cg(image) => macos::cg_to_jpeg(&image, f64::from(JPEG_QUALITY) / 100.0),
        }
    }
}

/// Drop alpha (pages are opaque) and JPEG-encode.
pub fn raw_to_jpeg(frame: &RawFrame, quality: u8) -> Result<Vec<u8>, String> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 {
        return Err("snapshot has no pixels".into());
    }
    let row_bytes = w
        .checked_mul(4)
        .ok_or_else(|| "snapshot row too wide".to_string())?;
    if frame.stride < row_bytes {
        return Err(format!("snapshot stride {} < row {}", frame.stride, row_bytes));
    }
    let needed = frame.stride * (h - 1) + row_bytes;
    if frame.data.len() < needed {
        return Err(format!(
            "snapshot buffer {} bytes < {} needed",
            frame.data.len(),
            needed
        ));
    }
    let [r, g, b] = frame.order.rgb_offsets();
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in frame.data.chunks(frame.stride).take(h) {
        for px in row[..row_bytes].chunks_exact(4) {
            rgb.extend_from_slice(&[px[r], px[g], px[b]]);
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode(&rgb, frame.width, frame.height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(out)
}

/// Result handler for one capture. Called exactly once, on the main thread.
pub type SnapshotHandler = Box<dyn FnOnce(Result<Captured, String>) + Send + 'static>;

#[cfg(target_os = "macos")]
pub mod macos {
    use std::cell::Cell;

    use block2::RcBlock;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSImageCompressionFactor};
    use objc2_core_graphics::CGImage;
    use objc2_foundation::{NSDictionary, NSError, NSNumber};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

    use super::{Captured, SnapshotHandler};

    /// Main thread. `handler` gets the page's visible region as a CGImage.
    pub fn capture(webview: &WKWebView, handler: SnapshotHandler) {
        let Some(mtm) = MainThreadMarker::new() else {
            handler(Err("snapshot must start on the main thread".into()));
            return;
        };
        let pending = Cell::new(Some(handler));
        let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let Some(handler) = pending.take() else {
                return;
            };
            // SAFETY: WebKit passes either a valid image or a valid error.
            handler(unsafe { image_result(image, error) });
        });
        // SAFETY: main thread (marker above); WebKit retains the block.
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            // What is on screen now: no extra layout/paint pass, faster.
            config.setAfterScreenUpdates(false);
            webview.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
        }
    }

    unsafe fn image_result(image: *mut NSImage, error: *mut NSError) -> Result<Captured, String> {
        if let Some(error) = error.as_ref() {
            return Err(format!(
                "snapshot failed ({}:{}): {}",
                error.domain(),
                error.code(),
                error.localizedDescription()
            ));
        }
        let image = image.as_ref().ok_or("snapshot returned no image")?;
        let cg: Retained<CGImage> = image
            .CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None)
            .ok_or("snapshot image has no bitmap")?;
        Ok(Captured::Cg(cg))
    }

    /// Any thread (ImageIO under NSBitmapImageRep is thread-safe for a rep
    /// created and used on one thread).
    pub fn cg_to_jpeg(image: &CGImage, quality: f64) -> Result<Vec<u8>, String> {
        autoreleasepool(|_| {
            let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
            let factor = NSNumber::new_f64(quality);
            // SAFETY: the extern static key is a valid NSString.
            let key = unsafe { NSImageCompressionFactor };
            let props = NSDictionary::from_slices(&[key], &[factor.as_ref()]);
            // SAFETY: JPEG with a compression factor is a documented pairing.
            let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &props) }
                .ok_or("jpeg encode failed")?;
            Ok(data.to_vec())
        })
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub mod gtk {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    use super::{Captured, PixelOrder, RawFrame, SnapshotHandler};

    /// Main thread. `handler` gets the visible region as raw pixels.
    pub fn capture(webview: &webkit2gtk::WebView, handler: SnapshotHandler) {
        webview.snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            None::<&webkit2gtk::gio::Cancellable>,
            move |result| {
                handler(
                    result
                        .map_err(|e| format!("snapshot failed: {e}"))
                        .and_then(surface_frame),
                )
            },
        );
    }

    fn surface_frame(surface: cairo::Surface) -> Result<Captured, String> {
        let image = cairo::ImageSurface::try_from(surface)
            .map_err(|_| "snapshot is not an image surface".to_string())?;
        match image.format() {
            cairo::Format::ARgb32 | cairo::Format::Rgb24 => {}
            other => return Err(format!("snapshot format {other:?} is not 32-bit")),
        }
        let width = u32::try_from(image.width()).map_err(|_| "negative width")?;
        let height = u32::try_from(image.height()).map_err(|_| "negative height")?;
        let stride = usize::try_from(image.stride()).map_err(|_| "negative stride")?;
        image.flush();
        let mut data = Vec::new();
        image
            .with_data(|bytes| data.extend_from_slice(bytes))
            .map_err(|e| format!("snapshot pixels unreadable: {e}"))?;
        Ok(Captured::Raw(RawFrame {
            width,
            height,
            stride,
            // cairo ARGB32/RGB24 is a native-endian 32-bit 0xAARRGGBB.
            order: if cfg!(target_endian = "little") {
                PixelOrder::Bgra
            } else {
                PixelOrder::Argb
            },
            data,
        }))
    }
}

#[cfg(windows)]
pub mod windows {
    use std::cell::Cell;

    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
    };
    use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    use super::{Captured, SnapshotHandler};

    /// Main (UI) thread. WebView2 encodes the JPEG itself.
    pub fn capture(controller: &ICoreWebView2Controller, handler: SnapshotHandler) {
        let pending = std::rc::Rc::new(Cell::new(Some(handler)));
        let fail = |pending: &Cell<Option<SnapshotHandler>>, msg: String| {
            if let Some(handler) = pending.take() {
                handler(Err(msg));
            }
        };
        // SAFETY: COM calls on the controller's own UI thread.
        let started = unsafe {
            (|| -> windows_core::Result<()> {
                let core = controller.CoreWebView2()?;
                let stream: IStream = SHCreateMemStream(None)
                    .ok_or_else(|| windows_core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY))?;
                let read_stream = stream.clone();
                let done = pending.clone();
                let completed = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                    let bytes = result.and_then(|()| read_all(&read_stream));
                    if let Some(handler) = done.take() {
                        handler(match bytes {
                            Ok(bytes) => Ok(Captured::Jpeg(bytes)),
                            Err(e) => Err(format!("CapturePreview failed: {e}")),
                        });
                    }
                    Ok(())
                }));
                core.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG, &stream, &completed)
            })()
        };
        if let Err(e) = started {
            fail(&pending, format!("CapturePreview did not start: {e}"));
        }
    }

    unsafe fn read_all(stream: &IStream) -> windows_core::Result<Vec<u8>> {
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            stream
                .Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read))
                .ok()?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(order: PixelOrder, stride: usize, w: u32, h: u32, px: [u8; 4]) -> RawFrame {
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                data[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&px);
            }
        }
        RawFrame {
            width: w,
            height: h,
            stride,
            order,
            data,
        }
    }

    fn decode(jpeg: &[u8]) -> image::RgbImage {
        image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            .expect("our own jpeg decodes")
            .to_rgb8()
    }

    fn close(a: u8, b: u8) -> bool {
        (i16::from(a) - i16::from(b)).abs() <= 6
    }

    #[test]
    fn bgra_with_padded_rows_encodes_the_right_colour() {
        // Pure red in BGRA, rows padded to 40 bytes (8 px wide = 32 bytes).
        let f = frame(PixelOrder::Bgra, 40, 8, 6, [0, 0, 255, 255]);
        let jpeg = raw_to_jpeg(&f, 90).expect("encodes");
        assert_eq!(&jpeg[..3], &[0xff, 0xd8, 0xff], "JPEG magic");
        let img = decode(&jpeg);
        assert_eq!((img.width(), img.height()), (8, 6));
        let p = img.get_pixel(4, 3).0;
        assert!(close(p[0], 255) && close(p[1], 0) && close(p[2], 0), "got {p:?}");
    }

    #[test]
    fn every_pixel_order_maps_to_the_same_colour() {
        // (order, bytes for R=10 G=200 B=90 A=255)
        let cases = [
            (PixelOrder::Bgra, [90, 200, 10, 255]),
            (PixelOrder::Rgba, [10, 200, 90, 255]),
            (PixelOrder::Argb, [255, 10, 200, 90]),
            (PixelOrder::Abgr, [255, 90, 200, 10]),
        ];
        for (order, px) in cases {
            let img = decode(&raw_to_jpeg(&frame(order, 32, 8, 8, px), 95).expect("encodes"));
            let p = img.get_pixel(3, 3).0;
            assert!(
                close(p[0], 10) && close(p[1], 200) && close(p[2], 90),
                "{order:?} → {p:?}"
            );
        }
    }

    #[test]
    fn short_buffers_and_bad_strides_fail_loudly() {
        let mut f = frame(PixelOrder::Rgba, 32, 8, 4, [1, 2, 3, 4]);
        f.data.truncate(100);
        assert!(raw_to_jpeg(&f, 80).unwrap_err().contains("needed"));
        let f = frame(PixelOrder::Rgba, 16, 8, 4, [1, 2, 3, 4]);
        assert!(raw_to_jpeg(&f, 80).unwrap_err().contains("stride"));
        let f = frame(PixelOrder::Rgba, 0, 0, 0, [0; 4]);
        assert!(raw_to_jpeg(&f, 80).unwrap_err().contains("no pixels"));
    }

    #[test]
    fn empty_jpeg_stream_is_an_error() {
        assert!(Captured::Jpeg(Vec::new()).into_jpeg().is_err());
        assert_eq!(Captured::Jpeg(vec![1, 2]).into_jpeg().expect("passes through"), vec![1, 2]);
    }
}
