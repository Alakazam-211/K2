// Off-screen people on a live text (build-editors.md E4b; editors-notes.md v3
// Spacing): someone whose caret is above or below the view gets a small chip
// in a 34px band at that edge, centred over the column; it jumps to them.
// Pinned here:
//   - above, below or in view, with an 8px allowance at each edge;
//   - you, away people, the person you follow and people with no caret get
//     no chip;
//   - the band centres its chips over the column (or the editor);
//   - the jump scrolls their line to a third of the way down the view.

import { describe, expect, it } from 'vitest'
import { BAND_H, bandCentre, jumpTop, offscreen } from './offscreen'

const view = { top: 1000, bottom: 1600 }
const at = (id: string, top: number, o: Record<string, unknown> = {}) => ({ id, top, bottom: top + 24, ...o })

describe('offscreen', () => {
  it('sorts others into above, below and in view', () => {
    const r = offscreen([at('atlas', 400), at('sam', 2400), at('brook', 1200), at('quill', 990)], view, { following: null })
    expect(r).toEqual({ above: ['atlas'], below: ['sam'] })
  })

  it('leaves out you, away people, whom you follow and people with no caret', () => {
    const r = offscreen([at('me', 100, { self: true }), at('brook', 100, { away: true }), at('atlas', 3000), { id: 'sam', top: null, bottom: null }], view, { following: 'atlas' })
    expect(r).toEqual({ above: [], below: [] })
  })

  it('keeps the order people came in', () => {
    expect(offscreen([at('sam', 3000), at('atlas', 2000)], view, { following: null }).below).toEqual(['sam', 'atlas'])
  })
})

describe('the band', () => {
  it('is 34px and centres its chips over the column', () => {
    expect(BAND_H).toBe(34)
    expect(bandCentre({ left: 300, right: 780 }, 1100)).toBe(540)
    expect(bandCentre(null, 1100)).toBe(550)
  })
  it('jumps so their line sits a third of the way down', () => {
    expect(jumpTop(2400, 600)).toBe(2200)
    expect(jumpTop(50, 600)).toBe(0)
  })
})
