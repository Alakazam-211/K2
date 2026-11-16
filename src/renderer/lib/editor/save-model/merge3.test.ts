// The plain-file three-way merge: changes only one side made land, and where
// both changed the same lines both are kept, yours first (the kit's rule).

import { describe, expect, it } from 'vitest'
import { merge3 } from './merge3'

const L = (...lines: string[]): string => lines.join('\n')

describe('merge3', () => {
  const base = L('# Harbor', '', 'Intro.', '', '## Setup', '', '1. Clone.', '', '## Tasks', '', '- [ ] Add the checker', '')

  it('non-overlapping changes from both sides land', () => {
    const yours = base.replace('Intro.', 'Intro, with a new sentence.')
    const theirs = base.replace('- [ ] Add the checker', '- [x] Add the checker')
    const r = merge3(base, theirs, yours)
    expect(r.text).toBe(base.replace('Intro.', 'Intro, with a new sentence.').replace('- [ ]', '- [x]'))
    expect(r.kept).toEqual([])
  })

  it('both changed the same line: both are kept, yours first, with their ranges', () => {
    const yours = base.replace('Intro.', 'Intro, mine.')
    const theirs = base.replace('Intro.', 'Intro, theirs.')
    const r = merge3(base, theirs, yours)
    const lines = r.text.split('\n')
    expect(lines.slice(0, 5)).toEqual(['# Harbor', '', 'Intro, mine.', 'Intro, theirs.', ''])
    expect(r.kept).toEqual([{ yours: { from: 2, to: 3 }, theirs: { from: 3, to: 4 } }])
    // Nothing else moved.
    expect(lines.slice(4).join('\n')).toBe(base.split('\n').slice(3).join('\n'))
  })

  it('overlap and a separate change in one merge: the change lands, the overlap keeps both', () => {
    const yours = base.replace('Intro.', 'Intro, mine.').replace('1. Clone.', '1. Clone it.')
    const theirs = base.replace('Intro.', 'Intro, theirs.').replace('- [ ] Add', '- [x] Add')
    const r = merge3(base, theirs, yours)
    expect(r.text).toContain('1. Clone it.')
    expect(r.text).toContain('- [x] Add the checker')
    expect(r.kept).toHaveLength(1)
  })

  it('the same change on both sides is taken once', () => {
    const both = base.replace('Intro.', 'Intro, same.')
    expect(merge3(base, both, both)).toEqual({ text: both, kept: [] })
  })

  it('an unchanged side takes the other side whole', () => {
    const yours = base.replace('Intro.', 'Mine.')
    expect(merge3(base, base, yours)).toEqual({ text: yours, kept: [] })
    expect(merge3(base, yours, base)).toEqual({ text: yours, kept: [] })
  })

  it('an empty base (a new file both made): both kept', () => {
    const r = merge3('', L('theirs line', ''), L('my line', ''))
    expect(r.text).toBe(L('my line', 'theirs line', ''))
    expect(r.kept).toEqual([{ yours: { from: 0, to: 1 }, theirs: { from: 1, to: 2 } }])
  })

  it('an empty base and the same text on both sides: once', () => {
    expect(merge3('', 'same\n', 'same\n')).toEqual({ text: 'same\n', kept: [] })
  })

  it('their deletion and your edit of the same line: nothing dropped', () => {
    const yours = base.replace('Intro.', 'Intro, mine.')
    const theirs = base.replace('Intro.\n', '')
    const r = merge3(base, theirs, yours)
    expect(r.text).toContain('Intro, mine.')
    expect(r.kept).toHaveLength(1)
  })

  it('an insertion at the edge of the other side’s change is not a clash', () => {
    const yours = base.replace('## Setup', 'A new line.\n## Setup')
    const theirs = base.replace('## Setup', '## Set up')
    const r = merge3(base, theirs, yours)
    expect(r.text).toContain('A new line.\n## Set up')
    expect(r.kept).toEqual([])
  })

  it('a many-line overlap keeps both blocks whole', () => {
    const yours = base.replace('## Setup\n\n1. Clone.', '## Setup\n\n1. Clone.\n2. Mine.\n3. Mine too.')
    const theirs = base.replace('## Setup\n\n1. Clone.', '## Setup\n\n1. Clone.\n2. Theirs.')
    const r = merge3(base, theirs, yours)
    const lines = r.text.split('\n')
    const [pair] = r.kept
    expect(lines.slice(pair.yours.from, pair.yours.to)).toEqual(['2. Mine.', '3. Mine too.'])
    expect(lines.slice(pair.theirs.from, pair.theirs.to)).toEqual(['2. Theirs.'])
  })
})
