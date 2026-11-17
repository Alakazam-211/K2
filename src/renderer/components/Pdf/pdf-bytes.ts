// A PDF tab's bytes from its room's server.
//
// Desktop: `fs/read-binary` as before (one JSON answer, base64; the local
// daemon is on loopback).
//
// Hosted web: the file's raw bytes over the transfer door (`lib/file-link`):
// no base64, no JSON, no 50 MB cap, no `/cli` slot held while it streams,
// and progress as it arrives. A server without the person token (or a
// view-only room) falls back to `fs/read-binary`.

import { daemonCliGet, isHostSwitchedError } from '@/lib/daemon-cli'
import { isWebClient } from '@/lib/is-web'
import { fetchLinkBytes, mintFileLink, VIEW_LINK_TTL_SECS, type FileLink } from '@/lib/file-link'
import type { ServerScope } from '@/kessel/server-scope'

export interface PdfBytesOptions {
  /** 0..1 as the bytes arrive (the web door only). */
  onProgress?: (ratio: number) => void
  signal?: AbortSignal
}

/** The bytes of the file at `path` through `fs/read-binary`. */
export async function readBinaryBytes(scope: ServerScope, path: string): Promise<Uint8Array> {
  const r = await daemonCliGet<{ base64?: unknown }>(scope, 'fs/read-binary', { path })
  if (!r || typeof r.base64 !== 'string') throw new Error('fs/read-binary answered no file')
  return base64Bytes(r.base64)
}

/** Load the PDF at `path` on `scope`'s server. */
export async function loadPdfBytes(scope: ServerScope, path: string, opts: PdfBytesOptions = {}): Promise<Uint8Array> {
  if (!isWebClient()) return readBinaryBytes(scope, path)
  let link: FileLink
  try {
    link = await mintFileLink(scope, path, VIEW_LINK_TTL_SECS)
  } catch (err) {
    if (isHostSwitchedError(err)) throw err
    // An older server or a view-only room: the old read (it reports a
    // missing file or a refusal itself).
    return readBinaryBytes(scope, path)
  }
  return fetchLinkBytes(link, opts)
}

/** Base64 to bytes. */
export function base64Bytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/** Bytes to a binary string (one char per byte, as `atob` gives), in
 *  slices so a big file never overflows the argument list. */
export function bytesToBinaryString(bytes: Uint8Array): string {
  const SLICE = 0x8000
  const parts: string[] = []
  for (let i = 0; i < bytes.length; i += SLICE) {
    parts.push(String.fromCharCode.apply(null, bytes.subarray(i, i + SLICE) as unknown as number[]))
  }
  return parts.join('')
}
