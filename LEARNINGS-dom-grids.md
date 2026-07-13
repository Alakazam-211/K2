# DOM grid-scroll sluggishness — research + fix record

Three-agent research pass (2026-07-13): xterm.js DOM renderer perf
study, WebKit paint-cost research (primary sources), K2 DOM
scroll-path audit. Companion to LEARNINGS-webgl-scroll.md.

## The smoking gun (WebKit source, GradientRendererCG.cpp)

WebKit's CoreGraphics backend has **NO solid-fill fast path for CSS
gradients** — every gradient, including our 2-stop hard-stop
"rectangle" gradients, is rasterized by the CGGradient per-pixel
shader on every paint. Only the CGGradient *object* is cached, never
the pixels. A K2 border row = ~80 synthetic spans × ~2 gradient
layers ≈ 160 gradient shader draws; `╬` cells hit 8 layers each.

## K2 audit conclusions (verified file:line in agent report)

- Sub-row scrolling is ALREADY free: memo chain holds (row array
  reference identity), frame = pure compositor translate. The stutter
  is quantized to ROW-BOUNDARY crossings (~5/frame at fast scroll).
- Each crossing mounts 1 row = up to ~80 absolutely-positioned
  gradient spans, built from scratch (no style-string cache; only
  glyphMetrics is memoized), inside ONE big promoted strip layer with
  no per-row containment → WebKit re-rasters gradient-dense tiles.
- `─` painted as 2 gradient layers (left+right arm), not 1 — doubling
  fill count for the most common glyph.
- Arcs already cheap (border-radius child div, no gradients).

## xterm.js DOM renderer lessons

- No custom box-drawing glyphs AT ALL in their DOM path (fonts only;
  "lacks some features such as custom glyphs") — our synthetic-in-DOM
  approach is novel territory; nobody pre-solved its perf.
- Their #1 technique: run-merging (1 span per same-styled run).
  Their per-glyph letter-spacing correction fragments runs exactly
  like our per-char cells do.
- Fixed recycled row divs — content rewritten in place, rows never
  mount/unmount on scroll. Classes in a shared <style> over inline
  styles. No contain/will-change/transforms — structural stability.

## WebKit facts that decided the fix

- Dirty-rect tracking is real: a row mount invalidates ~its rect in
  the strip's backing store, but re-rastering that rect full of
  gradients is the cost. `contain: paint` bounds invalidation/
  traversal scope; per-row LAYER promotion would churn 40-80 backing
  stores per scroll — containment yes, promotion no.
- content-visibility: Safari 18+, overlaps our manual windowing;
  known Safari bug: content-visibility:auto + SVG <text> never
  paints (avoid SVG for cells).
- box-shadow is in the same "expensive" paint bucket as gradients —
  not an alternative. Many solid divs beat gradients on pixels but
  pay per-element overhead (~2,000-6,400 spans on screen today).
- Canvas2D per row: fillRects drawn once at mount; afterwards WebKit
  just composites the static bitmap when the strip translates. All
  three reports independently rank this #1.

## Fix implemented (clean commits on top of dffaa84; git-revert to
## roll back — user decision: no runtime killswitch, won't release
## before testing)

1. `syntheticRaster.ts`: drawSyntheticGlyph gains (ink, alpha)
   params (defaults '#ffffff'/1 keep the WebGL atlas callsite
   byte-identical). One rasterizer now serves both painters.
2. `rowRender.tsx`: synthetic cells (box/block/sextant) no longer
   emit gradient spans — collected per row and drawn into ONE
   absolutely-positioned per-row <canvas> via drawSyntheticGlyph
   (solid fillRects + arcTo arcs, device-pixel sized, seam-free
   per-cell edges x0=round(colCss·dpr)). Font-path exotic cells
   (braille, geometric, ╱╲╳, powerline) stay as text spans; run
   bg underlays unchanged. Arc child divs gone (canvas arcs match
   WebGL). CSS paint stage (paintSyntheticGlyph) left intact +
   tested for conflict-free revert.
3. `rowRender.tsx`: rows get `contain: layout paint` (bounds
   invalidation scope; measured rows only).

Expected effect: border-row mount goes from ~80 spans/160 gradient
shader draws to 1 canvas + N solid fills; scroll re-raster becomes a
bitmap blit. DOM renderer's mount cost per row-crossing drops by
roughly an order of magnitude on grid-heavy content.

## Deferred

- Row-div recycling (xterm's fixed row pool) — bigger React
  restructuring; only if crossings still stutter after canvas fix.
- Run-merging for font-path exotic runs (braille art) — different
  workload (text, not geometry), not implicated in grids.
- Profiling recipe if ever needed: Safari Develop → [machine] →
  K2 WKWebView → Timelines; compare Paint lane with/without grids
  (needs WKWebView isInspectable; Tauri debug builds have it).
