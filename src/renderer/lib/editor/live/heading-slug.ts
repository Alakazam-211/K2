// Heading slugs for the kit's heading addresses (`@h:<slug>`; K2 Cores: kit
// addr.rs is the one grammar, and the markdown pack refuses a heading it
// doesn't know), by the pack's rules (its cases: ./fixtures/heading-slugs.json):
//   - the heading as written: no Markdown rendered first, so a link's target,
//     backticks' contents and underscores count like any other characters;
//   - lowercased one character at a time ('ΟΔΟΣ' → 'οδοσ', no final 'ς');
//   - kept: Unicode Alphabetic and Number characters, the combining
//     diacritics blocks, '-' and '_'; every space a '-'; everything else
//     dropped (a virama too: 'नमस्ते' → 'नमसते'); nothing encoded;
//   - repeats in document order get -1, -2, never a slug already used, as
//     github-slugger numbers them ('Notes', 'Notes', 'Notes', 'Notes 1' →
//     notes, notes-1, notes-2, notes-1-1). A heading with nothing kept has the
//     empty slug, and repeats of it number too ('', '-1').
// Pure.

const HEADING = /^ {0,3}#{1,6}(?:[ \t]+(.*?))?[ \t#]*$/

/** What a slug keeps: letters (Alphabetic), numbers, the combining diacritics blocks, '-' and '_'; whitespace becomes '-'. */
const DROP = /[^\p{Alphabetic}\p{N}\u0300-\u036F\u1AB0-\u1AFF\u1DC0-\u1DFF\u20D0-\u20FF\uFE20-\uFE2F\s_-]/gu

/** The slug of one heading's text (before repeats are numbered). */
export function slugify(heading: string): string {
  // One character at a time: no final-sigma rule.
  const lower = [...heading.trim()].map((c) => c.toLowerCase()).join('')
  return lower.replace(DROP, '').replace(/\s/g, '-')
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
  // Each slug used so far, with how many repeats of it as an original have been numbered.
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
    let slug = base
    while (seen.has(slug)) {
      const n = (seen.get(base) ?? 0) + 1
      seen.set(base, n)
      slug = `${base}-${n}`
    }
    seen.set(slug, 0)
    out.push({ line: i + 1, text: words, slug })
  })
  return out
}
