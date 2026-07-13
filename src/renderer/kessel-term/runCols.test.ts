import { describe, expect, it } from 'vitest'

import {
  colToTextIndex,
  isEmojiCp,
  isWideCp,
  rowColSpan,
  runCells,
  runColOffsets,
  runColSpan,
  runNeedsPerCharCells,
  sliceRowByCols,
  type ColRun,
} from './runCols'

// The canonical wide-char row from the wire contract: "ab日本cd" is
// 6 chars spanning 8 terminal columns (each CJK char is 2 columns),
// so the daemon stamps cols: 8 on the (single-style) run.
const wideRow: ColRun[] = [{ text: 'ab日本cd', cols: 8 }]

// Same content split across runs (style boundary mid-row): the ASCII
// runs carry no cols, the CJK run does.
const splitRow: ColRun[] = [
  { text: 'ab' },
  { text: '日本', cols: 4 },
  { text: 'cd' },
]

describe('runColSpan / rowColSpan', () => {
  it('uses the explicit wire cols when present', () => {
    expect(runColSpan({ text: '日本', cols: 4 })).toBe(4)
  })

  it('falls back to code-point count when absent', () => {
    expect(runColSpan({ text: 'abc' })).toBe(3)
    // Non-BMP single code point counts once, not twice (UTF-16 pair).
    expect(runColSpan({ text: '\u{1d400}x' })).toBe(2)
  })

  it('sums runs across a row', () => {
    expect(rowColSpan(wideRow)).toBe(8)
    expect(rowColSpan(splitRow)).toBe(8)
  })
})

describe('runColOffsets', () => {
  it('returns the prefix sums of run column spans (mixed cols runs)', () => {
    // ab | 日本 (4 cols) | cd → starts at 0, 2, 6.
    expect(runColOffsets(splitRow)).toEqual([0, 2, 6])
  })

  it('anchors runs after a wide-char run at the wire span, not the char count', () => {
    // Single annotated run: whatever follows starts at column 8
    // (the wire's span), not 6 (the char count).
    expect(runColOffsets([...wideRow, { text: 'x' }])).toEqual([0, 8])
  })

  it('uses code-point count for unannotated runs (braille art carries no cols)', () => {
    // Braille cells (U+2800–U+28FF) are single-width, one column per
    // code point, so the daemon omits `cols` — the run after grok's
    // logo art must start at exactly the art's column span even
    // though the FONT may advance those glyphs at a different width.
    const braille = '⠋⠙⠀⠀⠈' // includes invisible BRAILLE BLANK padding
    expect(runColOffsets([{ text: braille }, { text: 'MENU' }])).toEqual([
      0, 5,
    ])
  })

  it('treats zero-span and empty-text runs as zero columns', () => {
    expect(
      runColOffsets([
        { text: 'a' },
        { text: '́', cols: 0 }, // isolated combining accent
        { text: '' },
        { text: 'b' },
      ]),
    ).toEqual([0, 1, 1, 1])
  })

  it('returns an empty array for an empty row', () => {
    expect(runColOffsets([])).toEqual([])
  })
})

describe('colToTextIndex', () => {
  it('is the identity for plain ASCII rows', () => {
    const row: ColRun[] = [{ text: 'hello' }]
    for (let col = 0; col < 5; col++) {
      expect(colToTextIndex(row, col)).toBe(col)
    }
  })

  it('maps both halves of a double-width char to the char', () => {
    // Columns: a=0 b=1 日=2,3 本=4,5 c=6 d=7.
    expect(colToTextIndex(wideRow, 2)).toBe(2) // 日 left half
    expect(colToTextIndex(wideRow, 3)).toBe(2) // 日 right half
    expect(colToTextIndex(wideRow, 4)).toBe(3) // 本 — the spec case
    expect(colToTextIndex(wideRow, 5)).toBe(3)
    expect(colToTextIndex(wideRow, 6)).toBe(4) // c
    expect(colToTextIndex(wideRow, 7)).toBe(5) // d
  })

  it('handles runs split at style boundaries identically', () => {
    expect(colToTextIndex(splitRow, 4)).toBe(3)
    expect(colToTextIndex(splitRow, 7)).toBe(5)
  })

  it('maps columns past the content to the text length', () => {
    expect(colToTextIndex(wideRow, 8)).toBe(6)
    expect(colToTextIndex(wideRow, 100)).toBe(6)
    expect(colToTextIndex([], 3)).toBe(0)
  })

  it('returns UTF-16 indices for non-BMP wide chars (emoji)', () => {
    // 😀 is one code point, two UTF-16 units, two columns.
    // Columns: o=0 k=1 😀=2,3 !=4. Text: 'ok😀!' (utf16 len 5).
    const row: ColRun[] = [{ text: 'ok😀!', cols: 5 }]
    expect(colToTextIndex(row, 2)).toBe(2)
    expect(colToTextIndex(row, 3)).toBe(2)
    expect(colToTextIndex(row, 4)).toBe(4) // '!' AFTER the surrogate pair
  })

  it('never lands inside a zero-width follower', () => {
    // 'e' + combining acute: 2 chars over 1 column (cols: 2 on a
    // 3-char text: e, U+0301, x).
    const row: ColRun[] = [{ text: 'e\u0301x', cols: 2 }]
    expect(colToTextIndex(row, 0)).toBe(0)
    expect(colToTextIndex(row, 1)).toBe(2) // x sits after the accent
  })
})

describe('sliceRowByCols', () => {
  it('slices the spec case: cols [2,6) of "ab日本cd" is "日本"', () => {
    expect(sliceRowByCols(wideRow, 2, 6)).toBe('日本')
    expect(sliceRowByCols(splitRow, 2, 6)).toBe('日本')
  })

  it('includes a wide char when the range covers either half', () => {
    expect(sliceRowByCols(wideRow, 3, 5)).toBe('日本')
    expect(sliceRowByCols(wideRow, 0, 3)).toBe('ab日')
  })

  it('is plain slicing for ASCII rows', () => {
    expect(sliceRowByCols([{ text: 'hello' }], 1, 4)).toBe('ell')
  })

  it('carries zero-width followers with their base char', () => {
    const row: ColRun[] = [{ text: 'e\u0301x', cols: 2 }]
    expect(sliceRowByCols(row, 0, 1)).toBe('e\u0301')
    expect(sliceRowByCols(row, 1, 2)).toBe('x')
  })

  it('returns empty for degenerate ranges', () => {
    expect(sliceRowByCols(wideRow, 4, 4)).toBe('')
    expect(sliceRowByCols(wideRow, 6, 2)).toBe('')
    expect(sliceRowByCols([], 0, 5)).toBe('')
  })
})

describe('runNeedsPerCharCells', () => {
  it('is false for printable ASCII (the fast path) and for the empty string', () => {
    expect(runNeedsPerCharCells('')).toBe(false)
    expect(runNeedsPerCharCells('ls -la | grep "foo" ~!@#$%^&*()')).toBe(false)
  })

  it('is false for plain non-ASCII text whose advance is trustworthy', () => {
    expect(runNeedsPerCharCells('café naïve')).toBe(false) // Latin-1 accents
    expect(runNeedsPerCharCells('Ω λ π')).toBe(false) // Greek
    expect(runNeedsPerCharCells('привет')).toBe(false) // Cyrillic
    expect(runNeedsPerCharCells('—…‘’“”')).toBe(false) // punctuation
  })

  it('is true for braille patterns (U+2800–28FF), including BRAILLE BLANK', () => {
    expect(runNeedsPerCharCells('⠋⠙⠚')).toBe(true)
    expect(runNeedsPerCharCells('⠀')).toBe(true)
    expect(runNeedsPerCharCells('mixed ⣿ ascii')).toBe(true)
  })

  it('is true for box drawing, block elements and geometric shapes', () => {
    expect(runNeedsPerCharCells('┌─┐')).toBe(true) // box drawing
    expect(runNeedsPerCharCells('▀▄█▓')).toBe(true) // block elements
    expect(runNeedsPerCharCells('■▶◆')).toBe(true) // geometric shapes
  })

  it('is true for private-use (powerline / Nerd Font) glyphs', () => {
    expect(runNeedsPerCharCells('')).toBe(true) // powerline arrow
    expect(runNeedsPerCharCells('')).toBe(true) // PUA upper bound
  })

  it('is true for wide chars — same set the column math treats as wide', () => {
    expect(runNeedsPerCharCells('日本')).toBe(true) // CJK
    expect(runNeedsPerCharCells('안녕')).toBe(true) // Hangul
    expect(runNeedsPerCharCells('😀')).toBe(true) // non-BMP emoji
  })

  it('is true for legacy-computing sextants (synthetic-geometry glyphs)', () => {
    expect(runNeedsPerCharCells('\u{1FB00}')).toBe(true) // block start
    expect(runNeedsPerCharCells('\u{1FB3B}')).toBe(true) // block end
    expect(runNeedsPerCharCells('\u{1FB3C}')).toBe(false) // past the sextants
  })

  it('is false for zero-width combining content alone (rides its base cell)', () => {
    expect(runNeedsPerCharCells('é')).toBe(false)
    expect(runNeedsPerCharCells('́')).toBe(false)
  })
})

describe('runCells', () => {
  it('splits an unannotated run one column per code point', () => {
    expect(runCells({ text: '⠋⠙⠈' })).toEqual([
      { text: '⠋', col: 0, width: 1 },
      { text: '⠙', col: 1, width: 1 },
      { text: '⠈', col: 2, width: 1 },
    ])
  })

  it('gives wide chars 2-column cells in annotated runs', () => {
    expect(runCells({ text: 'a日b', cols: 4 })).toEqual([
      { text: 'a', col: 0, width: 1 },
      { text: '日', col: 1, width: 2 },
      { text: 'b', col: 3, width: 1 },
    ])
  })

  it('folds zero-width followers into the preceding cell', () => {
    expect(runCells({ text: 'éx', cols: 2 })).toEqual([
      { text: 'é', col: 0, width: 1 },
      { text: 'x', col: 1, width: 1 },
    ])
  })

  it('iterates by code point: a non-BMP emoji is ONE cell of width 2', () => {
    expect(runCells({ text: '😀!', cols: 3 })).toEqual([
      { text: '😀', col: 0, width: 2 },
      { text: '!', col: 2, width: 1 },
    ])
  })

  it('cell columns agree with the run span used for the NEXT run\'s offset', () => {
    const run: ColRun = { text: '日本', cols: 4 }
    const cells = runCells(run)
    const last = cells[cells.length - 1]
    expect(last.col + last.width).toBe(runColSpan(run))
  })
})

describe('BMP emoji width (the squished-✅ fix)', () => {
  it('counts BMP emoji-presentation chars as wide, matching the daemon', () => {
    for (const ch of ['✅', '❌', '⚡', '⭐', '⌚', '✊', '⛔']) {
      expect(isWideCp(ch.codePointAt(0)!)).toBe(true)
    }
    // Non-emoji BMP symbols stay narrow.
    for (const ch of ['✓', '→', '•', '§']) {
      expect(isWideCp(ch.codePointAt(0)!)).toBe(false)
    }
  })

  it('gives ✅ a two-column cell inside an annotated run', () => {
    const cells = runCells({ text: '✅x', cols: 3 })
    expect(cells).toEqual([
      { text: '✅', col: 0, width: 2 },
      { text: 'x', col: 2, width: 1 },
    ])
  })

  it('isEmojiCp covers BMP emoji + plane-1, not text symbols', () => {
    expect(isEmojiCp(0x2705)).toBe(true) // ✅
    expect(isEmojiCp(0x1f600)).toBe(true) // 😀
    expect(isEmojiCp(0x2764)).toBe(true) // ❤
    expect(isEmojiCp(0x2713)).toBe(false) // ✓ text check mark
    expect(isEmojiCp(0x2500)).toBe(false) // ─ box drawing
    expect(isEmojiCp(0x2800)).toBe(false) // braille
  })
})
