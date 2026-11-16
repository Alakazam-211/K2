// The scene's picture: where the tab gets a still. K2 Render draws a clay
// still per (doc rev, view): lossless WebP with alpha (`look=transparent`,
// the tab paints the dark stage behind it), AO, 4:3 or 16:9 (the scene and
// `@obj` at 1920×1440 or 1920×1080, `shape=wide|tall` coming), the camera
// that drew it, and an object-id image beside it (`layer=ids`).
//
// Two sources implement it: the sample's stand-in (stand-in/, a small
// rasteriser, sample only) and, in S7, DocsPreviewPictureSource (the docs
// preview route). What the picture is doing (drawing, ready, failed, none)
// is not the source's: K2 Render pushes `preview_rendering` and
// `preview_ready {state, render_ms, without_modifiers, look, reused_from,
// camera, ids}` on the docs socket, and the live source folds them into the
// snapshot as a PictureStatus, the way the sample's simulation does.

import type { IdLayer } from './ids'
import type { StillCamera } from './project'
import type { Doc } from './types'

/** K2 Render's view address (view-address.ts): `@scene:main`, `@cam:<enc(id)>`, `@obj:<enc(id)>`, `@turntable:<0-7>`, `@cam-n:<i>` / `@obj-n:<i>`. */
export type ViewAddress = string

/** 16:9, or 4:3 for a tall pane. */
export type StillShape = 'wide' | 'tall'
export const SHAPE_ASPECT: Record<StillShape, number> = { wide: 16 / 9, tall: 4 / 3 }

/** The width K2 Render draws the picture at, and a thumbnail's. */
export const STILL_WIDTH = 1920
export const THUMB_WIDTH = 320

export interface StillRequest {
  rev: number
  view: ViewAddress
  shape: StillShape
  width: number
  /** Also fetch the id image (overlays and hover). */
  ids?: boolean
}

/** A still's pixels: RGBA the tab draws (the stand-in) or an image the browser decoded (a real still). */
export type StillImage = { kind: 'rgba'; data: Uint8ClampedArray } | { kind: 'bitmap'; image: CanvasImageSource }

export interface Still {
  rev: number
  view: ViewAddress
  shape: StillShape
  width: number
  height: number
  /** With alpha: nothing drawn is transparent. */
  image: StillImage
  /** The camera that drew it; null when K2 Render had none (a degenerate scene). */
  camera: StillCamera | null
  /** The id image and table, when asked for and drawn with one. */
  ids: IdLayer | null
  renderMs: number | null
  /** The response meta; null when the source has none. */
  meta?: StillMeta | null
}

/** An object in K2 Render's listing: its local bounds (mesh-local, from the mesh read), read by listing-read.ts. */
export interface ObjectBounds {
  min: number[]
  max: number[]
  /** The bounds miss geometry: the box is dashed. A bool; the why is `approx_reason`. */
  approx?: boolean
  approx_reason?: 'modifiers' | 'driven' | null
  /** Hidden in the file: no clay, and a dashed "(hidden)" ghost box only while an agent acts on or holds it. */
  hidden?: boolean
  /** What hides it ("self", "collection"). */
  hidden_by?: string | null
}

/** A still's response meta: what K2 Render left out of it (counts are 0 when nothing was). */
export interface StillMeta {
  /** Drawn from base values (provisional: modelled, not shown yet). */
  base_values: boolean
  /** Hidden objects were left out of the picture. */
  hidden_left_out: boolean
  objects_hidden: number
  objects_not_evaluated: number
}

export interface PictureSource {
  readonly kind: 'sample' | 'docs-preview'
  /** The still of `rev` from `view`. Null when this view has no still (the snapshot's PictureStatus says why). Cached by (rev, view, shape, width). */
  still(req: StillRequest, signal?: AbortSignal): Promise<Still | null>
  /** An object's local bounds, for its box over the picture; null when it isn't drawn. Absent: no boxes. */
  bounds?(D: Doc, a: string): ObjectBounds | null
  close(): void
}

/** Keep a still's big arrays (its pixels and id values: millions of indexes at 1920 px) out of
 *  enumeration. React's dev build walks every changed prop's keys into its performance track and the
 *  browser keeps those measures, so an enumerable pixel array cost gigabytes a step. Still readable. */
export function sealStill(st: Still): Still {
  if (st.image.kind === 'rgba') Object.defineProperty(st.image, 'data', { enumerable: false })
  if (st.ids) Object.defineProperty(st.ids, 'values', { enumerable: false })
  return st
}

/** The picture's state, folded from K2 Render's pushes (preview_rendering, preview_ready). */
export interface PictureStatus {
  /** The rev of the still on screen; null before the first one. */
  shown: number | null
  /** The rev wanted (newest asked for). */
  want: number | null
  /** ready; drawing a newer one over an older still (stale); drawing the first (rendering); the newest failed; no picture can be made (nopic); none yet. */
  state: 'none' | 'ready' | 'rendering' | 'stale' | 'failed' | 'nopic'
  renderMs: number | null
  reusedFrom: number | null
  withoutModifiers: boolean
  reason: string | null
  /** A new still just landed (this step). */
  landed: boolean
  /** It landed after a failure. */
  recovered: boolean
  /** Drawing again after a failure. */
  retrying: boolean
  /** Failed again after a retry. */
  again: boolean
}

/** Tall when the pane is narrower than 3:2 (the narrow window, or 1440 with the panel open in two views). */
export const shapeFor = (w: number, h: number): StillShape => (h > 0 && w / h < 1.5 ? 'tall' : 'wide')

export interface Letterbox {
  shape: StillShape
  /** The still's box in the pane, in px: never cropped, centred, inside a margin. */
  left: number
  top: number
  width: number
  height: number
  /** It covers its pane (cropped), so its edges aren't feathered. */
  cover?: boolean
}

/** The still's place in a pane: a `pad` margin all round (its edges fade into the stage), the shape the pane asks for, letterboxed and centred. */
export function letterbox(paneW: number, paneH: number, pad = 16): Letterbox {
  const W = Math.max(0, paneW - 2 * pad)
  const H = Math.max(0, paneH - 2 * pad)
  const shape = shapeFor(W, H)
  const k = SHAPE_ASPECT[shape]
  let w = W
  let h = W / k
  if (h > H) {
    h = H
    w = H * k
  }
  return { shape, left: pad + (W - w) / 2, top: pad + (H - h) / 2, width: w, height: h }
}

/** The still covering a pane (a view in half the stage): the shape nearest the pane's, scaled to fill it, centred, cropped where it overflows. No margin and no feathered edge: the pane's edges are the picture's. */
export function coverBox(paneW: number, paneH: number): Letterbox {
  const shape = shapeFor(paneW, paneH)
  const k = SHAPE_ASPECT[shape]
  let w = paneW
  let h = paneW / k
  if (h < paneH) {
    h = paneH
    w = paneH * k
  }
  return { shape, left: (paneW - w) / 2, top: (paneH - h) / 2, width: w, height: h, cover: true }
}

/** A still's height for a width and shape. */
export const stillHeight = (width: number, shape: StillShape): number => Math.round(width / SHAPE_ASPECT[shape])
