// The markdown pack's own heading-slug cases (K2 Cores), run through K2's
// slugify and headingSlugs: the pack refuses an @h: address it doesn't know,
// so K2 must slug every heading exactly as the pack does, repeats included.
//
// fixtures/heading-slugs.json is a verbatim copy of
//   k2-cores/k2-mpc-markdown @ 8df9d53: crates/markdown/tests/data/heading-slugs.json
// copied 2026-11-16 (24 single headings, 9 documents).
// TODO: read the vendored copy instead (the vendored markdown pack's
// crates/markdown/tests/data/heading-slugs.json) once the next vendoring
// brings it in, and delete fixtures/heading-slugs.json.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { headingSlugs, slugify } from './heading-slug'

type One = { heading: string; slug: string }
type Doc = { headings: string[]; slugs: string[] }
const cases = JSON.parse(readFileSync(join(__dirname, 'fixtures', 'heading-slugs.json'), 'utf8')) as unknown
const ones = (cases as Array<One | Doc>).filter((c): c is One => 'heading' in c)
const docs = (cases as Array<One | Doc>).filter((c): c is Doc => 'headings' in c)

describe('the pack’s heading slugs', () => {
  it('has the cases it should (24 headings, 9 documents)', () => {
    expect([ones.length, docs.length]).toEqual([24, 9])
  })

  it.each(ones)('$heading → "$slug"', ({ heading, slug }) => {
    expect(slugify(heading)).toBe(slug)
  })

  it.each(docs)('$headings → $slugs', ({ headings, slugs }) => {
    const text = headings.map((h) => `## ${h}`).join('\n\nA line.\n\n')
    expect(headingSlugs(text).map((h) => h.slug)).toEqual(slugs)
  })
})
