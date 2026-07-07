use fontdue::{Font, FontSettings};

// ── Embedded font data ──────────────────────────────────────────────────

static FONT_REGULAR: &[u8] = include_bytes!("../../fonts/MesloLGMNerdFontMono-Regular.ttf");

// ── Cell-metrics provider ───────────────────────────────────────────────
//
// 0.40.31 warning-zero pass: the server-side glyph ATLAS half of this
// module (STYLE_* indices, GlyphKey, RasterizedGlyph, rasterize /
// rasterize_glyph and the per-glyph bitmap cache — the experimental GPU
// painter's data source) was provably unreachable and has been deleted;
// recover it from git history if the WebGL painter lands. What remains
// is the live half: fontdue-derived CELL METRICS that
// `alacritty_backend` serves to the frontend for mouse mapping.

pub struct GlyphCache {
    /// Regular font — the monospace metrics reference.
    font: Font,

    // ── Public cell metrics ──
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: u32,

    /// Device pixel ratio.
    dpr: f32,
}

impl GlyphCache {
    /// Create a new glyph cache with the given logical font size and DPR.
    /// NOTE: We cap DPR at 1.0 for bitmap rendering to keep frame sizes manageable.
    /// The browser handles upscaling via CSS — slightly less crisp but 4x less data.
    pub fn new(font_size: f32, _dpr: f32) -> Self {
        let dpr = 1.0f32; // Force 1x rendering — browser upscales via CSS
        let font = Font::from_bytes(FONT_REGULAR, FontSettings::default())
            .expect("failed to load regular font");

        let px_size = font_size * dpr;
        let (cell_width, cell_height, baseline) = Self::compute_metrics(&font, px_size);

        GlyphCache {
            font,
            cell_width,
            cell_height,
            baseline,
            dpr,
        }
    }

    /// Recompute metrics when font size changes.
    /// DPR is capped at 1.0 — browser handles CSS upscaling.
    pub fn set_font_size(&mut self, font_size: f32, _dpr: f32) {
        let dpr = 1.0f32;
        self.dpr = dpr;
        let (cw, ch, bl) = Self::compute_metrics(&self.font, font_size * dpr);
        self.cell_width = cw;
        self.cell_height = ch;
        self.baseline = bl;
    }

    /// Get logical (pre-DPR) cell dimensions for frontend mouse mapping.
    pub fn logical_cell_width(&self) -> u32 {
        (self.cell_width as f32 / self.dpr).round() as u32
    }

    pub fn logical_cell_height(&self) -> u32 {
        (self.cell_height as f32 / self.dpr).round() as u32
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
}
