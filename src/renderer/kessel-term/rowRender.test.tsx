// @vitest-environment jsdom
//
// Column-anchor contract for the DOM row renderer (rowRender.tsx).
//
// The defect these tests pin: rows used to render as naturally
// flowing inline spans, so any glyph the font falls back for —
// grok's braille-art logo, including its invisible U+2800 BRAILLE
// BLANK padding — advanced at the FALLBACK font's width and pushed
// every later run off its column (~2 chars drift in the wild). The
// fix anchors each run at `left = startCol × cellWidth` computed
// from the MODEL (runColOffsets prefix sums), so a neighbor's
// rendered width can never move a run. jsdom does no glyph layout,
// which is exactly the point: the asserted `left`/`width` come from
// the model, not from flow.

import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { TerminalRow, type RenderRun } from './rowRender'

const CELL_W = 9
const CELL_H = 20

function run(text: string, extra: Partial<RenderRun> = {}): RenderRun {
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
    ...extra,
  }
}

function renderRow(row: RenderRun[], cellWidth = CELL_W, cellHeight = CELL_H) {
  const { container } = render(
    <TerminalRow
      row={row}
      absRow={7}
      defaultFg="rgb(224,224,224)"
      defaultBg="rgb(0,0,0)"
      cellWidth={cellWidth}
      cellHeight={cellHeight}
    />,
  )
  const rowDiv = container.querySelector('[data-abs-row]') as HTMLElement
  expect(rowDiv).not.toBeNull()
  return { rowDiv, spans: Array.from(rowDiv.querySelectorAll('span')) }
}

describe('TerminalRow column anchoring', () => {
  it('anchors the run after braille art at its model column, independent of the braille run\'s natural width', () => {
    // 5 braille code points (incl. U+2800 BRAILLE BLANK padding) = 5
    // columns. Braille is exotic → one span per char, each at its
    // own cell; the following ASCII run must sit at 5 × cellWidth
    // regardless of how any font would advance the braille glyphs.
    const { spans } = renderRow([run('⠋⠙⠀⠀⠈'), run('MENU')])
    expect(spans).toHaveLength(6)
    for (let c = 0; c < 5; c++) {
      expect(spans[c].style.left).toBe(`${c * CELL_W}px`)
      expect(spans[c].style.width).toBe(`${CELL_W}px`)
    }
    expect(spans[5].style.left).toBe(`${5 * CELL_W}px`)
    expect(spans[5].style.width).toBe(`${4 * CELL_W}px`)
    expect(spans[5].textContent).toBe('MENU')
  })

  it('positions every run absolutely with overflow clipped to its cell rect', () => {
    const { spans } = renderRow([run('ab'), run('cd')])
    for (const s of spans) {
      expect(s.style.position).toBe('absolute')
      expect(s.style.overflow).toBe('hidden')
      expect(s.style.whiteSpace).toBe('pre')
    }
  })

  it('advances by the wire cols span after a wide-char run, not the char count', () => {
    // 日本 = 2 chars, 4 columns. Wide chars are exotic → one span
    // per char, each 2 cells wide; the next run starts at column 4.
    const { spans } = renderRow([run('日本', { cols: 4 }), run('!')])
    expect(spans).toHaveLength(3)
    expect(spans[0].style.left).toBe('0px')
    expect(spans[0].style.width).toBe(`${2 * CELL_W}px`)
    expect(spans[1].style.left).toBe(`${2 * CELL_W}px`)
    expect(spans[1].style.width).toBe(`${2 * CELL_W}px`)
    expect(spans[2].style.left).toBe(`${4 * CELL_W}px`)
  })

  it('accumulates offsets across mixed annotated/unannotated runs', () => {
    // ab | 日本(4) | cd → run starts 0, 2, 6; the wide run splits
    // into per-char cells at columns 2 and 4.
    const { spans } = renderRow([
      run('ab'),
      run('日本', { cols: 4 }),
      run('cd'),
    ])
    expect(spans.map((s) => s.style.left)).toEqual([
      '0px',
      `${2 * CELL_W}px`,
      `${4 * CELL_W}px`,
      `${6 * CELL_W}px`,
    ])
    expect(spans.map((s) => s.textContent)).toEqual(['ab', '日', '本', 'cd'])
  })

  it('fixes the row box at one cell height with relative positioning', () => {
    const { rowDiv } = renderRow([run('x')])
    expect(rowDiv.style.position).toBe('relative')
    expect(rowDiv.style.height).toBe(`${CELL_H}px`)
    expect(rowDiv.dataset.absRow).toBe('7')
  })

  it('renders the nbsp placeholder for an empty row and keeps the fixed height', () => {
    const { rowDiv, spans } = renderRow([])
    expect(spans).toHaveLength(0)
    expect(rowDiv.textContent).toBe('\u00a0')
    expect(rowDiv.style.height).toBe(`${CELL_H}px`)
  })

  it('does not clip a zero-span run (isolated combining char rides its base cell)', () => {
    const { spans } = renderRow([run('a'), run('́', { cols: 0 })])
    expect(spans[1].style.left).toBe(`${1 * CELL_W}px`)
    expect(spans[1].style.width).toBe('')
    expect(spans[1].style.overflow).toBe('')
  })

  it('falls back to natural flow before cell metrics are measured', () => {
    const { rowDiv, spans } = renderRow([run('ab'), run('日本', { cols: 4 })], 0, 0)
    expect(rowDiv.style.position).toBe('')
    expect(spans[0].style.position).toBe('')
    // Legacy ch-width pinning still applies to annotated runs.
    expect(spans[1].style.width).toBe('4ch')
    expect(spans[1].style.display).toBe('inline-block')
  })
})

describe('TerminalRow per-character cell anchoring (exotic runs)', () => {
  // The shimmer defect these tests pin: grok's braille logo animates
  // its colors, so every frame the SAME characters arrive sliced
  // into DIFFERENT runs. With one flowing span per run, each frame
  // clipped the fallback font's internal overflow at different run
  // boundaries → glyphs popped in/out and the art grew/shrank as it
  // shimmered. Per-char cells make each glyph's rect a function of
  // its column alone, so geometry is invariant under re-slicing.

  const BRAILLE_10 = '⠋⠙⠚⠀⠓⠛⠀⠊⠋⠁' // 10 code points, 10 columns

  function glyphGeometry(row: RenderRun[]): Array<[string, string, string]> {
    // [text, left, width] of every glyph span (skip bg underlays,
    // which are empty).
    const { spans } = renderRow(row)
    return spans
      .filter((s) => s.textContent !== '')
      .map((s) => [s.textContent ?? '', s.style.left, s.style.width])
  }

  it('renders one span per braille char, each at its exact column', () => {
    const { spans } = renderRow([run(BRAILLE_10)])
    expect(spans).toHaveLength(10)
    spans.forEach((s, c) => {
      expect(s.style.left).toBe(`${c * CELL_W}px`)
      expect(s.style.width).toBe(`${CELL_W}px`)
      expect(s.style.position).toBe('absolute')
      expect(s.style.overflow).toBe('hidden')
      expect(s.style.whiteSpace).toBe('pre')
      expect(s.style.textAlign).toBe('center')
    })
  })

  it('SHIMMER INVARIANCE: per-char geometry is identical however the same text is sliced into runs', () => {
    // One 10-char braille run vs the same chars split 3/4/3 with
    // different colors (what a shimmer animation does every frame).
    const whole = glyphGeometry([run(BRAILLE_10)])
    const sliced = glyphGeometry([
      run(BRAILLE_10.slice(0, 3), { fg: 0xff0000 }),
      run(BRAILLE_10.slice(3, 7), { fg: 0x00ff00 }),
      run(BRAILLE_10.slice(7), { fg: 0x0000ff }),
    ])
    expect(sliced).toEqual(whole)
    // And a different slicing again (2/5/3) — still identical.
    const resliced = glyphGeometry([
      run(BRAILLE_10.slice(0, 2), { fg: 0x123456 }),
      run(BRAILLE_10.slice(2, 7)),
      run(BRAILLE_10.slice(7), { bold: true }),
    ])
    expect(resliced).toEqual(whole)
  })

  it('keeps a plain ASCII run as ONE flowing span', () => {
    const { spans } = renderRow([run('ls -la | grep foo')])
    expect(spans).toHaveLength(1)
    expect(spans[0].textContent).toBe('ls -la | grep foo')
  })

  it('gives each wide (CJK) char a 2-cell rect', () => {
    const { spans } = renderRow([run('日本語', { cols: 6 })])
    expect(spans).toHaveLength(3)
    spans.forEach((s, c) => {
      expect(s.style.left).toBe(`${c * 2 * CELL_W}px`)
      expect(s.style.width).toBe(`${2 * CELL_W}px`)
    })
  })

  it('paints the run background as ONE underlay spanning the full run box, glyph spans transparent', () => {
    const { spans } = renderRow([run('⠋⠙⠈', { bg: 0x102030 })])
    expect(spans).toHaveLength(4)
    const underlay = spans[0]
    expect(underlay.textContent).toBe('')
    expect(underlay.style.backgroundColor).toBe('rgb(16, 32, 48)')
    expect(underlay.style.left).toBe('0px')
    expect(underlay.style.width).toBe(`${3 * CELL_W}px`)
    for (const glyph of spans.slice(1)) {
      expect(glyph.style.backgroundColor).toBe('')
    }
  })

  it('inverse-video exotic run gets its block from the underlay (swapped colors)', () => {
    const { spans } = renderRow([run('⠋⠙', { inverse: true })])
    expect(spans).toHaveLength(3)
    // Inverse with default colors: bg becomes the default fg.
    expect(spans[0].style.backgroundColor).toBe('rgb(224, 224, 224)')
    for (const glyph of spans.slice(1)) {
      expect(glyph.style.backgroundColor).toBe('')
      expect(glyph.style.color).toBe('rgb(0, 0, 0)')
    }
  })

  it('folds a zero-width follower into its base char\'s cell', () => {
    // e + combining acute + 日 : 3 code points over 3 columns.
    const { spans } = renderRow([run('é日', { cols: 3 })])
    expect(spans).toHaveLength(2)
    expect(spans[0].textContent).toBe('é')
    expect(spans[0].style.width).toBe(`${CELL_W}px`)
    expect(spans[1].textContent).toBe('日')
    expect(spans[1].style.left).toBe(`${CELL_W}px`)
    expect(spans[1].style.width).toBe(`${2 * CELL_W}px`)
  })

  it('iterates by code point, not UTF-16 unit (non-BMP emoji)', () => {
    // 😀 = one code point, two UTF-16 units, two columns.
    const { spans } = renderRow([run('😀🚀', { cols: 4 })])
    expect(spans).toHaveLength(2)
    expect(spans[0].textContent).toBe('😀')
    expect(spans[1].style.left).toBe(`${2 * CELL_W}px`)
    expect(spans[1].style.width).toBe(`${2 * CELL_W}px`)
  })

  it('does not split exotic runs in the unmeasured (pre-layout) fallback', () => {
    const { spans } = renderRow([run('⠋⠙⠈')], 0, 0)
    expect(spans).toHaveLength(1)
  })
})
