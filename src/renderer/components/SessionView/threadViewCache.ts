// The last messages of a Thread, shown at once on open
// (research-remote-thread-load-v1 Fix 4; speed pass P3b: the phone's chat
// cache model, K2 Companion `src/zen/chatCache.ts`).
//
// A VIEW cache: what this window last drew for one Thread on one server,
// kept so reopening it paints in the first frame, before any read. The
// daemon stays the truth: the hook brings the copy up to date with a delta
// (`after_seq` from the copy's newest server seq, `threadCacheMerge.ts`)
// or replaces it with the newest page, and nothing here is ever posted,
// counted as unread or marked read.
//
//   - Key: (server host key, Thread address). This computer's Threads and
//     other servers' alike.
//   - The newest 100 Thread messages per Thread (`CACHE_MAX_MESSAGES`).
//   - In memory: an LRU of 40 Threads, read synchronously (`peekThreadView`)
//     so the first frame already shows the chat.
//   - In IndexedDB on this computer (never localStorage: message bodies):
//     at most 150 Threads and 15 MB, least recently saved out first. Writes
//     go behind (coalesced per Thread) and are flushed when the window is
//     hidden. A small `meta` row per Thread (sizes, ages, cursor) is what
//     the bounds and the catch-up read, never the bodies. Several windows
//     share the database: the last write wins.
//   - Every IndexedDB call is wrapped: a window without it, or a failing
//     one, just has no saved copy.
//   - Versioned (`v`): a row of another version is ignored.
//   - Forgotten for a server when it is removed from this computer or its
//     login is signed out or replaced.
//
// The same database holds this computer's unsent Thread posts (the
// `outbox` store, `threadOutbox.ts`; thread-send-resilience 2026-11-11):
// a send written here first, shown as pending from here, and deleted only
// when the server confirmed it. Its rows are NOT dropped with a server's
// view rows (a token refresh after sleep "replaces" the login), and they
// are rows of their own (the view rows are rewritten whole by every read).

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { homeHostKey } from '@/lib/host-key'
import { isThreadSurfaceItem, type OverlayThreadItem } from './overlayThread'
import { CACHE_MAX_MESSAGES, cacheSlice, newestSeq } from './threadCacheMerge'

/** 3: 100 messages, `hasMore`, push-only rows marked `provisional`, local
 *  Threads too. 2: version 1 copies could hold a deleted message without
 *  its `deleted` flag (the read dropped it), painted as an empty bubble. */
export const THREAD_VIEW_CACHE_VERSION = 3
/** Messages kept per Thread. */
export const THREAD_VIEW_CACHE_ITEMS = CACHE_MAX_MESSAGES
/** Threads kept in this window's memory (the synchronous first paint). */
export const MEMORY_MAX_CHATS = 40
/** Threads kept on this computer. */
export const THREAD_VIEW_CACHE_THREADS = 150
/** Bytes kept on this computer (the JSON of the saved messages). */
export const THREAD_VIEW_CACHE_MAX_BYTES = 15 * 1024 * 1024
const DB_NAME = 'k2.thread-view-cache'
const DB_VERSION = 3
const STORE = 'threads'
/** One small row per saved Thread: what the bounds and the catch-up read. */
const META_STORE = 'meta'
/** Unsent Thread posts (`threadOutbox.ts`), keyed by `clientMsgId`. */
const OUTBOX_STORE = 'outbox'
/** Writes to IndexedDB are coalesced per Thread. */
const WRITE_DELAY_MS = 750

export interface CachedThreadView {
  hostKey: string
  addr: string
  conversationId: string
  items: OverlayThreadItem[]
  /** Older messages exist on the server than the newest in `items`. */
  hasMore: boolean
  savedAt: number
}

interface CacheRow extends CachedThreadView {
  key: string
  v: number
}

/** A saved Thread without its messages (`meta` store, `listThreadViews`). */
export interface ThreadViewMeta {
  key: string
  hostKey: string
  addr: string
  conversationId: string
  savedAt: number
  /** JSON size of the saved messages. */
  bytes: number
  /** Newest server seq held (`threadCacheMerge.newestSeq`), else null. */
  cursor: number | null
  v: number
}

const memory = new Map<string, CachedThreadView>()
const pendingWrites = new Map<string, ReturnType<typeof setTimeout>>()
let watching: (() => void) | null = null

function keyOf(hostKey: string, addr: string): string {
  return `${hostKey}|${addr.trim()}`
}

function touch(key: string, view: CachedThreadView): void {
  memory.delete(key)
  memory.set(key, view)
  while (memory.size > MEMORY_MAX_CHATS) {
    const oldest = memory.keys().next().value
    if (oldest === undefined) break
    // A copy still waiting for its write keeps it (the write reads memory).
    if (pendingWrites.has(oldest)) void flushOne(oldest)
    memory.delete(oldest)
  }
}

/** The newest Thread messages, oldest first, without this window's
 *  unconfirmed send times; `truncated` when older ones were cut. */
function trimItems(items: readonly OverlayThreadItem[]): { items: OverlayThreadItem[]; truncated: boolean } {
  const thread = items.filter((it) => isThreadSurfaceItem(it))
  const sorted = [...thread].sort((a, b) => a.seq - b.seq)
  const slice = cacheSlice(sorted, THREAD_VIEW_CACHE_ITEMS)
  return {
    truncated: slice.truncated,
    items: slice.messages.map((it) => {
      if (!it.localTime) return it
      const { localTime: _local, ...rest } = it
      return rest
    }),
  }
}

// ── IndexedDB (every call wrapped) ─────────────────────────────────────────

let opening: Promise<IDBDatabase> | null = null

function idbFactory(): IDBFactory | null {
  try {
    return (globalThis as { indexedDB?: IDBFactory }).indexedDB ?? null
  } catch {
    return null
  }
}

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result)
    r.onerror = () => reject(r.error ?? new Error('IndexedDB request failed'))
  })
}

function openDb(): Promise<IDBDatabase> | null {
  const f = idbFactory()
  if (!f) return null
  if (opening) return opening
  const p = new Promise<IDBDatabase>((resolve, reject) => {
    const r = f.open(DB_NAME, DB_VERSION)
    r.onupgradeneeded = (ev) => {
      const db = r.result
      if (!db.objectStoreNames.contains(STORE)) {
        const s = db.createObjectStore(STORE, { keyPath: 'key' })
        s.createIndex('savedAt', 'savedAt')
        s.createIndex('hostKey', 'hostKey')
      } else if (ev.oldVersion < 3 && r.transaction) {
        // Rows from before the meta store: of an older version (never
        // painted) and outside the bounds. The outbox store stays.
        r.transaction.objectStore(STORE).clear()
      }
      if (!db.objectStoreNames.contains(META_STORE)) {
        db.createObjectStore(META_STORE, { keyPath: 'key' })
      }
      if (!db.objectStoreNames.contains(OUTBOX_STORE)) {
        db.createObjectStore(OUTBOX_STORE, { keyPath: 'clientMsgId' })
      }
    }
    r.onsuccess = () => {
      const db = r.result
      db.onversionchange = () => {
        db.close()
        if (opening === p) opening = null
      }
      db.onclose = () => {
        if (opening === p) opening = null
      }
      resolve(db)
    }
    r.onerror = () => reject(r.error ?? new Error('Could not open the Thread view cache'))
    r.onblocked = () => reject(new Error('The Thread view cache is held by another window'))
  })
  opening = p
  p.catch(() => {
    if (opening === p) opening = null
  })
  return p
}

function quiet(what: string, err: unknown): void {
  // A window without a working IndexedDB just has no saved copy.
  console.debug(`[thread-cache] ${what}:`, err)
}

async function idbGet(key: string): Promise<CacheRow | null> {
  const open = openDb()
  if (!open) return null
  try {
    const db = await open
    const row = (await req(db.transaction(STORE, 'readonly').objectStore(STORE).get(key))) as CacheRow | undefined
    return row ?? null
  } catch (e) {
    quiet('read', e)
    return null
  }
}

function metaOf(row: CacheRow): ThreadViewMeta {
  return {
    key: row.key,
    hostKey: row.hostKey,
    addr: row.addr,
    conversationId: row.conversationId,
    savedAt: row.savedAt,
    bytes: JSON.stringify(row.items).length,
    cursor: newestSeq(row.items),
    v: row.v,
  }
}

/** Which saved Threads leave so `metas` fits the bounds (count, bytes):
 *  least recently saved first, never `keep`. Pure. */
export function evictionPlan(
  metas: readonly Pick<ThreadViewMeta, 'key' | 'savedAt' | 'bytes'>[],
  keep: string,
  maxChats = THREAD_VIEW_CACHE_THREADS,
  maxBytes = THREAD_VIEW_CACHE_MAX_BYTES,
): string[] {
  const order = [...metas].sort((a, b) => a.savedAt - b.savedAt)
  let count = order.length
  let bytes = order.reduce((n, m) => n + (m.bytes || 0), 0)
  const out: string[] = []
  for (const m of order) {
    if (count <= maxChats && bytes <= maxBytes) break
    if (m.key === keep) continue
    out.push(m.key)
    count -= 1
    bytes -= m.bytes || 0
  }
  return out
}

async function idbPut(row: CacheRow): Promise<void> {
  const open = openDb()
  if (!open) return
  try {
    const db = await open
    const tx = db.transaction([STORE, META_STORE], 'readwrite')
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error ?? new Error('write failed'))
      tx.onabort = () => reject(tx.error ?? new Error('write aborted'))
    })
    const store = tx.objectStore(STORE)
    const meta = tx.objectStore(META_STORE)
    store.put(row)
    meta.put(metaOf(row))
    // The bounds, from the small rows (every window's, read now).
    const metas = (await req(meta.getAll())) as ThreadViewMeta[]
    for (const key of evictionPlan(metas, row.key)) {
      store.delete(key)
      meta.delete(key)
    }
    await done
  } catch (e) {
    quiet('write', e)
  }
}

async function idbDeleteWhere(match: (row: Pick<ThreadViewMeta, 'key' | 'hostKey' | 'addr'>) => boolean): Promise<void> {
  const open = openDb()
  if (!open) return
  try {
    const db = await open
    const tx = db.transaction([STORE, META_STORE], 'readwrite')
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error ?? new Error('delete failed'))
    })
    const store = tx.objectStore(STORE)
    const meta = tx.objectStore(META_STORE)
    const rows = (await req(meta.getAll())) as ThreadViewMeta[]
    for (const row of rows) {
      if (!match(row)) continue
      store.delete(row.key)
      meta.delete(row.key)
    }
    // The outbox store is never touched here (see the header).
    await done
  } catch (e) {
    quiet('delete', e)
  }
}

async function idbMetas(): Promise<ThreadViewMeta[]> {
  const open = openDb()
  if (!open) return []
  try {
    const db = await open
    const rows = (await req(db.transaction(META_STORE, 'readonly').objectStore(META_STORE).getAll())) as ThreadViewMeta[]
    return rows.filter((m) => m && m.v === THREAD_VIEW_CACHE_VERSION)
  } catch (e) {
    quiet('meta read', e)
    return []
  }
}

function coerceRow(raw: CacheRow | null, hostKey: string, addr: string): CachedThreadView | null {
  if (!raw || raw.v !== THREAD_VIEW_CACHE_VERSION) return null
  if (raw.hostKey !== hostKey || raw.addr !== addr.trim() || !Array.isArray(raw.items)) return null
  return {
    hostKey: raw.hostKey,
    addr: raw.addr,
    conversationId: typeof raw.conversationId === 'string' ? raw.conversationId : '',
    items: raw.items,
    hasMore: raw.hasMore === true,
    savedAt: typeof raw.savedAt === 'number' ? raw.savedAt : 0,
  }
}

// ── Unsent posts (the outbox store) ────────────────────────────────────────

/** One unsent Thread post as saved on this computer (JSON only). */
export interface PendingSendRow {
  clientMsgId: string
  hostKey: string
  addr: string
  text: string
  body: Record<string, unknown>
  keyed: boolean
  createdAtSec: number
  state: string
  error?: string
  attempts: number
  /** The login it was written under (`''` for this computer). */
  login: string
  afterSeq?: number
  v: number
}

async function outboxTx<T>(mode: IDBTransactionMode, fn: (store: IDBObjectStore) => Promise<T> | T): Promise<T> {
  const open = openDb()
  if (!open) throw new Error('no IndexedDB')
  const db = await open
  const tx = db.transaction(OUTBOX_STORE, mode)
  const done = new Promise<void>((resolve, reject) => {
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error ?? new Error('outbox write failed'))
    tx.onabort = () => reject(tx.error ?? new Error('outbox write aborted'))
  })
  const out = await fn(tx.objectStore(OUTBOX_STORE))
  await done
  return out
}

/** Save (or replace) an unsent post. Resolves true once it is on disk,
 *  false when this window has no working IndexedDB (never throws). */
export async function savePendingSend(row: PendingSendRow): Promise<boolean> {
  try {
    await outboxTx('readwrite', (store) => {
      store.put(row)
    })
    return true
  } catch (e) {
    quiet('outbox write', e)
    return false
  }
}

/** Delete an unsent post (the server confirmed it, or the person deleted it). */
export async function deletePendingSend(clientMsgId: string): Promise<void> {
  try {
    await outboxTx('readwrite', (store) => {
      store.delete(clientMsgId)
    })
  } catch (e) {
    quiet('outbox delete', e)
  }
}

/** Every unsent post saved on this computer. */
export async function readPendingSends(): Promise<PendingSendRow[]> {
  try {
    return await outboxTx('readonly', async (store) => (await req(store.getAll())) as PendingSendRow[])
  } catch (e) {
    quiet('outbox read', e)
    return []
  }
}

// ── Sign-out / server removal ──────────────────────────────────────────────

function loginOf(h: ConnectHost): string {
  return h.token ? `${h.username ?? ''}:${h.token}` : ''
}

/** Forget a server's Threads when it leaves this computer or its login is
 *  signed out or replaced. Installed on first use. */
function watchLogins(): void {
  watchHide()
  if (watching) return
  const logins = (hosts: readonly ConnectHost[]): Map<string, string> => {
    const m = new Map<string, string>()
    for (const h of hosts) {
      const key = homeHostKey(h)
      const login = loginOf(h)
      if (login || !m.has(key)) m.set(key, login)
    }
    return m
  }
  let before = logins(useConnectHostStore.getState().hosts)
  watching = useConnectHostStore.subscribe((s) => {
    const now = logins(s.hosts)
    for (const [key, login] of before) {
      if (!login) continue
      if (now.get(key) !== login) forgetThreadViewsForHost(key)
    }
    before = now
  })
}

// ── API ────────────────────────────────────────────────────────────────────

/** This window's copy, at once (no IndexedDB). */
export function peekThreadView(hostKey: string, addr: string): CachedThreadView | null {
  if (!addr.trim()) return null
  return memory.get(keyOf(hostKey, addr)) ?? null
}

/** This window's copy, else the one saved on this computer. */
export async function readThreadView(hostKey: string, addr: string): Promise<CachedThreadView | null> {
  if (!addr.trim()) return null
  watchLogins()
  const key = keyOf(hostKey, addr)
  const hit = memory.get(key)
  if (hit) return hit
  const row = coerceRow(await idbGet(key), hostKey, addr)
  // A save that landed meanwhile is newer than the disk copy.
  const now = memory.get(key)
  if (now) return now
  if (row) touch(key, row)
  return row
}

/** Remember what the hook now holds for this Thread (the server's read and
 *  what followed it). `hasMore`: the server has older messages than these
 *  (a copy cut to the newest 100 has them either way). */
export function saveThreadView(
  hostKey: string,
  addr: string,
  conversationId: string,
  items: readonly OverlayThreadItem[],
  now: number = Date.now(),
  opts: { hasMore?: boolean } = {},
): void {
  if (!addr.trim()) return
  watchLogins()
  const key = keyOf(hostKey, addr)
  const trimmed = trimItems(items)
  const view: CachedThreadView = {
    hostKey,
    addr: addr.trim(),
    conversationId,
    items: trimmed.items,
    hasMore: opts.hasMore === true || trimmed.truncated,
    savedAt: now,
  }
  touch(key, view)
  const prev = pendingWrites.get(key)
  if (prev) clearTimeout(prev)
  pendingWrites.set(
    key,
    setTimeout(() => {
      void flushOne(key)
    }, WRITE_DELAY_MS),
  )
}

/** Write one Thread's pending save now (reads memory at once). */
function flushOne(key: string): Promise<void> {
  const t = pendingWrites.get(key)
  if (t) clearTimeout(t)
  pendingWrites.delete(key)
  const cur = memory.get(key)
  if (!cur) return Promise.resolve()
  return idbPut({ ...cur, key, v: THREAD_VIEW_CACHE_VERSION })
}

/** Write every pending save now (the window is going to the background). */
export function flushThreadViewWrites(): Promise<void> {
  return Promise.all([...pendingWrites.keys()].map((key) => flushOne(key))).then(() => undefined)
}

let flushOnHide = false
function watchHide(): void {
  if (flushOnHide || typeof document === 'undefined') return
  flushOnHide = true
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') void flushThreadViewWrites()
  })
  if (typeof window !== 'undefined') window.addEventListener('pagehide', () => void flushThreadViewWrites())
}

/** Forget one Thread's copy (its access went, it moved, or its cursor is
 *  too far behind to catch up). */
export function forgetThreadView(hostKey: string, addr: string): Promise<void> {
  const key = keyOf(hostKey, addr)
  memory.delete(key)
  const t = pendingWrites.get(key)
  if (t) clearTimeout(t)
  pendingWrites.delete(key)
  return idbDeleteWhere((row) => row.key === key)
}

/** Every Thread saved on this computer or held in this window (no
 *  messages): what the catch-up brings up to date. Newest save first. */
export async function listThreadViews(hostKey?: string): Promise<Array<Pick<ThreadViewMeta, 'hostKey' | 'addr' | 'conversationId' | 'cursor' | 'savedAt'>>> {
  const out = new Map<string, Pick<ThreadViewMeta, 'hostKey' | 'addr' | 'conversationId' | 'cursor' | 'savedAt'>>()
  for (const m of await idbMetas()) {
    out.set(m.key, { hostKey: m.hostKey, addr: m.addr, conversationId: m.conversationId, cursor: m.cursor, savedAt: m.savedAt })
  }
  // This window's copies are as new as any on disk (a write may wait).
  for (const [key, v] of memory) {
    out.set(key, { hostKey: v.hostKey, addr: v.addr, conversationId: v.conversationId, cursor: newestSeq(v.items), savedAt: v.savedAt })
  }
  return [...out.values()].filter((m) => hostKey === undefined || m.hostKey === hostKey).sort((a, b) => b.savedAt - a.savedAt)
}

/** Forget every Thread of one server (removed, signed out). */
export function forgetThreadViewsForHost(hostKey: string): Promise<void> {
  for (const [key, view] of [...memory]) {
    if (view.hostKey !== hostKey) continue
    memory.delete(key)
    const t = pendingWrites.get(key)
    if (t) clearTimeout(t)
    pendingWrites.delete(key)
  }
  return idbDeleteWhere((row) => row.hostKey === hostKey)
}

/** Forget every Thread of every server. */
export function clearThreadViews(): Promise<void> {
  memory.clear()
  for (const t of pendingWrites.values()) clearTimeout(t)
  pendingWrites.clear()
  return idbDeleteWhere(() => true)
}

/** Tests: write pending saves now, one after another. */
export async function flushThreadViewWritesForTests(): Promise<void> {
  for (const key of [...pendingWrites.keys()]) await flushOne(key)
}

/** Tests: forget this window's copies and the open database handle (a
 *  "reload": the IndexedDB rows stay). */
export async function resetThreadViewCacheForTests(opts: { keepDisk?: boolean } = {}): Promise<void> {
  if (!opts.keepDisk) await clearThreadViews()
  memory.clear()
  for (const t of pendingWrites.values()) clearTimeout(t)
  pendingWrites.clear()
  if (watching) {
    watching()
    watching = null
  }
  const p = opening
  opening = null
  if (p) {
    try {
      ;(await p).close()
    } catch {
      /* never opened */
    }
  }
}

/** Tests: delete every saved unsent post. */
export async function clearPendingSendsForTests(): Promise<void> {
  try {
    await outboxTx('readwrite', (store) => {
      store.clear()
    })
  } catch (e) {
    quiet('outbox clear', e)
  }
}
