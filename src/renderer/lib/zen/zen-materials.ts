// 0.46.0 (Rosson 2026-10-16) — Garden 1's Glass look uses K2 Companion's real
// materials. Mobile has three, and a fourth thing that is not an object:
//
//   GLASS  (`.zen-glass` on the phone): a see-through white fill over a
//          28px blur with 180% saturation, a 0.5px bright edge and a top
//          highlight. Only on small floating controls: top-bar buttons
//          (pills), the rail's Home | Agents toggle, chips on rows, search
//          fields, close buttons. Radius: pill 999, round 16, field 22.
//   TILE   (`.k2-tile`): desktop Zen's tile (`zen-glass.ts`, the
//          `data-zen-glass` marker): a SOLID fill + edge + sheen + soft
//          shadow, no blur. Cards, sheets, the message box and Send, the
//          activity drawer (its tab has no bottom rim).
//   SOLID  neither: iMessage bubbles (mine #0a84ff, the agent's #3a3a3c,
//          the theme's bubble tokens), the choice buttons inside a bubble
//          (outlined pills, white when picked), attachment chips, and
//          "Earlier messages" as plain muted text.
//   BLUR   the top bar and the area behind the message box are a
//          progressive blur (four masked backdrop layers, 2/6/12/24px, and
//          a dark tint): heavy at the top edge running down
//          (`.zen-header-blur`), heavy at the bottom edge running up
//          (`.zen-footer-blur`). Drawn by `ZenProgressiveBlur`.
//
// Elements say which material they are with `data-zen-material` (and glass
// its shape with `data-zen-glass-shape`). The marker does nothing on its
// own: only a theme that draws Companion's materials turns it on (the Zen
// root's `data-zen-materials="mobile"`, from `zenThemeMaterials`). Today
// that is the built-in Glass theme (and an override of it, same name), so
// basic, midnight, paper and haunted look exactly as before.
//
// Reduced transparency: no blur anywhere, opaque fills (the media query,
// or the root's `data-zen-reduced-transparency`).

import { createContext, useContext } from 'react'

/** The three materials. */
export type ZenMaterial = 'glass' | 'tile' | 'solid'
/** A glass control's corner: pill 999, round 16, field 22 (Companion's). */
export type ZenGlassShape = 'pill' | 'round' | 'field'

export const ZEN_MATERIAL_ATTR = 'data-zen-material'
export const ZEN_GLASS_SHAPE_ATTR = 'data-zen-glass-shape'
/** On the Zen root: which material set the theme draws. */
export const ZEN_MATERIALS_ATTR = 'data-zen-materials'
/** On the Zen root while reduced transparency is on. */
export const ZEN_REDUCED_TRANSPARENCY_ATTR = 'data-zen-reduced-transparency'

export const ZEN_GLASS_RADII: Record<ZenGlassShape, number> = { pill: 999, round: 16, field: 22 }

/** Companion's GLASS, verbatim. */
export const ZEN_MOBILE_GLASS = {
  fill: 'rgba(255, 255, 255, 0.10)',
  blur: 'blur(28px) saturate(180%)',
  edge: '0.5px solid rgba(255, 255, 255, 0.22)',
  highlight: 'inset 0 1px 0 rgba(255, 255, 255, 0.28)',
} as const

/** Light schemes: the same glass, readable on a light canvas. */
export const ZEN_MOBILE_GLASS_LIGHT = {
  fill: 'rgba(255, 255, 255, 0.62)',
  edge: '0.5px solid rgba(0, 0, 0, 0.12)',
  highlight: 'inset 0 1px 0 rgba(255, 255, 255, 0.85)',
} as const

/** Themes that draw Companion's materials. */
export const ZEN_MOBILE_MATERIAL_THEMES: ReadonlySet<string> = new Set(['glass'])

/** The material set a theme draws: "mobile" for Glass, else none. */
export function zenThemeMaterials(themeName: string | null | undefined): 'mobile' | null {
  return themeName && ZEN_MOBILE_MATERIAL_THEMES.has(themeName) ? 'mobile' : null
}

/** Spread onto an element: `<button {...zenMaterialProps('glass', 'round')}>`. */
export function zenMaterialProps(material: ZenMaterial, shape?: ZenGlassShape): Record<string, string> {
  if (shape && material !== 'glass') throw new Error(`zen materials: only glass has a shape (got ${material})`)
  return shape ? { [ZEN_MATERIAL_ATTR]: material, [ZEN_GLASS_SHAPE_ATTR]: shape } : { [ZEN_MATERIAL_ATTR]: material }
}

export const ZEN_GLASS_PILL_PROPS = zenMaterialProps('glass', 'pill')
export const ZEN_GLASS_ROUND_PROPS = zenMaterialProps('glass', 'round')
export const ZEN_GLASS_FIELD_PROPS = zenMaterialProps('glass', 'field')
export const ZEN_TILE_PROPS = zenMaterialProps('tile')
/** A tile on a tile (the message box, Send, the drawer's tab) or over the
 *  page (menus, dialogs): the raised surface, still solid. */
export const ZEN_TILE_RAISED_ATTR = 'data-zen-tile'
export const ZEN_TILE_RAISED_PROPS = { ...ZEN_TILE_PROPS, [ZEN_TILE_RAISED_ATTR]: 'raised' }
export const ZEN_SOLID_PROPS = zenMaterialProps('solid')

/** What the widgets read: the theme's material set, and reduced transparency. */
export interface ZenMaterialsState {
  materials: 'mobile' | null
  reducedTransparency: boolean
}

export const ZenMaterialsContext = createContext<ZenMaterialsState>({ materials: null, reducedTransparency: false })

export function useZenMaterials(): ZenMaterialsState {
  return useContext(ZenMaterialsContext)
}

// ── The stylesheet ──────────────────────────────────────────────────────

const ROOT = `[data-zen-root][${ZEN_MATERIALS_ATTR}="mobile"]`
const LIGHT = `${ROOT}[data-zen-scheme="light"]`
const RT = `${ROOT}[${ZEN_REDUCED_TRANSPARENCY_ATTR}]`

/** Views that can't put the marker on someone else's elements. */
const FOREIGN_GLASS = [
  '[data-zen-usage] [data-testid="subscription-usage"]',
  '[data-zen-focus-group-picker] [data-focus-group-trigger]',
]
const FOREIGN_FIELDS = ['[data-zen-tickets] input[aria-label="Search tickets"]']
const FOREIGN_TILES = ['ticket-list', 'ticket-list-rail', 'ticket-detail', 'ticket-agent-rail'].map(
  (id) => `[data-zen-tickets] [data-testid="${id}"]`,
)
/** Pop-ups over the page: the raised tile. */
const FOREIGN_RAISED_TILES = [
  '[data-zen-usage]:not([data-zen-usage-inline]) [data-testid="subscription-usage-menu"]',
  '[data-zen-focus-group-menu]',
]

const GLASS_SEL = [`[${ZEN_MATERIAL_ATTR}="glass"]`, ...FOREIGN_GLASS, ...FOREIGN_FIELDS]
/** Every tile: the shared Zen tile marker (unless it is a glass control) and the tile material. */
const TILE_SEL = [
  `[data-zen-glass]:not([${ZEN_MATERIAL_ATTR}="glass"])`,
  `[${ZEN_MATERIAL_ATTR}="tile"]`,
  ...FOREIGN_TILES,
  ...FOREIGN_RAISED_TILES,
]

const under = (root: string, sels: readonly string[], suffix = ''): string => sels.map((s) => `${root} ${s}${suffix}`).join(',\n')

/** A solid version of `fill`: the fill over the window's canvas, so a
 *  see-through surface token still paints an opaque tile. */
export const zenSolidFill = (fill: string): string => `linear-gradient(${fill}, ${fill}), var(--zen-canvas)`

const blurOff = '-webkit-backdrop-filter: none !important; backdrop-filter: none !important;'

// ── Progressive blur (the top bar, behind the message box) ──────────────

/** Companion's four layers, light to heavy: [blur px, mask clear until %,
 *  fully on at %], measured from the light edge toward the heavy one. Each
 *  starts before the one before it is fully on, so there are no bands. */
export const ZEN_PROGRESSIVE_BLUR_LAYERS: ReadonlyArray<readonly [number, number, number]> = [
  [2, 0, 25],
  [6, 15, 50],
  [12, 40, 75],
  [24, 65, 100],
]

/** The dark tint (the window's canvas) over the blur, same direction:
 *  [% from the light edge, % of the canvas]. */
export const ZEN_PROGRESSIVE_TINT_STOPS: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [30, 10],
  [60, 24],
  [100, 42],
]

/** Reduced transparency (no blur layers): the tint goes nearly opaque
 *  within 8px of the light edge ([px, % of the canvas]). */
export const ZEN_PROGRESSIVE_TINT_STOPS_OPAQUE: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [2, 40],
  [4, 72],
  [8, 96],
]

/** Which way the heavy end points: the header's at the top (the blur
 *  runs down from it), the footer's at the bottom (it runs up). */
const TOWARD = { header: 'to top', footer: 'to bottom' } as const

const tint = (dir: string, stops: ReadonlyArray<readonly [number, number]>, unit: '%' | 'px'): string =>
  `background: linear-gradient(${dir}, ${stops
    .map(([at, pct]) => `color-mix(in srgb, var(--zen-canvas) ${pct}%, transparent) ${at}${unit}`)
    .join(', ')});`

const progressiveCss = (edge: 'header' | 'footer'): string => {
  const scope = `${ROOT} .zen-${edge}-blur`
  const dir = TOWARD[edge]
  const layers = ZEN_PROGRESSIVE_BLUR_LAYERS.map(([px, from, to], i) => {
    const mask = `linear-gradient(${dir}, transparent ${from}%, #000 ${to}%)`
    return (
      `${scope} > .zen-blur-layer-${i + 1} { -webkit-backdrop-filter: blur(${px}px); backdrop-filter: blur(${px}px); ` +
      `-webkit-mask-image: ${mask}; mask-image: ${mask}; }`
    )
  }).join('\n')
  return `${layers}
${scope} > .zen-blur-tint { ${tint(dir, ZEN_PROGRESSIVE_TINT_STOPS, '%')} }
${ROOT}[${ZEN_REDUCED_TRANSPARENCY_ATTR}] .zen-${edge}-blur > .zen-blur-tint { ${tint(dir, ZEN_PROGRESSIVE_TINT_STOPS_OPAQUE, 'px')} }
@media (prefers-reduced-transparency: reduce) {
  ${scope} > .zen-blur-tint { ${tint(dir, ZEN_PROGRESSIVE_TINT_STOPS_OPAQUE, 'px')} }
}`
}

/** Every glass selector under `roots`, at rest, hovered and open. */
const glassStates = (roots: readonly string[]): string =>
  roots
    .flatMap((r) => ['', ':hover:not(:disabled)', '[aria-expanded="true"]'].map((suffix) => under(r, GLASS_SEL, suffix)))
    .join(',\n')

const reducedGlass = `
  background: ${zenSolidFill('var(--zen-surface-raised)')} !important;
  ${blurOff}
  border: 1px solid var(--zen-border) !important;
  box-shadow: none !important;`

export const ZEN_MATERIALS_CSS = `
${under(ROOT, GLASS_SEL)} {
  background: ${ZEN_MOBILE_GLASS.fill} !important;
  -webkit-backdrop-filter: ${ZEN_MOBILE_GLASS.blur} !important;
  backdrop-filter: ${ZEN_MOBILE_GLASS.blur} !important;
  border: ${ZEN_MOBILE_GLASS.edge} !important;
  box-shadow: ${ZEN_MOBILE_GLASS.highlight} !important;
  border-radius: ${ZEN_GLASS_RADII.pill}px !important;
}
${under(ROOT, [`[${ZEN_GLASS_SHAPE_ATTR}="round"]`])} { border-radius: ${ZEN_GLASS_RADII.round}px !important; }
${under(ROOT, [`[${ZEN_GLASS_SHAPE_ATTR}="field"]`, ...FOREIGN_FIELDS])} { border-radius: ${ZEN_GLASS_RADII.field}px !important; }
${under(ROOT, GLASS_SEL, ':hover:not(:disabled)')},
${under(ROOT, GLASS_SEL, '[aria-expanded="true"]')} { background: rgba(255, 255, 255, 0.16) !important; color: var(--zen-text); }
${under(LIGHT, GLASS_SEL)} {
  background: ${ZEN_MOBILE_GLASS_LIGHT.fill} !important;
  border: ${ZEN_MOBILE_GLASS_LIGHT.edge} !important;
  box-shadow: ${ZEN_MOBILE_GLASS_LIGHT.highlight} !important;
}
${under(LIGHT, GLASS_SEL, ':hover:not(:disabled)')},
${under(LIGHT, GLASS_SEL, '[aria-expanded="true"]')} { background: rgba(255, 255, 255, 0.82) !important; }
${under(ROOT, TILE_SEL)} {
  background: ${zenSolidFill('var(--zen-glass, var(--zen-surface))')} !important;
  ${blurOff}
  border-color: var(--zen-glass-edge) !important;
  box-shadow: var(--zen-glass-sheen), var(--zen-glass-shadow) !important;
}
${under(ROOT, [`[${ZEN_MATERIAL_ATTR}="tile"][${ZEN_TILE_RAISED_ATTR}="raised"]`, ...FOREIGN_RAISED_TILES])} {
  background: ${zenSolidFill('var(--zen-surface-raised)')} !important;
}
${ROOT} [${ZEN_MATERIAL_ATTR}="tile"][data-zen-tile-sheet] { border-bottom-width: 0 !important; box-shadow: var(--zen-glass-sheen) !important; }
${ROOT} [data-zen-drawer-tile][${ZEN_MATERIAL_ATTR}="tile"]:hover { background: ${zenSolidFill('color-mix(in srgb, var(--zen-surface-raised) 88%, var(--zen-text))')} !important; }
${ROOT} [data-zen-send][${ZEN_MATERIAL_ATTR}="tile"]:disabled { opacity: 0.45; }
${ROOT} [data-zen-send][${ZEN_MATERIAL_ATTR}="tile"]:not(:disabled) {
  background: var(--zen-accent) !important;
  color: var(--zen-accent-text) !important;
}
${ROOT} [data-zen-load-older][${ZEN_MATERIAL_ATTR}="solid"],
${ROOT} [data-zen-load-older][${ZEN_MATERIAL_ATTR}="solid"]:hover:not(:disabled) {
  background: transparent !important;
  box-shadow: none !important;
  padding-left: 0 !important;
  padding-right: 0 !important;
}
${ROOT} [data-zen-load-older][${ZEN_MATERIAL_ATTR}="solid"]:hover:not(:disabled) { color: var(--zen-text) !important; }
${ROOT} [data-zen-attachment][${ZEN_MATERIAL_ATTR}="solid"] { background: var(--zen-surface-raised) !important; ${blurOff} }
${ROOT} [data-zen-bubble] { ${blurOff} }
${ROOT} [data-zen-bubble] [data-testid="thread-choice-card"] {
  background: transparent !important;
  border-color: transparent !important;
  padding-left: 0 !important;
  padding-right: 0 !important;
}
${ROOT} [data-zen-bubble] [data-testid="thread-choice-chip"] {
  border-radius: 999px !important;
  background: transparent !important;
  border-color: color-mix(in srgb, currentColor 45%, transparent) !important;
  color: inherit !important;
}
${ROOT} [data-zen-bubble] [data-testid="thread-choice-chip"] > span:first-child { color: inherit !important; opacity: 0.7; }
${ROOT} [data-zen-bubble] [data-testid="thread-choice-chip"][data-selected="true"] {
  background: #ffffff !important;
  border-color: #ffffff !important;
  color: #000000 !important;
}
${ROOT} .zen-progressive-blur { position: absolute; left: 0; right: 0; pointer-events: none; z-index: 0; }
${ROOT} .zen-progressive-blur > span { position: absolute; inset: 0; pointer-events: none; }
${progressiveCss('header')}
${progressiveCss('footer')}
${RT} .zen-progressive-blur > [class^="zen-blur-layer-"] { display: none; }
${glassStates([RT, `${RT}[data-zen-scheme="light"]`])} {${reducedGlass}
}
${under(RT, TILE_SEL)} { ${blurOff} }
@media (prefers-reduced-transparency: reduce) {
  ${ROOT} .zen-progressive-blur > [class^="zen-blur-layer-"] { display: none; }
  ${glassStates([ROOT, LIGHT]).split('\n').join('\n  ')} {${reducedGlass}
  }
  ${under(ROOT, TILE_SEL).split('\n').join('\n  ')} { ${blurOff} }
}
`
