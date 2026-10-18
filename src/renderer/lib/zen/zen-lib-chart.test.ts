// @vitest-environment jsdom
//
// 0.46.1: a widget draws a chart offline with a vendored, pinned library.
// uPlot is read from the checked-in zen-lib folder exactly as the manifest
// pins it (size + sha256), inlined the way the sealed frame inlines library
// scripts (classic scripts, no eval, no network), and drawn into a canvas.
// jsdom has no 2D canvas, so the test records the drawing calls.

import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import manifest from '../../../shared/zen-lib.json'
import { zenLibInlineProblem } from './zen-lib-inline'

const LIB_DIR = resolve(__dirname, '../../zen-lib')

function libFiles(id: string): Array<{ name: string; text: string }> {
  const row = (manifest as { libs: Array<{ id: string; version: string; files: Array<{ name: string; bytes: number; sha256: string }> }> }).libs.find(
    (l) => l.id === id,
  )
  if (!row) throw new Error(`${id} isn't in zen-lib.json`)
  return row.files.map((f) => {
    const b = readFileSync(resolve(LIB_DIR, `${row.id}@${row.version}`, f.name))
    expect(b.length, f.name).toBe(f.bytes)
    expect(createHash('sha256').update(b).digest('hex'), f.name).toBe(f.sha256)
    return { name: f.name, text: b.toString('utf8') }
  })
}

/** A 2D context that records every call (jsdom has none). */
function recordingCanvas(): string[] {
  const calls: string[] = []
  const ctx = new Proxy(
    {},
    {
      get(_t, k) {
        if (k === 'measureText') return () => ({ width: 10, actualBoundingBoxAscent: 8, actualBoundingBoxDescent: 2 })
        if (k === 'canvas') return document.createElement('canvas')
        return (...args: unknown[]) => {
          calls.push(String(k))
          return args.length >= 0 ? { addColorStop: () => undefined } : undefined
        }
      },
      set() {
        return true
      },
    },
  )
  HTMLCanvasElement.prototype.getContext = (() => ctx) as unknown as HTMLCanvasElement['getContext']
  // uPlot builds its lines as Path2D objects.
  class RecordingPath {
    constructor() {
      return new Proxy(this, {
        get: (_t, k) => (k === 'then' ? undefined : () => void calls.push(`path.${String(k)}`)),
      })
    }
  }
  ;(globalThis as Record<string, unknown>).Path2D = RecordingPath
  return calls
}

describe('zen-lib: uPlot draws a chart offline in a widget frame', () => {
  it('is pinned, inline-safe, needs no eval, and draws a series', async () => {
    const files = libFiles('uplot')
    expect(files.map((f) => f.name)).toEqual(['uPlot.iife.min.js', 'uplot-style.js'])
    for (const f of files) {
      expect(zenLibInlineProblem(f.text), f.name).toBeNull()
      // The frame's CSP has no 'unsafe-eval': the library must not need it.
      expect(f.text, f.name).not.toMatch(/\bnew Function\b|\beval\(/)
      // No network: no fetch, XHR, importScripts or dynamic import.
      expect(f.text, f.name).not.toMatch(/\bfetch\(|XMLHttpRequest|importScripts|\bimport\(/)
    }
    const calls = recordingCanvas()
    if (!window.matchMedia) {
      Object.defineProperty(window, 'matchMedia', {
        value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined, addListener: () => undefined, removeListener: () => undefined }),
      })
    }
    // Classic scripts, in manifest order, the way the frame inlines them.
    const uPlot = new Function(`${files[0].text}\n;return uPlot`)() as new (opts: object, data: number[][], el: HTMLElement) => {
      root: HTMLElement
      destroy(): void
    }
    new Function(files[1].text)()
    expect(document.querySelector('style[data-k2-lib="uplot"]')).not.toBeNull()

    const el = document.createElement('div')
    document.body.appendChild(el)
    const t0 = 1_792_200_000
    const xs = [0, 1, 2, 3, 4].map((i) => t0 + i * 60)
    const chart = new uPlot(
      { width: 300, height: 80, series: [{}, { label: 'behind', stroke: '#d29922' }], axes: [{ show: false }, { show: false }], legend: { show: false } },
      [xs, [0, 1, 2, 1, 0]],
      el,
    )
    expect(el.querySelector('.uplot')).toBe(chart.root)
    expect(el.querySelector('canvas')).not.toBeNull()
    // uPlot commits its first draw on a microtask.
    await new Promise((r) => setTimeout(r, 20))
    expect(calls).toContain('path.lineTo')
    expect(calls.some((c) => c === 'stroke')).toBe(true)
    chart.destroy()
    expect(el.querySelector('.uplot')).toBeNull()
  })

  it('is listed next to the other chart libraries a widget can name in requires.libs', () => {
    const ids = (manifest as { libs: Array<{ id: string; version: string; license: string }> }).libs.map((l) => `${l.id}@${l.version}`)
    for (const want of ['uplot@1.6.32', 'chart.js@4.5.1', 'echarts@6.1.0', 'd3@7.9.0', 'plotly.js-dist-min@4.1.1']) expect(ids).toContain(want)
  })
})
