// Find in a PDF (P1): each page's text items joined into one string (an
// item that ends a line adds a space), matched case-blind by default, and
// each match mapped back to the text items it covers, so the text layer
// (one span per item, in the same order) can mark it.

export interface TextItemLike {
  str?: string
  hasEOL?: boolean
}

export interface PageText {
  text: string
  /** Where each item starts in `text`, and its length. */
  items: { start: number; len: number }[]
}

export interface FindMatch {
  page: number
  start: number
  end: number
}

export function pageText(items: readonly TextItemLike[]): PageText {
  let text = ''
  const out: PageText['items'] = []
  for (const it of items) {
    const s = it.str ?? ''
    out.push({ start: text.length, len: s.length })
    text += s
    if (it.hasEOL && !/\s$/.test(text)) text += ' '
  }
  return { text, items: out }
}

/** Every match, page by page (pages[0] is page 1), in reading order. */
export function findMatches(pages: readonly (PageText | null)[], query: string, opts: { matchCase?: boolean } = {}): FindMatch[] {
  const q = opts.matchCase ? query : query.toLocaleLowerCase()
  if (q.trim() === '') return []
  const out: FindMatch[] = []
  pages.forEach((p, i) => {
    if (!p) return
    const hay = opts.matchCase ? p.text : p.text.toLocaleLowerCase()
    for (let at = hay.indexOf(q); at >= 0; at = hay.indexOf(q, at + q.length)) out.push({ page: i + 1, start: at, end: at + q.length })
  })
  return out
}

/** The parts of each text item a match covers. */
export function matchSegments(page: PageText, m: FindMatch): { item: number; from: number; to: number }[] {
  const out: { item: number; from: number; to: number }[] = []
  page.items.forEach((it, i) => {
    const from = Math.max(m.start, it.start)
    const to = Math.min(m.end, it.start + it.len)
    if (to > from) out.push({ item: i, from: from - it.start, to: to - it.start })
  })
  return out
}

/** "2 of 9", or "No matches" (none for an empty query). */
export function findCount(query: string, matches: readonly FindMatch[], current: number): string {
  if (query.trim() === '') return ''
  return matches.length === 0 ? 'No matches' : `${current + 1} of ${matches.length}`
}
