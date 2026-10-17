// EXPERIMENT (Rosson 2026-10-16, throwaway): the square Style's chamfered
// Thread with the K2 Companion mobile mapping of tiles and glass. Remove by
// reverting the commit that added this file.
//
// TILE (no blur), the Zen tile recipe (zen-glass.ts): solid fill, a 1px
// edge (the Zen glass edge), a subtle top sheen, a soft wide shadow.
//   - the message field and Send (a raised tile, dimmed until ready),
//   - the activity drawer's tab (a sheet tile, no bottom rim),
//   - choice and secret cards.
// GLASS (true glass) on small floating controls only: a white wash,
// backdrop blur 28px + saturate 180%, a 0.5px white edge, an inset top
// highlight. In the desktop Thread that is the drawer's floating Stop (the
// column has no jump-to-latest pill and no floating header buttons).
// SOLID, unchanged: bubbles, file chips, "Earlier messages", tags. The
// progressive blur behind the message box is untouched.
//
// Shapes stay the chamfers of ./chamfer: tiles recolour its `::before`
// (edge) / `::after` (fill) layers and put the sheen inside `::after`'s
// clip. The glass Stop is clipped to its chamfer itself (so its backdrop
// blur is too) and `::before` becomes a 0.5px ring along the cut polygon.
// Reduced transparency (or no backdrop-filter): Stop goes opaque, no blur.

import { CHAMFER_SCOPE } from './chamfer'

const S = CHAMFER_SCOPE
const LIGHT = 'html[data-style="square"][data-scheme="light"] [data-k2-thread-zen]'

/** The chamfer polygon (as ./chamfer draws it), at `t` px in from the box. */
function cut(k: string, t: number): string {
  return `max(0px, calc(var(--ch-${k}) - ${(t * 0.586).toFixed(3)}px))`
}

function chamferPoints(t: number): string[] {
  const p = (x: string, y: string): string => `${x} ${y}`
  const a = `${t}px`
  const b = (side: string): string => `calc(100% - ${t}px${side ? ` - ${side}` : ''})`
  return [
    p(`calc(${a} + ${cut('tl', t)})`, a),
    p(b(cut('tr', t)), a),
    p(b(''), `calc(${a} + ${cut('tr', t)})`),
    p(b(''), b(cut('br', t))),
    p(b(cut('br', t)), b('')),
    p(`calc(${a} + ${cut('bl', t)})`, b('')),
    p(a, b(cut('bl', t))),
    p(a, `calc(${a} + ${cut('tl', t)})`),
  ]
}

const chamferClip = `polygon(${chamferPoints(0).join(', ')})`

/** A ring `w` px wide along the chamfer: the outer path, then the inner one
 *  the other way round (nonzero winding leaves the inside empty). */
function chamferRing(w: number): string {
  const outer = chamferPoints(0)
  const inner = chamferPoints(w)
  const innerReversed = [inner[0], ...inner.slice(1).reverse()]
  return `polygon(${[...outer, outer[0], ...innerReversed, inner[0]].join(', ')})`
}

const COMPOSE_BOX = [`${S} [data-zen-compose] [data-zen-compose-box][data-chamfer]`, `${S} [data-zen-compose-box][data-chamfer]`]
const DRAWER_TAB = [`${S} [data-zen-compose] [data-zen-drawer-tile][data-chamfer]`, `${S} [data-zen-drawer-tile][data-chamfer]`]
const CARDS = [`${S} [data-testid="thread-choice-card"][data-chamfer]`, `${S} [data-testid="thread-secret-card"][data-chamfer]`]
const SEND = `${S} [data-zen-send][data-chamfer]`
const TILES = [...COMPOSE_BOX, ...DRAWER_TAB, ...CARDS, SEND]
const STOP = `${S} [data-zen-drawer-stop][data-chamfer]`
const STOP_LIGHT = `${LIGHT} [data-zen-drawer-stop][data-chamfer]`

const each = (sels: readonly string[], suffix = ''): string => sels.map((s) => s + suffix).join(',\n')

export const THREAD_GLASS_EXPERIMENT_CSS = `
${S} {
  --tg-edge: color-mix(in srgb, var(--zen-border) 55%, color-mix(in srgb, var(--zen-text) 14%, transparent));
  --tg-sheen: inset 0 1px 0 color-mix(in srgb, white 22%, transparent);
  --tg-shadow: 0 10px 30px color-mix(in srgb, black 8%, transparent);
}
${LIGHT} { --tg-sheen: inset 0 1px 0 color-mix(in srgb, white 60%, transparent); }

/* TILE */
${each(TILES)} { --ch-edge: var(--tg-edge) !important; box-shadow: var(--tg-shadow); }
${each(TILES, '::after')} { box-shadow: var(--tg-sheen); }
${each(COMPOSE_BOX)}, ${each(DRAWER_TAB, ':not(:hover)')} { --ch-fill: var(--zen-surface-raised) !important; }
${S} [data-testid="thread-secret-card"][data-chamfer]:not([data-warning=""]) { --ch-edge: var(--color-danger, #e5484d) !important; }
${SEND} { --ch-bw: 1px !important; --ch-outset: 0px !important; }
${SEND}::after { inset: 1px; }
${SEND}:not(:disabled) { --ch-edge: color-mix(in srgb, var(--zen-accent) 70%, white 30%) !important; }
${SEND}:disabled { --ch-fill: var(--zen-surface-raised) !important; opacity: 0.55; }

/* GLASS */
${STOP} {
  background: rgba(255, 255, 255, 0.10) !important;
  -webkit-backdrop-filter: blur(28px) saturate(180%);
  backdrop-filter: blur(28px) saturate(180%);
  clip-path: ${chamferClip};
}
${STOP}::before { inset: 0; background: rgba(255, 255, 255, 0.22); clip-path: ${chamferRing(0.5)}; }
${STOP}::after { inset: 0; background: transparent; box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.28); clip-path: ${chamferClip}; }
${STOP_LIGHT} { background: rgba(255, 255, 255, 0.55) !important; }
${STOP_LIGHT}::before { background: rgba(17, 17, 17, 0.14); }
${STOP_LIGHT}::after { box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.85); }
@media (prefers-reduced-transparency: reduce) {
  ${STOP}, ${STOP_LIGHT} { background: var(--zen-surface) !important; -webkit-backdrop-filter: none; backdrop-filter: none; }
}
@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {
  ${STOP}, ${STOP_LIGHT} { background: var(--zen-surface) !important; }
}
`
