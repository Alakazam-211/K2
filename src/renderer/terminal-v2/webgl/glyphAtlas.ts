// White-glyph atlas: Zed's economics on xterm's plumbing (brief §1.3).
//
// Glyphs are rasterized WHITE on transparent into a single Canvas2D
// page and tinted per-instance in the shader — the alpha channel IS
// the coverage mask, so one atlas entry serves every color and theme
// (theme changes never rebuild the atlas; only font/DPR changes do).
// Cache key: (cluster text, bold, italic) — never color.
//
// Multi-color glyphs (emoji) rasterize normally; a one-time pixel
// scan marks the entry `color`, and the shader samples them directly
// instead of tinting (per-instance flag — one page, two shader
// paths).
//
// Browser-bound (Canvas2D + getImageData). All the packing MATH
// lives in atlasLayout.ts, which is pure and unit-tested.

import {
  allocSlot,
  ATLAS_MAX_SIZE,
  createLayout,
  growLayout,
  type AtlasLayout,
} from './atlasLayout'

/** 1px transparent border around every slot so LINEAR sampling (or a
 *  half-texel rounding slip) can never bleed a neighbor glyph. */
const SLOT_PAD = 1

export interface GlyphSlot {
  texX: number
  texY: number
  /** Quad/texture size in device px (uniform per atlas: widthCells ×
   *  cell). */
  w: number
  h: number
  /** True ⇒ emoji/multi-color: shader outputs the sample untinted. */
  color: boolean
}

/** The face packFrame consumes — narrow so tests can stub it with a
 *  deterministic fake (no Canvas2D in the node test env). */
export interface GlyphSource {
  /** Bumps when the page is cleared (overflow escape hatch) — cached
   *  row slabs referencing old coordinates must be dropped. Growth
   *  does NOT bump it (coordinates survive re-blitting). */
  readonly epoch: number
  get(
    text: string,
    bold: boolean,
    italic: boolean,
    widthCells: number,
  ): GlyphSlot | null
}

export interface GlyphAtlasConfig {
  deviceCellW: number
  deviceCellH: number
  fontFamily: string
  /** Font size in DEVICE px (css fontSize × dpr). */
  fontDevicePx: number
}

export class GlyphAtlas implements GlyphSource {
  canvas: HTMLCanvasElement
  /** Bumps on every mutation of the page pixels (new glyph, growth,
   *  clear) — the painter's re-upload trigger. */
  version = 0
  epoch = 0

  private ctx: CanvasRenderingContext2D
  private layout: AtlasLayout
  private glyphs = new Map<string, GlyphSlot>()
  private baseline: number

  constructor(private cfg: GlyphAtlasConfig) {
    this.layout = createLayout()
    this.canvas = document.createElement('canvas')
    this.canvas.width = this.layout.size
    this.canvas.height = this.layout.size
    const ctx = this.canvas.getContext('2d', { willReadFrequently: true })
    if (!ctx) throw new Error('canvas 2d context unavailable')
    this.ctx = ctx
    // CSS line-box vertical centering: baseline sits so the font
    // bounding box is centered in the cell — closest match to how
    // the DOM strip (line-height = cellH) places glyphs.
    this.ctx.font = this.fontString(false, false)
    const m = this.ctx.measureText('Mg')
    const ascent = m.fontBoundingBoxAscent ?? m.actualBoundingBoxAscent
    const descent = m.fontBoundingBoxDescent ?? m.actualBoundingBoxDescent
    this.baseline =
      Number.isFinite(ascent) && Number.isFinite(descent)
        ? Math.round(
            (cfg.deviceCellH - (ascent + descent)) / 2 + ascent,
          )
        : Math.round(cfg.deviceCellH * 0.8)
  }

  get size(): number {
    return this.layout.size
  }

  get glyphCount(): number {
    return this.glyphs.size
  }

  private fontString(bold: boolean, italic: boolean): string {
    return `${italic ? 'italic ' : ''}${bold ? 'bold ' : ''}${this.cfg.fontDevicePx}px ${this.cfg.fontFamily}`
  }

  /** Pre-rasterize printable ASCII so the first frame never draws
   *  blanks (brief §7.9). Synchronous — ~94 fillTexts is cheap. */
  warmUp(): void {
    for (let cp = 33; cp <= 126; cp++) {
      this.get(String.fromCharCode(cp), false, false, 1)
    }
  }

  get(
    text: string,
    bold: boolean,
    italic: boolean,
    widthCells: number,
  ): GlyphSlot | null {
    const key = `${bold ? 'b' : ''}${italic ? 'i' : ''}${widthCells}|${text}`
    const hit = this.glyphs.get(key)
    if (hit) return hit

    const w = widthCells * this.cfg.deviceCellW
    const h = this.cfg.deviceCellH
    const slotW = w + SLOT_PAD * 2
    const slotH = h + SLOT_PAD * 2
    // A slot that can never fit ANY page must bail (guards the
    // grow/clear retry loop against pathological cell sizes).
    if (slotW > ATLAS_MAX_SIZE || slotH > ATLAS_MAX_SIZE) return null

    let pos = allocSlot(this.layout, slotW, slotH)
    while (!pos) {
      if (growLayout(this.layout)) {
        // Double the page, preserving existing pixels at (0,0) so
        // every already-issued coordinate stays valid.
        const grown = document.createElement('canvas')
        grown.width = this.layout.size
        grown.height = this.layout.size
        const gctx = grown.getContext('2d', { willReadFrequently: true })
        if (!gctx) return null
        gctx.drawImage(this.canvas, 0, 0)
        this.canvas = grown
        this.ctx = gctx
        this.version++
      } else {
        // Page cap hit: clear-and-restart (xterm's clearTexture
        // escape hatch). Consumers drop stale slabs via `epoch`.
        // eslint-disable-next-line no-console
        console.warn('[terminal-v2/webgl] glyph atlas full at cap — clearing page')
        this.glyphs.clear()
        this.layout = createLayout(this.layout.size)
        this.ctx.clearRect(0, 0, this.layout.size, this.layout.size)
        this.epoch++
        this.version++
      }
      pos = allocSlot(this.layout, slotW, slotH)
    }

    const x = pos.x + SLOT_PAD
    const y = pos.y + SLOT_PAD
    const ctx = this.ctx
    ctx.save()
    ctx.beginPath()
    ctx.rect(x, y, w, h)
    ctx.clip()
    ctx.fillStyle = '#ffffff'
    ctx.font = this.fontString(bold, italic)
    ctx.textBaseline = 'alphabetic'
    ctx.fillText(text, x, y + this.baseline)
    ctx.restore()

    // Monochrome test: white fill ⇒ every covered pixel has r=g=b.
    // Emoji (color fonts ignore fillStyle) fail it → direct-sample
    // path. One getImageData per UNIQUE glyph, never per frame.
    let color = false
    const img = ctx.getImageData(x, y, w, h).data
    for (let i = 0; i < img.length; i += 4) {
      if (img[i + 3] === 0) continue
      if (img[i] !== img[i + 1] || img[i + 1] !== img[i + 2]) {
        color = true
        break
      }
    }

    const slot: GlyphSlot = { texX: x, texY: y, w, h, color }
    this.glyphs.set(key, slot)
    this.version++
    return slot
  }
}
