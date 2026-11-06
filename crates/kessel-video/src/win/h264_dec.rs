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
            if x + w > alloc_w || y + h > alloc_h || w == 0 || h == 0 {
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

    fn copy_out(&self, sample: &IMFSample, l: Layout) -> Result<Planes, VideoError> {
        let (w, h) = (l.w as usize, l.h as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = Vec::with_capacity(PixelFormat::Nv12.frame_len(l.w, l.h));
        // SAFETY: the buffer is locked while we read rows inside it (checked
        // against its length), then unlocked.
        unsafe {
            let buf = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| map_error(&e, "ConvertToContiguousBuffer"))?;
            let two_d = buf.cast::<IMF2DBuffer>().ok();
            let mut p: *mut u8 = std::ptr::null_mut();
            let (pitch, len) = match &two_d {
                Some(b2) => {
                    let mut pitch = 0i32;
                    b2.Lock2D(&mut p, &mut pitch)
                        .map_err(|e| map_error(&e, "Lock2D"))?;
                    // A 2D buffer spans pitch x (rows of Y + rows of UV).
                    let rows = l.alloc_h as usize + (l.alloc_h as usize).div_ceil(2);
                    (pitch, pitch.max(0) as usize * rows)
                }
                None => {
                    let mut n = 0u32;
                    buf.Lock(&mut p, None, Some(&mut n))
                        .map_err(|e| map_error(&e, "Lock"))?;
                    (l.stride as i32, n as usize)
                }
            };
            let unlock = || match &two_d {
                Some(b2) => {
                    let _ = b2.Unlock2D();
                }
                None => {
                    let _ = buf.Unlock();
                }
            };
            let pitch_u = pitch.max(0) as usize;
            let (x, y) = (l.x as usize, l.y as usize);
            let uv_at = pitch_u * l.alloc_h as usize;
            let last_y = (y + h - 1) * pitch_u + x + w;
            let last_uv = uv_at + (y / 2 + ch - 1) * pitch_u + (x & !1) + cw * 2;
            if p.is_null() || pitch_u < l.alloc_w as usize || last_y > len || last_uv > len {
                unlock();
                return Err(VideoError::os(
                    0,
                    format!(
                        "an NV12 output of {len} bytes, pitch {pitch}, for {}x{}",
                        l.alloc_w, l.alloc_h
                    ),
                ));
            }
            let src = std::slice::from_raw_parts(p, len);
            for row in 0..h {
                let at = (y + row) * pitch_u + x;
                out.extend_from_slice(&src[at..at + w]);
            }
            for row in 0..ch {
                let at = uv_at + (y / 2 + row) * pitch_u + (x & !1);
                out.extend_from_slice(&src[at..at + cw * 2]);
            }
            unlock();
        }
        Planes::tight(PixelFormat::Nv12, l.w, l.h, out)
    }

    fn decode_inner(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, VideoError> {
        com_on_this_thread()?;
        let time =
            ((input.capture_us.min(i64::MAX as u64 / 10) * 10) as i64).max(self.last_time + 1);
        self.last_time = time;
        let sample = sample_from(&input.data, time, 333_333)?;
        let out_id = self.mft.out_id();
        let mut changed = false;
        let outs = self.mft.process(&sample, NoOutput::Wait, &mut |t| {
            changed = true;
            set_nv12_output(t, out_id)
        })?;
        if changed {
            self.layout = None;
        }
        // Low latency gives one frame per access unit; should more come out
        // together, the newest is the one to show.
        let Some(last) = outs.last() else {
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
        let planes = self.copy_out(last, layout)?;
        Ok(Some(VideoFrame::from_planes(
            planes,
            layout.w,
            layout.h,
            input.capture_us,
        )?))
    }
}

fn open_one(found: &Found) -> Result<Mft, VideoError> {
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
