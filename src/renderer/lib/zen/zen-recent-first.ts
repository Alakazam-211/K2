// "Recent agents first" in Gardens (Rosson 2026-10-15, ported from K2
// Companion's `recentFirst.ts`, Zen Settings → Lists). A setting of the
// Garden itself, reached from Garden 1's Agents header (its cog), never the
// app's Settings window, and never sent to a server: it lives on this
// computer only. Off by default.
//
// On, a Garden's Agents widget keeps pinned agents at the top, then puts the
// agents you last sent a message to from a Garden on this computer, newest
// first; agents never messaged keep the list's own order after them. Off,
// the list is exactly the Home's (or the Agents view's) order. The app's own
// Agents page and sidebar never read it.
//
// "Sent to" = a send from a Garden's message box (`ZenCompose`). The time is
// kept per row address (`handle::host`, lowercase — host + handle, so the
// same handle on two servers is two keys) in `k2.zen.recentSent.v1`, capped
// at the newest 200, and recorded whether the setting is on or not, so
// turning it on already has history.
//
// Every localStorage read and write is in try/catch: a failure keeps the
// value in memory for this window.

import { create } from 'zustand'

export const ZEN_RECENT_FIRST_KEY = 'k2.zen.recentFirst'
export const ZEN_RECENT_SENT_KEY = 'k2.zen.recentSent.v1'
/** Rows remembered; the oldest sends drop off past this. */
export const ZEN_RECENT_SENT_MAX = 200
/** The setting before the person picks (Rosson 2026-10-15: off, like the
 *  phone). One line to change. */
export const ZEN_RECENT_FIRST_DEFAULT = false

/** The stored setting, else the default. */
export function readZenRecentFirst(): boolean {
  try {
    const v = localStorage.getItem(ZEN_RECENT_FIRST_KEY)
    if (v === '1') return true
    if (v === '0') return false
    return ZEN_RECENT_FIRST_DEFAULT
  } catch {
    return ZEN_RECENT_FIRST_DEFAULT
  }
}

/** `{address: ms}`; anything else reads as empty. */
export function readZenRecentSent(): Record<string, number> {
  try {
    const raw = localStorage.getItem(ZEN_RECENT_SENT_KEY)
    if (!raw) return {}
    const v = JSON.parse(raw) as unknown
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {}
    const out: Record<string, number> = {}
    for (const [k, at] of Object.entries(v as Record<string, unknown>)) {
      if (typeof at === 'number' && Number.isFinite(at)) out[k] = at
    }
    return out
  } catch {
    return {}
  }
}

/** A row's key: its address (`handle::host`), trimmed and lowercased. */
export function zenRecentKey(address: string | null | undefined): string | null {
  const key = (address ?? '').trim().toLowerCase()
  return key || null
}

/** Keep the newest `max` sends. */
export function trimZenRecentSent(sent: Record<string, number>, max = ZEN_RECENT_SENT_MAX): Record<string, number> {
  const keys = Object.keys(sent)
  if (keys.length <= max) return sent
  return Object.fromEntries(
    keys
      .sort((a, b) => sent[b] - sent[a])
      .slice(0, max)
      .map((k) => [k, sent[k]]),
  )
}

/**
 * The "Recent agents first" order: pinned rows first (their order kept),
 * then rows sent to, newest send first, then every other row in its
 * original relative order.
 */
export function orderZenRecentFirst<T>(
  rows: readonly T[],
  sent: Record<string, number>,
  keyOf: (row: T) => string | null,
  isPinned: (row: T) => boolean = () => false,
): T[] {
  const pinned: T[] = []
  const recent: { row: T; at: number; index: number }[] = []
  const rest: T[] = []
  rows.forEach((row, index) => {
    if (isPinned(row)) {
      pinned.push(row)
      return
    }
    const key = keyOf(row)
    const at = key ? sent[key] : undefined
    if (typeof at === 'number') recent.push({ row, at, index })
    else rest.push(row)
  })
  recent.sort((a, b) => b.at - a.at || a.index - b.index)
  return [...pinned, ...recent.map((r) => r.row), ...rest]
}

interface ZenRecentFirstState {
  /** The setting. */
  on: boolean
  setOn(on: boolean): void
  /** Last send from a Garden on this computer, ms, by `zenRecentKey`. */
  sent: Record<string, number>
  markSent(address: string | null | undefined, at?: number): void
  /** Re-read storage (tests). */
  reload(): void
}

export const useZenRecentFirstStore = create<ZenRecentFirstState>((set, get) => ({
  on: readZenRecentFirst(),
  setOn: (on) => {
    try {
      localStorage.setItem(ZEN_RECENT_FIRST_KEY, on ? '1' : '0')
    } catch {
      /* the choice lasts this window */
    }
    set({ on })
  },
  sent: readZenRecentSent(),
  markSent: (address, at = Date.now()) => {
    const key = zenRecentKey(address)
    if (!key) return
    const sent = trimZenRecentSent({ ...get().sent, [key]: at })
    try {
      localStorage.setItem(ZEN_RECENT_SENT_KEY, JSON.stringify(sent))
    } catch {
      /* quota / private mode: kept in memory */
    }
    set({ sent })
  },
  reload: () => set({ on: readZenRecentFirst(), sent: readZenRecentSent() }),
}))
