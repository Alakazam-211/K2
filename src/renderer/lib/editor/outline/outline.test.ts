// The outline (build-editors.md E3): the headings of a markdown file, with
// GitHub's anchor slugs (`@h:<slug>`), the line each starts on, and which one
// the top of the view is in. Pure.

import { describe, expect, it } from 'vitest'
import { Text } from '@codemirror/state'
import { githubSlug, headingSlugger, slugAddress } from './slug'
import { headingAt, outlineOf } from './outline'

describe('GitHub anchor slugs', () => {
  it('lower case, spaces to hyphens, punctuation dropped', () => {
    expect(githubSlug('Setup')).toBe('setup')
    expect(githubSlug('How a draft moves')).toBe('how-a-draft-moves')
    expect(githubSlug('Tokens & limits!')).toBe('tokens--limits')
    expect(githubSlug('What’s new in 0.46.14?')).toBe('whats-new-in-04614')
    expect(githubSlug('  Leading and trailing  ')).toBe('--leading-and-trailing--')
  })

  it('keeps letters and numbers in any script, hyphens and underscores; drops emoji', () => {
    expect(githubSlug('Café au lait')).toBe('café-au-lait')
    expect(githubSlug('日本語 の 見出し')).toBe('日本語-の-見出し')
    expect(githubSlug('snake_case-and-kebab')).toBe('snake_case-and-kebab')
    expect(githubSlug('Ship it 🚀')).toBe('ship-it-')
  })

  it('a heading used twice gets -1, -2 after it, as GitHub numbers them', () => {
    const s = headingSlugger()
    expect([s.slug('Notes'), s.slug('Notes'), s.slug('Notes'), s.slug('Notes 1')]).toEqual(['notes', 'notes-1', 'notes-2', 'notes-1-1'])
  })

  it('the address is @h:<slug>', () => {
    expect(slugAddress('how-a-draft-moves')).toBe('@h:how-a-draft-moves')
  })
})

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
