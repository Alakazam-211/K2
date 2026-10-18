// Home P1 (prd-home-v1 R1 + vs-live H11/H13) — the named Homes on this
// computer.
//
// A Home is a named, ordered list of agents drawn from any saved server.
// The whole set lives in localStorage `k2.homes.v1`, beside
// `k2.connect-hosts.v1`: every window of the app on this computer shares
// one origin, so every window sees the same Homes, and other windows pick
// up a change through the `storage` event. (The window-chrome rule against
// `storage` sync is for per-window state; this state is shared.)
//
// This store must NEVER subscribe to `onActiveHostChange`. A server switch
// never changes a Home — that is the whole point (R1). It is view state for
// one person, like the saved-server list, and runs no loop, so the headless
// daemon misses nothing. Not named "roster": presence, users:roster, and
// the federation roster already use that word.
//
// Selection is per window: sessionStorage (one per webview, survives a
// reload, never piles up per window label) holds this window's pick, and
// `k2.homes.lastSelected` in localStorage is what a NEW window opens on.
//
// homes-sync-v1 (0.46.1): this copy is still the working copy — the UI
// only ever reads it, so an offline home server never blocks the Home
// screen. The sync runner (`lib/homes-sync/runner.ts`) watches it, queues
// changes for the person's home server and writes merged results back
// through `applySynced`. Selection and expanded state never sync. A Home
// merged into another on a first sync keeps its id as a redirect
// (`redirects`, from `k2.homes.sync.v1`), so a window's selection, the
// last-selected id and a Garden's `seedHome` still resolve.
//
// Storage can throw (private mode, quota, a locked-down webview). Every
// read and write goes through `safeGet` / `safeSet`; a storage ERROR falls
// back to an in-memory seed. Data that parses but is not ours (a future
// version, hand-edited junk) is not overwritten until the user changes a
// Home.

import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { findWorkspaceForRow, homeAddress, parseHomeAddress, workspaceHandle } from '@/lib/home-address'
import { homeHostKey } from '@/lib/host-key'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { onSavedHostRekey } from '@/lib/connect-host-hooks'

export const HOMES_STORAGE_KEY = 'k2.homes.v1'
export const HOMES_LAST_SELECTED_KEY = 'k2.homes.lastSelected'
/** sessionStorage — this window's selected Home. */
export const HOMES_WINDOW_SELECTED_KEY = 'k2.homes.windowSelected'
/** homes-sync-v1: the sync runner's state (cursor, outbox, redirects). */
export const HOMES_SYNC_STORAGE_KEY = 'k2.homes.sync.v1'

export const DEFAULT_HOME_NAME = 'Home'
export const HOME_NAME_MAX = 60

export interface HomeRow {
  /** `handle::host`, lowercase — the row key. */
  address: string
  /** `projects.id` on that host — rename-repair hint, not identity. */
  workspaceId: string | null
  /** Display name when the row was added / last repaired. */
  label: string
}

export interface Home {
  id: string
  name: string
  rows: HomeRow[]
}

export interface HomesDoc {
  version: 1
  homes: Home[]
}

/** The storage surface we use (localStorage / sessionStorage shape). */
export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

type SafeRead = { ok: true; value: string | null } | { ok: false }

function safeGet(kv: KeyValueStorage | null, key: string): SafeRead {
  if (!kv) return { ok: false }
  try {
    return { ok: true, value: kv.getItem(key) }
  } catch (err) {
    console.warn('[homes] storage read failed:', key, err)
    return { ok: false }
  }
}

function safeSet(kv: KeyValueStorage | null, key: string, value: string): boolean {
  if (!kv) return false
  try {
    kv.setItem(key, value)
    return true
  } catch (err) {
    console.warn('[homes] storage write failed:', key, err)
    return false
  }
}

function isRow(v: unknown): v is HomeRow {
  if (!v || typeof v !== 'object') return false
  const r = v as Record<string, unknown>
  return (
    typeof r.address === 'string' &&
    parseHomeAddress(r.address) !== null &&
    (r.workspaceId === null || typeof r.workspaceId === 'string') &&
    typeof r.label === 'string'
  )
}

function isHome(v: unknown): v is Home {
  if (!v || typeof v !== 'object') return false
  const h = v as Record<string, unknown>
  return (
    typeof h.id === 'string' &&
    h.id.length > 0 &&
    typeof h.name === 'string' &&
    Array.isArray(h.rows) &&
    h.rows.every(isRow)
  )
}

/** Parse a stored `k2.homes.v1` value. Null when missing or not a valid
 *  version-1 doc with at least one Home. */
export function parseHomesDoc(raw: string | null): HomesDoc | null {
  if (raw === null) return null
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object') return null
  const d = v as Record<string, unknown>
  if (d.version !== 1 || !Array.isArray(d.homes) || d.homes.length === 0) return null
  if (!d.homes.every(isHome)) return null
  return { version: 1, homes: d.homes as Home[] }
}

/** Trim + cap a Home name. Null for an empty name (refused). */
export function cleanHomeName(name: string): string | null {
  const t = name.trim().replace(/\s+/g, ' ')
  if (!t) return null
  return t.slice(0, HOME_NAME_MAX)
}

export interface HomesState {
  homes: Home[]
  /**
   * P28 (prd-home-picker-and-remote-avatars-v1): true when `homes` came from
   * storage — `k2.homes.v1` parsed as a v1 doc, the first-run seed was
   * written, or another window's good doc was applied. False on a read
   * error or an unreadable doc (the in-memory seed). Avatar cache cleanup
   * never runs while it is false, so one storage error can't wipe it.
   */
  storageOk: boolean
  /** This window's selected Home id (always one of `homes`). */
  selectedId: string
  /** homes-sync-v1: merged-away Home id → the Home it went into. */
  redirects: Record<string, string>
  selectHome: (id: string) => void
  /** Create an empty Home and select it. Null when the name is empty. */
  createHome: (name: string) => string | null
  /** False when the name is empty or the Home is gone. */
  renameHome: (id: string, name: string) => boolean
  /** False for the last Home (never deleted) or an unknown id. */
  deleteHome: (id: string) => boolean
  /** False when the address is already on that Home (dedupe per Home). */
  addRow: (homeId: string, row: HomeRow) => boolean
  removeRow: (homeId: string, address: string) => void
  moveRow: (homeId: string, from: number, to: number) => void
  /**
   * Rename repair for one host's rows, on every Home. `resolve` maps an
   * old row to its fresh `{address, workspaceId, label}` or null (leave
   * it). A repaired address that already sits on the same Home drops the
   * old row instead of duplicating it. Writes only on a real change.
   */
  repairRows: (resolve: (row: HomeRow) => HomeRow | null) => void
  /** Another window wrote `k2.homes.v1` (the `storage` event). */
  applyExternal: (raw: string | null) => void
  /** homes-sync-v1: the sync runner merged server changes. Saves them
   *  (other windows follow through `storage`); keeps this window's
   *  selection, following a redirect when its Home was merged away. */
  applySynced: (homes: Home[], redirects: Record<string, string>) => void
  /** homes-sync-v1: redirects changed in another window. */
  setRedirects: (redirects: Record<string, string>) => void
}

export interface HomesEnv {
  /** Shared by every window (localStorage). */
  local: KeyValueStorage | null
  /** This window only (sessionStorage). */
  session: KeyValueStorage | null
  newId: () => string
  /** Saved servers, for the one-shot MS60 host-key rewrite. */
  savedHosts?: () => ReadonlyArray<Pick<ConnectHost, 'hostname' | 'port' | 'secure'>>
}

/**
 * Home M1 / MS60: plain `http` now always carries its port (`box.lan:80`),
 * where the old key dropped a default `:80`. A row whose host is a bare
 * hostname is rewritten to `<host>:80` when the ONLY saved server with that
 * hostname is plain http on port 80 (so the bare key can no longer match
 * it). Every other row is left exactly as it was. Returns null when nothing
 * changed.
 */
export function migrateHomeRowHostKeys(
  homes: Home[],
  hosts: ReadonlyArray<Pick<ConnectHost, 'hostname' | 'port' | 'secure'>>,
): Home[] | null {
  let changed = false
  const next = homes.map((h) => ({
    ...h,
    rows: h.rows.map((r) => {
      const parsed = parseHomeAddress(r.address)
      if (!parsed || parsed.host === 'local' || parsed.host.includes(':')) return r
      const same = hosts.filter((x) => x.hostname.trim().toLowerCase() === parsed.host)
      if (same.length === 0) return r
      if (same.some((x) => homeHostKey(x) === parsed.host)) return r
      if (!same.every((x) => !x.secure && x.port === 80)) return r
      changed = true
      return { ...r, address: `${parsed.handle}::${parsed.host}:80` }
    }),
  }))
  return changed ? next : null
}

function seedHomes(newId: () => string): Home[] {
  return [{ id: newId(), name: DEFAULT_HOME_NAME, rows: [] }]
}

/** Follow merge redirects (A → B → C), stopping on a loop. */
export function followHomeRedirects(redirects: Readonly<Record<string, string>>, id: string): string {
  let at = id
  const seen = new Set<string>([at])
  while (Object.prototype.hasOwnProperty.call(redirects, at)) {
    at = redirects[at]
    if (seen.has(at)) break
    seen.add(at)
  }
  return at
}

/** A Home id as it is now: itself when it exists, else where its merge
 *  redirect leads, else null. */
export function resolveHomeId(
  s: Pick<HomesState, 'homes' | 'redirects'>,
  id: string | null | undefined,
): string | null {
  if (!id) return null
  if (s.homes.some((h) => h.id === id)) return id
  const to = followHomeRedirects(s.redirects, id)
  return s.homes.some((h) => h.id === to) ? to : null
}

/** Redirects stored in the sync state (`k2.homes.sync.v1`). */
export function parseHomeRedirects(raw: string | null): Record<string, string> {
  if (!raw) return {}
  try {
    const v = JSON.parse(raw) as { redirects?: unknown }
    const out: Record<string, string> = {}
    if (v && v.redirects && typeof v.redirects === 'object') {
      for (const [k, val] of Object.entries(v.redirects as Record<string, unknown>)) if (typeof val === 'string') out[k] = val
    }
    return out
  } catch {
    return {}
  }
}

function pickSelected(homes: Home[], env: HomesEnv, redirects: Record<string, string> = {}): string {
  const resolve = (id: string | null): string | null => resolveHomeId({ homes, redirects }, id)
  const win = safeGet(env.session, HOMES_WINDOW_SELECTED_KEY)
  const w = win.ok ? resolve(win.value) : null
  if (w) return w
  const last = safeGet(env.local, HOMES_LAST_SELECTED_KEY)
  const l = last.ok ? resolve(last.value) : null
  if (l) return l
  return homes[0].id
}

export function createHomesStore(env: HomesEnv): UseBoundStore<StoreApi<HomesState>> {
  // Boot: a storage error → in-memory seed (never written back until a
  // change); first run (key absent) → seed one empty "Home" and save it;
  // unreadable data → in-memory seed, left on disk untouched.
  const read = safeGet(env.local, HOMES_STORAGE_KEY)
  let initial: Home[]
  let storageOk = false
  if (!read.ok) {
    initial = seedHomes(env.newId)
  } else if (read.value === null) {
    initial = seedHomes(env.newId)
    storageOk = safeSet(env.local, HOMES_STORAGE_KEY, JSON.stringify({ version: 1, homes: initial }))
  } else {
    const doc = parseHomesDoc(read.value)
    if (doc) {
      storageOk = true
      const migrated = env.savedHosts ? migrateHomeRowHostKeys(doc.homes, env.savedHosts()) : null
      initial = migrated ?? doc.homes
      if (migrated) {
        safeSet(env.local, HOMES_STORAGE_KEY, JSON.stringify({ version: 1, homes: migrated } satisfies HomesDoc))
      }
    } else {
      console.warn('[homes] k2.homes.v1 is not a version-1 doc; using an unsaved default')
      initial = seedHomes(env.newId)
    }
  }

  const syncRead = safeGet(env.local, HOMES_SYNC_STORAGE_KEY)
  const initialRedirects = syncRead.ok ? parseHomeRedirects(syncRead.value) : {}

  const persist = (homes: Home[]): void => {
    safeSet(env.local, HOMES_STORAGE_KEY, JSON.stringify({ version: 1, homes } satisfies HomesDoc))
  }
  const rememberSelection = (id: string): void => {
    safeSet(env.session, HOMES_WINDOW_SELECTED_KEY, id)
    safeSet(env.local, HOMES_LAST_SELECTED_KEY, id)
  }

  return create<HomesState>((set, get) => {
    const commit = (homes: Home[], selectedId?: string): void => {
      const sel = selectedId ?? get().selectedId
      set({ homes, selectedId: homes.some((h) => h.id === sel) ? sel : homes[0].id })
      persist(homes)
    }
    const mapHome = (id: string, fn: (h: Home) => Home): Home[] | null => {
      if (!get().homes.some((h) => h.id === id)) return null
      return get().homes.map((h) => (h.id === id ? fn(h) : h))
    }

    return {
      homes: initial,
      storageOk,
      selectedId: pickSelected(initial, env, initialRedirects),
      redirects: initialRedirects,

      selectHome: (id) => {
        if (!get().homes.some((h) => h.id === id)) return
        set({ selectedId: id })
        rememberSelection(id)
      },

      createHome: (name) => {
        const clean = cleanHomeName(name)
        if (!clean) return null
        const id = env.newId()
        commit([...get().homes, { id, name: clean, rows: [] }], id)
        rememberSelection(id)
        return id
      },

      renameHome: (id, name) => {
        const clean = cleanHomeName(name)
        if (!clean) return false
        const next = mapHome(id, (h) => ({ ...h, name: clean }))
        if (!next) return false
        commit(next)
        return true
      },

      deleteHome: (id) => {
        const { homes, selectedId } = get()
        if (homes.length <= 1) return false
        const idx = homes.findIndex((h) => h.id === id)
        if (idx < 0) return false
        const next = homes.filter((h) => h.id !== id)
        const sel = selectedId === id ? next[Math.max(0, idx - 1)].id : selectedId
        commit(next, sel)
        if (sel !== selectedId) rememberSelection(sel)
        return true
      },

      addRow: (homeId, row) => {
        const address = row.address.trim().toLowerCase()
        if (!parseHomeAddress(address)) return false
        const home = get().homes.find((h) => h.id === homeId)
        if (!home || home.rows.some((r) => r.address === address)) return false
        const next = mapHome(homeId, (h) => ({ ...h, rows: [...h.rows, { ...row, address }] }))
        if (!next) return false
        commit(next)
        return true
      },

      removeRow: (homeId, address) => {
        const home = get().homes.find((h) => h.id === homeId)
        if (!home || !home.rows.some((r) => r.address === address)) return
        const next = mapHome(homeId, (h) => ({ ...h, rows: h.rows.filter((r) => r.address !== address) }))
        if (next) commit(next)
      },

      moveRow: (homeId, from, to) => {
        const home = get().homes.find((h) => h.id === homeId)
        if (!home) return
        if (from === to || from < 0 || to < 0 || from >= home.rows.length || to >= home.rows.length) return
        const rows = [...home.rows]
        const [moved] = rows.splice(from, 1)
        rows.splice(to, 0, moved)
        const next = mapHome(homeId, (h) => ({ ...h, rows }))
        if (next) commit(next)
      },

      repairRows: (resolve) => {
        let changed = false
        const next = get().homes.map((h) => {
          let rows: HomeRow[] = []
          let homeChanged = false
          for (const r of h.rows) {
            const fresh = resolve(r)
            if (
              !fresh ||
              (fresh.address === r.address && fresh.workspaceId === r.workspaceId && fresh.label === r.label)
            ) {
              rows.push(r)
              continue
            }
            homeChanged = true
            const address = fresh.address.trim().toLowerCase()
            if (address !== r.address && h.rows.some((o) => o.address === address)) {
              // The new address is already on this Home — drop the stale row.
              continue
            }
            rows.push({ ...fresh, address })
          }
          if (!homeChanged) return h
          changed = true
          // Two stale rows can repair to the same address; keep the first.
          const seen = new Set<string>()
          rows = rows.filter((r) => (seen.has(r.address) ? false : (seen.add(r.address), true)))
          return { ...h, rows }
        })
        if (changed) commit(next)
      },

      applyExternal: (raw) => {
        const doc = parseHomesDoc(raw)
        if (!doc) return
        const { selectedId, redirects } = get()
        const sel =
          resolveHomeId({ homes: doc.homes, redirects }, selectedId) ?? pickSelected(doc.homes, env, redirects)
        // No write-back: the other window already saved this exact doc.
        set({ homes: doc.homes, selectedId: sel, storageOk: true })
      },

      applySynced: (homes, redirects) => {
        if (homes.length === 0) return
        const { selectedId } = get()
        const sel = resolveHomeId({ homes, redirects }, selectedId) ?? pickSelected(homes, env, redirects)
        set({ homes, redirects, selectedId: sel, storageOk: true })
        persist(homes)
        if (sel !== selectedId) safeSet(env.session, HOMES_WINDOW_SELECTED_KEY, sel)
      },

      setRedirects: (redirects) => {
        const { homes, selectedId } = get()
        const sel = resolveHomeId({ homes, redirects }, selectedId) ?? selectedId
        set({ redirects, selectedId: sel })
      },
    }
  })
}

/** MS61: rows on `oldKey` move to `newKey` (a saved server's address was
 *  edited). A row whose new address is already on that Home is dropped.
 *  Idempotent: the window that saved the edit writes the Homes, and the
 *  other windows find nothing left to move. */
export function rekeyHomeRows(
  store: UseBoundStore<StoreApi<HomesState>>,
  oldKey: string,
  newKey: string,
): void {
  if (oldKey === newKey) return
  store.getState().repairRows((row) => {
    const p = parseHomeAddress(row.address)
    if (!p || p.host !== oldKey) return null
    return { ...row, address: homeAddress(p.handle, newKey) }
  })
}

/** Rename repair from one server's fresh workspace list: rows on `hostKey`
 *  whose handle moved get the new address; labels follow the name. Rows
 *  whose workspace isn't in the list are left alone. */
export function repairHomeRowsFromList(
  store: UseBoundStore<StoreApi<HomesState>>,
  hostKey: string,
  workspaces: ReadonlyArray<{ id: string; handle?: string | null; name: string }>,
): void {
  const list = [...workspaces]
  store.getState().repairRows((row) => {
    const p = parseHomeAddress(row.address)
    if (!p || p.host !== hostKey) return null
    const ws = findWorkspaceForRow(list, row)
    if (!ws) return null
    const handle = workspaceHandle(ws)
    if (!handle) return null
    return { address: homeAddress(handle, hostKey), workspaceId: ws.id, label: ws.name }
  })
}

/** Keep a store in step with other windows. Returns the unsubscribe. */
export function attachHomesStorageSync(
  store: UseBoundStore<StoreApi<HomesState>>,
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>,
  local: KeyValueStorage | null,
): () => void {
  const onStorage = (e: Event): void => {
    const se = e as Event & { key?: string | null; newValue?: string | null }
    if (se.key === HOMES_STORAGE_KEY) {
      store.getState().applyExternal(se.newValue ?? null)
    } else if (se.key === HOMES_SYNC_STORAGE_KEY) {
      store.getState().setRedirects(parseHomeRedirects(se.newValue ?? null))
    } else if (se.key === null) {
      // storage.clear() in another window — re-read what is there now.
      const r = safeGet(local, HOMES_STORAGE_KEY)
      if (r.ok) store.getState().applyExternal(r.value)
    }
  }
  target.addEventListener('storage', onStorage)
  return () => target.removeEventListener('storage', onStorage)
}

function storageOrNull(pick: () => Storage | undefined): KeyValueStorage | null {
  try {
    return pick() ?? null
  } catch {
    return null
  }
}

function newHomeId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `home-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

const localKv = storageOrNull(() => globalThis.localStorage)
const sessionKv = storageOrNull(() => globalThis.sessionStorage)

export const useHomesStore = createHomesStore({
  local: localKv,
  session: sessionKv,
  newId: newHomeId,
  savedHosts: () => useConnectHostStore.getState().hosts,
})

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  attachHomesStorageSync(useHomesStore, window, localKv)
}

onSavedHostRekey((oldKey, newKey) => rekeyHomeRows(useHomesStore, oldKey, newKey))

/** The selected Home object (always defined). */
export function selectedHome(s: Pick<HomesState, 'homes' | 'selectedId'>): Home {
  return s.homes.find((h) => h.id === s.selectedId) ?? s.homes[0]
}
