// prd-zen-mode-v1 Z5 (Rosson 2026-10-04 answer 2) — Zen on/off, per Home,
// shared by every window on this computer.
//
// A sibling of `k2.homes.v1` (stores/homes.ts), never a field on `Home`, so
// the Homes schema is untouched: localStorage `k2.zen.homes.v1` =
// `{version: 1, on: {<homeId>: true}}`. Every window of the app shares one
// origin, so other windows pick a change up through the `storage` event
// (the `attachHomesStorageSync` pattern). A Home that is off has no key.
//
// This is view state for one person on this computer, like the Homes
// themselves: no loop, nothing the headless daemon could miss. It must never
// follow a server switch.
//
// Storage can throw (private mode, quota, a locked-down webview): reads and
// writes go through try/catch and fall back to memory. A stored value that
// isn't ours (another version, hand-edited junk) is not overwritten until the
// user turns a Home on or off.

import { create, type StoreApi, type UseBoundStore } from 'zustand'

export const ZEN_HOMES_STORAGE_KEY = 'k2.zen.homes.v1'

export interface ZenHomesDoc {
  version: 1
  on: Record<string, true>
}

export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

/** Parse a stored `k2.zen.homes.v1` value. Null when missing or not a
 *  version-1 doc. Keys whose value isn't `true` are dropped. */
export function parseZenHomesDoc(raw: string | null): ZenHomesDoc | null {
  if (raw === null) return null
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object') return null
  const d = v as Record<string, unknown>
  if (d.version !== 1 || !d.on || typeof d.on !== 'object' || Array.isArray(d.on)) return null
  const on: Record<string, true> = {}
  for (const [id, val] of Object.entries(d.on as Record<string, unknown>)) {
    if (val === true && id.length > 0) on[id] = true
  }
  return { version: 1, on }
}

export interface ZenHomesState {
  /** Homes whose Zen is on. */
  on: Record<string, true>
  /** Turn one Home's Zen on or off (and save). */
  setOn(homeId: string, on: boolean): void
  /** Another window saved `raw` (the `storage` event). */
  applyExternal(raw: string | null): void
}

function safeGet(kv: KeyValueStorage | null): string | null {
  if (!kv) return null
  try {
    return kv.getItem(ZEN_HOMES_STORAGE_KEY)
  } catch (err) {
    console.warn('[zen] storage read failed:', err)
    return null
  }
}

function safeSet(kv: KeyValueStorage | null, value: string): void {
  if (!kv) return
  try {
    kv.setItem(ZEN_HOMES_STORAGE_KEY, value)
  } catch (err) {
    console.warn('[zen] storage write failed:', err)
  }
}

export function createZenHomesStore(local: KeyValueStorage | null): UseBoundStore<StoreApi<ZenHomesState>> {
  const initial = parseZenHomesDoc(safeGet(local))
  return create<ZenHomesState>((set, get) => ({
    on: initial?.on ?? {},
    setOn(homeId, on) {
      if (!homeId) return
      const cur = get().on
      if (Boolean(cur[homeId]) === on) return
      const next = { ...cur }
      if (on) next[homeId] = true
      else delete next[homeId]
      set({ on: next })
      safeSet(local, JSON.stringify({ version: 1, on: next } satisfies ZenHomesDoc))
    },
    applyExternal(raw) {
      const doc = parseZenHomesDoc(raw)
      // A clear (null) in another window turns every Home off here too.
      set({ on: doc?.on ?? {} })
    },
  }))
}

/** Keep `store` in step with other windows. Returns the unsubscribe. */
export function attachZenHomesStorageSync(
  store: UseBoundStore<StoreApi<ZenHomesState>>,
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>,
  local: KeyValueStorage | null,
): () => void {
  const onStorage = (e: Event): void => {
    const se = e as Event & { key?: string | null; newValue?: string | null }
    if (se.key === ZEN_HOMES_STORAGE_KEY) store.getState().applyExternal(se.newValue ?? null)
    else if (se.key === null) store.getState().applyExternal(safeGet(local))
  }
  target.addEventListener('storage', onStorage)
  return () => target.removeEventListener('storage', onStorage)
}

function localOrNull(): KeyValueStorage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

const localKv = localOrNull()

export const useZenHomesStore = createZenHomesStore(localKv)

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  attachZenHomesStorageSync(useZenHomesStore, window, localKv)
}

/** Is Zen on for `homeId`? */
export function isZenOnFor(homeId: string): boolean {
  return useZenHomesStore.getState().on[homeId] === true
}

/** Has this computer ever turned Zen on (any Home on now)? Gates
 *  `POST /cli/zen/homes/sync` so people who never use Zen never make a
 *  `~/.k2/zen/` folder. */
export function anyZenHomeOn(): boolean {
  return Object.keys(useZenHomesStore.getState().on).length > 0
}
