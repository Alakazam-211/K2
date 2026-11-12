// Live deck previews (V3): the daemon draws each slide of a deck opened live
// by the Slides core and serves the images with the doc's read right
// (`GET /cli/docs/preview`, `crates/k2-daemon/src/docs/preview/route.rs`).
// This file only reads its answers: the daemon decides what is current,
// draws only the slides that changed, and keeps the cache.
//
// - no `container`: the slides at the head, each with its render hash and
//   whether its thumb and full images are drawn (`ready`, `rendering`,
//   `failed`, `none`); `want: 'thumb'` asks for the missing thumbs.
// - `container` + `width`: one image, as base64 with its metadata, `ready`;
//   or `rendering` (202), with the slide's previous image as `stale` when
//   there is one; or `failed` with a code.
//
// Images become object URLs: no token ever goes in an `<img src>`.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export type PreviewWidth = 'thumb' | 'full'
export type PreviewState = 'ready' | 'rendering' | 'failed' | 'none'

export interface DeckSlide {
  /** The slide's address (`@sld:256`). */
  key: string
  position: number
  title: string
  hidden: boolean
  renderHash: string
  thumb: PreviewState
  full: PreviewState
}

export interface DeckListing {
  rev: number
  page: { w: number; h: number; unit: string } | null
  slides: DeckSlide[]
}

export interface PreviewImage {
  state: 'ready' | 'stale' | 'rendering' | 'failed'
  /** The hash of the image's bytes (for `stale`, the older one drawn). */
  renderHash: string
  bytes: Uint8Array | null
  mime: string | null
  width: number | null
  height: number | null
  failedCode: string | null
}

const STATES: ReadonlySet<string> = new Set(['ready', 'rendering', 'failed', 'none'])

function obj(v: unknown, what: string): Record<string, unknown> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error(`docs/preview: ${what} is not an object`)
  return v as Record<string, unknown>
}

function str(o: Record<string, unknown>, k: string, what: string): string {
  const v = o[k]
  if (typeof v !== 'string') throw new Error(`docs/preview: ${what}.${k} is not a string`)
  return v
}

function state(v: unknown, what: string): PreviewState {
  if (typeof v !== 'string' || !STATES.has(v)) throw new Error(`docs/preview: ${what} is not a preview state`)
  return v as PreviewState
}

/** The listing answer; loud on a shape this client doesn't know. */
export function parseDeckListing(body: unknown): DeckListing {
  const b = obj(body, 'the answer')
  if (b.kind !== 'deck') throw new Error(`docs/preview: kind ${String(b.kind)} is not a deck`)
  if (!Array.isArray(b.containers)) throw new Error('docs/preview: no containers')
  const page = b.page === null || b.page === undefined ? null : obj(b.page, 'page')
  return {
    rev: typeof b.rev === 'number' ? b.rev : 0,
    page: page ? { w: Number(page.w), h: Number(page.h), unit: String(page.unit) } : null,
    slides: b.containers.map((c, i) => {
      const o = obj(c, `containers[${i}]`)
      const previews = obj(o.previews, `containers[${i}].previews`)
      return {
        key: str(o, 'key', 'container'),
        position: typeof o.position === 'number' ? o.position : i + 1,
        title: typeof o.title === 'string' ? o.title : '',
        hidden: o.hidden === true,
        renderHash: str(o, 'render_hash', 'container'),
        thumb: state(previews.thumb, 'previews.thumb'),
        full: state(previews.full, 'previews.full'),
      }
    }),
  }
}

function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/** One image answer; loud on a shape this client doesn't know. */
export function parsePreviewImage(body: unknown): PreviewImage {
  const b = obj(body, 'the answer')
  const s = b.state
  if (s !== 'ready' && s !== 'stale' && s !== 'rendering' && s !== 'failed') {
    throw new Error(`docs/preview: state ${String(s)} is not an image state`)
  }
  const hasImage = s === 'ready' || s === 'stale'
  if (hasImage && typeof b.base64 !== 'string') throw new Error('docs/preview: an image answer without its bytes')
  const size = hasImage ? obj(b.size, 'size') : null
  const failed = s === 'failed' ? obj(b.failed, 'failed') : null
  return {
    state: s,
    renderHash: str(b, 'render_hash', 'the answer'),
    bytes: hasImage ? decodeBase64(b.base64 as string) : null,
    mime: hasImage && typeof b.mime === 'string' ? b.mime : null,
    width: size && typeof size.w === 'number' ? size.w : null,
    height: size && typeof size.h === 'number' ? size.h : null,
    failedCode: failed && typeof failed.code === 'string' ? failed.code : null,
  }
}

/** The deck's slides at the head; `want` asks for the missing images at that width. */
export async function fetchDeckListing(scope: ServerScope, path: string, want?: PreviewWidth): Promise<DeckListing> {
  return parseDeckListing(await daemonCliGet<unknown>(scope, 'docs/preview', want ? { path, want } : { path }))
}

/** One slide's image at `width` (queued when it isn't drawn yet). */
export async function fetchPreviewImage(scope: ServerScope, path: string, container: string, width: PreviewWidth): Promise<PreviewImage> {
  return parsePreviewImage(await daemonCliGet<unknown>(scope, 'docs/preview', { path, container, width }))
}
