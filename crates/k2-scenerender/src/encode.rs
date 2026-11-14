//! Image encoders: WebP lossless (`image-webp`, pure Rust; the default) and
//! PNG (`png`). Both are deterministic: the same pixels give the same bytes.
//! RGB8, or RGBA8 (straight alpha) for a transparent image. An id image
//! ([`ids_png`]) is always PNG, 24 bits a pixel.

use crate::proto::ImageFormat;
use crate::raster::Image;

pub fn encode(img: &Image, f: ImageFormat) -> Result<Vec<u8>, String> {
    match f {
        ImageFormat::Webp => webp(img),
        ImageFormat::Png => png(img),
    }
}

pub fn webp(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(&img.pixels, img.width, img.height, if img.alpha { image_webp::ColorType::Rgba8 } else { image_webp::ColorType::Rgb8 })
        .map_err(|e| format!("webp: {e}"))?;
    Ok(out)
}

pub fn png(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(if img.alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| format!("png: {e}"))?;
        w.write_image_data(&img.pixels).map_err(|e| format!("png: {e}"))?;
        w.finish().map_err(|e| format!("png: {e}"))?;
    }
    Ok(out)
}

/// An id image ([`crate::raster::render_with_ids`]) as PNG: 8-bit RGB (colour type 2), each
/// pixel's value in 24 bits, big-endian: `value = r << 16 | g << 8 | b` (0: nothing; else the
/// object's index + 1). No gamma, colour profile or sRGB chunk, so a browser's canvas reads the
/// values back exactly. A value above 2^24 - 1 (never: a scene has far fewer objects) is refused.
pub fn ids_png(ids: &[u32], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if ids.len() as u64 != width as u64 * height as u64 {
        return Err(format!("ids png: {} values for {width} x {height}", ids.len()));
    }
    let mut rgb = Vec::with_capacity(ids.len() * 3);
    for &v in ids {
        if v > MAX_ID_VALUE {
            return Err(format!("ids png: {v} doesn't fit 24 bits"));
        }
        rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    png(&Image { width, height, alpha: false, pixels: rgb })
}

/// The largest value an id image holds.
pub const MAX_ID_VALUE: u32 = (1 << 24) - 1;

/// The values of an id image [`ids_png`] wrote (for the tests and the tools; the worker never
/// decodes).
pub fn ids_from_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u32>), String> {
    let mut r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().map_err(|e| format!("ids png: {e}"))?;
    let (w, h, color, depth) = {
        let i = r.info();
        (i.width, i.height, i.color_type, i.bit_depth)
    };
    if color != png::ColorType::Rgb || depth != png::BitDepth::Eight {
        return Err(format!("ids png: {color:?} {depth:?}, not 8-bit RGB"));
    }
    let mut buf = vec![0u8; r.output_buffer_size().ok_or("ids png: too large")?];
    let n = r.next_frame(&mut buf).map_err(|e| format!("ids png: {e}"))?.buffer_size();
    let px: Vec<u32> = buf[..n].chunks_exact(3).map(|c| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect();
    if px.len() as u64 != w as u64 * h as u64 {
        return Err("ids png: short".into());
    }
    Ok((w, h, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_image_round_trips_in_24_bits() {
        let ids = vec![0, 1, 2, 255, 256, 65_535, 65_536, 100_001, MAX_ID_VALUE, 7, 0, 3];
        let png = ids_png(&ids, 4, 3).expect("encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!((png[24], png[25]), (8, 2), "8-bit RGB");
        assert!(!png.windows(4).any(|w| w == b"gAMA" || w == b"sRGB" || w == b"iCCP"), "no colour management");
        assert_eq!(ids_from_png(&png).expect("decode"), (4, 3, ids.clone()));
        assert_eq!(ids_png(&ids, 4, 3).expect("again"), png, "deterministic");
        assert!(ids_png(&[MAX_ID_VALUE + 1], 1, 1).is_err());
        assert!(ids_png(&ids, 5, 3).is_err());
    }
}
