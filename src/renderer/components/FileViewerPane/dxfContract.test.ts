// The five client fixes K2 Render's draw contract asks for (K2DRAW1 r0,
// §9.9 O4), one fixture each under __fixtures__/dxf-*.dxf, so the viewer
// and the server's 2D stills follow one rule set.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { buildDrawing, FG, type DxfNode } from './dxfModel'

const fixture = (name: string): string => readFileSync(resolve(__dirname, '__fixtures__', name), 'utf8')
const flat = (nodes: DxfNode[]): DxfNode[] => nodes.flatMap((n) => (n.k === 'group' ? [n, ...flat(n.children)] : [n]))
const of = <K extends DxfNode['k']>(nodes: DxfNode[], k: K) =>
  flat(nodes).filter((n): n is Extract<DxfNode, { k: K }> => n.k === k)

describe('O4 #1: true colour (group 420)', () => {
  const d = buildDrawing(fixture('dxf-true-color.dxf'))

  it('wins when present, then ACI, then BYLAYER', () => {
    expect(of(d.nodes, 'path').map((p) => p.c)).toEqual(['#3399cc', '#ff8800', '#8844aa', '#0000ff', '#ff0000'])
  })
})

describe('O4 #2: a layer frozen only in new viewports', () => {
  const d = buildDrawing(fixture('dxf-frozen-new-vp.dxf'))

  it('is drawn in model space; frozen (flag 1) and off layers are not', () => {
    // THAWED green, NEWVP blue, LOCKED magenta; FROZEN and OFF hidden.
    expect(of(d.nodes, 'path').map((p) => p.c)).toEqual(['#00ff00', '#0000ff', '#ff00ff'])
    expect(d.drawn).toBe(3)
  })
})

describe('O4 #3: aligned (72 = 3) and fit (72 = 5) TEXT', () => {
  const text = fixture('dxf-aligned-fit-text.dxf')
  const [aligned, fit, tilted] = of(buildDrawing(text).nodes, 'text')

  it('start at the first point, run along p1→p2 and are stretched to its length', () => {
    for (const [t, x, y, rot, len] of [
      [aligned, 0, 0, 0, 100],
      [fit, 0, -30, 0, 100],
      [tilted, 0, -70, (Math.atan2(20, 60) * 180) / Math.PI, Math.hypot(60, 20)],
    ] as const) {
      expect([t.x, t.y, t.anchor, t.y0]).toEqual([x, y, 'start', 0])
      expect(t.rot).toBeCloseTo(rot, 9)
      expect(t.len).toBeCloseTo(len, 9)
    }
  })

  it('fit keeps its height; aligned scales its height with its width', () => {
    expect(fit.h).toBe(5)
    // "ALIGNED" at cap height h is 7 × 0.6·h wide by the estimate: 100 = 4.2·h.
    expect(aligned.h).toBeCloseTo(100 / 4.2, 9)
    expect(aligned.fs).toBeCloseTo(100 / 4.2 / 0.7, 9)
    // With the page's own advance widths.
    const m = { cap: 0.7, advance: (s: string) => s.length * 0.5 }
    const [a2] = of(buildDrawing(text, m).nodes, 'text')
    // Font size f draws 7 × 0.5·f = 100.
    expect(a2.fs).toBeCloseTo(100 / 3.5, 9)
  })
})

describe('O4 #5: rational spline weights (group 41)', () => {
  const text = fixture('dxf-rational-spline.dxf')
  const spline = (src: string) => of(buildDrawing(src).nodes, 'path')[1]
  const points = (d: string): [number, number][] =>
    d.split(/\s*[ML]\s*/).filter(Boolean).map((p) => p.split(' ').map(Number) as [number, number])

  it('weights 1, √2/2, 1 draw the exact quarter circle r = 50, within the contract tolerance', () => {
    const pts = points(spline(text).d)
    expect(pts[0]).toEqual([50, 0])
    expect(pts[pts.length - 1]).toEqual([0, 50])
    // Every vertex on the circle (to the path's 1e-4 rounding)…
    for (const [x, y] of pts) expect(Math.abs(Math.hypot(x, y) - 50)).toBeLessThan(2e-4)
    // …and every chord within 2.5e-4 of the spline's box diagonal (K2DRAW1 §4).
    const tol = 2.5e-4 * Math.hypot(50, 50)
    for (let i = 1; i < pts.length; i++) {
      const [ax, ay] = pts[i - 1]
      const [bx, by] = pts[i]
      const mid = Math.hypot((ax + bx) / 2, (ay + by) / 2)
      expect(50 - mid).toBeLessThan(tol)
    }
    expect(pts.length).toBeLessThanOrEqual(4096)
  })

  it('2D control points (no 30, so the reader re-reads each 41) keep one weight each', () => {
    expect(spline(text.replace(/\n30\n0(?=\n41)/g, '')).d).toBe(spline(text).d)
  })

  it('equal weights draw the plain B-spline', () => {
    const plain = text.replace(/\n41\n[^\n]+/g, '')
    const equal = text.replace(/\n41\n[^\n]+/g, '\n41\n2')
    expect(spline(equal).d).toBe(spline(plain).d)
  })
})

describe('O4 #4: a DXF text height is the cap height', () => {
  const text = fixture('dxf-cap-height.dxf')

  it('the font is scaled so capitals measure h (0.7 cap ratio until the page measures its font)', () => {
    const t = of(buildDrawing(text).nodes, 'text')
    expect(t).toHaveLength(2)
    for (const n of t) {
      expect(n.h).toBe(10)
      expect(n.fs).toBeCloseTo(10 / 0.7, 9)
    }
  })

  it('uses the measured cap ratio of the font the page draws in', () => {
    const t = of(buildDrawing(text, { cap: 0.75, advance: (s) => s.length * 0.6 }).nodes, 'text')
    expect(t.map((n) => n.fs)).toEqual([10 / 0.75, 10 / 0.75])
  })
})
