//! H.264 byte-stream helpers. VideoToolbox hands out AVCC samples
//! (length-prefixed NAL units, parameter sets in the format description);
//! the wire carries Annex B (start codes, SPS + PPS in band before every
//! keyframe), which WebCodecs decodes when it is configured with no
//! `description`.

pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// NAL unit types we care about.
pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

/// Append one NAL unit with a 4-byte start code.
pub fn push_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&START_CODE);
    out.extend_from_slice(nal);
}

/// Convert an AVCC sample (each NAL prefixed by a big-endian length of
/// `len_size` bytes) to Annex B, appending to `out`.
pub fn avcc_to_annexb(sample: &[u8], len_size: usize, out: &mut Vec<u8>) -> Result<(), String> {
    if !(1..=4).contains(&len_size) {
        return Err(format!("AVCC length size {len_size} is not 1–4"));
    }
    let mut i = 0usize;
    while i < sample.len() {
        if i + len_size > sample.len() {
            return Err(format!("AVCC sample truncated at {i} of {}", sample.len()));
        }
        let mut n = 0usize;
        for b in &sample[i..i + len_size] {
            n = (n << 8) | *b as usize;
        }
        i += len_size;
        if n == 0 || i + n > sample.len() {
            return Err(format!("AVCC NAL of {n} bytes at {i} overruns a {}-byte sample", sample.len()));
        }
        push_nal(out, &sample[i..i + n]);
        i += n;
    }
    Ok(())
}

/// Split an Annex B stream into NAL units (3- or 4-byte start codes).
pub fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
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
        // A 4-byte start code's leading zero belongs to the next code.
        while end > s && k + 1 < starts.len() && stream[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&stream[s..end]);
        }
    }
    out
}

pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1f)
}

/// Does this access unit hold an IDR slice?
pub fn is_keyframe(stream: &[u8]) -> bool {
    nal_units(stream).iter().any(|n| nal_type(n) == Some(NAL_IDR))
}

/// The WebCodecs / RFC 6381 codec string from the first SPS:
/// `avc1.PPCCLL` (profile_idc, constraint flags, level_idc).
pub fn codec_string(stream: &[u8]) -> Option<String> {
    let sps = nal_units(stream).into_iter().find(|n| nal_type(n) == Some(NAL_SPS))?;
    if sps.len() < 4 {
        return None;
    }
    Some(format!("avc1.{:02X}{:02X}{:02X}", sps[1], sps[2], sps[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_becomes_annexb_and_splits_back() {
        let sample = [0, 0, 0, 3, 0x65, 0xaa, 0xbb, 0, 0, 0, 2, 0x41, 0xcc];
        let mut out = Vec::new();
        push_nal(&mut out, &[0x67, 0x64, 0x00, 0x1f, 0xac]);
        push_nal(&mut out, &[0x68, 0xee]);
        avcc_to_annexb(&sample, 4, &mut out).expect("convert");
        let nals = nal_units(&out);
        assert_eq!(nals, vec![&[0x67, 0x64, 0x00, 0x1f, 0xac][..], &[0x68, 0xee][..], &[0x65, 0xaa, 0xbb][..], &[0x41, 0xcc][..]]);
        assert!(is_keyframe(&out));
        assert_eq!(codec_string(&out).as_deref(), Some("avc1.64001F"));
        // Three-byte start codes split too.
        assert_eq!(nal_units(&[0, 0, 1, 0x41, 1, 0, 0, 1, 0x41, 2]), vec![&[0x41, 1][..], &[0x41, 2][..]]);
    }

    #[test]
    fn bad_avcc_fails_loudly() {
        let mut out = Vec::new();
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 1, 2], 4, &mut out).is_err(), "overrun");
        assert!(avcc_to_annexb(&[0, 0], 4, &mut out).is_err(), "truncated length");
        assert!(avcc_to_annexb(&[0, 0, 0, 0], 4, &mut out).is_err(), "zero-length NAL");
        assert!(avcc_to_annexb(&[1, 2], 5, &mut out).is_err(), "length size");
    }

    #[test]
    fn delta_frames_are_not_keyframes_and_have_no_codec() {
        let mut out = Vec::new();
        push_nal(&mut out, &[0x41, 0x9a]);
        assert!(!is_keyframe(&out));
        assert_eq!(codec_string(&out), None);
    }
}
