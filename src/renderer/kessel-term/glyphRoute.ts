// Which path draws a terminal cluster — the ONE decision both
// painters share (rowRender.tsx DOM cells, webgl/glyphAtlas.ts), so
// they can't drift apart:
//
//   synthetic → painted geometry (box drawing, blocks, sextants, and
//               the TUI status marks ⏺⏹⏸⏴⏵⏶⏷), never a font glyph
//   emoji     → colour emoji: EMOJI_FONT_SCALE, clip exemption, two
//               columns for the wide ones (✅ 🎉)
//   text      → font text. `draw` is what to hand the text engine:
//               the cluster itself, or — for a bare one-column symbol
//               that also has an emoji form (✔ ☀ ⏯ ↔ …) — the
//               cluster + U+FE0E (VS15) to request TEXT presentation,
//               so font fallback skips Apple Color Emoji / Noto Color
//               Emoji. (Belt-and-braces: an engine that ignores VS15
//               still draws the glyph clipped to its cell, same as
//               before; the common TUI marks don't depend on it —
//               they're synthetic.)
//
// The bug this exists for: Claude Code's ⏺ status bullet (U+23FA)
// is missing from MesloLGM, SF Mono and Menlo, and is Emoji=Yes; on
// iOS the fallback lands on Apple Color Emoji, a grey rounded-square
// button that the one-column cell clipped on both sides.

import { isEmojiCp, isTextDefaultEmojiCp } from './runCols'
import type { SyntheticGlyph } from './syntheticGlyphs'
import { syntheticSpecForCluster } from './webgl/syntheticRaster'

/** VARIATION SELECTOR-15: request text presentation. */
export const VS15 = '\uFE0E'

export type GlyphRoute =
  | { kind: 'synthetic'; spec: SyntheticGlyph }
  | { kind: 'emoji' }
  | { kind: 'text'; draw: string }

export function glyphRoute(text: string): GlyphRoute {
  const spec = syntheticSpecForCluster(text)
  if (spec) return { kind: 'synthetic', spec }
  const cp = text.codePointAt(0) ?? 0
  if (isEmojiCp(cp)) return { kind: 'emoji' }
  // Bare cluster only: an explicit follower (VS16 = "I want the
  // emoji", or a combining mark) is the app's call, not ours.
  if (isTextDefaultEmojiCp(cp) && String.fromCodePoint(cp) === text) {
    return { kind: 'text', draw: text + VS15 }
  }
  return { kind: 'text', draw: text }
}
