//! H.264 samples from length-prefixed (`avcC`) form to Annex B byte streams
//! (ITU-T H.264 Annex B), as kessel-video's decoder takes them.

use crate::config::AvcConfig;
use crate::error::{Error, FourCC, Result};

const START: [u8; 4] = [0, 0, 0, 1];
const MDAT: FourCC = FourCC(*b"mdat");

/// Converts one length-prefixed H.264 sample to Annex B: every NAL unit gets
/// a 4-byte start code, and the configuration's SPS and PPS are put before
/// the first IDR slice of the sample unless the sample already carries its
/// own SPS and PPS before it. `out` is cleared first.
///
/// Errors: a length prefix that runs past the sample (`Truncated`), or a
/// `length_size` that is not 1, 2 or 4 (`Invalid`). Zero-length NAL units
/// are dropped.
pub fn avcc_to_annexb(sample: &[u8], config: &AvcConfig, out: &mut Vec<u8>) -> Result<()> {
    out.clear();
    let ls = config.length_size as usize;
    if !matches!(ls, 1 | 2 | 4) {
        return Err(Error::Invalid { fourcc: FourCC(*b"avcC"), what: "length_size is not 1, 2 or 4" });
    }
    out.reserve(sample.len() + sample.len() / (ls + 1) * (4 - ls.min(4)) + 64);
    let (mut seen_sps, mut seen_pps, mut inserted) = (false, false, false);
    let mut pos = 0usize;
    while pos < sample.len() {
        if sample.len() - pos < ls {
            return Err(Error::Truncated(MDAT));
        }
        let mut len = 0usize;
        for &b in &sample[pos..pos + ls] {
            len = (len << 8) | b as usize;
        }
        pos += ls;
        if len > sample.len() - pos {
            return Err(Error::Truncated(MDAT));
        }
        let nal = &sample[pos..pos + len];
        pos += len;
        let Some(&h) = nal.first() else { continue };
        match h & 0x1f {
            7 => seen_sps = true,
            8 => seen_pps = true,
            5 if !inserted && !(seen_sps && seen_pps) => {
                for p in config.sps.iter().chain(config.pps.iter()) {
                    out.extend_from_slice(&START);
                    out.extend_from_slice(p);
                }
                inserted = true;
            }
            5 => inserted = true,
            _ => {}
        }
        out.extend_from_slice(&START);
        out.extend_from_slice(nal);
    }
    Ok(())
}
