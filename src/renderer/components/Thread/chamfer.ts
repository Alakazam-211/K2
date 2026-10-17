// Chamfered corners in K2's square Style.
//
// Rosson 2026-10-16: in K2's square Style the Thread doesn't go round. Its
// bubbles, message field, Send, activity drawer (tab, Stop, tags and pulse
// dot), file chips, cards and the "Earlier messages" pill get the chamfered
// corners of the presence chips (mockup-activity-status-v1 §Presence: small
// 45° corner cuts). Bezel and Glass (rounded Styles) and Zen's own themes
// keep the rounded look.
//
// An element opts in with `data-chamfer` and `chamferVars(...)`. Under
// `html[data-style="square"]` inside the desktop Thread box, its own fill,
// border and radius go transparent/0 and two pseudo layers draw it instead:
// `::before` the edge (border colour) and `::after` the fill, each clipped
// to the cut polygon. The element itself is never clipped, so focus rings
// and children stay whole. Borderless inline tags (the drawer's counters
// and code) are clipped directly.
//
// Its own module (no components) so the cards in SessionView can use it
// without an import cycle through the Thread kit.

import type { CSSProperties } from 'react'

/** The attribute the desktop Thread puts on its box (the CSS scope). */
export const THREAD_ZEN_SCOPE_ATTR = 'data-k2-thread-zen'

/** Corner cuts (px): top-left, top-right, bottom-right, bottom-left. */
export type ChamferCorners = readonly [number, number, number, number]

/** Fixed cuts: big surfaces (bubbles, the field, cards) 4px, Send 5px at
 *  32px (Rosson: 3px was too subtle), the drawer's 32px tab 5px. Smaller
 *  controls scale with `chamferCutFor`. */
export const CHAMFER_CUT = { surface: 4, button: 3, send: 5, drawer: 5 } as const

/** A control's cut by its height: Send's 5px at 32px, to the nearest 0.5px. */
export function chamferCutFor(sizePx: number): number {
  return Math.round(((sizePx * CHAMFER_CUT.send) / 32) * 2) / 2
}

export function chamferVars(o: {
  /** One cut for every corner, or one per corner (TL, TR, BR, BL). */
  cut: number | ChamferCorners
  /** The element's fill. */
  fill: string
  /** Its border colour and width (none by default). */
  edge?: string
  border?: number
  /** `::before`'s inset when the border isn't on every side (CSS inset). */
  outset?: string
}): CSSProperties {
  const [tl, tr, br, bl] = typeof o.cut === 'number' ? [o.cut, o.cut, o.cut, o.cut] : o.cut
  const v: Record<string, string> = {
    '--ch-tl': `${tl}px`,
    '--ch-tr': `${tr}px`,
    '--ch-br': `${br}px`,
    '--ch-bl': `${bl}px`,
    '--ch-fill': o.fill,
    '--ch-edge': o.edge ?? o.fill,
    '--ch-bw': `${o.border ?? 0}px`,
  }
  if (o.outset) v['--ch-outset'] = o.outset
  return v as CSSProperties
}

/** The cut polygon, each corner's cut less `less` (the inner fill's). */
function chamferPolygon(less: string): string {
  const c = (k: string): string => `max(0px, calc(var(--ch-${k}) - ${less}))`
  return (
    `polygon(${c('tl')} 0, calc(100% - ${c('tr')}) 0, 100% ${c('tr')}, 100% calc(100% - ${c('br')}), ` +
    `calc(100% - ${c('br')}) 100%, ${c('bl')} 100%, 0 calc(100% - ${c('bl')}), 0 ${c('tl')})`
  )
}

/** Where chamfers apply: the desktop Thread box under K2's square Style. */
export const CHAMFER_SCOPE = `html[data-style="square"] [${THREAD_ZEN_SCOPE_ATTR}]`

const allCorners = (px: number): string => `--ch-tl: ${px}px; --ch-tr: ${px}px; --ch-br: ${px}px; --ch-bl: ${px}px;`

export const THREAD_CHAMFER_CSS = `
${CHAMFER_SCOPE} [data-chamfer] { border-radius: 0 !important; border-color: transparent !important; background: transparent !important; position: relative; isolation: isolate; }
${CHAMFER_SCOPE} [data-chamfer]::before, ${CHAMFER_SCOPE} [data-chamfer]::after { content: ''; position: absolute; pointer-events: none; z-index: -1; }
${CHAMFER_SCOPE} [data-chamfer]::before { inset: var(--ch-outset, calc(-1 * var(--ch-bw, 0px))); background: var(--ch-edge, var(--ch-fill)); clip-path: ${chamferPolygon('0px')}; }
${CHAMFER_SCOPE} [data-chamfer]::after { inset: 0; background: var(--ch-fill); clip-path: ${chamferPolygon('calc(var(--ch-bw, 0px) * 0.586)')}; }
${CHAMFER_SCOPE} [data-zen-drawer-tile][data-chamfer] { overflow: visible !important; }
${CHAMFER_SCOPE} [data-zen-soft-button][data-chamfer]:hover:not(:disabled) { --ch-fill: var(--zen-surface-raised) !important; }
${CHAMFER_SCOPE} [data-testid="thread-choice-chip"][data-chamfer]:hover:not(:disabled) { --ch-edge: var(--thread-card-accent, var(--color-accent)) !important; }
${CHAMFER_SCOPE} [data-zen-drawer-dot] { border-radius: 0 !important; }
${CHAMFER_SCOPE} .zen-drawer-tag { border-radius: 0 !important; ${allCorners(chamferCutFor(18))} clip-path: ${chamferPolygon('0px')}; }
${CHAMFER_SCOPE} .zen-drawer-tag.zen-drawer-code { ${allCorners(chamferCutFor(16))} }
`
