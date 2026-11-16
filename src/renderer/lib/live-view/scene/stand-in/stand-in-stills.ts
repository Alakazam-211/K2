// SAMPLE ONLY. The sample's PictureSource: stills drawn in the page by the
// stand-in rasteriser, in K2 Render's shape (pixels with alpha, the camera
// as view and projection matrices, the id image and its table), so the tab
// reads them exactly as it will read real ones (S7's
// DocsPreviewPictureSource). Loaded on demand with the sample. Cached by
// the doc's content and the view, so stepping back and forth redraws
// nothing. Retirement ledger: goes when real stills are proven.

import { UNITS } from '../fold'
import { stillCamera } from '../project'
import { stillHeight, type PictureSource, type Still, type StillRequest } from '../picture-source'
import type { Doc } from '../types'
import { standInCamera } from './camera'
import { rasterise } from './raster'
import { sampleBounds } from './shapes'

/** About 1920 × 1080: a tall still keeps the same pixel count (the stand-in draws on the main thread). */
const MAX_PIXELS = 1920 * 1080
const CACHE = 48

/** A short signature of what the still shows (objects, places, shapes). */
export function docSignature(D: Doc): string {
  let h = 5381
  const units = D.lists[UNITS] ?? []
  for (const a of units) {
    const p = D.props[a] ?? {}
    const s = a + (D.units[a]?.derived?.data ?? '') + (p.xform ?? '') + (p.quat ?? '') + (p.rotmode ?? '') + (p.parent ?? '') + (p.norender ?? '') + (p.edit ?? '')
    for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0
  }
  return `${(h >>> 0).toString(36)}:${units.length}:${D.pinned ? 'p' : ''}`
}

export class StandInStills implements PictureSource {
  readonly kind = 'sample' as const
  private readonly cache = new Map<string, Still>()
  private closed = false

  /** `docOf` finds the doc at a rev (the sample keeps one per rev it has seen). */
  constructor(private readonly docOf: (rev: number) => Doc | null) {}

  /** Synchronous: the still now (tests, and the cache). */
  draw(req: StillRequest): Still | null {
    const D = this.docOf(req.rev)
    if (!D) return null
    let W = Math.round(req.width)
    let H = stillHeight(W, req.shape)
    const k = Math.min(1, Math.sqrt(MAX_PIXELS / (W * H)))
    W = Math.max(16, Math.round(W * k))
    H = Math.max(9, Math.round(H * k))
    const key = `${docSignature(D)}|${req.view}|${W}x${H}`
    const hit = this.cache.get(key)
    if (hit) return { ...hit, rev: req.rev }
    const cam = standInCamera(D, req.view, W / H)
    const r = rasterise(D, cam, W, H)
    const still: Still = {
      rev: req.rev,
      view: req.view,
      shape: req.shape,
      width: W,
      height: H,
      image: { kind: 'rgba', data: r.rgba },
      camera: stillCamera(cam),
      ids: { width: W, height: H, values: r.ids, objects: r.objects },
      renderMs: r.ms,
      meta: { base_values: false, hidden_left_out: false, objects_hidden: 0, objects_not_evaluated: 0 },
    }
    this.cache.set(key, still)
    if (this.cache.size > CACHE) this.cache.delete(this.cache.keys().next().value as string)
    return still
  }

  still(req: StillRequest, signal?: AbortSignal): Promise<Still | null> {
    return new Promise((resolve) => {
      // Let the tab paint first: a 1920 px still with AO takes a moment.
      setTimeout(() => resolve(this.closed || signal?.aborted ? null : this.draw(req)), 0)
    })
  }

  bounds(D: Doc, a: string) {
    return sampleBounds(D, a)
  }

  close(): void {
    this.closed = true
    this.cache.clear()
  }
}
