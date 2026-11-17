// How a dashboard tile's 32px bar (PaneChrome) fits its parts. The name has
// priority over what can give way: as the tile narrows, first the hint
// hides, then the view picker stops being centred and sits right after the
// name, then the picker folds to one menu button (the current view's icon),
// then the bell folds away (search stays: ⌘F is the tile's find), and only
// then does the name truncate. Every step keeps one row (the owner's mockup).
//
// The ⌘N hint always shows (Rosson, 2026-11-17): its room is always kept,
// so the name truncates before it would.
//
// An asleep agent's mark is state, not a hint (Rosson, 2026-11-11): it is
// never dropped. The word "asleep" holds while the picker moves off centre;
// then it shrinks to a moon (named "Asleep"), before the picker folds. The
// name truncates before the moon would.

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
  /** The asleep word after the name ("asleep"); 0 when awake. */
  asleep?: number
  /** The asleep moon, the word's tight form; 0 when awake. */
  asleepIcon?: number
}

export interface TileBarFit {
  /** The hint shows. */
  hint: boolean
  /** The picker sits in the bar's centre (else right after the name). */
  centred: boolean
  /** The bell keeps its button (true when there is none to fold). */
  bell: boolean
  /** The picker is folded to its menu button. */
  compact: boolean
  /** An asleep agent's mark shows as the word (else as the moon). It never
   *  hides. True when awake. */
  asleepWord: boolean
}

/** Before the bar is measured (and in a test DOM): everything in place. */
export const TILE_BAR_FIT_DEFAULT: TileBarFit = { hint: true, centred: true, bell: true, compact: false, asleepWord: true }

export function sameTileBarFit(a: TileBarFit, b: TileBarFit): boolean {
  return (
    a.hint === b.hint &&
    a.centred === b.centred &&
    a.bell === b.bell &&
    a.compact === b.compact &&
    a.asleepWord === b.asleepWord
  )
}

/** Sub-pixel text widths are rounded by `scrollWidth`: leave a pixel. */
const SLACK = 1

export function fitTileBar(s: TileBarSizes): TileBarFit {
  // The controls: [bell search |] ⌘N ×. ⌘N's room is always kept.
  const end = s.close + s.keys + s.tools
  const asleep = (s.asleep ?? 0) > 0
  // The asleep mark rides with the name: the word, or the moon when tight.
  const mark = (word: boolean): number => (asleep ? s.gap + (word ? (s.asleep ?? 0) : (s.asleepIcon ?? 0)) : 0)
  const named = (hint: boolean, word = true): number => s.name + (hint && s.hint > 0 ? s.gap + s.hint : 0) + mark(word)
  const hasHint = s.hint > 0
  const canFold = (s.pickerCompact ?? 0) > 0
  const fit = (o: Partial<TileBarFit>): TileBarFit => ({ hint: false, centred: false, bell: true, compact: false, asleepWord: true, ...o })
  if (s.picker <= 0) {
    // name [asleep] [hint] … [bell search |] [⌘N] ×
    const fits = (hint: boolean, word = true): boolean => named(hint, word) + s.gap + end + SLACK <= s.width
    if (fits(true)) return fit({ hint: true })
    if (hasHint && fits(false)) return fit({})
    if (asleep && fits(false, false)) return fit({ asleepWord: false })
    return fit({ bell: s.bell <= 0, asleepWord: !asleep })
  }
  // name [asleep] [hint] | picker | [bell search |] [⌘N] ×, the side columns equal while centred.
  const side = (s.width - s.picker - 2 * s.gap) / 2
  const centres = (hint: boolean): boolean => Math.max(named(hint), end) + SLACK <= side
  if (centres(true)) return fit({ hint: true, centred: true })
  if (hasHint && centres(false)) return fit({ centred: true })
  // Then the picker follows the name: in full, then folded to its button.
  // The asleep word holds through both, and gives way to the moon before
  // the picker folds.
  const follows = (compact: boolean, word = true): boolean =>
    named(false, word) + s.gap + (compact ? (s.pickerCompact ?? 0) : s.picker) + s.gap + end + SLACK <= s.width
  if (follows(false)) return fit({})
  if (asleep && follows(false, false)) return fit({ asleepWord: false })
  if (canFold && follows(true, !asleep)) return fit({ compact: true, asleepWord: !asleep })
  return fit({ compact: canFold, bell: s.bell <= 0, asleepWord: !asleep })
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
    // Both forms keep their full size as their slots' scroll width.
    asleep: q('[data-tile-asleep-word]')?.scrollWidth ?? 0,
    asleepIcon: q('[data-tile-asleep-icon]')?.scrollWidth ?? 0,
  }
}

function px(el: Element, prop: 'marginLeft' | 'marginRight'): number {
  return parseFloat(getComputedStyle(el)[prop]) || 0
}
