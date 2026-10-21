// 0.46.3 Thread parity in Gardens (Rosson 2026-10-20: "app gateway and
// garden gateway both need the attachments and any other systems to match
// parity with the new thread features"). What K2's own data verbs do for a
// Garden's Thread, beside `zen-data.ts` (which registers them with the open
// conversation's server, Thread address and workspace):
//
//   - `thread.read` with `afterSeq` (thread-delta-v1): items after a seq,
//     oldest first, plus `changed` (items at or below it that changed: a
//     card answered) and `removed`; a voided card counts as removed;
//   - `thread.markRead` (thread-read-v1): `POST /cli/thread/read` on the
//     agent's server for this person's login there; on the window's server
//     the unread store moves at once and takes the daemon's answer;
//   - `thread.attachment` (thread-images-v1): one attachment's live meta, a
//     preview or an image thumbnail, from `GET /cli/thread/attachment`,
//     built key by key into the catalog's ThreadFileData (never a path or a
//     hash); a hidden one is `{kind, state}`;
//   - `thread.links` (thread-attachments-v1): which printed paths lead to a
//     file or folder in the agent's workspace (`fs/resolve-links`, the same
//     batched route the desktop Thread uses), answered as `{input,
//     linkable, kind, rel}` (the host path and root never leave here);
//   - `thread.open`: an attachment (`{docId, n}`) or a linked file
//     (`{rel, line}`). In Zen (no tab strip) it shows full size on K2's
//     preview sheet (`zen-file-preview`), with "Open in K2"; outside Zen it
//     opens as a file tab in the agent's room through the desktop Thread's
//     own opener (`lib/thread-file-link-open`);
//   - `thread.pickFiles`: K2's own picker (the Tauri dialog the compose "+"
//     uses, or a file input in the browser). A file already inside the
//     agent's workspace on this computer is used where it is; anything else
//     is uploaded into the workspace's `.k2/downloads` (compose-attach's
//     path), then named workspace-relative. Empty when the person cancels;
//   - a post with attachments (`thread-attachments-v1`): `POST
//     /cli/thread/post` with `attachments: [{path, line?, lineEnd?}]`
//     (workspace-relative), up to 10. The built-in compose's picked files
//     ride the same way on a server that has the key.

import type { ServerScope } from '@/kessel/server-scope'
import { scopeMayWrite } from '@/kessel/server-scope'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { scopeFileLinkTransport, type FileLinkAnswer, type FileLinkTransport } from '@/components/common/file-links'
import { matchWholeFilePath } from '@/lib/file-link-match'
import {
  isVoidedHitl,
  postThreadCompose,
  threadCardsParams,
  threadItemsFromSnapshot,
  type OverlayThreadItem,
  type ThreadFileObject,
  type ThreadWidgetOrigin,
} from '@/components/SessionView/overlayThread'
import { parseUnreadEntry, unreadKey, useUnreadStore } from '@/stores/unread'
import { resolveThreadComposeFiles } from '@/lib/thread-compose-attachments'
import { pickLocalComposeFiles } from '@/lib/pick-compose-files'
import { isWebClient } from '@/lib/is-web'
import type { CatalogErrorCode } from '../contract/catalog-types'

/** The open conversation a verb acts on (`zen-data` resolves it). */
export interface ZenThreadTarget {
  /** The row address (`handle::host`). */
  address: string
  /** The agent's own server. */
  scope: ServerScope
  /** The Thread address on that server. */
  threadAddr: string
  /** The agent's workspace on that server. */
  workspacePath: string
  /** The conversation id when the live feed knows it (unread store key). */
  conversationId?: string | null
}

/** A refusal with its code from the shared list (`zen-custom-bridge`'s
 *  `toRefusal` keeps the code). */
export class ZenThreadVerbError extends Error {
  readonly code: CatalogErrorCode
  constructor(code: CatalogErrorCode, message: string) {
    super(message)
    this.name = 'ZenThreadVerbError'
    this.code = code
  }
}

/** Up to 10 files ride one message. */
export const ZEN_THREAD_ATTACH_MAX = 10
/** Up to 200 candidates per `thread.links`. */
export const ZEN_THREAD_LINKS_MAX = 200

function needFeature(scope: ServerScope, feature: Parameters<ServerScope['serverSupports']>[0], verb: string): void {
  if (!scope.serverSupports(feature)) {
    throw new ZenThreadVerbError('verb_unavailable', `${verb}: ${scope.label} is older (no ${feature}); update it.`)
  }
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function intOr(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isInteger(v) ? v : undefined
}

// ── Delta reads ───────────────────────────────────────────────────────────

export interface ZenThreadDelta {
  items: OverlayThreadItem[]
  hasMore: boolean
  changed: OverlayThreadItem[]
  removed: Array<{ id: string; seq: number }>
}

/** `thread.read(address, {afterSeq})`: the thread-delta-v1 read. */
export async function zenThreadDelta(t: ZenThreadTarget, afterSeq: number, limit?: number): Promise<ZenThreadDelta> {
  if (!Number.isInteger(afterSeq) || afterSeq < 0) throw new ZenThreadVerbError('bad_args', 'afterSeq is a whole number ≥ 0')
  // thread-delta-v1 shipped with thread-attachments-v1 (0.46.1, both in
  // the daemon's boot list); the renderer gates on the key it already reads.
  needFeature(t.scope, 'thread-attachments-v1', 'thread.read afterSeq')
  const raw = await daemonCliGet<unknown>(t.scope, 'thread', {
    addr: t.threadAddr,
    after_seq: afterSeq,
    ...(limit !== undefined ? { limit } : {}),
    // Permission cards: the `cards=2` view, as the live feed gets it.
    ...threadCardsParams(t.scope),
  })
  const r = isObj(raw) ? raw : {}
  const snap = threadItemsFromSnapshot(r)
  const changedAll = threadItemsFromSnapshot({ items: Array.isArray(r.changed) ? r.changed : [] }).items
  const removed: Array<{ id: string; seq: number }> = []
  for (const x of Array.isArray(r.removed) ? r.removed : []) {
    if (isObj(x) && typeof x.id === 'string' && typeof x.seq === 'number') removed.push({ id: x.id, seq: x.seq })
  }
  const changed: OverlayThreadItem[] = []
  for (const it of changedAll) {
    // A voided card leaves the view like the live feed drops it.
    if (isVoidedHitl(it.doc)) removed.push({ id: it.id, seq: it.seq })
    else changed.push(it)
  }
  return {
    items: snap.items.filter((it) => !isVoidedHitl(it.doc)),
    hasMore: snap.has_more,
    changed,
    removed,
  }
}

// ── Read state ────────────────────────────────────────────────────────────

export interface ZenThreadReadState {
  changed: boolean
  lastReadSeq: number
  lastSeq: number
  unread: number
}

/** `thread.markRead(address, {seq?})`: forward only, this person's login. */
export async function zenThreadMarkRead(t: ZenThreadTarget, seq?: number): Promise<ZenThreadReadState> {
  if (seq !== undefined && (!Number.isInteger(seq) || seq < 0)) throw new ZenThreadVerbError('bad_args', 'seq is a whole number ≥ 0')
  needFeature(t.scope, 'thread-read-v1', 'thread.markRead')
  if (!scopeMayWrite(t.scope, 'thread/read')) {
    throw new ZenThreadVerbError('verb_unavailable', `thread.markRead: this room on ${t.scope.label} is view-only.`)
  }
  const windowServer = t.scope.isWindowHost()
  // On the window's server the badges move at once (the store's own rule:
  // nothing unread from here on, waiting cards stay).
  if (windowServer && t.conversationId) {
    const store = useUnreadStore.getState()
    const before = store.byKey[unreadKey('thread', t.conversationId)]
    if (before) {
      const readTo = seq === undefined ? before.lastSeq : Math.max(before.lastReadSeq, seq)
      store.applyState({ ...before, unread: seq === undefined || readTo >= before.lastSeq ? 0 : before.unread, lastReadSeq: readTo }, null)
    }
  }
  const raw = await daemonCliPost<unknown>(t.scope, 'thread/read', { addr: t.threadAddr, ...(seq !== undefined ? { seq } : {}) })
  const r = isObj(raw) ? raw : {}
  if (windowServer) {
    const entry = parseUnreadEntry(raw, 'thread')
    if (entry) useUnreadStore.getState().applyState(entry, null)
  }
  const n = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
  return { changed: r.changed === true, lastReadSeq: n(r.lastReadSeq), lastSeq: n(r.lastSeq), unread: n(r.unread) }
}

// ── One attachment ────────────────────────────────────────────────────────

export type ZenThreadFileMode = 'meta' | 'preview' | 'thumb'

/** The catalog's ThreadFileData, built key by key (no path, no sha256). */
export interface ZenThreadFileData {
  kind: string
  state?: string
  name?: string
  mime?: string
  size?: number
  modifiedAt?: number
  line?: number
  lineEnd?: number
  text?: string
  truncated?: boolean
  base64?: string
  width?: number
  height?: number
  sanitized?: boolean
}

/** The daemon's answer as the widget may see it. */
export function projectZenThreadFileData(raw: unknown): ZenThreadFileData {
  const r = isObj(raw) ? raw : {}
  const kind = typeof r.kind === 'string' && r.kind ? r.kind : 'file'
  const state = typeof r.state === 'string' ? r.state : undefined
  if (state === 'hidden') return { kind, state }
  const out: ZenThreadFileData = { kind }
  if (state) out.state = state
  if (typeof r.name === 'string') out.name = r.name
  if (typeof r.mime === 'string') out.mime = r.mime
  if (typeof r.size === 'number' && Number.isFinite(r.size)) out.size = r.size
  if (typeof r.modifiedAt === 'number' && Number.isFinite(r.modifiedAt)) out.modifiedAt = r.modifiedAt
  const line = intOr(r.line)
  if (line !== undefined) out.line = line
  const lineEnd = intOr(r.lineEnd)
  if (lineEnd !== undefined) out.lineEnd = lineEnd
  if (typeof r.text === 'string') out.text = r.text
  if (typeof r.truncated === 'boolean') out.truncated = r.truncated
  if (typeof r.base64 === 'string') out.base64 = r.base64
  const width = intOr(r.width)
  if (width !== undefined) out.width = width
  const height = intOr(r.height)
  if (height !== undefined) out.height = height
  if (typeof r.sanitized === 'boolean') out.sanitized = r.sanitized
  return out
}

/** The route's raw answer (K2-side: carries `path`). */
async function attachmentRaw(
  t: ZenThreadTarget,
  docId: string,
  n: number,
  mode: ZenThreadFileMode,
  px?: number,
): Promise<Record<string, unknown>> {
  try {
    const raw = await daemonCliGet<unknown>(t.scope, 'thread/attachment', {
      addr: t.threadAddr,
      id: docId,
      n,
      mode,
      ...(px !== undefined ? { px } : {}),
    })
    return isObj(raw) ? raw : {}
  } catch (err) {
    if (err instanceof DaemonHttpError) {
      const code = err.code ?? ''
      if (err.status === 413) throw new ZenThreadVerbError('too_large', `${code || 'preview_too_large'}: ${err.message}`)
      throw new ZenThreadVerbError('failed', code ? `${code}: ${err.message}` : err.message)
    }
    throw err
  }
}

/** `thread.attachment(address, docId, n, {mode, px})`. */
export async function zenThreadAttachment(
  t: ZenThreadTarget,
  docId: string,
  n: number,
  opts: { mode?: ZenThreadFileMode; px?: number } = {},
): Promise<ZenThreadFileData> {
  if (typeof docId !== 'string' || !docId) throw new ZenThreadVerbError('bad_args', 'thread.attachment needs a message id')
  if (!Number.isInteger(n) || n < 0) throw new ZenThreadVerbError('bad_args', 'thread.attachment needs an index ≥ 0')
  const mode = opts.mode ?? 'meta'
  if (mode !== 'meta' && mode !== 'preview' && mode !== 'thumb') throw new ZenThreadVerbError('bad_args', 'mode is meta, preview or thumb')
  let px: number | undefined
  if (opts.px !== undefined) {
    if (!Number.isInteger(opts.px) || opts.px < 64 || opts.px > 512) throw new ZenThreadVerbError('bad_args', 'px is 64 to 512')
    px = opts.px > 256 ? 512 : 256
  }
  needFeature(t.scope, 'thread-images-v1', 'thread.attachment')
  return projectZenThreadFileData(await attachmentRaw(t, docId, n, mode, mode === 'thumb' ? px : undefined))
}

// ── Links ─────────────────────────────────────────────────────────────────

const transports = new WeakMap<ServerScope, FileLinkTransport>()

/** One transport per server (it learns once whether the route exists). */
export function zenLinkTransport(scope: ServerScope): FileLinkTransport {
  let t = transports.get(scope)
  if (!t) {
    t = scopeFileLinkTransport(scope)
    transports.set(scope, t)
  }
  return t
}

export interface ZenThreadLink {
  input: string
  linkable: boolean
  kind?: string
  rel?: string
}

/** `thread.links(address, candidates)`: `:line[:col]` is kept in `input`
 *  and left off the path the daemon checks. */
export async function zenThreadLinks(t: ZenThreadTarget, candidates: readonly string[]): Promise<{ links: ZenThreadLink[] }> {
  if (!Array.isArray(candidates) || candidates.some((c) => typeof c !== 'string')) {
    throw new ZenThreadVerbError('bad_args', 'thread.links takes a list of strings')
  }
  if (candidates.length > ZEN_THREAD_LINKS_MAX) throw new ZenThreadVerbError('bad_args', 'thread.links takes up to 200 candidates')
  needFeature(t.scope, 'thread-attachments-v1', 'thread.links')
  const paths = candidates.map((c) => matchWholeFilePath(c)?.path ?? null)
  const ask = [...new Set(paths.filter((p): p is string => !!p))]
  const byPath = new Map<string, FileLinkAnswer>()
  if (ask.length > 0) {
    const res = await zenLinkTransport(t.scope)(t.threadAddr, ask)
    for (const a of res.links ?? []) byPath.set(a.input, a)
  }
  return {
    links: candidates.map((input, i) => {
      const p = paths[i]
      const a = p ? byPath.get(p) : undefined
      if (!a || !a.linkable || typeof a.rel !== 'string') return { input, linkable: false }
      const out: ZenThreadLink = { input, linkable: true, rel: a.rel }
      if (a.kind) out.kind = a.kind
      return out
    }),
  }
}

/** A workspace-relative path as the daemon resolves it now (a file), with
 *  its host path for K2's own use. */
async function resolveRel(t: ZenThreadTarget, rel: string): Promise<(FileLinkAnswer & { path: string }) | null> {
  const res = await zenLinkTransport(t.scope)(t.threadAddr, [rel])
  const a = (res.links ?? [])[0]
  if (!a || !a.linkable || !a.path) return null
  const root = typeof res.root === 'string' && res.root ? res.root : undefined
  return { ...a, path: a.path, ...(root ? { root } : {}) }
}

// ── Opening ───────────────────────────────────────────────────────────────

/** Where `thread.open` goes: the attachment (with its stored workspace
 *  path, K2-side) or a linked file. */
export type ZenThreadOpenTarget =
  | { kind: 'attachment'; docId: string; n: number }
  | { kind: 'rel'; rel: string; line?: number }

/** Check a widget's `thread.open` target. */
export function zenThreadOpenTarget(v: unknown): ZenThreadOpenTarget {
  if (!isObj(v)) throw new ZenThreadVerbError('bad_args', 'thread.open takes {docId, n} or {rel, line?}')
  const keys = Object.keys(v)
  if (typeof v.docId === 'string' && v.docId) {
    if (!Number.isInteger(v.n) || (v.n as number) < 0) throw new ZenThreadVerbError('bad_args', 'thread.open: n is an index ≥ 0')
    if (keys.some((k) => k !== 'docId' && k !== 'n')) throw new ZenThreadVerbError('bad_args', 'thread.open: {docId, n} only')
    return { kind: 'attachment', docId: v.docId, n: v.n as number }
  }
  if (typeof v.rel === 'string' && v.rel.trim()) {
    if (keys.some((k) => k !== 'rel' && k !== 'line')) throw new ZenThreadVerbError('bad_args', 'thread.open: {rel, line?} only')
    if (v.line !== undefined && (!Number.isInteger(v.line) || (v.line as number) < 1)) {
      throw new ZenThreadVerbError('bad_args', 'thread.open: line is ≥ 1')
    }
    return { kind: 'rel', rel: v.rel, ...(v.line !== undefined ? { line: v.line as number } : {}) }
  }
  throw new ZenThreadVerbError('bad_args', 'thread.open takes {docId, n} or {rel, line?}')
}

/** The file object a message carries (from the live feed), else the
 *  route's live meta. K2-side only: it has the workspace path. */
export async function zenAttachmentObject(
  t: ZenThreadTarget,
  docId: string,
  n: number,
  feedItems: readonly OverlayThreadItem[],
): Promise<ThreadFileObject | null> {
  const doc = feedItems.find((it) => it.id === docId || it.doc.id === docId)?.doc
  const fromFeed = doc?.attachments?.[n]
  if (fromFeed) return fromFeed
  if (!t.scope.serverSupports('thread-images-v1')) return null
  const raw = await attachmentRaw(t, docId, n, 'meta')
  const f: ThreadFileObject = { kind: typeof raw.kind === 'string' ? raw.kind : 'file' }
  if (typeof raw.state === 'string') f.state = raw.state as ThreadFileObject['state']
  if (typeof raw.path === 'string') f.path = raw.path
  if (typeof raw.name === 'string') f.name = raw.name
  if (typeof raw.mime === 'string') f.mime = raw.mime
  if (typeof raw.size === 'number') f.size = raw.size
  const line = intOr(raw.line)
  if (line !== undefined) f.line = line
  return f
}

const IMAGE_MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  heic: 'image/heic',
  heif: 'image/heif',
  svg: 'image/svg+xml',
  bmp: 'image/bmp',
  tif: 'image/tiff',
  tiff: 'image/tiff',
}

/** An image type by file name (the image kinds the Thread shows inline). */
export function zenImageMimeOf(name: string): string | null {
  const ext = /\.([A-Za-z0-9]+)$/.exec(name)?.[1]?.toLowerCase()
  return ext ? (IMAGE_MIME[ext] ?? null) : null
}

/** What K2's preview sheet shows for a host path (no attachment route:
 *  an older server, or a linked file): an image's bytes, else the text. */
export async function zenPreviewByPath(
  scope: ServerScope,
  hostPath: string,
  name: string,
): Promise<import('./zen-file-preview').ZenFilePreviewBody> {
  const mime = zenImageMimeOf(name)
  if (mime) {
    const r = await daemonCliGet<{ base64?: unknown }>(scope, 'fs/read-binary', { path: hostPath })
    if (typeof r?.base64 !== 'string') return { kind: 'none', reason: 'This image can’t be shown here.' }
    return { kind: 'image', mime, base64: r.base64 }
  }
  const r = await daemonCliGet<{ content?: unknown }>(scope, 'fs/read-file', { path: hostPath })
  const text = typeof r?.content === 'string' ? r.content : ''
  if (text.includes('\u0000')) return { kind: 'none', reason: `${name} has no preview here. Open it in K2.` }
  return { kind: 'text', text, truncated: false }
}

/** What the preview sheet shows for an attachment, through the route. */
export async function zenPreviewAttachment(
  t: ZenThreadTarget,
  docId: string,
  n: number,
  file: ThreadFileObject | null,
): Promise<import('./zen-file-preview').ZenFilePreviewBody> {
  if (file?.state === 'hidden') return { kind: 'none', reason: 'This file isn’t shared with you.' }
  if (file?.state === 'gone') return { kind: 'none', reason: `${file.name ?? 'The file'} was moved or deleted.` }
  if (t.scope.serverSupports('thread-images-v1')) {
    let raw: Record<string, unknown>
    try {
      raw = await attachmentRaw(t, docId, n, 'preview')
    } catch (err) {
      if (err instanceof ZenThreadVerbError && err.code === 'too_large') {
        return { kind: 'none', reason: `${file?.name ?? 'The file'} is too large to preview here. Open it in K2.` }
      }
      throw err
    }
    const d = projectZenThreadFileData(raw)
    if (d.kind === 'text' && typeof d.text === 'string') return { kind: 'text', text: d.text, truncated: d.truncated === true }
    if (d.kind === 'image' && typeof d.base64 === 'string' && d.mime) {
      return {
        kind: 'image',
        mime: d.mime,
        base64: d.base64,
        fallback: async () => {
          const th = projectZenThreadFileData(await attachmentRaw(t, docId, n, 'thumb', 512))
          return th.base64 && th.mime ? { mime: th.mime, base64: th.base64 } : null
        },
      }
    }
    return { kind: 'none', reason: `${d.name ?? file?.name ?? 'The file'} has no preview here. Open it in K2.` }
  }
  if (!file?.path) return { kind: 'none', reason: 'This file can’t be shown here.' }
  const link = await resolveRel(t, file.path)
  if (!link || link.kind !== 'file') return { kind: 'none', reason: `${file.name ?? 'The file'} was moved or deleted.` }
  return zenPreviewByPath(t.scope, link.path, file.name ?? link.rel ?? file.path)
}

/** A linked workspace file for the preview sheet (and its host path). */
export async function zenPreviewRel(t: ZenThreadTarget, rel: string): Promise<import('./zen-file-preview').ZenFilePreviewBody> {
  const link = await resolveRel(t, rel)
  if (!link) return { kind: 'none', reason: `${rel} isn’t a file in this workspace any more.` }
  if (link.kind !== 'file') return { kind: 'none', reason: `${rel} is a folder. Open it in K2.` }
  return zenPreviewByPath(t.scope, link.path, rel)
}

/** Resolve a workspace-relative path to an openable link (desktop opener). */
export async function zenResolveForOpen(t: ZenThreadTarget, rel: string): Promise<(FileLinkAnswer & { path: string }) | null> {
  return resolveRel(t, rel)
}

// ── Picking and posting files ─────────────────────────────────────────────

export interface ZenPickedFile {
  relPath: string
  name: string
  size?: number
  mime?: string
}

/** Host paths on the agent's server → workspace-relative (the daemon's
 *  realpath; only files inside the workspace answer). */
async function relOnServer(t: ZenThreadTarget, hostPaths: string[]): Promise<Array<string | null>> {
  if (hostPaths.length === 0) return []
  const res = await zenLinkTransport(t.scope)(t.threadAddr, hostPaths)
  const by = new Map((res.links ?? []).map((a) => [a.input, a]))
  return hostPaths.map((p) => {
    const a = by.get(p)
    return a && a.linkable && a.kind === 'file' && typeof a.rel === 'string' ? a.rel : null
  })
}

/** Files picked on THIS computer (paths) or in the browser (`File`s) as
 *  workspace-relative paths on the agent's server, by the desktop Thread's
 *  own rule (`resolveThreadComposeFiles`, S4): a local file already in the
 *  workspace stays where it is; everything else is uploaded into the
 *  workspace's `.k2/downloads` first. Null when nothing was attached (the
 *  upload was cancelled or failed: it already toasted why). */
export async function zenAttachRelPaths(
  t: ZenThreadTarget,
  input: { paths?: string[]; files?: File[] },
): Promise<ZenPickedFile[] | null> {
  const files = input.files ?? []
  const onServer = await resolveThreadComposeFiles(t.scope, {
    ...(input.paths && input.paths.length > 0 ? { paths: input.paths } : {}),
    ...(files.length > 0 ? { files } : {}),
    workspacePath: t.workspacePath,
  })
  if (!onServer) return null
  const rels = await relOnServer(
    t,
    onServer.map((f) => f.path),
  )
  return onServer.map((f, i) => {
    const rel = rels[i]
    if (!rel) throw new ZenThreadVerbError('failed', `${f.name} isn’t a file in the agent’s workspace.`)
    const picked = files.find((b) => b.name === f.name)
    return {
      relPath: rel,
      name: f.name,
      ...(picked ? { size: picked.size } : {}),
      ...(picked?.type ? { mime: picked.type } : {}),
    }
  })
}

/** The browser's own picker (a file input): the files, or [] on cancel. */
function pickBrowserFiles(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.multiple = true
    input.style.position = 'fixed'
    input.style.left = '-9999px'
    let done = false
    const finish = (files: File[]): void => {
      if (done) return
      done = true
      input.remove()
      resolve(files)
    }
    input.addEventListener('change', () => finish(input.files ? [...input.files] : []))
    input.addEventListener('cancel', () => finish([]))
    document.body.appendChild(input)
    input.click()
  })
}

/** The pickers, injectable for tests. */
export interface ZenPickers {
  native(): Promise<string[] | null>
  browser(): Promise<File[]>
  web(): boolean
}

const defaultPickers: ZenPickers = {
  native: () => pickLocalComposeFiles(),
  browser: () => pickBrowserFiles(),
  web: () => isWebClient(),
}

/** `thread.pickFiles(address)`: K2's picker, then relPaths (empty on cancel). */
export async function zenThreadPickFiles(t: ZenThreadTarget, pickers: ZenPickers = defaultPickers): Promise<{ files: ZenPickedFile[] }> {
  needFeature(t.scope, 'thread-attachments-v1', 'thread.pickFiles')
  if (pickers.web()) {
    const files = await pickers.browser()
    if (files.length === 0) return { files: [] }
    return { files: ((await zenAttachRelPaths(t, { files: files.slice(0, ZEN_THREAD_ATTACH_MAX) })) ?? []) }
  }
  const paths = await pickers.native()
  if (!paths || paths.length === 0) return { files: [] }
  return { files: (await zenAttachRelPaths(t, { paths: paths.slice(0, ZEN_THREAD_ATTACH_MAX) })) ?? [] }
}

export interface ZenThreadAttach {
  relPath: string
  line?: number
  lineEnd?: number
}

/** Check a list of attachments (the catalog's ThreadAttach). */
export function zenThreadAttachments(v: unknown): ZenThreadAttach[] {
  if (!Array.isArray(v)) throw new ZenThreadVerbError('bad_args', 'attachments is a list')
  if (v.length > ZEN_THREAD_ATTACH_MAX) throw new ZenThreadVerbError('bad_args', 'Up to 10 attachments a message')
  return v.map((a) => {
    if (!isObj(a) || typeof a.relPath !== 'string' || !a.relPath.trim()) {
      throw new ZenThreadVerbError('bad_args', 'An attachment is {relPath, line?, lineEnd?}')
    }
    if (Object.keys(a).some((k) => k !== 'relPath' && k !== 'line' && k !== 'lineEnd')) {
      throw new ZenThreadVerbError('bad_args', 'An attachment is {relPath, line?, lineEnd?}')
    }
    const out: ZenThreadAttach = { relPath: a.relPath }
    for (const k of ['line', 'lineEnd'] as const) {
      if (a[k] === undefined) continue
      if (!Number.isInteger(a[k]) || (a[k] as number) < 1) throw new ZenThreadVerbError('bad_args', `${k} is a whole number ≥ 1`)
      out[k] = a[k] as number
    }
    return out
  })
}

/** A compose post with files riding it (thread-attachments-v1). */
export async function zenThreadPostAttachments(
  t: ZenThreadTarget,
  text: string,
  attachments: readonly ZenThreadAttach[],
  origin?: ThreadWidgetOrigin,
): Promise<OverlayThreadItem | null> {
  needFeature(t.scope, 'thread-attachments-v1', 'thread.post attachments')
  if (attachments.length > ZEN_THREAD_ATTACH_MAX) throw new ZenThreadVerbError('bad_args', 'Up to 10 attachments a message')
  // The desktop Thread's one compose send (S4): the daemon resolves each
  // workspace-relative `path` against the Thread's workspace and checks it.
  const result = await postThreadCompose(t.scope, t.threadAddr, text, null, {
    ...(origin ? { origin } : {}),
    attachments: attachments.map((a) => ({
      path: a.relPath,
      ...(a.line !== undefined ? { line: a.line } : {}),
      ...(a.lineEnd !== undefined ? { lineEnd: a.lineEnd } : {}),
    })),
  })
  if (!result.ok) throw new Error(result.error)
  return result.item
}
