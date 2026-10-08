// prd-zen-user-widgets-v2 UWB7, UWB8, UWA8, QA1 — what a custom widget may
// see: its granted scope, resolved to agent rows on THIS device.
//
// A scope is one of `agent` · `home` · `homes` · `allHomes` · `server` ·
// `allServers` (`ZenScope`, wire form one key). Homes are device-local
// (`k2.homes.v1`), so the renderer resolves the scope at Allow (to write the
// grant's `entries`) and again at every mount and change (the bound rows):
//   - bound rows = the scope's rows NOW (QA1: an agent added to a granted
//     Home appears at once; a deleted Home gives fewer rows, never the
//     window's Home or a loose any-Home view);
//   - `server` = that server's agents: its rows on any Home plus its
//     workspaces (the window's own server from the projects store, any other
//     from one `projects/list` with your login there, cached per session);
//   - `allServers` = this computer plus every saved server, at most
//     `ZEN_WIDGET_MAX_SERVERS` (R7) live at once; the rest are left out and
//     the dialog says so.
// Remote rows use your own Connect login on that server, so its role floors
// apply (UWB8). Nothing here grants anything: the daemon stores and signs
// the grant, and the bridge enforces it (`zen-custom-bridge.ts`).

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
  homeHostKey,
  parseHomeAddress,
  savedHostForKey,
  workspaceHandle,
} from '@/lib/home-address'
import type { ZenGrantEntry, ZenScope } from './zen-custom-types'

/** R7: at most this many servers live per widget. */
export const ZEN_WIDGET_MAX_SERVERS = 8
/** The daemon takes 1–200 entries (UWA8). */
export const ZEN_GRANT_MAX_ENTRIES = 200

export type ZenScopeKind = 'agent' | 'home' | 'homes' | 'allHomes' | 'server' | 'allServers'

export function zenScopeKind(scope: ZenScope): ZenScopeKind {
  if ('agent' in scope) return 'agent'
  if ('home' in scope) return 'home'
  if ('homes' in scope) return 'homes'
  if ('allHomes' in scope) return 'allHomes'
  if ('server' in scope) return 'server'
  return 'allServers'
}

/** The canonical one-key JSON of a scope (equality and keys). */
export function zenScopeKey(scope: ZenScope): string {
  if ('homes' in scope) return JSON.stringify({ homes: [...scope.homes].sort() })
  return JSON.stringify(scope)
}

/** What a placement's `home` / `agent` props ask for (UW7, signed in the grant). */
export interface ZenPlacementAsk {
  home?: string
  agent?: string
}

// ── Server rosters (the `server` and `allServers` scopes) ────────────────

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

// ── Resolving a scope ─────────────────────────────────────────────────────

/** Every server a widget could see, this computer first, then saved ones. */
export function zenKnownServers(): string[] {
  const out = [LOCAL_HOME_HOST]
  for (const h of useConnectHostStore.getState().hosts) {
    const key = homeHostKey(h)
    if (!out.includes(key)) out.push(key)
  }
  return out
}

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

/** A server's agents: its rows on every Home, then its workspaces. */
function serverRows(hostKey: string, homes: readonly Home[]): HomeRow[] {
  const out = new Map<string, HomeRow>()
  for (const h of homes) pushRows(out, h.rows.filter((r) => hostOf(r) === hostKey))
  const windowHost = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  if (hostKey === windowHost) {
    for (const p of useProjectsStore.getState().projects) {
      const handle = workspaceHandle(p)
      if (!handle) continue
      const address = homeAddress(handle, hostKey)
      if (!out.has(address)) out.set(address, { address, workspaceId: p.id, label: p.name })
    }
  } else {
    requestZenServerRoster(hostKey)
    pushRows(out, useZenServerRosters.getState().rosters[hostKey]?.rows ?? [])
  }
  return [...out.values()]
}

/** A row by address (or handle, or name) on any Home. */
export function zenFindHomeRow(homes: readonly Home[], ref: string, homeId?: string | null): HomeRow | null {
  const want = ref.trim().toLocaleLowerCase()
  const pool = homeId ? homes.filter((h) => h.id === homeId) : homes
  for (const h of pool) {
    for (const r of h.rows) {
      if (r.address === want) return r
      if (parseHomeAddress(r.address)?.handle === want || r.label.toLocaleLowerCase() === want) return r
    }
  }
  return null
}

/** A Home by id, or by name (case aside). */
export function zenFindHome(homes: readonly Home[], ref: string | undefined | null): Home | null {
  if (!ref) return null
  const byId = homes.find((h) => h.id === ref)
  if (byId) return byId
  const lower = ref.toLocaleLowerCase()
  return homes.find((h) => h.name.toLocaleLowerCase() === lower) ?? null
}

export interface ZenScopeRows {
  /** The bound rows, in Home order (deduplicated). */
  rows: HomeRow[]
  /** Servers those rows are on, in order (≤ 8). */
  servers: string[]
  /** Servers left out by the 8-server limit (R7). */
  overflow: string[]
}

/** Resolve `scope` to rows now. Pure over the stores (plus a roster ask). */
export function zenScopeRows(scope: ZenScope | null): ZenScopeRows {
  if (!scope) return { rows: [], servers: [], overflow: [] }
  const homes = useHomesStore.getState().homes
  const all = new Map<string, HomeRow>()
  if ('agent' in scope) {
    const row = zenFindHomeRow(homes, scope.agent)
    if (row) all.set(row.address, row)
    else {
      // An agent on no Home: a server-roster row with that address.
      const host = parseHomeAddress(scope.agent)?.host
      if (host) {
        const r = serverRows(host, homes).find((x) => x.address === scope.agent.toLowerCase())
        if (r) all.set(r.address, r)
      }
    }
  } else if ('home' in scope) {
    pushRows(all, homes.find((h) => h.id === scope.home)?.rows ?? [])
  } else if ('homes' in scope) {
    for (const id of scope.homes) pushRows(all, homes.find((h) => h.id === id)?.rows ?? [])
  } else if ('allHomes' in scope) {
    for (const h of homes) pushRows(all, h.rows)
  } else if ('server' in scope) {
    pushRows(all, serverRows(scope.server, homes))
  } else {
    for (const host of zenKnownServers()) pushRows(all, serverRows(host, homes))
  }
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

/** The grant's `entries` (UWA8): `{server, room}` per bound row, sorted, ≤ 200. */
export function zenGrantEntries(rows: readonly HomeRow[]): ZenGrantEntry[] {
  const out: ZenGrantEntry[] = []
  const seen = new Set<string>()
  for (const r of rows) {
    const p = parseHomeAddress(r.address)
    if (!p) continue
    const key = `${p.host}\n${p.handle}`
    if (seen.has(key)) continue
    seen.add(key)
    out.push({ server: p.host, room: p.handle })
  }
  out.sort((a, b) => (a.server === b.server ? (a.room < b.room ? -1 : a.room > b.room ? 1 : 0) : a.server < b.server ? -1 : 1))
  return out.slice(0, ZEN_GRANT_MAX_ENTRIES)
}

// ── Words ─────────────────────────────────────────────────────────────────

function listWords(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ''
  if (names.length === 2) return `${names[0]} and ${names[1]}`
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/** The `{where}` words of a cap sentence for `scope` ("Work", "every Home"). */
export function zenScopeWhere(scope: ZenScope | null): string {
  if (!scope) return 'a Home'
  const homes = useHomesStore.getState().homes
  if ('agent' in scope) {
    const row = zenFindHomeRow(homes, scope.agent)
    return `only ${row?.label ?? parseHomeAddress(scope.agent)?.handle ?? scope.agent}`
  }
  if ('home' in scope) return zenFindHome(homes, scope.home)?.name ?? 'a Home that no longer exists'
  if ('homes' in scope) return listWords(scope.homes.map((id) => zenFindHome(homes, id)?.name ?? 'a deleted Home'))
  if ('allHomes' in scope) return 'all your Homes'
  if ('server' in scope) return zenServerName(scope.server)
  return 'every server you use'
}

/** The placement's ask as a fixed scope (UW22: "fixed when the placement
 *  names home"; "Only <agent>" when it names agent), or null when the
 *  person picks. A name that matches nothing on this device is `missing`. */
export function zenAskScope(ask: ZenPlacementAsk): { scope: ZenScope } | { missing: string } | null {
  const homes = useHomesStore.getState().homes
  const home = ask.home ? zenFindHome(homes, ask.home) : null
  if (ask.home && !home) return { missing: `No Home called “${ask.home}” on this computer.` }
  if (ask.agent) {
    const row = zenFindHomeRow(homes, ask.agent, home?.id ?? null)
    if (!row) return { missing: `No agent “${ask.agent}”${home ? ` in ${home.name}` : ' on your Homes'}.` }
    return { scope: { agent: row.address } }
  }
  if (home) return { scope: { home: home.id } }
  return null
}

/** Tests only. */
export function __resetZenScopeForTests(): void {
  rosterAsked.clear()
  useZenServerRosters.setState({ rosters: {} })
}
