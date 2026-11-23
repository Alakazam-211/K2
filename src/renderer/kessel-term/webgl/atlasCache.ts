// One glyph atlas per font, shared by every WebGL terminal on screen and
// handed from the pane that hides to the pane that shows (switch speed,
// 0.46.18). Before, each painter built its own atlas on every show: a new
// page canvas, the ASCII warm-up rasterized again, every glyph of the new
// screen rasterized again and the whole page uploaded.
//
// The atlas is colorless and keyed by what shapes its pixels only (font
// family, device font size, device cell size), so any pane with the same
// font can draw from it. Each painter keeps its own row cache and its own
// uploaded version; the atlas's `epoch` still drops their slabs when a full
// page is cleared.
//
// Memory: never more than before. Panes with the same font now hold one
// page between them instead of one each. When no pane uses an atlas it is
// kept for ATLAS_IDLE_MS (a switch to Home and back) and then freed; only
// one unused atlas is ever kept, and a font change frees the old one at
// once when nothing else uses it.

import { GlyphAtlas, type GlyphAtlasConfig } from './glyphAtlas'

export const ATLAS_IDLE_MS = 30_000

interface Entry {
  key: string
  atlas: GlyphAtlas
  users: number
  idleTimer: ReturnType<typeof setTimeout> | null
}

const entries = new Map<string, Entry>()

function keyOf(cfg: GlyphAtlasConfig): string {
  return `${cfg.fontFamily}|${cfg.fontDevicePx}|${cfg.deviceCellW}x${cfg.deviceCellH}`
}

function free(e: Entry): void {
  if (e.idleTimer) clearTimeout(e.idleTimer)
  e.idleTimer = null
  entries.delete(e.key)
  e.atlas.dispose()
}

/** The shared atlas for this font (built and warmed up on first use). */
export function acquireAtlas(cfg: GlyphAtlasConfig): GlyphAtlas {
  const key = keyOf(cfg)
  let e = entries.get(key)
  if (!e) {
    // Only one unused atlas is kept: a new font frees the idle ones.
    for (const other of [...entries.values()]) if (other.users === 0) free(other)
    const atlas = new GlyphAtlas(cfg)
    atlas.warmUp()
    e = { key, atlas, users: 0, idleTimer: null }
    entries.set(key, e)
  }
  if (e.idleTimer) {
    clearTimeout(e.idleTimer)
    e.idleTimer = null
  }
  e.users += 1
  return e.atlas
}

/** A painter is done with `atlas` (font change, hide, close). */
export function releaseAtlas(atlas: GlyphAtlas): void {
  for (const e of entries.values()) {
    if (e.atlas !== atlas) continue
    e.users = Math.max(0, e.users - 1)
    if (e.users > 0) return
    // Another font is in use now: this one won't come back soon.
    const otherInUse = [...entries.values()].some((o) => o !== e && o.users > 0)
    if (otherInUse) {
      free(e)
      return
    }
    e.idleTimer = setTimeout(() => free(e), ATLAS_IDLE_MS)
    return
  }
  // Not a shared atlas (a painter that owns its own): free it here.
  atlas.dispose()
}

/** Atlases held now, used or idle (tests, the bench). */
export function liveAtlasCount(): number {
  return entries.size
}

/** Free every atlas now (tests; nothing in the app needs it). */
export function releaseAllAtlases(): void {
  for (const e of [...entries.values()]) free(e)
}
