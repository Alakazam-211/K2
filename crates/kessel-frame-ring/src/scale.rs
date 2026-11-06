//! Fitting a frame into the ring's box, and copying or box-filtering its
//! planes straight into a slot.

use crate::Nv12Frame;

/// The size `width` x `height` takes in a `box_w` x `box_h` box that turns
/// with the frame (a portrait frame uses the box portrait). Unchanged when
/// it fits; else scaled down keeping the aspect, rounded down to even (at
/// least 2).
pub(crate) fn fit(width: u32, height: u32, box_w: u32, box_h: u32) -> (u32, u32) {
    let (bw, bh) = if (width > height) != (box_w > box_h) && width != height && box_w != box_h {
        (box_h, box_w)
    } else {
        (box_w, box_h)
    };
    if width <= bw && height <= bh {
        return (width, height);
    }
    let (w, h, bw, bh) = (width as u64, height as u64, bw as u64, bh as u64);
    // Width-limited when width / bw >= height / bh.
    let (dw, dh) = if w * bh >= h * bw {
        (bw, h * bw / w)
    } else {
        (w * bh / h, bh)
    };
    let even = |v: u64| ((v as u32) & !1).max(2);
    (even(dw), even(dh))
}

/// One plane: `cols` x `rows` samples of `channels` bytes, rows `stride`
/// apart.
struct Src<'a> {
    data: &'a [u8],
    stride: usize,
    cols: usize,
    rows: usize,
    channels: usize,
}

/// Write `src` into `dst` (tight, `dcols` x `drows`), averaging the box of
/// source samples under each destination sample.
fn plane(src: &Src<'_>, dst: &mut [u8], dcols: usize, drows: usize) {
    let ch = src.channels;
    if (dcols, drows) == (src.cols, src.rows) {
        let row = dcols * ch;
        for y in 0..drows {
            dst[y * row..][..row].copy_from_slice(&src.data[y * src.stride..][..row]);
        }
        return;
    }
    for dy in 0..drows {
        let y0 = dy * src.rows / drows;
        let y1 = ((dy + 1) * src.rows / drows).max(y0 + 1);
        for dx in 0..dcols {
            let x0 = dx * src.cols / dcols;
            let x1 = ((dx + 1) * src.cols / dcols).max(x0 + 1);
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            for c in 0..ch {
                let mut sum = 0u64;
                for y in y0..y1 {
                    let row = &src.data[y * src.stride..];
                    for x in x0..x1 {
                        sum += row[x * ch + c] as u64;
                    }
                }
                dst[(dy * dcols + dx) * ch + c] = ((sum + n / 2) / n) as u8;
            }
        }
    }
}

/// Write `f` (already validated) into `dst` as tight NV12 of `dw` x `dh`.
/// `dst` holds at least [`crate::nv12_len`] of that size.
pub(crate) fn write_nv12(f: &Nv12Frame<'_>, dw: u32, dh: u32, dst: &mut [u8]) {
    let (w, h) = (f.width as usize, f.height as usize);
    let (dw, dh) = (dw as usize, dh as usize);
    let (luma, chroma) = dst.split_at_mut(dw * dh);
    plane(
        &Src {
            data: f.y,
            stride: f.y_stride,
            cols: w,
            rows: h,
            channels: 1,
        },
        luma,
        dw,
        dh,
    );
    plane(
        &Src {
            data: f.uv,
            stride: f.uv_stride,
            cols: w.div_ceil(2),
            rows: h.div_ceil(2),
            channels: 2,
        },
        chroma,
        dw.div_ceil(2),
        dh.div_ceil(2),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_turns_the_box_with_the_frame() {
        // An iPhone screen held upright into a 1280 x 720 box.
        assert_eq!(fit(1170, 2532, 1280, 720), (590, 1280));
        // Turned.
        assert_eq!(fit(2532, 1170, 1280, 720), (1280, 590));
        // Fits: unchanged, odd sizes too.
        assert_eq!(fit(641, 359, 1280, 720), (641, 359));
        assert_eq!(fit(359, 641, 1280, 720), (359, 641));
        // Square frames and boxes.
        assert_eq!(fit(2000, 2000, 1280, 720), (720, 720));
        assert_eq!(fit(4000, 1000, 1000, 1000), (1000, 250));
        // Tiny results stay at least 2.
        assert_eq!(fit(16_000, 4, 64, 48), (64, 2));
    }

    #[test]
    fn box_filtering_averages_and_copying_is_exact() {
        // 4 x 2 luma with 6-byte rows, scaled to 2 x 1: the average of each
        // 2 x 2 block.
        let y = [10, 20, 30, 40, 0xEE, 0xEE, 30, 40, 50, 60, 0xEE, 0xEE];
        let uv = [100, 200, 110, 210, 0xEE, 0xEE];
        let f = Nv12Frame::new(&y, 6, &uv, 6, 4, 2);
        let mut out = vec![0u8; 2 + 2];
        write_nv12(&f, 2, 1, &mut out);
        assert_eq!(&out[..2], &[25, 45]);
        // Chroma 2 x 1 to 1 x 1: U and V averaged apart.
        assert_eq!(&out[2..], &[105, 205]);
        // The same size: a tight copy without the padding.
        let mut out = vec![0u8; crate::nv12_len(4, 2)];
        write_nv12(&f, 4, 2, &mut out);
        assert_eq!(
            out,
            vec![10, 20, 30, 40, 30, 40, 50, 60, 100, 200, 110, 210]
        );
    }
}
