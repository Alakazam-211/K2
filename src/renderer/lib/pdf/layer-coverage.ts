// What each layer actually covers. Many CAD exports keep hundreds of layer
// names as optional content groups that no content is marked with, so a
// switch for one changes nothing. Each page's operator list (pdf.js) says
// which drawing sits inside optional content: page content marked
// /OC … BDC, and XObjects, images and annotations with an /OC, all of which
// pdf.js turns into beginMarkedContentProps("OC", …). We count the drawing
// operations under each group (a path, a run of text, an image, a form),
// page by page, and only call a layer empty once every page is counted.

import type { PdfLayer } from './layers'

export interface OpList {
  fnArray: number[]
  argsArray: unknown[]
}

/** The operator codes the count needs (pdf.js's OPS). */
export type OpCodes = Record<string, number>

const PAINT = [
  'constructPath',
  'stroke',
  'closeStroke',
  'fill',
  'eoFill',
  'fillStroke',
  'eoFillStroke',
  'closeFillStroke',
  'closeEOFillStroke',
  'shadingFill',
  'showText',
  'showSpacedText',
  'nextLineShowText',
  'nextLineSetSpacingShowText',
  'paintFormXObjectBegin',
  'paintImageXObject',
  'paintInlineImageXObject',
  'paintImageMaskXObject',
  'paintImageXObjectRepeat',
  'paintImageMaskXObjectRepeat',
  'paintImageMaskXObjectGroup',
  'paintInlineImageXObjectGroup',
  'paintSolidColorImageMask',
]

/** The groups an /OC names: one OCG, or every group an OCMD's list or expression mentions. */
function groupsOf(oc: unknown): string[] {
  if (!oc || typeof oc !== 'object') return []
  const o = oc as { type?: string; id?: string; ids?: string[]; expression?: unknown[] }
  if (o.type === 'OCG' && o.id) return [o.id]
  if (Array.isArray(o.ids)) return o.ids
  const out: string[] = []
  const walk = (e: unknown): void => {
    if (Array.isArray(e)) e.forEach(walk)
    else if (typeof e === 'string' && e !== 'And' && e !== 'Or' && e !== 'Not') out.push(e)
  }
  walk(o.expression)
  return out
}

/** Drawing operations per group on one page. A group inside another counts for both. */
export function countCoverage(ops: OpList, OPS: OpCodes, into = new Map<string, number>()): Map<string, number> {
  const paint = new Set(PAINT.map((n) => OPS[n]).filter((n) => n !== undefined))
  const begin = OPS.beginMarkedContent
  const beginProps = OPS.beginMarkedContentProps
  const end = OPS.endMarkedContent
  // One entry per open marked-content section: the groups it adds (none for a plain one).
  const stack: string[][] = []
  for (let i = 0; i < ops.fnArray.length; i++) {
    const fn = ops.fnArray[i]
    if (fn === begin) stack.push([])
    else if (fn === beginProps) {
      const args = ops.argsArray[i] as unknown[] | null
      stack.push(args && args[0] === 'OC' ? groupsOf(args[1]) : [])
    } else if (fn === end) stack.pop()
    else if (paint.has(fn) && stack.length) {
      const open = new Set(stack.flat())
      for (const id of open) into.set(id, (into.get(id) ?? 0) + 1)
    }
  }
  return into
}

export interface Coverage {
  /** Pages counted so far, of `total`. */
  scanned: number
  total: number
  counts: ReadonlyMap<string, number>
}

export interface CoveredLayer extends PdfLayer {
  /** Drawing operations under it so far. */
  items: number
}

export interface LayerView {
  /** Still counting: every layer lists normally, none is empty yet. */
  checking: boolean
  /** Layers that cover something (all of them while checking). */
  used: CoveredLayer[]
  /** Layers with nothing on them, known once every page is counted. */
  empty: CoveredLayer[]
}

export function layerView(layers: readonly PdfLayer[], cov: Coverage | null): LayerView {
  const checking = !cov || cov.scanned < cov.total
  const all = layers.map((l) => ({ ...l, items: cov?.counts.get(l.id) ?? 0 }))
  if (checking) return { checking, used: all, empty: [] }
  return { checking, used: all.filter((l) => l.items > 0), empty: all.filter((l) => l.items === 0) }
}

export const itemsWord = (n: number): string => `${n.toLocaleString('en-GB')} ${n === 1 ? 'item' : 'items'}`

/** The status line's words, from what the layers cover. */
export function coverageSummary(view: LayerView): string {
  const { used, empty } = view
  if (used.length === 0 && empty.length === 0) return 'No layers'
  if (used.length === 0) return `${empty.length} ${empty.length === 1 ? 'layer' : 'layers'}, none in use`
  const on = used.filter((l) => l.visible).length
  return `${on} of ${used.length} ${used.length === 1 ? 'layer' : 'layers'}`
}
