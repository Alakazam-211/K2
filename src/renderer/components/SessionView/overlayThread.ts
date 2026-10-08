/** Overlay Thread snapshot + WS frame helpers. Thread pane never shows chatter. */

import { daemonCliPost } from '@/lib/daemon-cli'
import {
  isThreadAddressMiss,
  requestThreadAddressRelookup,
  threadErrorText,
} from '@/lib/thread-address-bus'
import type { ServerScope } from '@/kessel/server-scope'

export const OVERLAY_PAGE_SIZE = 25

export interface OverlayChoice {
  prompt: string
  options: { label: string }[]
  allow_custom: boolean
  status: string
  answer?: string | null
}

export interface OverlaySecret {
  name: string
  status: string
  prompt?: string | null
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
}

export interface OverlayThreadItem {
  collection: string
  seq: number
  id: string
  doc: OverlayDoc
  conversation_id?: string | null
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

export function overlayItemFromThreadPost(
  resp: Record<string, unknown> | null | undefined,
  fallbackBody: string,
): OverlayThreadItem | null {
  if (!resp || resp.ok === false) return null
  const id = typeof resp.id === 'string' ? resp.id : ''
  if (!id) return null
  const seq = overlaySeq(resp.seq)
  const conversation_id =
    typeof resp.conversation_id === 'string' ? resp.conversation_id : null
  return {
    collection: 'thread',
    seq: Number.isFinite(seq) ? seq : 0,
    id,
    conversation_id,
    doc: {
      id,
      kind: typeof resp.kind === 'string' ? resp.kind : 'text',
      from: typeof resp.from === 'string' ? resp.from : '',
      to: typeof resp.to === 'string' ? resp.to : null,
      body: typeof resp.body === 'string' ? resp.body : fallbackBody,
      via: typeof resp.via === 'string' ? resp.via : 'compose',
    },
  }
}

/** What `postThreadCompose` did: the server took it (`item` is the new row,
 *  already pushed to every live Thread list), or it answered `ok: false`. */
export type ThreadComposeResult =
  | { ok: true; item: OverlayThreadItem | null }
  | { ok: false; error: string }

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
): Promise<ThreadComposeResult> {
  const body: { addr: string; text: string; via: string; command?: string } = {
    addr,
    text,
    via: 'compose',
  }
  if (command) body.command = command
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
    return { ok: false, error: threadErrorText(e) }
  }
  if (resp?.ok === false) {
    return { ok: false, error: typeof resp.error === 'string' ? resp.error : 'The server refused the message.' }
  }
  if (typeof resp?.movedFrom === 'string' && typeof resp.addr === 'string' && resp.addr !== addr) {
    requestThreadAddressRelookup(addr, resp.addr)
  }
  const item = overlayItemFromThreadPost(resp, text)
  if (item) ingestOverlayThreadItem(item)
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
  return a.seq === b.seq && JSON.stringify(a.doc) === JSON.stringify(b.doc)
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
      const merged: OverlayThreadItem = {
        ...list[at],
        ...item,
        conversation_id: item.conversation_id ?? list[at].conversation_id,
      }
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
  const doc = frame.doc
  if (!doc || isChatterDoc(doc)) return items
  const existing = items.findIndex((it) => it.id === id)
  if (existing >= 0) {
    return mergeThreadItems(items, [
      { ...items[existing], seq: Number.isFinite(seq) ? seq : items[existing].seq, doc, collection: 'thread' },
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
  /** reply | done | interrupted | failed | session_gone | delivery_failed | superseded | stale */
  reason: string
  detail: string | null
  /** Server ms. */
  at: number
}

export interface ThreadTurn {
  turnId: string
  state: ThreadTurnState
  phase: ThreadTurnPhase
  /** Server ms: the user's message. */
  startedAt: number
  /** Server ms: the current phase. */
  phaseSince: number
  /** The redacted tool line (TW8) while `phase` is `tool`, else null. */
  line: string | null
  subagents: number
  subagentsDone: number
  background: number
  tally: ThreadTurnTally
  /** Phase `waiting`: what the agent is stuck on. Null otherwise (and from
   *  a server that doesn't say). */
  waitingOn: 'permission' | 'question' | null
  end: ThreadTurnEnd | null
  rev: number
  /** `serverNow − Date.now()` when this frame arrived. Server times read
   *  on this client's clock as `Date.now() + skewMs`. */
  skewMs: number
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
    phaseSince: finite(a.phaseSince) ?? startedAt,
    line: typeof a.line === 'string' && a.line.trim() ? a.line : null,
    subagents: count(a.subagents),
    subagentsDone: count(a.subagentsDone),
    background: count(a.background),
    tally: { read: count(t.read), search: count(t.search), cmd: count(t.cmd), edit: count(t.edit) },
    waitingOn: a.waitingOn === 'permission' || a.waitingOn === 'question' ? a.waitingOn : null,
    end,
    rev: finite(a.rev) ?? 0,
    skewMs: serverNow - receivedAt,
  }
}

/** A newer state of the turn wins: another turn replaces it (a new message
 *  supersedes the old turn), the same turn moves only forward by `rev`. */
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
  }
}

function coerceSecret(raw: unknown): OverlaySecret | null {
  if (!raw || typeof raw !== 'object') return null
  const s = raw as Record<string, unknown>
  return {
    name: typeof s.name === 'string' ? s.name : '',
    status: typeof s.status === 'string' ? s.status : 'pending',
    prompt: typeof s.prompt === 'string' ? s.prompt : null,
  }
}
