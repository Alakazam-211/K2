// The frame's `k2.blobs` for a board file, over `/cli/board/file/blob` and `blob/put`.
//
// A manifest can lie, so only what the daemon's sniff calls a picture (`display: true`) ever reaches the frame as
// bytes for an `<img>`; anything else answers `blob_missing`, and the image draws its placeholder (BT-21). Each picture
// is kept in the session too, because Save a copy has to send them again when the file is gone.

import { ZenCustomRefusal } from '@/lib/zen/zen-custom-bridge'
import type { ZenBlobInfo, ZenCustomBlobs } from '@/lib/zen/zen-custom-blobs'
import { base64ToBytes, bytesToBase64 } from '@/lib/zen/zen-custom-blobs'
import { BOARD_FILE_BLOB_CHUNK, blobHashOfRef, type BoardFileApi, type BoardFileSession } from './board-file-session'

const sha256 = async (bytes: Uint8Array): Promise<string> => {
  const d = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes as unknown as ArrayBuffer))
  return Array.from(d, (b) => b.toString(16).padStart(2, '0')).join('')
}
/** PNG, JPEG, WebP or GIF by their first bytes, as the daemon sniffs; anything else is not a picture. */
function sniffImage(b: Uint8Array): string | null {
  if (b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return 'image/png'
  if (b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return 'image/jpeg'
  if (b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46) return 'image/gif'
  if (b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46 && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50) return 'image/webp'
  return null
}

export function createBoardFileBlobs(api: BoardFileApi, path: string, session: BoardFileSession): ZenCustomBlobs {
  const fetchAll = async (ref: string): Promise<{ mime: string; bytes: Uint8Array }> => {
    const hash = blobHashOfRef(ref)
    const have = session.cachedBlob(hash)
    if (have) return have
    let out: Uint8Array | null = null
    let at = 0
    let mime = 'application/octet-stream'
    for (;;) {
      let a: { mime?: string; total: number; offset: number; data: string; display?: boolean }
      try {
        a = await api.get('board/file/blob', { path, hash, offset: at, len: BOARD_FILE_BLOB_CHUNK })
      } catch (err) {
        throw new ZenCustomRefusal('blob_missing', err instanceof Error ? err.message : 'That picture can’t be read.')
      }
      if (a.display !== true) throw new ZenCustomRefusal('blob_missing', 'That part of the file isn’t a picture K2 shows.')
      mime = a.mime ?? mime
      const b = base64ToBytes(a.data)
      if (!out) out = new Uint8Array(a.total)
      if (at + b.length > out.length) throw new ZenCustomRefusal('failed', 'The picture is longer than its size.')
      out.set(b, at)
      at += b.length
      if (at >= a.total) break
      if (b.length === 0) throw new ZenCustomRefusal('failed', 'K2 answered an empty slice before the end of a picture.')
    }
    session.rememberBlob(hash, mime, out)
    return { mime, bytes: out }
  }

  return {
    async put(bytes, opts): Promise<ZenBlobInfo> {
      const u8 = new Uint8Array(bytes)
      const mime = sniffImage(u8)
      if (!mime) throw new ZenCustomRefusal('bad_args', 'Pictures here are PNG, JPEG, WebP or GIF.')
      const hash = `sha256-${await sha256(u8)}`
      for (let offset = 0; ; offset += BOARD_FILE_BLOB_CHUNK) {
        const end = Math.min(u8.length, offset + BOARD_FILE_BLOB_CHUNK)
        await api.post('board/file/blob/put', { hash, offset, total: u8.length, mime, data: bytesToBase64(u8.subarray(offset, end)) })
        if (end >= u8.length) break
      }
      session.rememberBlob(hash, mime, u8)
      return { ref: `k2blob:${hash}`, size: u8.length, mime, name: opts.name ?? null, new: true }
    },
    async head(ref) {
      const b = await fetchAll(ref)
      return { mime: b.mime, size: b.bytes.length }
    },
    async get(ref) {
      const b = await fetchAll(ref)
      // A copy: the frame's buffer moves across, and the session keeps its own.
      return { info: { ref, size: b.bytes.length, mime: b.mime, name: null }, bytes: b.bytes.slice().buffer as ArrayBuffer }
    },
    reader(ref) {
      return async (offset: number, len: number) => {
        const b = await fetchAll(ref)
        const slice = b.bytes.subarray(offset, offset + len)
        return { base64: bytesToBase64(slice), len: slice.length, size: b.bytes.length, eof: offset + slice.length >= b.bytes.length }
      }
    },
  }
}
