//! Stride-aware 4:2:0 images: planes with any row and pixel stride
//! (I420-like, NV12-like or NV21-like), copied out as tight I420 and turned
//! clockwise by a quarter turn on the way. Camera images on Android and
//! frames pushed into a frame-in source both use it.

use crate::error::VideoError;

/// One plane of a 4:2:0 image: its bytes (which may stop short of
/// the last row's stride), the distance between rows, and between pixels
/// (1 for planar chroma, 2 for interleaved).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Plane<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) row_stride: usize,
    pub(crate) pixel_stride: usize,
}

/// A 4:2:0 image: Y, U and V planes. Chroma is half size, rounded up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Yuv420<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) planes: [Plane<'a>; 3],
}

/// The frame size after a turn of `rotation` degrees.
pub(crate) fn rotated_size(width: u32, height: u32, rotation: u32) -> (u32, u32) {
    if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    }
}

fn check(p: &Plane<'_>, cols: usize, rows: usize, what: &str) -> Result<(), VideoError> {
    let fail = |why: String| Err(VideoError::os(0, format!("image {what} plane: {why}")));
    if p.pixel_stride == 0 {
        return fail("pixel stride 0".into());
    }
    let row = (cols - 1) * p.pixel_stride + 1;
    if p.row_stride < row {
        return fail(format!(
            "row stride {} under a {row}-byte row",
            p.row_stride
        ));
    }
    let need = (rows - 1) * p.row_stride + row;
    if p.data.len() < need {
        return fail(format!("{} bytes, {need} needed", p.data.len()));
    }
    Ok(())
}

/// Copy one plane of `cols` x `rows` into `out` (tight), turned clockwise
/// by `rotation`.
fn copy_plane(p: &Plane<'_>, cols: usize, rows: usize, rotation: u32, out: &mut [u8]) {
    let (rs, ps) = (p.row_stride, p.pixel_stride);
    let at = |x: usize, y: usize| p.data[y * rs + x * ps];
    match rotation {
        0 => {
            for y in 0..rows {
                let dst = &mut out[y * cols..][..cols];
                if ps == 1 {
                    dst.copy_from_slice(&p.data[y * rs..][..cols]);
                } else {
                    for (x, d) in dst.iter_mut().enumerate() {
                        *d = at(x, y);
                    }
                }
            }
        }
        90 => {
            // Out is rows wide: out(x, y) = in(y, rows - 1 - x).
            for y in 0..cols {
                for x in 0..rows {
                    out[y * rows + x] = at(y, rows - 1 - x);
                }
            }
        }
        180 => {
            for y in 0..rows {
                for x in 0..cols {
                    out[y * cols + x] = at(cols - 1 - x, rows - 1 - y);
                }
            }
        }
        _ => {
            // 270: out(x, y) = in(cols - 1 - y, x).
            for y in 0..cols {
                for x in 0..rows {
                    out[y * rows + x] = at(cols - 1 - y, x);
                }
            }
        }
    }
}

/// The image as tight I420, turned clockwise by `rotation` (0, 90, 180 or
/// 270): the bytes and the new width and height. A plane that doesn't fit
/// its strides fails loudly.
pub(crate) fn to_i420(img: &Yuv420<'_>, rotation: u32) -> Result<(Vec<u8>, u32, u32), VideoError> {
    if img.width == 0 || img.height == 0 || img.width > 16_384 || img.height > 16_384 {
        return Err(VideoError::os(
            0,
            format!("a {}x{} image", img.width, img.height),
        ));
    }
    if !rotation.is_multiple_of(90) || rotation >= 360 {
        return Err(VideoError::Invalid(format!(
            "a rotation of {rotation} degrees"
        )));
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    check(&img.planes[0], w, h, "Y")?;
    check(&img.planes[1], cw, ch, "U")?;
    check(&img.planes[2], cw, ch, "V")?;
    let mut out = vec![0u8; w * h + 2 * cw * ch];
    let (luma, chroma) = out.split_at_mut(w * h);
    let (u, v) = chroma.split_at_mut(cw * ch);
    copy_plane(&img.planes[0], w, h, rotation, luma);
    copy_plane(&img.planes[1], cw, ch, rotation, u);
    copy_plane(&img.planes[2], cw, ch, rotation, v);
    let (ow, oh) = rotated_size(img.width, img.height, rotation);
    Ok((out, ow, oh))
}
