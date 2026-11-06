//! Frame buffers and the public picture types.

/// One plane of a frame buffer. Its stride is its width.
#[derive(Clone, Default)]
pub(crate) struct Plane {
    pub(crate) data: Vec<u8>,
    pub(crate) w: usize,
    pub(crate) h: usize,
}

impl Plane {
    fn new(w: usize, h: usize) -> Self {
        Plane { data: vec![0; w * h], w, h }
    }
}

/// A decoded frame at the macroblock-aligned coded size.
#[derive(Clone, Default)]
pub(crate) struct FrameBuf {
    pub(crate) y: Plane,
    pub(crate) u: Plane,
    pub(crate) v: Plane,
}

impl FrameBuf {
    pub(crate) fn new(mb_cols: usize, mb_rows: usize) -> Self {
        FrameBuf {
            y: Plane::new(mb_cols * 16, mb_rows * 16),
            u: Plane::new(mb_cols * 8, mb_rows * 8),
            v: Plane::new(mb_cols * 8, mb_rows * 8),
        }
    }
}

/// A borrowed planar YUV 4:2:0 (I420) picture.
///
/// The decoder returns one; the encoder takes one. Chroma planes are
/// `(width + 1) / 2` by `(height + 1) / 2`.
#[derive(Clone, Copy, Debug)]
pub struct Picture<'a> {
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// The luma plane; row `r` starts at `r * y_stride`.
    pub y: &'a [u8],
    /// The Cb plane; row `r` starts at `r * uv_stride`.
    pub u: &'a [u8],
    /// The Cr plane; row `r` starts at `r * uv_stride`.
    pub v: &'a [u8],
    /// Bytes between luma rows.
    pub y_stride: usize,
    /// Bytes between chroma rows.
    pub uv_stride: usize,
}

impl<'a> Picture<'a> {
    /// Width of the chroma planes.
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }

    /// Height of the chroma planes.
    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }

    /// Wraps a tightly packed I420 buffer (Y, then U, then V, no padding).
    /// Returns `None` if `data` is shorter than such a picture needs.
    pub fn from_i420(data: &'a [u8], width: u32, height: u32) -> Option<Picture<'a>> {
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let y_len = w * h;
        let c_len = cw * ch;
        if width == 0 || height == 0 || data.len() < y_len + 2 * c_len {
            return None;
        }
        Some(Picture {
            width,
            height,
            y: &data[..y_len],
            u: &data[y_len..y_len + c_len],
            v: &data[y_len + c_len..y_len + 2 * c_len],
            y_stride: w,
            uv_stride: cw,
        })
    }

    /// Appends the picture to `out` as tightly packed I420 (Y, U, V).
    pub fn write_i420(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (cw, ch) = (self.chroma_width() as usize, self.chroma_height() as usize);
        out.reserve(w * h + 2 * cw * ch);
        for r in 0..h {
            out.extend_from_slice(&self.y[r * self.y_stride..r * self.y_stride + w]);
        }
        for plane in [self.u, self.v] {
            for r in 0..ch {
                out.extend_from_slice(&plane[r * self.uv_stride..r * self.uv_stride + cw]);
            }
        }
    }

    /// The picture as a new, tightly packed I420 buffer.
    pub fn to_i420(&self) -> Vec<u8> {
        let mut v = Vec::new();
        self.write_i420(&mut v);
        v
    }
}
