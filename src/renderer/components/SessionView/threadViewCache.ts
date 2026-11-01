// The last messages of a remote Thread, shown at once on open
// (research-remote-thread-load-v1 Fix 4).
//
// A VIEW cache: what this window last drew for one Thread on one server,
// kept so reopening it paints before the tunnel answers. The daemon stays
// the truth: the hook replaces this with its own read as soon as it lands,
// and nothing here is ever posted, counted as unread or marked read.
//
//   - Key: (server host key, Thread address). Remote servers only (a local
//     read takes ~60 ms; nothing to hide).
//   - The newest 25 Thread messages per Thread, at most 50 Threads, least
//     recently used dropped first.
//   - In memory for this window, and in IndexedDB on this computer (never
//     localStorage: message bodies). Every IndexedDB call is wrapped: a
//     window without it, or a failing one, just has no saved copy.
//   - Versioned (`v`): a row of another version is ignored.
//   - Forgotten for a server when it is removed from this computer or its
//     login is signed out or replaced.

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { homeHostKey } from '@/lib/host-key'
import { isThreadSurfaceItem, type OverlayThreadItem } from './overlayThread'

export const THREAD_VIEW_CACHE_VERSION = 1
/** Messages kept per Thread. */
export const THREAD_VIEW_CACHE_ITEMS = 25
/** Threads kept. */
export const THREAD_VIEW_CACHE_THREADS = 50
const DB_NAME = 'k2.thread-view-cache'
const DB_VERSION = 1
const STORE = 'threads'
/** Writes to IndexedDB are coalesced per Thread. */
const WRITE_DELAY_MS = 750

export interface CachedThreadView {
  hostKey: string
  addr: string
  conversationId: string
  items: OverlayThreadItem[]
  savedAt: number
}

interface CacheRow extends CachedThreadView {
  key: string
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
  while (memory.size > THREAD_VIEW_CACHE_THREADS) {
    const oldest = memory.keys().next().value
    if (oldest === undefined) break
    memory.delete(oldest)
  }
}

/** The newest Thread messages, oldest first, without this window's
 *  unconfirmed send times. */
function trimItems(items: readonly OverlayThreadItem[]): OverlayThreadItem[] {
  const thread = items.filter((it) => isThreadSurfaceItem(it))
  const sorted = [...thread].sort((a, b) => a.seq - b.seq)
  return sorted.slice(-THREAD_VIEW_CACHE_ITEMS).map((it) => {
    if (!it.localTime) return it
    const { localTime: _local, ...rest } = it
    return rest
  })
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
    r.onupgradeneeded = () => {
      const db = r.result
      if (!db.objectStoreNames.contains(STORE)) {
        const s = db.createObjectStore(STORE, { keyPath: 'key' })
        s.createIndex('savedAt', 'savedAt')
        s.createIndex('hostKey', 'hostKey')
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

async function idbPut(row: CacheRow): Promise<void> {
  const open = openDb()
  if (!open) return
  try {
    const db = await open
    const tx = db.transaction(STORE, 'readwrite')
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error ?? new Error('write failed'))
      tx.onabort = () => reject(tx.error ?? new Error('write aborted'))
    })
    const store = tx.objectStore(STORE)
    store.put(row)
    // LRU bound: the oldest rows past the cap go.
    const keys = (await req(store.index('savedAt').getAllKeys())) as IDBValidKey[]
    const extra = keys.length - THREAD_VIEW_CACHE_THREADS
    for (let i = 0; i < extra; i++) store.delete(keys[i])
    await done
  } catch (e) {
    quiet('write', e)
  }
}

async function idbDeleteWhere(match: (row: CacheRow) => boolean): Promise<void> {
  const open = openDb()
  if (!open) return
  try {
    const db = await open
    const tx = db.transaction(STORE, 'readwrite')
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error ?? new Error('delete failed'))
    })
    const store = tx.objectStore(STORE)
    const rows = (await req(store.getAll())) as CacheRow[]
    for (const row of rows) if (match(row)) store.delete(row.key)
    await done
  } catch (e) {
    quiet('delete', e)
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
    savedAt: typeof raw.savedAt === 'number' ? raw.savedAt : 0,
  }
}

// ── Sign-out / server removal ──────────────────────────────────────────────

function loginOf(h: ConnectHost): string {
  return h.token ? `${h.username ?? ''}:${h.token}` : ''
}

/** Forget a server's Threads when it leaves this computer or its login is
 *  signed out or replaced. Installed on first use. */
function watchLogins(): void {
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
 *  what followed it). */
export function saveThreadView(
  hostKey: string,
  addr: string,
  conversationId: string,
  items: readonly OverlayThreadItem[],
  now: number = Date.now(),
): void {
  if (!addr.trim()) return
  watchLogins()
  const key = keyOf(hostKey, addr)
  const view: CachedThreadView = { hostKey, addr: addr.trim(), conversationId, items: trimItems(items), savedAt: now }
  touch(key, view)
  const prev = pendingWrites.get(key)
  if (prev) clearTimeout(prev)
  pendingWrites.set(
    key,
    setTimeout(() => {
      pendingWrites.delete(key)
      const cur = memory.get(key)
      if (!cur) return
      void idbPut({ ...cur, key, v: THREAD_VIEW_CACHE_VERSION })
    }, WRITE_DELAY_MS),
  )
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

/** Tests: write pending saves now. */
export async function flushThreadViewWritesForTests(): Promise<void> {
  const keys = [...pendingWrites.keys()]
  for (const key of keys) {
    const t = pendingWrites.get(key)
    if (t) clearTimeout(t)
    pendingWrites.delete(key)
    const cur = memory.get(key)
    if (cur) await idbPut({ ...cur, key, v: THREAD_VIEW_CACHE_VERSION })
  }
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
