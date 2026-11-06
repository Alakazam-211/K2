//! Screen images on Android: what an `ImageReader` behind a MediaProjection
//! `VirtualDisplay` hands out (`RGBA_8888` or `RGBX_8888` with a row
//! stride wider than the row, or `YUV_420_888`), to tight NV12; and the
//! virtual display's size for a size box.

use crate::convert::rgb_to_yuv;
use crate::error::VideoError;
use crate::frame::{ColorSpace, PixelFormat, Planes, VideoFrame};
use crate::yuv::{to_i420, Yuv420};

/// One image from the screen's reader.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ScreenImage<'a> {
    /// 8-bit R, G, B, A (or X) per pixel: `pixel_stride` bytes apart (4),
    /// rows `row_stride` bytes apart. `data` may stop short of the last
    /// row's stride.
    Rgba {
        data: &'a [u8],
        width: u32,
        height: u32,
        row_stride: usize,
        pixel_stride: usize,
    },
    /// `YUV_420_888`.
    Yuv(Yuv420<'a>),
}

impl ScreenImage<'_> {
    pub(crate) fn size(&self) -> (u32, u32) {
        match self {
            ScreenImage::Rgba { width, height, .. } => (*width, *height),
            ScreenImage::Yuv(y) => (y.width, y.height),
        }
    }
}

fn bad(why: String) -> VideoError {
    VideoError::os(0, format!("screen image: {why}"))
}

fn check_size(width: u32, height: u32) -> Result<(), VideoError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(bad(format!("{width}x{height}")));
    }
    Ok(())
}

/// The RGBA image's layout, checked: every pixel it reads is in `data`.
fn check_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    pixel_stride: usize,
) -> Result<(), VideoError> {
    check_size(width, height)?;
    if pixel_stride < 4 {
        return Err(bad(format!("pixel stride {pixel_stride} under 4")));
    }
    let row = (width as usize - 1) * pixel_stride + 4;
    if row_stride < row {
        return Err(bad(format!(
            "row stride {row_stride} under a {row}-byte row"
        )));
    }
    let need = (height as usize - 1) * row_stride + row;
    if data.len() < need {
        return Err(bad(format!("{} bytes, {need} needed", data.len())));
    }
    Ok(())
}

/// The image as a tight NV12 frame (BT.601 limited range, the frames'
/// default), stamped `capture_us`.
pub(crate) fn to_nv12(img: &ScreenImage<'_>, capture_us: u64) -> Result<VideoFrame, VideoError> {
    let (w, h) = img.size();
    check_size(w, h)?;
    let (wu, hu) = (w as usize, h as usize);
    let (cw, ch) = (wu.div_ceil(2), hu.div_ceil(2));
    let mut out = vec![0u8; wu * hu + 2 * cw * ch];
    match *img {
        ScreenImage::Rgba {
            data,
            row_stride,
            pixel_stride,
            ..
        } => {
            check_rgba(data, w, h, row_stride, pixel_stride)?;
            let px = |x: usize, y: usize| {
                let at = y * row_stride + x * pixel_stride;
                (data[at] as i32, data[at + 1] as i32, data[at + 2] as i32)
            };
            let cs = ColorSpace::Bt601Limited;
            let (luma, chroma) = out.split_at_mut(wu * hu);
            for y in 0..hu {
                for x in 0..wu {
                    let (r, g, b) = px(x, y);
                    luma[y * wu + x] = rgb_to_yuv(cs, r, g, b).0;
                }
            }
            // Chroma from the average of each 2x2 block (cut short at an
            // odd edge).
            for cy in 0..ch {
                for cx in 0..cw {
                    let (mut r, mut g, mut b, mut n) = (0, 0, 0, 0);
                    for y in 2 * cy..(2 * cy + 2).min(hu) {
                        for x in 2 * cx..(2 * cx + 2).min(wu) {
                            let p = px(x, y);
                            r += p.0;
                            g += p.1;
                            b += p.2;
                            n += 1;
                        }
                    }
                    let (_, u, v) =
                        rgb_to_yuv(cs, (r + n / 2) / n, (g + n / 2) / n, (b + n / 2) / n);
                    chroma[cy * 2 * cw + 2 * cx] = u;
                    chroma[cy * 2 * cw + 2 * cx + 1] = v;
                }
            }
        }
        ScreenImage::Yuv(ref yuv) => {
            let (i420, _, _) = to_i420(yuv, 0)?;
            out[..wu * hu].copy_from_slice(&i420[..wu * hu]);
            let (u, v) = i420[wu * hu..].split_at(cw * ch);
            for (i, (u, v)) in u.iter().zip(v).enumerate() {
                out[wu * hu + 2 * i] = *u;
                out[wu * hu + 2 * i + 1] = *v;
            }
        }
    }
    VideoFrame::from_planes(
        Planes::tight(PixelFormat::Nv12, w, h, out)?,
        w,
        h,
        capture_us,
    )
}

/// An RGBA image's pixels as tight RGBA (alpha made opaque for RGBX).
pub(crate) fn rgba_tight(
    data: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    opaque: bool,
) -> Result<Vec<u8>, VideoError> {
    check_rgba(data, width, height, row_stride, 4)?;
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&data[y * row_stride..][..row]);
    }
    if opaque {
        out.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    Ok(out)
}

/// The virtual display's size: the display (`width` x `height`) fitted into
/// the size box, which is orientation-free (a 1280 x 720 box takes a
/// portrait screen as 720 x 1280). Even, at least 2 x 2. No box: the
/// display's own size, made even.
pub(crate) fn virtual_size(
    width: u32,
    height: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> (u32, u32) {
    let even = |v: u32| (v & !1).max(2);
    let (bw, bh) = crate::android::image::oriented_box(max_width, max_height, height > width);
    let (bw, bh) = (bw.unwrap_or(u32::MAX), bh.unwrap_or(u32::MAX));
    if width <= bw && height <= bh {
        return (even(width), even(height));
    }
    let (w, h) = crate::convert::fit_within(width, height, bw, bh);
    (even(w), even(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv::Plane;

    /// A `w` x `h` RGBA image, each row padded to `stride`: the left half
    /// white, the right half black, alpha 0 (RGBX leaves it undefined).
    fn halves(w: u32, h: u32, stride: usize) -> Vec<u8> {
        let mut data = vec![0xEE; stride * (h as usize - 1) + w as usize * 4];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let v = if x < w as usize / 2 { 255 } else { 0 };
                data[y * stride + x * 4..][..4].copy_from_slice(&[v, v, v, 0]);
            }
        }
        data
    }

    #[test]
    fn padded_rgba_becomes_tight_nv12() {
        // 6 x 3 with 32-byte rows (24 bytes of pixels): the last row stops
        // short at its pixels, as an ImageReader's buffer can.
        let data = halves(6, 3, 32);
        let img = ScreenImage::Rgba {
            data: &data,
            width: 6,
            height: 3,
            row_stride: 32,
            pixel_stride: 4,
        };
        let f = to_nv12(&img, 9).expect("nv12");
        assert_eq!(
            (f.format(), f.width, f.height, f.capture_us),
            (PixelFormat::Nv12, 6, 3, 9)
        );
        let p = f.planes().expect("planes");
        assert_eq!(p.strides, [6, 6, 0], "tight");
        for y in 0..3 {
            assert_eq!(p.row(0, y, 6, 3), &[235, 235, 235, 16, 16, 16], "row {y}");
        }
        // Chroma of grey is neutral; the middle block straddles both halves.
        for y in 0..2 {
            for &c in p.row(1, y, 6, 3) {
                assert!((126..=130).contains(&c), "neutral chroma: {c}");
            }
        }
    }

    #[test]
    fn colours_survive_the_trip() {
        // Pure red at 4 x 2, tight.
        let data: Vec<u8> = [255u8, 0, 0, 255].repeat(8);
        let img = ScreenImage::Rgba {
            data: &data,
            width: 4,
            height: 2,
            row_stride: 16,
            pixel_stride: 4,
        };
        let f = to_nv12(&img, 0).expect("nv12");
        let back = crate::convert::convert(&f, 4, 2, PixelFormat::Rgba).expect("rgba");
        let px = &back.planes().expect("planes").data[..4];
        assert!(px[0] > 240 && px[1] < 15 && px[2] < 15, "red: {px:?}");
    }

    #[test]
    fn yuv_screens_become_nv12_too() {
        // 4 x 2, Y padded to 6, interleaved chroma (NV12-like views).
        let y = [1u8, 2, 3, 4, 0xEE, 0xEE, 5, 6, 7, 8];
        let uv = [10u8, 20, 11, 21];
        let img = ScreenImage::Yuv(Yuv420 {
            width: 4,
            height: 2,
            planes: [
                Plane {
                    data: &y,
                    row_stride: 6,
                    pixel_stride: 1,
                },
                Plane {
                    data: &uv,
                    row_stride: 4,
                    pixel_stride: 2,
                },
                Plane {
                    data: &uv[1..],
                    row_stride: 4,
                    pixel_stride: 2,
                },
            ],
        });
        let f = to_nv12(&img, 0).expect("nv12");
        assert_eq!(
            &f.planes().expect("planes").data[..],
            &[1, 2, 3, 4, 5, 6, 7, 8, 10, 20, 11, 21]
        );
    }

    #[test]
    fn odd_sizes_and_bad_layouts() {
        let data = halves(3, 3, 12);
        let img = ScreenImage::Rgba {
            data: &data,
            width: 3,
            height: 3,
            row_stride: 12,
            pixel_stride: 4,
        };
        let f = to_nv12(&img, 0).expect("3x3");
        assert_eq!(f.planes().expect("planes").data.len(), 9 + 8);
        for (row_stride, pixel_stride, len, what) in [
            (11, 4, data.len(), "a row stride under the row"),
            (12, 3, data.len(), "a pixel stride under 4"),
            (12, 4, data.len() - 1, "a short buffer"),
        ] {
            let img = ScreenImage::Rgba {
                data: &data[..len],
                width: 3,
                height: 3,
                row_stride,
                pixel_stride,
            };
            assert!(to_nv12(&img, 0).is_err(), "{what}");
        }
        let img = ScreenImage::Rgba {
            data: &data,
            width: 0,
            height: 3,
            row_stride: 12,
            pixel_stride: 4,
        };
        assert!(to_nv12(&img, 0).is_err(), "zero width");
    }

    #[test]
    fn rgbx_is_made_opaque() {
        let data = halves(2, 2, 12);
        let t = rgba_tight(&data, 2, 2, 12, true).expect("tight");
        assert_eq!(
            t,
            vec![255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255]
        );
        let t = rgba_tight(&data, 2, 2, 12, false).expect("tight");
        assert_eq!(t[3], 0, "RGBA keeps its alpha");
        assert!(rgba_tight(&data, 2, 2, 7, false).is_err());
    }

    #[test]
    fn the_virtual_display_fits_the_box_either_way_round() {
        // A 1080 x 2400 phone held upright, a 1280 x 720 box: 576 x 1280.
        assert_eq!(virtual_size(1080, 2400, Some(1280), Some(720)), (576, 1280));
        // Turned: 1280 x 576.
        assert_eq!(virtual_size(2400, 1080, Some(1280), Some(720)), (1280, 576));
        // Small enough already, or no box: the display, made even.
        assert_eq!(virtual_size(641, 361, Some(1280), Some(720)), (640, 360));
        assert_eq!(virtual_size(1080, 2400, None, None), (1080, 2400));
        // Only a width: the height follows.
        assert_eq!(virtual_size(1080, 2400, Some(540), None), (540, 1200));
    }
}
