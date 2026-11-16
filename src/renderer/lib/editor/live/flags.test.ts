// Name flags on a live text (build-editors.md E4b; editors-notes.md §8, v3.2b).
// Pinned here:
//   - a flag sits GAP px right of the reading column, at its caret's line,
//     with a hairline from the caret to the flag along the line's top;
//   - with no column (Source, code) it sits right of the last glyph on the
//     lines its height spans, so it never covers a glyph;
//   - two flags at one height sit side by side;
//   - a tight margin shrinks a flag to its initial; tighter still, no flag;
//   - a caret half out of view gets no flag;
//   - the margin rule: across many random layouts, no flag touches a glyph
//     and every flag sits wholly inside the clip box.

import { describe, expect, it } from 'vitest'
import { FLAG_EDGE, FLAG_GAP, FLAG_H, FLAG_MINI_W, TAG_H, TAG_MS, TagClock, placeFlags, touches, type FlagCaret, type FlagRoom } from './flags'
import { assignSlots, caretSlotCss } from './presence-slots'

interface Glyph {
  left: number
  top: number
  right: number
  bottom: number
}

/** A room over `glyphs`: textRight is the rightmost glyph overlapping the band. */
function room(glyphs: Glyph[], o: { column?: number | null; right?: number; top?: number; bottom?: number } = {}): FlagRoom {
  return {
    column: o.column === undefined ? 600 : o.column,
    clip: { left: 0, right: o.right ?? 1000, top: o.top ?? 0, bottom: o.bottom ?? 800 },
    textRight: (top, bottom) => Math.max(-Infinity, ...glyphs.filter((g) => g.top < bottom && top < g.bottom).map((g) => g.right)),
    width: (label, agent) => label.length * 7 + 12 + (agent ? 16 : 0),
  }
}
const caret = (id: string, x: number, line: number, o: Partial<FlagCaret> = {}): FlagCaret => ({ id, name: id[0].toUpperCase() + id.slice(1), agent: false, x, top: line * 24, bottom: line * 24 + 24, ...o })
const box = (f: { left: number; top: number; width: number }) => ({ left: f.left, top: f.top, right: f.left + f.width, bottom: f.top + FLAG_H })

describe('placeFlags', () => {
  it('puts the flag right of the column at the caret line, with a hairline back to the caret', () => {
    const [f] = placeFlags([caret('atlas', 200, 4, { agent: true })], room([]))
    expect(f.left).toBe(600 + FLAG_GAP)
    expect(f.top).toBe(4 * 24 + 12 - FLAG_H / 2)
    expect(f.label).toBe('Atlas')
    expect(f.mini).toBe(false)
    expect(f.width).toBe(5 * 7 + 12 + 16)
    expect(f.line).toEqual({ x0: 201, x1: 600 + FLAG_GAP, y: 96 })
  })

  it('with no column, clears the glyphs on the lines its height spans', () => {
    const glyphs = [
      { left: 40, top: 72, right: 420, bottom: 96 }, // the line above, longer
      { left: 40, top: 96, right: 300, bottom: 120 }, // the caret's line
    ]
    const [f] = placeFlags([caret('sam', 120, 4)], room(glyphs, { column: null }))
    // The flag (96..116 centred on the line) touches only the caret's line: clear of 300.
    expect(f.left).toBe(300 + FLAG_GAP)
    for (const g of glyphs) expect(touches(box(f), g)).toBe(false)
  })

  it('sets two flags at one height side by side', () => {
    const [a, b] = placeFlags([caret('atlas', 100, 3), caret('sam', 220, 3)], room([]))
    expect(b.left).toBeGreaterThanOrEqual(a.left + a.width + 4)
    expect(touches(box(a), box(b))).toBe(false)
  })

  it('shrinks to the initial when the margin is tight', () => {
    const tight = placeFlags([caret('brook', 100, 2)], room([], { right: 600 + FLAG_GAP + FLAG_MINI_W + FLAG_EDGE + 10 }))
    expect(tight).toHaveLength(1)
    expect(tight[0]).toMatchObject({ label: 'B', mini: true, tag: false, width: FLAG_MINI_W, name: 'Brook' })
  })

  it('with no room in the margin even for the initial, falls back to a name tag just above the caret line', () => {
    const [t] = placeFlags([caret('brook', 100, 2, { agent: true })], room([], { right: 600 + FLAG_GAP + 4 }))
    expect(t).toMatchObject({ tag: true, mini: false, label: 'Brook', left: 100, top: 2 * 24 - TAG_H - 1 })
    expect(t.line).toBeNull()
    // Near the clip's right edge it shifts left to stay whole; on the first visible line it sits under the line.
    const [r] = placeFlags([caret('brook', 590, 2)], room([], { right: 610 }))
    expect(r.left + r.width).toBeLessThanOrEqual(610 - FLAG_EDGE)
    const [b] = placeFlags([caret('brook', 100, 0)], room([], { right: 604 }))
    expect(b.top).toBe(24 + 1)
  })

  it('gives no flag to a caret half out of view', () => {
    expect(placeFlags([caret('atlas', 100, 0)], room([], { top: 10 }))).toEqual([])
    expect(placeFlags([caret('atlas', 100, 33)], room([], { bottom: 33 * 24 + 10 }))).toEqual([])
  })

  it('never touches a glyph, and stays inside the clip box (the margin rule, 400 layouts)', () => {
    let seed = 7
    const rnd = (): number => {
      seed = (seed * 1103515245 + 12345) % 2147483648
      return seed / 2147483648
    }
    for (let n = 0; n < 400; n++) {
      const column = rnd() < 0.5 ? null : 380 + Math.round(rnd() * 200)
      const right = (column ?? 400) + Math.round(rnd() * 260)
      const glyphs: Glyph[] = []
      for (let l = 0; l < 30; l++) {
        const end = column ?? right
        glyphs.push({ left: 40, top: l * 24 + 4, right: 40 + Math.round(rnd() * (end - 40)), bottom: l * 24 + 20 })
      }
      const r = room(glyphs, { column, right, top: Math.round(rnd() * 40), bottom: 700 + Math.round(rnd() * 40) })
      const carets = Array.from({ length: 4 }, (_, i) => {
        const l = Math.floor(rnd() * 30)
        const g = glyphs[l]
        return caret(['atlas', 'sam', 'brook', 'quill'][i], g.left + Math.round(rnd() * (g.right - g.left)), l, { agent: i === 0 })
      })
      for (const f of placeFlags(carets, r)) {
        // The caret tag is a passing label over the text, in fallback only: the margin rule skips it.
        if (f.tag) continue
        const b = box(f)
        for (const g of glyphs) expect(touches(b, g), `layout ${n}: ${f.id} on a glyph`).toBe(false)
        expect(b.right).toBeLessThanOrEqual(r.clip.right - FLAG_EDGE)
        expect(b.top).toBeGreaterThanOrEqual(r.clip.top)
        expect(b.bottom).toBeLessThanOrEqual(r.clip.bottom)
      }
    }
  })
})

describe('the caret tag clock', () => {
  it('shows a tag for 2 seconds after the caret moves or types, then hides it', () => {
    const c = new TagClock()
    c.observe('atlas', 120, 1000)
    expect(c.visible('atlas', 1000)).toBe(true)
    expect(c.visible('atlas', 1000 + TAG_MS - 1)).toBe(true)
    expect(c.visible('atlas', 1000 + TAG_MS)).toBe(false)
    // The same place again isn't a move; a new place is.
    c.observe('atlas', 120, 2500)
    expect(c.visible('atlas', 3100)).toBe(false)
    c.observe('atlas', 121, 3200)
    expect(c.visible('atlas', 3300)).toBe(true)
    expect(c.nextExpiry(3300)).toBe(3200 + TAG_MS)
    expect(c.nextExpiry(3200 + TAG_MS)).toBeNull()
  })

  it('keeps the tag while the pointer is over that caret or its selection', () => {
    const c = new TagClock()
    c.observe('sam', 50, 0)
    c.hover('sam')
    expect(c.visible('sam', 10_000)).toBe(true)
    c.hover(null)
    expect(c.visible('sam', 10_000)).toBe(false)
  })
})

describe('presence slots', () => {
  it('gives each other person a slot by first sight, keeps it, and frees it when they leave', () => {
    const a = assignSlots(new Map(), [{ id: 'me', self: true }, { id: 'atlas' }, { id: 'sam' }, { id: 'brook' }])
    expect([...a]).toEqual([
      ['atlas', 1],
      ['sam', 2],
      ['brook', 3],
    ])
    const b = assignSlots(a, [{ id: 'me', self: true }, { id: 'brook' }, { id: 'quill' }, { id: 'sam' }])
    expect(b.get('brook')).toBe(3)
    expect(b.get('sam')).toBe(2)
    expect(b.get('quill')).toBe(1)
  })

  it('maps each caret key (handle, or handle#client) to its token', () => {
    const css = caretSlotCss(new Map([['agent:"x"', 2]]))
    expect(css).toContain('[data-mpc-caret="agent:\\"x\\""]')
    expect(css).toContain('[data-mpc-caret^="agent:\\"x\\"#"]')
    expect(css).toContain('--mpc-user: var(--color-presence-2) !important')
  })
})
