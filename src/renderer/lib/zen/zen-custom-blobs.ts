// Garden blobs and K2 downloads, the renderer half (prd-garden-board-needs-v1
// GB12, GS1–GS7). One per placement's frame, beside the custom layer.
//
// - Bytes never ride `/cli/zen/call` (64 KiB bodies) or the frame's JSON
//   check (an ArrayBuffer stringifies to `{}`, V1). The frame posts them as a
//   transferred ArrayBuffer beside the message; the custom layer checks that
//   buffer itself (an ArrayBuffer, only on `blobs.put` / `downloads.save`,
//   at most 16 / 50 MiB) and this file moves it to the daemon's widget door
//   (`/cli/zen/blob/put`, `/cli/zen/blob/get`) in 8 MiB chunks. The daemon
//   runs the same gates as `/cli/zen/call` on every chunk.
// - A download never names a folder: it goes through K2's own download path
//   (`downloadChunks`: the transfer card, Cancel, the toast, a collision-free
//   name in Downloads on the desktop, `saveBlobInBrowser` on the web).

import type { DownloadSliceReader, ReadRangeResponse } from '../fs-transfer'

export const ZEN_BLOB_MAX_BYTES = 16 * 1024 * 1024
export const ZEN_DOWNLOAD_MAX_BYTES = 50 * 1024 * 1024
/** Raw bytes per chunk on the wire (the daemon takes up to 8 MiB). */
export const ZEN_BLOB_CHUNK_BYTES = 8 * 1024 * 1024

/** The verbs that may carry transferred bytes, and their caps. */
export const ZEN_BYTE_VERBS: Readonly<Record<string, number>> = {
  'blobs.put': ZEN_BLOB_MAX_BYTES,
  'downloads.save': ZEN_DOWNLOAD_MAX_BYTES,
}

/** GS3 / Q6: what `k2.download` saves (no html, zip or anything that runs;
 *  no SVG either: it runs script when opened in a browser, security review #4). */
export const ZEN_DOWNLOAD_EXTENSIONS: readonly string[] = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'pdf', 'json', 'csv', 'tsv', 'txt', 'md']

/** What the bytes are, as far as a download needs to know (review #4: the
 *  name's extension must match). Null: none of the saved kinds. */
export function zenSniffDownload(head: Uint8Array): 'png' | 'jpeg' | 'gif' | 'webp' | 'pdf' | 'text' | null {
  const b = head
  const starts = (sig: number[]): boolean => sig.every((x, i) => b[i] === x)
  if (starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return 'png'
  if (starts([0xff, 0xd8, 0xff])) return 'jpeg'
  if (starts([0x47, 0x49, 0x46, 0x38])) return 'gif'
  if (starts([0x52, 0x49, 0x46, 0x46]) && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50) return 'webp'
  if (starts([0x25, 0x50, 0x44, 0x46, 0x2d])) return 'pdf'
  try {
    // Text: well-formed UTF-8 with no NUL (a cut multi-byte tail is fine).
    const text = new TextDecoder('utf-8', { fatal: true }).decode(b.length > 64 ? b.subarray(0, b.length - 3) : b)
    if (text.includes('\u0000')) return null
    return 'text'
  } catch {
    return null
  }
}

const EXT_KIND: Readonly<Record<string, string>> = {
  png: 'png',
  jpg: 'jpeg',
  jpeg: 'jpeg',
  gif: 'gif',
  webp: 'webp',
  pdf: 'pdf',
  json: 'text',
  csv: 'text',
  tsv: 'text',
  txt: 'text',
  md: 'text',
}

const MIME_KIND: Readonly<Record<string, string>> = {
  'image/png': 'png',
  'image/jpeg': 'jpeg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'application/pdf': 'pdf',
  'application/json': 'text',
  'text/plain': 'text',
}

/** Does a saved name's extension match what the file is (review #4)? A
 *  sentence when it doesn't. `kind` is `zenSniffDownload`'s, or a blob's
 *  stored (sniffed) mime. */
export function zenDownloadMatches(name: string, what: { head: Uint8Array } | { mime: string }): string | null {
  const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase()
  const want = EXT_KIND[ext]
  const got = 'head' in what ? zenSniffDownload(what.head) : (MIME_KIND[what.mime] ?? null)
  if (!want || got !== want) {
    return `the file isn’t a .${ext} (K2 saves a file only under the extension its bytes match)`
  }
  return null
}

const MIME_EXT: Readonly<Record<string, string>> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/webp': 'webp',
  'image/gif': 'gif',
  'application/pdf': 'pdf',
  'application/json': 'json',
  'text/csv': 'csv',
  'text/tab-separated-values': 'tsv',
  'text/plain': 'txt',
  'text/markdown': 'md',
}

export const ZEN_DOWNLOAD_NAME_MAX = 120

/** GS2 / GS7: a press within 5 s (the layer's), once every 2 s, 10 a
 *  minute, one at a time; a ticket is one file for 60 s. */
export const ZEN_DOWNLOADS = { everyMs: 2_000, perMinute: 10, ticketMs: 60_000 } as const

export const ZEN_BLOB_REF = /^k2blob:sha256-[0-9a-f]{64}$/

/** An ArrayBuffer from any realm (a MessagePort clone makes one here, but
 *  tests and other realms may not share the constructor). */
export function isArrayBuffer(v: unknown): v is ArrayBuffer {
  return Object.prototype.toString.call(v) === '[object ArrayBuffer]'
}

/** Standard base64 of some bytes (chunked, so a big buffer can't blow the
 *  argument limit of `String.fromCharCode`). */
export function bytesToBase64(u8: Uint8Array): string {
  let bin = ''
  const step = 0x8000
  for (let i = 0; i < u8.length; i += step) {
    bin += String.fromCharCode(...u8.subarray(i, i + step))
  }
  return btoa(bin)
}

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/**
 * The file name a download lands as, or a sentence saying why not (GS3):
 * the basename, control characters and leading dots gone, at most 120
 * characters, with an extension on the list (one is added from the mime
 * when there is none).
 */
export function zenDownloadName(raw: unknown, mime?: unknown): { name: string } | { problem: string } {
  if (typeof raw !== 'string') return { problem: 'the file name is text' }
  const base = raw.split(/[\\/]/).pop() ?? ''
  // Control and format characters go (review #4: bidi overrides, isolates
  // and zero-width characters can make `x<RLO>gnp.svg` read as `xgvs.png`).
  let name = base.replace(/[\p{Cc}\p{Cf}]/gu, '').trim().replace(/^\.+/, '')
  if (!name) return { problem: 'the file needs a name' }
  const dot = name.lastIndexOf('.')
  let ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''
  if (!ext) {
    const fromMime = typeof mime === 'string' ? MIME_EXT[mime.split(';')[0].trim().toLowerCase()] : undefined
    if (!fromMime) return { problem: 'K2 saves images, PDFs and text files: give the name an extension (like .png)' }
    ext = fromMime
    name = `${name}.${ext}`
  }
  if (!ZEN_DOWNLOAD_EXTENSIONS.includes(ext)) {
    return { problem: `K2 saves images, PDFs and text files (${ZEN_DOWNLOAD_EXTENSIONS.join(' ')}), not .${ext}` }
  }
  if (name.length > ZEN_DOWNLOAD_NAME_MAX) {
    const keep = ZEN_DOWNLOAD_NAME_MAX - ext.length - 1
    name = `${name.slice(0, keep)}.${ext}`
  }
  return { name }
}

/** A download reader over bytes already in memory (8 MiB slices). */
export function zenBytesReader(bytes: ArrayBuffer): DownloadSliceReader {
  const u8 = new Uint8Array(bytes)
  return async (offset: number, len: number): Promise<ReadRangeResponse> => {
    const slice = u8.subarray(offset, Math.min(u8.length, offset + len))
    return { base64: bytesToBase64(slice), len: slice.length, size: u8.length, eof: offset + slice.length >= u8.length }
  }
}

/** `BlobInfo` as the daemon answers a committed upload. */
export interface ZenBlobInfo {
  ref: string
  size: number
  mime: string
  name: string | null
  new: boolean
}

/** One ranged read as the widget door answers it. */
export interface ZenBlobSlice {
  ref: string
  size: number
  mime: string
  name: string | null
  offset: number
  len: number
  eof: boolean
  base64: string
}

export interface ZenCustomBlobsDeps {
  /** `POST /cli/zen/blob/put` for this placement and the hash its frame runs. */
  put(body: Record<string, unknown>): Promise<unknown>
  /** `GET /cli/zen/blob/get` for this placement and the hash its frame runs. */
  get(params: Record<string, string | number>): Promise<unknown>
}

export interface ZenCustomBlobs {
  put(bytes: ArrayBuffer, opts: { mime?: string; name?: string }): Promise<ZenBlobInfo>
  /** A blob's stored (sniffed) mime and size, from its first bytes. */
  head(ref: string): Promise<{ mime: string; size: number }>
  /** The whole blob (or its thumbnail with `px`). */
  get(ref: string, opts: { px?: number; from?: string }): Promise<{ info: Omit<ZenBlobSlice, 'offset' | 'len' | 'eof' | 'base64'>; bytes: ArrayBuffer }>
  /** A download reader for a blob this widget's data holds (GS4: the bytes
   *  go daemon → Downloads and never round-trip through the frame). */
  reader(ref: string): DownloadSliceReader
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function asSlice(v: unknown): ZenBlobSlice {
  if (!isObj(v) || typeof v.base64 !== 'string' || typeof v.len !== 'number' || typeof v.eof !== 'boolean') {
    throw new Error('The daemon answered a blob read K2 doesn’t understand.')
  }
  return v as unknown as ZenBlobSlice
}

export function createZenCustomBlobs(deps: ZenCustomBlobsDeps): ZenCustomBlobs {
  const put = async (bytes: ArrayBuffer, opts: { mime?: string; name?: string }): Promise<ZenBlobInfo> => {
    const u8 = new Uint8Array(bytes)
    let upload: string | undefined
    let offset = 0
    for (;;) {
      const end = Math.min(u8.length, offset + ZEN_BLOB_CHUNK_BYTES)
      const last = end >= u8.length
      const body: Record<string, unknown> = { offset, base64: bytesToBase64(u8.subarray(offset, end)), last }
      if (upload) body.upload = upload
      if (opts.mime) body.mime = opts.mime
      if (opts.name) body.name = opts.name
      const r = await deps.put(body)
      if (last) {
        if (!isObj(r) || typeof r.ref !== 'string') throw new Error('The daemon didn’t answer the upload with a ref.')
        return r as unknown as ZenBlobInfo
      }
      if (!isObj(r) || typeof r.upload !== 'string') throw new Error('The daemon didn’t open the upload.')
      upload = r.upload
      offset = end
    }
  }

  const slice = (ref: string, offset: number, len: number, extra: Record<string, string | number>): Promise<ZenBlobSlice> =>
    deps.get({ ref, offset, len, ...extra }).then(asSlice)

  const get = async (
    ref: string,
    opts: { px?: number; from?: string },
  ): Promise<{ info: Omit<ZenBlobSlice, 'offset' | 'len' | 'eof' | 'base64'>; bytes: ArrayBuffer }> => {
    const extra: Record<string, string | number> = {}
    if (opts.px !== undefined) extra.px = opts.px
    if (opts.from) extra.from = opts.from
    const first = await slice(ref, 0, ZEN_BLOB_CHUNK_BYTES, extra)
    const out = new Uint8Array(first.size)
    let at = 0
    let s = first
    for (;;) {
      const b = base64ToBytes(s.base64)
      if (at + b.length > out.length) throw new Error('The blob is longer than its size.')
      out.set(b, at)
      at += b.length
      if (s.eof) break
      if (b.length === 0) throw new Error('The daemon answered an empty slice before the end.')
      s = await slice(ref, at, ZEN_BLOB_CHUNK_BYTES, extra)
    }
    if (at !== out.length) throw new Error('The blob ended before its size.')
    return { info: { ref: first.ref, size: first.size, mime: first.mime, name: first.name }, bytes: out.buffer }
  }

  const head = async (ref: string): Promise<{ mime: string; size: number }> => {
    const s = await slice(ref, 0, 1, {})
    return { mime: s.mime, size: s.size }
  }

  const reader = (ref: string): DownloadSliceReader => async (offset, len) => {
    const s = await slice(ref, offset, len, {})
    return { base64: s.base64, len: s.len, size: s.size, eof: s.eof }
  }

  return { put, head, get, reader }
}
