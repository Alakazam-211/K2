//! VP8 through the pure-Rust `vp8-encoder` and `vp8-decoder` crates
//! (feature `vp8`), as software codecs in every backend.

use crate::api::{VideoDecoder, VideoEncoder, Wake};
use crate::error::{DecodeError, VideoError};
use crate::frame::{FrameBuffer, PixelFormat, Planes, VideoFrame};
use crate::kit::{CoreFrame, DecoderCore, EncoderCore, ThreadedDecoder, ThreadedEncoder};
use crate::types::*;

/// The names in capability entries and stats.
pub(crate) const DECODER_NAME: &str = "vp8-decoder 0.1 (Rust)";
pub(crate) const ENCODER_NAME: &str = "vp8-encoder 0.1 (Rust)";

/// The VP8 entries every backend lists.
pub(crate) fn capabilities() -> Vec<CodecCapability> {
    let mut e = CodecCapability::new(
        VideoCodec::Vp8,
        CodecDirection::Encode,
        Implementation::Software,
        ENCODER_NAME,
        1920,
        1080,
        30,
    );
    e.max_temporal_layers = 3;
    e.formats = vec![
        PixelFormat::I420,
        PixelFormat::Nv12,
        PixelFormat::Bgra,
        PixelFormat::Rgba,
        PixelFormat::Native,
    ];
    let mut d = CodecCapability::new(
        VideoCodec::Vp8,
        CodecDirection::Decode,
        Implementation::Software,
        DECODER_NAME,
        1920,
        1080,
        30,
    );
    d.formats = vec![
        PixelFormat::I420,
        PixelFormat::Nv12,
        PixelFormat::Bgra,
        PixelFormat::Rgba,
    ];
    vec![e, d]
}

/// A VP8 encoder, if `config` asks for one this crate can make.
pub(crate) fn encoder(
    config: EncoderConfig,
    wake: Wake,
) -> Result<Box<dyn VideoEncoder>, VideoError> {
    config.validate()?;
    if config.codec != CodecConfig::Vp8 {
        return Err(VideoError::Unsupported(format!(
            "vp8::encoder for {:?}",
            config.codec.codec()
        )));
    }
    if config.implementation == ImplPreference::HardwareOnly {
        return Err(VideoError::Unsupported(
            "no hardware VP8 encoder here".into(),
        ));
    }
    let core = Vp8Encoder::open(&config)?;
    Ok(Box::new(ThreadedEncoder::start(
        Box::new(core),
        config,
        wake,
    )?))
}

struct Vp8Encoder {
    enc: vp8_encoder::Encoder,
    cfg: EncoderConfig,
}

fn settings(cfg: &EncoderConfig) -> vp8_encoder::EncoderConfig {
    let layers = match cfg.temporal_layers {
        2 => vp8_encoder::TemporalLayers::L1T2,
        3 => vp8_encoder::TemporalLayers::L1T3,
        _ => vp8_encoder::TemporalLayers::L1T1,
    };
    let mut s = vp8_encoder::EncoderConfig::new(cfg.width, cfg.height, cfg.fps, cfg.bitrate_bps)
        .with_temporal_layers(layers)
        .with_keyframe_interval(cfg.keyframe_interval);
    s.max_bitrate_bps = cfg.max_bitrate_bps;
    s
}

impl Vp8Encoder {
    fn open(cfg: &EncoderConfig) -> Result<Vp8Encoder, VideoError> {
        let enc = vp8_encoder::Encoder::new(settings(cfg))
            .map_err(|e| VideoError::Unsupported(format!("VP8: {e}")))?;
        Ok(Vp8Encoder { enc, cfg: *cfg })
    }
}

impl EncoderCore for Vp8Encoder {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::I420
    }

    fn encode(
        &mut self,
        frame: &VideoFrame,
        keyframe: bool,
    ) -> Result<Option<CoreFrame>, VideoError> {
        let FrameBuffer::Planes(p) = &frame.buffer else {
            return Err(VideoError::Invalid(
                "the VP8 encoder takes CPU planes".into(),
            ));
        };
        let tight = p.to_tight(frame.width, frame.height);
        let pic = vp8_encoder::Picture::from_i420(&tight.data, frame.width, frame.height)
            .ok_or_else(|| {
                VideoError::Invalid(format!(
                    "an I420 picture of {}x{}",
                    frame.width, frame.height
                ))
            })?;
        let out = self.enc.encode(&pic, keyframe).map_err(|e| match e {
            vp8_encoder::EncodeError::Internal(why) => {
                VideoError::os(0, format!("VP8 encoder: {why}"))
            }
            other => VideoError::Invalid(other.to_string()),
        })?;
        Ok(out.map(|f| {
            let mut cf = CoreFrame::new(f.data, f.keyframe);
            cf.layer = LayerInfo::new(f.temporal_id, f.layer_sync);
            cf
        }))
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        self.enc.set_rates(rates.bitrate_bps, rates.fps);
        self.cfg.bitrate_bps = rates.bitrate_bps;
        self.cfg.fps = rates.fps;
        Ok(())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        let mut cfg = self.cfg;
        cfg.width = width;
        cfg.height = height;
        *self = Vp8Encoder::open(&cfg)?;
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        Implementation::Software
    }

    fn name(&self) -> String {
        ENCODER_NAME.into()
    }
}

/// A VP8 decoder, if `config` asks for one this crate can make.
pub(crate) fn decoder(
    config: DecoderConfig,
    wake: Wake,
) -> Result<Box<dyn VideoDecoder>, VideoError> {
    if config.codec != VideoCodec::Vp8 {
        return Err(VideoError::Unsupported(format!(
            "vp8::decoder for {:?}",
            config.codec
        )));
    }
    if config.implementation == ImplPreference::HardwareOnly {
        return Err(VideoError::Unsupported(
            "no hardware VP8 decoder here".into(),
        ));
    }
    Ok(Box::new(ThreadedDecoder::start(
        Box::new(Vp8Core::new()),
        config,
        wake,
    )?))
}

/// The decoder behind `decoder`'s thread. Its input comes from the network:
/// see `hostile_tests`.
pub(crate) struct Vp8Core {
    dec: vp8_decoder::Decoder,
}

impl Vp8Core {
    pub(crate) fn new() -> Vp8Core {
        Vp8Core {
            dec: vp8_decoder::Decoder::new(),
        }
    }
}

fn map_err(e: vp8_decoder::DecodeError) -> DecodeError {
    match e {
        vp8_decoder::DecodeError::NeedKeyframe => DecodeError::NeedKeyframe,
        vp8_decoder::DecodeError::Truncated => DecodeError::Corrupt("truncated VP8 frame".into()),
        vp8_decoder::DecodeError::Invalid(why) => {
            DecodeError::Corrupt(format!("invalid VP8 frame: {why}"))
        }
        vp8_decoder::DecodeError::Unsupported(what) => {
            DecodeError::Video(VideoError::Unsupported(format!("VP8: {what}")))
        }
        other => DecodeError::Corrupt(other.to_string()),
    }
}

impl DecoderCore for Vp8Core {
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError> {
        let Some(p) = self.dec.decode(&input.data).map_err(map_err)? else {
            return Ok(None);
        };
        let (w, h) = (p.width, p.height);
        let planes =
            Planes::tight(PixelFormat::I420, w, h, p.to_i420()).map_err(DecodeError::Video)?;
        VideoFrame::from_planes(planes, w, h, input.capture_us)
            .map(Some)
            .map_err(DecodeError::Video)
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        self.dec.reset();
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        Implementation::Software
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;
    use crate::fake::{pattern_frame, read_frame_code};
    use crate::kit::wait_for;
    use std::time::Duration;

    fn encode(
        enc: &mut Box<dyn VideoEncoder>,
        f: &VideoFrame,
        opts: EncodeOptions,
    ) -> Option<EncodedFrame> {
        let skipped = enc.stats().dropped_by_rc;
        enc.submit(f, opts).expect("submit");
        let r = wait_for(Duration::from_secs(10), || match enc.poll() {
            Some(r) => Some(Some(r)),
            None if enc.stats().dropped_by_rc > skipped => Some(None),
            None => None,
        })
        .expect("an output or a skip within 10 s");
        r.map(|r| r.expect("encode"))
    }

    #[test]
    fn vp8_round_trips_with_the_frame_code_and_follows_the_rules() {
        let mut cfg = EncoderConfig::new(CodecConfig::Vp8, 640, 360, 30, 1_200_000);
        cfg.temporal_layers = 3;
        let mut enc = encoder(cfg, no_wake()).expect("encoder");
        assert_eq!(enc.info().name, ENCODER_NAME);
        let mut dec = decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake()).expect("decoder");
        let mut keys = Vec::new();
        let mut tids = Vec::new();
        for n in 0..40u32 {
            if n == 20 {
                enc.request_keyframe();
            }
            if n == 10 {
                enc.set_rates(Rates::new(600_000, 30)).expect("rates");
            }
            let f = pattern_frame(1280, 720, n, 1_000 + n as u64 * 33_333).expect("frame");
            let Some(e) = encode(&mut enc, &f, EncodeOptions::default()) else {
                continue;
            };
            assert_eq!(e.codec, VideoCodec::Vp8);
            assert_eq!(
                e.capture_us,
                1_000 + n as u64 * 33_333,
                "capture_us passes through"
            );
            assert_eq!((e.width, e.height), (640, 360));
            // RFC 6386: the P bit (bit 0 of the frame tag) is 0 on a
            // keyframe, which also carries the start code 9d 01 2a.
            assert_eq!(e.keyframe, e.data[0] & 1 == 0);
            if e.keyframe {
                assert_eq!(&e.data[3..6], &[0x9d, 0x01, 0x2a]);
                keys.push(n);
            }
            tids.push(e.layer.temporal_id);
            dec.submit(EncodedInput::new(
                VideoCodec::Vp8,
                e.data.clone(),
                e.keyframe,
                e.capture_us,
            ))
            .expect("decode submit");
            let d = wait_for(Duration::from_secs(10), || dec.poll())
                .expect("decoded")
                .expect("decode");
            assert_eq!((d.width, d.height), (640, 360));
            assert_eq!(d.capture_us, e.capture_us);
            assert_eq!(
                read_frame_code(&d),
                Some(n),
                "the frame code survives VP8 at frame {n}"
            );
        }
        // A skipped keyframe is owed to the next frame, so 20 or 21.
        assert!(
            keys.len() == 2 && keys[0] == 0 && (20..=21).contains(&keys[1]),
            "the first and the requested keyframe only; set_rates made none: {keys:?}"
        );
        assert_eq!(&tids[..4], &[0, 2, 1, 2], "L1T3's pattern");
        let st = enc.stats();
        assert_eq!(st.keyframes, 2);
        assert!(st.frames_out >= 36, "{st:?}");
    }

    #[test]
    fn decoder_gates_and_reports_corrupt_frames() {
        let mut dec = decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake()).expect("decoder");
        let delta = EncodedInput::new(VideoCodec::Vp8, vec![0x01u8, 0, 0, 0, 0, 0], false, 1);
        assert_eq!(dec.submit(delta), Err(DecodeError::NeedKeyframe));
        let bad = EncodedInput::new(
            VideoCodec::Vp8,
            vec![0x00u8, 0x01, 0x00, 0xff, 0xff, 0xff, 16, 0, 16, 0],
            true,
            2,
        );
        dec.submit(bad).expect("queued");
        let r = wait_for(Duration::from_secs(2), || dec.poll()).expect("an answer");
        assert!(matches!(r, Err(DecodeError::Corrupt(_))), "{r:?}");
        assert!(dec.stats().errors >= 2);
        assert_eq!(dec.stats().implementation, Some(Implementation::Software));
    }

    #[test]
    fn capabilities_and_refusals() {
        let caps = capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps
            .iter()
            .all(|c| c.codec == VideoCodec::Vp8 && c.implementation == Implementation::Software));
        assert_eq!(caps[0].max_temporal_layers, 3);
        assert!(decoder(DecoderConfig::new(VideoCodec::H264), no_wake()).is_err());
        assert!(encoder(EncoderConfig::h264(640, 360, 30, 500_000), no_wake()).is_err());
        let mut hw = EncoderConfig::new(CodecConfig::Vp8, 640, 360, 30, 500_000);
        hw.implementation = ImplPreference::HardwareOnly;
        assert!(matches!(
            encoder(hw, no_wake()).err(),
            Some(VideoError::Unsupported(_))
        ));
    }
}
