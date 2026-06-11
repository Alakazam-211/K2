use std::collections::HashMap;

use fontdue::{Font, FontSettings};

// ── Embedded font data ──────────────────────────────────────────────────

static FONT_REGULAR: &[u8] = include_bytes!("../../fonts/MesloLGMNerdFontMono-Regular.ttf");
static FONT_BOLD: &[u8] = include_bytes!("../../fonts/MesloLGMNerdFontMono-Bold.ttf");
static FONT_ITALIC: &[u8] = include_bytes!("../../fonts/MesloLGMNerdFontMono-Italic.ttf");

// ── Font style indices ──────────────────────────────────────────────────

pub const STYLE_REGULAR: u8 = 0;
pub const STYLE_BOLD: u8 = 1;
pub const STYLE_ITALIC: u8 = 2;
pub const STYLE_BOLD_ITALIC: u8 = 3; // uses bold font (no separate bold-italic TTF)

// ── Glyph cache key ─────────────────────────────────────────────────────

#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    ch: char,
    /// Font size in tenths of a pixel (e.g. 130 = 13.0px) to avoid float hashing.
    font_size_tenths: u16,
    style: u8,
}

// ── Rasterized glyph ────────────────────────────────────────────────────

pub struct RasterizedGlyph {
    /// Alpha-only bitmap (one byte per pixel).
    pub bitmap: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Horizontal offset from cell left edge to glyph origin.
    pub x_offset: i32,
    /// Vertical offset from cell top edge to glyph top.
    pub y_offset: i32,
}

// ── Glyph cache ─────────────────────────────────────────────────────────

pub struct GlyphCache {
    /// [regular, bold, italic] — bold_italic falls back to bold.
    fonts: [Font; 3],
    cache: HashMap<GlyphKey, RasterizedGlyph>,

    // ── Public cell metrics ──
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: u32,

    /// The raw font size (before DPR scaling).
    font_size: f32,
    /// Device pixel ratio.
    dpr: f32,
    /// Effective pixel size = font_size * dpr.
    px_size: f32,
}

impl GlyphCache {
    /// Create a new glyph cache with the given logical font size and DPR.
    /// NOTE: We cap DPR at 1.0 for bitmap rendering to keep frame sizes manageable.
    /// The browser handles upscaling via CSS — slightly less crisp but 4x less data.
    pub fn new(font_size: f32, _dpr: f32) -> Self {
        let dpr = 1.0f32; // Force 1x rendering — browser upscales via CSS
        let fonts = [
            Font::from_bytes(FONT_REGULAR, FontSettings::default())
                .expect("failed to load regular font"),
            Font::from_bytes(FONT_BOLD, FontSettings::default())
                .expect("failed to load bold font"),
            Font::from_bytes(FONT_ITALIC, FontSettings::default())
                .expect("failed to load italic font"),
        ];

        let px_size = font_size * dpr;
        let (cell_width, cell_height, baseline) = Self::compute_metrics(&fonts[0], px_size);

        GlyphCache {
            fonts,
            cache: HashMap::with_capacity(256),
            cell_width,
            cell_height,
            baseline,
            font_size,
            dpr,
            px_size,
        }
    }

    /// Recompute metrics and clear cache when font size changes.
    /// DPR is capped at 1.0 — browser handles CSS upscaling.
    pub fn set_font_size(&mut self, font_size: f32, _dpr: f32) {
        let dpr = 1.0f32;
        self.font_size = font_size;
        self.dpr = dpr;
        self.px_size = font_size * dpr;
        let (cw, ch, bl) = Self::compute_metrics(&self.fonts[0], self.px_size);
        self.cell_width = cw;
        self.cell_height = ch;
        self.baseline = bl;
        self.cache.clear();
    }

    /// Get logical (pre-DPR) cell dimensions for frontend mouse mapping.
    pub fn logical_cell_width(&self) -> u32 {
        (self.cell_width as f32 / self.dpr).round() as u32
    }

    pub fn logical_cell_height(&self) -> u32 {
        (self.cell_height as f32 / self.dpr).round() as u32
    }

    /// Rasterize a glyph (or retrieve from cache).
    /// Returns None only if the character cannot be rasterized at all.
    pub fn rasterize(&mut self, ch: char, style: u8) -> &RasterizedGlyph {
        let key = GlyphKey {
            ch,
            font_size_tenths: (self.px_size * 10.0) as u16,
            style,
        };

        // Use entry API to avoid double lookup
        self.cache.entry(key).or_insert_with_key(|k| {
            Self::rasterize_glyph(&self.fonts, k.ch, k.style, self.px_size, self.cell_width, self.baseline)
        })
    }

    // ── Private helpers ──────────────────────────────────────────────────

    fn compute_metrics(regular_font: &Font, px_size: f32) -> (u32, u32, u32) {
        // Use horizontal line metrics for ascent/descent
        let line_metrics = regular_font
            .horizontal_line_metrics(px_size)
            .expect("font must have horizontal line metrics");

        let ascent = line_metrics.ascent;
        let descent = line_metrics.descent.abs();

        // Cell height = ascent + descent, with 20% line spacing (matching JS fontSize * 1.2)
        let raw_height = ascent + descent;
        let cell_height = (px_size * 1.2).round() as u32;

        // Baseline = distance from top of cell to baseline
        // Center the glyph vertically, then place baseline at ascent
        let vertical_pad = cell_height as f32 - raw_height;
        let baseline = (ascent + vertical_pad / 2.0).round() as u32;

        // Cell width = advance width of 'M' (standard monospace reference character)
        let m_index = regular_font.lookup_glyph_index('M');
        let m_metrics = regular_font.metrics_indexed(m_index, px_size);
        let cell_width = m_metrics.advance_width.round() as u32;

        // Ensure minimum dimensions
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);

        (cell_width, cell_height, baseline)
    }

    fn rasterize_glyph(
        fonts: &[Font; 3],
        ch: char,
        style: u8,
        px_size: f32,
        cell_width: u32,
        baseline: u32,
    ) -> RasterizedGlyph {
        // Select font based on style
        let font_idx = match style {
            STYLE_BOLD | STYLE_BOLD_ITALIC => 1,
            STYLE_ITALIC => 2,
            _ => 0, // STYLE_REGULAR and fallback
        };
        let font = &fonts[font_idx];

        // Check if glyph exists in selected font, fall back to regular
        let glyph_index = font.lookup_glyph_index(ch);
        let (used_font, _glyph_idx) = if glyph_index == 0 {
            // Glyph not found in styled font, try regular
            let reg_idx = fonts[0].lookup_glyph_index(ch);
            if reg_idx == 0 {
                // Not found anywhere — return empty glyph
                return RasterizedGlyph {
                    bitmap: Vec::new(),
                    width: 0,
                    height: 0,
                    x_offset: 0,
                    y_offset: 0,
                };
            }
            (&fonts[0], reg_idx)
        } else {
            (font, glyph_index)
        };

        let (metrics, bitmap) = used_font.rasterize(ch, px_size);

        if bitmap.is_empty() || metrics.width == 0 || metrics.height == 0 {
            return RasterizedGlyph {
                bitmap: Vec::new(),
                width: 0,
                height: 0,
                x_offset: 0,
                y_offset: 0,
            };
        }

        // Calculate glyph positioning within the cell.
        //
        // fontdue coordinate system: baseline = 0, positive y = up
        //   ymin = y-coordinate of bottom edge of glyph bounding box
        //   Top of glyph in font coords = ymin + height
        //
        // Screen coordinate system: y=0 at top of cell, positive y = down
        //   baseline_screen = distance from cell top to baseline
        //   glyph_top_screen = baseline_screen - (ymin + height)
        let y_offset = baseline as i32 - (metrics.ymin as i32 + metrics.height as i32);

        // Horizontal positioning: use font's xmin (left bearing)
        let x_offset = metrics.xmin as i32;

        RasterizedGlyph {
            bitmap,
            width: metrics.width as u32,
            height: metrics.height as u32,
            x_offset,
            y_offset,
        }
    }
}
