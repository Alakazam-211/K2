// prd-zen-user-widgets-v2 UWB8, R7 — which agents a custom widget reaches:
// the Garden's reach, resolved on THIS device (Rosson 2026-10-08: no
// permissions, no scope picker).
//
// A widget in your own Garden reaches:
//   - this computer's agents (its workspaces, from the projects store when
//     the window is on this computer, else one `projects/list` from the
//     local daemon, cached per session), and
//   - every row on your Homes (`k2.homes.v1`): the rooms and servers you
//     connected in Home.
// Rows are the reach NOW (an agent added to a Home appears at once; a
// deleted Home gives fewer rows). A server you saved but never put on a
// Home is outside the reach. At most `ZEN_WIDGET_MAX_SERVERS` (R7) servers
// are live per widget. Remote rows use your own Connect login on that
// server, so its role floors apply (UWB8). The bridge enforces the reach
// (`zen-custom-bridge.ts`: `not_bound`).

import { create } from 'zustand'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { daemonCliGet } from '@/lib/daemon-cli'
import { scopeForHost } from '@/kessel/server-scope'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  homeAddress,
  parseHomeAddress,
  savedHostForKey,
  workspaceHandle,
} from '@/lib/home-address'

/** R7: at most this many servers live per widget. */
export const ZEN_WIDGET_MAX_SERVERS = 8

// ── Server rosters (this computer's agents when the window is elsewhere) ──

interface Roster {
  status: 'loading' | 'ready' | 'failed'
  rows: HomeRow[]
  at: number
}

/** Per-session cache of `projects/list` per server (never the window's own:
 *  that is the projects store). */
export const useZenServerRosters = create<{ rosters: Record<string, Roster> }>(() => ({ rosters: {} }))

/** Retry a failed roster no more often than this (ms). */
const ROSTER_RETRY_MS = 60_000
/** Asks queued for the next microtask (deduplicates within one render). */
const rosterAsked = new Set<string>()

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** `projects/list` → rows on `hostKey`. */
export function rosterRowsFrom(raw: unknown, hostKey: string): HomeRow[] {
  const list = Array.isArray(raw) ? raw : isObj(raw) && Array.isArray(raw.projects) ? raw.projects : null
  if (!list) throw new Error('projects/list: not a list')
  const out: HomeRow[] = []
  const seen = new Set<string>()
  for (const w of list) {
    if (!isObj(w) || typeof w.id !== 'string' || typeof w.name !== 'string') continue
    const handle = workspaceHandle({ handle: typeof w.handle === 'string' ? w.handle : null, name: w.name })
    if (!handle) continue
    const address = homeAddress(handle, hostKey)
    if (seen.has(address)) continue
    seen.add(address)
    out.push({ address, workspaceId: w.id, label: w.name })
  }
  return out
}

/** Ask for `hostKey`'s workspaces once (no-op while loading, ready, or a
 *  recent failure). Not a render-path fetch of the window's projects. */
export function requestZenServerRoster(hostKey: string, now: number = Date.now()): void {
  if (rosterAsked.has(hostKey)) return
  const cur = useZenServerRosters.getState().rosters[hostKey]
  if (cur && (cur.status !== 'failed' || now - cur.at < ROSTER_RETRY_MS)) return
  rosterAsked.add(hostKey)
  const set = (r: Roster): void =>
    useZenServerRosters.setState((s) => ({ rosters: { ...s.rosters, [hostKey]: r } }))
  // Never a store write during a render: everything happens next microtask.
  queueMicrotask(() => {
    set({ status: 'loading', rows: cur?.rows ?? [], at: now })
    rosterAsked.delete(hostKey)
    void daemonCliGet<unknown>(scopeForHost(hostKey), 'projects/list')
      .then((raw) => set({ status: 'ready', rows: rosterRowsFrom(raw, hostKey), at: Date.now() }))
      .catch((err: unknown) => {
        console.warn(`[zen] agents on ${hostKey} unavailable for a widget:`, err)
        set({ status: 'failed', rows: cur?.rows ?? [], at: Date.now() })
      })
  })
}

// ── The Garden's reach ─────────────────────────────────────────────────

/** The server's display name: "this computer", or the saved label. */
export function zenServerName(hostKey: string): string {
  if (hostKey === LOCAL_HOME_HOST) return 'this computer'
  const saved = savedHostForKey(useConnectHostStore.getState().hosts, hostKey)
  return saved ? saved.label || saved.hostname : hostKey
}

function hostOf(row: HomeRow): string {
  return parseHomeAddress(row.address)?.host ?? ''
}

function pushRows(out: Map<string, HomeRow>, rows: readonly HomeRow[]): void {
  for (const r of rows) if (!out.has(r.address)) out.set(r.address, r)
}

/** This computer's agents: its rows on every Home, then its workspaces. */
function localRows(homes: readonly Home[]): HomeRow[] {
  const out = new Map<string, HomeRow>()
  for (const h of homes) pushRows(out, h.rows.filter((r) => hostOf(r) === LOCAL_HOME_HOST))
  const windowHost = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  if (windowHost === LOCAL_HOME_HOST) {
    for (const p of useProjectsStore.getState().projects) {
      const handle = workspaceHandle(p)
      if (!handle) continue
      const address = homeAddress(handle, LOCAL_HOME_HOST)
      if (!out.has(address)) out.set(address, { address, workspaceId: p.id, label: p.name })
    }
  } else {
    requestZenServerRoster(LOCAL_HOME_HOST)
    pushRows(out, useZenServerRosters.getState().rosters[LOCAL_HOME_HOST]?.rows ?? [])
  }
  return [...out.values()]
}

export interface ZenReachRows {
  /** The rows a widget reaches now: this computer's first, then each
   *  Home's, deduplicated. */
  rows: HomeRow[]
  /** Servers those rows are on, in order (≤ 8). */
  servers: string[]
  /** Servers left out by the 8-server limit (R7). */
  overflow: string[]
}

/** The Garden's reach now (see the file comment). Pure over the stores
 *  (plus a roster ask when the window is on another server). */
export function zenWidgetReachRows(): ZenReachRows {
  const homes = useHomesStore.getState().homes
  const all = new Map<string, HomeRow>()
  pushRows(all, localRows(homes))
  for (const h of homes) pushRows(all, h.rows)
  const servers: string[] = []
  const overflow: string[] = []
  const rows: HomeRow[] = []
  for (const r of all.values()) {
    const host = hostOf(r)
    if (!servers.includes(host)) {
      if (servers.length >= ZEN_WIDGET_MAX_SERVERS) {
        if (!overflow.includes(host)) overflow.push(host)
        continue
      }
      servers.push(host)
    }
    rows.push(r)
  }
  return { rows, servers, overflow }
}

/** Tests only. */
export function __resetZenScopeForTests(): void {
  rosterAsked.clear()
  useZenServerRosters.setState({ rosters: {} })
}
