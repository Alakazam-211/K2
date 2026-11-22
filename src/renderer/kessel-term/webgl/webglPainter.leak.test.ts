// @vitest-environment jsdom
//
// GPU leak checks for the WebGL terminal painter (field report: WebKit's
// GPU process at 21 GB after 16 h). The real painter and the real WebGL2
// backend run against the counting fakes in test-utils/gpu-ledger: each
// trigger is repeated 100× and what the GPU process would hold must come
// back to where it started. A painter that leaks fails here, loudly.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createWebglPainter } from './webglPainter'
import type { PainterFrame, PainterMetrics, TerminalPainter } from './painterTypes'
import { installGpuLedger, type GpuLedger } from '@/test-utils/gpu-ledger'

const REPEAT = 100

const run = (text: string) => ({
  text,
  fg: null,
  bg: null,
  bold: false,
  italic: false,
  underline: false,
  inverse: false,
  dim: false,
  strikeout: false,
})

function frame(cols: number, rows: number, tag: string): PainterFrame {
  return {
    snapshot: {
      cols,
      rows,
      grid: Array.from({ length: rows }, (_, r) => [run(`${tag} row ${r} ┌─┐ ✓`)]),
      scrollback: [],
    },
    scrollPx: 0,
    selection: null,
    theme: { fg: 0xdddddd, bg: 0x101010, selection: 0x334455, textGamma: 1 },
  } as unknown as PainterFrame
}

const metrics = (fontSize = 13, dpr = 2): PainterMetrics => ({
  cssCellW: fontSize * 0.6,
  cssCellH: fontSize * 1.3,
  dpr,
  fontFamily: 'monospace',
  fontSize,
})

const atlasPages = (): number => document.querySelectorAll('[data-k2-atlas-page]').length

let ledger: GpuLedger
const fatals: string[] = []

function mounted(): { painter: TerminalPainter; canvas: HTMLCanvasElement } {
  const canvas = document.createElement('canvas')
  document.body.appendChild(canvas)
  const painter = createWebglPainter()
  painter.onFatal((r) => fatals.push(r))
  painter.mount(canvas)
  painter.setMetrics(metrics())
  painter.render(frame(120, 40, 'first'))
  return { painter, canvas }
}

beforeEach(() => {
  fatals.length = 0
  ledger = installGpuLedger()
  vi.spyOn(console, 'warn').mockImplementation(() => {})
})

afterEach(() => {
  ledger.restore()
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

describe('WebGL terminal painter: GPU memory returns to baseline', () => {
  it('one live painter holds one context, its GL objects, one atlas page and one texture', () => {
    const { painter } = mounted()
    expect(fatals).toEqual([])
    expect(ledger.liveGlContexts()).toBe(1)
    expect(ledger.liveGlObjects()).toBeGreaterThan(0)
    expect(ledger.textureBytes()).toBeGreaterThan(0)
    expect(atlasPages()).toBe(1)
    painter.dispose()
  })

  it(`show/hide (mount → paint → dispose on a fresh canvas) ${REPEAT}×: nothing stays behind`, () => {
    for (let i = 0; i < REPEAT; i++) {
      const { painter, canvas } = mounted()
      painter.dispose()
      canvas.remove()
    }
    expect(fatals).toEqual([])
    expect(ledger.glContextsMade).toBe(REPEAT)
    expect(ledger.liveGlContexts(), ledger.summary()).toBe(0)
    expect(ledger.drawingBufferBytes(), ledger.summary()).toBe(0)
    expect(ledger.liveGlObjects(), ledger.summary()).toBe(0)
    expect(ledger.textureBytes(), ledger.summary()).toBe(0)
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(0)
    expect(atlasPages()).toBe(0)
  })

  it(`font / zoom / DPR change ${REPEAT}×: one atlas page, the old ones freed`, () => {
    const { painter } = mounted()
    const baseObjects = ledger.liveGlObjects()
    for (let i = 0; i < REPEAT; i++) {
      painter.setMetrics(metrics(11 + (i % 8), i % 2 ? 2 : 1))
      painter.render(frame(120, 40, `font ${i}`))
    }
    expect(fatals).toEqual([])
    expect(atlasPages()).toBe(1)
    expect(ledger.liveGlObjects()).toBe(baseObjects)
    expect(ledger.liveGlContexts()).toBe(1)
    // Only the current page has pixels: 2D bytes equal its own size.
    const page = document.querySelector<HTMLCanvasElement>('[data-k2-atlas-page]')!
    expect(ledger.canvas2dBytes(), ledger.summary()).toBe(page.width * page.height * 4)
    painter.dispose()
    expect(ledger.canvas2dBytes()).toBe(0)
    expect(atlasPages()).toBe(0)
  })

  it(`resize ${REPEAT}×: one drawing buffer, sized to the last frame`, () => {
    const { painter, canvas } = mounted()
    const baseObjects = ledger.liveGlObjects()
    for (let i = 0; i < REPEAT; i++) painter.render(frame(80 + (i % 60), 24 + (i % 30), `size ${i}`))
    expect(fatals).toEqual([])
    expect(ledger.liveGlContexts()).toBe(1)
    expect(ledger.liveGlObjects()).toBe(baseObjects)
    expect(ledger.drawingBufferBytes()).toBe(canvas.width * canvas.height * 4)
    painter.dispose()
  })

  it(`context lost and restored ${REPEAT}×: rebuilt once each time, nothing doubled`, () => {
    const { painter, canvas } = mounted()
    const baseObjects = ledger.liveGlObjects()
    for (let i = 0; i < REPEAT; i++) {
      ledger.loseContext(canvas)
      expect(ledger.liveGlObjects()).toBe(0)
      ledger.restoreContext(canvas)
      painter.render(frame(120, 40, `restored ${i}`))
    }
    expect(fatals).toEqual([])
    expect(ledger.liveGlObjects(), ledger.summary()).toBe(baseObjects)
    expect(atlasPages()).toBe(1)
    painter.dispose()
    expect(ledger.liveGlContexts()).toBe(0)
  })

  it('a painter that fails its sanity probe still gives its context back', () => {
    const canvas = document.createElement('canvas')
    const painter = createWebglPainter({
      createBackend: () => null,
    })
    painter.onFatal((r) => fatals.push(r))
    painter.mount(canvas)
    expect(fatals).toEqual(['webgl2-unavailable'])
    painter.dispose()
    expect(canvas.width * canvas.height).toBe(0)
  })
})
