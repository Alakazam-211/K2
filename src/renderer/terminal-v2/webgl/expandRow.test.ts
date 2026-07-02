import { describe, it, expect } from 'vitest'
import { DIM_ALPHA, expandRow } from './expandRow'
import type { WireCellRun } from '../gridWire'

const THEME = { fg: 0xe0e0e0, bg: 0x0a0a0a }

function run(text: string, over: Partial<WireCellRun> = {}): WireCellRun {
  return {
    text,
    fg: null,
    bg: null,
    bold: false,
    italic: false,
    underline: false,
    inverse: false,
    dim: false,
    strikeout: false,
    ...over,
  }
}

describe('expandRow — glyph expansion', () => {
  it('emits one glyph per non-blank code point at its column', () => {
    const er = expandRow([run('ab c')], THEME)
    expect(er.glyphs).toEqual([
      { col: 0, widthCells: 1, text: 'a', bold: false, italic: false, fg: THEME.fg, alpha: 1 },
      { col: 1, widthCells: 1, text: 'b', bold: false, italic: false, fg: THEME.fg, alpha: 1 },
      { col: 3, widthCells: 1, text: 'c', bold: false, italic: false, fg: THEME.fg, alpha: 1 },
    ])
    expect(er.bgSpans).toEqual([])
    expect(er.decoSpans).toEqual([])
  })

  it('carries bold/italic and resolved explicit fg', () => {
    const er = expandRow([run('x', { bold: true, italic: true, fg: 0xff0000 })], THEME)
    expect(er.glyphs).toEqual([
      { col: 0, widthCells: 1, text: 'x', bold: true, italic: true, fg: 0xff0000, alpha: 1 },
    ])
  })

  it('dim multiplies alpha (matching the DOM opacity value)', () => {
    const er = expandRow([run('d', { dim: true })], THEME)
    expect(er.glyphs[0].alpha).toBe(DIM_ALPHA)
  })

  it('inverse with null colors swaps THEME defaults (TUI cursor case)', () => {
    const er = expandRow([run('P', { inverse: true })], THEME)
    expect(er.glyphs[0].fg).toBe(THEME.bg)
    expect(er.bgSpans).toEqual([{ col: 0, width: 1, color: THEME.fg }])
  })

  it('inverse resolves explicit colors before swapping', () => {
    const er = expandRow(
      [run('x', { inverse: true, fg: 0x112233, bg: 0x445566 })],
      THEME,
    )
    expect(er.glyphs[0].fg).toBe(0x445566)
    expect(er.bgSpans).toEqual([{ col: 0, width: 1, color: 0x112233 }])
  })

  it('wide chars occupy two columns and advance the cursor by cols', () => {
    // '日本' = 2 chars, 4 columns; annotated run carries cols=4.
    const er = expandRow([run('日本', { cols: 4 }), run('x')], THEME)
    expect(er.glyphs).toEqual([
      expect.objectContaining({ col: 0, widthCells: 2, text: '日' }),
      expect.objectContaining({ col: 2, widthCells: 2, text: '本' }),
      expect.objectContaining({ col: 4, widthCells: 1, text: 'x' }),
    ])
  })

  it('zero-width followers fold into the preceding cluster', () => {
    // e + combining acute (U+0301): 2 code points, 1 column.
    const er = expandRow([run('e\u0301', { cols: 1 }), run('z')], THEME)
    expect(er.glyphs).toEqual([
      expect.objectContaining({ col: 0, widthCells: 1, text: 'e\u0301' }),
      expect.objectContaining({ col: 1, widthCells: 1, text: 'z' }),
    ])
  })

  it('trusts the wire span over the classifier for total run width', () => {
    // Pretend an exotic cp the classifier calls narrow is actually
    // wide per the wire: run of 1 char annotated cols=2.
    const er = expandRow([run('↔', { cols: 2 }), run('x')], THEME)
    expect(er.glyphs[1]).toEqual(
      expect.objectContaining({ col: 2, text: 'x' }),
    )
  })

  it('unannotated runs are strictly one column per char', () => {
    // Legacy-daemon contract: no cols field ⇒ no width classification
    // even for CJK content.
    const er = expandRow([run('日x')], THEME)
    expect(er.glyphs).toEqual([
      expect.objectContaining({ col: 0, widthCells: 1, text: '日' }),
      expect.objectContaining({ col: 1, widthCells: 1, text: 'x' }),
    ])
  })
})

describe('expandRow — background spans', () => {
  it('emits nothing for default-bg cells (viewport clear covers them)', () => {
    const er = expandRow([run('abc')], THEME)
    expect(er.bgSpans).toEqual([])
  })

  it('merges adjacent runs with the same resolved bg', () => {
    const er = expandRow(
      [run('ab', { bg: 0x222222 }), run('cd', { bg: 0x222222, bold: true })],
      THEME,
    )
    expect(er.bgSpans).toEqual([{ col: 0, width: 4, color: 0x222222 }])
  })

  it('breaks spans on bg color change and on gaps', () => {
    const er = expandRow(
      [run('a', { bg: 0x111111 }), run('b'), run('c', { bg: 0x111111 })],
      THEME,
    )
    expect(er.bgSpans).toEqual([
      { col: 0, width: 1, color: 0x111111 },
      { col: 2, width: 1, color: 0x111111 },
    ])
  })

  it('bg span width follows the wire cols for wide runs', () => {
    const er = expandRow([run('日', { cols: 2, bg: 0x333333 })], THEME)
    expect(er.bgSpans).toEqual([{ col: 0, width: 2, color: 0x333333 }])
  })
})

describe('expandRow — decorations', () => {
  it('emits underline and strikeout spans with resolved fg + dim alpha', () => {
    const er = expandRow(
      [run('ab', { underline: true, strikeout: true, dim: true, fg: 0xabcdef })],
      THEME,
    )
    expect(er.decoSpans).toEqual([
      { col: 0, width: 2, color: 0xabcdef, alpha: DIM_ALPHA, kind: 'underline' },
      { col: 0, width: 2, color: 0xabcdef, alpha: DIM_ALPHA, kind: 'strikeout' },
    ])
  })

  it('underline on spaces produces a deco span but no glyphs', () => {
    const er = expandRow([run('  ', { underline: true })], THEME)
    expect(er.glyphs).toEqual([])
    expect(er.decoSpans).toEqual([
      { col: 0, width: 2, color: THEME.fg, alpha: 1, kind: 'underline' },
    ])
  })
})
