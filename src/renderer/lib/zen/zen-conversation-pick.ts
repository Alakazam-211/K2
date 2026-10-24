// Fleet Ops 2 (Rosson 2026-10-24) — a Conversation widget with `pick`: a
// short list of agents, and its header is the drop-down that switches
// between them (one layer, no Agents widget above it).
//
// `pick` is 2 to 8 agent names or addresses (a TOML array; a
// comma-separated string is read too). Each is matched like the pinned
// `agent` prop (`zenMatchesAgent`: the row address, else its handle, else
// its name) against the Garden's reach: this computer's agents and every
// row on the person's Homes, on any server (`zenWidgetReachRows`). The
// strongest match wins (address, then handle, then name), first in reach
// order (this computer first).
//
// Which one is shown is view state, like a column fold: per Garden and
// widget, on this computer, in localStorage `k2.zen.conversationPicks.v1`
// = `{version: 1, picks: {"<gardenId>/<widgetId>": "<row address>"}}`,
// shared by every window through the `storage` event. Unset (or the saved
// agent is gone): the first agent in `pick` that is found.
//
// Storage can throw: reads and writes go through try/catch and fall back to
// memory. A stored value that isn't ours is not overwritten until a pick.

import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { parseHomeAddress } from '@/lib/home-address'

export const ZEN_CONVERSATION_PICKS_KEY = 'k2.zen.conversationPicks.v1'
/** `pick` holds 2 to 8 agents (the validator refuses others). */
export const ZEN_PICK_MIN = 2
export const ZEN_PICK_MAX = 8

/** The widget's `pick` prop: the names in file order (trimmed, blanks
 *  dropped), or null when unset or empty. */
export function zenConversationPick(props: Record<string, unknown>): string[] | null {
  const v = props.pick
  const raw = Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : typeof v === 'string' ? v.split(',') : []
  const names = raw.map((s) => s.trim()).filter((s) => s !== '')
  return names.length > 0 ? names : null
}

/** Does `row` answer to `agent` (a row address, handle or name)? The
 *  pinned `agent` prop's rule. */
export function zenMatchesAgent(row: { address: string; label: string }, agent: string): boolean {
  const a = agent.trim().toLocaleLowerCase()
  if (row.address === a) return true
  const p = parseHomeAddress(row.address)
  return (p !== null && p.handle === a) || row.label.toLocaleLowerCase() === a
}

/** The row `agent` names among `rows`: an address match, else a handle
 *  match, else a name match; the first in `rows` order. */
export function zenFindPickRow<T extends { address: string; label: string }>(rows: readonly T[], agent: string): T | null {
  const a = agent.trim().toLocaleLowerCase()
  return (
    rows.find((r) => r.address === a) ??
    rows.find((r) => parseHomeAddress(r.address)?.handle === a) ??
    rows.find((r) => zenMatchesAgent(r, a)) ??
    null
  )
}

export interface ZenConversationPicksDoc {
  version: 1
  picks: Record<string, string>
}

export interface KeyValueStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export function parseZenConversationPicksDoc(raw: string | null): ZenConversationPicksDoc | null {
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

export interface ZenConversationPicksState {
  /** The shown agent's row address, by `<gardenId>/<widgetId>`. */
  picks: Record<string, string>
  setPick(key: string, address: string): void
  /** Another window saved `raw` (the `storage` event). */
  applyExternal(raw: string | null): void
}

function safeGet(kv: KeyValueStorage | null): string | null {
  if (!kv) return null
  try {
    return kv.getItem(ZEN_CONVERSATION_PICKS_KEY)
  } catch (err) {
    console.warn('[zen] conversation pick storage read failed:', err)
    return null
  }
}

function safeSet(kv: KeyValueStorage | null, value: string): void {
  if (!kv) return
  try {
    kv.setItem(ZEN_CONVERSATION_PICKS_KEY, value)
  } catch (err) {
    console.warn('[zen] conversation pick storage write failed:', err)
  }
}

export function createZenConversationPicksStore(
  local: KeyValueStorage | null,
): UseBoundStore<StoreApi<ZenConversationPicksState>> {
  const initial = parseZenConversationPicksDoc(safeGet(local))
  return create<ZenConversationPicksState>((set, get) => ({
    picks: initial?.picks ?? {},
    setPick(key, address) {
      if (!key || !address || get().picks[key] === address) return
      const picks = { ...get().picks, [key]: address }
      set({ picks })
      safeSet(local, JSON.stringify({ version: 1, picks } satisfies ZenConversationPicksDoc))
    },
    applyExternal(raw) {
      set({ picks: parseZenConversationPicksDoc(raw)?.picks ?? {} })
    },
  }))
}

/** Keep `store` in step with other windows. Returns the unsubscribe. */
export function attachZenConversationPicksStorageSync(
  store: UseBoundStore<StoreApi<ZenConversationPicksState>>,
  target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>,
  local: KeyValueStorage | null,
): () => void {
  const onStorage = (e: Event): void => {
    const se = e as Event & { key?: string | null; newValue?: string | null }
    if (se.key === ZEN_CONVERSATION_PICKS_KEY) store.getState().applyExternal(se.newValue ?? null)
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

export const useZenConversationPicksStore = createZenConversationPicksStore(localKv)

if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  attachZenConversationPicksStorageSync(useZenConversationPicksStore, window, localKv)
}
