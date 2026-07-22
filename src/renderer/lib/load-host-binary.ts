// Host-aware binary file loader for FileViewer previews (images, PDF, media).
//
// Always reads via `daemonCliGet('fs/read-binary')` so local, remote, and web
// clients share one path. Never use `convertFileSrc` for host files — that
// only works for local Tauri asset protocol and breaks remote/web.
//
// Blob URL guidance: prefer object URLs for media/images; always
// `URL.revokeObjectURL` on unmount or path change (see `revokeObjectUrl`).

import { daemonCliGet } from '@/lib/daemon-cli'

/** Pure base64 → bytes. Exported for unit tests (no daemon needed). */
export function decodeBase64ToUint8Array(base64: string): Uint8Array {
  const binary = atob(base64)
  const data = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) {
    data[i] = binary.charCodeAt(i)
  }
  return data
}

/**
 * Load a host file as raw bytes via the daemon binary route.
 * Cap is daemon-side (~50 MB for full read-binary).
 */
export async function loadHostBinary(path: string): Promise<Uint8Array> {
  const r = await daemonCliGet<{ base64: string }>('fs/read-binary', { path })
  return decodeBase64ToUint8Array(r.base64)
}

/** Load host bytes and wrap as a Blob with the given MIME type. */
export async function loadHostBlob(path: string, mime: string): Promise<Blob> {
  const bytes = await loadHostBinary(path)
  // Copy into a fresh ArrayBuffer so BlobPart typing is clean across targets.
  const copy = new Uint8Array(bytes.byteLength)
  copy.set(bytes)
  return new Blob([copy.buffer], { type: mime })
}

/**
 * Load host bytes → Blob → object URL.
 * Caller MUST revoke the URL when done (unmount / path change):
 *   `revokeObjectUrl(url)` or `URL.revokeObjectURL(url)`.
 */
export async function loadHostObjectUrl(path: string, mime: string): Promise<string> {
  const blob = await loadHostBlob(path, mime)
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
