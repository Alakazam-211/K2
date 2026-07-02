import { describe, it, expect } from 'vitest'
import {
  buildCopyText,
  colToTextEnd,
  colToTextStart,
  copySelectionText,
  type CopyGrid,
} from './copyText'
import type { WireCellRun } from './gridWire'

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

function snap(rows: WireCellRun[][], scrollback: WireCellRun[][] = []): CopyGrid {
  return { grid: rows, scrollback }
}

describe('buildCopyText — DOM copy semantics (text offsets)', () => {
  it('single-row slice', () => {
    const s = snap([[run('hello world')]])
    expect(buildCopyText(s, 0, 6, 0, 11)).toBe('world')
  })

  it('trims per-line trailing whitespace', () => {
    const s = snap([[run('abc   ')], [run('def')]])
    expect(buildCopyText(s, 0, 0, 1, 3)).toBe('abc\ndef')
  })

  it('empty rows contribute a bare newline', () => {
    const s = snap([[run('a')], [], [run('b')]])
    expect(buildCopyText(s, 0, 0, 2, 1)).toBe('a\n\nb')
  })

  it('soft-wrapped rows join without newline or trim', () => {
    const s = snap([
      [run('long-command-that-', { wrapped: true })],
      [run('continues')],
    ])
    expect(buildCopyText(s, 0, 0, 1, 9)).toBe('long-command-that-continues')
  })

  it('wrapped flag on the LAST selected row does not join past the end', () => {
    const s = snap([[run('abc', { wrapped: true })]])
    expect(buildCopyText(s, 0, 0, 0, 3)).toBe('abc')
  })

  it('spans scrollback into the live grid', () => {
    const s = snap([[run('grid')]], [[run('old')]])
    expect(buildCopyText(s, 0, 0, 1, 4)).toBe('old\ngrid')
  })

  it('degenerate reversed offsets on one row yield an empty segment', () => {
    const s = snap([[run('abc')]])
    expect(buildCopyText(s, 0, 2, 0, 1)).toBe('')
  })
})

describe('column ↔ text boundary conversion (wide chars)', () => {
  // '日本' = 4 columns / 2 chars, then 'ab'.
  const row = [run('日本', { cols: 4 }), run('ab')]

  it('start boundary on a cluster start', () => {
    expect(colToTextStart(row, 0)).toBe(0)
    expect(colToTextStart(row, 2)).toBe(1)
    expect(colToTextStart(row, 4)).toBe(2)
  })

  it('start boundary mid-wide-char includes the whole char', () => {
    expect(colToTextStart(row, 1)).toBe(0)
    expect(colToTextStart(row, 3)).toBe(1)
  })

  it('start boundary past content maps to text length', () => {
    expect(colToTextStart(row, 99)).toBe(4)
  })

  it('end boundary includes a half-covered wide char', () => {
    // endCol 1 lands inside 日 → include it.
    expect(colToTextEnd(row, 1)).toBe(1)
    expect(colToTextEnd(row, 2)).toBe(1)
    expect(colToTextEnd(row, 3)).toBe(2)
    expect(colToTextEnd(row, 6)).toBe(4)
  })

  it('end boundary at 0 selects nothing', () => {
    expect(colToTextEnd(row, 0)).toBe(0)
  })
})

describe('copySelectionText — grid-coordinate selection', () => {
  it('slices CJK rows by columns', () => {
    const s = snap([[run('日本語', { cols: 6 })]])
    // Columns [2, 6) = 本語.
    expect(
      copySelectionText(s, { startAbs: 0, startCol: 2, endAbs: 0, endCol: 6 }),
    ).toBe('本語')
  })

  it('half-covered wide chars are included at both boundaries', () => {
    const s = snap([[run('日本', { cols: 4 })]])
    // Start mid-日, end mid-本 → both chars.
    expect(
      copySelectionText(s, { startAbs: 0, startCol: 1, endAbs: 0, endCol: 3 }),
    ).toBe('日本')
  })

  it('multi-row selection matches the DOM path byte-for-byte', () => {
    const s = snap([
      [run('first line   ')],
      [run('wrapped-', { wrapped: true })],
      [run('tail')],
    ])
    const viaCols = copySelectionText(s, {
      startAbs: 0,
      startCol: 0,
      endAbs: 2,
      endCol: 4,
    })
    const viaOffsets = buildCopyText(s, 0, 0, 2, 4)
    expect(viaCols).toBe(viaOffsets)
    expect(viaCols).toBe('first line\nwrapped-tail')
  })

  it('full-line selection (line mode uses endCol = cols) trims padding', () => {
    const s = snap([[run('ls -la      ')]])
    expect(
      copySelectionText(s, { startAbs: 0, startCol: 0, endAbs: 0, endCol: 80 }),
    ).toBe('ls -la')
  })
})
