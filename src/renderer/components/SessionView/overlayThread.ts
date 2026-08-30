/** Overlay Thread snapshot + WS frame helpers. Thread pane never shows chatter. */

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

export function applyOverlayFrame(
  items: OverlayThreadItem[],
  frame: OverlayWsFrame,
  snapshotSeq: number,
): OverlayThreadItem[] {
  if (frame.collection !== 'thread') return items
  const seq = overlaySeq(frame.seq)
  const id = typeof frame.id === 'string' ? frame.id : ''
  if (!id) return items
  const doc = frame.doc
  if (!doc || isChatterDoc(doc)) return items
  const existing = items.findIndex((it) => it.id === id)
  if (existing >= 0) {
    const next = items.slice()
    next[existing] = {
      ...items[existing],
      seq: Number.isFinite(seq) ? seq : items[existing].seq,
      doc,
      collection: 'thread',
    }
    return next
  }
  // New id: drop only true replays (seq already in the snapshot). Missing
  // seq still appends — compose ingest must not vanish if seq is omitted.
  if (Number.isFinite(seq) && seq <= snapshotSeq) return items
  const next = [
    ...items,
    { collection: 'thread', seq: Number.isFinite(seq) ? seq : snapshotSeq + 1, id, doc },
  ]
  next.sort((a, b) => a.seq - b.seq)
  return next
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
