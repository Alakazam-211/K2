/** Overlay Thread snapshot + WS frame helpers. Thread pane never shows chatter. */

import { daemonCliPost } from '@/lib/daemon-cli'
import {
  isThreadAddressMiss,
  requestThreadAddressRelookup,
  threadErrorText,
} from '@/lib/thread-address-bus'
import type { ServerScope } from '@/kessel/server-scope'
import { httpCodeOf } from '@/lib/daemon-http-error'
import { threadAttachmentRefusalText } from '@/lib/thread-compose-attachments'
import { parseThreadAuthor, type ThreadAuthor } from '@/lib/thread-viewer'
import { readPermissionCard, type PermissionCardData } from './permissionCardView'

// ── Permission cards (prd-agent-settings-cards-v1 rev 3.1 §4.2, §17.1) ────
//
// The daemon draws a permission card per viewer. A client that can draw the
// `cards=2` view opts in on every surface that serves Thread docs (`GET
// /cli/thread` pages and deltas, `WS /cli/overlay/events`), and only on a
// server that lists `permission-cards-v1` in `/boot-status` (the server's
// switch is on). Everywhere else nothing is sent and nothing changes: the
// daemon serves the fallback, which draws as an ordinary card or line.

/** The `/boot-status` feature a server lists while its switch is on. */
export const PERMISSION_CARDS_FEATURE = 'permission-cards-v1'
/** The card contract this client draws (`cards=2`). */
export const THREAD_CARDS_CONTRACT = 2

/** `{cards: '2'}` for a GET's query on a server with the feature, else `{}`. */
export function threadCardsParams(scope: Pick<ServerScope, 'serverSupports'>): { cards?: string } {
  return scope.serverSupports(PERMISSION_CARDS_FEATURE) ? { cards: String(THREAD_CARDS_CONTRACT) } : {}
}

/** `&cards=2` for a socket URL on a server with the feature, else ''. */
export function threadCardsQuery(scope: Pick<ServerScope, 'serverSupports'>): string {
  return scope.serverSupports(PERMISSION_CARDS_FEATURE) ? `&cards=${THREAD_CARDS_CONTRACT}` : ''
}

export const OVERLAY_PAGE_SIZE = 25

/** `widget` / `author` / `design` / `attachments` when the daemon sent
 *  them (left off otherwise, so a coerced doc stays equal to the socket's).
 *
 *  Every doc field a renderer reads must be carried here. The socket hands
 *  its doc through whole, but a `GET /cli/thread` page (`coerceItem`) and a
 *  `thread/post` answer (`overlayItemFromThreadPost`) are rebuilt field by
 *  field, so a field left out here vanishes on load (0.46.1 RC: file chips
 *  never drew from a loaded Thread, and a files-only post was an empty
 *  bubble). */
type DocExtras = Pick<OverlayDoc, 'widget' | 'author' | 'design' | 'attachments' | 'context'>
function docExtras(src: Record<string, unknown>): DocExtras {
  const out: DocExtras = {}
  const w = src.widget
  if (w && typeof w === 'object' && !Array.isArray(w)) {
    const o = w as Record<string, unknown>
    if (typeof o.widget === 'string' && typeof o.garden === 'string') {
      out.widget = { widget: o.widget, garden: o.garden }
    }
  }
  const author = parseThreadAuthor(src.author)
  if (author) out.author = author
  const design = coerceDesign(src.design)
  if (design) out.design = design
  const attachments = coerceThreadFileObjects(src.attachments)
  if (attachments.length > 0) out.attachments = attachments
  const context = coerceThreadContext(src.context)
  if (context) out.context = context
  return out
}

/** The doc's `context` (thread-context-v1): all four strings, else none. */
export function coerceThreadContext(raw: unknown): ThreadContextDoc | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const c = raw as Record<string, unknown>
  if (typeof c.text !== 'string' || !c.text || typeof c.widget !== 'string' || typeof c.garden !== 'string' || typeof c.placement !== 'string') {
    return null
  }
  return { text: c.text, widget: c.widget, garden: c.garden, placement: c.placement }
}

function coerceDesign(raw: unknown): OverlayDoc['design'] {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const d = raw as Record<string, unknown>
  if (typeof d.url !== 'string' || typeof d.notes !== 'number') return null
  return { ...(typeof d.itemId === 'string' ? { itemId: d.itemId } : {}), url: d.url, notes: d.notes }
}

const THREAD_FILE_STATES: ReadonlySet<string> = new Set(['ok', 'changed', 'gone', 'hidden'])

/** The daemon's `attachments` list (`k2_core::overlay::attachments`, plus
 *  the per-read `state`) as file objects: unknown fields dropped, a
 *  wrong-typed field left off, a row without a `kind` skipped. */
export function coerceThreadFileObjects(raw: unknown): ThreadFileObject[] {
  if (!Array.isArray(raw)) return []
  const out: ThreadFileObject[] = []
  for (const row of raw) {
    if (!row || typeof row !== 'object' || Array.isArray(row)) continue
    const r = row as Record<string, unknown>
    if (typeof r.kind !== 'string' || !r.kind) continue
    const f: ThreadFileObject = { kind: r.kind }
    if (typeof r.state === 'string' && THREAD_FILE_STATES.has(r.state)) f.state = r.state as ThreadFileState
    if (typeof r.path === 'string') f.path = r.path
    if (typeof r.name === 'string') f.name = r.name
    if (typeof r.size === 'number') f.size = r.size
    if (typeof r.mime === 'string') f.mime = r.mime
    if (typeof r.modifiedAt === 'number') f.modifiedAt = r.modifiedAt
    if (typeof r.sha256 === 'string' || r.sha256 === null) f.sha256 = r.sha256 as string | null
    if (typeof r.line === 'number') f.line = r.line
    if (typeof r.lineEnd === 'number') f.lineEnd = r.lineEnd
    if (typeof r.canDownload === 'boolean') f.canDownload = r.canDownload
    out.push(f)
  }
  return out
}

export interface OverlayChoice {
  prompt: string
  options: { label: string }[]
  allow_custom: boolean
  status: string
  answer?: string | null
  /** Who answered or dismissed the card (daemon-stamped from the
   *  credential, like a post's `author`). Absent while pending, on an
   *  agent's own answer and on older cards. */
  answeredBy?: ThreadAuthor | null
  /** Unix seconds it was answered, dismissed or expired. */
  answeredAt?: number | null
  /** Unix seconds a still-pending card expires (the daemon turns it
   *  `expired`; never a client timer). Absent: open until answered. */
  expiresAt?: number | null
  /** 0.46.3, a settings (action) card the person answered: what K2 did,
   *  from the doc's `choice.action.result.message` ("Kept as yours. Undo
   *  in Settings → Gardens → Local Data."). Absent on every other card. */
  resultMessage?: string
  /** A permission card's `cards=2` view for this viewer (`choice.action`
   *  with id `permission` / `permission.batch` and a readable `view`).
   *  Absent on every other card and on the fallback. */
  permission?: PermissionCardData
}

export interface OverlaySecret {
  name: string
  status: string
  prompt?: string | null
  /** K2 vault (prd-k2-vault-v1 §10.1, KV9): what the asking agent proposes.
   *  Never a value. `workspace` | `server` | `session`. */
  scope?: string | null
  /** Proposed sites, normalised origins (`https://api.stripe.com:443`). */
  hosts?: string[]
  /** `fetch` | `file` | `env` (the daemon may also say `proxy`). */
  delivery?: string | null
  /** `api_key` | `password` | `ssh_key` | … */
  secretKind?: string | null
  /** After a person saved it: the `k2v://` reference (never the value). */
  ref?: string | null
  /** Who answered or dismissed the card (daemon-stamped from the
   *  credential, like a post's `author`). Absent while pending, on an
   *  agent's own answer and on older cards. */
  answeredBy?: ThreadAuthor | null
  /** Unix seconds it was answered, dismissed or expired. */
  answeredAt?: number | null
  /** Unix seconds a still-pending card expires (the daemon turns it
   *  `expired`; never a client timer). Absent: open until answered. */
  expiresAt?: number | null
}

/** A secret another agent sent (prd-agent-secret-handoff-v1 §10.1, H18):
 *  names, sites and the note only, never a value. A person answers it
 *  through `/cli/vault/incoming/decide`, not `thread/answer`. */
export interface OverlayHandoff {
  deliveryId: string
  name: string
  /** The sender: `sales` (this server) or `sales::rosson.k2.dev`. */
  from: string
  host?: string | null
  kind: string
  hosts: string[]
  note: string
  reason?: string | null
  /** The receiving workspace already has a secret by that name. */
  clash: boolean
  /** `pending` | `landed` | `declined` | `expired` */
  status: string
  /** After it landed: the `k2v://` reference. */
  reference?: string | null
  answeredBy?: ThreadAuthor | null
  answeredAt?: number | null
  expiresAt?: number | null
  /** The receiving workspace (label). */
  workspace?: string | null
}

/** Where a secret card's answer is kept (the vault's scope). */
export type SecretScope = 'workspace' | 'server' | 'session'
/** How the agent gets a secret card's answer. */
export type SecretDelivery = 'fetch' | 'file' | 'env'

/** A custom Garden widget's mark on a post or card answer it sends for the
 *  person (prd-zen-user-widgets-v2 UWB12a). `placement` + `hash` (0.46.1)
 *  name the Garden slot it sits in and the bundle hash its frame runs, so
 *  the daemon holding the Garden can check the widget's approved
 *  `thread:post` cap. They go to this computer's daemon only; another
 *  server gets the label (`widget`, `garden`) after this computer's daemon
 *  checked it (`zen/widget/check`). */
export interface ThreadWidgetOrigin {
  widget: string
  garden: string
  placement?: string
  hash?: string
}

/** The wire copy of an origin: only its known fields. */
export function threadWidgetOriginBody(o: ThreadWidgetOrigin): ThreadWidgetOrigin {
  const out: ThreadWidgetOrigin = { widget: o.widget, garden: o.garden }
  if (o.placement !== undefined && o.hash !== undefined) {
    out.placement = o.placement
    out.hash = o.hash
  }
  return out
}

/** A Thread card answer. `secret` is the pasted value: it travels in the POST
 *  body only and never lands in Thread state, a store or a log. The other
 *  fields are what the person chose on a secret card (sent only when they
 *  differ from the agent's proposal); `confirm` is the second click after a
 *  `card_host_mismatch` / `card_readable_delivery` warning. `disclosed`
 *  (0.46.3): the card showed the agent's choice in a plain sentence before
 *  Save ("Sales will get this as a file it can read"), so the daemon skips
 *  the readable-delivery second click. It never skips the mismatch check. */
export interface ThreadAnswerPayload {
  answer?: string
  secret?: string
  scope?: SecretScope
  hosts?: string[]
  delivery?: SecretDelivery
  confirm?: boolean
  disclosed?: boolean
  /** Set when a custom Garden widget answers (`thread.answer`, a choice
   *  only): the daemon checks the widget's approved `thread:post`. */
  origin?: ThreadWidgetOrigin
  /** Permission cards (§17.3): a batch item, a Deny reason, the confirm
   *  token, and where the answer came from (`view.answer.via`). */
  item?: string
  reason?: string
  confirmToken?: string
  client?: 'desktop' | 'web' | 'phone' | 'terminal'
}

/** The `POST thread/answer` body for a payload (values in the body only). */
export function threadAnswerBody(
  addr: string,
  id: string,
  payload: ThreadAnswerPayload,
): Record<string, string | ThreadWidgetOrigin> {
  const body: Record<string, string | ThreadWidgetOrigin> = { addr, id }
  if (payload.origin) body.origin = threadWidgetOriginBody(payload.origin)
  if (payload.answer !== undefined) body.answer = payload.answer
  if (payload.item) body.item = payload.item
  if (payload.reason) body.reason = payload.reason
  if (payload.confirmToken) body.confirmToken = payload.confirmToken
  if (payload.client) body.client = payload.client
  if (payload.secret !== undefined) {
    body.secret = payload.secret
    if (payload.scope) body.scope = payload.scope
    if (payload.hosts) body.hosts = payload.hosts.join(',')
    if (payload.delivery) body.delivery = payload.delivery
    if (payload.confirm) body.confirm = '1'
    if (payload.disclosed) body.disclosed = '1'
  }
  return body
}

export interface OverlayDoc {
  id: string
  kind: string
  from: string
  to?: string | null
  created_at?: number
  body?: string | null
  via?: string | null
  choice?: OverlayChoice | null
  secret?: OverlaySecret | null
  handoff?: OverlayHandoff | null
  /** Set when a Garden widget sent this post for the person
   *  (prd-zen-user-widgets-v2 UWB12a; daemon feature `thread-widget-origin`). */
  widget?: { widget: string; garden: string } | null
  /** Set when a Design tab (or `k2 browser note`) sent these page notes
   *  (prd-browser-design-mode-v1 D-A10). */
  design?: { itemId?: string; url: string; notes: number } | null
  /** Who sent a person's post, stamped by the daemon from the credential
   *  (`owner` / `user` / `guest`). Absent on agent posts and older docs;
   *  `isOwnThreadPost` (lib/thread-viewer) decides "You". */
  author?: ThreadAuthor | null
  /** Chatter only: the `m_…` id of this live message's `msg_deliveries`
   *  receipt (`k2 msg who <id>`). Absent on unrecorded sends and older docs. */
  deliveryId?: string | null
  /** Chatter only (0.46.3): one line for people — the sender's
   *  (`k2 msg --summary`) or derived from the body's first line. */
  summary?: string | null
  /** `sender` | `derived`: label a derived one "first line · no summary sent". */
  summarySource?: 'sender' | 'derived' | null
  /** File objects riding the message (0.46.1, daemon feature
   *  `thread-attachments-v1`): pointers to files in the Thread's workspace,
   *  set only by the daemon. Absent on older docs and servers. */
  attachments?: ThreadFileObject[] | null
  /** The Board context that rode the person's post (0.46.3 r10, daemon
   *  feature `thread-context-v1`): a Garden widget's line, shown as a chip,
   *  never the person's words. Absent on every other doc. */
  context?: ThreadContextDoc | null
}

/** A doc's `context` (`k2_core::overlay::doc::ThreadContext`). */
export interface ThreadContextDoc {
  text: string
  /** The widget's folder name (author-chosen). */
  widget: string
  garden: string
  placement: string
}

/** The `context` a compose post sends (thread-context-v1): the text and
 *  where it came from. `origin.hash` only to this computer's daemon. */
export interface ThreadContextBody {
  text: string
  origin: { widget: string; garden: string; placement: string; hash?: string }
}

/** Live state of a Thread file object, computed by the daemon on each read
 *  (`ok` on a frame sent at post time). `hidden`: an App guest whose own
 *  file rights don't cover the path; it carries no name or path. */
export type ThreadFileState = 'ok' | 'changed' | 'gone' | 'hidden'

/** One file object on a Thread message (`k2_core::overlay::attachments`). */
export interface ThreadFileObject {
  kind: 'file' | string
  state?: ThreadFileState
  /** Workspace-relative, `/` separators. Absent when `hidden`. */
  path?: string
  name?: string
  size?: number
  mime?: string
  modifiedAt?: number
  sha256?: string | null
  line?: number
  lineEnd?: number
  /** The daemon's answer for THIS viewer (0.46.3): may they save the file
   *  (`mode=download`)? People: true; an App guest only with
   *  `files:download`. Absent (an older server, an agent's view): no
   *  Download button. */
  canDownload?: boolean
}

export interface OverlayThreadItem {
  collection: string
  seq: number
  id: string
  doc: OverlayDoc
  conversation_id?: string | null
  /** This window's own send, before the server's copy came back:
   *  `doc.created_at` is the local send time (the `thread/post` answer has
   *  no `created_at`). The server's stamp replaces it on merge, and a local
   *  time never replaces a server one. */
  localTime?: true
}

export interface OverlaySnapshot {
  conversation_id: string
  items: OverlayThreadItem[]
  has_more: boolean
}

export interface OverlayWsFrame {
  collection?: string
  seq?: number
  id?: string
  doc?: OverlayDoc | null
  /** `collection: "activity"` only: the Thread turn (§7.6). Never an item. */
  activity?: unknown
  /** `collection: "moved"` only: the Thread's old and new conversation keys. */
  from?: string
  to?: string
  /** `collection: "address"` only (rename, TR13): the new and old address. */
  address?: string
  previous?: string
}

/**
 * The conversation a `moved` frame says this Thread now lives under, or
 * null for any other frame. A harness that mints its own id (Codex,
 * Hermes) keeps its Thread under the pane key until K2 finds the id; then
 * the daemon moves the Thread to that id and tells every socket on either
 * key (overlay_ws.rs `publish_moved`). Never an item.
 */
export function movedConversation(frame: OverlayWsFrame): string | null {
  if (frame.collection !== 'moved') return null
  const to = typeof frame.to === 'string' && frame.to.trim() ? frame.to : frame.id
  return typeof to === 'string' && to.trim() ? to.trim() : null
}

export function isChatterDoc(doc: OverlayDoc | null | undefined): boolean {
  return (doc?.kind ?? '').trim() === 'chatter'
}

/** Thread tab walks the Thread collection only. Never mix in A2A. */
export function isThreadSurfaceItem(item: OverlayThreadItem): boolean {
  if (item.collection !== 'thread') return false
  if (isChatterDoc(item.doc)) return false
  return true
}

export function threadItemsFromSnapshot(raw: unknown): OverlaySnapshot {
  const obj = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const conversation_id =
    typeof obj.conversation_id === 'string' ? obj.conversation_id : ''
  const rawItems = Array.isArray(obj.items) ? obj.items : []
  const items: OverlayThreadItem[] = []
  for (const row of rawItems) {
    const item = coerceItem(row)
    if (item && isThreadSurfaceItem(item)) items.push(item)
  }
  items.sort((a, b) => a.seq - b.seq)
  return { conversation_id, items, has_more: obj.has_more === true }
}

/** Prepend unique older items (by id); keep ascending seq. */
export function mergeOlderOverlayItems(
  current: OverlayThreadItem[],
  older: OverlayThreadItem[],
): OverlayThreadItem[] {
  if (older.length === 0) return current
  const seen = new Set(current.map((it) => it.id))
  const prepend = older.filter((it) => !seen.has(it.id))
  if (prepend.length === 0) return current
  return [...prepend, ...current].sort((a, b) => a.seq - b.seq)
}

export function overlaySeq(raw: unknown): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) return raw
  if (typeof raw === 'string' && raw.trim() !== '') {
    const n = Number(raw)
    if (Number.isFinite(n)) return n
  }
  return Number.NaN
}

type OverlayLiveListener = (item: OverlayThreadItem) => void
const threadLiveListeners = new Set<OverlayLiveListener>()

/** Compose `thread/post` is a sibling of the overlay hook — push the
 *  returned row in so the Thread list does not wait on overlay WS. */
export function ingestOverlayThreadItem(item: OverlayThreadItem): void {
  if (item.collection !== 'thread' || !item.id) return
  for (const listener of threadLiveListeners) listener(item)
}

export function subscribeOverlayThreadLive(listener: OverlayLiveListener): () => void {
  threadLiveListeners.add(listener)
  return () => {
    threadLiveListeners.delete(listener)
  }
}

/** The row for this window's own `thread/post`. The answer carries no
 *  `created_at`, so the row is stamped with the send time (`sentAtSec`,
 *  unix seconds) and reads "just now" at once, like an agent's message; the
 *  socket's copy brings the server's stamp (`mergeThreadItems`). */
export function overlayItemFromThreadPost(
  resp: Record<string, unknown> | null | undefined,
  fallbackBody: string,
  sentAtSec: number = Math.floor(Date.now() / 1000),
): OverlayThreadItem | null {
  if (!resp || resp.ok === false) return null
  const id = typeof resp.id === 'string' ? resp.id : ''
  if (!id) return null
  const seq = overlaySeq(resp.seq)
  const conversation_id =
    typeof resp.conversation_id === 'string' ? resp.conversation_id : null
  const serverAt =
    typeof resp.created_at === 'number' && Number.isFinite(resp.created_at) ? resp.created_at : null
  const item: OverlayThreadItem = {
    collection: 'thread',
    seq: Number.isFinite(seq) ? seq : 0,
    id,
    conversation_id,
    doc: {
      id,
      kind: typeof resp.kind === 'string' ? resp.kind : 'text',
      from: typeof resp.from === 'string' ? resp.from : '',
      to: typeof resp.to === 'string' ? resp.to : null,
      created_at: serverAt ?? sentAtSec,
      body: typeof resp.body === 'string' ? resp.body : fallbackBody,
      via: typeof resp.via === 'string' ? resp.via : 'compose',
      ...docExtras(resp),
    },
  }
  if (serverAt === null) item.localTime = true
  return item
}

/** What `postThreadCompose` did: the server took it (`item` is the new row,
 *  already pushed to every live Thread list), or it answered `ok: false`. */
export type ThreadComposeResult =
  /** `contextRefused`: the server took the message but not its Board
   *  context (thread-context-v1), and says why. */
  | { ok: true; item: OverlayThreadItem | null; contextRefused?: { code: string; message: string } }
  /** `code`: the daemon's refusal code when it sent one (`attachment_*`). */
  | { ok: false; error: string; code?: string }

/** One file on a compose post (`thread-attachments-v1`). */
export interface ThreadPostAttachment {
  path: string
  line?: number
  lineEnd?: number
}

/**
 * The one "message the agent on its Thread" send (prd-zen-mode-v1 Z42,
 * vs-live Z67): `POST /cli/thread/post {addr, text, via:'compose'}` on the
 * agent's own server, then the returned row goes to every live Thread list
 * (`ingestOverlayThreadItem`) without waiting on the overlay socket. A
 * `via=compose` post wakes a dormant session and injects `[thread:<addr>]`
 * into the agent's PTY (overlay_routes.rs). The Agents page compose bar and
 * Zen's compose both send through here. Throws on a transport error.
 */
/** Compose-bar text when a Thread send's address no longer resolves. */
export const THREAD_ADDRESS_MOVED =
  "This chat's address changed. Looking up the new one; your draft is still here, send again."

export async function postThreadCompose(
  scope: ServerScope,
  addr: string,
  text: string,
  command?: string | null,
  /** prd-zen-user-widgets-v2 UWB12a: a custom Garden widget's post names
   *  its widget (shown "You · via <widget>", kept out of compose history).
   *  Only sent to a server that reports `thread-widget-origin`; its
   *  `placement` + `hash` only to this computer's daemon (0.46.1). */
  extra?: {
    origin?: ThreadWidgetOrigin
    /** S4 (`thread-attachments-v1`): files riding the post, paths on the
     *  Thread's server inside its workspace. `text` may then be empty. The
     *  daemon refuses the whole post on a bad one (`attachment_*`). */
    attachments?: ThreadPostAttachment[]
    /** thread-context-v1: the Board context chip the person saw. Only to a
     *  server that reports the feature (the caller checks). */
    context?: ThreadContextBody
  },
): Promise<ThreadComposeResult> {
  const body: {
    addr: string
    text: string
    via: string
    command?: string
    origin?: ThreadWidgetOrigin
    attachments?: ThreadPostAttachment[]
    context?: ThreadContextBody
  } = {
    addr,
    text,
    via: 'compose',
  }
  if (command) body.command = command
  if (extra?.origin) body.origin = threadWidgetOriginBody(extra.origin)
  if (extra?.context) {
    const o = extra.context.origin
    body.context = {
      text: extra.context.text,
      origin: { widget: o.widget, garden: o.garden, placement: o.placement, ...(o.hash !== undefined ? { hash: o.hash } : {}) },
    }
  }
  if (extra?.attachments && extra.attachments.length > 0) {
    body.attachments = extra.attachments.map((a) => ({
      path: a.path,
      ...(typeof a.line === 'number' ? { line: a.line } : {}),
      ...(typeof a.lineEnd === 'number' ? { lineEnd: a.lineEnd } : {}),
    }))
  }
  let resp: Record<string, unknown>
  try {
    resp = await daemonCliPost<Record<string, unknown>>(scope, 'thread/post', body)
  } catch (e) {
    // Thread survives a tab rename (S4): a 404 on this address (an older
    // daemon after a rename) makes the sidecar view look it up again; the
    // person sees why the send did not post (Side finding C).
    if (isThreadAddressMiss(e)) {
      requestThreadAddressRelookup(addr)
      return { ok: false, error: THREAD_ADDRESS_MOVED }
    }
    const text = threadErrorText(e)
    const code = httpCodeOf(e)
    // S4: a refused file says which rule, in words (the daemon's hint).
    const refusal = threadAttachmentRefusalText(e, text)
    return { ok: false, error: refusal ?? text, ...(code ? { code } : {}) }
  }
  if (resp?.ok === false) {
    return { ok: false, error: typeof resp.error === 'string' ? resp.error : 'The server refused the message.' }
  }
  if (typeof resp?.movedFrom === 'string' && typeof resp.addr === 'string' && resp.addr !== addr) {
    requestThreadAddressRelookup(addr, resp.addr)
  }
  const item = overlayItemFromThreadPost(resp, text)
  if (item) ingestOverlayThreadItem(item)
  const refused = resp?.contextRefused
  if (refused && typeof refused === 'object' && !Array.isArray(refused)) {
    const r = refused as Record<string, unknown>
    return {
      ok: true,
      item,
      contextRefused: {
        code: typeof r.code === 'string' ? r.code : 'failed',
        message: typeof r.message === 'string' && r.message ? r.message : 'The server didn’t take the Board context.',
      },
    }
  }
  return { ok: true, item }
}

/**
 * The oldest seq a Thread list holds while older pages are still on the
 * server (`hasMore`), else 0. A frame for an id the list doesn't have,
 * below this seq, is an older card changing (it shows when its page loads);
 * every other new id joins the list.
 */
export function threadWindowFloor(items: OverlayThreadItem[], hasMore: boolean): number {
  if (!hasMore || items.length === 0) return 0
  return items.reduce((m, it) => (it.seq < m ? it.seq : m), Number.POSITIVE_INFINITY)
}

function sameItem(a: OverlayThreadItem, b: OverlayThreadItem): boolean {
  return (
    a.seq === b.seq &&
    a.localTime === b.localTime &&
    JSON.stringify(a.doc) === JSON.stringify(b.doc)
  )
}

/** The same message drawn the same: one object, or equal by content (a
 *  re-read or a cache swap hands over new, equal objects). What the
 *  memoized Thread rows compare by. */
export function sameThreadItem(a: OverlayThreadItem, b: OverlayThreadItem): boolean {
  if (a === b) return true
  return a.id === b.id && a.collection === b.collection && (a.doc === b.doc ? a.seq === b.seq && a.localTime === b.localTime : sameItem(a, b))
}

/**
 * Thread sync: merge rows from any source (a socket frame, a catch-up
 * `GET thread?since_seq=`, this window's own send) into a list BY ID.
 * A known id takes the incoming doc (a card answered elsewhere); a new id
 * is added; the list stays in seq order. Never a duplicate, whoever sent
 * it and however many paths deliver it. Returns `current` itself when
 * nothing changed.
 */
export function mergeThreadItems(
  current: OverlayThreadItem[],
  incoming: OverlayThreadItem[],
): OverlayThreadItem[] {
  let next: OverlayThreadItem[] | null = null
  for (const raw of incoming) {
    if (!raw.id || !isThreadSurfaceItem({ ...raw, collection: raw.collection || 'thread' })) continue
    const item: OverlayThreadItem = { ...raw, collection: 'thread' }
    const list: OverlayThreadItem[] = next ?? current
    const at = list.findIndex((it) => it.id === item.id)
    if (at >= 0) {
      const prev = list[at]
      const prevServerAt = !prev.localTime && typeof prev.doc.created_at === 'number'
      // A send's local time never replaces the server's stamp (the socket's
      // copy can land before the send's answer).
      const doc =
        item.localTime && prevServerAt ? { ...item.doc, created_at: prev.doc.created_at } : item.doc
      const merged: OverlayThreadItem = {
        ...prev,
        ...item,
        doc,
        conversation_id: item.conversation_id ?? prev.conversation_id,
      }
      if (item.localTime && !prevServerAt) merged.localTime = true
      else delete merged.localTime
      if (sameItem(list[at], merged)) continue
      next ??= current.slice()
      next[at] = merged
    } else {
      next ??= current.slice()
      next.push(item)
    }
  }
  if (!next) return current
  next.sort((a, b) => a.seq - b.seq)
  return next
}

/**
 * Apply one overlay socket frame to a Thread list (merge by id). `floorSeq`
 * is `threadWindowFloor(items, hasMore)`. A frame never drops because its
 * seq is at or below the newest seq seen: another sender's message can
 * reach this window after its own later send came back (the send's answer
 * and the socket are two connections).
 */
export function applyOverlayFrame(
  items: OverlayThreadItem[],
  frame: OverlayWsFrame,
  floorSeq: number,
): OverlayThreadItem[] {
  if (frame.collection !== 'thread') return items
  const seq = overlaySeq(frame.seq)
  const id = typeof frame.id === 'string' ? frame.id : ''
  if (!id) return items
  const doc = withChoiceResult(frame.doc)
  if (!doc || isChatterDoc(doc)) return items
  const existing = items.findIndex((it) => it.id === id)
  if (existing >= 0) {
    const { localTime: _local, ...prev } = items[existing]
    return mergeThreadItems(items, [
      { ...prev, seq: Number.isFinite(seq) ? seq : prev.seq, doc, collection: 'thread' },
    ])
  }
  // New id below the loaded window: an older card changing; its page has it.
  if (Number.isFinite(seq) && seq < floorSeq) return items
  // Missing seq still appends (after the newest) — never vanish.
  const newest = items.reduce((m, it) => (it.seq > m ? it.seq : m), 0)
  return mergeThreadItems(items, [
    { collection: 'thread', seq: Number.isFinite(seq) ? seq : newest + 1, id, doc },
  ])
}

// ── The Thread turn (prd-daemon-activity-and-thread-working-v1 S7) ─────────
//
// While the agent works on a Thread message, the daemon sends ephemeral
// `collection: "activity"` frames on the same overlay socket (thread_
// activity.rs, §7.6): never stored, no seq, never replayed by `since_seq`.
// They feed a separate `turn` (TW11), never `items`: `applyOverlayFrame`
// keeps its `thread` gate. On every socket (re)open the client reads the
// live turn from `GET /cli/thread/activity?addr=` (TW9).

/** The frame's coarse state. Live: working | needs-you | unverifiable.
 *  An end: idle | monitoring (a clean end) or stopped (anything else). */
export type ThreadTurnState = 'working' | 'monitoring' | 'needs-you' | 'unverifiable' | 'stopped' | 'idle'

/** What the agent is doing inside the turn (TW6; `stale` is A24).
 *  `children`: the agent replied, and its subagents or background tasks
 *  are still running (state `working`, or `monitoring` when only
 *  background tasks remain); the turn ends when they finish. */
export type ThreadTurnPhase = 'delivering' | 'working' | 'tool' | 'thinking' | 'waiting' | 'stale' | 'children'

export interface ThreadTurnTally {
  read: number
  search: number
  cmd: number
  edit: number
}

export interface ThreadTurnEnd {
  /** reply | done | interrupted | failed | session_gone | delivery_failed | superseded | stale.
   *  `superseded`: a message sent while the turn wasn't running (or, from a
   *  server before 0.46.1, any message sent mid-turn). From 0.46.1 a message
   *  sent while the agent works joins the running turn instead (same
   *  `turnId`, `messages` + 1): no end at all. */
  reason: string
  detail: string | null
  /** Server ms. */
  at: number
}

/** One child of the turn's session, by name (0.46.5, Rosson 2026-10-27:
 *  "show what's actually going on"): a subagent, a background shell or a
 *  monitor. The daemon picks the label (a subagent's type, a shell's
 *  redacted command line, a task type); the renderer only says it. */
export interface ThreadTurnChild {
  /** `task`: any other background task the CLI reports (shown, never
   *  holding the turn). */
  kind: 'subagent' | 'shell' | 'monitor' | 'task'
  /** `owed`: a subagent that finished and whose result the agent hasn't
   *  taken yet. */
  state: 'running' | 'waiting' | 'owed'
  label: string | null
  /** Server ms. */
  since: number
}

export interface ThreadTurn {
  turnId: string
  state: ThreadTurnState
  phase: ThreadTurnPhase
  /** Server ms: the user's message (the first one, when more joined it). */
  startedAt: number
  /** The person's messages in this turn: a message sent while the agent
   *  works joins its running turn (0.46.1). 1 from an older server. */
  messages?: number
  /** Server ms: the newest of those messages. `startedAt` from an older
   *  server. */
  lastComposeAt?: number
  /** Server ms: the current phase. */
  phaseSince: number
  /** The redacted tool line (TW8) while `phase` is `tool`, else null. */
  line: string | null
  subagents: number
  subagentsDone: number
  background: number
  /** The live children by name, oldest first (0.46.5). Absent from an
   *  older server: the counts above are all there is. */
  children?: ThreadTurnChild[]
  /** The agent has posted in Thread during this turn (0.46.5). Absent
   *  from an older server, whose `children` phase only ever followed a
   *  reply. */
  replied?: boolean
  /** Frame schema 2 (0.46.5, additive; absent from an older server). */
  schema?: number
  /** The tool step's kind while `phase` is `tool`, else null. */
  stepKind?: 'read' | 'search' | 'cmd' | 'edit' | 'web' | 'subagent' | 'tool' | null
  /** The live children by kind. */
  childCounts?: { subagent: number; shell: number; monitor: number; task: number }
  /** Why the turn is still open; null while delivering. */
  openBecause?: 'lead' | 'children' | 'waiting' | 'stale' | null
  /** Server ms: the agent's last evidence of any kind. */
  evidenceAt?: number | null
  tally: ThreadTurnTally
  /** Phase `waiting`: what the agent is stuck on. Null otherwise (and from
   *  a server that doesn't say). */
  waitingOn: 'permission' | 'question' | null
  end: ThreadTurnEnd | null
  rev: number
  /** `serverNow − Date.now()` when this frame arrived. Server times read
   *  on this client's clock as `Date.now() + skewMs`. */
  skewMs: number
  /** Server ms: when the daemon built this frame (the frame's own
   *  `serverNow`, 0.46.5), so a projection can hand it on unchanged. */
  serverNow?: number
}

const TURN_STATES: readonly ThreadTurnState[] = ['working', 'monitoring', 'needs-you', 'unverifiable', 'stopped', 'idle']
const TURN_PHASES: readonly ThreadTurnPhase[] = ['delivering', 'working', 'tool', 'thinking', 'waiting', 'stale', 'children']

function finite(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function count(v: unknown): number {
  const n = finite(v)
  return n !== null && n > 0 ? Math.floor(n) : 0
}

const CHILD_KINDS: readonly ThreadTurnChild['kind'][] = ['subagent', 'shell', 'monitor', 'task']
const CHILD_STATES: readonly ThreadTurnChild['state'][] = ['running', 'waiting', 'owed']
const STEP_KINDS: readonly NonNullable<ThreadTurn['stepKind']>[] = ['read', 'search', 'cmd', 'edit', 'web', 'subagent', 'tool']
const OPEN_BECAUSE: readonly NonNullable<ThreadTurn['openBecause']>[] = ['lead', 'children', 'waiting', 'stale']

/** The frame's `children`, or undefined when it has none (an older
 *  server). Unknown kinds and states are dropped, never guessed. */
function coerceChildren(raw: unknown, fallbackSince: number): ThreadTurnChild[] | undefined {
  if (!Array.isArray(raw)) return undefined
  const out: ThreadTurnChild[] = []
  for (const c of raw) {
    if (!c || typeof c !== 'object') continue
    const r = c as Record<string, unknown>
    const kind = CHILD_KINDS.find((k) => k === r.kind)
    const state = CHILD_STATES.find((s) => s === r.state)
    if (!kind || !state) continue
    const label = typeof r.label === 'string' && r.label.trim() ? r.label.trim() : null
    out.push({ kind, state, label, since: finite(r.since) ?? fallbackSince })
  }
  return out
}

/** One §7.6 activity body, or null when it isn't one. `receivedAt` is this
 *  client's clock when it arrived (for the `serverNow` skew). */
export function coerceThreadTurn(raw: unknown, receivedAt: number): ThreadTurn | null {
  if (!raw || typeof raw !== 'object') return null
  const a = raw as Record<string, unknown>
  const turnId = typeof a.turnId === 'string' ? a.turnId : ''
  const startedAt = finite(a.startedAt)
  const serverNow = finite(a.serverNow)
  if (!turnId || startedAt === null || serverNow === null) return null
  const state = TURN_STATES.find((s) => s === a.state)
  const phase = TURN_PHASES.find((p) => p === a.phase)
  if (!state || !phase) return null
  const t = a.tally && typeof a.tally === 'object' ? (a.tally as Record<string, unknown>) : {}
  let end: ThreadTurnEnd | null = null
  if (a.end && typeof a.end === 'object') {
    const e = a.end as Record<string, unknown>
    end = {
      reason: typeof e.reason === 'string' ? e.reason : 'done',
      detail: typeof e.detail === 'string' ? e.detail : null,
      at: finite(e.at) ?? serverNow,
    }
  }
  return {
    turnId,
    state,
    phase,
    startedAt,
    messages: Math.max(1, count(a.messages)),
    lastComposeAt: finite(a.lastComposeAt) ?? startedAt,
    phaseSince: finite(a.phaseSince) ?? startedAt,
    line: typeof a.line === 'string' && a.line.trim() ? a.line : null,
    subagents: count(a.subagents),
    subagentsDone: count(a.subagentsDone),
    background: count(a.background),
    ...(Array.isArray(a.children) ? { children: coerceChildren(a.children, startedAt) } : {}),
    ...(typeof a.replied === 'boolean' ? { replied: a.replied } : {}),
    ...(finite(a.schema) !== null ? { schema: finite(a.schema) as number } : {}),
    ...('stepKind' in a ? { stepKind: STEP_KINDS.find((k) => k === a.stepKind) ?? null } : {}),
    ...(a.childCounts && typeof a.childCounts === 'object'
      ? (() => {
          const c = a.childCounts as Record<string, unknown>
          return { childCounts: { subagent: count(c.subagent), shell: count(c.shell), monitor: count(c.monitor), task: count(c.task) } }
        })()
      : {}),
    ...('openBecause' in a ? { openBecause: OPEN_BECAUSE.find((k) => k === a.openBecause) ?? null } : {}),
    ...('evidenceAt' in a ? { evidenceAt: finite(a.evidenceAt) } : {}),
    tally: { read: count(t.read), search: count(t.search), cmd: count(t.cmd), edit: count(t.edit) },
    waitingOn: a.waitingOn === 'permission' || a.waitingOn === 'question' ? a.waitingOn : null,
    end,
    rev: finite(a.rev) ?? 0,
    skewMs: serverNow - receivedAt,
    serverNow,
  }
}

/** A newer state of the turn wins: another turn replaces it (a new message
 *  after the old turn, or one that superseded it), the same turn moves only
 *  forward by `rev` (a message that joined the running turn is one more
 *  frame of the same turn). */
function nextTurn(current: ThreadTurn | null, incoming: ThreadTurn): ThreadTurn {
  if (current && current.turnId === incoming.turnId && incoming.rev < current.rev) return current
  return incoming
}

/** Apply one overlay socket frame to the Thread turn. Anything but an
 *  `activity` frame leaves it as it is (returns `turn` itself). */
export function applyActivityFrame(
  turn: ThreadTurn | null,
  frame: OverlayWsFrame,
  receivedAt: number,
): ThreadTurn | null {
  if (frame.collection !== 'activity') return turn
  const incoming = coerceThreadTurn(frame.activity, receivedAt)
  return incoming ? nextTurn(turn, incoming) : turn
}

/** Apply the catch-up `GET /cli/thread/activity` body: `{turn: null}` means
 *  no turn is running; a body without `turn` changes nothing. */
export function applyActivityCatchUp(
  turn: ThreadTurn | null,
  body: unknown,
  receivedAt: number,
): ThreadTurn | null {
  if (!body || typeof body !== 'object' || !('turn' in body)) return turn
  const raw = (body as { turn: unknown }).turn
  if (raw === null) return null
  const incoming = coerceThreadTurn(raw, receivedAt)
  return incoming ? nextTurn(turn, incoming) : turn
}

/** The turn is running (not ended). */
export function isTurnLive(turn: ThreadTurn | null | undefined): turn is ThreadTurn {
  return !!turn && turn.end === null
}

/** Chatter tab walks the Chatter collection only. Never mix in Thread. */
export function isChatterSurfaceItem(item: OverlayThreadItem): boolean {
  return item.collection === 'chatter'
}

export function chatterItemsFromSnapshot(raw: unknown): OverlaySnapshot {
  const obj = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const conversation_id =
    typeof obj.conversation_id === 'string' ? obj.conversation_id : ''
  const rawItems = Array.isArray(obj.items) ? obj.items : []
  const items: OverlayThreadItem[] = []
  for (const row of rawItems) {
    const item = coerceItem(row)
    if (item && isChatterSurfaceItem(item)) items.push(item)
  }
  items.sort((a, b) => a.seq - b.seq)
  return { conversation_id, items, has_more: obj.has_more === true }
}

export function applyChatterFrame(
  items: OverlayThreadItem[],
  frame: OverlayWsFrame,
  snapshotSeq: number,
): OverlayThreadItem[] {
  if (frame.collection !== 'chatter') return items
  const seq = typeof frame.seq === 'number' ? frame.seq : Number.NaN
  const id = typeof frame.id === 'string' ? frame.id : ''
  if (!id) return items
  const doc = frame.doc
  if (!doc) return items
  const existing = items.findIndex((it) => it.id === id)
  if (existing >= 0) {
    const next = items.slice()
    next[existing] = {
      ...items[existing],
      seq: Number.isFinite(seq) ? seq : items[existing].seq,
      doc,
      collection: 'chatter',
    }
    return next
  }
  if (!Number.isFinite(seq) || seq <= snapshotSeq) return items
  const next = [
    ...items,
    { collection: 'chatter', seq, id, doc },
  ]
  next.sort((a, b) => a.seq - b.seq)
  return next
}

/**
 * Tear down an overlay events socket without aborting a CONNECTING
 * handshake (WebKit logs that as "The network connection was lost").
 */
export function releaseOverlayWebSocket(ws: {
  readyState: number
  close: () => void
  onmessage: ((ev: MessageEvent) => void) | null
  onerror: ((ev: Event) => void) | null
  onopen: ((ev: Event) => void) | null
}): void {
  ws.onmessage = null
  ws.onerror = null
  if (ws.readyState === 0) {
    ws.onopen = () => {
      try {
        ws.close()
      } catch {
        /* ignore */
      }
    }
    return
  }
  ws.onopen = null
  if (ws.readyState === 1) {
    try {
      ws.close()
    } catch {
      /* ignore */
    }
  }
}

export function isVoidedHitl(doc: OverlayDoc): boolean {
  if (doc.kind === 'choice' && doc.choice?.status === 'voided') return true
  if (doc.kind === 'secret' && doc.secret?.status === 'voided') return true
  return false
}

function coerceItem(row: unknown): OverlayThreadItem | null {
  if (!row || typeof row !== 'object') return null
  const r = row as Record<string, unknown>
  const doc = r.doc
  if (!doc || typeof doc !== 'object') return null
  const d = doc as Record<string, unknown>
  const id = typeof r.id === 'string' ? r.id : typeof d.id === 'string' ? d.id : ''
  if (!id) return null
  const seq = typeof r.seq === 'number' ? r.seq : 0
  const collection = typeof r.collection === 'string' ? r.collection : 'thread'
  return {
    collection,
    seq,
    id,
    conversation_id: typeof r.conversation_id === 'string' ? r.conversation_id : null,
    doc: {
      id: typeof d.id === 'string' ? d.id : id,
      kind: typeof d.kind === 'string' ? d.kind : 'text',
      from: typeof d.from === 'string' ? d.from : '',
      to: typeof d.to === 'string' ? d.to : null,
      created_at: typeof d.created_at === 'number' ? d.created_at : undefined,
      body: typeof d.body === 'string' ? d.body : null,
      via: typeof d.via === 'string' ? d.via : null,
      choice: coerceChoice(d.choice),
      secret: coerceSecret(d.secret),
      ...(d.handoff ? { handoff: coerceHandoff(d.handoff) } : {}),
      ...docExtras(d),
    },
  }
}

function coerceChoice(raw: unknown): OverlayChoice | null {
  if (!raw || typeof raw !== 'object') return null
  const c = raw as Record<string, unknown>
  const optionsRaw = Array.isArray(c.options) ? c.options : []
  const options = optionsRaw
    .map((o) => {
      if (typeof o === 'string') return { label: o }
      if (o && typeof o === 'object' && typeof (o as { label?: unknown }).label === 'string') {
        return { label: (o as { label: string }).label }
      }
      return null
    })
    .filter((o): o is { label: string } => o !== null)
  return {
    prompt: typeof c.prompt === 'string' ? c.prompt : '',
    options,
    allow_custom: c.allow_custom === true,
    status: typeof c.status === 'string' ? c.status : 'pending',
    answer: typeof c.answer === 'string' ? c.answer : null,
    ...closedExtras(c),
    ...actionResultMessage(c),
    ...permissionExtra(c),
  }
}

/** `permission` when the choice is a permission card with a `cards=2` view
 *  (left off otherwise, so every other card coerces as before). */
function permissionExtra(src: Record<string, unknown>): Pick<OverlayChoice, 'permission'> {
  const permission = readPermissionCard(src.action)
  return permission ? { permission } : {}
}

/** A live frame's doc (frames are not coerced like snapshot rows): a
 *  settings card gains `choice.resultMessage` from `action.result.message`,
 *  a permission card `choice.permission` from its view; every other doc is
 *  returned as is (the same object). */
function withChoiceResult(doc: OverlayDoc | null | undefined): OverlayDoc | null | undefined {
  const choice = doc?.choice as (OverlayChoice & Record<string, unknown>) | null | undefined
  if (!doc || !choice || typeof choice !== 'object') return doc
  const extra = { ...actionResultMessage(choice), ...permissionExtra(choice) }
  return extra.resultMessage || extra.permission ? { ...doc, choice: { ...choice, ...extra } } : doc
}

/** `resultMessage` from `action.result.message` when the daemon sent one
 *  (left off otherwise, so a card without an action coerces as before). */
function actionResultMessage(src: Record<string, unknown>): Pick<OverlayChoice, 'resultMessage'> {
  const action = src.action
  if (!action || typeof action !== 'object' || Array.isArray(action)) return {}
  const result = (action as Record<string, unknown>).result
  if (!result || typeof result !== 'object' || Array.isArray(result)) return {}
  const message = (result as Record<string, unknown>).message
  return typeof message === 'string' && message.trim() ? { resultMessage: message.trim() } : {}
}

/** `answeredBy` / `answeredAt` / `expiresAt` when the daemon sent them
 *  (left off otherwise, so an older card coerces as before). */
function closedExtras(
  src: Record<string, unknown>,
): Pick<OverlayChoice, 'answeredBy' | 'answeredAt' | 'expiresAt'> {
  const out: Pick<OverlayChoice, 'answeredBy' | 'answeredAt' | 'expiresAt'> = {}
  const by = parseThreadAuthor(src.answeredBy)
  if (by) out.answeredBy = by
  if (typeof src.answeredAt === 'number') out.answeredAt = src.answeredAt
  if (typeof src.expiresAt === 'number') out.expiresAt = src.expiresAt
  return out
}

const plural = (n: number, unit: string): string => `${n} ${unit}${n === 1 ? '' : 's'}`

/** "expires in 3 days" for a pending card with a time frame; "expired"
 *  once its time passed (the daemon's sweep closes it). Null without one. */
export function cardExpiryLabel(expiresAt: number | null | undefined, nowSec: number): string | null {
  if (typeof expiresAt !== 'number') return null
  const left = expiresAt - nowSec
  if (left <= 0) return 'expired'
  if (left < 3600) return `expires in ${plural(Math.max(1, Math.ceil(left / 60)), 'minute')}`
  if (left < 2 * 86400) return `expires in ${plural(Math.round(left / 3600), 'hour')}`
  return `expires in ${plural(Math.round(left / 86400), 'day')}`
}

export function coerceHandoff(raw: unknown): OverlayHandoff | null {
  if (!raw || typeof raw !== 'object') return null
  const h = raw as Record<string, unknown>
  const deliveryId = typeof h.deliveryId === 'string' ? h.deliveryId : ''
  if (!deliveryId) return null
  // Names and the reference only; anything else on the card is dropped.
  const out: OverlayHandoff = {
    deliveryId,
    name: typeof h.name === 'string' ? h.name : '',
    from: typeof h.from === 'string' ? h.from : '',
    kind: typeof h.kind === 'string' ? h.kind : '',
    hosts: Array.isArray(h.hosts) ? h.hosts.filter((x): x is string => typeof x === 'string') : [],
    note: typeof h.note === 'string' ? h.note : '',
    clash: h.clash === true,
    status: typeof h.status === 'string' ? h.status : 'pending',
  }
  if (typeof h.host === 'string') out.host = h.host
  if (typeof h.reason === 'string') out.reason = h.reason
  if (typeof h.reference === 'string') out.reference = h.reference
  if (typeof h.workspace === 'string') out.workspace = h.workspace
  Object.assign(out, closedExtras(h))
  return out
}

function coerceSecret(raw: unknown): OverlaySecret | null {
  if (!raw || typeof raw !== 'object') return null
  const s = raw as Record<string, unknown>
  const out: OverlaySecret = {
    name: typeof s.name === 'string' ? s.name : '',
    status: typeof s.status === 'string' ? s.status : 'pending',
    prompt: typeof s.prompt === 'string' ? s.prompt : null,
  }
  // Only the proposal and the reference are copied. A card body never
  // carries a value; anything else on it is dropped here.
  if (typeof s.scope === 'string') out.scope = s.scope
  if (Array.isArray(s.hosts)) out.hosts = s.hosts.filter((h): h is string => typeof h === 'string')
  if (typeof s.delivery === 'string') out.delivery = s.delivery
  if (typeof s.secretKind === 'string') out.secretKind = s.secretKind
  if (typeof s.ref === 'string') out.ref = s.ref
  Object.assign(out, closedExtras(s))
  return out
}
