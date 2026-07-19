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
//
// SYNTHETIC (2026-07): box-drawing / block-element / sextant cells
// rasterize procedurally via syntheticRaster.ts (fillRect/stroke of
// the same device-px rect math the DOM painter uses) instead of
// fillText, so stacked blocks tile seamlessly and TUI borders stay
// continuous — DOM-renderer parity. Diagonals ╱╲╳ and powerline PUA
// stay font glyphs (same exclusions as the DOM). Synthetic entries
// ignore bold/italic (weight is intrinsic to the code point).

import {
  allocSlot,
  ATLAS_MAX_SIZE,
  createLayout,
  growLayout,
  type AtlasLayout,
} from './atlasLayout'
import { drawSyntheticGlyph, syntheticSpecForCluster } from './syntheticRaster'
import { EMOJI_FONT_SCALE, isEmojiCp } from '../runCols'

/** 1px transparent border around every slot so LINEAR sampling (or a
 *  half-texel rounding slip) can never bleed a neighbor glyph. */
const SLOT_PAD = 1

/** Ask WebKit for the same thin grayscale AA the DOM strip gets
 *  (globals.css sets -webkit-font-smoothing: antialiased): macOS
 *  otherwise rasterizes canvas text with dilated "smoothed" stems,
 *  which read as chunkier glyphs than the DOM at the same font.
 *
 *  The inline style alone is NOT enough: canvas-text smoothing comes
 *  from the element's COMPUTED style, and a detached canvas resolves
 *  none — so the page canvas is also parked in the document, fixed
 *  far offscreen (never painted, no layout impact), where the style
 *  actually applies. Engines that still ignore it fall back to the
 *  shader's coverage-gamma correction (glBackend u_gamma). Pages are
 *  detached again on atlas dispose/growth. */
function attachAtlasPage(canvas: HTMLCanvasElement): void {
  ;(canvas.style as CSSStyleDeclaration & { webkitFontSmoothing?: string })
    .webkitFontSmoothing = 'antialiased'
  canvas.style.position = 'fixed'
  canvas.style.left = '-100000px'
  canvas.style.top = '0'
  canvas.setAttribute('aria-hidden', 'true')
  canvas.setAttribute('data-k2-atlas-page', '')
  document.body?.appendChild(canvas)
}

export interface GlyphSlot {
  texX: number
  texY: number
  /** Quad/texture size in device px. Normally widthCells × cell;
   *  emoji slots may be WIDER (clip exemption — sized to the emoji
   *  font's measured advance so the frame can't truncate ink). */
  w: number
  h: number
  /** True ⇒ emoji/multi-color: shader outputs the sample untinted. */
  color: boolean
  /** Horizontal quad offset in device px (a_geom.x): emoji slots
   *  wider than their cell box hang the overflow SYMMETRICALLY over
   *  the neighbors (offsetX = -(overhang/2)); 0/absent otherwise. */
  offsetX?: number
}

/** The face packFrame consumes — narrow so tests can stub it with a
 *  deterministic fake (no Canvas2D in the node test env). */
export interface GlyphSource {
  /** Bumps when the page is cleared (overflow escape hatch) — cached
   *  row slabs referencing old coordinates must be dropped. Growth
   *  does NOT bump it (coordinates survive re-blitting). */
  readonly epoch: number
  /** True when the page can no longer GROW — the next overflow
   *  CLEARS it (epoch bump, every cached slab invalidated).
   *  Speculative work (prewarm) checks this and stops rasterizing
   *  new glyphs rather than risk wiping the live window's slabs
   *  (audit finding: prewarm-induced clear = self-inflicted
   *  full-window rebuild on the next paint). */
  readonly nearCapacity?: boolean
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
    attachAtlasPage(this.canvas)
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

  /** Detach the page canvas parked in the document (see
   *  attachAtlasPage). Call when the atlas is replaced (font/DPR
   *  change) or the painter is disposed. */
  dispose(): void {
    this.canvas.remove()
  }

  get size(): number {
    return this.layout.size
  }

  get nearCapacity(): boolean {
    return this.layout.size >= ATLAS_MAX_SIZE
  }

  get glyphCount(): number {
    return this.glyphs.size
  }

  private fontString(bold: boolean, italic: boolean, scale = 1): string {
    const px = Math.round(this.cfg.fontDevicePx * scale)
    return `${italic ? 'italic ' : ''}${bold ? 'bold ' : ''}${px}px ${this.cfg.fontFamily}`
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
    // Synthetic glyphs share one slot across bold/italic — the DOM
    // painter likewise drops text styles on painted cells.
    const spec = syntheticSpecForCluster(text)
    const key = spec
      ? `s${widthCells}|${text}`
      : `${bold ? 'b' : ''}${italic ? 'i' : ''}${widthCells}|${text}`
    const hit = this.glyphs.get(key)
    if (hit) return hit

    const cellBoxW = widthCells * this.cfg.deviceCellW
    let w = cellBoxW
    let offsetX = 0
    let advance = 0
    let emoji = false
    if (!spec) {
      // Measure BEFORE slot allocation: emoji slots widen to the
      // emoji font's real advance so the frame can't truncate ink
      // (clip exemption — DOM counterpart lifts overflow:hidden).
      // The overflow hangs symmetrically via offsetX → a_geom.x.
      // Emoji measure AND draw at EMOJI_FONT_SCALE (they render
      // visibly smaller than text at equal point size).
      emoji = isEmojiCp(text.codePointAt(0) ?? 0)
      this.ctx.font = this.fontString(
        bold,
        italic,
        emoji ? EMOJI_FONT_SCALE : 1,
      )
      advance = this.ctx.measureText(text).width
      if (emoji && advance > w) {
        w = Math.ceil(advance)
        offsetX = -Math.round((w - cellBoxW) / 2)
      }
    }
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
        this.canvas.remove()
        attachAtlasPage(grown)
        this.canvas = grown
        this.ctx = gctx
        this.version++
      } else {
        // Page cap hit: clear-and-restart (xterm's clearTexture
        // escape hatch). Consumers drop stale slabs via `epoch`.
        // eslint-disable-next-line no-console
        console.warn('[kessel-term/webgl] glyph atlas full at cap — clearing page')
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

    if (spec) {
      // White by construction — skip the getImageData mono-scan.
      drawSyntheticGlyph(ctx, spec, x, y, w, h)
      ctx.restore()
      const slot: GlyphSlot = { texX: x, texY: y, w, h, color: false }
      this.glyphs.set(key, slot)
      this.version++
      return slot
    }

    ctx.fillStyle = '#ffffff'
    ctx.font = this.fontString(bold, italic, emoji ? EMOJI_FONT_SCALE : 1)
    ctx.textBaseline = 'alphabetic'
    // Center horizontally in the slot: exact-monospace advances
    // round dx to 0; over-wide fallback glyphs (braille art from a
    // substituted font) clip symmetrically instead of right-only —
    // parity with the DOM strip's textAlign:center per-char cells.
    // (Emoji slots were widened above, so their dx centering never
    // clips — the slot fits the full advance.)
    const dx = Math.round((w - advance) / 2)
    ctx.fillText(text, x + dx, y + this.baseline)
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

    const slot: GlyphSlot = { texX: x, texY: y, w, h, color, offsetX }
    this.glyphs.set(key, slot)
    this.version++
    return slot
  }
}
