// Pure shelf-packing math for the glyph atlas (node-safe).
//
// Simplification over xterm's TextureAtlas: K2 slots are uniform
// height (one font size per atlas — cellH + padding), so plain
// left→right shelves suffice; no fixed-row trick, no per-glyph
// bounding-box trim. Growth is by doubling the single page — the
// caller re-blits the old canvas at (0,0), so EXISTING SLOT
// COORDINATES STAY VALID across growth (that invariant is what lets
// cached row slabs survive; the shader normalizes by a texture-size
// uniform). At the size cap the caller clears and re-warms instead
// of adding pages (brief §1.3's escape hatch — a 4096² page of
// cell-sized monochrome glyphs is ~thousands of entries, so this is
// a theoretical path).

export const ATLAS_INITIAL_SIZE = 512
export const ATLAS_MAX_SIZE = 4096

export interface AtlasLayout {
  size: number
  shelfX: number
  shelfY: number
  shelfH: number
}

export function createLayout(size: number = ATLAS_INITIAL_SIZE): AtlasLayout {
  return { size, shelfX: 0, shelfY: 0, shelfH: 0 }
}

/** Allocate a w×h slot. Returns its top-left, or null when the page
 *  is full (caller grows or clears). Mutates the layout. */
export function allocSlot(
  l: AtlasLayout,
  w: number,
  h: number,
): { x: number; y: number } | null {
  if (w > l.size || h > l.size) return null
  if (l.shelfX + w > l.size) {
    // Wrap to a new shelf.
    if (l.shelfY + l.shelfH + h > l.size) return null
    l.shelfY += l.shelfH
    l.shelfX = 0
    l.shelfH = 0
  }
  if (l.shelfY + h > l.size) return null
  const x = l.shelfX
  const y = l.shelfY
  l.shelfX += w
  if (h > l.shelfH) l.shelfH = h
  return { x, y }
}

/** Double the page (coordinates preserved). Returns false at the
 *  cap — the caller's cue to clear-and-rewarm. */
export function growLayout(l: AtlasLayout): boolean {
  if (l.size >= ATLAS_MAX_SIZE) return false
  l.size *= 2
  return true
}
