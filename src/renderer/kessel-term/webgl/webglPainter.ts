// WebGL2 terminal painter — orchestrates the pure pieces (packFrame,
// RowCache, contextLoss) against the narrow GL backend. Browser-bound
// glue only: everything with logic worth testing lives in the pure
// modules.
//
// Fatal policy: ANY unrecoverable condition (no WebGL2, shader
// compile failure, failed sanity readback, unrestored context loss)
// fires `onFatal` exactly once; TerminalPane responds by rendering
// the DOM strip for the rest of the pane's life. The DOM renderer is
// the permanent fallback — the painter never retries across frames.

import type {
  PainterFrame,
  PainterMetrics,
  TerminalPainter,
} from './painterTypes'
import {
  createWebgl2Backend,
  type PainterBackend,
} from './glBackend'
import { FrameBuffers, packFrame, prewarmRows, RowCache } from './packFrame'
import { GlyphAtlas } from './glyphAtlas'
import { createContextLossTracker } from './contextLoss'

/** Known clear color for the mount-time pixel-readback sanity probe
 *  (brief §7.7: WKWebView's WebGL history demands a rendered-pixels
 *  check before trusting the context). Arbitrary, unlikely theme
 *  color. */
const SANITY_COLOR = 0x3a7b19

/** Post-draw row-cache prewarm (scroll-hop fix, LEARNINGS-webgl-
 *  scroll.md): rows outside the window pre-expanded each frame so a
 *  fast scroll hits warm slabs. Band = how far to reach in each
 *  direction; budget caps the leftover-frame time it may spend. */
const PREWARM_BAND = 24
const PREWARM_BUDGET_MS = 2

/** Text-weight tuning ("chonky text" fix): coverage-gamma exponent
 *  for tinted glyphs. macOS dilates Canvas2D text stems relative to
 *  the DOM's -webkit-font-smoothing:antialiased rendering; >1 thins
 *  AA edges back toward DOM weight. Feel-tunable live without a
 *  rebuild: localStorage.K2SO_WEBGL_TEXT_GAMMA = '1.4' (re-open the
 *  tab to apply); clamped to a sane band. */
// 1.3 over-thinned once the atlas font-smoothing fix (attachAtlasPage)
// removed the dilation it was compensating for — user side-by-side
// 2026-07-13 read "a teenie tiny margin too thin"; 1.2 adds ~5% edge
// ink back.
const TEXT_GAMMA_DEFAULT = 1.2
function textGammaSetting(): number {
  try {
    const raw = localStorage.getItem('K2SO_WEBGL_TEXT_GAMMA')
    const v = raw === null ? NaN : parseFloat(raw)
    if (Number.isFinite(v)) return Math.min(3, Math.max(0.5, v))
  } catch {
    // localStorage unavailable (tests) — fall through to default.
  }
  return TEXT_GAMMA_DEFAULT
}

export interface WebglPainterDeps {
  /** Test seam: swap the real WebGL2 backend for a stub. */
  createBackend?: (canvas: HTMLCanvasElement) => PainterBackend | null
}

export function createWebglPainter(
  deps: WebglPainterDeps = {},
): TerminalPainter {
  const createBackend = deps.createBackend ?? createWebgl2Backend

  let canvas: HTMLCanvasElement | null = null
  let backend: PainterBackend | null = null
  let metrics: PainterMetrics | null = null
  let deviceCellW = 0
  let deviceCellH = 0
  let canvasW = 0
  let canvasH = 0
  let disposed = false
  let fatalFired = false
  let fatalCb: ((reason: string) => void) | null = null
  let atlas: GlyphAtlas | null = null
  let uploadedAtlasVersion = -1
  let decoThickness = 1
  /** Last frame drawn — replayed after a successful context restore
   *  (nothing upstream re-renders until the next snapshot/scroll). */
  let lastFrame: PainterFrame | null = null

  const cache = new RowCache()
  const buffers = new FrameBuffers()
  const textGamma = textGammaSetting()

  // Diagnostics (owner feel-test aid): localStorage.K2SO_WEBGL_DIAG='1'
  // → one console line per 60 rendered frames with timing + volume.
  const diagEnabled =
    typeof localStorage !== 'undefined' &&
    localStorage.getItem('K2SO_WEBGL_DIAG') === '1'
  let diagFrames = 0
  let diagMs = 0

  const fatal = (reason: string): void => {
    if (fatalFired) return
    fatalFired = true
    // eslint-disable-next-line no-console
    console.warn(`[kessel-term/webgl] fatal: ${reason} — DOM fallback takes over`)
    fatalCb?.(reason)
  }

  const doRender = (frame: PainterFrame): void => {
    if (disposed || fatalFired || !backend || !canvas || !metrics) return
    if (!atlas || lossTracker.state !== 'live') return
    const t0 = diagEnabled ? performance.now() : 0

    const { cols, rows } = frame.snapshot
    if (cols <= 0 || rows <= 0) return
    const w = cols * deviceCellW
    const h = rows * deviceCellH
    if (w !== canvasW || h !== canvasH) {
      canvasW = w
      canvasH = h
      canvas.width = w
      canvas.height = h
      canvas.style.width = `${w / metrics.dpr}px`
      canvas.style.height = `${h / metrics.dpr}px`
      backend.resize(w, h)
    }

    const packInput = {
      frame,
      cssCellH: metrics.cssCellH,
      deviceCellW,
      deviceCellH,
      dpr: metrics.dpr,
      cache,
      buffers,
      glyphs: atlas,
      decoThickness,
    }
    const packed = packFrame(packInput)

    // Packing may have rasterized new glyphs — re-upload the page
    // once per frame at most, keyed off the atlas version.
    if (atlas.version !== uploadedAtlasVersion) {
      backend.uploadAtlas(atlas.canvas)
      uploadedAtlasVersion = atlas.version
    }

    backend.beginFrame(frame.theme.bg)
    backend.drawRects(packed.bg.data, packed.bg.count)
    // Selection under glyphs: text stays full-contrast (brief §2.2).
    backend.drawRects(packed.selection.data, packed.selection.count)
    backend.drawGlyphs(packed.glyphData, packed.glyphCount, {
      cols: frame.snapshot.cols,
      cellW: deviceCellW,
      cellH: deviceCellH,
      scrollY: packed.fractionDevice,
      texW: atlas.size,
      texH: atlas.size,
      textGamma,
    })
    // Decorations LAST: underline/strikethrough bars draw over their
    // glyphs (pass order per brief §2.2; the block cursor stays a DOM
    // overlay above the whole canvas — §5).
    backend.drawRects(packed.deco.data, packed.deco.count)

    // Leftover-frame prewarm: expand rows just outside the window so
    // the NEXT scroll frame's reveals are cache hits. New glyphs it
    // rasterizes ride the next frame's atlas-version upload.
    prewarmRows(packInput, packed, PREWARM_BAND, PREWARM_BUDGET_MS)

    if (diagEnabled) {
      diagMs += performance.now() - t0
      if (++diagFrames >= 60) {
        // eslint-disable-next-line no-console
        console.info(
          `[webgl-diag] frames=${diagFrames} avg=${(diagMs / diagFrames).toFixed(2)}ms ` +
            `glyphInstances=${packed.glyphCount} bgRects=${packed.bg.count} ` +
            `rows=${packed.rowCount} cacheRows=${cache.size} ` +
            `atlas=${atlas.size}px/${atlas.glyphCount} glyphs ` +
            `textGamma=${textGamma}`,
        )
        diagFrames = 0
        diagMs = 0
      }
    }
  }

  // Context loss protocol (brief §6.4): lost → preventDefault + a
  // restore window; restored in time → rebuild ALL GL state (the
  // restored context is blank: programs/buffers/textures are gone)
  // and replay the last frame; window expiry → fatal → DOM fallback.
  const lossTracker = createContextLossTracker({
    onRestore: () => {
      if (disposed || !canvas) return
      backend?.dispose()
      backend = createBackend(canvas)
      if (!backend) {
        fatal('webgl2-restore-reinit-failed')
        return
      }
      // Atlas PIXELS survive (the page is a plain canvas) but the GL
      // texture didn't — force re-upload; same for the drawing-buffer
      // size and both programs' resolution uniforms.
      uploadedAtlasVersion = -1
      canvasW = 0
      canvasH = 0
      if (lastFrame) doRender(lastFrame)
    },
    onFatal: fatal,
  })
  const onContextLost = (e: Event): void => {
    e.preventDefault()
    lossTracker.handleLost()
  }
  const onContextRestored = (): void => {
    lossTracker.handleRestored()
  }

  return {
    mount(el: HTMLCanvasElement): void {
      if (disposed) return
      canvas = el
      canvas.addEventListener('webglcontextlost', onContextLost)
      canvas.addEventListener('webglcontextrestored', onContextRestored)
      backend = createBackend(canvas)
      if (!backend) {
        fatal('webgl2-unavailable')
        return
      }
      // Sanity probe: clear a tiny buffer to a known color and read it
      // back. Catches contexts that "work" but never present real
      // pixels (the historical WKWebView failure mode).
      canvas.width = 4
      canvas.height = 4
      backend.resize(4, 4)
      backend.beginFrame(SANITY_COLOR)
      const px = backend.readPixel(1, 1)
      const ok =
        px !== null &&
        Math.abs(px[0] - ((SANITY_COLOR >> 16) & 0xff)) <= 2 &&
        Math.abs(px[1] - ((SANITY_COLOR >> 8) & 0xff)) <= 2 &&
        Math.abs(px[2] - (SANITY_COLOR & 0xff)) <= 2
      if (!ok) {
        fatal('webgl2-sanity-readback-failed')
        return
      }
      // Collapse the probe buffer so no colored speck shows before
      // the first real frame sizes the canvas.
      canvas.width = 0
      canvas.height = 0
    },

    setMetrics(m: PainterMetrics): void {
      metrics = m
      // Integer device grid (brief §1.3): floor the width, round the
      // height, so glyphs land on whole device pixels.
      deviceCellW = Math.max(1, Math.floor(m.cssCellW * m.dpr))
      deviceCellH = Math.max(1, Math.round(m.cssCellH * m.dpr))
      // xterm's bar-thickness rule (TextureAtlas underline math).
      decoThickness = Math.max(1, Math.floor((m.fontSize * m.dpr) / 15))
      // Font/DPR change ⇒ new atlas; every cached slab holds stale
      // atlas coordinates → drop the row cache wholesale. Theme
      // changes deliberately do NOT land here (color is
      // per-instance; the atlas is colorless — brief §1.3's win over
      // xterm's rebuild-on-theme).
      atlas?.dispose()
      atlas = new GlyphAtlas({
        deviceCellW,
        deviceCellH,
        fontFamily: m.fontFamily,
        fontDevicePx: Math.round(m.fontSize * m.dpr),
      })
      atlas.warmUp()
      uploadedAtlasVersion = -1
      cache.clear()
      // Force a canvas re-size on the next render.
      canvasW = 0
      canvasH = 0
    },

    render(frame: PainterFrame): void {
      lastFrame = frame
      doRender(frame)
    },

    onFatal(cb: (reason: string) => void): void {
      fatalCb = cb
      // A fatal that happened before the pane subscribed (mount-time
      // sanity failure) still has to reach it.
      if (fatalFired) cb('webgl2-init-failed')
    },

    dispose(): void {
      disposed = true
      lossTracker.dispose()
      if (canvas) {
        canvas.removeEventListener('webglcontextlost', onContextLost)
        canvas.removeEventListener('webglcontextrestored', onContextRestored)
      }
      backend?.dispose()
      backend = null
      canvas = null
      lastFrame = null
      atlas?.dispose()
      atlas = null
      cache.clear()
    },
  }
}
