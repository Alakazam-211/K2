// How a dashboard tile's 32px bar (PaneChrome) fits its parts. The name has
// priority: as the tile narrows, first the hint hides, then the view picker
// stops being centred and sits right after the name, then the ⌘N slot goes,
// then the picker folds to one menu button (the current view's icon), then
// the bell folds away (search stays: ⌘F is the tile's find), and only then
// does the name truncate. Every step keeps one row (the owner's mockup).

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
  /** The picker folded to one menu button; 0 when it can't fold. */
  pickerCompact?: number
  /** The ⌘N badge, with its gap to what follows it; 0 for none. */
  keys: number
  /** The bell and search (TileBarTools) and the rule after them, with
   *  their gaps; 0 for none. */
  tools: number
  /** The bell's part of `tools`; 0 for none. */
  bell: number
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
  /** The bell keeps its button (true when there is none to fold). */
  bell: boolean
  /** The picker is folded to its menu button. */
  compact: boolean
}

/** Before the bar is measured (and in a test DOM): everything in place. */
export const TILE_BAR_FIT_DEFAULT: TileBarFit = { hint: true, centred: true, keys: true, bell: true, compact: false }

export function sameTileBarFit(a: TileBarFit, b: TileBarFit): boolean {
  return a.hint === b.hint && a.centred === b.centred && a.keys === b.keys && a.bell === b.bell && a.compact === b.compact
}

/** Sub-pixel text widths are rounded by `scrollWidth`: leave a pixel. */
const SLACK = 1

export function fitTileBar(s: TileBarSizes): TileBarFit {
  const end = (keys: boolean, bell = true): number => s.close + (keys ? s.keys : 0) + s.tools - (bell ? 0 : s.bell)
  const named = (hint: boolean): number => s.name + (hint && s.hint > 0 ? s.gap + s.hint : 0)
  const hasHint = s.hint > 0
  const canFold = (s.pickerCompact ?? 0) > 0
  const fit = (o: Partial<TileBarFit>): TileBarFit => ({ hint: false, centred: false, keys: false, bell: true, compact: false, ...o })
  if (s.picker <= 0) {
    // name [hint] … [bell search |] [⌘N] ×
    const fits = (hint: boolean, keys: boolean, bell = true): boolean => named(hint) + s.gap + end(keys, bell) + SLACK <= s.width
    if (fits(true, true)) return fit({ hint: true, keys: true })
    if (hasHint && fits(false, true)) return fit({ keys: true })
    if (fits(false, false)) return fit({})
    return fit({ bell: s.bell <= 0 })
  }
  // name [hint] | picker | [bell search |] [⌘N] ×, the side columns equal while centred.
  const side = (s.width - s.picker - 2 * s.gap) / 2
  const centres = (hint: boolean): boolean => Math.max(named(hint), end(true)) + SLACK <= side
  if (centres(true)) return fit({ hint: true, centred: true, keys: true })
  if (hasHint && centres(false)) return fit({ centred: true, keys: true })
  // Then the picker follows the name: in full, then folded to its button.
  const follows = (keys: boolean, compact: boolean, bell = true): boolean =>
    s.name + s.gap + (compact ? (s.pickerCompact ?? 0) : s.picker) + s.gap + end(keys, bell) + SLACK <= s.width
  if (follows(true, false)) return fit({ keys: true })
  if (follows(false, false)) return fit({})
  if (canFold && follows(false, true)) return fit({ compact: true })
  return fit({ compact: canFold, bell: s.bell <= 0 })
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
  // Folded parts keep their full size as their slots' scroll width.
  const full = q('[data-tile-picker-full]')
  const compact = q('[data-tile-picker-compact]')
  const tools = q('[data-tile-tools]')
  // A folded bell keeps its full size as its slot's scroll width; search is
  // its button (its open field hangs under the bar, outside the count).
  const bell = q('[data-tile-tool="bell"]')?.scrollWidth ?? 0
  const searchSlot = q('[data-tile-tool="search"]')
  const search = searchSlot ? (searchSlot.querySelector<HTMLElement>('[data-tile-button]')?.offsetWidth ?? 0) + px(searchSlot, 'marginLeft') : 0
  const sep = q('[data-tile-sep]')
  const rule = sep ? sep.offsetWidth + px(sep, 'marginLeft') + px(sep, 'marginRight') : 0
  return {
    width,
    gap,
    name: (lead ? lead.offsetWidth + gap : 0) + (q('[data-tile-title]')?.scrollWidth ?? 0),
    hint: q('[data-tile-hint]')?.scrollWidth ?? 0,
    picker: full ? full.scrollWidth : (picker?.offsetWidth ?? 0),
    pickerCompact: compact?.scrollWidth ?? 0,
    // The badge's slot keeps a gap to the close button (CSS margin).
    keys: keys ? keys.scrollWidth + (close ? gap : 0) : 0,
    tools: tools && bell + search > 0 ? bell + search + rule : 0,
    bell,
    close: close?.offsetWidth ?? 0,
  }
}

function px(el: Element, prop: 'marginLeft' | 'marginRight'): number {
  return parseFloat(getComputedStyle(el)[prop]) || 0
}
