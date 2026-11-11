// How a dashboard tile's 32px bar (PaneChrome) fits its parts. The name has
// priority: as the tile narrows, first the hint hides, then the view picker
// stops being centred and sits right after the name, then the ⌘N slot goes,
// and only then does the name truncate. (A tile narrower than 240px puts the
// picker on its own row: globals.css, agent-tile.)

/** Natural widths in px, read from the bar (`measureTileBar`). */
export interface TileBarSizes {
  /** The bar's content box. */
  width: number
  /** The bar's column gap. */
  gap: number
  /** The picture and the full name. */
  name: number
  /** The hint after the name (a doc tile's owner); 0 for none. */
  hint: number
  /** The view picker; 0 for none (a doc tile). */
  picker: number
  /** The ⌘N badge, with its gap to the close button; 0 for none. */
  keys: number
  /** The close button; 0 when read-only. */
  close: number
}

export interface TileBarFit {
  /** The hint shows. */
  hint: boolean
  /** The picker sits in the bar's centre (else right after the name). */
  centred: boolean
  /** The ⌘N badge keeps its slot (it shows on hover and keyboard focus). */
  keys: boolean
}

/** Before the bar is measured (and in a test DOM): everything in place. */
export const TILE_BAR_FIT_DEFAULT: TileBarFit = { hint: true, centred: true, keys: true }

export function sameTileBarFit(a: TileBarFit, b: TileBarFit): boolean {
  return a.hint === b.hint && a.centred === b.centred && a.keys === b.keys
}

/** Sub-pixel text widths are rounded by `scrollWidth`: leave a pixel. */
const SLACK = 1

export function fitTileBar(s: TileBarSizes): TileBarFit {
  const end = (keys: boolean): number => s.close + (keys ? s.keys : 0)
  const named = (hint: boolean): number => s.name + (hint && s.hint > 0 ? s.gap + s.hint : 0)
  const hasHint = s.hint > 0
  if (s.picker <= 0) {
    // name [hint] … [⌘N] ×
    const fits = (hint: boolean, keys: boolean): boolean => named(hint) + s.gap + end(keys) + SLACK <= s.width
    if (fits(true, true)) return { hint: true, centred: false, keys: true }
    if (hasHint && fits(false, true)) return { hint: false, centred: false, keys: true }
    return { hint: false, centred: false, keys: false }
  }
  // name [hint] | picker | [⌘N] ×, the side columns equal while centred.
  const side = (s.width - s.picker - 2 * s.gap) / 2
  const centres = (hint: boolean): boolean => Math.max(named(hint), end(true)) + SLACK <= side
  if (centres(true)) return { hint: true, centred: true, keys: true }
  if (hasHint && centres(false)) return { hint: false, centred: true, keys: true }
  const follows = s.name + s.gap + s.picker + s.gap + end(true) + SLACK <= s.width
  return { hint: false, centred: false, keys: follows }
}

/** Reads the natural widths from a rendered bar. `null` before layout (a
 *  zero-width bar, as in a test DOM). Hidden parts collapse to zero width
 *  with `overflow: hidden`, so `scrollWidth` still gives their full size. */
export function measureTileBar(bar: HTMLElement): TileBarSizes | null {
  const cs = getComputedStyle(bar)
  const width = bar.clientWidth - (parseFloat(cs.paddingLeft) || 0) - (parseFloat(cs.paddingRight) || 0)
  if (!(width > 0)) return null
  const gap = parseFloat(cs.columnGap) || 0
  const q = (sel: string): HTMLElement | null => bar.querySelector<HTMLElement>(sel)
  const lead = q('[data-tile-lead]')
  const keys = q('[data-tile-shortcut]')
  const close = q('[data-tile-close]')
  const picker = q('[data-tile-picker]')
  return {
    width,
    gap,
    name: (lead ? lead.offsetWidth + gap : 0) + (q('[data-tile-title]')?.scrollWidth ?? 0),
    hint: q('[data-tile-hint]')?.scrollWidth ?? 0,
    picker: picker?.offsetWidth ?? 0,
    // The badge's slot keeps a gap to the close button (CSS margin).
    keys: keys ? keys.scrollWidth + (close ? gap : 0) : 0,
    close: close?.offsetWidth ?? 0,
  }
}
