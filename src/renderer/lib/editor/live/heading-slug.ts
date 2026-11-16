// Heading slugs for the kit's heading addresses (`@h:<slug>`; K2 Cores: kit
// addr.rs is the one grammar), GitHub-style: the heading's words lowercased,
// each space a '-', punctuation dropped (letters and numbers in any script,
// '-' and '_' kept), nothing encoded. Markdown's own marks go first: inline
// code's backticks, emphasis, a link's target (its words stay). Repeats in
// document order get -1, -2 (`getting-started`, `getting-started-1`). An
// empty heading has no slug. Pure.

const HEADING = /^ {0,3}#{1,6}(?:[ \t]+(.*?))?[ \t#]*$/

/** The slug of one heading's text. */
export function slugify(heading: string): string {
  const words = heading
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[`*~]/g, '')
    .trim()
    .toLowerCase()
  return words.replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '').replace(/\s/g, '-')
}

export interface HeadingSlug {
  /** The heading's line (1-based). */
  line: number
  text: string
  slug: string
}

/** Every heading with a slug, in document order, repeats numbered (code fences skipped). */
export function headingSlugs(text: string): HeadingSlug[] {
  const out: HeadingSlug[] = []
  const seen = new Map<string, number>()
  let fence = false
  text.split('\n').forEach((l, i) => {
    if (/^ {0,3}(```|~~~)/.test(l)) {
      fence = !fence
      return
    }
    if (fence) return
    const m = HEADING.exec(l)
    if (!m) return
    const words = (m[1] ?? '').trim()
    const base = slugify(words)
    if (!base) return
    const n = seen.get(base) ?? 0
    seen.set(base, n + 1)
    out.push({ line: i + 1, text: words, slug: n === 0 ? base : `${base}-${n}` })
  })
  return out
}
