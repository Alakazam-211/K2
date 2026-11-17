// A file's raw bytes over the daemon's transfer door, for the hosted web
// client (0.46.15).
//
// The web client used to read every file through `/cli/fs/read-binary` (the
// whole file as base64 in one JSON answer, capped at 50 MB) or loop
// `/cli/fs/read-range` (8 MB base64 slices). Through the Worker and the
// tunnel that is a third more bytes, a JSON parse and a byte-by-byte decode
// on the page's thread, and each request holds one of the four `/cli` slots
// the page has per server (`daemon-cli`), so the rest of the app waits
// behind a big PDF or download.
//
// Instead the daemon mints a short-lived download token for the file
// (`POST /cli/fs/download/token {path}` for a person; a Thread attachment's
// `POST /cli/thread/attachment/token`), and the bytes come from
// `GET /_k2/transfers/downloads/<id>?token=…`: raw, `Content-Disposition:
// attachment`, Range. A Download is then the browser's own (an `<a download>`
// to that link: streamed to disk, the browser's progress, nothing held in
// the page) and the PDF tab fetches raw bytes with progress.
//
// A server that predates the person token (or a view-only room, which never
// posts) answers the mint with an error: callers fall back to the old reads.

import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonHttpBase } from '@/kessel/daemon-ws'
import type { ServerScope } from '@/kessel/server-scope'

/** The daemon's answer to a download token route. */
interface TokenAnswer {
  url: string
  transferToken: string
  name?: string
  size?: number
  mime?: string
  etag?: string
  expiresAt?: number
}

/** A ready link to one file's raw bytes. */
export interface FileLink {
  /** Absolute URL with the token in the query (a link can't set a header). */
  url: string
  name: string
  size: number | null
  mime: string
}

/** A PDF tab's link lives this long: the fetch starts at once. */
export const VIEW_LINK_TTL_SECS = 120
/** A Download's link: the browser may wait for the person's save dialog. */
export const DOWNLOAD_LINK_TTL_SECS = 600

const TOKEN_SHAPE = /^k2dn_[A-Za-z0-9._-]+$/
const DOOR_PATH = /^\/_k2\/transfers\/downloads\/[A-Za-z0-9]+$/

/** The link for a token answer on `scope`'s server. Throws on an answer
 *  that isn't a door link (never builds a URL from something else). */
export async function linkFromAnswer(scope: ServerScope, raw: unknown, fallbackName: string): Promise<FileLink> {
  const a = (raw && typeof raw === 'object' ? raw : {}) as Partial<TokenAnswer>
  if (typeof a.url !== 'string' || !DOOR_PATH.test(a.url)) throw new Error('the server answered no download link')
  if (typeof a.transferToken !== 'string' || !TOKEN_SHAPE.test(a.transferToken)) throw new Error('the server answered no download token')
  const creds = await scope.creds()
  return {
    url: `${daemonHttpBase(creds)}${a.url}?token=${a.transferToken}`,
    name: typeof a.name === 'string' && a.name.trim() !== '' ? a.name : fallbackName,
    size: typeof a.size === 'number' && Number.isFinite(a.size) && a.size >= 0 ? a.size : null,
    mime: typeof a.mime === 'string' && a.mime !== '' ? a.mime : 'application/octet-stream',
  }
}

/** A link to the file at absolute `path` on `scope`'s server. */
export async function mintFileLink(scope: ServerScope, path: string, ttlSecs = DOWNLOAD_LINK_TTL_SECS): Promise<FileLink> {
  const raw = await daemonCliPost<unknown>(scope, 'fs/download/token', { path, tokenTtlSecs: ttlSecs })
  return linkFromAnswer(scope, raw, baseName(path))
}

/** A link to a Thread attachment (`addr`, message `docId`, index `n`). */
export async function mintAttachmentLink(
  scope: ServerScope,
  ref: { addr: string; docId: string; n: number },
  fallbackName: string,
  ttlSecs = DOWNLOAD_LINK_TTL_SECS,
): Promise<FileLink> {
  const raw = await daemonCliPost<unknown>(scope, 'thread/attachment/token', { addr: ref.addr, id: ref.docId, n: ref.n, tokenTtlSecs: ttlSecs })
  return linkFromAnswer(scope, raw, fallbackName)
}

/** The refusal text of a door answer (`{error: {code, hint}}`), else the status. */
async function doorError(res: Response): Promise<Error> {
  let hint = ''
  try {
    const v = (await res.json()) as { error?: { hint?: unknown; code?: unknown } }
    if (v && v.error && typeof v.error.hint === 'string') hint = v.error.hint
    else if (v && v.error && typeof v.error.code === 'string') hint = v.error.code
  } catch {
    // not JSON: the status says it
  }
  return new Error(hint || `the server answered ${res.status}`)
}

/**
 * The file's bytes from `link`, read as they arrive. `onProgress(0..1)`
 * when the size is known. Throws on a refusal or a short read.
 */
export async function fetchLinkBytes(
  link: FileLink,
  opts: { signal?: AbortSignal; onProgress?: (ratio: number) => void } = {},
): Promise<Uint8Array> {
  // The token is the only credential: no cookie rides along.
  const res = await fetch(link.url, { method: 'GET', credentials: 'omit', cache: 'no-store', signal: opts.signal })
  if (!res.ok) throw await doorError(res)
  const header = Number(res.headers.get('Content-Length'))
  const total = Number.isFinite(header) && header > 0 ? header : (link.size ?? 0)
  if (!res.body) {
    const all = new Uint8Array(await res.arrayBuffer())
    opts.onProgress?.(1)
    return all
  }
  const reader = res.body.getReader()
  const parts: Uint8Array[] = []
  let got = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    parts.push(value)
    got += value.byteLength
    if (total > 0) opts.onProgress?.(Math.min(got / total, 1))
  }
  if (total > 0 && got !== total) throw new Error(`the download stopped early (${got} of ${total} bytes)`)
  opts.onProgress?.(1)
  if (parts.length === 1) return parts[0]
  const out = new Uint8Array(got)
  let at = 0
  for (const p of parts) {
    out.set(p, at)
    at += p.byteLength
  }
  return out
}

/**
 * Hand `link` to the browser's own download manager: an `<a download>`
 * click (same origin, so the name holds; the daemon's
 * `Content-Disposition: attachment` names it too). The page holds nothing
 * and the browser shows the progress.
 */
export function startBrowserDownload(link: FileLink): void {
  const a = document.createElement('a')
  a.href = link.url
  a.download = link.name || 'download'
  a.rel = 'noopener'
  a.style.display = 'none'
  document.body.appendChild(a)
  try {
    a.click()
  } finally {
    a.remove()
  }
}

function baseName(p: string): string {
  const seg = p.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : p
}
