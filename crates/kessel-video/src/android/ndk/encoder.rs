//! H.264 encoding through `AMediaCodec` ("video/avc") with ByteBuffer
//! input: NV12 (`COLOR_FormatYUV420SemiPlanar`), or I420 where the codec
//! takes only that. Synchronous: queue one picture, wait briefly for its
//! output.

use super::ffi::*;
use crate::android::codec::{
    access_unit, avc_level, implementation_of, input_format, parameter_sets, AvcProbe, InputLayout,
    COLOR_I420, COLOR_NV12,
};
use crate::android::meta::media_result;
use crate::error::VideoError;
use crate::frame::{FrameBuffer, PixelFormat, VideoFrame};
use crate::kit::{CoreFrame, EncoderCore};
use crate::types::*;
use std::ffi::CStr;
use std::time::{Duration, Instant};

const MIME: &CStr = c"video/avc";
/// Android's own software encoders, newest first (Codec2, then OMX).
const SOFTWARE: [&CStr; 2] = [c"c2.android.avc.encoder", c"OMX.google.h264.encoder"];
/// `BITRATE_MODE_CBR`: steady for a call.
const BITRATE_MODE_CBR: i32 = 2;
/// `AVCProfileBaseline` and `AVCProfileHigh`.
const AVC_PROFILE_BASELINE: i32 = 1;
const AVC_PROFILE_HIGH: i32 = 8;
/// How long `encode` waits for a free input buffer, and for the output.
const INPUT_WAIT_US: i64 = 20_000;
const OUTPUT_WAIT: Duration = Duration::from_millis(100);

/// Creates the encoders once, without configuring them, to see what the
/// device has: the component `video/avc` resolves to (by name), and the
/// platform's software encoder.
pub(crate) fn probe() -> AvcProbe {
    let default_name = create(false).and_then(|c| name_of(&c));
    let software = SOFTWARE.iter().find_map(|name| {
        // SAFETY: a NUL-terminated name; null when absent.
        let c = unsafe { AMediaCodec_createCodecByName(name.as_ptr()) };
        (!c.is_null()).then(|| {
            // SAFETY: created just above, never started; deleted once.
            unsafe { AMediaCodec_delete(c) };
            name.to_string_lossy().into_owned()
        })
    });
    AvcProbe {
        default_name,
        software,
    }
}

/// The component's name (`AMediaCodec_getName`, API 28).
fn name_of(codec: &Codec) -> Option<String> {
    let (get, release) = late().name?;
    let mut out: *mut std::ffi::c_char = std::ptr::null_mut();
    // SAFETY: a live codec and a valid out pointer.
    if unsafe { get(codec.0, &mut out) } != 0 || out.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated name the codec owns until released, copied
    // first and released once.
    let name = unsafe { CStr::from_ptr(out) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: as above.
    unsafe { release(codec.0, out) };
    Some(name)
}

/// An `AMediaFormat`, deleted on drop.
struct Format(*mut AMediaFormat);

impl Format {
    fn new() -> Result<Format, VideoError> {
        // SAFETY: no arguments; null on failure, checked.
        let f = unsafe { AMediaFormat_new() };
        if f.is_null() {
            return Err(VideoError::NoResources);
        }
        Ok(Format(f))
    }

    fn int(&self, key: &CStr, value: i32) -> &Format {
        // SAFETY: a live format and a NUL-terminated key.
        unsafe { AMediaFormat_setInt32(self.0, key.as_ptr(), value) };
        self
    }

    fn string(&self, key: &CStr, value: &CStr) -> &Format {
        // SAFETY: a live format; NUL-terminated key and value (copied).
        unsafe { AMediaFormat_setString(self.0, key.as_ptr(), value.as_ptr()) };
        self
    }

    fn get_int(&self, key: &CStr) -> Option<i32> {
        let mut v = 0;
        // SAFETY: a live format, a NUL-terminated key, a valid out pointer.
        unsafe { AMediaFormat_getInt32(self.0, key.as_ptr(), &mut v) }.then_some(v)
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: made by AMediaFormat_new (or handed over by the codec),
        // deleted once.
        unsafe { AMediaFormat_delete(self.0) };
    }
}

/// A started codec, stopped and deleted on drop.
struct Codec(*mut AMediaCodec);

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: a codec we created; stopping an unstarted codec is an
        // error the NDK reports and ignores.
        unsafe {
            AMediaCodec_stop(self.0);
            AMediaCodec_delete(self.0);
        }
    }
}

pub(crate) struct McEncoder {
    codec: Codec,
    cfg: EncoderConfig,
    layout: InputLayout,
    /// What the component is, by its name.
    implementation: Implementation,
    /// For logs: the component's name, when the device tells it.
    component: Option<String>,
    /// SPS and PPS from the codec-config output.
    sets: Vec<Vec<u8>>,
}

// SAFETY: the codec is used only from the encoder thread that owns this
// value (ThreadedEncoder moves it there once).
unsafe impl Send for McEncoder {}

fn create(software: bool) -> Option<Codec> {
    let codec = if software {
        SOFTWARE.iter().find_map(|name| {
            // SAFETY: a NUL-terminated name; null when absent.
            let c = unsafe { AMediaCodec_createCodecByName(name.as_ptr()) };
            (!c.is_null()).then_some(c)
        })?
    } else {
        // SAFETY: a NUL-terminated MIME type; null when there's no encoder.
        let c = unsafe { AMediaCodec_createEncoderByType(MIME.as_ptr()) };
        if c.is_null() {
            return None;
        }
        c
    };
    Some(Codec(codec))
}

fn format(cfg: &EncoderConfig, color: i32, profile: bool) -> Result<Format, VideoError> {
    let f = Format::new()?;
    f.string(c"mime", MIME)
        .int(c"width", cfg.width as i32)
        .int(c"height", cfg.height as i32)
        .int(c"bitrate", cfg.bitrate_bps.min(i32::MAX as u32) as i32)
        .int(c"bitrate-mode", BITRATE_MODE_CBR)
        .int(c"frame-rate", cfg.fps as i32)
        .int(c"color-format", color)
        // Keyframes come on request (`request-sync`); this is a fallback
        // the codec needs, in seconds.
        .int(c"i-frame-interval", 3600)
        // Real time, and no reordering where the codec knows the key.
        .int(c"priority", 0)
        .int(c"latency", 1)
        .int(c"max-bframes", 0);
    if let (true, CodecConfig::H264 { profile: p, level }) = (profile, cfg.codec) {
        let p = match p {
            H264Profile::ConstrainedHigh => AVC_PROFILE_HIGH,
            _ => AVC_PROFILE_BASELINE,
        };
        f.int(c"profile", p).int(c"level", avc_level(level.0));
    }
    Ok(f)
}

/// Configure and start `codec` for `cfg`: NV12 first, then I420, each
/// with the profile and level asked, then without them (some codecs refuse
/// a level they would pick anyway). The input layout the codec reports
/// (API 28), or tight.
fn start(codec: &Codec, cfg: &EncoderConfig) -> Result<InputLayout, VideoError> {
    let mut last = VideoError::Unsupported("no colour format".into());
    for (color, profile) in [
        (COLOR_NV12, true),
        (COLOR_I420, true),
        (COLOR_NV12, false),
        (COLOR_I420, false),
    ] {
        let f = format(cfg, color, profile)?;
        // SAFETY: a live codec and format; no surface, no crypto.
        let status = unsafe {
            AMediaCodec_configure(
                codec.0,
                f.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                AMEDIACODEC_CONFIGURE_FLAG_ENCODE,
            )
        };
        if let Err(e) = media_result(status, "AMediaCodec_configure") {
            last = e;
            continue;
        }
        let (mut stride, mut slice) = (0, 0);
        if let Some(get) = late().get_input_format {
            // SAFETY: a configured codec; the result is ours to delete.
            let input = unsafe { get(codec.0) };
            if !input.is_null() {
                let input = Format(input);
                stride = input.get_int(c"stride").unwrap_or(0);
                slice = input.get_int(c"slice-height").unwrap_or(0);
            }
        }
        // SAFETY: a configured codec.
        media_result(unsafe { AMediaCodec_start(codec.0) }, "AMediaCodec_start")?;
        return Ok(InputLayout::new(
            color, cfg.width, cfg.height, stride, slice,
        ));
    }
    Err(last)
}

impl McEncoder {
    pub(crate) fn open(cfg: &EncoderConfig) -> Result<McEncoder, VideoError> {
        if cfg.temporal_layers != 1 {
            return Err(VideoError::Unsupported(
                "temporal layers on the MediaCodec H.264 encoder".into(),
            ));
        }
        let software = cfg.implementation == ImplPreference::SoftwareOnly;
        let codec = create(software).ok_or_else(|| {
            VideoError::Unsupported(if software {
                "no software H.264 encoder on this device".into()
            } else {
                "no H.264 encoder on this device".into()
            })
        })?;
        let component = name_of(&codec);
        // A software codec asked for by name is software even where the
        // device can't name it.
        let implementation = if software {
            Implementation::Software
        } else {
            implementation_of(component.as_deref())
        };
        // Before configuring anything: `video/avc` resolved to a software
        // codec (the hardware one is busy) or to one that can't be named.
        if cfg.implementation == ImplPreference::HardwareOnly
            && implementation != Implementation::Hardware
        {
            return Err(VideoError::NoResources);
        }
        let layout = start(&codec, cfg)?;
        Ok(McEncoder {
            codec,
            cfg: *cfg,
            layout,
            implementation,
            component,
            sets: Vec::new(),
        })
    }

    fn set_params(&self, key: &CStr, value: i32) -> bool {
        let Some(set) = late().set_parameters else {
            return false;
        };
        let Ok(f) = Format::new() else {
            return false;
        };
        f.int(key, value);
        // SAFETY: a started codec and a live format.
        unsafe { set(self.codec.0, f.0) == 0 }
    }

    /// Stop and configure the same codec again (a keyframe follows): for
    /// a new size, or a keyframe where `request-sync` isn't available. The
    /// same codec, not a new one: a device may allow only one at a time.
    fn restart(&mut self) -> Result<(), VideoError> {
        // SAFETY: a started codec; stop returns it to the uninitialised
        // state, ready to configure.
        media_result(
            unsafe { AMediaCodec_stop(self.codec.0) },
            "AMediaCodec_stop",
        )?;
        self.sets.clear();
        self.layout = start(&self.codec, &self.cfg)?;
        Ok(())
    }
}

impl EncoderCore for McEncoder {
    fn input_format(&self) -> PixelFormat {
        input_format(self.layout.color)
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        keyframe: bool,
    ) -> Result<Option<CoreFrame>, VideoError> {
        let FrameBuffer::Planes(planes) = &frame.buffer else {
            return Err(VideoError::Invalid(
                "the MediaCodec encoder takes CPU planes".into(),
            ));
        };
        if keyframe && !self.set_params(c"request-sync", 0) {
            self.restart()?;
        }
        let codec = self.codec.0;
        // SAFETY: a started codec.
        let index = unsafe { AMediaCodec_dequeueInputBuffer(codec, INPUT_WAIT_US) };
        if index < 0 {
            // The codec is behind: skip this picture, as rate control would.
            return Ok(None);
        }
        let index = index as usize;
        let mut size = 0usize;
        // SAFETY: an input buffer index the codec just gave us; it owns the
        // buffer of `size` bytes until we queue it.
        let buf = unsafe { AMediaCodec_getInputBuffer(codec, index, &mut size) };
        if buf.is_null() {
            return Err(VideoError::os(
                0,
                "AMediaCodec_getInputBuffer returned null",
            ));
        }
        // SAFETY: as above; nothing else touches the buffer until it is
        // queued below.
        let dst = unsafe { std::slice::from_raw_parts_mut(buf, size) };
        let written = self.layout.fill(planes, dst)?;
        // SAFETY: the index we filled, `written` bytes from its start.
        media_result(
            unsafe { AMediaCodec_queueInputBuffer(codec, index, 0, written, frame.capture_us, 0) },
            "AMediaCodec_queueInputBuffer",
        )?;
        let deadline = Instant::now() + OUTPUT_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // Late output comes out of a later call.
                return Ok(None);
            }
            let mut info = AMediaCodecBufferInfo::default();
            // SAFETY: a started codec and a valid out pointer.
            let out = unsafe {
                AMediaCodec_dequeueOutputBuffer(codec, &mut info, left.as_micros() as i64)
            };
            match out {
                AMEDIACODEC_INFO_TRY_AGAIN_LATER
                | AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED
                | AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED => continue,
                i if i < 0 => {
                    return Err(VideoError::os(
                        i as i64,
                        "AMediaCodec_dequeueOutputBuffer failed",
                    ))
                }
                i => {
                    let i = i as usize;
                    let mut size = 0usize;
                    // SAFETY: an output index the codec just gave us.
                    let p = unsafe { AMediaCodec_getOutputBuffer(codec, i, &mut size) };
                    let (off, len) = (info.offset.max(0) as usize, info.size.max(0) as usize);
                    let data = if p.is_null() || off + len > size {
                        None
                    } else {
                        // SAFETY: `size` bytes at `p`, valid until released
                        // below; copied before that.
                        Some(unsafe { std::slice::from_raw_parts(p.add(off), len) }.to_vec())
                    };
                    // SAFETY: the index we dequeued, released once.
                    unsafe { AMediaCodec_releaseOutputBuffer(codec, i, false) };
                    let Some(data) = data else {
                        return Err(VideoError::os(0, "an encoder output outside its buffer"));
                    };
                    if info.flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG != 0 {
                        self.sets = parameter_sets(&data);
                        continue;
                    }
                    if data.is_empty() {
                        continue;
                    }
                    let key = info.flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME != 0;
                    let (au, key) = access_unit(&data, key, &self.sets);
                    return Ok(Some(CoreFrame::new(au, key)));
                }
            }
        }
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        self.cfg.bitrate_bps = rates.bitrate_bps;
        self.cfg.max_bitrate_bps = self.cfg.max_bitrate_bps.max(rates.bitrate_bps);
        self.cfg.fps = rates.fps;
        // API 26+; older devices keep the rate they opened with until the
        // next restart.
        self.set_params(
            c"video-bitrate",
            rates.bitrate_bps.min(i32::MAX as u32) as i32,
        );
        Ok(())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        self.cfg.width = width;
        self.cfg.height = height;
        self.restart()
    }

    fn implementation(&self) -> Implementation {
        self.implementation
    }

    fn name(&self) -> String {
        format!(
            "MediaCodec {} ({:?})",
            self.component.as_deref().unwrap_or("video/avc"),
            input_format(self.layout.color)
        )
    }
}
