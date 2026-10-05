// Rosson 2026-10-04 — one glass for every Zen tile: "the tile that holds the
// tickets feels like glass, same with the usage button tile … update the
// other tiles to all have that effect. It's very subtle, but it looks good!"
//
// The recipe is the Tickets view's panels and the usage tool's pill, made
// one definition:
//   - a flat Zen surface inside (no see-through gradient; Rosson 2026-10-04),
//   - backdrop blur + saturate (with the `-webkit-` prefix: WebKit draws
//     it; no SVG filters, no `url(`),
//   - a soft edge (the Zen border mixed with a little text colour),
//   - a top highlight (inset white line, the subtle sheen) and a faint,
//     wide drop shadow.
// Colours are Zen tokens (or plain white / black at low alpha), so it reads
// in every Zen theme, light and dark. Reduced transparency: a solid Zen
// surface, no blur.
//
// A tile opts in with `data-zen-glass` (`ZEN_GLASS_PROPS`). Views that can't
// put the marker on someone else's elements (the Tickets board, the usage
// chip) pass their selectors to `zenGlassRule`, so they get the very same
// declarations. The tokens live on the Zen root (`ZEN_GLASS_CSS`, drawn
// once by `ZenRoot`).

/** The marker attribute a glass tile carries. */
export const ZEN_GLASS_ATTR = 'data-zen-glass'

/** Spread onto a tile: `<div {...ZEN_GLASS_PROPS}>`. */
export const ZEN_GLASS_PROPS = { [ZEN_GLASS_ATTR]: '' } as const

/** The glass tokens, on the Zen root. */
export const ZEN_GLASS_TOKENS_CSS = `
[data-zen-root] {
  /* Rosson 2026-10-04: a flat inside — no backdrop showing through as a
     gradient. The glass is the edge: rim, top highlight, shadow. */
  --zen-glass: var(--zen-surface);
  --zen-glass-raised: var(--zen-surface-raised);
  --zen-glass-edge: color-mix(in srgb, var(--zen-border) 55%, color-mix(in srgb, var(--zen-text) 14%, transparent));
  /* No backdrop blur: the inside is solid, and WebKit left stale blurred
     layers behind after a theme switch (paper → basic). Rosson 2026-10-04. */
  --zen-glass-blur: none;
  --zen-glass-sheen: inset 0 1px 0 color-mix(in srgb, white 22%, transparent);
  --zen-glass-shadow: 0 10px 30px color-mix(in srgb, black 8%, transparent);
}
`

/** The glass declarations for `selectors`, plus the reduced-transparency
 *  fallback (solid surface, no blur). Border radius is the tile's own. */
export function zenGlassRule(selectors: readonly string[]): string {
  if (selectors.length === 0) throw new Error('zen glass: no selectors')
  const sel = selectors.join(',\n')
  return `
${sel} {
  background: var(--zen-glass);
  -webkit-backdrop-filter: var(--zen-glass-blur);
  backdrop-filter: var(--zen-glass-blur);
  border: 1px solid var(--zen-glass-edge);
  box-shadow: var(--zen-glass-sheen), var(--zen-glass-shadow);
}
@media (prefers-reduced-transparency: reduce) {
  ${selectors.join(',\n  ')} {
    background: var(--zen-surface);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }
}
`
}

const TILE = `[data-zen-root] [${ZEN_GLASS_ATTR}]`

/** The shared stylesheet: tokens, every `data-zen-glass` tile, and the
 *  pressed / hovered look of a glass button (the usage chip's). */
export const ZEN_GLASS_CSS = `${ZEN_GLASS_TOKENS_CSS}${zenGlassRule([TILE])}
${TILE}[data-zen-soft-button]:hover:not(:disabled),
${TILE}[aria-expanded="true"] {
  color: var(--zen-text);
  background: var(--zen-glass-raised);
}
@media (prefers-reduced-transparency: reduce) {
  ${TILE}[data-zen-soft-button]:hover:not(:disabled),
  ${TILE}[aria-expanded="true"] { background: var(--zen-surface-raised); }
}
`
