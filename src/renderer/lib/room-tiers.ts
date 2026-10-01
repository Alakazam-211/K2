// Home M4 — room tiers (prd-home-multi-server-client MS24, MS46; answer Q7).
//
// How connected each open room on another server stays, per window:
//
//   | Tier | When                                         | Sockets (M4 wires them)             |
//   |------|----------------------------------------------|-------------------------------------|
//   | hot  | on screen, or off screen < 5 min (cap 3)     | full set: events + grids + overlays |
//   | warm | off screen 5–30 min (cap 6)                  | one workspace sessions/events socket|
//   | cold | off screen ≥ 30 min, never shown, or closed  | none (the Home row's status check)  |
//
// A fourth hot room demotes the least recently shown hot room to warm; a
// seventh warm room demotes the least recent warm room to cold. Tiers only
// go DOWN while a room is off screen: a room demoted by a cap is not
// re-promoted when another room closes. Only showing it again makes it hot.
//
// The 3 hot / 5 min values are approved (plan decision 2). 6 warm and the
// 30 min warm-to-cold value are placeholders until the M4 measurement on a
// signed z3mbpZ build (MS47/MS48, Q7). All four live in
// `DEFAULT_ROOM_TIER_CONFIG` and can be changed with `reconfigure`.
//
// This module is pure logic plus a small store. It opens nothing itself: it
// emits tier changes, and M4's room shell opens and closes that room's
// sockets on them.
//
// `stores/home-rooms.ts` drives it: `show(room.key)` when a Home row's room
// comes on screen, `hide` when another room (or the window's own room, or
// another page) replaces it, `close` on dispose. On `onTierChange`: hot =
// the room is mounted (grids, overlays, its workspace socket), warm = only
// its tabs store and ONE workspace socket stay, cold = the room is disposed
// and forgotten. Keys are `Room.key` (`<hostKey>|<projectId>:<workspaceId>`).

import { createStore, type StoreApi } from 'zustand/vanilla'
import { useStore } from 'zustand'

export type RoomTier = 'hot' | 'warm' | 'cold'

export interface RoomTierConfig {
  /** Hot rooms per window (MS24: 3, approved). */
  hotCap: number
  /** Warm rooms per window (MS24: 6, placeholder). */
  warmCap: number
  /** Off screen this long → warm (MS24: 5 min, approved). */
  hotGraceMs: number
  /** Off screen this long → cold (MS24: 30 min, placeholder, Q7). */
  coldAfterMs: number
}

export const DEFAULT_ROOM_TIER_CONFIG: Readonly<RoomTierConfig> = Object.freeze({
  hotCap: 3,
  warmCap: 6,
  hotGraceMs: 5 * 60_000,
  coldAfterMs: 30 * 60_000,
})

export interface RoomTierEntry {
  roomKey: string
  tier: RoomTier
  /** On screen right now. */
  visible: boolean
  /** When it last left the screen; null while visible or if never shown. */
  hiddenAt: number | null
  /** When it was last shown; null if never shown. */
  shownAt: number | null
  /** Show order in this window (recency for the caps; breaks clock ties). */
  shownSeq: number
  /** The highest tier it may hold until shown again (caps only demote). */
  ceiling: RoomTier
}

export interface RoomTierChange {
  roomKey: string
  from: RoomTier
  to: RoomTier
}

const RANK: Record<RoomTier, number> = { cold: 0, warm: 1, hot: 2 }

function minTier(a: RoomTier, b: RoomTier): RoomTier {
  return RANK[a] <= RANK[b] ? a : b
}

function validateConfig(c: RoomTierConfig): void {
  for (const [k, v] of Object.entries(c)) {
    if (!Number.isFinite(v) || v < 0) throw new Error(`room tiers: ${k} must be a finite number ≥ 0 (got ${v})`)
  }
  if (c.coldAfterMs < c.hotGraceMs) {
    throw new Error('room tiers: coldAfterMs must be ≥ hotGraceMs')
  }
}

/** The tier time alone gives a room (no caps). */
export function timeTier(
  entry: Pick<RoomTierEntry, 'visible' | 'hiddenAt'>,
  now: number,
  config: RoomTierConfig,
): RoomTier {
  if (entry.visible) return 'hot'
  if (entry.hiddenAt === null) return 'cold'
  const off = now - entry.hiddenAt
  if (off < config.hotGraceMs) return 'hot'
  if (off < config.coldAfterMs) return 'warm'
  return 'cold'
}

/** Most recent first: visible rooms, then by when they were last shown. */
function byRecency(a: RoomTierEntry, b: RoomTierEntry): number {
  if (a.visible !== b.visible) return a.visible ? -1 : 1
  return b.shownSeq - a.shownSeq
}

/**
 * Pure: every room's tier at `now`. Time first, then the ceiling (a
 * hidden room never climbs back), then the hot cap (excess → warm), then
 * the warm cap (excess → cold). Visible rooms are always hot, even past
 * the cap. Returns `roomKey → tier`.
 */
export function computeRoomTiers(
  entries: readonly RoomTierEntry[],
  now: number,
  config: RoomTierConfig,
): Record<string, RoomTier> {
  const out: Record<string, RoomTier> = {}
  for (const e of entries) {
    const t = timeTier(e, now, config)
    out[e.roomKey] = e.visible ? t : minTier(t, e.ceiling)
  }
  const sorted = [...entries].sort(byRecency)
  let hot = 0
  for (const e of sorted) {
    if (out[e.roomKey] !== 'hot') continue
    if (e.visible || hot < config.hotCap) {
      hot += 1
      continue
    }
    out[e.roomKey] = 'warm'
  }
  let warm = 0
  for (const e of sorted) {
    if (out[e.roomKey] !== 'warm') continue
    if (warm < config.warmCap) {
      warm += 1
      continue
    }
    out[e.roomKey] = 'cold'
  }
  return out
}

/** When the next time-based change can happen, or null if none. */
export function nextTierDeadline(
  entries: readonly RoomTierEntry[],
  config: RoomTierConfig,
): number | null {
  let next: number | null = null
  for (const e of entries) {
    if (e.visible || e.hiddenAt === null) continue
    const at =
      e.tier === 'hot' ? e.hiddenAt + config.hotGraceMs : e.tier === 'warm' ? e.hiddenAt + config.coldAfterMs : null
    if (at !== null && (next === null || at < next)) next = at
  }
  return next
}

export interface RoomTierManagerDeps {
  now(): number
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(handle: unknown): void
  config?: Partial<RoomTierConfig>
}

export interface RoomTierManager {
  readonly store: StoreApi<{ rooms: Record<string, RoomTierEntry> }>
  config(): RoomTierConfig
  /** The room is on screen: hot. Registers it if new. */
  show(roomKey: string): void
  /** The room left the screen (still mounted): its off-screen clock starts. */
  hide(roomKey: string): void
  /** The room was disposed: emits a change to cold (if not cold) and forgets it. */
  close(roomKey: string): void
  /** The room's tier, or cold for an unknown room. */
  tier(roomKey: string): RoomTier
  onTierChange(fn: (change: RoomTierChange) => void): () => void
  /** Change the numbers (Q7 placeholders) and re-apply them now. */
  reconfigure(patch: Partial<RoomTierConfig>): void
  /** Stop the timer and drop every room (no events). */
  dispose(): void
}

export function createRoomTierManager(deps: RoomTierManagerDeps): RoomTierManager {
  let cfg: RoomTierConfig = { ...DEFAULT_ROOM_TIER_CONFIG, ...deps.config }
  validateConfig(cfg)
  const store = createStore<{ rooms: Record<string, RoomTierEntry> }>(() => ({ rooms: {} }))
  const listeners = new Set<(c: RoomTierChange) => void>()
  let timer: unknown = null
  let seq = 0

  const schedule = (): void => {
    if (timer !== null) {
      deps.clearTimer(timer)
      timer = null
    }
    const at = nextTierDeadline(Object.values(store.getState().rooms), cfg)
    if (at === null) return
    timer = deps.setTimer(() => {
      timer = null
      recompute()
    }, Math.max(0, at - deps.now()))
  }

  /** Re-apply time and caps to `rooms`, store it, emit changes, re-arm. */
  const commit = (rooms: Record<string, RoomTierEntry>, extra: RoomTierChange[] = []): void => {
    const now = deps.now()
    const tiers = computeRoomTiers(Object.values(rooms), now, cfg)
    const changes: RoomTierChange[] = [...extra]
    const next: Record<string, RoomTierEntry> = {}
    for (const [key, e] of Object.entries(rooms)) {
      const to = tiers[key]
      if (to !== e.tier) changes.push({ roomKey: key, from: e.tier, to })
      next[key] = { ...e, tier: to, ceiling: e.visible ? 'hot' : to }
    }
    store.setState({ rooms: next })
    schedule()
    for (const c of changes) for (const fn of [...listeners]) fn(c)
  }

  const recompute = (): void => commit({ ...store.getState().rooms })

  const blank = (roomKey: string): RoomTierEntry => ({
    roomKey,
    tier: 'cold',
    visible: false,
    hiddenAt: null,
    shownAt: null,
    shownSeq: 0,
    ceiling: 'cold',
  })

  return {
    store,
    config: () => ({ ...cfg }),
    show(roomKey) {
      const rooms = { ...store.getState().rooms }
      const prev = rooms[roomKey] ?? blank(roomKey)
      rooms[roomKey] = { ...prev, visible: true, hiddenAt: null, shownAt: deps.now(), shownSeq: ++seq, ceiling: 'hot' }
      commit(rooms)
    },
    hide(roomKey) {
      const rooms = { ...store.getState().rooms }
      const prev = rooms[roomKey]
      if (!prev || !prev.visible) return
      rooms[roomKey] = { ...prev, visible: false, hiddenAt: deps.now() }
      commit(rooms)
    },
    close(roomKey) {
      const rooms = { ...store.getState().rooms }
      const prev = rooms[roomKey]
      if (!prev) return
      delete rooms[roomKey]
      commit(rooms, prev.tier === 'cold' ? [] : [{ roomKey, from: prev.tier, to: 'cold' }])
    },
    tier(roomKey) {
      return store.getState().rooms[roomKey]?.tier ?? 'cold'
    },
    onTierChange(fn) {
      listeners.add(fn)
      return () => {
        listeners.delete(fn)
      }
    },
    reconfigure(patch) {
      const next = { ...cfg, ...patch }
      validateConfig(next)
      cfg = next
      recompute()
    },
    dispose() {
      if (timer !== null) deps.clearTimer(timer)
      timer = null
      listeners.clear()
      store.setState({ rooms: {} })
    },
  }
}

/** This window's tier manager (one per webview). */
export const roomTiers: RoomTierManager = createRoomTierManager({
  now: () => Date.now(),
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
})

/** React hook: a room's tier in this window. */
export function useRoomTier(roomKey: string): RoomTier {
  return useStore(roomTiers.store, (s) => s.rooms[roomKey]?.tier ?? 'cold')
}
