// The dark stage: the picture sits on a dark surface in every look, like a
// video monitor, because K2 Render's clay contents tone reads at about
// 1.3:1 on a light background and 8.8–10:1 on a dark one, and one
// transparent still serves all four looks (live-scene-v2-notes.md, v2.4b).
// The stage re-scopes the palette on its own element: the look's style
// with its dark palette, so every token under it (text, edges, menus, the
// focus ring) resolves dark in the light looks too. A dark palette already
// on <html> (Charcoal, Obsidian, Dusk) stays as it is.

import { getStyle } from '@/styles.generated'

export interface StageScope {
  'data-style': string
  'data-palette': string
  'data-scheme': 'dark'
}

/** The attributes that put the stage on the look's dark palette, or null for an unknown style (it keeps the page's). */
export function stageScope(styleId: string | null, palette: string | null, scheme: string | null): StageScope | null {
  const style = styleId ? getStyle(styleId) : undefined
  if (!style) return null
  const own = palette && scheme === 'dark' && style.palettes.some((p) => p.id === palette && p.schemes.includes('dark')) ? palette : null
  const dark = own ?? style.defaultPalettes.dark ?? null
  return dark ? { 'data-style': style.id, 'data-palette': dark, 'data-scheme': 'dark' } : null
}

/** The palette a light look's stage takes (for its accent re-derivation). */
export function stagePaletteMeta(scope: StageScope | null) {
  if (!scope) return null
  return getStyle(scope['data-style'])?.palettes.find((p) => p.id === scope['data-palette']) ?? null
}
