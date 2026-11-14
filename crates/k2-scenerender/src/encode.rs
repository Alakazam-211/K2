//! Image encoders: WebP lossless (`image-webp`, pure Rust; the default) and
//! PNG (`png`). Both are deterministic: the same pixels give the same bytes.
//! RGB8, or RGBA8 (straight alpha) for a transparent image.

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
