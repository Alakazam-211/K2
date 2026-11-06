//! CPU colour conversion and scaling between I420, NV12, BGRA and RGBA.
//!
//! Everything goes through I420 at the source size, is scaled there
//! (bilinear; area-averaged when shrinking by 2 or more), then packed into
//! the target layout. RGB uses the frame's [`ColorSpace`].

use crate::error::{invalid, VideoError};
use crate::frame::{ColorSpace, FrameBuffer, PixelFormat, Planes, VideoFrame};

/// I420 planes, tightly packed, at one size.
pub(crate) struct Yuv {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
}

impl Yuv {
    fn cw(&self) -> usize {
        self.w.div_ceil(2)
    }
    fn ch(&self) -> usize {
        self.h.div_ceil(2)
    }
}

/// `frame` scaled to `width` x `height` and converted to `format` (I420,
/// NV12, BGRA or RGBA), as tight planes. A native buffer is read back
/// first. The same size and format: a cheap clone.
pub fn convert(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<VideoFrame, VideoError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(invalid(format!("convert to {width}x{height}")));
    }
    if format == PixelFormat::Native {
        return Err(invalid(
            "convert to Native: only a backend makes native buffers",
        ));
    }
    let src = cpu_planes(frame, format)?;
    let same_size = (width, height) == (frame.width, frame.height);
    if same_size && src.format == format {
        let mut out = frame.clone();
        out.buffer = FrameBuffer::Planes(src.to_tight(width, height));
        return Ok(out);
    }
    let yuv = to_yuv(&src, frame.width, frame.height, frame.color)?;
    let yuv = if same_size {
        yuv
    } else {
        scale_yuv(&yuv, width as usize, height as usize)
    };
    let planes = from_yuv(&yuv, format, frame.color);
    let mut out = VideoFrame::from_planes(planes, width, height, frame.capture_us)?;
    out.color = frame.color;
    Ok(out)
}

/// The frame's CPU planes, checked (a native buffer is read back, in
/// `prefer` when it can).
fn cpu_planes(frame: &VideoFrame, prefer: PixelFormat) -> Result<Planes, VideoError> {
    let p = match &frame.buffer {
        FrameBuffer::Planes(p) => p.clone(),
        FrameBuffer::Native(n) => {
            let want = if prefer == PixelFormat::Native {
                n.0.format()
            } else {
                prefer
            };
            n.0.to_planes(want)?
        }
    };
    p.validate(frame.width, frame.height)?;
    Ok(p)
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// RGB → (Y, U, V) coefficients (×256) for a colour space, and the luma
/// offset.
pub(crate) fn rgb_to_yuv(cs: ColorSpace, r: i32, g: i32, b: i32) -> (u8, u8, u8) {
    match cs {
        ColorSpace::Bt709Limited => (
            clamp(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16),
            clamp(((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128),
            clamp(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128),
        ),
        ColorSpace::Bt709Full => (
            clamp((54 * r + 183 * g + 18 * b + 128) >> 8),
            clamp(((-29 * r - 99 * g + 128 * b + 128) >> 8) + 128),
            clamp(((128 * r - 116 * g - 12 * b + 128) >> 8) + 128),
        ),
        _ => (
            clamp(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16),
            clamp(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128),
            clamp(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128),
        ),
    }
}

fn yuv_to_rgb(cs: ColorSpace, y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let (d, e) = (u as i32 - 128, v as i32 - 128);
    match cs {
        ColorSpace::Bt709Limited => {
            let c = 298 * (y as i32 - 16);
            (
                clamp((c + 459 * e + 128) >> 8),
                clamp((c - 55 * d - 136 * e + 128) >> 8),
                clamp((c + 541 * d + 128) >> 8),
            )
        }
        ColorSpace::Bt709Full => {
            let c = 256 * y as i32;
            (
                clamp((c + 403 * e + 128) >> 8),
                clamp((c - 48 * d - 120 * e + 128) >> 8),
                clamp((c + 475 * d + 128) >> 8),
            )
        }
        _ => {
            let c = 298 * (y as i32 - 16);
            (
                clamp((c + 409 * e + 128) >> 8),
                clamp((c - 100 * d - 208 * e + 128) >> 8),
                clamp((c + 516 * d + 128) >> 8),
            )
        }
    }
}

pub(crate) fn to_yuv(
    p: &Planes,
    width: u32,
    height: u32,
    cs: ColorSpace,
) -> Result<Yuv, VideoError> {
    p.validate(width, height)?;
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Yuv {
        w,
        h,
        y: vec![0; w * h],
        u: vec![0; cw * ch],
        v: vec![0; cw * ch],
    };
    match p.format {
        PixelFormat::I420 | PixelFormat::Nv12 => {
            for y in 0..h {
                out.y[y * w..][..w].copy_from_slice(p.row(0, y, width, height));
            }
            for y in 0..ch {
                if p.format == PixelFormat::I420 {
                    out.u[y * cw..][..cw].copy_from_slice(p.row(1, y, width, height));
                    out.v[y * cw..][..cw].copy_from_slice(p.row(2, y, width, height));
                } else {
                    let r = p.row(1, y, width, height);
                    for x in 0..cw {
                        out.u[y * cw + x] = r[2 * x];
                        out.v[y * cw + x] = r[2 * x + 1];
                    }
                }
            }
        }
        PixelFormat::Bgra | PixelFormat::Rgba => {
            let (ri, bi) = if p.format == PixelFormat::Bgra {
                (2, 0)
            } else {
                (0, 2)
            };
            let px = |x: usize, y: usize| {
                let r = p.row(0, y, width, height);
                (
                    r[4 * x + ri] as i32,
                    r[4 * x + 1] as i32,
                    r[4 * x + bi] as i32,
                )
            };
            for y in 0..h {
                for x in 0..w {
                    let (r, g, b) = px(x, y);
                    out.y[y * w + x] = rgb_to_yuv(cs, r, g, b).0;
                }
            }
            for y in 0..ch {
                for x in 0..cw {
                    let (mut r, mut g, mut b, mut n) = (0, 0, 0, 0);
                    for yy in 2 * y..(2 * y + 2).min(h) {
                        for xx in 2 * x..(2 * x + 2).min(w) {
                            let (pr, pg, pb) = px(xx, yy);
                            r += pr;
                            g += pg;
                            b += pb;
                            n += 1;
                        }
                    }
                    let (_, u, v) = rgb_to_yuv(cs, r / n, g / n, b / n);
                    out.u[y * cw + x] = u;
                    out.v[y * cw + x] = v;
                }
            }
        }
        PixelFormat::Native => return Err(invalid("CPU planes can't be Native")),
    }
    Ok(out)
}

pub(crate) fn from_yuv(yuv: &Yuv, format: PixelFormat, cs: ColorSpace) -> Planes {
    let (w, h, cw) = (yuv.w, yuv.h, yuv.cw());
    let mut data = Vec::with_capacity(format.frame_len(w as u32, h as u32));
    match format {
        PixelFormat::I420 => {
            data.extend_from_slice(&yuv.y);
            data.extend_from_slice(&yuv.u);
            data.extend_from_slice(&yuv.v);
        }
        PixelFormat::Nv12 => {
            data.extend_from_slice(&yuv.y);
            for (u, v) in yuv.u.iter().zip(&yuv.v) {
                data.push(*u);
                data.push(*v);
            }
        }
        _ => {
            let bgra = format == PixelFormat::Bgra;
            for y in 0..h {
                for x in 0..w {
                    let c = (y / 2) * cw + x / 2;
                    let (r, g, b) = yuv_to_rgb(cs, yuv.y[y * w + x], yuv.u[c], yuv.v[c]);
                    if bgra {
                        data.extend_from_slice(&[b, g, r, 255]);
                    } else {
                        data.extend_from_slice(&[r, g, b, 255]);
                    }
                }
            }
        }
    }
    let (strides, offsets) = crate::frame::tight_layout(format, w as u32, h as u32);
    Planes {
        format,
        data: data.into(),
        strides,
        offsets,
    }
}

pub(crate) fn scale_yuv(src: &Yuv, w: usize, h: usize) -> Yuv {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Yuv {
        w,
        h,
        y: vec![0; w * h],
        u: vec![0; cw * ch],
        v: vec![0; cw * ch],
    };
    scale_plane(&src.y, src.w, src.h, &mut out.y, w, h);
    scale_plane(&src.u, src.cw(), src.ch(), &mut out.u, cw, ch);
    scale_plane(&src.v, src.cw(), src.ch(), &mut out.v, cw, ch);
    out
}

fn scale_plane(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let get = |x: usize, y: usize| src[y * sw + x] as u32;
    if fx >= 2.0 || fy >= 2.0 {
        // Shrinking a lot: average each source box, so edges don't alias.
        for y in 0..dh {
            let y1 = (((y + 1) as f32 * fy) as usize).clamp(1, sh);
            let y0 = ((y as f32 * fy) as usize).min(y1 - 1);
            for x in 0..dw {
                let x1 = (((x + 1) as f32 * fx) as usize).clamp(1, sw);
                let x0 = ((x as f32 * fx) as usize).min(x1 - 1);
                let mut sum = 0u32;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        sum += get(xx, yy);
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as u32;
                dst[y * dw + x] = ((sum + n / 2) / n) as u8;
            }
        }
        return;
    }
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ay) = (sy as usize, sy - sy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, ax) = (sx as usize, sx - sx.floor());
            let x1 = (x0 + 1).min(sw - 1);
            let top = get(x0, y0) as f32 * (1.0 - ax) + get(x1, y0) as f32 * ax;
            let bottom = get(x0, y1) as f32 * (1.0 - ax) + get(x1, y1) as f32 * ax;
            dst[y * dw + x] = (top * (1.0 - ay) + bottom * ay + 0.5) as u8;
        }
    }
}

/// The size that fits `width` x `height` inside `box_w` x `box_h` keeping
/// the aspect ratio, rounded to even numbers (at least 2).
pub fn fit_within(width: u32, height: u32, box_w: u32, box_h: u32) -> (u32, u32) {
    if width <= box_w && height <= box_h {
        return (width, height);
    }
    let s = (box_w as f64 / width as f64).min(box_h as f64 / height as f64);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    (even(width as f64 * s), even(height as f64 * s))
}

/// The luma PSNR between two frames of the same size, in dB (100 for
/// identical frames). For tests and quality checks.
pub fn psnr_y(a: &VideoFrame, b: &VideoFrame) -> Result<f64, VideoError> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(invalid(format!(
            "{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        )));
    }
    let ya = to_yuv(
        &cpu_planes(a, PixelFormat::I420)?,
        a.width,
        a.height,
        a.color,
    )?;
    let yb = to_yuv(
        &cpu_planes(b, PixelFormat::I420)?,
        b.width,
        b.height,
        b.color,
    )?;
    let sse: u64 =
        ya.y.iter()
            .zip(&yb.y)
            .map(|(p, q)| ((*p as i64 - *q as i64).pow(2)) as u64)
            .sum();
    if sse == 0 {
        return Ok(100.0);
    }
    let mse = sse as f64 / ya.y.len() as f64;
    Ok(10.0 * (255.0 * 255.0 / mse).log10())
}

/// Luma at (`x`, `y`) of a CPU frame (tests and the fake's frame code).
pub(crate) fn luma_plane(frame: &VideoFrame) -> Result<(Vec<u8>, usize), VideoError> {
    let yuv = to_yuv(
        &cpu_planes(frame, PixelFormat::I420)?,
        frame.width,
        frame.height,
        frame.color,
    )?;
    Ok((yuv.y, yuv.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> VideoFrame {
        let (cw, ch) = ((w as usize).div_ceil(2), (h as usize).div_ceil(2));
        let mut buf = Vec::new();
        for y in 0..h as usize {
            for x in 0..w as usize {
                buf.push((40 + x * 2 + y) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((100 + x + y % 2) as u8);
            }
        }
        for y in 0..ch {
            for x in 0..cw {
                buf.push((140 + y / 2 + x % 3) as u8);
            }
        }
        VideoFrame::from_planes(
            Planes::tight(PixelFormat::I420, w, h, buf).expect("planes"),
            w,
            h,
            5,
        )
        .expect("frame")
    }

    #[test]
    fn nv12_and_i420_convert_both_ways_exactly() {
        for (w, h) in [(16, 8), (7, 5)] {
            let a = gradient(w, h);
            let n = convert(&a, w, h, PixelFormat::Nv12).expect("nv12");
            assert_eq!(n.format(), PixelFormat::Nv12);
            let back = convert(&n, w, h, PixelFormat::I420).expect("i420");
            assert_eq!(back.planes(), a.planes());
            assert_eq!(back.capture_us, 5);
        }
    }

    #[test]
    fn rgb_round_trips_closely() {
        let a = gradient(32, 16);
        for f in [PixelFormat::Bgra, PixelFormat::Rgba] {
            let rgb = convert(&a, 32, 16, f).expect("rgb");
            let back = convert(&rgb, 32, 16, PixelFormat::I420).expect("back");
            let p = psnr_y(&a, &back).expect("psnr");
            assert!(p > 40.0, "{f:?}: {p:.1} dB");
        }
        // Pure colours land where BT.601 puts them.
        let white = VideoFrame::filled(PixelFormat::Bgra, 4, 4, [235, 128, 128], 0).expect("white");
        assert!(white.planes().expect("p").data.iter().all(|&b| b == 255));
        let black = VideoFrame::filled(PixelFormat::Rgba, 4, 4, [16, 128, 128], 0).expect("black");
        assert!(black
            .planes()
            .expect("p")
            .data
            .chunks(4)
            .all(|px| px == [0, 0, 0, 255]));
    }

    #[test]
    fn scaling_keeps_flat_colour_and_follows_gradients() {
        let flat = VideoFrame::filled(PixelFormat::Nv12, 64, 48, [100, 90, 160], 9).expect("flat");
        for (w, h) in [(32, 24), (16, 12), (96, 72), (31, 17)] {
            let s = convert(&flat, w, h, PixelFormat::Nv12).expect("scale");
            assert_eq!((s.width, s.height, s.capture_us), (w, h, 9));
            let p = s.planes().expect("planes");
            assert!(p.data[..(w * h) as usize].iter().all(|&v| v == 100));
            assert_eq!(&p.data[(w * h) as usize..][..2], &[90, 160]);
            let back = convert(&s, 64, 48, PixelFormat::I420).expect("back");
            assert_eq!(psnr_y(&flat, &back).expect("psnr"), 100.0);
        }
        let g = gradient(64, 64);
        let half = convert(&g, 32, 32, PixelFormat::I420).expect("half");
        let up = convert(&half, 64, 64, PixelFormat::I420).expect("up");
        let p = psnr_y(&g, &up).expect("psnr");
        assert!(
            p > 25.0,
            "a smooth gradient survives half and back: {p:.1} dB"
        );
        assert!(convert(&g, 0, 4, PixelFormat::I420).is_err());
        assert!(convert(&g, 4, 4, PixelFormat::Native).is_err());
    }

    #[test]
    fn fit_keeps_the_aspect_and_even_sizes() {
        assert_eq!(fit_within(1280, 720, 640, 640), (640, 360));
        assert_eq!(fit_within(1280, 720, 320, 100), (178, 100));
        assert_eq!(
            fit_within(320, 240, 640, 480),
            (320, 240),
            "never scales up"
        );
    }

    #[test]
    fn psnr_needs_the_same_size() {
        let a = VideoFrame::filled(PixelFormat::I420, 8, 8, [16, 128, 128], 0).expect("a");
        let b = VideoFrame::filled(PixelFormat::I420, 8, 8, [26, 128, 128], 0).expect("b");
        let p = psnr_y(&a, &b).expect("psnr");
        assert!((p - 28.13).abs() < 0.01, "{p}");
        let small = convert(&a, 4, 4, PixelFormat::I420).expect("s");
        assert!(psnr_y(&a, &small).is_err());
    }
}
