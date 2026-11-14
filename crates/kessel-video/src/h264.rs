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
        let mut end = if k + 1 < starts.len() {
            starts[k + 1] - 3
        } else {
            stream.len()
        };
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
        return Err(invalid(format!(
            "AVCC length size {len_size} is not 1 to 4"
        )));
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < sample.len() {
        if i + len_size > sample.len() {
            return Err(invalid(format!(
                "AVCC sample truncated at {i} of {}",
                sample.len()
            )));
        }
        let mut n = 0usize;
        for b in &sample[i..i + len_size] {
            n = (n << 8) | *b as usize;
        }
        i += len_size;
        // Not `i + n > len`: a 4-byte length can overflow that sum on a
        // 32-bit target. `i <= len` here, so this can't underflow.
        if n == 0 || n > sample.len() - i {
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

// ───────────────────────────── sequence parameter sets ─────────────────────────────

/// What a sequence parameter set says that a decoder needs before it
/// decodes: profile and level, sampling and bit depth, size, and how many
/// frames may come out of order. From [`parse_sps`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SpsInfo {
    /// `profile_idc`: 66 Baseline, 77 Main, 100 High, 110 High 10, …
    pub profile_idc: u8,
    /// The constraint flags byte (`constraint_set0_flag` is the top bit).
    pub constraint_flags: u8,
    /// `level_idc`: 31 is level 3.1.
    pub level_idc: u8,
    /// 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma_format_idc: u8,
    /// Luma bit depth (8 to 14).
    pub bit_depth_luma: u8,
    /// Chroma bit depth (8 to 14).
    pub bit_depth_chroma: u8,
    /// Width in macroblocks.
    pub width_mbs: u32,
    /// Frame height in macroblocks (both fields of an interlaced frame).
    pub height_mbs: u32,
    /// Every picture is a frame (no fields).
    pub frame_mbs_only: bool,
    /// The picture's width in pixels, after cropping.
    pub width: u32,
    /// The picture's height in pixels, after cropping.
    pub height: u32,
    /// The VUI's `max_num_reorder_frames`, when the SPS has one.
    pub max_num_reorder_frames: Option<u32>,
    /// The VUI's `max_dec_frame_buffering`, when the SPS has one.
    pub max_dec_frame_buffering: Option<u32>,
}

impl SpsInfo {
    /// `MaxDpbFrames` (H.264 A.3.1): how many frames the level's decoded
    /// picture buffer holds at this size, at most 16. A level it doesn't
    /// know, or a frame larger than its level allows, gives 16.
    pub fn max_dpb_frames(&self) -> u32 {
        let level_1b = self.level_idc == 11 && self.constraint_flags & 0x10 != 0;
        let max_dpb_mbs: u64 = match self.level_idc {
            9 => 396,
            11 if level_1b && matches!(self.profile_idc, 66 | 77 | 88) => 396,
            10 => 396,
            11 => 900,
            12 | 13 | 20 => 2376,
            21 => 4752,
            22 | 30 => 8100,
            31 => 18000,
            32 => 20480,
            40 | 41 => 32768,
            42 => 34816,
            50 => 110400,
            51 | 52 => 184320,
            60..=62 => 696320,
            _ => return 16,
        };
        let frame = self.width_mbs as u64 * self.height_mbs as u64;
        match max_dpb_mbs.checked_div(frame) {
            Some(0) | None => 16,
            Some(n) => n.min(16) as u32,
        }
    }

    /// How many frames may precede a frame in decode order and follow it
    /// in presentation order: the VUI's `max_num_reorder_frames` when it is
    /// there, 0 for the intra-only profiles, else [`max_dpb_frames`]
    /// (H.264 E.2.1's inference). At most 16.
    ///
    /// [`max_dpb_frames`]: SpsInfo::max_dpb_frames
    pub fn reorder_depth(&self) -> u32 {
        if let Some(n) = self.max_num_reorder_frames {
            return n.min(16);
        }
        let intra = self.constraint_flags & 0x10 != 0
            && matches!(self.profile_idc, 44 | 86 | 100 | 110 | 122 | 244);
        if intra {
            0
        } else {
            self.max_dpb_frames()
        }
    }
}

/// Reads bits of an RBSP (emulation prevention bytes already removed).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

fn short() -> VideoError {
    invalid("SPS truncated")
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32, VideoError> {
        let byte = *self.data.get(self.pos / 8).ok_or_else(short)?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(b as u32)
    }

    fn flag(&mut self) -> Result<bool, VideoError> {
        Ok(self.bit()? == 1)
    }

    /// Up to 32 bits, most significant first.
    fn bits(&mut self, n: u32) -> Result<u32, VideoError> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    /// Exp-Golomb `ue(v)`, at most 2^32 - 2.
    fn ue(&mut self) -> Result<u32, VideoError> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(invalid("an Exp-Golomb code over 32 bits in the SPS"));
            }
        }
        let rest = self.bits(zeros)? as u64;
        Ok(((1u64 << zeros) - 1 + rest) as u32)
    }

    /// `ue(v)` no larger than `max`.
    fn ue_max(&mut self, max: u32, what: &str) -> Result<u32, VideoError> {
        let v = self.ue()?;
        if v > max {
            return Err(invalid(format!("SPS {what} {v} (at most {max})")));
        }
        Ok(v)
    }

    /// Exp-Golomb `se(v)`.
    fn se(&mut self) -> Result<i64, VideoError> {
        let k = self.ue()? as i64;
        Ok(if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

/// Skip one `scaling_list()` of `size` entries.
fn skip_scaling_list(r: &mut Bits<'_>, size: usize) -> Result<(), VideoError> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            let delta = r.se()?;
            if !(-128..=127).contains(&delta) {
                return Err(invalid(format!("SPS scaling list delta {delta}")));
            }
            next = (last + delta).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
}

/// Skip `hrd_parameters()`.
fn skip_hrd(r: &mut Bits<'_>) -> Result<(), VideoError> {
    let cpb_cnt = r.ue_max(31, "cpb_cnt_minus1")? + 1;
    r.bits(8)?; // bit_rate_scale, cpb_size_scale
    for _ in 0..cpb_cnt {
        r.ue()?; // bit_rate_value_minus1
        r.ue()?; // cpb_size_value_minus1
        r.bit()?; // cbr_flag
    }
    r.bits(20)?; // four 5-bit delay and length fields
    Ok(())
}

/// The VUI's `(max_num_reorder_frames, max_dec_frame_buffering)`, when it
/// has a bitstream restriction.
fn vui_reorder(r: &mut Bits<'_>) -> Result<Option<(u32, u32)>, VideoError> {
    if r.flag()? {
        // aspect_ratio_info: Extended_SAR carries its own 16:16.
        if r.bits(8)? == 255 {
            r.bits(32)?;
        }
    }
    if r.flag()? {
        r.bit()?; // overscan_appropriate_flag
    }
    if r.flag()? {
        r.bits(4)?; // video_format, video_full_range_flag
        if r.flag()? {
            r.bits(24)?; // colour primaries, transfer, matrix
        }
    }
    if r.flag()? {
        r.ue()?; // chroma_sample_loc_type_top_field
        r.ue()?; // and bottom
    }
    if r.flag()? {
        r.bits(32)?; // num_units_in_tick
        r.bits(32)?; // time_scale
        r.bit()?; // fixed_frame_rate_flag
    }
    let nal_hrd = r.flag()?;
    if nal_hrd {
        skip_hrd(r)?;
    }
    let vcl_hrd = r.flag()?;
    if vcl_hrd {
        skip_hrd(r)?;
    }
    if nal_hrd || vcl_hrd {
        r.bit()?; // low_delay_hrd_flag
    }
    r.bit()?; // pic_struct_present_flag
    if !r.flag()? {
        return Ok(None);
    }
    r.bit()?; // motion_vectors_over_pic_boundaries_flag
    for _ in 0..4 {
        r.ue()?; // max bytes and bits, the two log2 mv lengths
    }
    let reorder = r.ue()?;
    let buffering = r.ue()?;
    Ok(Some((reorder, buffering)))
}

/// Parse an SPS NAL unit (its header byte first, emulation prevention bytes
/// in place). `Invalid` for one that isn't an SPS, is truncated, or names
/// values H.264 doesn't allow (more than 2048 macroblocks a side, a bit
/// depth over 14). The VUI is read only for its reorder limits: a VUI that
/// doesn't parse leaves them `None`.
pub fn parse_sps(nal: &[u8]) -> Result<SpsInfo, VideoError> {
    if nal_type(nal) != Some(NAL_SPS) {
        return Err(invalid("not an SPS NAL unit"));
    }
    let rbsp = unescape_rbsp(&nal[1..]);
    let mut r = Bits {
        data: &rbsp,
        pos: 0,
    };
    let profile_idc = r.bits(8)? as u8;
    let constraint_flags = r.bits(8)? as u8;
    let level_idc = r.bits(8)? as u8;
    r.ue_max(31, "seq_parameter_set_id")?;
    let (mut chroma_format_idc, mut bit_depth_luma, mut bit_depth_chroma) = (1u8, 8u8, 8u8);
    let mut separate_planes = false;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = r.ue_max(3, "chroma_format_idc")? as u8;
        if chroma_format_idc == 3 {
            separate_planes = r.flag()?;
        }
        bit_depth_luma = 8 + r.ue_max(6, "bit_depth_luma_minus8")? as u8;
        bit_depth_chroma = 8 + r.ue_max(6, "bit_depth_chroma_minus8")? as u8;
        r.bit()?; // qpprime_y_zero_transform_bypass_flag
        if r.flag()? {
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.flag()? {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    r.ue_max(12, "log2_max_frame_num_minus4")?;
    match r.ue_max(2, "pic_order_cnt_type")? {
        0 => {
            r.ue_max(12, "log2_max_pic_order_cnt_lsb_minus4")?;
        }
        1 => {
            r.bit()?; // delta_pic_order_always_zero_flag
            r.se()?; // offset_for_non_ref_pic
            r.se()?; // offset_for_top_to_bottom_field
            let n = r.ue_max(255, "num_ref_frames_in_pic_order_cnt_cycle")?;
            for _ in 0..n {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue_max(16, "max_num_ref_frames")?;
    r.bit()?; // gaps_in_frame_num_value_allowed_flag
    let width_mbs = r.ue_max(2047, "pic_width_in_mbs_minus1")? + 1;
    let map_units = r.ue_max(2047, "pic_height_in_map_units_minus1")? + 1;
    let frame_mbs_only = r.flag()?;
    if !frame_mbs_only {
        r.bit()?; // mb_adaptive_frame_field_flag
    }
    let height_mbs = map_units * if frame_mbs_only { 1 } else { 2 };
    if height_mbs > 2048 {
        return Err(invalid(format!("an SPS {height_mbs} macroblocks high")));
    }
    r.bit()?; // direct_8x8_inference_flag
    let (mut width, mut height) = (width_mbs * 16, height_mbs * 16);
    if r.flag()? {
        let crop: [u32; 4] = [r.ue()?, r.ue()?, r.ue()?, r.ue()?];
        let chroma_array = if separate_planes {
            0
        } else {
            chroma_format_idc
        };
        let field = if frame_mbs_only { 1 } else { 2 };
        let (ux, uy) = match chroma_array {
            0 => (1, field),
            1 => (2, 2 * field),
            2 => (2, field),
            _ => (1, field),
        };
        let cut = |a: u32, b: u32, unit: u32| (a as u64 + b as u64) * unit as u64;
        let (cx, cy) = (cut(crop[0], crop[1], ux), cut(crop[2], crop[3], uy));
        if cx >= width as u64 || cy >= height as u64 {
            return Err(invalid("an SPS cropped to nothing"));
        }
        width -= cx as u32;
        height -= cy as u32;
    }
    let mut reorder = None;
    if r.flag()? {
        // Reorder limits only: a VUI that doesn't parse leaves them unknown
        // (the safe, larger default).
        reorder = vui_reorder(&mut r).unwrap_or(None);
    }
    Ok(SpsInfo {
        profile_idc,
        constraint_flags,
        level_idc,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        width_mbs,
        height_mbs,
        frame_mbs_only,
        width,
        height,
        max_num_reorder_frames: reorder.map(|(n, _)| n),
        max_dec_frame_buffering: reorder.map(|(_, n)| n),
    })
}

/// The largest side File mode decodes, in pixels (H.264 level 6.2 is
/// 8192 x 4320).
pub(crate) const FILE_MAX_SIDE: u32 = 8192;

/// File mode's check of one access unit's SPS NAL units: the last SPS's
/// information, or `None` when it has none. `Unsupported` for a stream
/// this crate doesn't decode yet (not 8-bit 4:2:0, or larger than
/// [`FILE_MAX_SIDE`]); `Corrupt` for an SPS that doesn't parse.
pub(crate) fn check_file_sps(au: &[u8]) -> Result<Option<SpsInfo>, crate::DecodeError> {
    use crate::DecodeError;
    let mut last = None;
    for nal in annexb_nals(au) {
        if nal_type(nal) != Some(NAL_SPS) {
            continue;
        }
        let sps = parse_sps(nal).map_err(|e| DecodeError::Corrupt(e.to_string()))?;
        if sps.chroma_format_idc != 1 || sps.bit_depth_luma != 8 || sps.bit_depth_chroma != 8 {
            let sampling = match sps.chroma_format_idc {
                0 => "monochrome",
                1 => "4:2:0",
                2 => "4:2:2",
                _ => "4:4:4",
            };
            return Err(DecodeError::Video(crate::error::unsupported(format!(
                "H.264 {sampling} at {} bits (profile_idc {}): File mode decodes 8-bit 4:2:0 only",
                sps.bit_depth_luma, sps.profile_idc
            ))));
        }
        if sps.width > FILE_MAX_SIDE || sps.height > FILE_MAX_SIDE {
            return Err(DecodeError::Video(crate::error::unsupported(format!(
                "a {}x{} H.264 stream (at most {FILE_MAX_SIDE} a side)",
                sps.width, sps.height
            ))));
        }
        last = Some(sps);
    }
    Ok(last)
}

/// SPS NAL units for tests: any profile, sampling, bit depth and size.
#[cfg(test)]
pub(crate) mod test_sps {
    use super::*;

    /// Writes bits most significant first, then Exp-Golomb codes.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        bits: u32,
    }

    impl Writer {
        fn bit(&mut self, b: u32) {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if b & 1 == 1 {
                *self.bytes.last_mut().expect("a byte") |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }

        fn bits(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.bit(v >> i);
            }
        }

        fn ue(&mut self, v: u32) {
            let x = v as u64 + 1;
            let len = 64 - x.leading_zeros();
            self.bits(0, len - 1);
            for i in (0..len).rev() {
                self.bit((x >> i) as u32);
            }
        }
    }

    /// What an SPS says.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Spec {
        pub profile: u8,
        pub level: u8,
        pub chroma: u32,
        pub depth: u32,
        pub width_mbs: u32,
        pub height_mbs: u32,
        /// A VUI with only a bitstream restriction: max_num_reorder_frames.
        pub reorder: Option<u32>,
    }

    impl Default for Spec {
        fn default() -> Spec {
            Spec {
                profile: 100,
                level: 30,
                chroma: 1,
                depth: 8,
                width_mbs: 40,
                height_mbs: 23,
                reorder: None,
            }
        }
    }

    /// The SPS NAL unit (header byte first, emulation prevention in place).
    pub(crate) fn sps(s: Spec) -> Vec<u8> {
        let mut w = Writer::default();
        w.bits(s.profile as u32, 8);
        w.bits(0, 8);
        w.bits(s.level as u32, 8);
        w.ue(0); // seq_parameter_set_id
        if matches!(s.profile, 100 | 110 | 122 | 244) {
            w.ue(s.chroma);
            if s.chroma == 3 {
                w.bit(0);
            }
            w.ue(s.depth - 8);
            w.ue(s.depth - 8);
            w.bit(0); // qpprime_y_zero_transform_bypass_flag
            w.bit(0); // seq_scaling_matrix_present_flag
        }
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(0); // pic_order_cnt_type
        w.ue(2); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(4); // max_num_ref_frames
        w.bit(0);
        w.ue(s.width_mbs - 1);
        w.ue(s.height_mbs - 1);
        w.bit(1); // frame_mbs_only_flag
        w.bit(1); // direct_8x8_inference_flag
        w.bit(0); // frame_cropping_flag
        match s.reorder {
            None => w.bit(0),
            Some(n) => {
                w.bit(1); // vui_parameters_present_flag
                          // No aspect, overscan, signal, chroma location, timing,
                          // HRDs or pic_struct.
                w.bits(0, 8);
                w.bit(1); // bitstream_restriction_flag
                w.bit(1); // motion_vectors_over_pic_boundaries_flag
                for v in [2, 1, 16, 16] {
                    w.ue(v);
                }
                w.ue(n);
                w.ue(n.max(4));
            }
        }
        w.bit(1); // rbsp_stop_one_bit
        let mut nal = vec![0x67];
        escape_rbsp(&w.bytes, &mut nal);
        nal
    }
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
            vec![
                &[0x67, 0x42, 0xe0, 0x1f, 0xac][..],
                &[0x68, 0xee][..],
                &[0x65, 0xaa, 0xbb][..],
                &[0x41, 0xcc][..]
            ]
        );
        assert!(is_keyframe(&nals));
        assert_eq!(codec_string(&nals).as_deref(), Some("avc1.42E01F"));
        assert_eq!(profile_level_id(nals[0]).as_deref(), Some("42e01f"));
        let avcc = annexb_to_avcc(&out);
        assert_eq!(avcc_nals(&avcc, 4).expect("split"), nals);
        // Three-byte start codes split too.
        assert_eq!(
            annexb_nals(&[0, 0, 1, 0x41, 1, 0, 0, 1, 0x41, 2]),
            vec![&[0x41, 1][..], &[0x41, 2][..]]
        );
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
        let payloads: [&[u8]; 4] = [
            &[0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 7],
            &[0, 0],
            &[],
            &[0, 0, 0, 0, 0, 0],
        ];
        for p in payloads {
            let mut esc = Vec::new();
            escape_rbsp(p, &mut esc);
            for w in esc.windows(3) {
                assert!(
                    !(w[0] == 0 && w[1] == 0 && w[2] <= 2),
                    "a start code survived in {esc:?}"
                );
            }
            assert_eq!(unescape_rbsp(&esc), p);
        }
    }

    #[test]
    fn sps_parse_reads_sampling_size_and_reorder_limits() {
        use test_sps::{sps, Spec};
        // The fake encoder's Baseline SPS: 1280 x 720, a VUI without limits.
        let fake = [0x67, 0x42, 0xe0, 0x1f, 0xda, 0x01, 0x40, 0x16, 0xe8, 0x40];
        let i = parse_sps(&fake).expect("parses");
        assert_eq!((i.profile_idc, i.level_idc), (66, 31));
        assert_eq!(
            (i.width, i.height, i.width_mbs, i.height_mbs),
            (1280, 720, 80, 45)
        );
        assert_eq!((i.chroma_format_idc, i.bit_depth_luma), (1, 8));
        assert_eq!(i.max_num_reorder_frames, None);
        assert_eq!(i.max_dpb_frames(), 5, "18000 / 3600 at level 3.1");
        assert_eq!(i.reorder_depth(), 5);

        let i = parse_sps(&sps(Spec {
            reorder: Some(2),
            ..Spec::default()
        }))
        .expect("parses");
        assert_eq!((i.width, i.height), (640, 368));
        assert_eq!(i.max_num_reorder_frames, Some(2));
        assert_eq!(i.reorder_depth(), 2);
        let i = parse_sps(&sps(Spec {
            profile: 110,
            depth: 10,
            ..Spec::default()
        }))
        .expect("10-bit");
        assert_eq!((i.bit_depth_luma, i.bit_depth_chroma), (10, 10));
        let i = parse_sps(&sps(Spec {
            chroma: 2,
            profile: 122,
            ..Spec::default()
        }))
        .expect("4:2:2");
        assert_eq!(i.chroma_format_idc, 2);
        // Over 2048 macroblocks a side, or cut short: refused.
        assert!(parse_sps(&sps(Spec {
            width_mbs: 4096,
            ..Spec::default()
        }))
        .is_err());
        assert!(parse_sps(&fake[..5]).is_err());
        assert!(parse_sps(&[0x68, 1, 2, 3]).is_err(), "a PPS");
    }

    #[test]
    fn file_mode_refuses_what_it_cant_decode_as_unsupported() {
        use crate::{DecodeError, VideoError};
        use test_sps::{sps, Spec};
        let au = |s: Spec| {
            let mut au = Vec::new();
            push_annexb(&mut au, &sps(s));
            push_annexb(&mut au, &[0x68, 0xce, 0x3c, 0x80]);
            push_annexb(&mut au, &[0x65, 0x88, 0x84]);
            au
        };
        let unsupported = |s: Spec| {
            matches!(
                check_file_sps(&au(s)),
                Err(DecodeError::Video(VideoError::Unsupported(_)))
            )
        };
        assert!(check_file_sps(&au(Spec::default()))
            .expect("8-bit 4:2:0")
            .is_some());
        assert!(
            unsupported(Spec {
                profile: 110,
                depth: 10,
                ..Spec::default()
            }),
            "10-bit"
        );
        assert!(
            unsupported(Spec {
                profile: 122,
                chroma: 2,
                ..Spec::default()
            }),
            "4:2:2"
        );
        assert!(
            unsupported(Spec {
                profile: 244,
                chroma: 3,
                ..Spec::default()
            }),
            "4:4:4"
        );
        assert!(
            unsupported(Spec {
                chroma: 0,
                ..Spec::default()
            }),
            "monochrome"
        );
        assert!(unsupported(Spec {
            width_mbs: 1024,
            height_mbs: 1024,
            ..Spec::default()
        }));
        assert!(matches!(
            check_file_sps(&au(Spec {
                width_mbs: 9000,
                ..Spec::default()
            })),
            Err(DecodeError::Corrupt(_))
        ));
        assert_eq!(check_file_sps(&[0, 0, 0, 1, 0x41, 1]), Ok(None), "no SPS");
    }
}
