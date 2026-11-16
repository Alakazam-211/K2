// The outline (build-editors.md E3): the headings of a markdown file, with
// the slugs of their addresses (`@h:<slug>`, the same as
// lib/editor/live/heading-slug), the line each starts on, and which one the
// top of the view is in. Pure.

import { describe, expect, it } from 'vitest'
import { Text } from '@codemirror/state'
import { headingSlugs } from '@/lib/editor/live/heading-slug'
import { headingAt, outlineOf } from './outline'

const DOC = [
  '# Quill',
  '',
  'Intro words.',
  '',
  '## Setup',
  '```bash',
  '# not a heading',
  '```',
  '## The **bold** `code` and [a link](x.md)',
  'Setext two',
  '----------',
  'Setext one',
  '==========',
  '### Setup',
  '#hashtag is not a heading',
  '    # indented code is not either',
  '###### Six ######',
].join('\n')

describe('the outline', () => {
  const items = outlineOf(Text.of(DOC.split('\n')))

  it('lists ATX and setext headings outside code, as text without their marks', () => {
    expect(items.map((h) => [h.level, h.text, h.line])).toEqual([
      [1, 'Quill', 1],
      [2, 'Setup', 5],
      [2, 'The bold code and a link', 9],
      [2, 'Setext two', 10],
      [1, 'Setext one', 12],
      [3, 'Setup', 14],
      [6, 'Six', 17],
    ])
  })

  it('slugs each, numbering repeats', () => {
    expect(items.map((h) => h.slug)).toEqual(['quill', 'setup', 'the-bold-code-and-a-link', 'setext-two', 'setext-one', 'setup-1', 'six'])
  })

  it('an ATX heading’s slug is its address slug (heading-slug.ts), so outline and addresses agree', () => {
    const atx = ['# Quill', '## Setup', '## Tokens & limits!', '## The **bold** `code` and [a link](x.md)', '### Setup', '## What’s new in 0.46.14?'].join('\n\n')
    expect(outlineOf(Text.of(atx.split('\n'))).map((h) => h.slug)).toEqual(headingSlugs(atx).map((h) => h.slug))
  })

  it('the heading a line is in: the last one at or above it, none above the first', () => {
    expect(headingAt(items, 1)).toBe(0)
    expect(headingAt(items, 7)).toBe(1)
    expect(headingAt(items, 13)).toBe(4)
    expect(headingAt(items, 100)).toBe(6)
    expect(headingAt(outlineOf(Text.of(['Intro', '', '## Later'])), 1)).toBe(-1)
  })

  it('an empty file has no headings', () => {
    expect(outlineOf(Text.of(['']))).toEqual([])
  })
})
