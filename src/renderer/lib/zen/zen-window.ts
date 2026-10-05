// prd-zen-gardens-v1 G1, G21 — Zen is a mode of the WINDOW: one remembered
// on/off per window, plus the Garden that window shows.
//
// localStorage `k2.zen.window.v1.<window label>` =
// `{version: 1, on: boolean, garden: string | null}` (the
// `k2.windowChrome.<label>` pattern, `lib/window-chrome.ts`). Windows are
// independent: turning Zen on in one never changes another, and there is no
// `storage` sync between windows, by design. A new window (a new label)
// has no doc, so it starts outside Zen. The Gardens themselves are shared by
// every window: they live in this computer's daemon (`zen-gardens.ts`).
//
// Per-Home Zen never shipped (Rosson 2026-10-04): there is no import from
// its old per-Home store.
//
// Storage can throw (private mode, quota, a locked-down webview): reads and
// writes go through try/catch and fall back to memory. A stored value that
// isn't ours (another version, hand-edited junk) is not overwritten until
// the window's Zen switch or Garden changes.
//
// View state for one person on this computer: no loop, nothing the headless
// daemon could miss. It never follows a server switch.

import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { getWindowLabel } from '@/lib/window-chrome'

export const ZEN_WINDOW_KEY_PREFIX = 'k2.zen.window.v1.'

export function zenWindowKey(label: string): string {
  return `${ZEN_WINDOW_KEY_PREFIX}${label}`
}

export interface ZenWindowDoc {
  version: 1
  on: boolean
  garden: string | null
}

export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

/** Parse a stored `k2.zen.window.v1.<label>` value. Null when missing or not
 *  a version-1 doc. */
export function parseZenWindowDoc(raw: string | null): ZenWindowDoc | null {
  if (raw === null) return null
  let v: unknown
  try {
    v = JSON.parse(raw)
  } catch {
    return null
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const d = v as Record<string, unknown>
  if (d.version !== 1 || typeof d.on !== 'boolean') return null
  const garden = typeof d.garden === 'string' && d.garden.length > 0 ? d.garden : null
  return { version: 1, on: d.on, garden }
}

export interface ZenWindowState {
  /** This window's label (`main`, `window-<uuid>`, …). */
  label: string
  /** Is Zen switched on in this window (shown unless Settings covers it)? */
  on: boolean
  /** The Garden this window shows, as last picked (null: the first one). */
  garden: string | null
  setOn(on: boolean): void
  setGarden(id: string | null): void
}

function safeGet(kv: KeyValueStorage | null, key: string): string | null {
  if (!kv) return null
  try {
    return kv.getItem(key)
  } catch (err) {
    console.warn('[zen] storage read failed:', err)
    return null
  }
}

function safeSet(kv: KeyValueStorage | null, key: string, value: string): void {
  if (!kv) return
  try {
    kv.setItem(key, value)
  } catch (err) {
    console.warn('[zen] storage write failed:', err)
  }
}

export function createZenWindowStore(
  local: KeyValueStorage | null,
  label: string,
): UseBoundStore<StoreApi<ZenWindowState>> {
  const initial = parseZenWindowDoc(safeGet(local, zenWindowKey(label)))
  return create<ZenWindowState>((set, get) => {
    const save = (): void => {
      const s = get()
      safeSet(local, zenWindowKey(s.label), JSON.stringify({ version: 1, on: s.on, garden: s.garden } satisfies ZenWindowDoc))
    }
    return {
      label,
      on: initial?.on ?? false,
      garden: initial?.garden ?? null,
      setOn(on) {
        if (get().on === on) return
        set({ on })
        save()
      },
      setGarden(id) {
        if (get().garden === id) return
        set({ garden: id })
        save()
      },
    }
  })
}

function localOrNull(): KeyValueStorage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

/** This window's Zen switch. */
export const useZenWindowStore = createZenWindowStore(localOrNull(), getWindowLabel())

/** Tests only: start over as a fresh window `label` would (reads storage). */
export function __reloadZenWindowForTests(label = useZenWindowStore.getState().label): void {
  const doc = parseZenWindowDoc(safeGet(localOrNull(), zenWindowKey(label)))
  useZenWindowStore.setState({ label, on: doc?.on ?? false, garden: doc?.garden ?? null })
}
