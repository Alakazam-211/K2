// Holds placed by the version their lines were computed at (K2 Cores: a
// claim's lines are at the event's `version`). Pinned here:
//   - at the text's own version: the lines directly, or a byte range (when
//     the kit sends one) read as UTF-8, a multi-byte line included;
//   - from an older version: placed in the text as it was then and mapped
//     forward through the changes since (an insert above moves it down);
//   - from a newer version than the text: held, then placed once the text
//     gets there;
//   - a version that has fallen out of the ring: the lines in the text as it
//     is now, with a debug note.

import { describe, expect, it, vi } from 'vitest'
import { ChangeSet, Text } from '@codemirror/state'
import { RevRing, resolveClaim, utf8ToPos } from './claim-resolve'

const doc = (s: string): Text => Text.of(s.split('\n'))
const slice = (d: Text, r: ReturnType<typeof resolveClaim>): string | null => (r && r !== 'hold' ? d.sliceString(r.from, r.to) : null)
const BASE = ['# Doc', '', '## Setup', 'one', 'two', '', '## Next', 'three'].join('\n')

/** Apply `insert` at `pos` to `d`, recording it in the ring as the change to `after`. */
function change(ring: RevRing, d: Text, pos: number, insert: string, before: number, after: number): Text {
  const cs = ChangeSet.of({ from: pos, insert }, d.length)
  ring.record(before, after, cs, cs.invert(d))
  return cs.apply(d)
}

describe('utf8ToPos', () => {
  it('reads byte offsets as UTF-8, not UTF-16', () => {
    const d = doc('é😀x\nnext')
    // é = 2 bytes, 😀 = 4 bytes (2 UTF-16 units), x = 1 byte
    expect(utf8ToPos(d, 0)).toBe(0)
    expect(utf8ToPos(d, 2)).toBe(1)
    expect(utf8ToPos(d, 6)).toBe(3)
    expect(utf8ToPos(d, 7)).toBe(4)
    expect(utf8ToPos(d, 8)).toBe(5)
  })
})

describe('resolveClaim', () => {
  it('at the text’s version: lines directly, or a UTF-8 byte range', () => {
    const ring = new RevRing(64, 5)
    const d = doc('# Doc\n## Café 😀\nmore\nend')
    expect(resolveClaim({ at: 5, first: 2, last: 3 }, d, ring)).toEqual({ from: 6, to: 6 + '## Café 😀'.length + 1 + 'more'.length })
    const start = new TextEncoder().encode('# Doc\n').length
    const end = new TextEncoder().encode('# Doc\n## Café 😀').length
    expect(resolveClaim({ at: 5, first: 9, last: 9, bytes: { start, end } }, d, ring)).toEqual({ from: 6, to: 6 + '## Café 😀'.length })
  })

  it('from an older version: mapped forward across an insert above it', () => {
    const ring = new RevRing(64, 3)
    let d = doc(BASE)
    d = change(ring, d, 0, 'Intro line.\n', 3, 4) // a line above, at version 4
    const r = resolveClaim({ at: 3, first: 3, last: 5 }, d, ring)
    expect(slice(d, r)).toBe('## Setup\none\ntwo')
  })

  it('from a newer version than the text: held, then placed once the text gets there', () => {
    const ring = new RevRing(64, 3)
    let d = doc(BASE)
    expect(resolveClaim({ at: 4, first: 4, last: 6 }, d, ring)).toBe('hold')
    d = change(ring, d, 0, 'Intro line.\n', 3, 4)
    const r = resolveClaim({ at: 4, first: 4, last: 6 }, d, ring)
    expect(slice(d, r)).toBe('## Setup\none\ntwo')
  })

  it('a version fallen out of the ring: the lines as the text is now, with a debug note', () => {
    const debug = vi.spyOn(console, 'debug').mockImplementation(() => undefined)
    const ring = new RevRing(2, 1)
    let d = doc(BASE)
    d = change(ring, d, 0, 'a\n', 1, 2)
    d = change(ring, d, 0, 'b\n', 2, 3)
    d = change(ring, d, 0, 'c\n', 3, 4)
    const r = resolveClaim({ at: 1, first: 6, last: 8 }, d, ring)
    expect(slice(d, r)).toBe('## Setup\none\ntwo')
    expect(debug).toHaveBeenCalledWith(expect.stringContaining('fell out'))
    debug.mockRestore()
  })
})
