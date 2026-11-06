//! The VP8 decoder (RFC 6386).

use std::fmt;

use crate::bool_decoder::BoolDecoder;
use crate::frame::{FrameBuf, Picture, Plane};
use crate::idct;
use crate::loop_filter::{self, FilterParams};
use crate::predict::{self, RefPlane, CHROMA_WS_SIZE, CHROMA_WS_STRIDE, LUMA_WS_SIZE, LUMA_WS_STRIDE};
use crate::tables::*;

/// Why a frame could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// An interframe arrived before any keyframe, or after an error. Drop
    /// frames until the next keyframe (and ask the sender for one).
    NeedKeyframe,
    /// The frame is shorter than its own headers say.
    Truncated,
    /// The frame is malformed.
    Invalid(&'static str),
    /// A valid frame that uses something this decoder does not support.
    Unsupported(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::NeedKeyframe => f.write_str("need a keyframe"),
            DecodeError::Truncated => f.write_str("truncated frame"),
            DecodeError::Invalid(why) => write!(f, "invalid frame: {why}"),
            DecodeError::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The size fields of a keyframe header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dimensions {
    /// Coded width in pixels.
    pub width: u16,
    /// Coded height in pixels.
    pub height: u16,
    /// Upscaling the sender asks for on display: 0 none, 1 5/4, 2 5/3, 3 2.
    /// Decoding ignores it (RFC 6386 Section 9.1).
    pub horizontal_scale: u8,
    /// As `horizontal_scale`, vertically.
    pub vertical_scale: u8,
}

/// What a frame's uncompressed header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameInfo {
    /// A keyframe (intra only; decodable on its own).
    pub keyframe: bool,
    /// The bitstream version (0 to 3): 0 uses the six-tap filter, 1 and 2
    /// bilinear, 3 full-pixel chroma motion.
    pub version: u8,
    /// False for a frame that only updates references (an alt-ref frame).
    pub show_frame: bool,
    /// Size of the first partition in bytes.
    pub first_partition_size: u32,
    /// Keyframes only.
    pub dimensions: Option<Dimensions>,
}

/// Parses a frame's uncompressed header (3 bytes, 10 on a keyframe)
/// without decoding it.
pub fn frame_info(data: &[u8]) -> Result<FrameInfo, DecodeError> {
    if data.len() < 3 {
        return Err(DecodeError::Truncated);
    }
    let tag = data[0] as u32 | (data[1] as u32) << 8 | (data[2] as u32) << 16;
    let keyframe = tag & 1 == 0;
    let version = ((tag >> 1) & 7) as u8;
    let show_frame = (tag >> 4) & 1 == 1;
    let first_partition_size = tag >> 5;
    if version > 3 {
        return Err(DecodeError::Unsupported("bitstream version above 3"));
    }
    let dimensions = if keyframe {
        if data.len() < 10 {
            return Err(DecodeError::Truncated);
        }
        if data[3..6] != [0x9d, 0x01, 0x2a] {
            return Err(DecodeError::Invalid("bad keyframe start code"));
        }
        let w = u16::from_le_bytes([data[6], data[7]]);
        let h = u16::from_le_bytes([data[8], data[9]]);
        let d = Dimensions {
            width: w & 0x3fff,
            height: h & 0x3fff,
            horizontal_scale: (w >> 14) as u8,
            vertical_scale: (h >> 14) as u8,
        };
        if d.width == 0 || d.height == 0 {
            return Err(DecodeError::Invalid("zero frame size"));
        }
        Some(d)
    } else {
        None
    };
    Ok(FrameInfo {
        keyframe,
        version,
        show_frame,
        first_partition_size,
        dimensions,
    })
}

/// A motion vector in quarter pixels times two (eighth pixels, always even
/// for luma), as the reference decoder stores them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Mv {
    x: i16,
    y: i16,
}

impl Mv {
    const ZERO: Mv = Mv { x: 0, y: 0 };

    fn is_zero(self) -> bool {
        self.x == 0 && self.y == 0
    }
}

/// What the decoder keeps about each macroblock.
#[derive(Clone, Copy)]
struct MbInfo {
    y_mode: u8,
    uv_mode: u8,
    ref_frame: u8,
    segment: u8,
    /// Any coefficient token was decoded (drives the loop filter's inner edges).
    has_coeffs: bool,
    mv: Mv,
    /// Subblock intra modes (for keyframe contexts); implied ones for the
    /// other intra modes.
    bmodes: [u8; 16],
    /// Per-subblock MVs (all equal unless SPLITMV; zero for intra).
    mvs: [Mv; 16],
}

impl Default for MbInfo {
    fn default() -> Self {
        MbInfo {
            y_mode: DC_PRED,
            uv_mode: DC_PRED,
            ref_frame: INTRA_FRAME,
            segment: 0,
            has_coeffs: false,
            mv: Mv::ZERO,
            bmodes: [B_DC_PRED; 16],
            mvs: [Mv::ZERO; 16],
        }
    }
}

fn implied_bmode(y_mode: u8) -> u8 {
    match y_mode {
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        TM_PRED => B_TM_PRED,
        _ => B_DC_PRED,
    }
}

/// The probabilities that persist from frame to frame.
#[derive(Clone)]
struct Entropy {
    coeff: CoeffProbs,
    mv: MvProbs,
    ymode: [u8; 4],
    uvmode: [u8; 3],
}

impl Default for Entropy {
    fn default() -> Self {
        Entropy {
            coeff: DEFAULT_COEFF_PROBS,
            mv: DEFAULT_MV_PROBS,
            ymode: DEFAULT_YMODE_PROBS,
            uvmode: DEFAULT_UV_MODE_PROBS,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Segmentation {
    enabled: bool,
    update_map: bool,
    abs_values: bool,
    quant: [i32; 4],
    filter_level: [i32; 4],
    tree_probs: [u8; 3],
}

#[derive(Clone, Copy, Default)]
struct FilterDeltas {
    enabled: bool,
    ref_frame: [i32; 4],
    mode: [i32; 4],
}

/// Dequantization factors for one segment: `[dc, ac]` per block type.
#[derive(Clone, Copy, Default)]
struct Dequant {
    y1: [i32; 2],
    y2: [i32; 2],
    uv: [i32; 2],
}

/// Everything parsed from the first partition's frame header.
struct Header {
    keyframe: bool,
    version: u8,
    filter_simple: bool,
    filter_level: i32,
    sharpness: u8,
    refresh_golden: bool,
    refresh_altref: bool,
    copy_to_golden: u32,
    copy_to_altref: u32,
    refresh_last: bool,
    skip_enabled: bool,
    prob_skip: u8,
    prob_intra: u8,
    prob_last: u8,
    prob_golden: u8,
}

/// A VP8 decoder.
///
/// Feed it whole frames in order with [`Decoder::decode`]. It starts at a
/// keyframe and, after any error, waits for the next keyframe
/// ([`DecodeError::NeedKeyframe`]).
pub struct Decoder {
    info: Option<FrameInfo>,
    width: u16,
    height: u16,
    mb_cols: usize,
    mb_rows: usize,
    bufs: [FrameBuf; 4],
    /// Buffer index of each reference frame (indexed by LAST_FRAME..=ALTREF_FRAME).
    refs: [usize; 4],
    shown: usize,
    sign_bias: [bool; 4],
    mbs: Vec<MbInfo>,
    above_ctx: Vec<[u8; 9]>,
    entropy: Entropy,
    seg: Segmentation,
    deltas: FilterDeltas,
    color_space: bool,
    ready: bool,
    coeffs: Box<[i16; 400]>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    /// A decoder waiting for its first keyframe.
    pub fn new() -> Self {
        Decoder {
            info: None,
            width: 0,
            height: 0,
            mb_cols: 0,
            mb_rows: 0,
            bufs: Default::default(),
            refs: [1; 4],
            shown: 0,
            sign_bias: [false; 4],
            mbs: Vec::new(),
            above_ctx: Vec::new(),
            entropy: Entropy::default(),
            seg: Segmentation::default(),
            deltas: FilterDeltas::default(),
            color_space: false,
            ready: false,
            coeffs: Box::new([0; 400]),
        }
    }

    /// Forgets the stream: the next frame must be a keyframe.
    pub fn reset(&mut self) {
        self.ready = false;
        self.info = None;
    }

    /// The header of the last frame decoded successfully.
    pub fn last_frame_info(&self) -> Option<FrameInfo> {
        self.info
    }

    /// The current coded size, after at least one keyframe.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        if self.width == 0 {
            None
        } else {
            Some((self.width as u32, self.height as u32))
        }
    }

    /// The colour space bit of the last keyframe: false for BT.601 (the
    /// only one RFC 6386 defines).
    pub fn color_space_reserved(&self) -> bool {
        self.color_space
    }

    /// Decodes one frame. Returns the picture to display, or `None` for a
    /// frame that is not shown (it only updates reference frames). The
    /// picture borrows the decoder until the next call.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Picture<'_>>, DecodeError> {
        let info = frame_info(data)?;
        if !info.keyframe && !self.ready {
            return Err(DecodeError::NeedKeyframe);
        }
        if let Err(e) = self.decode_frame(data, &info) {
            self.ready = false;
            return Err(e);
        }
        self.ready = true;
        self.info = Some(info);
        if !info.show_frame {
            return Ok(None);
        }
        Ok(Some(self.picture(self.shown)))
    }

    fn picture(&self, idx: usize) -> Picture<'_> {
        let b = &self.bufs[idx];
        Picture {
            width: self.width as u32,
            height: self.height as u32,
            y: &b.y.data,
            u: &b.u.data,
            v: &b.v.data,
            y_stride: b.y.w,
            uv_stride: b.u.w,
        }
    }

    fn resize(&mut self, d: Dimensions) {
        self.width = d.width;
        self.height = d.height;
        self.mb_cols = (d.width as usize).div_ceil(16);
        self.mb_rows = (d.height as usize).div_ceil(16);
        for b in self.bufs.iter_mut() {
            *b = FrameBuf::new(self.mb_cols, self.mb_rows);
        }
        self.refs = [1; 4];
        self.mbs = vec![MbInfo::default(); (self.mb_cols + 1) * (self.mb_rows + 1)];
        self.above_ctx = vec![[0; 9]; self.mb_cols];
    }

    fn decode_frame(&mut self, data: &[u8], info: &FrameInfo) -> Result<(), DecodeError> {
        let header_len = if info.keyframe { 10 } else { 3 };
        let first_len = info.first_partition_size as usize;
        if data.len() < header_len + first_len {
            return Err(DecodeError::Truncated);
        }
        if let Some(d) = info.dimensions {
            if d.width != self.width || d.height != self.height || self.mbs.is_empty() {
                self.resize(d);
            }
        }
        let mut bd = BoolDecoder::new(&data[header_len..header_len + first_len]);
        let rest = &data[header_len + first_len..];
        let hdr = self.read_header(&mut bd, info)?;

        // Token partitions.
        let num_parts = 1usize << bd.read_literal(2);
        let sizes_len = 3 * (num_parts - 1);
        if rest.len() < sizes_len {
            return Err(DecodeError::Truncated);
        }
        let mut parts: Vec<BoolDecoder<'_>> = Vec::with_capacity(num_parts);
        let mut off = sizes_len;
        for i in 0..num_parts {
            let size = if i + 1 < num_parts {
                let s = &rest[3 * i..3 * i + 3];
                s[0] as usize | (s[1] as usize) << 8 | (s[2] as usize) << 16
            } else {
                rest.len() - off
            };
            if rest.len() - off < size {
                return Err(DecodeError::Truncated);
            }
            parts.push(BoolDecoder::new(&rest[off..off + size]));
            off += size;
        }

        // Quantizer indices.
        let q_index = bd.read_literal(7) as i32;
        let y1_dc = bd.read_optional_signed(4);
        let y2_dc = bd.read_optional_signed(4);
        let y2_ac = bd.read_optional_signed(4);
        let uv_dc = bd.read_optional_signed(4);
        let uv_ac = bd.read_optional_signed(4);
        let mut dequant = [Dequant::default(); 4];
        for (s, dq) in dequant.iter_mut().enumerate() {
            let q = if self.seg.enabled {
                if self.seg.abs_values {
                    self.seg.quant[s]
                } else {
                    q_index + self.seg.quant[s]
                }
            } else {
                q_index
            };
            let qi = |delta: i32| (q + delta).clamp(0, 127) as usize;
            dq.y1 = [DC_QUANT[qi(y1_dc)], AC_QUANT[qi(0)]];
            dq.y2 = [DC_QUANT[qi(y2_dc)] * 2, (AC_QUANT[qi(y2_ac)] * 155 / 100).max(8)];
            dq.uv = [DC_QUANT[qi(uv_dc)].min(132), AC_QUANT[qi(uv_ac)]];
        }

        // Reference updates and the entropy context.
        let mut hdr = hdr;
        let refresh_entropy;
        if info.keyframe {
            hdr.refresh_golden = true;
            hdr.refresh_altref = true;
            self.sign_bias = [false; 4];
            refresh_entropy = bd.read_bit();
            hdr.refresh_last = true;
            self.entropy = Entropy::default();
        } else {
            hdr.refresh_golden = bd.read_bit();
            hdr.refresh_altref = bd.read_bit();
            if !hdr.refresh_golden {
                hdr.copy_to_golden = bd.read_literal(2);
            }
            if !hdr.refresh_altref {
                hdr.copy_to_altref = bd.read_literal(2);
            }
            self.sign_bias[GOLDEN_FRAME as usize] = bd.read_bit();
            self.sign_bias[ALTREF_FRAME as usize] = bd.read_bit();
            refresh_entropy = bd.read_bit();
            hdr.refresh_last = bd.read_bit();
        }
        let saved_entropy = if refresh_entropy { None } else { Some(self.entropy.clone()) };

        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        if bd.read(COEFF_UPDATE_PROBS[i][j][k][l]) {
                            self.entropy.coeff[i][j][k][l] = bd.read_literal(8) as u8;
                        }
                    }
                }
            }
        }
        hdr.skip_enabled = bd.read_bit();
        if hdr.skip_enabled {
            hdr.prob_skip = bd.read_literal(8) as u8;
        }
        if !info.keyframe {
            hdr.prob_intra = bd.read_literal(8) as u8;
            hdr.prob_last = bd.read_literal(8) as u8;
            hdr.prob_golden = bd.read_literal(8) as u8;
            if bd.read_bit() {
                for p in self.entropy.ymode.iter_mut() {
                    *p = bd.read_literal(8) as u8;
                }
            }
            if bd.read_bit() {
                for p in self.entropy.uvmode.iter_mut() {
                    *p = bd.read_literal(8) as u8;
                }
            }
            for i in 0..2 {
                for j in 0..MV_PROB_COUNT {
                    if bd.read(MV_UPDATE_PROBS[i][j]) {
                        let x = bd.read_literal(7) as u8;
                        self.entropy.mv[i][j] = if x != 0 { x << 1 } else { 1 };
                    }
                }
            }
        }

        // The buffer to decode into: one no reference frame uses.
        let cur_idx = (0..4)
            .find(|i| !self.refs[1..].contains(i))
            .expect("four buffers, three references");
        let mut cur = std::mem::take(&mut self.bufs[cur_idx]);
        let result = self.decode_macroblocks(&mut cur, &mut bd, &mut parts, &hdr, &dequant);
        self.bufs[cur_idx] = cur;
        result?;

        if let Some(e) = saved_entropy {
            self.entropy = e;
        }

        // Reference frame updates (the same order as the reference decoder).
        if info.keyframe {
            self.refs[LAST_FRAME as usize] = cur_idx;
            self.refs[GOLDEN_FRAME as usize] = cur_idx;
            self.refs[ALTREF_FRAME as usize] = cur_idx;
        } else {
            match hdr.copy_to_altref {
                1 => self.refs[ALTREF_FRAME as usize] = self.refs[LAST_FRAME as usize],
                2 => self.refs[ALTREF_FRAME as usize] = self.refs[GOLDEN_FRAME as usize],
                _ => {}
            }
            match hdr.copy_to_golden {
                1 => self.refs[GOLDEN_FRAME as usize] = self.refs[LAST_FRAME as usize],
                2 => self.refs[GOLDEN_FRAME as usize] = self.refs[ALTREF_FRAME as usize],
                _ => {}
            }
            if hdr.refresh_golden {
                self.refs[GOLDEN_FRAME as usize] = cur_idx;
            }
            if hdr.refresh_altref {
                self.refs[ALTREF_FRAME as usize] = cur_idx;
            }
            if hdr.refresh_last {
                self.refs[LAST_FRAME as usize] = cur_idx;
            }
        }
        self.shown = cur_idx;
        Ok(())
    }

    /// The frame header up to the token partition count.
    fn read_header(&mut self, bd: &mut BoolDecoder<'_>, info: &FrameInfo) -> Result<Header, DecodeError> {
        if info.keyframe {
            self.color_space = bd.read_bit();
            // The clamping type: whether the encoder promises in-range
            // values. Clamping always is equivalent.
            let _ = bd.read_bit();
            self.seg = Segmentation::default();
            self.deltas = FilterDeltas::default();
        }
        // Segmentation (Section 9.3).
        self.seg.enabled = bd.read_bit();
        if self.seg.enabled {
            self.seg.update_map = bd.read_bit();
            let update_data = bd.read_bit();
            if update_data {
                self.seg.abs_values = bd.read_bit();
                for i in 0..4 {
                    self.seg.quant[i] = bd.read_optional_signed(7);
                }
                for i in 0..4 {
                    self.seg.filter_level[i] = bd.read_optional_signed(6);
                }
            }
            if self.seg.update_map {
                for i in 0..3 {
                    self.seg.tree_probs[i] = if bd.read_bit() { bd.read_literal(8) as u8 } else { 255 };
                }
            }
        } else {
            self.seg.update_map = false;
        }
        // Loop filter (Section 9.4).
        let filter_simple = bd.read_bit();
        let filter_level = bd.read_literal(6) as i32;
        let sharpness = bd.read_literal(3) as u8;
        self.deltas.enabled = bd.read_bit();
        if self.deltas.enabled && bd.read_bit() {
            for i in 0..4 {
                if bd.read_bit() {
                    let v = bd.read_literal(6) as i32;
                    self.deltas.ref_frame[i] = if bd.read_bit() { -v } else { v };
                }
            }
            for i in 0..4 {
                if bd.read_bit() {
                    let v = bd.read_literal(6) as i32;
                    self.deltas.mode[i] = if bd.read_bit() { -v } else { v };
                }
            }
        }
        Ok(Header {
            keyframe: info.keyframe,
            version: info.version,
            filter_simple,
            filter_level,
            sharpness,
            refresh_golden: false,
            refresh_altref: false,
            copy_to_golden: 0,
            copy_to_altref: 0,
            refresh_last: false,
            skip_enabled: false,
            prob_skip: 0,
            prob_intra: 0,
            prob_last: 0,
            prob_golden: 0,
        })
    }

    fn decode_macroblocks(
        &mut self,
        cur: &mut FrameBuf,
        bd: &mut BoolDecoder<'_>,
        parts: &mut [BoolDecoder<'_>],
        hdr: &Header,
        dequant: &[Dequant; 4],
    ) -> Result<(), DecodeError> {
        let cols = self.mb_cols;
        let rows = self.mb_rows;
        let stride = cols + 1;
        for c in self.above_ctx.iter_mut() {
            *c = [0; 9];
        }
        let (filters, bilinear) = if hdr.version == 0 {
            (&predict::SIXTAP_FILTERS, false)
        } else {
            (&predict::BILINEAR_FILTERS, true)
        };
        let full_pixel = hdr.version == 3;
        let num_parts = parts.len();
        for row in 0..rows {
            let mut left_ctx = [0u8; 9];
            let tbd = &mut parts[row % num_parts];
            for col in 0..cols {
                let i = (row + 1) * stride + col + 1;
                let (mut mb, skip) = self.read_modes(bd, hdr, i, row, col);
                let has_y2 = mb.y_mode != B_PRED && mb.y_mode != SPLITMV;
                let mut eobs = [0u8; 25];
                let ctx_above = &mut self.above_ctx[col];
                if skip {
                    for k in 0..8 {
                        left_ctx[k] = 0;
                        ctx_above[k] = 0;
                    }
                    if has_y2 {
                        left_ctx[8] = 0;
                        ctx_above[8] = 0;
                    }
                } else {
                    let seg = if self.seg.enabled { mb.segment as usize } else { 0 };
                    mb.has_coeffs = decode_mb_tokens(
                        tbd,
                        &self.entropy.coeff,
                        ctx_above,
                        &mut left_ctx,
                        has_y2,
                        &dequant[seg],
                        &mut self.coeffs,
                        &mut eobs,
                    );
                }
                let x = col * 16;
                let y = row * 16;
                if mb.ref_frame == INTRA_FRAME {
                    recon_intra(cur, &mb, col, row, cols, &mut self.coeffs, &eobs, has_y2);
                } else {
                    let refb = &self.bufs[self.refs[mb.ref_frame as usize]];
                    recon_inter(cur, refb, &mb, x, y, &mut self.coeffs, &eobs, has_y2, filters, bilinear, full_pixel);
                }
                self.mbs[i] = mb;
            }
            if hdr.filter_level != 0 && row > 0 {
                self.filter_row(cur, row - 1, hdr);
            }
        }
        if hdr.filter_level != 0 && rows > 0 {
            self.filter_row(cur, rows - 1, hdr);
        }
        Ok(())
    }

    /// Reads one macroblock header (Section 19.3): segment, skip flag and
    /// modes. Returns the macroblock and its skip flag.
    fn read_modes(&self, bd: &mut BoolDecoder<'_>, hdr: &Header, i: usize, row: usize, col: usize) -> (MbInfo, bool) {
        let stride = self.mb_cols + 1;
        let mut mb = MbInfo {
            segment: self.mbs[i].segment,
            ..MbInfo::default()
        };
        if self.seg.update_map {
            let p = &self.seg.tree_probs;
            mb.segment = if bd.read(p[0]) {
                2 + bd.read(p[2]) as u8
            } else {
                bd.read(p[1]) as u8
            };
        } else if hdr.keyframe {
            mb.segment = 0;
        }
        let skip = hdr.skip_enabled && bd.read(hdr.prob_skip);

        if hdr.keyframe {
            mb.y_mode = bd.read_tree(&KF_YMODE_TREE, &KF_YMODE_PROBS);
            if mb.y_mode == B_PRED {
                let above = &self.mbs[i - stride];
                let left = &self.mbs[i - 1];
                for b in 0..16 {
                    let a = if b < 4 { above.bmodes[b + 12] } else { mb.bmodes[b - 4] };
                    let l = if b & 3 == 0 { left.bmodes[b + 3] } else { mb.bmodes[b - 1] };
                    mb.bmodes[b] = bd.read_tree(&BMODE_TREE, &KF_BMODE_PROBS[a as usize][l as usize]);
                }
            } else {
                mb.bmodes = [implied_bmode(mb.y_mode); 16];
            }
            mb.uv_mode = bd.read_tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS);
            return (mb, skip);
        }

        if !bd.read(hdr.prob_intra) {
            mb.y_mode = bd.read_tree(&YMODE_TREE, &self.entropy.ymode);
            if mb.y_mode == B_PRED {
                for b in 0..16 {
                    mb.bmodes[b] = bd.read_tree(&BMODE_TREE, &BMODE_PROBS);
                }
            } else {
                mb.bmodes = [implied_bmode(mb.y_mode); 16];
            }
            mb.uv_mode = bd.read_tree(&UV_MODE_TREE, &self.entropy.uvmode);
            return (mb, skip);
        }

        // An inter macroblock (Section 16.2).
        mb.ref_frame = if bd.read(hdr.prob_last) {
            GOLDEN_FRAME + bd.read(hdr.prob_golden) as u8
        } else {
            LAST_FRAME
        };
        let above = &self.mbs[i - stride];
        let left = &self.mbs[i - 1];
        let above_left = &self.mbs[i - stride - 1];
        let (near, cnt) = find_near_mvs(above, left, above_left, mb.ref_frame, &self.sign_bias);
        let probs = [
            MODE_CONTEXTS[cnt[0] as usize][0],
            MODE_CONTEXTS[cnt[1] as usize][1],
            MODE_CONTEXTS[cnt[2] as usize][2],
            MODE_CONTEXTS[cnt[3] as usize][3],
        ];
        mb.y_mode = bd.read_tree(&MV_REF_TREE, &probs);
        mb.uv_mode = mb.y_mode;
        // Clamp bounds in eighth pixels: one macroblock beyond the frame.
        let bounds = Bounds {
            to_left: -(((col + 1) << 7) as i32),
            to_right: ((self.mb_cols - col) << 7) as i32,
            to_top: -(((row + 1) << 7) as i32),
            to_bottom: ((self.mb_rows - row) << 7) as i32,
        };
        match mb.y_mode {
            NEARESTMV => mb.mv = bounds.clamp(near[1]),
            NEARMV => mb.mv = bounds.clamp(near[2]),
            ZEROMV => mb.mv = Mv::ZERO,
            NEWMV => {
                let best = bounds.clamp(near[0]);
                let d = read_mv(bd, &self.entropy.mv);
                mb.mv = Mv {
                    x: d.x.wrapping_add(best.x),
                    y: d.y.wrapping_add(best.y),
                };
            }
            SPLITMV => {
                let best = bounds.clamp(near[0]);
                let part = bd.read_tree(&SPLIT_MV_TREE, &SPLIT_MV_PROBS) as usize;
                let map = &MV_PARTITIONS[part];
                for j in 0..MV_PARTITION_COUNT[part] as u8 {
                    let k = map.iter().position(|&m| m == j).expect("every part has a block");
                    let left_mv = if k & 3 != 0 { mb.mvs[k - 1] } else { left.mvs[k + 3] };
                    let above_mv = if k >= 4 { mb.mvs[k - 4] } else { above.mvs[k + 12] };
                    let lez = left_mv.is_zero();
                    let aez = above_mv.is_zero();
                    let lea = left_mv == above_mv;
                    let ctx = if lea && lez {
                        4
                    } else if lea {
                        3
                    } else if aez {
                        2
                    } else if lez {
                        1
                    } else {
                        0
                    };
                    let mv = match bd.read_tree(&SUBMV_REF_TREE, &SUBMV_REF_PROBS[ctx]) {
                        LEFT4X4 => left_mv,
                        ABOVE4X4 => above_mv,
                        ZERO4X4 => Mv::ZERO,
                        _ => {
                            let d = read_mv(bd, &self.entropy.mv);
                            Mv {
                                x: d.x.wrapping_add(best.x),
                                y: d.y.wrapping_add(best.y),
                            }
                        }
                    };
                    for (kk, &m) in map.iter().enumerate() {
                        if m == j {
                            mb.mvs[kk] = mv;
                        }
                    }
                }
                mb.mv = mb.mvs[15];
                return (mb, skip);
            }
            _ => unreachable!("MV_REF_TREE has five leaves"),
        }
        mb.mvs = [mb.mv; 16];
        (mb, skip)
    }

    fn filter_level(&self, mb: &MbInfo, base: i32) -> i32 {
        let mut level = base;
        if self.seg.enabled {
            let s = mb.segment as usize;
            level = if self.seg.abs_values {
                self.seg.filter_level[s]
            } else {
                level + self.seg.filter_level[s]
            };
            level = level.clamp(0, 63);
        }
        if self.deltas.enabled {
            level += self.deltas.ref_frame[mb.ref_frame as usize];
            if mb.ref_frame == INTRA_FRAME {
                if mb.y_mode == B_PRED {
                    level += self.deltas.mode[0];
                }
            } else if mb.y_mode == ZEROMV {
                level += self.deltas.mode[1];
            } else if mb.y_mode == SPLITMV {
                level += self.deltas.mode[3];
            } else {
                level += self.deltas.mode[2];
            }
            level = level.clamp(0, 63);
        }
        level
    }

    fn filter_row(&self, cur: &mut FrameBuf, row: usize, hdr: &Header) {
        let stride = self.mb_cols + 1;
        for col in 0..self.mb_cols {
            let mb = &self.mbs[(row + 1) * stride + col + 1];
            let level = self.filter_level(mb, hdr.filter_level);
            if level == 0 {
                continue;
            }
            let p = FilterParams::new(level, hdr.sharpness, hdr.keyframe);
            let inner = mb.has_coeffs || mb.y_mode == B_PRED || mb.y_mode == SPLITMV;
            let mb_limit = (level + 2) * 2 + p.interior_limit;
            let b_limit = level * 2 + p.interior_limit;
            if hdr.filter_simple {
                let plane = &mut cur.y;
                let s = plane.w;
                let pos = row * 16 * s + col * 16;
                let buf = &mut plane.data;
                if col > 0 {
                    loop_filter::simple_edge(buf, pos, 1, s, 16, mb_limit);
                }
                if inner {
                    for k in [4, 8, 12] {
                        loop_filter::simple_edge(buf, pos + k, 1, s, 16, b_limit);
                    }
                }
                if row > 0 {
                    loop_filter::simple_edge(buf, pos, s, 1, 16, mb_limit);
                }
                if inner {
                    for k in [4, 8, 12] {
                        loop_filter::simple_edge(buf, pos + k * s, s, 1, 16, b_limit);
                    }
                }
            } else {
                filter_plane_mb(&mut cur.y, 16, row, col, inner, mb_limit, b_limit, &p);
                filter_plane_mb(&mut cur.u, 8, row, col, inner, mb_limit, b_limit, &p);
                filter_plane_mb(&mut cur.v, 8, row, col, inner, mb_limit, b_limit, &p);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn filter_plane_mb(
    plane: &mut Plane,
    n: usize,
    row: usize,
    col: usize,
    inner: bool,
    mb_limit: i32,
    b_limit: i32,
    p: &FilterParams,
) {
    let s = plane.w;
    let pos = row * n * s + col * n;
    let buf = &mut plane.data;
    if col > 0 {
        loop_filter::mb_edge(buf, pos, 1, s, n, mb_limit, p);
    }
    if inner {
        let mut k = 4;
        while k < n {
            loop_filter::subblock_edge(buf, pos + k, 1, s, n, b_limit, p);
            k += 4;
        }
    }
    if row > 0 {
        loop_filter::mb_edge(buf, pos, s, 1, n, mb_limit, p);
    }
    if inner {
        let mut k = 4;
        while k < n {
            loop_filter::subblock_edge(buf, pos + k * s, s, 1, n, b_limit, p);
            k += 4;
        }
    }
}

struct Bounds {
    to_left: i32,
    to_right: i32,
    to_top: i32,
    to_bottom: i32,
}

impl Bounds {
    fn clamp(&self, mv: Mv) -> Mv {
        Mv {
            x: (mv.x as i32).clamp(self.to_left, self.to_right) as i16,
            y: (mv.y as i32).clamp(self.to_top, self.to_bottom) as i16,
        }
    }
}

fn read_mv_component(bd: &mut BoolDecoder<'_>, p: &[u8; MV_PROB_COUNT]) -> i16 {
    let mut x: i32;
    if bd.read(p[MVP_IS_SHORT]) {
        x = 0;
        for i in 0..3 {
            x += (bd.read(p[MVP_BITS + i]) as i32) << i;
        }
        for i in (4..MV_LONG_BITS).rev() {
            x += (bd.read(p[MVP_BITS + i]) as i32) << i;
        }
        if x & 0xfff0 == 0 || bd.read(p[MVP_BITS + 3]) {
            x += 8;
        }
    } else {
        x = bd.read_tree(&SMALL_MV_TREE, &p[MVP_SHORT..MVP_SHORT + 7]) as i32;
    }
    if x != 0 && bd.read(p[MVP_SIGN]) {
        x = -x;
    }
    (x << 1) as i16
}

fn read_mv(bd: &mut BoolDecoder<'_>, probs: &MvProbs) -> Mv {
    let y = read_mv_component(bd, &probs[0]);
    let x = read_mv_component(bd, &probs[1]);
    Mv { x, y }
}

/// The near and nearest MVs and their weights (Section 16.3). Returns
/// `[best, nearest, near, _]` and the four counts.
fn find_near_mvs(above: &MbInfo, left: &MbInfo, above_left: &MbInfo, ref_frame: u8, sign_bias: &[bool; 4]) -> ([Mv; 4], [u8; 4]) {
    let mut mvs = [Mv::ZERO; 4];
    let mut cnt = [0u8; 4];
    let mut idx = 0usize;
    let bias = |nb: &MbInfo| -> Mv {
        let mv = nb.mv;
        if sign_bias[nb.ref_frame as usize] != sign_bias[ref_frame as usize] {
            Mv {
                x: mv.x.wrapping_neg(),
                y: mv.y.wrapping_neg(),
            }
        } else {
            mv
        }
    };
    if above.ref_frame != INTRA_FRAME {
        if !above.mv.is_zero() {
            idx += 1;
            mvs[idx] = bias(above);
            cnt[idx] += 2;
        } else {
            cnt[0] += 2;
        }
    }
    for (nb, weight) in [(left, 2u8), (above_left, 1u8)] {
        if nb.ref_frame != INTRA_FRAME {
            if !nb.mv.is_zero() {
                let this = bias(nb);
                if this != mvs[idx] {
                    idx += 1;
                    mvs[idx] = this;
                }
                cnt[idx] += weight;
            } else {
                cnt[0] += weight;
            }
        }
    }
    // Three distinct MVs: merge above-left into nearest if they are equal.
    if cnt[3] > 0 && mvs[3] == mvs[1] {
        cnt[1] += 1;
    }
    cnt[3] = (above.y_mode == SPLITMV) as u8 * 2 + (left.y_mode == SPLITMV) as u8 * 2 + (above_left.y_mode == SPLITMV) as u8;
    if cnt[2] > cnt[1] {
        cnt.swap(1, 2);
        mvs.swap(1, 2);
    }
    if cnt[1] >= cnt[0] {
        mvs[0] = mvs[1];
    }
    (mvs, cnt)
}

/// Reads one block's tokens (Section 13) into `out` (dequantized, natural
/// order). Returns the index after the last token decoded (0 = none, or
/// `first` if the block ended at once).
#[inline]
fn decode_block(
    bd: &mut BoolDecoder<'_>,
    probs: &[[[u8; 11]; 3]; 8],
    ctx: usize,
    first: usize,
    dq: &[i32; 2],
    out: &mut [i16],
) -> usize {
    let mut c = first;
    let mut p = &probs[COEFF_BANDS[c]][ctx];
    if !bd.read(p[0]) {
        return c;
    }
    loop {
        if !bd.read(p[1]) {
            // A zero; the next token can't be the end of block.
            c += 1;
            if c == 16 {
                return 16;
            }
            p = &probs[COEFF_BANDS[c]][0];
            continue;
        }
        let v: i32;
        let next_ctx;
        if !bd.read(p[2]) {
            v = 1;
            next_ctx = 1;
        } else {
            next_ctx = 2;
            if !bd.read(p[3]) {
                v = if !bd.read(p[4]) { 2 } else { 3 + bd.read(p[5]) as i32 };
            } else if !bd.read(p[6]) {
                v = if !bd.read(p[7]) {
                    5 + extra(bd, &PCAT1)
                } else {
                    7 + extra(bd, &PCAT2)
                };
            } else if !bd.read(p[8]) {
                v = if !bd.read(p[9]) {
                    11 + extra(bd, &PCAT3)
                } else {
                    19 + extra(bd, &PCAT4)
                };
            } else if !bd.read(p[10]) {
                v = 35 + extra(bd, &PCAT5);
            } else {
                v = 67 + extra(bd, &PCAT6);
            }
        }
        let signed = if bd.read_bit() { -v } else { v };
        out[ZIGZAG[c]] = (signed * dq[(c > 0) as usize]) as i16;
        c += 1;
        if c == 16 {
            return 16;
        }
        p = &probs[COEFF_BANDS[c]][next_ctx];
        if !bd.read(p[0]) {
            return c;
        }
    }
}

#[inline]
fn extra(bd: &mut BoolDecoder<'_>, probs: &[u8]) -> i32 {
    let mut v = 0;
    for &p in probs {
        v = (v << 1) | bd.read(p) as i32;
    }
    v
}

/// Reads all tokens of a macroblock. Returns whether any token was read.
#[allow(clippy::too_many_arguments)]
fn decode_mb_tokens(
    bd: &mut BoolDecoder<'_>,
    probs: &CoeffProbs,
    above: &mut [u8; 9],
    left: &mut [u8; 9],
    has_y2: bool,
    dq: &Dequant,
    coeffs: &mut [i16; 400],
    eobs: &mut [u8; 25],
) -> bool {
    let mut any = false;
    let (y_type, y_first) = if has_y2 {
        let ctx = (above[8] + left[8]) as usize;
        let eob = decode_block(bd, &probs[1], ctx, 0, &dq.y2, &mut coeffs[384..400]);
        let nz = (eob > 0) as u8;
        above[8] = nz;
        left[8] = nz;
        eobs[24] = eob as u8;
        any |= eob > 0;
        (0, 1)
    } else {
        (3, 0)
    };
    for b in 0..16 {
        let ctx = (above[b & 3] + left[b >> 2]) as usize;
        let eob = decode_block(bd, &probs[y_type], ctx, y_first, &dq.y1, &mut coeffs[b * 16..b * 16 + 16]);
        let nz = (eob > y_first) as u8;
        above[b & 3] = nz;
        left[b >> 2] = nz;
        eobs[b] = eob as u8;
        any |= eob > y_first;
    }
    for b in 16..24 {
        // U blocks 16..20 use contexts 4..6, V blocks 20..24 use 6..8.
        let base = if b < 20 { 4 } else { 6 };
        let j = b & 3;
        let ai = base + (j & 1);
        let li = base + (j >> 1);
        let ctx = (above[ai] + left[li]) as usize;
        let eob = decode_block(bd, &probs[2], ctx, 0, &dq.uv, &mut coeffs[b * 16..b * 16 + 16]);
        let nz = (eob > 0) as u8;
        above[ai] = nz;
        left[li] = nz;
        eobs[b] = eob as u8;
        any |= eob > 0;
    }
    any
}

/// Adds one block's residual to `dst` and clears its coefficients.
#[inline]
fn add_residual(coeffs: &mut [i16], eob: u8, dst: &mut [u8], pos: usize, stride: usize) {
    if eob > 1 {
        idct::idct_add(coeffs, dst, pos, stride);
        coeffs[..16].fill(0);
    } else if coeffs[0] != 0 {
        idct::idct_dc_add(coeffs[0], dst, pos, stride);
        coeffs[0] = 0;
    }
}

/// Runs the inverse WHT of the Y2 block into the luma DCs.
fn apply_y2(coeffs: &mut [i16; 400]) {
    let mut y2 = [0i16; 16];
    y2.copy_from_slice(&coeffs[384..400]);
    coeffs[384..400].fill(0);
    idct::inverse_wht(&y2, &mut coeffs[..256]);
}

/// Fills the border of an intra-prediction workspace from the frame
/// (Section 12.2): 127 above the frame, 129 left of it, the above-left
/// pixel 127 on the top row and 129 on the left column.
#[allow(clippy::too_many_arguments)]
fn fill_ws_border(ws: &mut [u8], ws_stride: usize, plane: &Plane, n: usize, mbx: usize, mby: usize, above_right: bool, last_col: bool) {
    let s = plane.w;
    let x = mbx * n;
    let y = mby * n;
    let extra = if above_right { 4 } else { 0 };
    if mby == 0 {
        ws[..1 + n + extra].fill(127);
    } else {
        let a = (y - 1) * s + x;
        ws[1..1 + n].copy_from_slice(&plane.data[a..a + n]);
        if above_right {
            if last_col {
                let v = plane.data[a + n - 1];
                ws[1 + n..1 + n + 4].fill(v);
            } else {
                ws[1 + n..1 + n + 4].copy_from_slice(&plane.data[a + n..a + n + 4]);
            }
        }
        ws[0] = if mbx == 0 { 129 } else { plane.data[a - 1] };
    }
    for r in 0..n {
        ws[(r + 1) * ws_stride] = if mbx == 0 { 129 } else { plane.data[(y + r) * s + x - 1] };
    }
}

fn store_ws(ws: &[u8], ws_stride: usize, plane: &mut Plane, n: usize, mbx: usize, mby: usize) {
    let s = plane.w;
    for r in 0..n {
        let d = (mby * n + r) * s + mbx * n;
        plane.data[d..d + n].copy_from_slice(&ws[(r + 1) * ws_stride + 1..(r + 1) * ws_stride + 1 + n]);
    }
}

#[allow(clippy::too_many_arguments)]
fn recon_intra(
    cur: &mut FrameBuf,
    mb: &MbInfo,
    mbx: usize,
    mby: usize,
    mb_cols: usize,
    coeffs: &mut [i16; 400],
    eobs: &[u8; 25],
    has_y2: bool,
) {
    // Luma.
    let mut ws = [0u8; LUMA_WS_SIZE];
    let st = LUMA_WS_STRIDE;
    fill_ws_border(&mut ws, st, &cur.y, 16, mbx, mby, true, mbx + 1 == mb_cols);
    if mb.y_mode == B_PRED {
        // Subblocks on the right use the macroblock's above-right pixels.
        let (top, rest) = ws.split_at_mut(st);
        for k in 1..4 {
            rest[(4 * k - 1) * st + 17..(4 * k - 1) * st + 21].copy_from_slice(&top[17..21]);
        }
        for b in 0..16 {
            let pos = (1 + (b >> 2) * 4) * st + 1 + (b & 3) * 4;
            predict::predict_subblock(&mut ws, st, pos, mb.bmodes[b]);
            add_residual(&mut coeffs[b * 16..b * 16 + 16], eobs[b], &mut ws, pos, st);
        }
    } else {
        predict::predict_block(&mut ws, st, 16, mb.y_mode, mby > 0, mbx > 0);
        if has_y2 && eobs_any(eobs) {
            apply_y2(coeffs);
        }
        for b in 0..16 {
            let pos = (1 + (b >> 2) * 4) * st + 1 + (b & 3) * 4;
            add_residual(&mut coeffs[b * 16..b * 16 + 16], eobs[b], &mut ws, pos, st);
        }
    }
    store_ws(&ws, st, &mut cur.y, 16, mbx, mby);

    // Chroma.
    for (plane, first) in [(&mut cur.u, 16usize), (&mut cur.v, 20usize)] {
        let mut ws = [0u8; CHROMA_WS_SIZE];
        let st = CHROMA_WS_STRIDE;
        fill_ws_border(&mut ws, st, plane, 8, mbx, mby, false, false);
        predict::predict_block(&mut ws, st, 8, mb.uv_mode, mby > 0, mbx > 0);
        for j in 0..4 {
            let b = first + j;
            let pos = (1 + (j >> 1) * 4) * st + 1 + (j & 1) * 4;
            add_residual(&mut coeffs[b * 16..b * 16 + 16], eobs[b], &mut ws, pos, st);
        }
        store_ws(&ws, st, plane, 8, mbx, mby);
    }
}

/// Whether the macroblock had tokens at all (so the Y2 block may be non-zero).
fn eobs_any(eobs: &[u8; 25]) -> bool {
    eobs[24] > 0
}

#[allow(clippy::too_many_arguments)]
fn recon_inter(
    cur: &mut FrameBuf,
    refb: &FrameBuf,
    mb: &MbInfo,
    x: usize,
    y: usize,
    coeffs: &mut [i16; 400],
    eobs: &[u8; 25],
    has_y2: bool,
    filters: &[predict::Filter; 8],
    bilinear: bool,
    full_pixel: bool,
) {
    let split = mb.y_mode == SPLITMV;
    // Luma.
    {
        let refp = RefPlane {
            data: &refb.y.data,
            w: refb.y.w,
            h: refb.y.h,
        };
        let s = cur.y.w;
        if !split {
            predict::inter_predict(
                &mut cur.y.data,
                y * s + x,
                s,
                &refp,
                x as i32,
                y as i32,
                16,
                16,
                mb.mv.x as i32,
                mb.mv.y as i32,
                filters,
                bilinear,
            );
        } else {
            for b in 0..16 {
                let bx = x + (b & 3) * 4;
                let by = y + (b >> 2) * 4;
                let mv = mb.mvs[b];
                predict::inter_predict(
                    &mut cur.y.data,
                    by * s + bx,
                    s,
                    &refp,
                    bx as i32,
                    by as i32,
                    4,
                    4,
                    mv.x as i32,
                    mv.y as i32,
                    filters,
                    bilinear,
                );
            }
        }
        if has_y2 && eobs_any(eobs) {
            apply_y2(coeffs);
        }
        for b in 0..16 {
            let pos = (y + (b >> 2) * 4) * s + x + (b & 3) * 4;
            add_residual(&mut coeffs[b * 16..b * 16 + 16], eobs[b], &mut cur.y.data, pos, s);
        }
    }

    // Chroma motion vectors (Section 17.4 of the reference decoder).
    let mut cmv = [(0i32, 0i32); 4];
    if !split {
        let cx = mb.mv.x as i32;
        let cy = mb.mv.y as i32;
        // Round half away from zero; the luma value is even, so this is exact.
        let mut c = ((cx + 1 + (cx >> 31) * 2) / 2, (cy + 1 + (cy >> 31) * 2) / 2);
        if full_pixel {
            c = (c.0 & !7, c.1 & !7);
        }
        cmv = [c; 4];
    } else {
        for (j, c) in cmv.iter_mut().enumerate() {
            let b = (j >> 1) * 8 + (j & 1) * 2;
            let mut sx = 0i32;
            let mut sy = 0i32;
            for k in [b, b + 1, b + 4, b + 5] {
                sx += mb.mvs[k].x as i32;
                sy += mb.mvs[k].y as i32;
            }
            let round = |t: i32| if t < 0 { (t - 4) / 8 } else { (t + 4) / 8 };
            let mut v = (round(sx), round(sy));
            if full_pixel {
                v = (v.0 & !7, v.1 & !7);
            }
            *c = v;
        }
    }
    let cx0 = x / 2;
    let cy0 = y / 2;
    for (plane, refplane, first) in [(&mut cur.u, &refb.u, 16usize), (&mut cur.v, &refb.v, 20usize)] {
        let refp = RefPlane {
            data: &refplane.data,
            w: refplane.w,
            h: refplane.h,
        };
        let s = plane.w;
        if !split {
            predict::inter_predict(
                &mut plane.data,
                cy0 * s + cx0,
                s,
                &refp,
                cx0 as i32,
                cy0 as i32,
                8,
                8,
                cmv[0].0,
                cmv[0].1,
                filters,
                bilinear,
            );
        } else {
            for (j, &(mx, my)) in cmv.iter().enumerate() {
                let bx = cx0 + (j & 1) * 4;
                let by = cy0 + (j >> 1) * 4;
                predict::inter_predict(&mut plane.data, by * s + bx, s, &refp, bx as i32, by as i32, 4, 4, mx, my, filters, bilinear);
            }
        }
        for j in 0..4 {
            let b = first + j;
            let pos = (cy0 + (j >> 1) * 4) * s + cx0 + (j & 1) * 4;
            add_residual(&mut coeffs[b * 16..b * 16 + 16], eobs[b], &mut plane.data, pos, s);
        }
    }
}
