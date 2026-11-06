//! Picture scaling for simulcast layers and mismatched input sizes.

use vp8_decoder::Picture;

/// Scales `src` to `width` x `height` and appends it to `out` as packed
/// I420. Downscaling averages the source area under each output pixel;
/// upscaling interpolates bilinearly.
pub fn scale_i420(src: &Picture<'_>, width: u32, height: u32, out: &mut Vec<u8>) {
    let (dw, dh) = (width.max(1) as usize, height.max(1) as usize);
    let (sw, sh) = (src.width as usize, src.height as usize);
    scale_plane(src.y, src.y_stride, sw, sh, dw, dh, out);
    let (scw, sch) = (sw.div_ceil(2), sh.div_ceil(2));
    let (dcw, dch) = (dw.div_ceil(2), dh.div_ceil(2));
    scale_plane(src.u, src.uv_stride, scw, sch, dcw, dch, out);
    scale_plane(src.v, src.uv_stride, scw, sch, dcw, dch, out);
}

fn scale_plane(src: &[u8], stride: usize, sw: usize, sh: usize, dw: usize, dh: usize, out: &mut Vec<u8>) {
    out.reserve(dw * dh);
    if dw <= sw && dh <= sh {
        // Box filter in 16.16 fixed point over the source area of each pixel.
        for y in 0..dh {
            let y0 = y * sh / dh;
            let y1 = ((y + 1) * sh / dh).max(y0 + 1);
            for x in 0..dw {
                let x0 = x * sw / dw;
                let x1 = ((x + 1) * sw / dw).max(x0 + 1);
                let mut sum = 0u32;
                for r in y0..y1 {
                    sum += src[r * stride + x0..r * stride + x1].iter().map(|&p| p as u32).sum::<u32>();
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                out.push(((sum + n / 2) / n) as u8);
            }
        }
    } else {
        // Bilinear, pixel centres aligned.
        for y in 0..dh {
            let fy = ((2 * y + 1) * sh * 256 / (2 * dh)).saturating_sub(128);
            let y0 = (fy >> 8).min(sh - 1);
            let y1 = (y0 + 1).min(sh - 1);
            let wy = (fy & 255) as u32;
            for x in 0..dw {
                let fx = ((2 * x + 1) * sw * 256 / (2 * dw)).saturating_sub(128);
                let x0 = (fx >> 8).min(sw - 1);
                let x1 = (x0 + 1).min(sw - 1);
                let wx = (fx & 255) as u32;
                let p = |r: usize, c: usize| src[r * stride + c] as u32;
                let top = p(y0, x0) * (256 - wx) + p(y0, x1) * wx;
                let bot = p(y1, x0) * (256 - wx) + p(y1, x1) * wx;
                out.push(((top * (256 - wy) + bot * wy + 32768) >> 16) as u8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_and_doubles_flat_pictures() {
        let data = vec![77u8; 64 * 48 * 3 / 2];
        let pic = Picture::from_i420(&data, 64, 48).expect("picture");
        let mut out = Vec::new();
        scale_i420(&pic, 32, 24, &mut out);
        assert_eq!(out.len(), 32 * 24 * 3 / 2);
        assert!(out.iter().all(|&p| p == 77));
        out.clear();
        scale_i420(&pic, 100, 75, &mut out);
        assert_eq!(out.len(), 100 * 75 + 2 * 50 * 38);
        assert!(out.iter().all(|&p| p == 77));
    }
}
