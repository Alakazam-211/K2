// `k2.pick` and `k2.onDrop`, the renderer half (prd-garden-board-needs-v1
// §5, GP1–GP6; vs-live V4, V5, V13). One per placement's frame.
//
// - Desktop: K2's own OS dialog (`pick_local_files` with extension filters,
//   V4), then the daemon reads the picked paths into the widget's data
//   (`POST /cli/zen/blob/ingest`, the widget door). Drops: the external
//   drop router finds this frame under the point (`zen-drop-targets`) and
//   hands it the dropped paths the same way.
// - Web client: K2's own file input outside the frame, then `blobs.put`
//   chunks; the frame sees its own HTML5 drops and the SDK uploads them.
// - Either way the widget gets refs, names, sizes and sniffed types, never a
//   path (GP3). Paths come only from K2's dialog or a drop, never the frame.

import type { ZenUploadedFile } from './zen-custom-bridge'
import { ZEN_BLOB_MAX_BYTES, type ZenBlobInfo } from './zen-custom-blobs'

export interface ZenPickOptions {
  accept: string[]
  multiple: boolean
  max: number
}

export interface ZenUploadsDeps {
  /** The web client (no OS dialog, no paths). */
  web(): boolean
  /** The OS dialog: paths, or null when cancelled. */
  native(opts: { filters: string[]; multiple: boolean }): Promise<string[] | null>
  /** The web client's file input. */
  browser(opts: { accept: string; multiple: boolean }): Promise<File[]>
  /** `POST /cli/zen/blob/ingest` for this placement. */
  ingest(paths: string[], accept: string[]): Promise<ZenUploadedFile[]>
  /** `blobs.put` for this placement (the web client). */
  put(bytes: ArrayBuffer, opts: { name?: string }): Promise<ZenBlobInfo>
}

/** Extensions a mime pattern stands for in the OS dialog's filter. */
const MIME_EXTS: Readonly<Record<string, readonly string[]>> = {
  'image/*': ['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'heif', 'avif', 'svg'],
  'image/png': ['png'],
  'image/jpeg': ['jpg', 'jpeg'],
  'image/gif': ['gif'],
  'image/webp': ['webp'],
  'image/heic': ['heic', 'heif'],
  'image/svg+xml': ['svg'],
  'application/pdf': ['pdf'],
  'application/json': ['json'],
  'text/*': ['txt', 'md', 'csv', 'tsv', 'json'],
  'text/plain': ['txt'],
  'text/markdown': ['md'],
  'text/csv': ['csv'],
}

/**
 * The OS dialog's extension filter for an `accept` list. Empty (show every
 * file) when the list is empty or names a type K2 can't spell as
 * extensions: the daemon checks every picked file again either way.
 */
export function zenPickFilters(accept: readonly string[]): string[] {
  const out: string[] = []
  for (const a of accept) {
    const exts = a.startsWith('.') ? [a.slice(1).toLowerCase()] : MIME_EXTS[a.toLowerCase()]
    if (!exts) return []
    for (const e of exts) if (!out.includes(e)) out.push(e)
  }
  return out
}

/** Does a browser `File` match the `accept` list (by name or its type)? */
export function zenFileAccepted(accept: readonly string[], name: string, type: string): boolean {
  if (accept.length === 0) return true
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : ''
  const t = type.toLowerCase()
  return accept.some((a) => {
    if (a.startsWith('.')) return ext === a.slice(1).toLowerCase()
    if (a.endsWith('/*')) return t.startsWith(a.slice(0, -1))
    return t === a
  })
}

/** Why a pick can't go on (`too_large` / `bad_args`), with the file's name. */
export class ZenUploadsError extends Error {
  constructor(
    readonly code: 'too_large' | 'bad_args',
    message: string,
  ) {
    super(message)
    this.name = 'ZenUploadsError'
  }
}

/** GP2/GP3: K2's picker, then the files into this widget's data. */
export async function zenPick(deps: ZenUploadsDeps, opts: ZenPickOptions): Promise<ZenUploadedFile[]> {
  if (deps.web()) {
    const files = (await deps.browser({ accept: opts.accept.join(','), multiple: opts.multiple })).slice(0, opts.max)
    for (const f of files) {
      if (f.size > ZEN_BLOB_MAX_BYTES) throw new ZenUploadsError('too_large', `${f.name} is over 16 MiB.`)
      if (!zenFileAccepted(opts.accept, f.name, f.type)) {
        throw new ZenUploadsError('bad_args', `${f.name} isn’t one of the kinds this widget takes (${opts.accept.join(' ')}).`)
      }
    }
    const out: ZenUploadedFile[] = []
    for (const f of files) {
      const info = await deps.put(await f.arrayBuffer(), { name: f.name })
      out.push({ ref: info.ref, name: f.name, size: info.size, mime: info.mime })
    }
    return out
  }
  const paths = await deps.native({ filters: zenPickFilters(opts.accept), multiple: opts.multiple })
  if (!paths || paths.length === 0) return []
  return deps.ingest(paths.slice(0, opts.max), opts.accept)
}
