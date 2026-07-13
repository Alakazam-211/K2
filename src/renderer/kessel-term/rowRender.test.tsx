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
  return {
    rowDiv,
    spans: Array.from(rowDiv.querySelectorAll('span')),
    canvas: rowDiv.querySelector(
      'canvas[data-synth-canvas]',
    ) as HTMLCanvasElement | null,
  }
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

describe('TerminalRow synthetic box/block glyphs', () => {
  // The seam defects these tests pin: font ink for █/▀/─ never
  // exactly fills the line box or reaches the cell edges, so stacked
  // blocks (Claude Code's logo) showed horizontal seams and TUI
  // borders broke at some zooms. Box-drawing and block-element cells
  // render as painted geometry with NO text node — since the grid-
  // scroll fix, drawn into ONE per-row <canvas> (solid fillRects via
  // drawSyntheticGlyph) instead of per-cell CSS gradient spans.
  // jsdom has no Canvas2D, so these tests pin the STRUCTURE (which
  // cells route to the canvas vs font spans, canvas geometry); the
  // draw itself is pinned by syntheticRaster.test.ts's stub-ctx
  // suite and the shared math by syntheticGlyphs.test.ts.

  it('renders a box-drawing char on the row canvas (no text span) while an ordinary letter still renders text', () => {
    const { spans, canvas } = renderRow([run('A─')])
    expect(spans).toHaveLength(1) // only the letter — ─ has no span
    expect(spans[0].textContent).toBe('A')
    expect(canvas).not.toBeNull()
    // Canvas covers through the last synthetic cell (col 1 + 1 wide)
    // at dpr 1, one cell tall.
    expect(canvas!.width).toBe(2 * CELL_W)
    expect(canvas!.height).toBe(CELL_H)
    expect(canvas!.style.pointerEvents).toBe('none')
  })

  it('█ routes to the canvas with no DOM span at all', () => {
    const { spans, canvas } = renderRow([run('█')])
    expect(spans).toHaveLength(0)
    expect(canvas).not.toBeNull()
    expect(canvas!.width).toBe(CELL_W)
  })

  it('one canvas serves the whole row regardless of cell count', () => {
    const { rowDiv, canvas } = renderRow([run('──'), run('x'), run('┼┼')])
    expect(rowDiv.querySelectorAll('canvas')).toHaveLength(1)
    expect(canvas!.width).toBe(5 * CELL_W) // through the last ┼
  })

  it('inverse ─ keeps its block via the run underlay span', () => {
    const { spans, canvas } = renderRow([run('─', { inverse: true })])
    // Inverse with default colors: underlay = default fg; the stroke
    // itself (ink = default bg) lives on the canvas.
    expect(spans).toHaveLength(1)
    expect(spans[0].style.backgroundColor).toBe('rgb(224, 224, 224)')
    expect(canvas).not.toBeNull()
  })

  it('░ shade keeps the run background underlay under the canvas', () => {
    const { spans, canvas } = renderRow([run('░', { bg: 0x102030 })])
    expect(spans).toHaveLength(1)
    expect(spans[0].style.backgroundColor).toBe('rgb(16, 32, 48)') // underlay
    expect(canvas).not.toBeNull()
  })

  it('╭ routes to the canvas (arc stroke, no child div)', () => {
    const { spans, canvas } = renderRow([run('╭')])
    expect(spans).toHaveLength(0)
    expect(canvas).not.toBeNull()
    expect(canvas!.querySelector('div')).toBeNull()
  })

  it('a sextant (U+1FB00) routes to the canvas; its neighbor stays a font span', () => {
    const { spans, canvas } = renderRow([run('\u{1FB00}x')])
    expect(spans).toHaveLength(1)
    expect(spans[0].textContent).toBe('x')
    expect(canvas).not.toBeNull()
  })

  it('geometric shapes (U+25A0+) and braille still render as font text, no canvas', () => {
    const { spans, canvas } = renderRow([run('■⠋')])
    expect(spans[0].textContent).toBe('■')
    expect(spans[1].textContent).toBe('⠋')
    expect(canvas).toBeNull()
  })

  it('scales the canvas bitmap to device pixels', () => {
    const { container } = render(
      <TerminalRow
        row={[run('─')]}
        absRow={7}
        defaultFg="rgb(224,224,224)"
        defaultBg="rgb(0,0,0)"
        cellWidth={CELL_W}
        cellHeight={CELL_H}
        dpr={2}
      />,
    )
    const canvas = container.querySelector('canvas') as HTMLCanvasElement
    expect(canvas.width).toBe(2 * CELL_W)
    expect(canvas.height).toBe(2 * CELL_H)
    // css box stays cell-sized so the bitmap maps 1:1 to physical px
    expect(canvas.style.width).toBe(`${CELL_W}px`)
    expect(canvas.style.height).toBe(`${CELL_H}px`)
  })

  it('falls back to font text in the unmeasured (pre-layout) state', () => {
    const { spans, canvas } = renderRow([run('─│')], 0, 0)
    expect(spans).toHaveLength(1)
    expect(spans[0].textContent).toBe('─│')
    expect(canvas).toBeNull()
  })
})

describe('TerminalRow emoji clip exemption (the squished-✅ fix)', () => {
  it('✅ gets a two-column anchored cell WITHOUT overflow clipping', () => {
    const { spans } = renderRow([run('✅', { cols: 2 })])
    expect(spans).toHaveLength(1)
    const cell = spans[0]
    expect(cell.textContent).toBe('✅')
    expect(cell.style.left).toBe('0px')
    expect(cell.style.width).toBe(`${2 * CELL_W}px`) // anchored box
    expect(cell.style.overflow).toBe('') // clip lifted — no truncation
    expect(cell.style.textAlign).toBe('center') // overflow hangs evenly
  })

  it('anchoring survives: the run AFTER an emoji sits at its model column', () => {
    const { spans } = renderRow([run('✅', { cols: 2 }), run('ok')])
    expect(spans[1].textContent).toBe('ok')
    expect(spans[1].style.left).toBe(`${2 * CELL_W}px`)
  })

  it('non-emoji exotic glyphs (braille) keep their clip', () => {
    const { spans } = renderRow([run('⠋')])
    expect(spans[0].style.overflow).toBe('hidden')
  })
})

describe('TerminalRow emoji size bump', () => {
  it('emoji cells render at EMOJI_FONT_SCALE em, text cells inherit', () => {
    const { spans } = renderRow([run('✅', { cols: 2 }), run('ok')])
    expect(spans[0].style.fontSize).toBe('1.15em')
    expect(spans[1].style.fontSize).toBe('')
  })

  it('braille keeps the inherited size (only emoji scale)', () => {
    const { spans } = renderRow([run('⠋')])
    expect(spans[0].style.fontSize).toBe('')
  })
})
