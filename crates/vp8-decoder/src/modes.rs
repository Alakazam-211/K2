//! Per-macroblock prediction records and the near-MV search (RFC 6386
//! Section 16.3), shared by the decoder and the encoder.

use crate::tables::*;

/// A motion vector in quarter pixels times two (eighth pixels, always even
/// for luma), as the reference decoder stores them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Mv {
    pub x: i16,
    pub y: i16,
}

impl Mv {
    pub const ZERO: Mv = Mv { x: 0, y: 0 };

    pub fn is_zero(self) -> bool {
        self.x == 0 && self.y == 0
    }
}

/// What the decoder keeps about each macroblock.
#[derive(Clone, Copy)]
pub struct MbInfo {
    pub y_mode: u8,
    pub uv_mode: u8,
    pub ref_frame: u8,
    pub segment: u8,
    /// Any coefficient token was decoded (drives the loop filter's inner edges).
    pub has_coeffs: bool,
    pub mv: Mv,
    /// Subblock intra modes (for keyframe contexts); implied ones for the
    /// other intra modes.
    pub bmodes: [u8; 16],
    /// Per-subblock MVs (all equal unless SPLITMV; zero for intra).
    pub mvs: [Mv; 16],
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

pub fn implied_bmode(y_mode: u8) -> u8 {
    match y_mode {
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        TM_PRED => B_TM_PRED,
        _ => B_DC_PRED,
    }
}

pub struct Bounds {
    pub to_left: i32,
    pub to_right: i32,
    pub to_top: i32,
    pub to_bottom: i32,
}

impl Bounds {
    /// MV clamp bounds for the macroblock at (`row`, `col`), in eighth
    /// pixels: one macroblock beyond the frame.
    pub fn for_mb(row: usize, col: usize, mb_rows: usize, mb_cols: usize) -> Self {
        Bounds {
            to_left: -(((col + 1) << 7) as i32),
            to_right: ((mb_cols - col) << 7) as i32,
            to_top: -(((row + 1) << 7) as i32),
            to_bottom: ((mb_rows - row) << 7) as i32,
        }
    }

    pub fn clamp(&self, mv: Mv) -> Mv {
        Mv {
            x: (mv.x as i32).clamp(self.to_left, self.to_right) as i16,
            y: (mv.y as i32).clamp(self.to_top, self.to_bottom) as i16,
        }
    }
}

/// The near and nearest MVs and their weights (Section 16.3). Returns
/// `[best, nearest, near, _]` and the four counts.
pub fn find_near_mvs(
    above: &MbInfo,
    left: &MbInfo,
    above_left: &MbInfo,
    ref_frame: u8,
    sign_bias: &[bool; 4],
) -> ([Mv; 4], [u8; 4]) {
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
    cnt[3] = (above.y_mode == SPLITMV) as u8 * 2
        + (left.y_mode == SPLITMV) as u8 * 2
        + (above_left.y_mode == SPLITMV) as u8;
    if cnt[2] > cnt[1] {
        cnt.swap(1, 2);
        mvs.swap(1, 2);
    }
    if cnt[1] >= cnt[0] {
        mvs[0] = mvs[1];
    }
    (mvs, cnt)
}
