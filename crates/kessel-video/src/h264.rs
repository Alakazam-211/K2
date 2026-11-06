//! H.264 bitstream helpers: Annex B and AVCC framing, NAL unit types,
//! parameter sets and the codec strings for SDP and WebCodecs.

use crate::error::{invalid, VideoError};

/// The 4-byte Annex B start code.
pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// A coded slice of a non-IDR picture.
pub const NAL_SLICE: u8 = 1;
/// A coded slice of an IDR picture (a keyframe).
pub const NAL_IDR: u8 = 5;
/// Supplemental enhancement information.
pub const NAL_SEI: u8 = 6;
/// Sequence parameter set.
pub const NAL_SPS: u8 = 7;
/// Picture parameter set.
pub const NAL_PPS: u8 = 8;
/// Access unit delimiter.
pub const NAL_AUD: u8 = 9;

/// The type of a NAL unit (the low 5 bits of its first byte).
pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1f)
}

/// Append one NAL unit after a 4-byte start code.
pub fn push_annexb(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&START_CODE);
    out.extend_from_slice(nal);
}

/// Append one NAL unit after its 4-byte big-endian length.
pub fn push_avcc(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    out.extend_from_slice(nal);
}

/// Split an Annex B stream into NAL units (3- or 4-byte start codes).
/// Bytes before the first start code are ignored.
pub fn annexb_nals(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = if k + 1 < starts.len() { starts[k + 1] - 3 } else { stream.len() };
        // Trailing zeros belong to the next start code (or are padding).
        while end > s && stream[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&stream[s..end]);
        }
    }
    out
}

/// Split an AVCC sample (each NAL after a big-endian length of `len_size`
/// bytes, 1 to 4) into NAL units.
pub fn avcc_nals(sample: &[u8], len_size: usize) -> Result<Vec<&[u8]>, VideoError> {
    if !(1..=4).contains(&len_size) {
        return Err(invalid(format!("AVCC length size {len_size} is not 1 to 4")));
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < sample.len() {
        if i + len_size > sample.len() {
            return Err(invalid(format!("AVCC sample truncated at {i} of {}", sample.len())));
        }
        let mut n = 0usize;
        for b in &sample[i..i + len_size] {
            n = (n << 8) | *b as usize;
        }
        i += len_size;
        if n == 0 || i + n > sample.len() {
            return Err(invalid(format!(
                "AVCC NAL of {n} bytes at {i} overruns a {}-byte sample",
                sample.len()
            )));
        }
        out.push(&sample[i..i + n]);
        i += n;
    }
    Ok(out)
}

/// AVCC (lengths of `len_size` bytes) to Annex B, appended to `out`.
pub fn avcc_to_annexb(sample: &[u8], len_size: usize, out: &mut Vec<u8>) -> Result<(), VideoError> {
    for nal in avcc_nals(sample, len_size)? {
        push_annexb(out, nal);
    }
    Ok(())
}

/// Annex B to AVCC with 4-byte lengths.
pub fn annexb_to_avcc(stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len() + 8);
    for nal in annexb_nals(stream) {
        push_avcc(&mut out, nal);
    }
    out
}

/// Does this access unit hold an IDR slice?
pub fn is_keyframe(nals: &[&[u8]]) -> bool {
    nals.iter().any(|n| nal_type(n) == Some(NAL_IDR))
}

/// Does this Annex B access unit hold an IDR slice?
pub fn annexb_is_keyframe(stream: &[u8]) -> bool {
    is_keyframe(&annexb_nals(stream))
}

/// `profile-level-id` for SDP (RFC 6184): six hex digits from the SPS's
/// profile_idc, constraint flags and level_idc, for example `42e01f`.
pub fn profile_level_id(sps: &[u8]) -> Option<String> {
    (nal_type(sps) == Some(NAL_SPS) && sps.len() >= 4)
        .then(|| format!("{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
}

/// The WebCodecs / RFC 6381 codec string from the stream's first SPS:
/// `avc1.PPCCLL`, for example `avc1.42E01F`.
pub fn codec_string(nals: &[&[u8]]) -> Option<String> {
    let sps = nals.iter().find(|n| nal_type(n) == Some(NAL_SPS))?;
    profile_level_id(sps).map(|p| format!("avc1.{}", p.to_uppercase()))
}

/// Add emulation prevention bytes (`00 00 0x` with x <= 3 becomes
/// `00 00 03 0x`) so a payload can't contain a start code.
pub fn escape_rbsp(payload: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0;
    for &b in payload {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

/// Remove emulation prevention bytes.
pub fn unescape_rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_and_annexb_convert_both_ways() {
        let sample = [0, 0, 0, 3, 0x65, 0xaa, 0xbb, 0, 0, 0, 2, 0x41, 0xcc];
        let mut out = Vec::new();
        push_annexb(&mut out, &[0x67, 0x42, 0xe0, 0x1f, 0xac]);
        push_annexb(&mut out, &[0x68, 0xee]);
        avcc_to_annexb(&sample, 4, &mut out).expect("convert");
        let nals = annexb_nals(&out);
        assert_eq!(
            nals,
            vec![&[0x67, 0x42, 0xe0, 0x1f, 0xac][..], &[0x68, 0xee][..], &[0x65, 0xaa, 0xbb][..], &[0x41, 0xcc][..]]
        );
        assert!(is_keyframe(&nals));
        assert_eq!(codec_string(&nals).as_deref(), Some("avc1.42E01F"));
        assert_eq!(profile_level_id(nals[0]).as_deref(), Some("42e01f"));
        let avcc = annexb_to_avcc(&out);
        assert_eq!(avcc_nals(&avcc, 4).expect("split"), nals);
        // Three-byte start codes split too.
        assert_eq!(annexb_nals(&[0, 0, 1, 0x41, 1, 0, 0, 1, 0x41, 2]), vec![&[0x41, 1][..], &[0x41, 2][..]]);
    }

    #[test]
    fn bad_avcc_fails_loudly() {
        assert!(avcc_nals(&[0, 0, 0, 9, 1, 2], 4).is_err(), "overrun");
        assert!(avcc_nals(&[0, 0], 4).is_err(), "truncated length");
        assert!(avcc_nals(&[0, 0, 0, 0], 4).is_err(), "zero-length NAL");
        assert!(avcc_nals(&[1, 2], 5).is_err(), "length size");
    }

    #[test]
    fn deltas_are_not_keyframes() {
        let mut out = Vec::new();
        push_annexb(&mut out, &[0x41, 0x9a]);
        assert!(!annexb_is_keyframe(&out));
        assert_eq!(codec_string(&annexb_nals(&out)), None);
    }

    #[test]
    fn emulation_prevention_round_trips_and_hides_start_codes() {
        let payloads: [&[u8]; 4] = [&[0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 7], &[0, 0], &[], &[0, 0, 0, 0, 0, 0]];
        for p in payloads {
            let mut esc = Vec::new();
            escape_rbsp(p, &mut esc);
            for w in esc.windows(3) {
                assert!(!(w[0] == 0 && w[1] == 0 && w[2] <= 2), "a start code survived in {esc:?}");
            }
            assert_eq!(unescape_rbsp(&esc), p);
        }
    }
}
