// Garden v3 data sources — test fixtures (card views and a fake server set
// behind `ZenSourcesDeps`). Made-up names only (box.example.test). Imported
// by tests and by the built-in card fixtures; never by app code.

import type { ServerScope } from '@/kessel/server-scope'
import type { ItemView, SourceResult, SourcesChangesResponse } from '@/lib/sources/sources-types'
import type { ZenSourcesDeps } from './zen-sources'

export function fixtureItem(key: string, over: Partial<ItemView> = {}): ItemView {
  return {
    key,
    health: 'ok',
    reason: null,
    since: null,
    soft: { count: 0, of: 3 },
    fields: { version: '0.45.6', phase: 'ready' },
    ...over,
  }
}

/** A live, green fleet view at serverNow 1_760_423_520 (seconds). */
export function fixtureView(over: Partial<SourceResult> = {}): SourceResult {
  return {
    name: 'fleet',
    label: 'Fleet boxes',
    kind: 'http-json',
    status: 'active',
    health: 'ok',
    phase: 'live',
    display: 'ok',
    reason: null,
    reasonText: null,
    httpStatus: 200,
    soft: { count: 0, of: 3 },
    serverNow: 1_760_423_520,
    updatedAt: 1_760_423_500,
    checkedAt: 1_760_423_500,
    nextAt: 1_760_423_560,
    hardSince: null,
    recoveredAt: null,
    gap: null,
    muted: null,
    simulated: null,
    pinned: false,
    keep: 'values',
    masked: false,
    link: 'https://box.example.test/admin',
    fields: { newest: 'v0.45.6' },
    items: [fixtureItem('alpha'), fixtureItem('beta')],
    counts: { ok: 2, warn: 0, crit: 0, unknown: 0 },
    rev: 10,
    ...over,
  }
}

export function fakeScope(hostKey: string, isPrimary = false): ServerScope {
  return { id: isPrimary ? 'primary' : `host:${hostKey}`, isPrimary, hostKey } as unknown as ServerScope
}

/** One fake server: its sources, an error to throw, and call counts. */
export interface FakeServer {
  views: Map<string, SourceResult>
  removed: Set<string>
  down: boolean
  rev: number
  error: Record<string, unknown> | null
  reads: number
  changes: number[]
  refreshes: number
}

export interface FakeSources {
  deps: ZenSourcesDeps
  /** A server by host key (made on first use). */
  servers: { get(key: string): FakeServer }
  /** Fire the socket for the window's server. */
  socket(rev: number): void
  /** Run the poll timers once. */
  tick(): void
  visible: { v: boolean }
  timers: number
}

function envelope(code: string, extra: Record<string, unknown> = {}): Error {
  return new Error(JSON.stringify({ error: { code, hint: `fake ${code}`, ...extra } }))
}

/** `saved` lists the hosts that are saved servers; `window` is the window's server ('local' by default). */
export function fakeSources(saved: string[] = [], window = 'local'): FakeSources {
  const servers = new Map<string, FakeServer>()
  const server = (key: string): FakeServer => {
    let s = servers.get(key)
    if (!s) {
      s = { views: new Map(), removed: new Set(), down: false, rev: 10, error: null, reads: 0, changes: [], refreshes: 0 }
      servers.set(key, s)
    }
    return s
  }
  const socketFns = new Set<(rev: number) => void>()
  const timers = new Map<number, () => void>()
  let nextTimer = 1
  const visible = { v: true }
  const fail = (s: FakeServer, name: string): void => {
    if (s.down) throw new TypeError('Failed to fetch')
    if (s.error) throw envelope(String(s.error.code), s.error)
    if (s.removed.has(name)) throw envelope('removed', { removedAt: 1 })
    if (!s.views.has(name)) throw envelope('source_not_found')
  }
  const deps: ZenSourcesDeps = {
    scopeFor: (host) => {
      const key = host ?? 'local'
      if (key !== 'local' && !saved.includes(key)) return null
      return fakeScope(key, key === window)
    },
    read: async (scope, name) => {
      const s = server(scope.hostKey)
      s.reads += 1
      fail(s, name)
      return structuredClone(s.views.get(name)!)
    },
    history: async (scope, name, field) => {
      const s = server(scope.hostKey)
      fail(s, name)
      if (s.views.get(name)!.keep === 'states') throw envelope('no_history')
      return { field, res: 'raw', points: [{ t: 1, v: 2 }, { t: 2, v: null }] }
    },
    changes: async (scope, since): Promise<SourcesChangesResponse> => {
      const s = server(scope.hostKey)
      s.changes.push(since)
      if (s.down) throw new TypeError('Failed to fetch')
      return {
        rev: s.rev,
        full: since === 0,
        changed: [...s.views.values()].filter((v) => v.rev > since).map((v) => structuredClone(v)),
        removed: [...s.removed],
      }
    },
    refresh: async (scope, name) => {
      const s = server(scope.hostKey)
      fail(s, name)
      s.refreshes += 1
      if (s.refreshes > 1) throw envelope('rate_limited', { retryAfterSecs: 15 })
      return { queued: true, nextAt: 1_760_423_521 }
    },
    onChanged: (scope, fn) => {
      if (!scope.isPrimary) return null
      socketFns.add(fn)
      return () => void socketFns.delete(fn)
    },
    visible: () => visible.v,
    setInterval: (fn) => {
      const id = nextTimer++
      timers.set(id, fn)
      return id
    },
    clearInterval: (t) => void timers.delete(t as number),
  }
  return {
    deps,
    servers: { get: server },
    socket: (rev) => {
      for (const fn of [...socketFns]) fn(rev)
    },
    tick: () => {
      for (const fn of [...timers.values()]) fn()
    },
    visible,
    get timers() {
      return timers.size
    },
  }
}

/** Let pending promises settle. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve()
}
