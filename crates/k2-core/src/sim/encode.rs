//! Frame pixels → images: decode a lane [`Frame`], scale it to points,
//! crop, and encode JPEG (the stream) or PNG (screenshots). Shared by every
//! lane; a `Jpeg` frame streams through untouched.

use image::{imageops, ImageEncoder, RgbImage};

use super::lane::{Frame, FrameData};
use super::tree::Rect;

/// Stream JPEG quality (S0: q70 ≈ 25 KB at 407x915, ~9 ms to encode).
pub const JPEG_QUALITY: u8 = 70;

/// Decode any frame to RGB.
pub fn to_rgb(f: &Frame) -> Result<RgbImage, String> {
    let (w, h) = (f.width_px, f.height_px);
    match &f.data {
        FrameData::Rgb888(b) => {
            RgbImage::from_raw(w, h, b.clone()).ok_or_else(|| format!("RGB888 frame of {} bytes is not {w}x{h}", b.len()))
        }
        FrameData::Rgba8888(b) => {
            if b.len() != (w as usize) * (h as usize) * 4 {
                return Err(format!("RGBA frame of {} bytes is not {w}x{h}", b.len()));
            }
            let mut out = Vec::with_capacity((w * h * 3) as usize);
            for px in b.chunks_exact(4) {
                out.extend_from_slice(&px[..3]);
            }
            RgbImage::from_raw(w, h, out).ok_or_else(|| "RGBA conversion".to_string())
        }
        FrameData::Jpeg(b) => image::load_from_memory_with_format(b, image::ImageFormat::Jpeg)
            .map(|i| i.to_rgb8())
            .map_err(|e| format!("decode JPEG frame: {e}")),
        FrameData::Png(b) => image::load_from_memory_with_format(b, image::ImageFormat::Png)
            .map(|i| i.to_rgb8())
            .map_err(|e| format!("decode PNG frame: {e}")),
    }
}

/// The frame's size in points (rounded).
pub fn points_size(f: &Frame) -> (u32, u32) {
    let s = if f.scale > 0.0 { f.scale } else { 1.0 };
    (((f.width_px as f32) / s).round().max(1.0) as u32, ((f.height_px as f32) / s).round().max(1.0) as u32)
}

/// Scale an image to `w`x`h` (no-op when it already is).
pub fn scale_to(img: RgbImage, w: u32, h: u32) -> RgbImage {
    if img.width() == w && img.height() == h {
        return img;
    }
    imageops::resize(&img, w, h, imageops::FilterType::Triangle)
}

/// Crop a points-resolution image to `r` (in points), clamped to the image.
pub fn crop_points(img: &RgbImage, r: Rect) -> Result<RgbImage, String> {
    let x = r.x.max(0.0).floor() as u32;
    let y = r.y.max(0.0).floor() as u32;
    if x >= img.width() || y >= img.height() {
        return Err("the element is off screen".into());
    }
    let w = (r.w.ceil() as u32).min(img.width() - x).max(1);
    let h = (r.h.ceil() as u32).min(img.height() - y).max(1);
    Ok(imageops::crop_imm(img, x, y, w, h).to_image())
}

pub fn jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(32 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("encode JPEG: {e}"))?;
    Ok(out)
}

pub fn png(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("encode PNG: {e}"))?;
    Ok(out)
}

/// A frame as a points-resolution JPEG `(jpeg, w, h)`. A `Jpeg` frame
/// already at points size passes through without a re-encode.
pub fn frame_jpeg(f: &Frame, quality: u8) -> Result<(Vec<u8>, u32, u32), String> {
    let (pw, ph) = points_size(f);
    if let FrameData::Jpeg(b) = &f.data {
        if f.width_px <= pw + 1 {
            return Ok((b.clone(), f.width_px, f.height_px));
        }
    }
    let img = scale_to(to_rgb(f)?, pw, ph);
    let (w, h) = (img.width(), img.height());
    Ok((jpeg(&img, quality)?, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::lane::Orientation;
    use std::time::Instant;

    fn frame(w: u32, h: u32, scale: f32) -> Frame {
        Frame {
            width_px: w,
            height_px: h,
            scale,
            orientation: Orientation::Portrait,
            seq: 1,
            captured_at: Instant::now(),
            data: FrameData::Rgb888(vec![200u8; (w * h * 3) as usize]),
        }
    }

    #[test]
    fn scales_to_points_and_encodes() {
        let f = frame(42, 84, 2.0);
        assert_eq!(points_size(&f), (21, 42));
        let (j, w, h) = frame_jpeg(&f, JPEG_QUALITY).expect("jpeg");
        assert_eq!((w, h), (21, 42));
        assert_eq!(&j[..2], &[0xff, 0xd8], "a JPEG");
        let back = to_rgb(&Frame { data: FrameData::Jpeg(j), width_px: 21, height_px: 42, scale: 1.0, ..f.clone() }).expect("decode");
        assert_eq!(back.dimensions(), (21, 42));
    }

    #[test]
    fn bad_sizes_fail_loudly_and_crops_clamp() {
        let mut f = frame(4, 4, 1.0);
        f.data = FrameData::Rgb888(vec![0; 5]);
        assert!(to_rgb(&f).is_err());
        let img = RgbImage::new(10, 10);
        let c = crop_points(&img, Rect { x: 8.0, y: 8.0, w: 5.0, h: 5.0 }).expect("crop");
        assert_eq!(c.dimensions(), (2, 2));
        assert!(crop_points(&img, Rect { x: 11.0, y: 0.0, w: 1.0, h: 1.0 }).is_err());
        assert!(!png(&img).expect("png").is_empty());
    }
}
