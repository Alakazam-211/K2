// @vitest-environment jsdom
//
// GlyphAtlas presentation contract, with a recording Canvas2D stub
// (jsdom has no canvas). Pins the clipped-emoji-dot fix on the WebGL
// path: Claude Code's ⏺ (U+23FA) used to fillText through the font
// stack, which on iOS fell back to Apple Color Emoji — a grey button
// clipped to one cell. Now it rasterizes procedurally; real emoji
// still fillText from the emoji font at their two-column width; and
// the DOM painter makes the same call for every cluster.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from '@testing-library/react'
import { GlyphAtlas } from './glyphAtlas'
import { TerminalRow } from '../rowRender'
import { VS15 } from '../glyphRoute'

const CELL_W = 18 // device px
const CELL_H = 40
const FONT_PX = 30

interface Rec {
  fillText: { text: string; font: string }[]
  arcs: number
  fills: number
  fillRects: number
}

function installCanvasStub(rec: Rec) {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    function (this: HTMLCanvasElement) {
      // Pretend colour glyphs come from the emoji font: anything
      // fillText'd whose first code point is ≥ U+1F000 or ✅ paints
      // coloured pixels (what the atlas's mono-scan detects).
      let lastColour = false
      const ctx = {
        canvas: this,
        font: '',
        fillStyle: '',
        strokeStyle: '',
        lineWidth: 1,
        globalAlpha: 1,
        textBaseline: 'alphabetic',
        measureText(text: string) {
          const cp = text.codePointAt(0) ?? 0
          const wide = cp >= 0x1f000 || cp === 0x2705
          return {
            width: wide ? 2.4 * CELL_W : CELL_W,
            fontBoundingBoxAscent: 28,
            fontBoundingBoxDescent: 9,
          }
        },
        fillText(text: string) {
          rec.fillText.push({ text, font: ctx.font })
          const cp = text.codePointAt(0) ?? 0
          lastColour = cp >= 0x1f000 || cp === 0x2705
        },
        getImageData(_x: number, _y: number, w: number, h: number) {
          const data = new Uint8ClampedArray(w * h * 4)
          data.set(lastColour ? [255, 200, 0, 255] : [255, 255, 255, 255])
          return { data }
        },
        arc() {
          rec.arcs++
        },
        fill() {
          rec.fills++
        },
        fillRect() {
          rec.fillRects++
        },
        save() {},
        restore() {},
        beginPath() {},
        closePath() {},
        rect() {},
        clip() {},
        moveTo() {},
        lineTo() {},
        arcTo() {},
        stroke() {},
        clearRect() {},
        drawImage() {},
      }
      return ctx as unknown as CanvasRenderingContext2D
    } as unknown as HTMLCanvasElement['getContext'],
  )
}

let rec: Rec
let atlas: GlyphAtlas

beforeEach(() => {
  rec = { fillText: [], arcs: 0, fills: 0, fillRects: 0 }
  installCanvasStub(rec)
  atlas = new GlyphAtlas({
    deviceCellW: CELL_W,
    deviceCellH: CELL_H,
    fontFamily: "'MesloLGM Nerd Font Mono', monospace",
    fontDevicePx: FONT_PX,
  })
})

afterEach(() => {
  atlas.dispose()
  vi.restoreAllMocks()
})

describe('GlyphAtlas presentation (the clipped emoji-dot fix)', () => {
  it('⏺ rasterizes procedurally: no fillText, one filled circle, one cell, tinted (not colour)', () => {
    const slot = atlas.get('⏺', false, false, 1)!
    expect(rec.fillText).toHaveLength(0)
    expect(rec.arcs).toBe(1)
    expect(rec.fills).toBe(1)
    expect(slot.w).toBe(CELL_W) // single width
    expect(slot.offsetX ?? 0).toBe(0) // no emoji overhang
    expect(slot.color).toBe(false) // shader tints it with the cell fg
  })

  it('⏸ / ⏹ / ⏵ are procedural too', () => {
    for (const ch of ['⏸', '⏹', '⏵']) atlas.get(ch, false, false, 1)
    expect(rec.fillText).toHaveLength(0)
  })

  it('✅ / 🎉 keep the emoji path: fillText at EMOJI_FONT_SCALE, two columns, colour, overhang', () => {
    for (const ch of ['✅', '🎉']) {
      rec.fillText.length = 0
      const slot = atlas.get(ch, false, false, 2)!
      expect(rec.fillText).toEqual([
        { text: ch, font: `${Math.round(FONT_PX * 1.15)}px 'MesloLGM Nerd Font Mono', monospace` },
      ])
      expect(slot.color).toBe(true)
      expect(slot.w).toBeGreaterThanOrEqual(2 * CELL_W)
      expect(slot.offsetX).toBeLessThanOrEqual(0)
    }
  })

  it('one-column symbols with an emoji form fillText with VS15, clipped to one cell', () => {
    const slot = atlas.get('✔', false, false, 1)!
    expect(rec.fillText.map((f) => f.text)).toEqual(['✔' + VS15])
    expect(slot.w).toBe(CELL_W)
    expect(slot.color).toBe(false)
  })

  it('both painters agree for every cluster: painted / emoji / text(+VS15)', () => {
    const cases: [string, number][] = [
      ['⏺', 1], ['⏸', 1], ['⏹', 1], ['⏵', 1], ['─', 1], ['█', 1],
      ['✅', 2], ['🎉', 2],
      ['✔', 1], ['☀', 1], ['⏯', 1],
      ['●', 1], ['○', 1], ['✓', 1], ['⠋', 1],
    ]
    for (const [ch, width] of cases) {
      // WebGL
      rec.fillText.length = 0
      rec.fills = rec.fillRects = rec.arcs = 0
      const slot = atlas.get(ch, false, false, width)!
      const gl =
        rec.fillText.length === 0
          ? 'painted'
          : slot.color
            ? 'emoji'
            : `text:${rec.fillText[0].text}`
      // DOM
      const { container, unmount } = render(
        <TerminalRow
          row={[{
            text: ch, fg: null, bg: null, bold: false, italic: false,
            underline: false, inverse: false, dim: false, strikeout: false,
            cols: width === 2 ? 2 : undefined,
          }]}
          absRow={0}
          defaultFg="rgb(224,224,224)"
          defaultBg="rgb(0,0,0)"
          cellWidth={9}
          cellHeight={20}
        />,
      )
      const span = container.querySelector('span') as HTMLElement | null
      const canvas = container.querySelector('canvas[data-synth-canvas]')
      const dom = canvas && !span
        ? 'painted'
        : span!.style.fontSize === '1.15em' && span!.style.overflow === ''
          ? 'emoji'
          : `text:${span!.textContent}`
      unmount()
      expect(dom, ch).toBe(gl)
    }
  })
})
