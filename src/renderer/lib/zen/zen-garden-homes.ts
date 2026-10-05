// prd-zen-gardens-v1 G27 — which Home each Agents widget in a Garden shows.
//
// The Agents widget has a Home picker of its own. Its pick is per Garden
// and widget, in localStorage `k2.zen.gardenHomes.v1` =
// `{version: 1, picks: {"<gardenId>/<widgetId>": "<homeId>"}}`, shared by
// every window through the `storage` event, so two windows on one Garden
// agree. Picking a Home here never calls `selectHome`: the Home page's
// selection, rows and sidebar never move (decision 7).
//
// Storage can throw: reads and writes go through try/catch and fall back to
// memory. A stored value that isn't ours is not overwritten until a pick.

import { create, type StoreApi, type UseBoundStore } from 'zustand'

export const ZEN_GARDEN_HOMES_KEY = 'k2.zen.gardenHomes.v1'

export interface ZenGardenHomesDoc {
  version: 1
  picks: Record<string, string>
}

export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

/** The picks key of one Agents widget in one Garden. */
export function zenGardenHomeKey(gardenId: string, widgetId: string): string {
  return `${gardenId}/${widgetId}`
}

export function parseZenGardenHomesDoc(raw: string | null): ZenGardenHomesDoc | null {
  if (raw === null) return null
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const d = v as Record<string, unknown>
  if (d.version !== 1 || !d.picks || typeof d.picks !== 'object' || Array.isArray(d.picks)) return null
  const picks: Record<string, string> = {}
  for (const [k, val] of Object.entries(d.picks as Record<string, unknown>)) {
    if (typeof val === 'string' && val.length > 0 && k.length > 0) picks[k] = val
  }
  return { version: 1, picks }
}

export interface ZenGardenHomesState {
  picks: Record<string, string>
  setPick(key: string, homeId: string): void
  /** Another window saved `raw` (the `storage` event). */
  applyExternal(raw: string | null): void
}

function safeGet(kv: KeyValueStorage | null): string | null {
  if (!kv) return null
  try {
    return kv.getItem(ZEN_GARDEN_HOMES_KEY)
  } catch (err) {
    console.warn('[zen] storage read failed:', err)
    return null
  }
}

function safeSet(kv: KeyValueStorage | null, value: string): void {
  if (!kv) return
  try {
    kv.setItem(ZEN_GARDEN_HOMES_KEY, value)
  } catch (err) {
    console.warn('[zen] storage write failed:', err)
  }
}

export function createZenGardenHomesStore(local: KeyValueStorage | null): UseBoundStore<StoreApi<ZenGardenHomesState>> {
  const initial = parseZenGardenHomesDoc(safeGet(local))
  return create<ZenGardenHomesState>((set, get) => ({
    picks: initial?.picks ?? {},
    setPick(key, homeId) {
      if (!key || !homeId || get().picks[key] === homeId) return
      const picks = { ...get().picks, [key]: homeId }
      set({ picks })
      safeSet(local, JSON.stringify({ version: 1, picks } satisfies ZenGardenHomesDoc))
    },
    applyExternal(raw) {
      set({ picks: parseZenGardenHomesDoc(raw)?.picks ?? {} })
    },
  }))
}

/** Keep `store` in step with other windows. Returns the unsubscribe. */
export function attachZenGardenHomesStorageSync(
  store: UseBoundStore<StoreApi<ZenGardenHomesState>>,
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>,
  local: KeyValueStorage | null,
): () => void {
  const onStorage = (e: Event): void => {
    const se = e as Event & { key?: string | null; newValue?: string | null }
    if (se.key === ZEN_GARDEN_HOMES_KEY) store.getState().applyExternal(se.newValue ?? null)
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

export const useZenGardenHomesStore = createZenGardenHomesStore(localKv)

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  attachZenGardenHomesStorageSync(useZenGardenHomesStore, window, localKv)
}
