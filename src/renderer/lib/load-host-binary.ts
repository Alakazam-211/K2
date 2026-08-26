// Host-aware binary loader for file-viewer previews (images, PDF, media).
//
// Always goes through the daemon (`fs/read-binary` / `fs/read-range`) so
// local, remote, and web clients share one path. NEVER use
// `convertFileSrc` for host files — it breaks remote + web.
//
// `read-binary` is capped at 50 MB server-side. Larger media can be
// assembled via ranged reads when `allowRangeAssembly` is true.

import { daemonCliGet } from '@/lib/daemon-cli'
import { throwIfRemoteMacTmp } from '@/lib/remote-mac-tmp'

/** Matches `k2_core::fs_commands::MAX_BINARY_SIZE`. */
export const READ_BINARY_MAX_BYTES = 50 * 1024 * 1024

/** Chunk size for ranged assembly — under the daemon's per-request cap. */
const RANGE_CHUNK_BYTES = 8 * 1024 * 1024

export interface ReadRangeResponse {
  base64: string
  len: number
  size: number
  eof: boolean
}

export interface LoadHostBinaryOptions {
  /**
   * When true and `read-binary` fails for size (or the file is known
   * large), loop `fs/read-range` to assemble the full file. Used by
   * audio/video; leave false for images/PDF/DOCX that should fail loud.
   */
  allowRangeAssembly?: boolean
  /** Optional progress callback (0..1) during ranged assembly. */
  onProgress?: (ratio: number) => void
  /** Abort when the signal is aborted. */
  signal?: AbortSignal
}

export class HostBinaryTooLargeError extends Error {
  readonly sizeBytes: number | null

  constructor(message: string, sizeBytes: number | null = null) {
    super(message)
    this.name = 'HostBinaryTooLargeError'
    this.sizeBytes = sizeBytes
  }
}

export function decodeBase64ToUint8Array(base64: string): Uint8Array {
  // Guard: daemonCliGet must return `{ base64: string }`. A missing field
  // used to produce garbage via `atob(undefined)` → empty/corrupt images
  // that fail with "Browser could not decode this image".
  if (typeof base64 !== 'string' || base64.length === 0) {
    throw new Error('fs/read-binary returned empty or missing base64 payload')
  }
  // Strip whitespace/newlines some proxies insert.
  const clean = base64.replace(/\s+/g, '')
  const binary = atob(clean)
  const data = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) data[i] = binary.charCodeAt(i)
  return data
}

/**
 * Sniff image MIME from magic bytes. Prefer this over extension-only MIME:
 * misnamed files and `.ico` containers (often embed PNG) decode more
 * reliably in WKWebView when the Blob type matches the payload.
 */
export function sniffImageMime(bytes: Uint8Array): string | null {
  if (bytes.length < 4) return null
  // PNG
  if (
    bytes[0] === 0x89 &&
    bytes[1] === 0x50 &&
    bytes[2] === 0x4e &&
    bytes[3] === 0x47
  ) {
    return 'image/png'
  }
  // JPEG
  if (bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
    return 'image/jpeg'
  }
  // GIF
  if (
    bytes[0] === 0x47 &&
    bytes[1] === 0x49 &&
    bytes[2] === 0x46 &&
    bytes[3] === 0x38
  ) {
    return 'image/gif'
  }
  // WEBP: RIFF....WEBP
  if (
    bytes.length >= 12 &&
    bytes[0] === 0x52 &&
    bytes[1] === 0x49 &&
    bytes[2] === 0x46 &&
    bytes[3] === 0x46 &&
    bytes[8] === 0x57 &&
    bytes[9] === 0x45 &&
    bytes[10] === 0x42 &&
    bytes[11] === 0x50
  ) {
    return 'image/webp'
  }
  // BMP
  if (bytes[0] === 0x42 && bytes[1] === 0x4d) return 'image/bmp'
  // ICO / CUR: 00 00 01 00 or 00 00 02 00
  if (
    bytes[0] === 0x00 &&
    bytes[1] === 0x00 &&
    (bytes[2] === 0x01 || bytes[2] === 0x02) &&
    bytes[3] === 0x00
  ) {
    return 'image/x-icon'
  }
  // SVG (text)
  const head = new TextDecoder('utf-8', { fatal: false })
    .decode(bytes.subarray(0, Math.min(256, bytes.length)))
    .trimStart()
  if (head.startsWith('<svg') || head.startsWith('<?xml')) {
    return 'image/svg+xml'
  }
  return null
}

/**
 * Prefer a browser-friendly payload for ICO: many modern favicons embed a
 * PNG. WKWebView often fails to paint multi-image ICO Blobs, but a bare PNG
 * works. Returns the original bytes when no embedded PNG is found.
 */
export function coerceImageBytesForPreview(bytes: Uint8Array): {
  bytes: Uint8Array
  mime: string
} {
  const sniffed = sniffImageMime(bytes)
  // Already a simple raster/svg — use sniffed MIME.
  if (
    sniffed === 'image/png' ||
    sniffed === 'image/jpeg' ||
    sniffed === 'image/gif' ||
    sniffed === 'image/webp' ||
    sniffed === 'image/bmp' ||
    sniffed === 'image/svg+xml'
  ) {
    return { bytes, mime: sniffed }
  }
  // ICO container: scan for an embedded PNG (common for favicon.ico).
  if (sniffed === 'image/x-icon' || sniffed === null) {
    const png = extractPngFromIco(bytes)
    if (png) return { bytes: png, mime: 'image/png' }
  }
  if (sniffed === 'image/x-icon') {
    // Fall back to vnd.microsoft.icon — sometimes accepted where x-icon is not.
    return { bytes, mime: 'image/vnd.microsoft.icon' }
  }
  return { bytes, mime: sniffed ?? 'application/octet-stream' }
}

/** Pull the first PNG payload out of an ICO, if any entry is PNG-encoded. */
function extractPngFromIco(bytes: Uint8Array): Uint8Array | null {
  if (bytes.length < 6) return null
  if (!(bytes[0] === 0x00 && bytes[1] === 0x00 && bytes[3] === 0x00)) return null
  if (!(bytes[2] === 0x01 || bytes[2] === 0x02)) return null
  const count = bytes[4] | (bytes[5] << 8)
  if (count === 0 || count > 64) return null
  let best: Uint8Array | null = null
  let bestSize = 0
  for (let i = 0; i < count; i++) {
    const entry = 6 + i * 16
    if (entry + 16 > bytes.length) break
    const size =
      bytes[entry + 8] |
      (bytes[entry + 9] << 8) |
      (bytes[entry + 10] << 16) |
      (bytes[entry + 11] << 24)
    const offset =
      bytes[entry + 12] |
      (bytes[entry + 13] << 8) |
      (bytes[entry + 14] << 16) |
      (bytes[entry + 15] << 24)
    if (size <= 0 || offset < 0 || offset + size > bytes.length) continue
    const slice = bytes.subarray(offset, offset + size)
    // PNG magic inside the image blob
    if (
      slice.length >= 8 &&
      slice[0] === 0x89 &&
      slice[1] === 0x50 &&
      slice[2] === 0x4e &&
      slice[3] === 0x47
    ) {
      if (size > bestSize) {
        bestSize = size
        best = slice
      }
    }
  }
  if (!best) return null
  // Copy out of the parent buffer so Blob construction is unambiguous.
  const out = new Uint8Array(best.length)
  out.set(best)
  return out
}

function isAbortError(err: unknown): boolean {
  return (
    (typeof DOMException !== 'undefined' &&
      err instanceof DOMException &&
      err.name === 'AbortError') ||
    (err instanceof Error && err.name === 'AbortError')
  )
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    const err = new DOMException('Aborted', 'AbortError')
    throw err
  }
}

function looksLikeTooLarge(message: string): boolean {
  const m = message.toLowerCase()
  return (
    m.includes('too large') ||
    m.includes('>50mb') ||
    m.includes('50mb') ||
    m.includes('file too large')
  )
}

/**
 * Load host file bytes via daemon. Prefer single-shot `fs/read-binary`;
 * optionally fall back to ranged assembly for large A/V.
 */
export async function loadHostBinary(
  path: string,
  options: LoadHostBinaryOptions = {},
): Promise<Uint8Array> {
  const { allowRangeAssembly = false, onProgress, signal } = options
  throwIfAborted(signal)
  throwIfRemoteMacTmp(path)

  try {
    const r = await daemonCliGet<{ base64?: string } | string>('fs/read-binary', {
      path,
    })
    throwIfAborted(signal)
    onProgress?.(1)
    // Defensive: never pass a non-string into atob.
    const b64 =
      typeof r === 'string'
        ? r
        : r && typeof r === 'object' && typeof r.base64 === 'string'
          ? r.base64
          : null
    if (b64 === null) {
      throw new Error(
        `fs/read-binary: unexpected response shape (expected { base64 }) for ${path}`,
      )
    }
    return decodeBase64ToUint8Array(b64)
  } catch (err) {
    if (isAbortError(err)) throw err
    const message = err instanceof Error ? err.message : String(err)
    if (!allowRangeAssembly || !looksLikeTooLarge(message)) {
      throw err instanceof Error ? err : new Error(message)
    }
  }

  return loadHostBinaryViaRange(path, { onProgress, signal })
}

/**
 * Assemble a full file by looping `fs/read-range`. No size hard-cap here —
 * callers decide whether to attempt this (media viewers typically cap UX).
 */
export async function loadHostBinaryViaRange(
  path: string,
  options: Pick<LoadHostBinaryOptions, 'onProgress' | 'signal'> = {},
): Promise<Uint8Array> {
  const { onProgress, signal } = options
  throwIfRemoteMacTmp(path)
  const chunks: Uint8Array[] = []
  let offset = 0
  let total = 0

  for (;;) {
    throwIfAborted(signal)
    const slice = await daemonCliGet<ReadRangeResponse>('fs/read-range', {
      path,
      offset,
      len: RANGE_CHUNK_BYTES,
    })
    throwIfAborted(signal)

    if (slice.len > 0) {
      chunks.push(decodeBase64ToUint8Array(slice.base64))
      offset += slice.len
    }
    total = slice.size
    if (total > 0) {
      onProgress?.(Math.min(offset / total, 1))
    }
    if (slice.eof) break
    if (slice.len === 0) {
      // Avoid infinite loop if the server returns empty non-eof.
      break
    }
  }

  onProgress?.(1)

  if (chunks.length === 0) return new Uint8Array(0)
  if (chunks.length === 1) return chunks[0]

  const out = new Uint8Array(offset)
  let pos = 0
  for (const c of chunks) {
    out.set(c, pos)
    pos += c.length
  }
  return out
}

/** Build a Blob URL for client-side media/image playback. Caller must revoke. */
export function bytesToObjectUrl(bytes: Uint8Array, mime: string): string {
  // Pass a *copy of the view* as a BlobPart. Using `bytes.buffer` alone is
  // wrong when `bytes` is a subarray (includes unrelated offset bytes) and
  // has caused "Browser could not decode this image" for valid PNGs.
  const copy = new Uint8Array(bytes.byteLength)
  copy.set(bytes)
  const blob = new Blob([copy], { type: mime || 'application/octet-stream' })
  return URL.createObjectURL(blob)
}

/** Safe revoke for blob: URLs; no-op for data: / empty / null. */
export function revokeObjectUrl(url: string | null | undefined): void {
  if (url && url.startsWith('blob:')) {
    URL.revokeObjectURL(url)
  }
}

function extOf(path: string): string {
  const name = path.split('/').pop()?.split('\\').pop() ?? path
  const idx = name.lastIndexOf('.')
  if (idx < 0 || idx === name.length - 1) return ''
  return name.slice(idx + 1).toLowerCase()
}

/** Extension → image MIME for host-aware image previews. */
const IMAGE_MIME_BY_EXT: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  svg: 'image/svg+xml',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  heic: 'image/heic',
  heif: 'image/heif',
  avif: 'image/avif',
  tif: 'image/tiff',
  tiff: 'image/tiff',
}

/** Infer image MIME from path extension; default application/octet-stream. */
export function imageMimeFromPath(path: string): string {
  return IMAGE_MIME_BY_EXT[extOf(path)] ?? 'application/octet-stream'
}

/**
 * Heuristic: text content that contains a NUL in the first sample is almost
 * certainly binary. Used after fs/read-file when extension said "text".
 */
export function looksLikeBinaryText(content: string, sampleBytes = 8192): boolean {
  const sample = content.length > sampleBytes ? content.slice(0, sampleBytes) : content
  return sample.includes('\0')
}

/** Convenience: load host bytes and return a Blob object URL. Caller must revoke. */
export async function loadHostObjectUrl(
  path: string,
  mime: string,
  options?: LoadHostBinaryOptions,
): Promise<string> {
  const bytes = await loadHostBinary(path, options)
  return bytesToObjectUrl(bytes, mime)
}

/**
 * Load an image for FileViewer preview: host binary → sniff/coerce → Blob URL.
 * Prefer this over `loadHostObjectUrl` + extension MIME for images.
 */
export async function loadHostImageObjectUrl(
  path: string,
  options?: LoadHostBinaryOptions,
): Promise<{ url: string; mime: string; byteLength: number }> {
  const raw = await loadHostBinary(path, options)
  if (raw.byteLength === 0) {
    throw new Error('Image file is empty (0 bytes)')
  }
  const { bytes, mime } = coerceImageBytesForPreview(raw)
  // Extension MIME as weak fallback only when sniff fails.
  const finalMime =
    mime !== 'application/octet-stream' ? mime : imageMimeFromPath(path)
  return {
    url: bytesToObjectUrl(bytes, finalMime),
    mime: finalMime,
    byteLength: bytes.byteLength,
  }
}
