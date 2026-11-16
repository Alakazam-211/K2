// Clip labels (v2.7b): a label is the clip's full name, else one of its short
// names, else the clip's kind glyph. Never a fragment and never "…". The
// full name is always the clip's tooltip and accessible name.
//
// Short names, longest first:
//   - the part before " · " or ":" ("Lower third · Interview C" → "Lower third");
//   - without leading zeros ("B-roll 03" → "B-roll 3");
//   - fewer trailing words, only where the dropped tail is a phrase that
//     starts with a small word ("Harbour at dawn" → "Harbour"). An
//     identifier is never dropped: "Interview B" stays whole.
// Each form is measured as painted by the caller (`fits`), longest first.

export type KindGlyph = 'title' | 'nest' | 'video' | 'audio'

const SMALL = /^(at|of|the|on|in|to|by|for|with|from|and)$/i

/** Every whole form of a name, the full name first. */
export function shortNames(full: string): string[] {
  const out = [full]
  const add = (x: string): void => {
    const t = String(x || '').trim()
    if (t && !out.includes(t)) out.push(t)
  }
  const base = full.split(' · ')[0]
  const pre = full.split(':')[0]
  ;[base, pre].forEach(add)
  ;[full, base].forEach((x) => add(x.replace(/\b0+(\d)/g, '$1')))
  const words = base.split(' ')
  for (let k = words.length - 1; k >= 1; k--) if (SMALL.test(words[k])) add(words.slice(0, k).join(' '))
  return out
}

/** The longest whole form within `n` characters; '' when none (the caller shows the glyph). */
export function bestName(full: string, n: number): string {
  if (n < 3) return ''
  for (const x of shortNames(full)) if (x.length <= n) return x
  return ''
}

/** The glyph a clip gives way to: titles (generators) and nests by their kind, else the lane's. */
export function kindGlyphOf(itemKind: string, trackKind: string): KindGlyph {
  return itemKind === 'generator' ? 'title' : itemKind === 'nest' ? 'nest' : trackKind === 'video' ? 'video' : 'audio'
}

export interface ClipLabel {
  /** The words painted: the full name or a short one; null when only the glyph shows. */
  text: string | null
  glyph: KindGlyph | null
  /** Always the full name. */
  full: string
}

/**
 * A clip's label, measured as painted: `fits(text)` says whether a form fits
 * whole in the visible part of the clip. Titles and nests lead with their
 * glyph; any clip whose names all fail shows its glyph when `glyphFits`.
 */
export function clipLabel(full: string, glyph: KindGlyph, fits: (text: string) => boolean, glyphFits = true): ClipLabel {
  const lead = glyph === 'title' || glyph === 'nest'
  for (const x of shortNames(full)) if (fits(x)) return { text: x, glyph: lead ? glyph : null, full }
  return { text: null, glyph: lead || glyphFits ? glyph : null, full }
}

/** The name band's padding (6 px before the name, 4 after). */
export const LABEL_PAD = 10
/** A kind glyph's room in the band: 10 px and the band's 4 px gap. */
export const GLYPH_W = 14

/**
 * A clip's label in `room` px (the visible part of its name band), measured
 * as painted: `width(text)` is the painted width of a name form, `badges`
 * the room its badges take. Titles and nests need their glyph's room too.
 * When no name fits, the glyph shows if it fits; else nothing.
 */
export function labelForRoom(full: string, glyph: KindGlyph, room: number, width: (text: string) => number, badges = 0): ClipLabel {
  const lead = glyph === 'title' || glyph === 'nest' ? GLYPH_W : 0
  const space = room - LABEL_PAD - badges
  const r = clipLabel(full, glyph, (t) => width(t) + lead <= space + 0.01, space >= GLYPH_W)
  return r.text === null && space < GLYPH_W ? { ...r, glyph: null } : r
}

/** A status in at most two lines, no ellipsis: when longer, trailing " · " phrases drop whole, then anything after a colon. */
export function twoLines(t: string, n: number): string {
  let x = String(t || '')
  while (x.length > n && x.includes(' · ')) x = x.slice(0, x.lastIndexOf(' · '))
  if (x.length > n) {
    const i = x.indexOf(':')
    if (i > 0) x = x.slice(0, i)
  }
  return x
}
