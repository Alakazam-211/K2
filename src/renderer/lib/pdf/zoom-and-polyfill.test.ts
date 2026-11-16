// The PDF tab's zoom (Fit page by default, Fit width, 100%, and steps) and
// the Map upsert polyfill pdf.js 5 needs on older WebKit.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { fitScale, zoomStep } from './zoom'
import { installMapUpsert } from './map-polyfill'

const SHEET = { w: 792, h: 612 }

describe('fit', () => {
  it('fits the whole page inside the desk, less its padding', () => {
    expect(fitScale('page', SHEET, { w: 1000, h: 500 })).toBe(0.73)
    expect(fitScale('page', SHEET, { w: 500, h: 2000 })).toBe(0.57)
  })
  it('fits the width alone', () => {
    expect(fitScale('width', SHEET, { w: 1000, h: 500 })).toBe(1.2)
  })
  it('never goes past the zoom limits', () => {
    expect(fitScale('page', { w: 9000, h: 6000 }, { w: 500, h: 400 })).toBe(0.1)
    expect(fitScale('width', { w: 20, h: 20 }, { w: 3000, h: 3000 })).toBe(4)
  })
  it('is 100% while the desk has no size yet', () => {
    expect(fitScale('page', SHEET, { w: 0, h: 0 })).toBe(1)
  })
})

describe('zoom steps', () => {
  it('moves to the next level up or down from any zoom', () => {
    expect(zoomStep(0.73, 1)).toBe(0.75)
    expect(zoomStep(0.73, -1)).toBe(0.67)
    expect(zoomStep(1, 1)).toBe(1.25)
    expect(zoomStep(1, -1)).toBe(0.75)
    expect(zoomStep(4, 1)).toBe(4)
    expect(zoomStep(0.1, -1)).toBe(0.1)
  })
})

describe('Map upsert polyfill', () => {
  function bare() {
    // A Map class without the methods, as older WebKit has.
    class M<K, V> extends Map<K, V> {}
    for (const k of ['getOrInsert', 'getOrInsertComputed'] as const) Object.defineProperty(M.prototype, k, { value: undefined, writable: true, configurable: true })
    return M
  }

  it('adds getOrInsert and getOrInsertComputed only where they are missing', () => {
    const M = bare()
    installMapUpsert(M)
    const m = new M<string, number>() as Map<string, number> & { getOrInsert(k: string, v: number): number; getOrInsertComputed(k: string, f: (k: string) => number): number }
    expect(m.getOrInsert('a', 1)).toBe(1)
    expect(m.getOrInsert('a', 2)).toBe(1)
    let calls = 0
    expect(m.getOrInsertComputed('b', (k) => (calls++, k.length + 10))).toBe(11)
    expect(m.getOrInsertComputed('b', () => (calls++, 99))).toBe(11)
    expect(calls).toBe(1)
    expect(Object.keys(M.prototype)).not.toContain('getOrInsert')
  })

  it('leaves a native method alone', () => {
    const own = function (this: unknown) {
      return 'native'
    }
    class M extends Map {}
    Object.defineProperty(M.prototype, 'getOrInsertComputed', { value: own, writable: true, configurable: true })
    installMapUpsert(M)
    expect((M.prototype as unknown as { getOrInsertComputed: unknown }).getOrInsertComputed).toBe(own)
  })

  it('works on WeakMap too', () => {
    class W extends WeakMap<object, number> {}
    Object.defineProperty(W.prototype, 'getOrInsertComputed', { value: undefined, writable: true, configurable: true })
    installMapUpsert(W)
    const w = new W() as WeakMap<object, number> & { getOrInsertComputed(k: object, f: () => number): number }
    const k = {}
    expect(w.getOrInsertComputed(k, () => 5)).toBe(5)
    expect(w.get(k)).toBe(5)
  })

  it('is installed before pdf.js loads, on the page and in the worker', () => {
    const src = (p: string): string => readFileSync(join(__dirname, '../../', p), 'utf8')
    const firstImport = (code: string): string | undefined => /^import\s+(?:[^'"]+from\s+)?['"]([^'"]+)['"]/m.exec(code)?.[1]
    expect(firstImport(src('components/FileViewerPane/PDFViewer.tsx'))).toBe('@/lib/pdf/install-map-polyfill')
    expect(firstImport(src('components/Pdf/pdf.worker.ts'))).toBe('@/lib/pdf/install-map-polyfill')
    expect(src('components/Pdf/pdf.worker.ts')).toContain("import 'pdfjs-dist/build/pdf.worker.min.mjs'")
    expect(src('components/Pdf/usePdfDocument.ts')).toMatch(/new Worker\(new URL\('\.\/pdf\.worker\.ts', import\.meta\.url\)/)
  })
})
