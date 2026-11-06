//! The real-time VP8 encoder.
//!
//! Each frame: rate control picks a quantizer; every macroblock gets a mode
//! (intra 16x16 or 4x4 on keyframes; zero, near, nearest or new motion, or
//! intra 16x16 on interframes), is predicted, transformed, quantized and
//! reconstructed exactly as a decoder will; then the frame is written with
//! probabilities fitted to its own token statistics. The encoder decodes its
//! own output with vp8-decoder to get the reference frames, so encoder and
//! receiver can never drift apart.

use vp8_decoder::bool_encoder::BoolEncoder;
use vp8_decoder::idct;
use vp8_decoder::modes::{find_near_mvs, implied_bmode, Bounds, MbInfo, Mv};
use vp8_decoder::predict::{self, RefPlane, CHROMA_WS_SIZE, CHROMA_WS_STRIDE, LUMA_WS_SIZE, LUMA_WS_STRIDE};
use vp8_decoder::tables::*;
use vp8_decoder::{Decoder, Picture};

use crate::rate::RateControl;
use crate::tokens::{self, Counter, Writer};
use crate::transform::{fdct4x4, fwht4x4, quantize, Quant};
use crate::{EncodeError, EncodedFrame, EncoderConfig, TemporalLayers};

/// Motion search range in whole pixels.
const SEARCH_RANGE: i32 = 48;
/// Padding around the motion-search copy of the reference luma.
const PAD: usize = 64;

/// Cost of a bool in 1/256 bits.
fn bit_cost(prob: u8, bit: bool) -> u32 {
    let p = if bit { 256 - prob as u32 } else { prob as u32 } as f64 / 256.0;
    (-p.log2() * 256.0).round() as u32
}

/// Cost of a tree-coded value in 1/256 bits.
fn tree_cost(tree: &[i8], probs: &[u8], value: u8) -> u32 {
    fn walk(tree: &[i8], probs: &[u8], node: usize, value: u8) -> Option<u32> {
        for bit in [false, true] {
            let next = tree[node + bit as usize];
            let c = bit_cost(probs[node >> 1], bit);
            if next <= 0 {
                if (-next) as u8 == value {
                    return Some(c);
                }
            } else if let Some(rest) = walk(tree, probs, next as usize, value) {
                return Some(c + rest);
            }
        }
        None
    }
    walk(tree, probs, 0, value).expect("value is a leaf of the tree")
}

/// Approximate cost in 1/256 bits of one MV component (quarter pixels).
fn mv_component_cost(v: i32) -> u32 {
    let a = v.unsigned_abs();
    if a == 0 {
        256
    } else if a < 8 {
        4 * 256
    } else {
        (12 + 2 * (32 - a.leading_zeros()).saturating_sub(3)) * 256
    }
}

#[inline]
fn sad16(a: &[u8], a_stride: usize, b: &[u8], b_stride: usize, n: usize) -> u32 {
    let mut s = 0u32;
    for r in 0..n {
        let ra = &a[r * a_stride..r * a_stride + n];
        let rb = &b[r * b_stride..r * b_stride + n];
        s += ra.iter().zip(rb).map(|(&x, &y)| x.abs_diff(y) as u32).sum::<u32>();
    }
    s
}

/// The reference frames of one frame: which one it predicts from, which it
/// replaces, and its temporal layer.
#[derive(Clone, Copy)]
pub(crate) struct LayerFrame {
    pub(crate) tid: u8,
    pub(crate) sync: bool,
    pub(crate) reference: u8,
    pub(crate) refresh_last: bool,
    pub(crate) refresh_golden: bool,
}

pub(crate) fn layer_frame(layers: TemporalLayers, n: u64) -> LayerFrame {
    let f = |tid, sync, reference, refresh_last, refresh_golden| LayerFrame {
        tid,
        sync,
        reference,
        refresh_last,
        refresh_golden,
    };
    match layers {
        TemporalLayers::L1T1 => f(0, false, LAST_FRAME, true, false),
        TemporalLayers::L1T2 => {
            if n % 2 == 0 {
                f(0, false, LAST_FRAME, true, false)
            } else {
                f(1, true, LAST_FRAME, false, false)
            }
        }
        TemporalLayers::L1T3 => match n % 4 {
            0 => f(0, false, LAST_FRAME, true, false),
            1 => f(2, true, LAST_FRAME, false, false),
            2 => f(1, true, LAST_FRAME, false, true),
            _ => f(2, false, GOLDEN_FRAME, false, false),
        },
    }
}

pub(crate) struct Core {
    pub(crate) cfg: EncoderConfig,
    mb_cols: usize,
    mb_rows: usize,
    aw: usize,
    ah: usize,
    src: [Vec<u8>; 3],
    rec: [Vec<u8>; 3],
    search: Vec<u8>,
    search_stride: usize,
    mbs: Vec<MbInfo>,
    levels: Vec<[i16; 400]>,
    skips: Vec<bool>,
    decoder: Decoder,
    rc: RateControl,
    frames_since_key: u64,
    key_pending: bool,
    started: bool,
}

/// A 4x4-mode luma candidate: the workspace, the modes, the levels and the cost.
type BPred = ([u8; LUMA_WS_SIZE], [u8; 16], [i16; 256], u32);

/// What one macroblock decision produced, for the cost comparisons.
struct Coded {
    info: MbInfo,
    levels: [i16; 400],
}

impl Core {
    pub(crate) fn new(cfg: EncoderConfig) -> Result<Self, EncodeError> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width > 16383 || cfg.height > 16383 {
            return Err(EncodeError::InvalidConfig("width and height must be 1 to 16383"));
        }
        if cfg.fps == 0 || cfg.bitrate_bps == 0 {
            return Err(EncodeError::InvalidConfig("fps and bitrate must be above zero"));
        }
        let mb_cols = (cfg.width as usize).div_ceil(16);
        let mb_rows = (cfg.height as usize).div_ceil(16);
        let aw = mb_cols * 16;
        let ah = mb_rows * 16;
        let weights = match cfg.temporal_layers {
            TemporalLayers::L1T1 => [1.0, 1.0, 1.0],
            TemporalLayers::L1T2 => [1.25, 0.75, 0.75],
            TemporalLayers::L1T3 => [1.4, 1.0, 0.8],
        };
        let rc = RateControl::new(cfg.bitrate_bps, cfg.max_bitrate_bps, cfg.fps, cfg.width * cfg.height, weights);
        let search_stride = aw + 2 * PAD;
        let mut decoder = Decoder::new();
        decoder.set_max_pixels(u64::MAX);
        Ok(Core {
            mb_cols,
            mb_rows,
            aw,
            ah,
            src: [vec![0; aw * ah], vec![0; aw * ah / 4], vec![0; aw * ah / 4]],
            rec: [vec![0; aw * ah], vec![0; aw * ah / 4], vec![0; aw * ah / 4]],
            search: vec![0; search_stride * (ah + 2 * PAD)],
            search_stride,
            mbs: vec![MbInfo::default(); (mb_cols + 1) * (mb_rows + 1)],
            levels: vec![[0; 400]; mb_cols * mb_rows],
            skips: vec![false; mb_cols * mb_rows],
            decoder,
            rc,
            frames_since_key: 0,
            key_pending: true,
            started: false,
            cfg,
        })
    }

    pub(crate) fn request_keyframe(&mut self) {
        self.key_pending = true;
    }

    pub(crate) fn set_rates(&mut self, bitrate_bps: u32, fps: u32) {
        self.cfg.bitrate_bps = bitrate_bps.max(1);
        self.cfg.max_bitrate_bps = self.cfg.max_bitrate_bps.max(self.cfg.bitrate_bps);
        self.cfg.fps = fps.max(1);
        self.rc.set_rates(self.cfg.bitrate_bps, self.cfg.max_bitrate_bps, self.cfg.fps);
    }

    pub(crate) fn encode(&mut self, pic: &Picture<'_>, force_keyframe: bool) -> Result<Option<EncodedFrame>, EncodeError> {
        if pic.width != self.cfg.width || pic.height != self.cfg.height {
            return Err(EncodeError::SizeMismatch {
                expected: (self.cfg.width, self.cfg.height),
                got: (pic.width, pic.height),
            });
        }
        let (w, h) = (pic.width as usize, pic.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        if pic.y_stride < w
            || pic.uv_stride < cw
            || pic.y.len() < (h - 1) * pic.y_stride + w
            || pic.u.len() < (ch - 1) * pic.uv_stride + cw
            || pic.v.len() < (ch - 1) * pic.uv_stride + cw
        {
            return Err(EncodeError::InvalidPicture);
        }
        let interval_due = self.cfg.keyframe_interval.is_some_and(|n| n > 0 && self.frames_since_key >= n as u64);
        let key = !self.started || force_keyframe || self.key_pending || interval_due;
        if !key && self.rc.should_drop() {
            self.rc.dropped();
            return Ok(None);
        }
        let layer = if key {
            LayerFrame {
                tid: 0,
                sync: true,
                reference: LAST_FRAME,
                refresh_last: true,
                refresh_golden: true,
            }
        } else {
            layer_frame(self.cfg.temporal_layers, self.frames_since_key)
        };
        let q = self.rc.pick_q(key, layer.tid as usize);
        self.load_source(pic);
        let data = self.encode_frame(key, q, &layer);
        // Decode our own output: it gives the references a receiver has.
        if let Err(e) = self.decoder.decode(&data) {
            return Err(EncodeError::Internal(format!("own output failed to decode: {e}")));
        }
        self.rc.update(data.len() * 8, key, layer.tid as usize, q);
        if key {
            self.frames_since_key = 1;
            self.key_pending = false;
            self.started = true;
        } else {
            self.frames_since_key += 1;
        }
        Ok(Some(EncodedFrame {
            data,
            keyframe: key,
            temporal_id: layer.tid,
            layer_sync: layer.sync,
            q_index: q,
        }))
    }

    /// Copies the picture into the macroblock-aligned source planes,
    /// repeating edge pixels into the padding.
    fn load_source(&mut self, pic: &Picture<'_>) {
        let (w, h) = (pic.width as usize, pic.height as usize);
        copy_padded(&mut self.src[0], self.aw, self.ah, pic.y, pic.y_stride, w, h);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        copy_padded(&mut self.src[1], self.aw / 2, self.ah / 2, pic.u, pic.uv_stride, cw, ch);
        copy_padded(&mut self.src[2], self.aw / 2, self.ah / 2, pic.v, pic.uv_stride, cw, ch);
    }

    fn encode_frame(&mut self, key: bool, q: u8, layer: &LayerFrame) -> Vec<u8> {
        let quant = Quant::new(q);
        // Lambda: SAD units per 1/256 bit.
        let lambda = AC_QUANT[q as usize] as u32 * 3 / 10;
        for m in self.mbs.iter_mut() {
            *m = MbInfo::default();
        }
        if !key {
            let reference = self
                .decoder
                .reference_frame(layer.reference)
                .expect("a keyframe has been decoded");
            fill_search(&mut self.search, self.search_stride, reference.y, self.aw, self.ah);
        }
        let stride = self.mb_cols + 1;
        for row in 0..self.mb_rows {
            for col in 0..self.mb_cols {
                let i = (row + 1) * stride + col + 1;
                let coded = if key {
                    self.code_intra(row, col, &quant, lambda, true)
                } else {
                    self.code_inter(row, col, &quant, lambda, layer.reference)
                };
                let k = row * self.mb_cols + col;
                self.skips[k] = coded.levels.iter().all(|&l| l == 0);
                self.levels[k] = coded.levels;
                self.mbs[i] = coded.info;
            }
        }
        let filter_level = loop_filter_level(q);
        self.write_frame(key, q, filter_level, layer)
    }

    /// Intra-codes a macroblock (16x16 or, on keyframes, 4x4 modes) into `rec`.
    fn code_intra(&mut self, row: usize, col: usize, quant: &Quant, lambda: u32, allow_bpred: bool) -> Coded {
        let key = allow_bpred;
        let mut levels = [0i16; 400];
        let mut info = MbInfo::default();
        let st = LUMA_WS_STRIDE;
        let aw = self.aw;
        let mut ws = [0u8; LUMA_WS_SIZE];
        predict::fill_ws_border(&mut ws, st, &self.rec[0], aw, 16, col, row, true, col + 1 == self.mb_cols);
        let src_pos = row * 16 * aw + col * 16;

        // Best whole-block mode.
        let ymode_probs: &[u8] = if key { &KF_YMODE_PROBS } else { &DEFAULT_YMODE_PROBS };
        let ytree: &[i8] = if key { &KF_YMODE_TREE } else { &YMODE_TREE };
        let mut best = (u32::MAX, DC_PRED);
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            let mut w = ws;
            predict::predict_block(&mut w, st, 16, mode, row > 0, col > 0);
            let sad = sad16(&w[st + 1..], st, &self.src[0][src_pos..], aw, 16);
            let cost = sad * 256 + lambda * tree_cost(ytree, ymode_probs, mode);
            if cost < best.0 {
                best = (cost, mode);
            }
        }

        // 4x4 modes, coded as we go so later subblocks predict from them.
        let mut bpred: Option<BPred> = None;
        if allow_bpred {
            let stride = self.mb_cols + 1;
            let i = (row + 1) * stride + col + 1;
            let above = self.mbs[i - stride].bmodes;
            let left = self.mbs[i - 1].bmodes;
            let mut w = ws;
            {
                let (top, rest) = w.split_at_mut(st);
                for k in 1..4 {
                    rest[(4 * k - 1) * st + 17..(4 * k - 1) * st + 21].copy_from_slice(&top[17..21]);
                }
            }
            let mut modes = [B_DC_PRED; 16];
            let mut blv = [0i16; 256];
            let mut total = lambda * tree_cost(ytree, ymode_probs, B_PRED);
            for b in 0..16 {
                let pos = (1 + (b >> 2) * 4) * st + 1 + (b & 3) * 4;
                let a = if b < 4 { above[b + 12] } else { modes[b - 4] };
                let l = if b & 3 == 0 { left[b + 3] } else { modes[b - 1] };
                let probs: &[u8] = if key { &KF_BMODE_PROBS[a as usize][l as usize] } else { &BMODE_PROBS };
                let sp = src_pos + (b >> 2) * 4 * aw + (b & 3) * 4;
                let mut best_b = (u32::MAX, B_DC_PRED);
                for mode in 0..10u8 {
                    predict::predict_subblock(&mut w, st, pos, mode);
                    let sad = sad16(&w[pos..], st, &self.src[0][sp..], aw, 4);
                    let cost = sad * 256 + lambda * tree_cost(&BMODE_TREE, probs, mode);
                    if cost < best_b.0 {
                        best_b = (cost, mode);
                    }
                }
                modes[b] = best_b.1;
                total += best_b.0;
                predict::predict_subblock(&mut w, st, pos, best_b.1);
                let lv = &mut blv[b * 16..b * 16 + 16];
                code_block(&self.src[0], sp, aw, &mut w, pos, st, quant.y1, 112, 0, lv);
                if total > best.0 {
                    break;
                }
            }
            if total < best.0 {
                bpred = Some((w, modes, blv, total));
            }
        }

        if let Some((w, modes, blv, _)) = bpred {
            info.y_mode = B_PRED;
            info.bmodes = modes;
            levels[..256].copy_from_slice(&blv);
            predict::store_ws(&w, st, &mut self.rec[0], aw, 16, col, row);
        } else {
            info.y_mode = best.1;
            info.bmodes = [implied_bmode(best.1); 16];
            predict::predict_block(&mut ws, st, 16, best.1, row > 0, col > 0);
            code_luma_y2(&self.src[0], src_pos, aw, &mut ws, st, 1, quant, if key { 112 } else { 96 }, &mut levels);
            predict::store_ws(&ws, st, &mut self.rec[0], aw, 16, col, row);
        }

        // Chroma: one mode for both planes.
        let cst = CHROMA_WS_STRIDE;
        let caw = aw / 2;
        let mut cws = [[0u8; CHROMA_WS_SIZE]; 2];
        for (p, w) in cws.iter_mut().enumerate() {
            predict::fill_ws_border(w, cst, &self.rec[1 + p], caw, 8, col, row, false, false);
        }
        let csrc = row * 8 * caw + col * 8;
        let uv_probs: &[u8] = if key { &KF_UV_MODE_PROBS } else { &DEFAULT_UV_MODE_PROBS };
        let mut best_uv = (u32::MAX, DC_PRED);
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            let mut sad = 0;
            for (p, ws) in cws.iter().enumerate() {
                let mut w = *ws;
                predict::predict_block(&mut w, cst, 8, mode, row > 0, col > 0);
                sad += sad16(&w[cst + 1..], cst, &self.src[1 + p][csrc..], caw, 8);
            }
            let cost = sad * 256 + lambda * tree_cost(&UV_MODE_TREE, uv_probs, mode);
            if cost < best_uv.0 {
                best_uv = (cost, mode);
            }
        }
        info.uv_mode = best_uv.1;
        for (p, w) in cws.iter_mut().enumerate() {
            predict::predict_block(w, cst, 8, best_uv.1, row > 0, col > 0);
            for j in 0..4 {
                let pos = (1 + (j >> 1) * 4) * cst + 1 + (j & 1) * 4;
                let sp = csrc + (j >> 1) * 4 * caw + (j & 1) * 4;
                let b = 16 + p * 4 + j;
                code_block(&self.src[1 + p], sp, caw, w, pos, cst, quant.uv, 112, 0, &mut levels[b * 16..b * 16 + 16]);
            }
            predict::store_ws(w, cst, &mut self.rec[1 + p], caw, 8, col, row);
        }
        Coded { info, levels }
    }

    /// Codes an interframe macroblock: motion search, mode choice (or intra), residual.
    fn code_inter(&mut self, row: usize, col: usize, quant: &Quant, lambda: u32, reference: u8) -> Coded {
        let stride = self.mb_cols + 1;
        let i = (row + 1) * stride + col + 1;
        let aw = self.aw;
        let x = col * 16;
        let y = row * 16;
        let src_pos = y * aw + x;
        let (near, cnt) = find_near_mvs(&self.mbs[i - stride], &self.mbs[i - 1], &self.mbs[i - stride - 1], reference, &[false; 4]);
        let bounds = Bounds::for_mb(row, col, self.mb_rows, self.mb_cols);
        let probs = [
            MODE_CONTEXTS[cnt[0] as usize][0],
            MODE_CONTEXTS[cnt[1] as usize][1],
            MODE_CONTEXTS[cnt[2] as usize][2],
            MODE_CONTEXTS[cnt[3] as usize][3],
        ];
        let nearest = bounds.clamp(near[1]);
        let nearv = bounds.clamp(near[2]);
        let best_mv = bounds.clamp(near[0]);
        let mode_cost = |m: u8| tree_cost(&MV_REF_TREE, &probs, m);

        // Candidates, in quarter pixels.
        let src = &self.src[0][src_pos..];
        let search = &self.search;
        let ss = self.search_stride;
        let in_range = |qx: i32, qy: i32| -> bool {
            let lim = SEARCH_RANGE * 4;
            qx.abs() <= lim && qy.abs() <= lim
        };
        let sad_q = |qx: i32, qy: i32| -> u32 { sad_subpel(src, aw, search, ss, x, y, qx, qy) };
        let new_cost = |qx: i32, qy: i32| -> u32 {
            mode_cost(NEWMV) + mv_component_cost(qx - best_mv.x as i32 / 2) + mv_component_cost(qy - best_mv.y as i32 / 2)
        };

        let zero_sad = sad_q(0, 0);
        let mut choice = (zero_sad * 256 + lambda * mode_cost(ZEROMV), ZEROMV, Mv::ZERO);
        for (m, mv) in [(NEARESTMV, nearest), (NEARMV, nearv)] {
            let (qx, qy) = (mv.x as i32 / 2, mv.y as i32 / 2);
            if !mv.is_zero() && in_range(qx, qy) {
                let c = sad_q(qx, qy) * 256 + lambda * mode_cost(m);
                if c < choice.0 {
                    choice = (c, m, mv);
                }
            }
        }
        // Static content: skip the search.
        if zero_sad > 256 * 2 {
            // Full-pixel diamond search from the best candidate.
            let mut starts = vec![(0i32, 0i32)];
            for mv in [nearest, nearv, best_mv] {
                let p = ((mv.x as i32 / 2 + 2) >> 2, (mv.y as i32 / 2 + 2) >> 2);
                if p.0.abs() <= SEARCH_RANGE && p.1.abs() <= SEARCH_RANGE {
                    starts.push(p);
                }
            }
            let full_cost = |px: i32, py: i32| -> u32 {
                let sad = sad16(src, aw, &search[(y as i32 + py + PAD as i32) as usize * ss + (x as i32 + px + PAD as i32) as usize..], ss, 16);
                sad * 256 + lambda * new_cost(px * 4, py * 4)
            };
            let mut best = (u32::MAX, 0, 0);
            for &(px, py) in &starts {
                let c = full_cost(px, py);
                if c < best.0 {
                    best = (c, px, py);
                }
            }
            for step in [4, 2, 1] {
                for _ in 0..16 {
                    let (bc, bx, by) = best;
                    let mut moved = false;
                    for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                        let (nx, ny) = (bx + dx, by + dy);
                        if nx.abs() > SEARCH_RANGE || ny.abs() > SEARCH_RANGE {
                            continue;
                        }
                        let c = full_cost(nx, ny);
                        if c < best.0 {
                            best = (c, nx, ny);
                            moved = true;
                        }
                    }
                    if !moved || best.0 == bc {
                        break;
                    }
                }
            }
            // Half, then quarter pixel refinement (bilinear estimate).
            let mut q = (best.1 * 4, best.2 * 4);
            let mut qcost = sad_q(q.0, q.1) * 256 + lambda * new_cost(q.0, q.1);
            for step in [2, 1] {
                let centre = q;
                for dy in [-step, 0, step] {
                    for dx in [-step, 0, step] {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (centre.0 + dx, centre.1 + dy);
                        if !in_range(nx, ny) {
                            continue;
                        }
                        let c = sad_q(nx, ny) * 256 + lambda * new_cost(nx, ny);
                        if c < qcost {
                            qcost = c;
                            q = (nx, ny);
                        }
                    }
                }
            }
            let mv = Mv {
                x: (q.0 * 2) as i16,
                y: (q.1 * 2) as i16,
            };
            let mode = if mv.is_zero() {
                ZEROMV
            } else if mv == nearest {
                NEARESTMV
            } else if mv == nearv {
                NEARMV
            } else {
                NEWMV
            };
            let c = if mode == NEWMV { qcost } else { sad_q(q.0, q.1) * 256 + lambda * mode_cost(mode) };
            if c < choice.0 {
                choice = (c, mode, mv);
            }
        }

        // Intra instead, if clearly better (it costs more in residual).
        if choice.0 > 256 * 256 * 3 {
            let mut ws = [0u8; LUMA_WS_SIZE];
            predict::fill_ws_border(&mut ws, LUMA_WS_STRIDE, &self.rec[0], aw, 16, col, row, true, col + 1 == self.mb_cols);
            let mut best_intra = u32::MAX;
            for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
                let mut w = ws;
                predict::predict_block(&mut w, LUMA_WS_STRIDE, 16, mode, row > 0, col > 0);
                let sad = sad16(&w[LUMA_WS_STRIDE + 1..], LUMA_WS_STRIDE, &self.src[0][src_pos..], aw, 16);
                best_intra = best_intra.min(sad * 256 + lambda * 256 * 6);
            }
            if best_intra + 256 * 256 < choice.0 {
                return self.code_intra(row, col, quant, lambda, false);
            }
        }

        let (_, mode, mv) = choice;
        let mut info = MbInfo {
            y_mode: mode,
            uv_mode: mode,
            ref_frame: reference,
            mv,
            mvs: [mv; 16],
            ..MbInfo::default()
        };
        info.bmodes = [B_DC_PRED; 16];
        let reference_pic = self.decoder.reference_frame(reference).expect("a reference frame");
        let mut levels = [0i16; 400];
        // Luma prediction straight into rec, then the residual.
        let refp = RefPlane {
            data: reference_pic.y,
            w: aw,
            h: self.ah,
        };
        predict::inter_predict(
            &mut self.rec[0],
            src_pos,
            aw,
            &refp,
            x as i32,
            y as i32,
            16,
            16,
            mv.x as i32,
            mv.y as i32,
            &predict::SIXTAP_FILTERS,
            false,
        );
        code_luma_y2(&self.src[0], src_pos, aw, &mut self.rec[0], src_pos, 0, quant, 96, &mut levels);
        // Chroma with the decoder's derived MV.
        let cx = mv.x as i32;
        let cy = mv.y as i32;
        let cmx = (cx + 1 + (cx >> 31) * 2) / 2;
        let cmy = (cy + 1 + (cy >> 31) * 2) / 2;
        let caw = aw / 2;
        let cpos = (y / 2) * caw + x / 2;
        for p in 0..2 {
            let plane = if p == 0 { reference_pic.u } else { reference_pic.v };
            let refp = RefPlane {
                data: plane,
                w: caw,
                h: self.ah / 2,
            };
            predict::inter_predict(
                &mut self.rec[1 + p],
                cpos,
                caw,
                &refp,
                (x / 2) as i32,
                (y / 2) as i32,
                8,
                8,
                cmx,
                cmy,
                &predict::SIXTAP_FILTERS,
                false,
            );
            for j in 0..4 {
                let off = (j >> 1) * 4 * caw + (j & 1) * 4;
                let b = 16 + p * 4 + j;
                let (src_plane, rec_plane) = (&self.src[1 + p], &mut self.rec[1 + p]);
                code_block(src_plane, cpos + off, caw, rec_plane, cpos + off, caw, quant.uv, 96, 0, &mut levels[b * 16..b * 16 + 16]);
            }
        }
        Coded { info, levels }
    }

    fn write_frame(&self, key: bool, q: u8, filter_level: u8, layer: &LayerFrame) -> Vec<u8> {
        let cols = self.mb_cols;
        let stride = cols + 1;

        // Token statistics with the default probabilities.
        let mut counter = Counter::new();
        self.walk_tokens(&mut counter);
        let mut probs = DEFAULT_COEFF_PROBS;
        let mut updates = [[[[false; 11]; 3]; 8]; 4];
        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        let [n0, n1] = counter.counts[i][j][k][l];
                        let total = n0 + n1;
                        if total == 0 {
                            continue;
                        }
                        let new = ((n0 as u64 * 256 + total as u64 / 2) / total as u64).clamp(1, 255) as u8;
                        let old = probs[i][j][k][l];
                        let cost = |p: u8| n0 as u64 * bit_cost(p, false) as u64 + n1 as u64 * bit_cost(p, true) as u64;
                        let up = COEFF_UPDATE_PROBS[i][j][k][l];
                        let overhead = 8 * 256 + bit_cost(up, true) as u64 - bit_cost(up, false) as u64;
                        if cost(new) + overhead < cost(old) {
                            probs[i][j][k][l] = new;
                            updates[i][j][k][l] = true;
                        }
                    }
                }
            }
        }

        // Frame-level probabilities from this frame's counts.
        let n = (cols * self.mb_rows) as u64;
        let prob_of = |zeros: u64, total: u64| -> u8 {
            match (zeros * 256 + total / 2).checked_div(total) {
                Some(p) => p.clamp(1, 255) as u8,
                None => 128,
            }
        };
        let mut n_noskip = 0u64;
        let mut n_intra = 0u64;
        let mut n_last = 0u64;
        let mut n_golden = 0u64;
        for row in 0..self.mb_rows {
            for col in 0..cols {
                let k = row * cols + col;
                let mb = &self.mbs[(row + 1) * stride + col + 1];
                n_noskip += !self.skips[k] as u64;
                match mb.ref_frame {
                    INTRA_FRAME => n_intra += 1,
                    LAST_FRAME => n_last += 1,
                    GOLDEN_FRAME => n_golden += 1,
                    _ => {}
                }
            }
        }
        let prob_skip = prob_of(n_noskip, n);
        let prob_intra = prob_of(n_intra, n);
        let prob_last = prob_of(n_last, n - n_intra);
        let prob_golden = prob_of(n_golden, n - n_intra - n_last);

        // The first partition: frame header, then the modes.
        let mut h = BoolEncoder::new();
        if key {
            h.write_bit(false); // colour space
            h.write_bit(false); // clamping required
        }
        h.write_bit(false); // no segmentation
        h.write_bit(false); // normal loop filter
        h.write_literal(filter_level as u32, 6);
        h.write_literal(0, 3); // sharpness
        h.write_bit(false); // no mode/ref filter deltas
        h.write_literal(0, 2); // one token partition
        h.write_literal(q as u32, 7);
        for _ in 0..5 {
            h.write_bit(false); // no quantizer deltas
        }
        if key {
            h.write_bit(false); // refresh_entropy_probs = 0
        } else {
            h.write_bit(layer.refresh_golden);
            h.write_bit(false); // alt-ref not refreshed
            if !layer.refresh_golden {
                h.write_literal(0, 2);
            }
            h.write_literal(0, 2); // nothing copied to alt-ref
            h.write_bit(false); // golden sign bias
            h.write_bit(false); // alt-ref sign bias
            // Probability updates last for this frame only, so a lost or
            // dropped frame never desynchronises a receiver's probabilities.
            h.write_bit(false);
            h.write_bit(layer.refresh_last);
        }
        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        let up = updates[i][j][k][l];
                        h.write(COEFF_UPDATE_PROBS[i][j][k][l], up);
                        if up {
                            h.write_literal(probs[i][j][k][l] as u32, 8);
                        }
                    }
                }
            }
        }
        h.write_bit(true); // mb_no_skip_coeff
        h.write_literal(prob_skip as u32, 8);
        if !key {
            h.write_literal(prob_intra as u32, 8);
            h.write_literal(prob_last as u32, 8);
            h.write_literal(prob_golden as u32, 8);
            h.write_bit(false); // no luma mode probability update
            h.write_bit(false); // no chroma mode probability update
            for &p in MV_UPDATE_PROBS.iter().flatten() {
                h.write(p, false);
            }
        }
        for row in 0..self.mb_rows {
            for col in 0..cols {
                let i = (row + 1) * stride + col + 1;
                let mb = &self.mbs[i];
                h.write(prob_skip, self.skips[row * cols + col]);
                if key {
                    h.write_tree(&KF_YMODE_TREE, &KF_YMODE_PROBS, mb.y_mode);
                    if mb.y_mode == B_PRED {
                        let above = &self.mbs[i - stride];
                        let left = &self.mbs[i - 1];
                        for b in 0..16 {
                            let a = if b < 4 { above.bmodes[b + 12] } else { mb.bmodes[b - 4] };
                            let l = if b & 3 == 0 { left.bmodes[b + 3] } else { mb.bmodes[b - 1] };
                            h.write_tree(&BMODE_TREE, &KF_BMODE_PROBS[a as usize][l as usize], mb.bmodes[b]);
                        }
                    }
                    h.write_tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS, mb.uv_mode);
                    continue;
                }
                h.write(prob_intra, mb.ref_frame != INTRA_FRAME);
                if mb.ref_frame == INTRA_FRAME {
                    h.write_tree(&YMODE_TREE, &DEFAULT_YMODE_PROBS, mb.y_mode);
                    if mb.y_mode == B_PRED {
                        for b in 0..16 {
                            h.write_tree(&BMODE_TREE, &BMODE_PROBS, mb.bmodes[b]);
                        }
                    }
                    h.write_tree(&UV_MODE_TREE, &DEFAULT_UV_MODE_PROBS, mb.uv_mode);
                    continue;
                }
                h.write(prob_last, mb.ref_frame != LAST_FRAME);
                if mb.ref_frame != LAST_FRAME {
                    h.write(prob_golden, mb.ref_frame == ALTREF_FRAME);
                }
                let (near, cnt) = find_near_mvs(&self.mbs[i - stride], &self.mbs[i - 1], &self.mbs[i - stride - 1], mb.ref_frame, &[false; 4]);
                let probs = [
                    MODE_CONTEXTS[cnt[0] as usize][0],
                    MODE_CONTEXTS[cnt[1] as usize][1],
                    MODE_CONTEXTS[cnt[2] as usize][2],
                    MODE_CONTEXTS[cnt[3] as usize][3],
                ];
                h.write_tree(&MV_REF_TREE, &probs, mb.y_mode);
                if mb.y_mode == NEWMV {
                    let best = Bounds::for_mb(row, col, self.mb_rows, cols).clamp(near[0]);
                    write_mv(&mut h, (mb.mv.y as i32 - best.y as i32) / 2, &DEFAULT_MV_PROBS[0]);
                    write_mv(&mut h, (mb.mv.x as i32 - best.x as i32) / 2, &DEFAULT_MV_PROBS[1]);
                }
            }
        }
        let part1 = h.finish();

        // The token partition.
        let mut t = BoolEncoder::new();
        {
            let mut w = Writer { enc: &mut t, probs: &probs };
            self.walk_tokens(&mut w);
        }
        let part2 = t.finish();

        let mut out = Vec::with_capacity(10 + part1.len() + part2.len());
        let tag = (!key as u32) | (1 << 4) | ((part1.len() as u32) << 5);
        out.extend_from_slice(&tag.to_le_bytes()[..3]);
        if key {
            out.extend_from_slice(&[0x9d, 0x01, 0x2a]);
            out.extend_from_slice(&(self.cfg.width as u16).to_le_bytes());
            out.extend_from_slice(&(self.cfg.height as u16).to_le_bytes());
        }
        out.extend_from_slice(&part1);
        out.extend_from_slice(&part2);
        out
    }

    /// Runs every macroblock's tokens through `sink` with the decoder's contexts.
    fn walk_tokens<S: tokens::TokenSink>(&self, sink: &mut S) {
        let cols = self.mb_cols;
        let stride = cols + 1;
        let mut above = vec![[0u8; 9]; cols];
        for row in 0..self.mb_rows {
            let mut left = [0u8; 9];
            for (col, above) in above.iter_mut().enumerate() {
                let k = row * cols + col;
                let mb = &self.mbs[(row + 1) * stride + col + 1];
                let has_y2 = mb.y_mode != B_PRED && mb.y_mode != SPLITMV;
                if self.skips[k] {
                    tokens::skip_mb(has_y2, above, &mut left);
                } else {
                    tokens::write_mb(sink, &self.levels[k], has_y2, above, &mut left);
                }
            }
        }
    }
}

/// One MV component (quarter pixels) with the default probabilities.
fn write_mv(h: &mut BoolEncoder, v: i32, p: &[u8; MV_PROB_COUNT]) {
    let a = v.unsigned_abs();
    debug_assert!(a < 1024, "MV component out of range: {v}");
    if a < 8 {
        h.write(p[MVP_IS_SHORT], false);
        h.write_tree(&SMALL_MV_TREE, &p[MVP_SHORT..MVP_SHORT + 7], a as u8);
    } else {
        h.write(p[MVP_IS_SHORT], true);
        for i in 0..3 {
            h.write(p[MVP_BITS + i], (a >> i) & 1 == 1);
        }
        for i in (4..MV_LONG_BITS).rev() {
            h.write(p[MVP_BITS + i], (a >> i) & 1 == 1);
        }
        if a & 0xfff0 != 0 {
            h.write(p[MVP_BITS + 3], (a >> 3) & 1 == 1);
        }
    }
    if a != 0 {
        h.write(p[MVP_SIGN], v < 0);
    }
}

fn loop_filter_level(q: u8) -> u8 {
    ((q as u32 * 3) / 8).min(63) as u8
}

fn copy_padded(dst: &mut [u8], dw: usize, dh: usize, src: &[u8], stride: usize, w: usize, h: usize) {
    for r in 0..dh {
        let s = &src[r.min(h - 1) * stride..r.min(h - 1) * stride + w];
        let d = &mut dst[r * dw..r * dw + dw];
        d[..w].copy_from_slice(s);
        let last = s[w - 1];
        d[w..].fill(last);
    }
}

/// Copies the reference luma into the padded search buffer, repeating edges.
fn fill_search(dst: &mut [u8], ds: usize, src: &[u8], w: usize, h: usize) {
    for r in 0..h + 2 * PAD {
        let sr = r.saturating_sub(PAD).min(h - 1);
        let s = &src[sr * w..sr * w + w];
        let d = &mut dst[r * ds..r * ds + ds];
        d[..PAD].fill(s[0]);
        d[PAD..PAD + w].copy_from_slice(s);
        d[PAD + w..].fill(s[w - 1]);
    }
}

/// SAD of the 16x16 block at (x, y) against the padded reference displaced by
/// (qx, qy) quarter pixels, with bilinear interpolation (an estimate of the
/// six-tap prediction, used only for the search).
#[allow(clippy::too_many_arguments)]
fn sad_subpel(src: &[u8], src_stride: usize, search: &[u8], ss: usize, x: usize, y: usize, qx: i32, qy: i32) -> u32 {
    let fx = (qx & 3) as u32;
    let fy = (qy & 3) as u32;
    let base = (y as i32 + (qy >> 2) + PAD as i32) as usize * ss + (x as i32 + (qx >> 2) + PAD as i32) as usize;
    if fx == 0 && fy == 0 {
        return sad16(src, src_stride, &search[base..], ss, 16);
    }
    let (w00, w01, w10, w11) = ((4 - fx) * (4 - fy), fx * (4 - fy), (4 - fx) * fy, fx * fy);
    let mut s = 0u32;
    for r in 0..16 {
        let a = &search[base + r * ss..base + r * ss + 17];
        let b = &search[base + (r + 1) * ss..base + (r + 1) * ss + 17];
        let o = &src[r * src_stride..r * src_stride + 16];
        for c in 0..16 {
            let p = (a[c] as u32 * w00 + a[c + 1] as u32 * w01 + b[c] as u32 * w10 + b[c + 1] as u32 * w11 + 8) >> 4;
            s += (p as i32 - o[c] as i32).unsigned_abs();
        }
    }
    s
}

/// Transforms, quantizes and reconstructs one 4x4 block (no Y2): the
/// prediction is in `rec` at `rpos`. Writes the levels and adds the
/// decoded residual to `rec`, exactly as a decoder will.
#[allow(clippy::too_many_arguments)]
fn code_block(
    src: &[u8],
    spos: usize,
    sstride: usize,
    rec: &mut [u8],
    rpos: usize,
    rstride: usize,
    q: [i32; 2],
    ac_bias: i32,
    first: usize,
    levels: &mut [i16],
) {
    let mut res = [0i32; 16];
    for r in 0..4 {
        for c in 0..4 {
            res[r * 4 + c] = src[spos + r * sstride + c] as i32 - rec[rpos + r * rstride + c] as i32;
        }
    }
    let mut co = [0i32; 16];
    fdct4x4(&res, &mut co);
    let mut deq = [0i16; 16];
    let mut any = false;
    for k in first..16 {
        let (step, bias) = if k == 0 { (q[0], 128) } else { (q[1], ac_bias) };
        let l = quantize(co[k], step, bias);
        levels[k] = l as i16;
        if l != 0 {
            any = true;
            deq[k] = (l * step) as i16;
        }
    }
    if any {
        idct::idct_add(&deq, rec, rpos, rstride);
    }
}

/// Codes a 16x16 luma block with a Y2 block: the prediction is in `rec`
/// (macroblock at `rpos`, stride `rstride`).
#[allow(clippy::too_many_arguments)]
fn code_luma_y2(
    src: &[u8],
    spos: usize,
    sstride: usize,
    rec: &mut [u8],
    rpos: usize,
    rstride_sel: usize,
    quant: &Quant,
    ac_bias: i32,
    levels: &mut [i16; 400],
) {
    // `rec` is either the luma workspace (stride LUMA_WS_STRIDE, offset to
    // the block) or the frame plane (stride `sstride`).
    let (rstride, rbase) = if rstride_sel == 1 {
        (LUMA_WS_STRIDE, LUMA_WS_STRIDE + 1)
    } else {
        (sstride, rpos)
    };
    let mut coeffs = [[0i32; 16]; 16];
    let mut dcs = [0i32; 16];
    for b in 0..16 {
        let so = spos + (b >> 2) * 4 * sstride + (b & 3) * 4;
        let ro = rbase + (b >> 2) * 4 * rstride + (b & 3) * 4;
        let mut res = [0i32; 16];
        for r in 0..4 {
            for c in 0..4 {
                res[r * 4 + c] = src[so + r * sstride + c] as i32 - rec[ro + r * rstride + c] as i32;
            }
        }
        fdct4x4(&res, &mut coeffs[b]);
        dcs[b] = coeffs[b][0];
    }
    let mut y2 = [0i32; 16];
    fwht4x4(&dcs, &mut y2);
    let mut y2_deq = [0i16; 16];
    for k in 0..16 {
        let step = quant.y2[(k > 0) as usize];
        let l = quantize(y2[k], step, 128);
        levels[384 + k] = l as i16;
        y2_deq[k] = (l * step) as i16;
    }
    let mut dc_out = [0i16; 256];
    if y2_deq.iter().any(|&v| v != 0) {
        idct::inverse_wht(&y2_deq, &mut dc_out);
    }
    for b in 0..16 {
        let mut deq = [0i16; 16];
        deq[0] = dc_out[b * 16];
        let mut any = deq[0] != 0;
        for k in 1..16 {
            let l = quantize(coeffs[b][k], quant.y1[1], ac_bias);
            levels[b * 16 + k] = l as i16;
            if l != 0 {
                any = true;
                deq[k] = (l * quant.y1[1]) as i16;
            }
        }
        if any {
            let ro = rbase + (b >> 2) * 4 * rstride + (b & 3) * 4;
            idct::idct_add(&deq, rec, ro, rstride);
        }
    }
}
