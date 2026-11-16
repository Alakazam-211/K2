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
