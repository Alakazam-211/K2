// Garden v3 data sources — the Garden's reader (contract-sources-v1 §6,
// §11.4; PRD §4.2, GV44, GV45). Cards never fetch on their own: the card
// view comes from the daemon that runs the source, read here and handed to
// the frame through the bridge's `sources.*` verbs.
//
// Addresses: `name` is this computer's daemon (the local owner token, so no
// role floor; GV44a); `name::host` is another server, resolved against your
// SAVED servers (Settings → Network → Connected Servers), never Home rows,
// and read with your own Connect login there, so its floors apply. A host
// that isn't saved is `server_not_saved`.
//
// Live updates (GV45): the app-level `sources_changed {rev}` socket for this
// computer and the window's own server; every other server is polled with
// `GET /cli/sources/changes?since=<rev>` every 20 s, only while a widget is
// subscribed and the window is visible. Per server, the reader keeps the
// last good view of each source; an unreachable server hands that view back
// greyed (`phase: "stale"`, `reach: "unreachable"`), a removed source as
// `reach: "removed"`.
//
// Limits: at most 8 source servers per Garden, not counting this computer.

import type { ServerScope } from '@/kessel/server-scope'
import type {
  JsonValue,
  SourceHistoryResponse,
  SourceRefreshResponse,
  SourceResult,
  SourcesChangesResponse,
} from '@/lib/sources/sources-types'

/** Renderer constants mirrored in `k2_core::sources` (contract §3.6). */
export const ZEN_SOURCES_POLL_MS = 20_000
export const ZEN_SOURCE_SERVERS_PER_GARDEN = 8

/** Why a view is the last one K2 has, or `ok`. */
export type ZenSourceReach = 'ok' | 'unreachable' | 'server_not_saved' | 'cap_not_granted' | 'removed'

/** A scalar the frame gets in `fields` (anything else arrives as JSON text). */
export type ZenSourceScalar = string | number | boolean | null

/** What a widget gets: the card view plus its address and reach. */
export interface ZenSourceView extends Omit<SourceResult, 'fields' | 'items'> {
  address: string
  reach: ZenSourceReach
  fields: Record<string, ZenSourceScalar>
  items: Array<Omit<SourceResult['items'][number], 'fields'> & { fields: Record<string, ZenSourceScalar> }>
}

export interface ZenSourceListRow {
  address: string
  label: string | null
  health: SourceResult['health'] | null
  phase: SourceResult['phase'] | null
  display: SourceResult['display'] | null
  reach: ZenSourceReach
}

export type ZenSourceErrorCode =
  | 'server_not_saved'
  | 'unreachable'
  | 'removed'
  | 'cap_not_granted'
  | 'no_history'
  | 'rate_limited'
  | 'failed'

/** A refused read, in the bridge's error codes. `last` rides `unreachable`
 *  and `removed`: the last view K2 has, greyed. */
export class ZenSourceError extends Error {
  readonly code: ZenSourceErrorCode
  readonly last: ZenSourceView | undefined
  readonly role: string | undefined
  constructor(code: ZenSourceErrorCode, message: string, extra: { last?: ZenSourceView; role?: string } = {}) {
    super(message)
    this.name = 'ZenSourceError'
    this.code = code
    this.last = extra.last
    this.role = extra.role
  }
}

// ── addresses ─────────────────────────────────────────────────────────

const NAME = /^[a-z0-9-]{1,40}$/
const HOST = /^[a-z0-9.-]{1,253}(:\d{1,5})?$/

/** `fleet` or `fleet::acme.k2.dev` (Rust `sources::Address::parse`). */
export function parseSourceAddress(address: string): { name: string; host: string | null } | null {
  const a = address.trim()
  const i = a.indexOf('::')
  const name = i < 0 ? a : a.slice(0, i)
  if (!NAME.test(name)) return null
  if (i < 0) return { name, host: null }
  const host = a.slice(i + 2).toLowerCase()
  if (!HOST.test(host) || host.startsWith('.') || host.endsWith('.') || host.includes('..')) return null
  const port = host.includes(':') ? Number(host.split(':')[1]) : null
  if (port !== null && (port < 1 || port > 65535)) return null
  return { name, host }
}

/** The address as one canonical string (host lower-cased). */
export function canonicalSourceAddress(address: string): string | null {
  const p = parseSourceAddress(address)
  return p ? (p.host ? `${p.name}::${p.host}` : p.name) : null
}

// ── projection ────────────────────────────────────────────────────────

function scalar(v: JsonValue): ZenSourceScalar {
  if (v === null || typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') return v
  return JSON.stringify(v)
}

function scalars(m: Record<string, JsonValue> | undefined): Record<string, ZenSourceScalar> {
  const out: Record<string, ZenSourceScalar> = {}
  for (const [k, v] of Object.entries(m ?? {})) out[k] = scalar(v)
  return out
}

/** The daemon's card view → the frame's (the catalog's `SourceView`). */
export function zenSourceView(address: string, r: SourceResult, reach: ZenSourceReach = 'ok'): ZenSourceView {
  return {
    ...r,
    address,
    reach,
    fields: scalars(r.fields),
    items: (r.items ?? []).map((it) => ({ ...it, fields: scalars(it.fields) })),
  }
}

/** A last view, greyed: the reach it's shown with, phase `stale`. */
export function zenStaleView(v: ZenSourceView, reach: ZenSourceReach): ZenSourceView {
  return { ...v, reach, phase: 'stale', display: reach === 'removed' ? v.display : 'stale' }
}

// ── errors ────────────────────────────────────────────────────────────

/** A daemon call's failure → the bridge's code. A failure with no daemon
 *  answer (offline, signed out, refused connection) is `unreachable`. */
export function zenSourceErrorCode(err: unknown): { code: ZenSourceErrorCode; role?: string; message: string } {
  if (err instanceof ZenSourceError) return { code: err.code, role: err.role, message: err.message }
  if (err instanceof Error && err.name === 'ServerRemovedError') {
    return { code: 'server_not_saved', message: 'That server isn’t one of your saved servers.' }
  }
  const text = err instanceof Error ? err.message : String(err)
  let parsed: unknown = null
  try {
    parsed = JSON.parse(text)
  } catch {
    parsed = null
  }
  const e =
    parsed && typeof parsed === 'object' && 'error' in parsed && parsed.error && typeof parsed.error === 'object'
      ? (parsed.error as { code?: unknown; hint?: unknown; role?: unknown })
      : null
  if (!e || typeof e.code !== 'string') return { code: 'unreachable', message: 'The source’s server didn’t answer.' }
  const hint = typeof e.hint === 'string' ? e.hint : e.code
  switch (e.code) {
    case 'removed':
      return { code: 'removed', message: hint }
    case 'cap_not_granted':
    case 'role_required':
      return { code: 'cap_not_granted', role: typeof e.role === 'string' ? e.role : undefined, message: hint }
    case 'invalid_token':
      return { code: 'unreachable', message: 'Signed out of that server.' }
    case 'no_history':
      return { code: 'no_history', message: hint }
    case 'rate_limited':
      return { code: 'rate_limited', message: hint }
    default:
      return { code: 'failed', message: hint }
  }
}

// ── the reader ────────────────────────────────────────────────────────

/** What the reader needs (the real one is `zenSourcesReader()`; tests fake it). */
export interface ZenSourcesDeps {
  /** `null` = this computer. A host that isn't saved throws `ServerRemovedError`-named or returns null. */
  scopeFor(host: string | null): ServerScope | null
  read(scope: ServerScope, name: string): Promise<SourceResult>
  history(scope: ServerScope, name: string, field: string, opts: { fromAt?: number; toAt?: number }): Promise<SourceHistoryResponse>
  changes(scope: ServerScope, since: number): Promise<SourcesChangesResponse>
  refresh(scope: ServerScope, name: string): Promise<SourceRefreshResponse>
  /** The app socket's `sources_changed` for this scope, when it is this
   *  computer or the window's server; null otherwise (the reader polls). */
  onChanged(scope: ServerScope, fn: (rev: number) => void): (() => void) | null
  visible(): boolean
  setInterval(fn: () => void, ms: number): unknown
  clearInterval(t: unknown): void
}

type Listener = (v: ZenSourceView) => void

interface Server {
  key: string
  scope: ServerScope
  rev: number
  views: Map<string, ZenSourceView>
  listeners: Map<string, Set<Listener>>
  stop: (() => void) | null
  timer: unknown
  inFlight: boolean
  down: boolean
}

export interface ZenSourcesReader {
  read(address: string, garden?: string): Promise<ZenSourceView>
  history(address: string, field: string, opts?: { fromAt?: number; toAt?: number }, garden?: string): Promise<SourceHistoryResponse>
  refresh(address: string, garden?: string): Promise<SourceRefreshResponse>
  list(addresses: readonly string[], garden?: string): Promise<ZenSourceListRow[]>
  /** Pushes the view now (or the error) and on every change; returns the unsubscribe. */
  subscribe(address: string, cb: Listener, onError: (e: ZenSourceError) => void, garden?: string): () => void
  /** The last view K2 has for an address (no call). */
  cached(address: string): ZenSourceView | undefined
  /** Live per-server state (tests). */
  servers(): string[]
  dispose(): void
}

export function createZenSourcesReader(deps: ZenSourcesDeps): ZenSourcesReader {
  const servers = new Map<string, Server>()
  const gardens = new Map<string, Set<string>>()

  const resolve = (address: string, garden: string | undefined): { name: string; server: Server; address: string } => {
    const p = parseSourceAddress(address)
    if (!p) throw new ZenSourceError('failed', `'${address}' isn’t a source address (name or name::host).`)
    const key = p.host ?? 'local'
    if (garden && p.host) {
      const used = gardens.get(garden) ?? new Set<string>()
      if (!used.has(key) && used.size >= ZEN_SOURCE_SERVERS_PER_GARDEN) {
        throw new ZenSourceError('rate_limited', `A Garden reads sources on at most ${ZEN_SOURCE_SERVERS_PER_GARDEN} other servers.`)
      }
      used.add(key)
      gardens.set(garden, used)
    }
    let s = servers.get(key)
    if (!s) {
      let scope: ServerScope | null
      try {
        scope = deps.scopeFor(p.host)
      } catch (err) {
        const c = zenSourceErrorCode(err)
        throw new ZenSourceError(c.code === 'unreachable' ? 'server_not_saved' : c.code, c.message)
      }
      if (!scope) {
        throw new ZenSourceError('server_not_saved', `Add ${p.host} to your servers (Settings → Network → Connected Servers).`)
      }
      s = { key, scope, rev: 0, views: new Map(), listeners: new Map(), stop: null, timer: null, inFlight: false, down: false }
      servers.set(key, s)
    }
    return { name: p.name, server: s, address: p.host ? `${p.name}::${p.host}` : p.name }
  }

  const addressOf = (s: Server, name: string): string => (s.key === 'local' ? name : `${name}::${s.key}`)

  const emit = (s: Server, name: string, v: ZenSourceView): void => {
    for (const cb of [...(s.listeners.get(name) ?? [])]) {
      try {
        cb(v)
      } catch (err) {
        console.warn(`[zen] sources listener for ${v.address} failed:`, err)
      }
    }
  }

  /** A failed call → the error, with the last view when there is one. */
  const failure = (s: Server, name: string, err: unknown): ZenSourceError => {
    const c = zenSourceErrorCode(err)
    const last = s.views.get(name)
    if ((c.code === 'unreachable' || c.code === 'removed') && last) {
      const stale = zenStaleView(last, c.code)
      return new ZenSourceError(c.code, c.message, { last: stale })
    }
    return new ZenSourceError(c.code, c.message, { role: c.role })
  }

  const store = (s: Server, r: SourceResult): ZenSourceView => {
    const v = zenSourceView(addressOf(s, r.name), r)
    s.views.set(r.name, v)
    return v
  }

  const pull = async (s: Server): Promise<void> => {
    if (s.inFlight) return
    s.inFlight = true
    try {
      const ch = await deps.changes(s.scope, s.rev)
      s.rev = Math.max(s.rev, ch.rev)
      s.down = false
      for (const r of ch.changed) {
        const v = store(s, r)
        emit(s, r.name, v)
      }
      for (const name of ch.removed) {
        const last = s.views.get(name)
        if (last) emit(s, name, zenStaleView(last, 'removed'))
      }
    } catch (err) {
      const c = zenSourceErrorCode(err)
      if (c.code === 'unreachable' && !s.down) {
        s.down = true
        for (const [name, last] of s.views) emit(s, name, zenStaleView(last, 'unreachable'))
      }
    } finally {
      s.inFlight = false
    }
  }

  const live = (s: Server): void => {
    if (s.stop || s.timer) return
    const off = deps.onChanged(s.scope, (rev) => {
      if (rev > s.rev) void pull(s)
    })
    if (off) {
      s.stop = off
      return
    }
    s.timer = deps.setInterval(() => {
      if (deps.visible()) void pull(s)
    }, ZEN_SOURCES_POLL_MS)
  }

  const idle = (s: Server): void => {
    if ([...s.listeners.values()].some((set) => set.size > 0)) return
    s.stop?.()
    s.stop = null
    if (s.timer !== null) deps.clearInterval(s.timer)
    s.timer = null
  }

  const read = async (address: string, garden?: string): Promise<ZenSourceView> => {
    const { name, server } = resolve(address, garden)
    try {
      return store(server, await deps.read(server.scope, name))
    } catch (err) {
      throw failure(server, name, err)
    }
  }

  return {
    read,
    async history(address, field, opts = {}, garden) {
      const { name, server } = resolve(address, garden)
      try {
        return await deps.history(server.scope, name, field, opts)
      } catch (err) {
        throw failure(server, name, err)
      }
    },
    async refresh(address, garden) {
      const { name, server } = resolve(address, garden)
      try {
        return await deps.refresh(server.scope, name)
      } catch (err) {
        throw failure(server, name, err)
      }
    },
    async list(addresses, garden) {
      return Promise.all(
        addresses.map(async (a): Promise<ZenSourceListRow> => {
          try {
            const v = await read(a, garden)
            return { address: v.address, label: v.label, health: v.health, phase: v.phase, display: v.display, reach: 'ok' }
          } catch (err) {
            const e = err instanceof ZenSourceError ? err : new ZenSourceError('failed', String(err))
            const reach: ZenSourceReach =
              e.code === 'server_not_saved' || e.code === 'unreachable' || e.code === 'cap_not_granted' || e.code === 'removed'
                ? e.code
                : 'unreachable'
            const last = e.last
            return {
              address: canonicalSourceAddress(a) ?? a,
              label: last?.label ?? null,
              health: last?.health ?? null,
              phase: last ? 'stale' : null,
              display: last ? last.display : null,
              reach,
            }
          }
        }),
      )
    },
    subscribe(address, cb, onError, garden) {
      const { name, server } = resolve(address, garden)
      const set = server.listeners.get(name) ?? new Set<Listener>()
      server.listeners.set(name, set)
      set.add(cb)
      live(server)
      let gone = false
      void (async () => {
        try {
          const v = store(server, await deps.read(server.scope, name))
          if (server.rev === 0) server.rev = Math.max(0, ...[...server.views.values()].map((x) => x.rev))
          if (!gone) cb(v)
        } catch (err) {
          if (gone) return
          const e = failure(server, name, err)
          if (e.last) cb(e.last)
          else onError(e)
        }
      })()
      return () => {
        gone = true
        set.delete(cb)
        idle(server)
      }
    },
    cached(address) {
      const p = parseSourceAddress(address)
      if (!p) return undefined
      return servers.get(p.host ?? 'local')?.views.get(p.name)
    },
    servers: () => [...servers.keys()],
    dispose() {
      for (const s of servers.values()) {
        s.listeners.clear()
        idle(s)
      }
      servers.clear()
      gardens.clear()
    },
  }
}
