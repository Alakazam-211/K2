// Host-aware binary loader for file-viewer previews (images, PDF, media).
//
// Always goes through the daemon (`fs/read-binary` / `fs/read-range`) so
// local, remote, and web clients share one path. NEVER use
// `convertFileSrc` for host files — it breaks remote + web.
//
// `read-binary` is capped at 50 MB server-side. Larger media can be
// assembled via ranged reads when `allowRangeAssembly` is true.

import { daemonCliGet } from '@/lib/daemon-cli'

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
  const binary = atob(base64)
  const data = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) data[i] = binary.charCodeAt(i)
  return data
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

  try {
    const r = await daemonCliGet<{ base64: string }>('fs/read-binary', { path })
    throwIfAborted(signal)
    onProgress?.(1)
    return decodeBase64ToUint8Array(r.base64)
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
  // Copy into a fresh ArrayBuffer-backed view so Blob always gets a
  // real ArrayBuffer (not SharedArrayBuffer) regardless of Uint8Array
  // construction path.
  const copy = new Uint8Array(bytes.byteLength)
  copy.set(bytes)
  const blob = new Blob([copy.buffer], { type: mime })
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
