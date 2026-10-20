// Pictures of image attachments on Thread messages (S3, daemon feature
// `thread-images-v1`; eval-thread-file-objects-v1 §3.3 / §6 S3).
//
// The daemon answers `GET /cli/thread/attachment?addr=&id=&n=&mode=thumb&px=`
// with `{kind:"image", mime, width, height, base64, …}`: a JPEG/PNG thumbnail
// it made (an SVG comes back sanitized). `mode=preview` answers the whole
// image the same way (≤ 20 MB). Both go through the room's own server scope,
// so a Thread on another server reads its pictures from that server.
//
// The bytes become a `blob:` object URL that only ever lands in an
// `<img src>` (never inline SVG markup). Thumbnails are kept in a small
// module-level LRU keyed by (server, Thread, message, index, modifiedAt,
// px): a re-render, a remount or the render window bringing a row back
// never fetches again, and an evicted URL is revoked. A refusal that will
// not change for the same key (415 no thumbnail, 404 gone, 403 not
// granted, 413 too large) is remembered too, so a chip that fell back stays
// a chip without asking again; anything else (network, a host switch) is
// forgotten so the next mount tries again.

import { daemonCliGet } from '@/lib/daemon-cli'
import { httpCodeOf, httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'

/** The daemon's attachment route. */
export const THREAD_ATTACHMENT_ROUTE = 'thread/attachment'

/** Image types the daemon makes a picture of (its extension table). */
export const THREAD_IMAGE_MIMES: ReadonlySet<string> = new Set([
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
  'image/heic',
  'image/heif',
  'image/svg+xml',
  'image/bmp',
  'image/tiff',
])

/** What a thumbnail / preview answer may carry: never anything but a
 *  picture type (an SVG thumbnail is already sanitized by the daemon and
 *  still only ever drawn through `<img>`). */
const ANSWER_MIMES: ReadonlySet<string> = new Set([...THREAD_IMAGE_MIMES])

const IMAGE_EXT = /\.(png|jpe?g|gif|webp|heic|heif|svg|bmp|tiff?)$/i

/** Thumbnail edge asked for: 2× the ≤ 240 px card, so it stays crisp. */
export const THREAD_THUMB_PX = 512

/** How many thumbnails stay cached (object URLs) before the oldest goes. */
export const THREAD_THUMB_CACHE_MAX = 64

/** Is this file object a picture the daemon can draw? Its `mime` (the
 *  daemon's), else its name's extension (an older doc without a mime). */
export function isThreadImageFile(f: Pick<ThreadFileObject, 'mime' | 'name' | 'path'>): boolean {
  const mime = (f.mime ?? '').toLowerCase().split(';')[0].trim()
  if (mime) return THREAD_IMAGE_MIMES.has(mime)
  return IMAGE_EXT.test(f.name ?? f.path ?? '')
}

/** A picture the daemon answered, ready for `<img src>`. */
export interface ThreadImage {
  url: string
  width: number | null
  height: number | null
  mime: string
}

/** Why there is no picture: the daemon said so (`code` = its refusal), or
 *  the request failed. */
export class ThreadImageError extends Error {
  readonly status: number | null
  readonly code: string | null
  /** The same key will answer the same: remembered, never asked again. */
  readonly settled: boolean
  constructor(message: string, status: number | null, code: string | null, settled: boolean) {
    super(message)
    this.name = 'ThreadImageError'
    this.status = status
    this.code = code
    this.settled = settled
  }
}

/** Statuses that will not change for the same (message, index, mtime). */
const SETTLED_STATUSES: ReadonlySet<number> = new Set([403, 404, 413, 415])

export interface ThreadImageRef {
  scope: ServerScope
  /** The Thread's address (or its conversation id). */
  addr: string
  /** The message (doc) id. */
  docId: string
  /** The attachment's index on the message. */
  n: number
  modifiedAt?: number
}

function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/** Turn one daemon answer into an object URL; anything that is not a
 *  picture answer is refused (settled: asking again won't change it). */
export function threadImageFromAnswer(raw: unknown): ThreadImage {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>
  const mime = typeof r.mime === 'string' ? r.mime.toLowerCase() : ''
  if (r.kind !== 'image' || typeof r.base64 !== 'string' || !r.base64 || !ANSWER_MIMES.has(mime)) {
    throw new ThreadImageError('the server sent no picture for this file', 415, 'no_thumbnail', true)
  }
  const bytes = decodeBase64(r.base64)
  const blob = new Blob([bytes as BlobPart], { type: mime })
  return {
    url: URL.createObjectURL(blob),
    width: typeof r.width === 'number' && r.width > 0 ? r.width : null,
    height: typeof r.height === 'number' && r.height > 0 ? r.height : null,
    mime,
  }
}

function asImageError(err: unknown): ThreadImageError {
  if (err instanceof ThreadImageError) return err
  const status = httpStatusOf(err)
  const code = httpCodeOf(err)
  const msg = err instanceof Error ? err.message : String(err)
  return new ThreadImageError(msg, status, code, status !== null && SETTLED_STATUSES.has(status))
}

/** One `GET /cli/thread/attachment` for a picture (`thumb` at `px`, or the
 *  whole image with `preview`). Not cached: the caller owns the URL. */
export async function fetchThreadImage(
  ref: ThreadImageRef,
  mode: 'thumb' | 'preview',
  px: number = THREAD_THUMB_PX,
): Promise<ThreadImage> {
  let raw: unknown
  try {
    raw = await daemonCliGet<unknown>(ref.scope, THREAD_ATTACHMENT_ROUTE, {
      addr: ref.addr,
      id: ref.docId,
      n: ref.n,
      mode,
      ...(mode === 'thumb' ? { px } : {}),
    })
  } catch (err) {
    throw asImageError(err)
  }
  return threadImageFromAnswer(raw)
}

// ── The thumbnail LRU ────────────────────────────────────────────────────

interface CacheEntry {
  promise: Promise<ThreadImage>
  /** Set once it resolved: what eviction revokes. */
  image: ThreadImage | null
}

/** Insertion order = recency (a hit moves the key to the end). */
const thumbs = new Map<string, CacheEntry>()

export function threadThumbKey(ref: ThreadImageRef, px: number = THREAD_THUMB_PX): string {
  return [ref.scope.hostKey, ref.addr, ref.docId, ref.n, ref.modifiedAt ?? '', px].join('\u0000')
}

function revokeEntry(entry: CacheEntry): void {
  if (entry.image) URL.revokeObjectURL(entry.image.url)
  else entry.promise.then((img) => URL.revokeObjectURL(img.url), () => undefined)
}

function evictOverflow(): void {
  while (thumbs.size > THREAD_THUMB_CACHE_MAX) {
    const oldest = thumbs.keys().next().value as string
    const entry = thumbs.get(oldest)
    thumbs.delete(oldest)
    if (entry) revokeEntry(entry)
  }
}

/** The thumbnail already in the cache for `ref` (resolved), or null. */
export function peekThreadThumb(ref: ThreadImageRef, px: number = THREAD_THUMB_PX): ThreadImage | null {
  return thumbs.get(threadThumbKey(ref, px))?.image ?? null
}

/** The thumbnail for `ref`: from the cache, or one fetch shared by every
 *  card that asks while it is in flight. A settled refusal stays cached
 *  (rejected); any other failure is dropped so the next ask retries. */
export function loadThreadThumb(ref: ThreadImageRef, px: number = THREAD_THUMB_PX): Promise<ThreadImage> {
  const key = threadThumbKey(ref, px)
  const hit = thumbs.get(key)
  if (hit) {
    thumbs.delete(key)
    thumbs.set(key, hit)
    return hit.promise
  }
  const entry = { image: null } as CacheEntry
  entry.promise = fetchThreadImage(ref, 'thumb', px).then(
    (img) => {
      if (thumbs.get(key) === entry) entry.image = img
      else URL.revokeObjectURL(img.url) // evicted while in flight
      return img
    },
    (err: unknown) => {
      const e = asImageError(err)
      if (!e.settled && thumbs.get(key) === entry) thumbs.delete(key)
      throw e
    },
  )
  // A cached rejection must not surface as an unhandled one.
  entry.promise.catch(() => undefined)
  thumbs.set(key, entry)
  evictOverflow()
  return entry.promise
}

/** Test-only: empty the cache (revoking what it held). */
export function __resetThreadThumbsForTests(): void {
  for (const entry of thumbs.values()) revokeEntry(entry)
  thumbs.clear()
}

/** Test-only: how many thumbnails are cached. */
export function __threadThumbCacheSize(): number {
  return thumbs.size
}
