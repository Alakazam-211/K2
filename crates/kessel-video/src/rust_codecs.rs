//! The pure-Rust codecs every backend offers next to the OS's own: VP8
//! decoding with the `vp8` feature (and VP8 encoding when that crate lands).

use crate::api::{VideoDecoder, Wake};
use crate::error::VideoError;
use crate::types::{Capabilities, DecoderConfig};

/// Add the Rust codecs' capability entries.
pub(crate) fn add_capabilities(caps: &mut Capabilities) {
    #[cfg(feature = "vp8")]
    caps.codecs.extend(crate::vp8::capabilities());
    let _ = caps;
}

/// A Rust decoder for `config`, or `None` when no Rust codec handles it.
pub(crate) fn decoder(config: &DecoderConfig, wake: &Wake) -> Option<Result<Box<dyn VideoDecoder>, VideoError>> {
    #[cfg(feature = "vp8")]
    if config.codec == crate::types::VideoCodec::Vp8 {
        return Some(crate::vp8::decoder(*config, wake.clone()));
    }
    let _ = (config, wake);
    None
}
