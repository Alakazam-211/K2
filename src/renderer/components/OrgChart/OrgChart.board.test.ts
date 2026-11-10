// @vitest-environment jsdom
// The Org Chart's lines are Board's (prd-org-chart-v1 §6.5, vs-live A5):
//   - `board-geom.ts` is board-file-1's own geom.js, not a copy;
//   - the dotted layer's dash is what board-file-1's render.js draws for a
//     `dotted` connector (render.js keeps `dash()` private, so the Org
//     Chart mirrors it; this pins the two together).
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { BoardGeom, boardDash } from './board-geom'

const DIR = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/board-file-1')
const run = (file: string): void => {
  // The widget's plain scripts, run in this window as the widget runs them.
  new Function(readFileSync(resolve(DIR, file), 'utf8'))()
}

describe("the Org Chart's lines are Board's", () => {
  it("BoardGeom is board-file-1's geom.js (same routes as a fresh load of the file)", () => {
    const ours = BoardGeom
    const g = window as unknown as { BoardGeom: typeof BoardGeom }
    delete (g as { BoardGeom?: unknown }).BoardGeom
    run('geom.js')
    const fresh = g.BoardGeom
    expect(fresh).not.toBe(ours)
    const a = { x: 0, y: 0, w: 220, h: 72 }
    const b = { x: 300, y: 200, w: 220, h: 72 }
    const c = { from: { id: 'a', anchor: 'bottom' as const }, to: { id: 'b', anchor: 'top' as const }, route: 'elbow' as const }
    const get = (id: string) => (id === 'a' ? a : id === 'b' ? b : null)
    expect(ours.route(c, get)).toEqual(fresh.route(c, get))
  })

  it("the dotted (and dashed) patterns are what Board's render.js draws", () => {
    HTMLCanvasElement.prototype.getContext = (() => null) as unknown as typeof HTMLCanvasElement.prototype.getContext
    run('schema.js')
    run('render.js')
    const R = (window as unknown as { BoardRender: { create: (world: HTMLElement, model: unknown) => { all: () => void } } }).BoardRender
    for (const [kind, sw] of [['dotted', 2], ['dotted', 3], ['dashed', 2]] as const) {
      const world = document.createElement('div')
      document.body.appendChild(world)
      const line = { id: 'c', type: 'connector', from: { x: 0, y: 0 }, to: { x: 100, y: 0 }, dash: kind, strokeWidth: sw, route: 'straight' }
      const model = { get: (id: string) => (id === 'c' ? line : null), all: () => [line], nameOf: (x: string) => x, isMe: () => false }
      R.create(world, model).all()
      const drawn = world.querySelector('[data-id="c"] path.line')!.getAttribute('stroke-dasharray')
      expect([kind, sw, boardDash(kind, sw)]).toEqual([kind, sw, drawn])
      world.remove()
    }
  })
})
