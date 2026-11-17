// Reading a drawing from its model: the header, every page of model space,
// and each block definition its placements and dimensions need (nested ones
// too), fetched once per digest and kept across versions. What comes back
// has what the viewer's own reading has (bounds, an index of boxes by handle
// and layer, counts), so fitting, view addresses and the change highlight work
// the same on either path, plus the shapes and what wasn't drawn.

import type { Bounds, DxfDrawing } from '../../dxfModel'
import type { DrawingIndex } from '../drawing-index'
import type { TextMetrics } from '../text-metrics'
import { ModelShapes } from './ModelShapes'
import { nextCursor, type DrawingModelSource } from './source'
import type { ModelHeader, ModelLayout, ModelPage, ModelRow } from './types'

export type NotDrawn = { count: number; lines: string[] }

export type ModelDrawing = DxfDrawing & {
  shapes: React.ReactNode
  notDrawn: NotDrawn
  /** Paper layouts: listed, drawn later. */
  paper: ModelLayout[]
  version: number
  /** What the drawing was read from: selection and edits work on these. */
  header: ModelHeader
  rows: ModelRow[]
}

/** Block definitions by digest, per source: a definition that hasn't changed is never fetched again. */
const definitions = new WeakMap<DrawingModelSource, Map<string, ModelPage>>()

/** Most pages any one space is read in, so a server that never stops can't hang the viewer. */
const MAX_PAGES = 2000

async function allPages(first: ModelPage | null, src: DrawingModelSource, space: string): Promise<ModelPage | null> {
  if (!first) return null
  const entities = [...first.entities]
  let cursor = nextCursor(first)
  for (let n = 0; cursor && n < MAX_PAGES; n++) {
    const p = await src.page(space, cursor)
    if (!p) return null
    entities.push(...p.entities)
    cursor = nextCursor(p)
  }
  return { ...first, entities, next: null }
}

const refs = (rows: readonly ModelRow[]): string[] =>
  rows.flatMap((r) => ((r.kind === 'INSERT' || r.kind === 'DIMENSION') && r.drawn && typeof r.geom?.block === 'string' ? [r.geom.block] : []))

async function loadBlocks(src: DrawingModelSource, header: ModelHeader, rows: readonly ModelRow[]): Promise<Map<string, ModelPage>> {
  let cache = definitions.get(src)
  if (!cache) definitions.set(src, (cache = new Map()))
  const digest = new Map(header.blocks.map((b) => [b.handle, b.digest]))
  const blocks = new Map<string, ModelPage>()
  const missing = new Set<string>()
  const queue = refs(rows)
  while (queue.length) {
    const h = queue.shift()!
    if (blocks.has(h) || missing.has(h)) continue
    const d = digest.get(h)
    let page = d ? cache.get(d) : undefined
    if (!page) {
      page = (await allPages(await src.blockPage(h), src, h)) ?? undefined
      if (page && d) cache.set(d, page)
    }
    if (!page) {
      missing.add(h)
      continue
    }
    blocks.set(h, page)
    queue.push(...refs(page.entities))
  }
  return blocks
}

// The model's reasons, in plain words. Attribute templates aren't counted: a
// block's placements show their values.
const WHY: Record<string, (n: number, kinds: string[]) => string> = {
  not_modelled: (_n, kinds) => `${kinds.length} ${kinds.length === 1 ? 'kind' : 'kinds'} this viewer doesn’t draw yet: ${kinds.join(', ')}`,
  spline_weights: (n) => `${n} ${n === 1 ? 'spline' : 'splines'} whose weights can’t be drawn`,
  over_cap: (n) => `${n} too large to draw`,
  unreadable: (n) => `${n} that couldn’t be read from the file`,
  viewport: (n) => `${n} ${n === 1 ? 'viewport' : 'viewports'}, shown with their layout`,
}

export function notDrawnOf(rows: readonly ModelRow[]): NotDrawn {
  const by = new Map<string, ModelRow[]>()
  for (const r of rows) {
    if (r.drawn || r.why === 'attdef') continue
    const k = r.why ?? 'not_drawn'
    by.set(k, [...(by.get(k) ?? []), r])
  }
  const order = Object.keys(WHY)
  const rank = (w: string) => (order.includes(w) ? order.indexOf(w) : order.length)
  const lines = [...by].sort((x, y) => rank(x[0]) - rank(y[0])).map(([why, rs]) => {
    const kinds = [...new Set(rs.map((r) => r.kind))]
    return (WHY[why] ?? ((n: number) => `${n} not drawn (${kinds.join(', ')})`))(rs.length, kinds)
  })
  return { count: [...by.values()].reduce((a, rs) => a + rs.length, 0), lines }
}

const grow = (b: Bounds | null, x: [number, number, number, number]): Bounds =>
  b ? { minX: Math.min(b.minX, x[0]), minY: Math.min(b.minY, x[1]), maxX: Math.max(b.maxX, x[2]), maxY: Math.max(b.maxY, x[3]) } : { minX: x[0], minY: x[1], maxX: x[2], maxY: x[3] }

/** The drawing from its model, or null when the source has none (the viewer reads the file itself). */
export async function loadModel(src: DrawingModelSource, metrics: TextMetrics): Promise<ModelDrawing | null> {
  const header = await src.header()
  if (!header) return null
  const model = await allPages(await src.page('model'), src, 'model')
  if (!model) return null
  const rows = model.entities
  const blocks = await loadBlocks(src, header, rows)

  let bounds: Bounds | null = null
  const index: DrawingIndex = { handles: {}, layers: {}, layerState: {} }
  for (const l of header.layers) index.layerState[l.name] = { frozen: l.frozen, off: !l.on }
  const used = new Set<string>()
  let drawn = 0
  for (const r of rows) {
    if (!r.drawn) continue
    if (r.bbox) index.handles[r.h.replace(/^@h:/i, '').toUpperCase()] = { minX: r.bbox[0], minY: r.bbox[1], maxX: r.bbox[2], maxY: r.bbox[3] }
    if (r.visible === false) continue
    drawn++
    used.add(r.layer)
    if (!r.bbox) continue
    bounds = grow(bounds, r.bbox)
    index.layers[r.layer] = grow(index.layers[r.layer] ?? null, r.bbox)
  }
  const diag = bounds ? Math.hypot(bounds.maxX - bounds.minX, bounds.maxY - bounds.minY) || 1 : 1
  const scene = { header, rows, blocks, reach: diag * 10, minSpacing: diag / 300 }
  return {
    nodes: [],
    bounds,
    drawn,
    skipped: {},
    skippedTotal: 0,
    layers: used.size,
    index,
    shapes: <ModelShapes scene={scene} m={metrics} />,
    notDrawn: notDrawnOf(rows),
    paper: header.layouts.filter((l) => l.kind === 'paper'),
    version: header.version,
    header,
    rows,
  }
}
