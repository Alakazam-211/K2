// @vitest-environment jsdom
//
// The spare WebGL surface and the shared glyph atlas (switch speed,
// 0.46.18): what a painter in host mode reuses, and the bounds that keep it
// from becoming "every pane keeps a context" again (a4b0b3b19c, WebKit's
// GPU process at 21 GB). Real painter, real backend, counting GL fakes.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createWebglPainter } from './webglPainter'
import type { PainterFrame, PainterMetrics, TerminalPainter } from './painterTypes'
import { installGpuLedger, type GpuLedger } from '@/test-utils/gpu-ledger'
import {
  SPARE_IDLE_MS,
  hasSpareSurface,
  liveSurfaceCount,
  releaseAllSurfaces,
} from './glSurfacePool'
import { ATLAS_IDLE_MS, liveAtlasCount, releaseAllAtlases } from './atlasCache'

const run = (text: string) => ({
  text, fg: null, bg: null, bold: false, italic: false, underline: false, inverse: false, dim: false, strikeout: false,
})
function frame(tag: string): PainterFrame {
  return {
    snapshot: { cols: 80, rows: 24, grid: Array.from({ length: 24 }, (_, r) => [run(`${tag} ${r} ┌─┐`)]), scrollback: [] },
    scrollPx: 0,
    selection: null,
    theme: { fg: 0xdddddd, bg: 0x101010, selection: 0x334455, textGamma: 1 },
  } as unknown as PainterFrame
}
const metrics = (fontSize = 13): PainterMetrics => ({
  cssCellW: fontSize * 0.6, cssCellH: fontSize * 1.3, dpr: 2, fontFamily: 'monospace', fontSize,
})

let ledger: GpuLedger
const fatals: string[] = []

function show(fontSize = 13): { painter: TerminalPainter; host: HTMLDivElement } {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const painter = createWebglPainter()
  painter.onFatal((r) => fatals.push(r))
  painter.mount(host)
  painter.setMetrics(metrics(fontSize))
  painter.render(frame('x'))
  return { painter, host }
}

beforeEach(() => {
  vi.useFakeTimers()
  ledger = installGpuLedger()
  fatals.length = 0
})
afterEach(() => {
  releaseAllSurfaces()
  releaseAllAtlases()
  ledger.restore()
  vi.useRealTimers()
  document.body.innerHTML = ''
})

describe('the spare surface', () => {
  it('a hide then a show moves one context: nothing is built for the show', () => {
    const a = show()
    expect(a.painter.reusedSurface?.()).toBe(false)
    const canvas = a.host.querySelector('canvas')
    a.painter.dispose()
    expect(hasSpareSurface()).toBe(true)
    expect(a.host.querySelector('canvas'), 'the spare leaves the document').toBeNull()
    const b = show()
    expect(b.painter.reusedSurface?.()).toBe(true)
    expect(b.host.querySelector('canvas')).toBe(canvas)
    expect(ledger.glContextsMade).toBe(1)
    expect(ledger.liveGlContexts()).toBe(1)
    expect(fatals).toEqual([])
    b.painter.dispose()
  })

  it('is freed after SPARE_IDLE_MS when nobody takes it', () => {
    show().painter.dispose()
    vi.advanceTimersByTime(SPARE_IDLE_MS - 1)
    expect(ledger.liveGlContexts()).toBe(1)
    vi.advanceTimersByTime(1)
    expect(hasSpareSurface()).toBe(false)
    expect(ledger.liveGlContexts()).toBe(0)
    expect(liveSurfaceCount()).toBe(0)
  })

  it('there is never more than one spare', () => {
    const shown = Array.from({ length: 5 }, () => show())
    expect(ledger.liveGlContexts()).toBe(5)
    for (const s of shown) s.painter.dispose()
    expect(ledger.liveGlContexts(), ledger.summary()).toBe(1)
    expect(liveSurfaceCount()).toBe(1)
  })

  it('a spare whose context WebKit took back is never handed out', () => {
    const a = show()
    const canvas = a.host.querySelector('canvas')!
    a.painter.dispose()
    ledger.loseContext(canvas)
    const b = show()
    expect(b.painter.reusedSurface?.()).toBe(false)
    expect(b.host.querySelector('canvas')).not.toBe(canvas)
    expect(fatals).toEqual([])
    expect(ledger.liveGlContexts()).toBe(1)
  })

  it('the spare holds no drawing buffer and a 1x1 atlas texture', () => {
    const a = show()
    expect(ledger.drawingBufferBytes()).toBeGreaterThan(4)
    expect(ledger.textureBytes()).toBeGreaterThan(4)
    a.painter.dispose()
    expect(ledger.drawingBufferBytes()).toBe(4)
    expect(ledger.textureBytes()).toBe(4)
  })

  it('a canvas-mode painter (owns its canvas) never touches the spare', () => {
    const canvas = document.createElement('canvas')
    const p = createWebglPainter()
    p.mount(canvas)
    p.setMetrics(metrics())
    p.render(frame('c'))
    p.dispose()
    expect(hasSpareSurface()).toBe(false)
    expect(ledger.liveGlContexts()).toBe(0)
    expect(liveAtlasCount()).toBe(0)
  })
})

describe('the shared atlas', () => {
  it('two panes with one font share one page; a third font change frees the unused one', () => {
    const a = show()
    const b = show()
    expect(liveAtlasCount()).toBe(1)
    a.painter.setMetrics(metrics(15))
    expect(liveAtlasCount()).toBe(2)
    b.painter.setMetrics(metrics(15))
    expect(liveAtlasCount(), 'nobody uses 13 px any more').toBe(1)
    a.painter.dispose()
    b.painter.dispose()
  })

  it('kept ATLAS_IDLE_MS after its last pane hides, then freed', () => {
    const a = show()
    a.painter.dispose()
    expect(liveAtlasCount()).toBe(1)
    const b = show()
    expect(liveAtlasCount()).toBe(1)
    b.painter.dispose()
    vi.advanceTimersByTime(ATLAS_IDLE_MS)
    expect(liveAtlasCount()).toBe(0)
    expect(document.querySelectorAll('[data-k2-atlas-page]').length).toBe(0)
  })
})
