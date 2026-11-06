//! H.264 decoding through Media Foundation: a hardware decoder MFT from the
//! GPU's driver when there is one, else Microsoft's H.264 decoder that ships
//! with Windows (on the CPU here: it is given no Direct3D device).
//!
//! Input is one Annex B access unit per frame, as the crate's encoders give
//! it. Low latency is on, so each frame comes out as soon as it is decoded.
//! Outputs are NV12 CPU planes, cropped to the picture's display area; the
//! decoder thread converts them to the caller's format.

use super::map_error;
use super::mft::{self, available, sample_from, unpack, Found, Mft, NoOutput, Role};
use crate::error::{DecodeError, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::DecoderCore;
use crate::types::*;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;

thread_local! {
    static COM_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn com_on_this_thread() -> Result<(), VideoError> {
    if COM_READY.with(|c| c.get()) {
        return Ok(());
    }
    super::camera::init()?;
    COM_READY.with(|c| c.set(true));
    Ok(())
}

/// Where the picture sits in the MFT's output buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    /// The allocated size (rounded up to macroblocks).
    alloc_w: u32,
    alloc_h: u32,
    /// The picture inside it.
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Row stride when the buffer isn't a 2D buffer.
    stride: u32,
}

/// Unlocks a 2D buffer locked with `Lock2DSize`.
struct Unlock2D<'a>(&'a IMF2DBuffer2);

impl Drop for Unlock2D<'_> {
    fn drop(&mut self) {
        // SAFETY: locked by the caller, unlocked once.
        let _ = unsafe { self.0.Unlock2D() };
    }
}

/// Unlocks a buffer locked with `Lock`.
struct Unlock<'a>(&'a IMFMediaBuffer);

impl Drop for Unlock<'_> {
    fn drop(&mut self) {
        // SAFETY: locked by the caller, unlocked once.
        let _ = unsafe { self.0.Unlock() };
    }
}

/// Copy the picture `l` describes out of NV12 bytes `src` (the first
/// scanline first, `pitch` bytes a row, the UV plane after `l.alloc_h`
/// rows) as tight NV12. Every range is computed with checked arithmetic and
/// taken with `get`, so a layout that doesn't fit `src` is an error, never
/// a read past it.
///
/// A crop that starts on an odd column or row starts one earlier, on the
/// even one: 4:2:0 chroma covers 2x2 blocks, and starting on a block keeps
/// each pixel's chroma its own.
fn copy_nv12(src: &[u8], pitch: usize, l: &Layout) -> Result<Vec<u8>, VideoError> {
    let misfit = || {
        VideoError::os(
            0,
            format!(
                "an NV12 output of {} bytes, pitch {pitch}, can't hold a {}x{}+{}+{} picture \
                 in {}x{}",
                src.len(),
                l.w,
                l.h,
                l.x,
                l.y,
                l.alloc_w,
                l.alloc_h
            ),
        )
    };
    let (w, h) = (l.w as usize, l.h as usize);
    let (x, y) = (l.x as usize & !1, l.y as usize & !1);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    // Rows of the picture never run into the next row.
    let fits_row = |n: usize| x.checked_add(n).is_some_and(|end| end <= pitch);
    if w == 0 || h == 0 || !fits_row(w) || !fits_row(cw * 2) {
        return Err(misfit());
    }
    let uv_at = pitch.checked_mul(l.alloc_h as usize).ok_or_else(misfit)?;
    let row = |base: usize, r: usize, n: usize| -> Option<&[u8]> {
        let at = r.checked_mul(pitch)?.checked_add(base)?.checked_add(x)?;
        src.get(at..at.checked_add(n)?)
    };
    let mut out = Vec::with_capacity(PixelFormat::Nv12.frame_len(l.w, l.h));
    for r in y..y + h {
        out.extend_from_slice(row(0, r, w).ok_or_else(misfit)?);
    }
    for r in y / 2..y / 2 + ch {
        out.extend_from_slice(row(uv_at, r, cw * 2).ok_or_else(misfit)?);
    }
    Ok(out)
}

pub(crate) struct MfDecoder {
    mft: Mft,
    /// What `mft` was opened from, to reopen it.
    found: Found,
    layout: Option<Layout>,
    last_time: i64,
    /// The session failed: `reset` opens a new one.
    failed: bool,
}

// SAFETY: the MFT and its activation object are free-threaded (see
// `Mft`); one thread at a time drives the decoder (&mut self).
unsafe impl Send for MfDecoder {}

/// Offer NV12 out (after the input type, or after a stream change).
fn set_nv12_output(t: &IMFTransform, out_id: u32) -> Result<(), VideoError> {
    let ty = available(t, out_id, false, &MFVideoFormat_NV12)
        .ok_or_else(|| VideoError::os(0, "the H.264 decoder offers no NV12 output"))?;
    // SAFETY: a type the MFT offered.
    unsafe { t.SetOutputType(out_id, &ty, 0) }.map_err(|e| map_error(&e, "SetOutputType (NV12)"))
}

impl MfDecoder {
    pub(crate) fn open(cfg: &DecoderConfig) -> Result<MfDecoder, VideoError> {
        if cfg.codec != VideoCodec::H264 {
            return Err(crate::error::unsupported(format!(
                "Media Foundation here decodes H.264, not {:?}",
                cfg.codec
            )));
        }
        let candidates = mft::find(Role::Decoder, cfg.implementation)?;
        let mut last = None;
        for f in candidates {
            match open_one(&f) {
                Ok(mft) => {
                    return Ok(MfDecoder {
                        mft,
                        found: f,
                        layout: None,
                        last_time: -1,
                        failed: false,
                    })
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| mft::none_found(Role::Decoder, cfg.implementation)))
    }

    /// Read where the picture is in the current output type.
    fn read_layout(&mut self) -> Result<Layout, VideoError> {
        // SAFETY: reads of the MFT's current output type.
        unsafe {
            let t = self
                .mft
                .transform
                .GetOutputCurrentType(self.mft.out_id())
                .map_err(|e| map_error(&e, "GetOutputCurrentType"))?;
            let (alloc_w, alloc_h) = unpack(
                t.GetUINT64(&MF_MT_FRAME_SIZE)
                    .map_err(|e| map_error(&e, "MF_MT_FRAME_SIZE"))?,
            );
            let mut area = MFVideoArea::default();
            let bytes = std::slice::from_raw_parts_mut(
                &mut area as *mut MFVideoArea as *mut u8,
                std::mem::size_of::<MFVideoArea>(),
            );
            let aperture = [MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_GEOMETRIC_APERTURE]
                .iter()
                .any(|k| t.GetBlob(k, bytes, None).is_ok());
            let (x, y, w, h) = if aperture
                && area.Area.cx > 0
                && area.Area.cy > 0
                && area.OffsetX.value >= 0
                && area.OffsetY.value >= 0
            {
                (
                    area.OffsetX.value as u32,
                    area.OffsetY.value as u32,
                    area.Area.cx as u32,
                    area.Area.cy as u32,
                )
            } else {
                (0, 0, alloc_w, alloc_h)
            };
            let stride = t
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|s| (s as i32).unsigned_abs())
                .unwrap_or(alloc_w);
            let inside = |o: u32, n: u32, total: u32| o.checked_add(n).is_some_and(|e| e <= total);
            if !inside(x, w, alloc_w) || !inside(y, h, alloc_h) || w == 0 || h == 0 {
                return Err(VideoError::os(
                    0,
                    format!("a {w}x{h}+{x}+{y} picture in a {alloc_w}x{alloc_h} buffer"),
                ));
            }
            Ok(Layout {
                alloc_w,
                alloc_h,
                x,
                y,
                w,
                h,
                stride: stride.max(alloc_w),
            })
        }
    }

    /// The picture in `sample`, as tight NV12 planes. The bytes read are
    /// the ones the buffer's lock reports, never a length worked out from
    /// the media type: a layout that doesn't fit the buffer (a sample from
    /// before a size change, a lying MFT) is an error, not a read past it.
    fn copy_out(sample: &IMFSample, l: Layout) -> Result<Planes, VideoError> {
        // SAFETY: a live sample's contiguous buffer.
        let buf = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|e| map_error(&e, "ConvertToContiguousBuffer"))?;
        let out = match buf.cast::<IMF2DBuffer2>() {
            Ok(b2) => {
                let (mut scan0, mut start) = (std::ptr::null_mut(), std::ptr::null_mut());
                let (mut pitch, mut len) = (0i32, 0u32);
                // SAFETY: a read lock that reports the first scanline, the
                // row pitch, and the buffer's start and length; unlocked by
                // the guard after the copy.
                unsafe {
                    b2.Lock2DSize(
                        MF2DBuffer_LockFlags_Read,
                        &mut scan0,
                        &mut pitch,
                        &mut start,
                        &mut len,
                    )
                }
                .map_err(|e| map_error(&e, "Lock2DSize"))?;
                let _unlock = Unlock2D(&b2);
                // Bytes from the first scanline to the buffer's end. A
                // bottom-up buffer (negative pitch) isn't an NV12 decoder
                // output; refuse it rather than walk backwards.
                let skip = (scan0 as usize).checked_sub(start as usize);
                let avail = skip.and_then(|s| (len as usize).checked_sub(s));
                let (Some(avail), Ok(pitch)) = (avail, usize::try_from(pitch)) else {
                    return Err(VideoError::os(
                        0,
                        format!("a locked NV12 buffer with pitch {pitch} and length {len}"),
                    ));
                };
                if scan0.is_null() {
                    return Err(VideoError::os(0, "a locked NV12 buffer with no memory"));
                }
                // SAFETY: the lock maps `len` bytes from `start`, so the
                // `avail` bytes from `scan0` are inside it, valid until the
                // guard unlocks (after this copy).
                let src = unsafe { std::slice::from_raw_parts(scan0 as *const u8, avail) };
                copy_nv12(src, pitch, &l)?
            }
            Err(_) => {
                let (mut p, mut cur) = (std::ptr::null_mut(), 0u32);
                // SAFETY: a read of the buffer's current bytes; unlocked by
                // the guard after the copy.
                unsafe { buf.Lock(&mut p, None, Some(&mut cur)) }
                    .map_err(|e| map_error(&e, "Lock"))?;
                let _unlock = Unlock(&buf);
                if p.is_null() {
                    return Err(VideoError::os(0, "a locked NV12 buffer with no memory"));
                }
                // SAFETY: the lock maps the buffer's `cur` current bytes,
                // valid until the guard unlocks.
                let src = unsafe { std::slice::from_raw_parts(p as *const u8, cur as usize) };
                copy_nv12(src, l.stride as usize, &l)?
            }
        };
        Planes::tight(PixelFormat::Nv12, l.w, l.h, out)
    }

    fn decode_inner(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, VideoError> {
        com_on_this_thread()?;
        let time = ((input.capture_us.min(i64::MAX as u64 / 10) * 10) as i64)
            .max(self.last_time.saturating_add(1));
        self.last_time = time;
        let sample = sample_from(&input.data, time, 333_333)?;
        let out_id = self.mft.out_id();
        let mut changed = false;
        let result = self.mft.process(&sample, NoOutput::Wait, &mut |t| {
            changed = true;
            set_nv12_output(t, out_id)
        });
        // Before anything can fail: a change seen at all makes the cached
        // layout stale, error or not.
        if changed {
            self.layout = None;
        }
        let outs = result?;
        // Only outputs in the current type can be read with its layout;
        // any that came out before a change are dropped (real time: the
        // newest frame is the one to show anyway).
        let fresh = outs.get(self.mft.fresh_from()..).unwrap_or_default();
        let Some(last) = fresh.last() else {
            return Ok(None);
        };
        let layout = match self.layout {
            Some(l) => l,
            None => {
                let l = self.read_layout()?;
                self.layout = Some(l);
                l
            }
        };
        let planes = MfDecoder::copy_out(last, layout)?;
        Ok(Some(VideoFrame::from_planes(
            planes,
            layout.w,
            layout.h,
            input.capture_us,
        )?))
    }
}

pub(super) fn open_one(found: &Found) -> Result<Mft, VideoError> {
    let mut mft = Mft::open(found)?;
    let input = mft::video_type(&MFVideoFormat_H264)?;
    // SAFETY: offering the MFT an H.264 input type.
    unsafe { mft.transform.SetInputType(mft.in_id(), &input, 0) }
        .map_err(|e| map_error(&e, "SetInputType (H.264)"))?;
    set_nv12_output(&mft.transform, mft.out_id())?;
    mft.begin()?;
    Ok(mft)
}

impl DecoderCore for MfDecoder {
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError> {
        self.decode_inner(input).map_err(|e| {
            if mft::is_lost(&e) {
                self.failed = true;
                DecodeError::SessionLost
            } else {
                match e {
                    VideoError::Os { detail, .. } => DecodeError::Corrupt(detail),
                    other => DecodeError::Video(other),
                }
            }
        })
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        com_on_this_thread()?;
        self.layout = None;
        self.last_time = -1;
        if !self.failed && self.mft.flush().is_ok() {
            return Ok(());
        }
        // A lost session: the same MFT again (never another kind, so the
        // implementation reported at open stays true).
        self.mft = open_one(&self.found)?;
        self.failed = false;
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        if self.mft.hardware {
            Implementation::Hardware
        } else {
            Implementation::Software
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(alloc: (u32, u32), at: (u32, u32), size: (u32, u32)) -> Layout {
        Layout {
            alloc_w: alloc.0,
            alloc_h: alloc.1,
            x: at.0,
            y: at.1,
            w: size.0,
            h: size.1,
            stride: alloc.0,
        }
    }

    /// An NV12 buffer of `alloc_w` x `alloc_h` with `pitch`-byte rows:
    /// luma byte = row * 16 + column, chroma byte = 200 + row * 16 + column.
    fn buffer(pitch: usize, alloc_w: usize, alloc_h: usize) -> Vec<u8> {
        let mut b = vec![0xEEu8; pitch * (alloc_h + alloc_h.div_ceil(2))];
        for r in 0..alloc_h {
            for c in 0..alloc_w {
                b[r * pitch + c] = (r * 16 + c) as u8;
            }
        }
        for r in 0..alloc_h.div_ceil(2) {
            for c in 0..alloc_w {
                b[(alloc_h + r) * pitch + c] = (200 + r * 16 + c) as u8;
            }
        }
        b
    }

    #[test]
    fn the_picture_comes_out_tight_and_cropped() {
        let src = buffer(6, 4, 4);
        let out = copy_nv12(&src, 6, &layout((4, 4), (0, 0), (4, 4))).expect("copy");
        assert_eq!(
            out,
            [
                0, 1, 2, 3, 16, 17, 18, 19, 32, 33, 34, 35, 48, 49, 50, 51, // Y
                200, 201, 202, 203, 216, 217, 218, 219, // UV
            ]
        );
        // A 2x2 crop at (2, 2): the second chroma row, the second pair.
        let out = copy_nv12(&src, 6, &layout((4, 4), (2, 2), (2, 2))).expect("copy");
        assert_eq!(out, [34, 35, 50, 51, 218, 219]);
        // An odd origin starts on the even column and row before it, so
        // luma and chroma stay the same block.
        let out = copy_nv12(&src, 6, &layout((4, 4), (3, 3), (1, 1))).expect("copy");
        assert_eq!(out, [34, 218, 219]);
    }

    /// A sample around a real Media Foundation 2D NV12 buffer (what a
    /// hardware decoder hands out), filled as `buffer` fills its bytes.
    fn sample_2d(w: u32, h: u32) -> IMFSample {
        type Create2D = unsafe extern "system" fn(
            u32,
            u32,
            u32,
            windows::core::BOOL,
            *mut *mut std::ffi::c_void,
        ) -> windows::core::HRESULT;
        super::super::camera::init().expect("Media Foundation");
        let m = super::super::mfdll::load("mfplat.dll").expect("mfplat");
        // SAFETY: MFCreate2DMediaBuffer's signature.
        let create: Create2D =
            unsafe { super::super::mfdll::sym(m, c"MFCreate2DMediaBuffer") }.expect("export");
        let mut p = std::ptr::null_mut();
        // SAFETY: an NV12 FourCC, top-down; an out pointer for a buffer we
        // then own.
        unsafe { create(w, h, MFVideoFormat_NV12.data1, false.into(), &mut p) }
            .ok()
            .expect("MFCreate2DMediaBuffer");
        // SAFETY: an owned IMFMediaBuffer reference.
        let buf = unsafe { IMFMediaBuffer::from_raw(p) };
        let b2: IMF2DBuffer2 = buf.cast().expect("IMF2DBuffer2");
        let (mut scan0, mut start) = (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut pitch, mut len) = (0i32, 0u32);
        // SAFETY: a write lock of a buffer we own; we write inside the
        // `len` bytes it reports, then unlock.
        unsafe {
            b2.Lock2DSize(
                MF2DBuffer_LockFlags_Write,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut len,
            )
            .expect("lock");
            let fill = buffer(pitch as usize, w as usize, h as usize);
            assert!(fill.len() <= len as usize, "{} > {len}", fill.len());
            std::ptr::copy_nonoverlapping(fill.as_ptr(), scan0, fill.len());
            b2.Unlock2D().expect("unlock");
            buf.SetCurrentLength(len).expect("length");
        }
        let s = super::super::mfdll::mfplat()
            .expect("mfplat")
            .create_sample()
            .expect("sample");
        // SAFETY: a new sample and a live buffer.
        unsafe { s.AddBuffer(&buf) }.expect("add");
        s
    }

    #[test]
    fn a_2d_buffer_is_read_within_the_length_its_lock_reports() {
        let s = sample_2d(64, 48);
        let ok = MfDecoder::copy_out(&s, layout((64, 48), (0, 0), (64, 48))).expect("copy");
        let tight = ok.to_tight(64, 48);
        assert_eq!(tight.data[64 + 2], 16 + 2, "row 1, column 2");
        assert_eq!(
            tight.data[64 * 48 + 64 + 2],
            (200 + 16 + 2) as u8,
            "UV row 1"
        );
        // The same buffer read as a far taller frame (a sample from before
        // a size change): refused against the lock's real length.
        let tall = layout((64, 2160), (0, 0), (64, 2160));
        assert!(MfDecoder::copy_out(&s, tall).is_err());
        let wide = layout((4096, 48), (0, 0), (4096, 48));
        assert!(MfDecoder::copy_out(&s, wide).is_err());
    }

    #[test]
    fn a_layout_bigger_than_the_buffer_is_an_error_not_a_read_past_it() {
        // The buffer a small frame came in, read with a larger frame's
        // layout (a size change between the sample and its layout).
        let small = buffer(1280, 1280, 16);
        let big = layout((1280, 2160), (0, 0), (1280, 2160));
        assert!(copy_nv12(&small, 1280, &big).is_err());
        // Just short of the last chroma row's end.
        let src = buffer(6, 4, 4);
        let short = &src[..src.len() - 3];
        assert!(copy_nv12(short, 6, &layout((4, 4), (0, 0), (4, 4))).is_err());
        // The UV plane said to start past the end.
        assert!(copy_nv12(&src, 6, &layout((4, 40), (0, 0), (4, 4))).is_err());
        // Rows wider than the pitch would run into the next row.
        assert!(copy_nv12(&src, 3, &layout((4, 4), (0, 0), (4, 4))).is_err());
        assert!(copy_nv12(&src, 6, &layout((8, 4), (4, 0), (4, 4))).is_err());
        // Sizes whose offsets overflow: an error, no panic.
        let huge = layout((u32::MAX, u32::MAX), (0, u32::MAX - 1), (2, 2));
        assert!(copy_nv12(&src, usize::MAX / 2, &huge).is_err());
        assert!(copy_nv12(&src, 6, &layout((4, 4), (0, 0), (0, 4))).is_err());
        assert!(copy_nv12(&[], 6, &layout((4, 4), (0, 0), (4, 4))).is_err());
    }
}
