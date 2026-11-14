// The last projects list this computer drew for one server login, so the
// next start paints the sidebar at once (speed pass P4).
//
// A VIEW copy: the daemon stays the truth. `fetchProjects` paints this copy
// only while nothing else is on screen, then replaces it with its own read
// as soon as that lands; nothing here is ever written to a server.
//
//   - Key: (server host key, login). A different login on the same server
//     never sees another's list.
//   - In memory for this window, and in IndexedDB on this computer. Every
//     IndexedDB call is wrapped: a window without it, or a failing one, just
//     has no saved copy.
//   - Versioned (`v`): a row of another version is ignored.
//   - With the list's tag (`projectsListTag`): the same tag the daemon puts
//     in the list's `ETag`, so the next read can ask "changed?" and get a
//     304 with no body when it has not.
//   - Forgotten for a server when its login is signed out or replaced.

import { useConnectHostStore, activeHostKey, type ConnectHost } from '@/stores/connect-host'

/** Bump when the stored shape changes; older rows are then ignored. */
export const PROJECTS_LIST_CACHE_VERSION = 1
/** Lists kept (one per server login), least recently saved dropped first. */
export const PROJECTS_LIST_CACHE_LISTS = 20
const DB_NAME = 'k2.projects-list-cache'
const DB_VERSION = 1
const STORE = 'lists'

export interface StoredProjectsList<T = unknown> {
  key: string
  projects: T[]
  /** `projectsListTag(projects)`, or null when this window could not work it out. */
  tag: string | null
  savedAt: number
}

interface Row extends StoredProjectsList {
  hostKey: string
  v: number
}

const memory = new Map<string, Row>()
let watching: (() => void) | null = null
let opening: Promise<IDBDatabase> | null = null

/** The key of one server login's list. `login` is the username ('' for this computer). */
export function projectsListCacheKey(hostKey: string, login: string): string {
  return `${hostKey}|${login}`
}

/** The tag the daemon sends as this list's `ETag`: `W/"<first 32 hex digits
 *  of the SHA-256 of its JSON>"`. The daemon's JSON and `JSON.stringify` of
 *  the parsed list are the same bytes (`crates/k2-daemon/src/response_wire.rs`
 *  pins the fixture both sides test). Null without Web Crypto (an insecure
 *  page): the read then simply asks for the whole list. */
export async function projectsListTag(projects: unknown): Promise<string | null> {
  try {
    const subtle = (globalThis as { crypto?: Crypto }).crypto?.subtle
    if (!subtle) return null
    const bytes = new TextEncoder().encode(JSON.stringify(projects))
    const digest = new Uint8Array(await subtle.digest('SHA-256', bytes))
    let hex = ''
    for (let i = 0; i < 16; i++) hex += digest[i].toString(16).padStart(2, '0')
    return `W/"${hex}"`
  } catch {
    return null
  }
}

// ── IndexedDB (every call wrapped) ─────────────────────────────────────────

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
      }
    }
    r.onsuccess = () => {
      const db = r.result
      db.onversionchange = () => {
        db.close()
        if (opening === p) opening = null
      }
      resolve(db)
    }
    r.onerror = () => reject(r.error ?? new Error('Could not open the projects list cache'))
    r.onblocked = () => reject(new Error('The projects list cache is held by another window'))
  })
  opening = p
  p.catch(() => {
    if (opening === p) opening = null
  })
  return p
}

function quiet(what: string, err: unknown): void {
  console.debug(`[projects-cache] ${what}:`, err)
}

async function idbGet(key: string): Promise<Row | null> {
  const open = openDb()
  if (!open) return null
  try {
    const db = await open
    const row = (await req(db.transaction(STORE, 'readonly').objectStore(STORE).get(key))) as Row | undefined
    return row ?? null
  } catch (e) {
    quiet('read', e)
    return null
  }
}

async function idbPut(row: Row): Promise<void> {
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
    const keys = (await req(store.index('savedAt').getAllKeys())) as IDBValidKey[]
    const extra = keys.length - PROJECTS_LIST_CACHE_LISTS
    for (let i = 0; i < extra; i++) store.delete(keys[i])
    await done
  } catch (e) {
    quiet('write', e)
  }
}

async function idbDeleteWhere(match: (row: Row) => boolean): Promise<void> {
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
    const rows = (await req(store.getAll())) as Row[]
    for (const row of rows) if (match(row)) store.delete(row.key)
    await done
  } catch (e) {
    quiet('delete', e)
  }
}

function coerce(raw: Row | null, key: string): Row | null {
  if (!raw || raw.v !== PROJECTS_LIST_CACHE_VERSION || raw.key !== key || !Array.isArray(raw.projects)) return null
  return {
    key: raw.key,
    hostKey: typeof raw.hostKey === 'string' ? raw.hostKey : '',
    projects: raw.projects,
    tag: typeof raw.tag === 'string' ? raw.tag : null,
    savedAt: typeof raw.savedAt === 'number' ? raw.savedAt : 0,
    v: raw.v,
  }
}

function view<T>(row: Row): StoredProjectsList<T> {
  return { key: row.key, projects: row.projects as T[], tag: row.tag, savedAt: row.savedAt }
}

// ── Sign-out / login change ────────────────────────────────────────────────

function loginOf(h: ConnectHost): string {
  return h.token ? `${h.username ?? ''}:${h.token}` : ''
}

/** Forget a server's lists when its login is signed out, replaced or the
 *  server is removed from this computer. Installed on first use. */
function watchLogins(): void {
  if (watching) return
  const logins = (hosts: readonly ConnectHost[] | undefined): Map<string, string> => {
    const m = new Map<string, string>()
    for (const h of hosts ?? []) m.set(activeHostKey(h), loginOf(h))
    return m
  }
  try {
    let before = logins(useConnectHostStore.getState().hosts)
    watching = useConnectHostStore.subscribe((s) => {
      const now = logins(s.hosts)
      for (const [hostKey, login] of before) {
        if (!login) continue
        const cur = now.get(hostKey)
        // A token refresh for the same user keeps the list; anything else
        // (signed out, another user, removed) forgets it.
        const sameUser = cur && cur.split(':')[0] === login.split(':')[0]
        if (!sameUser) {
          const p = forgetProjectsListsForHost(hostKey).finally(() => pendingWrites.delete(p))
          pendingWrites.add(p)
        }
      }
      before = now
    })
  } catch (e) {
    // No server list in this window (a test double): nothing to watch.
    quiet('watch', e)
  }
}

// ── API ────────────────────────────────────────────────────────────────────

/** This window's copy, at once (no IndexedDB). */
export function peekProjectsList<T = unknown>(key: string): StoredProjectsList<T> | null {
  const row = memory.get(key)
  return row ? view<T>(row) : null
}

/** This window's copy, else the one saved on this computer. */
export async function readProjectsList<T = unknown>(key: string): Promise<StoredProjectsList<T> | null> {
  watchLogins()
  const hit = memory.get(key)
  if (hit) return view<T>(hit)
  const row = coerce(await idbGet(key), key)
  // A save that landed meanwhile is newer than the disk copy.
  const now = memory.get(key)
  if (now) return view<T>(now)
  if (!row) return null
  memory.set(key, row)
  return view<T>(row)
}

/** Remember the list the daemon just gave (and its tag). */
export function saveProjectsList(key: string, hostKey: string, projects: readonly unknown[], tag: string | null, now: number = Date.now()): void {
  watchLogins()
  const row: Row = { key, hostKey, projects: [...projects], tag, savedAt: now, v: PROJECTS_LIST_CACHE_VERSION }
  memory.set(key, row)
  const p = idbPut(row).finally(() => pendingWrites.delete(p))
  pendingWrites.add(p)
}

const pendingWrites = new Set<Promise<void>>()

/** Tests: wait for the saves and forgets already started to reach IndexedDB. */
export async function flushProjectsListWritesForTests(): Promise<void> {
  await Promise.all([...pendingWrites])
}

/** Forget every list of one server (signed out, removed). */
export function forgetProjectsListsForHost(hostKey: string): Promise<void> {
  for (const [key, row] of [...memory]) if (row.hostKey === hostKey) memory.delete(key)
  return idbDeleteWhere((row) => row.hostKey === hostKey)
}

/** Tests: forget this window's copies and the open database handle (a
 *  "reload": the IndexedDB rows stay unless `clearDisk`). */
export async function resetProjectsListCacheForTests(opts: { clearDisk?: boolean } = {}): Promise<void> {
  if (opts.clearDisk) await idbDeleteWhere(() => true)
  memory.clear()
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
