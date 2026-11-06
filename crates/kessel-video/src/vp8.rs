//! VP8 through the pure-Rust `vp8-decoder` crate (feature `vp8`), as a
//! software codec in every backend. The encoder side plugs in the same way
//! when `vp8-encoder` lands.

use crate::api::{VideoDecoder, Wake};
use crate::error::{DecodeError, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::kit::{DecoderCore, ThreadedDecoder};
use crate::types::*;

/// The name in capability entries and stats.
pub(crate) const DECODER_NAME: &str = "vp8-decoder 0.1 (Rust)";

/// The VP8 entries every backend lists.
pub(crate) fn capabilities() -> Vec<CodecCapability> {
    let mut d = CodecCapability::new(VideoCodec::Vp8, CodecDirection::Decode, Implementation::Software, DECODER_NAME, 1920, 1080, 30);
    d.formats = vec![PixelFormat::I420, PixelFormat::Nv12, PixelFormat::Bgra, PixelFormat::Rgba];
    vec![d]
}

/// A VP8 decoder, if `config` asks for one this crate can make.
pub(crate) fn decoder(config: DecoderConfig, wake: Wake) -> Result<Box<dyn VideoDecoder>, VideoError> {
    if config.codec != VideoCodec::Vp8 {
        return Err(VideoError::Unsupported(format!("vp8::decoder for {:?}", config.codec)));
    }
    if config.implementation == ImplPreference::HardwareOnly {
        return Err(VideoError::Unsupported("no hardware VP8 decoder here".into()));
    }
    Ok(Box::new(ThreadedDecoder::start(
        Box::new(Vp8Core {
            dec: vp8_decoder::Decoder::new(),
        }),
        config,
        wake,
    )?))
}

struct Vp8Core {
    dec: vp8_decoder::Decoder,
}

fn map_err(e: vp8_decoder::DecodeError) -> DecodeError {
    match e {
        vp8_decoder::DecodeError::NeedKeyframe => DecodeError::NeedKeyframe,
        vp8_decoder::DecodeError::Truncated => DecodeError::Corrupt("truncated VP8 frame".into()),
        vp8_decoder::DecodeError::Invalid(why) => DecodeError::Corrupt(format!("invalid VP8 frame: {why}")),
        vp8_decoder::DecodeError::Unsupported(what) => DecodeError::Video(VideoError::Unsupported(format!("VP8: {what}"))),
        other => DecodeError::Corrupt(other.to_string()),
    }
}

impl DecoderCore for Vp8Core {
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError> {
        let Some(p) = self.dec.decode(&input.data).map_err(map_err)? else {
            return Ok(None);
        };
        let (w, h) = (p.width, p.height);
        let planes = Planes::tight(PixelFormat::I420, w, h, p.to_i420()).map_err(DecodeError::Video)?;
        VideoFrame::from_planes(planes, w, h, input.capture_us).map(Some).map_err(DecodeError::Video)
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
    use crate::kit::wait_for;
    use std::time::Duration;

    #[test]
    fn deltas_need_a_keyframe_and_garbage_is_corrupt() {
        let mut dec = decoder(DecoderConfig::new(VideoCodec::Vp8), no_wake()).expect("decoder");
        // An interframe tag first: the adapter refuses it before decoding.
        let delta = EncodedInput::new(VideoCodec::Vp8, vec![0x01u8, 0, 0, 0, 0, 0], false, 1);
        assert_eq!(dec.submit(delta), Err(DecodeError::NeedKeyframe));
        // A "keyframe" that isn't one: corrupt, then waiting again.
        let bad = EncodedInput::new(VideoCodec::Vp8, vec![0x00u8, 0x01, 0x00, 0xff, 0xff, 0xff, 16, 0, 16, 0], true, 2);
        dec.submit(bad).expect("queued");
        let r = wait_for(Duration::from_secs(2), || dec.poll()).expect("an answer");
        assert!(matches!(r, Err(DecodeError::Corrupt(_)) | Err(DecodeError::NeedKeyframe)), "{r:?}");
        assert!(dec.stats().errors >= 2);
        assert_eq!(dec.stats().implementation, Some(Implementation::Software));
    }

    #[test]
    fn the_capability_is_a_software_decoder() {
        let caps = capabilities();
        assert_eq!(caps.len(), 1);
        assert_eq!((caps[0].codec, caps[0].direction, caps[0].implementation), (VideoCodec::Vp8, CodecDirection::Decode, Implementation::Software));
        assert!(decoder(DecoderConfig::new(VideoCodec::H264), no_wake()).is_err());
    }
}
