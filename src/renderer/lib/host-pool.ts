// Home M2 — the per-server connection pool (prd-home-multi-server-client-v1
// MS23, MS25–MS29, MS31, MS32, MS35–MS37; vs-live MS59–MS62, MS70, MS71).
//
// One pool per webview, one entry per server (host key) that a Home row (and
// later a room) uses. The window's own server keeps today's paths — the
// ConnectionGate, `daemonCli*` with `primaryScope()`, `reviveRemoteSession`
// and its full-screen sign-in. The pool still keeps an entry for it, for
// version and `instanceId` (the "same server as …" note, MS81).
//
// An entry knows:
//   - reach: live / starting / offline, from that server's PUBLIC
//     `/boot-status` (MS29), plus version, protocol and `instanceId`. A new
//     `instanceId` means the server restarted: only that server's
//     `onRestart` listeners run;
//   - auth: ok / signing-in / signin-required / rotate-required / kicked
//     (MS27). Background failures only ever change this in-memory state:
//     the pool never deletes a keychain token, the `k2` CLI token mirror or
//     a remembered password, and never raises the full-screen overlay
//     (MS36, MS59);
//   - role: from that server's `GET /cli/auth/whoami` (MS83: below Member, or
//     a role this app does not know, is "No access");
//   - presence: `GET /cli/presence/summary` with that server's own login.
//
// Requests: `get/post/getText` are `daemonCli*` on `scopeForHost(key)`, so
// they share the per-server cap of 4 and the one quick `[0]` retry, and a
// refused login runs ONE revive through `revive()` and ONE replay. The
// status check's own requests take the same per-server slots
// (`withHostCliSlot`). Dials go through the per-server queue under the
// window-wide cap (`lib/grid-dial-queue.ts`, MS70).
//
// Automatic logins go through `lib/host-login-coord.ts`: one window holds
// the lease and posts, the others show "Signing in…" and pick the token up
// from the `k2:host-session` broadcast (MS28); budgets and blocks keep the
// client under every login limit (MS31) and stop it after a kick (MS37,
// MS71).

import { createStore, type StoreApi } from 'zustand/vanilla'
import type { ConnectHost, LoginResult } from '@/stores/connect-host'
import { LOCAL_HOME_HOST, savedHostForKey } from '@/lib/host-key'
import type { LoginCoordinator, LoginBlock } from '@/lib/host-login-coord'
import type { PresenceWorkspace } from '@/lib/home-status'

export type PoolAuth = 'ok' | 'signing-in' | 'signin-required' | 'rotate-required' | 'kicked'
export type PoolReach = 'unknown' | 'live' | 'starting' | 'offline'

export interface PoolBoot {
  phase: string | null
  ready: boolean
  version: string | null
  protocol: number | null
  instanceId: string | null
  at: number
}

export interface HostEntry {
  hostKey: string
  /** False when the server is not in the saved list (and not `local`). */
  saved: boolean
  /** The saved entry's id the last check resolved (MS61). */
  hostId: string | null
  reach: PoolReach
  boot: PoolBoot | null
  auth: PoolAuth
  /** Extra copy for the auth state (a throttle, a refused password). */
  authNote: string | null
  /** The login's role on that server, as whoami says it. Null = not read. */
  role: string | null
  presence: PresenceWorkspace[] | null
  /** Offline checks in a row, for the 5 s / 15 s / 30 s probe backoff. */
  offlineStreak: number
  checkedAt: number | null
}

/** `/boot-status` body fields the pool reads. */
export interface BootBody {
  phase?: unknown
  ready?: unknown
  version?: unknown
  protocol?: unknown
  instanceId?: unknown
}

export type ReviveResult =
  | 'revived'
  | 'still-valid'
  | 'waiting'
  | 'signin-required'
  | 'rotate-required'
  | 'kicked'
  | 'unreachable'
  | 'not-applicable'

export interface HostPoolDeps {
  /** The saved servers right now (fresh tokens). */
  hosts: () => ConnectHost[]
  /** The window's own server. The pool never revives it. */
  windowHostKey: () => string
  /** `{base, token}` for this computer's daemon. */
  localCreds: () => Promise<{ base: string; token: string }>
  /** One HTTP request to `hostKey` under its `/cli` cap, with one quick
   *  retry on a connection-level error. Throws on a network failure. */
  http: (hostKey: string, url: string, init?: RequestInit) => Promise<Response>
  /** `GET <base>/boot-status`, or null when unreachable. */
  bootStatus: (hostKey: string, base: string) => Promise<BootBody | null>
  resolvePassword: (hostId: string) => Promise<string | null>
  /** `loginToHost`: commits the token to the store and the keychain, and
   *  fires the login-landed broadcast. */
  login: (host: ConnectHost, password: string) => Promise<LoginResult>
  /** MS36/MS59: forget a dead token in MEMORY only. */
  dropSessionInMemory: (hostId: string) => void
  coord: LoginCoordinator
  noteVersion: (hostKey: string, version: string | null) => void
  now: () => number
}

/** Copy for a row or room in each sign-in state (MS27, MS45). */
export function authNoteFor(block: LoginBlock | null, serverLabel: string): string | null {
  if (!block) return null
  switch (block.reason) {
    case 'kicked':
      return `Removed from ${serverLabel}. Sign in again.`
    case 'refused':
      return `${serverLabel} refused the saved password. Sign in again.`
    case 'signed-out':
      return `Signed out of ${serverLabel}.`
    case 'throttled': {
      if (block.until === null) return 'Too many sign-ins from this network. Try again in a few minutes.'
      return null
    }
  }
}

/** Roles that may open an agent on another server (MS83). Anything else —
 *  Viewer, or a role this app does not know — is "No access". */
export function roleAllowsRoom(role: string | null): boolean {
  if (role === null) return true
  return role === 'owner' || role === 'admin' || role === 'member'
}

/** The probe cadence for one entry (MS26): offline 5 s, 15 s, 30 s, then
 *  every 30 s; anything else 30 s. */
export function nextCheckDelayMs(entry: Pick<HostEntry, 'reach' | 'offlineStreak'> | undefined): number {
  if (!entry || entry.reach !== 'offline') return 30_000
  if (entry.offlineStreak <= 1) return 5_000
  if (entry.offlineStreak === 2) return 15_000
  return 30_000
}

/** MS81 / answer Q2(a): host keys that report the same `instanceId` as an
 *  earlier key are the same daemon reached by another address. Returns
 *  `hostKey → the earlier hostKey` for each later one. "Earlier" is the
 *  order of `keys` (Home row order). */
export function sameServerPairs(
  keys: string[],
  entries: Record<string, Pick<HostEntry, 'boot'> | undefined>,
): Record<string, string> {
  const firstByInstance = new Map<string, string>()
  const out: Record<string, string> = {}
  for (const key of keys) {
    const id = entries[key]?.boot?.instanceId
    if (!id) continue
    const first = firstByInstance.get(id)
    if (first === undefined) firstByInstance.set(id, key)
    else if (first !== key) out[key] = first
  }
  return out
}

/** A WebSocket close code from a daemon (MS71 / MS44 a). */
export const WS_CLOSE_KICKED = 4001
export const WS_CLOSE_SESSION_REVOKED = 4003
/** A browser's code for a close frame with no status: what a daemon from
 *  before the kick code sends. */
const WS_CLOSE_NO_STATUS = 1005
const KICK_EVIDENCE_WINDOW_MS = 5_000

export interface HostPool {
  readonly store: StoreApi<{ entries: Record<string, HostEntry> }>
  entry(hostKey: string): HostEntry | undefined
  /** The status check: `/boot-status`, then whoami (role) and the presence
   *  summary with that server's own login. `bootOnly` skips the login half
   *  (the window's own server). Never throws. Single-flight per server. */
  check(hostKey: string, opts?: { bootOnly?: boolean }): Promise<HostEntry>
  /** One revive for a server whose login was refused (MS10). */
  revive(hostKey: string): Promise<ReviveResult>
  /** A daemon socket for this server closed with `code` (MS71). */
  noteSocketClose(hostKey: string, code: number): void
  /** Another window's login landed and this window re-read the token. */
  noteSessionRefreshed(hostKey: string): void
  /** The user signed out of this server (here or in another window). */
  noteSignedOut(hostKey: string): void
  /** A server left the saved list or changed host key: drop its entry. */
  forget(hostKey: string): void
  onRestart(fn: (hostKey: string, prevInstanceId: string, nextInstanceId: string) => void): () => void
  onAuth(fn: (hostKey: string, auth: PoolAuth) => void): () => void
}

function blankEntry(hostKey: string): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: null,
    reach: 'unknown',
    boot: null,
    auth: 'signin-required',
    authNote: null,
    role: null,
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
  }
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null
}

function parseBoot(b: BootBody, at: number): PoolBoot {
  return {
    phase: str(b.phase),
    ready: b.phase === 'ready' || b.ready === true,
    version: str(b.version),
    protocol: typeof b.protocol === 'number' ? b.protocol : null,
    instanceId: str(b.instanceId),
    at,
  }
}

function withToken(base: string, path: string, token: string): string {
  const sep = path.includes('?') ? '&' : '?'
  return `${base}${path}${sep}token=${encodeURIComponent(token)}`
}

function baseOf(h: Pick<ConnectHost, 'hostname' | 'port' | 'secure'>): string {
  const scheme = h.secure ? 'https' : 'http'
  const authority = h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
  return `${scheme}://${authority}`
}

export function createHostPool(deps: HostPoolDeps): HostPool {
  const store = createStore<{ entries: Record<string, HostEntry> }>(() => ({ entries: {} }))
  const checking = new Map<string, Promise<HostEntry>>()
  const reviving = new Map<string, Promise<ReviveResult>>()
  const closes = new Map<string, Array<{ code: number; at: number }>>()
  const restartListeners = new Set<(k: string, a: string, b: string) => void>()
  const authListeners = new Set<(k: string, a: PoolAuth) => void>()

  const read = (hostKey: string): HostEntry => store.getState().entries[hostKey] ?? blankEntry(hostKey)

  const write = (hostKey: string, patch: Partial<HostEntry>): HostEntry => {
    const prev = read(hostKey)
    const next: HostEntry = { ...prev, ...patch, hostKey }
    store.setState((s) => ({ entries: { ...s.entries, [hostKey]: next } }))
    if (prev.auth !== next.auth) {
      for (const fn of [...authListeners]) fn(hostKey, next.auth)
    }
    return next
  }

  const labelOf = (hostKey: string): string => {
    if (hostKey === LOCAL_HOME_HOST) return 'This computer'
    const saved = savedHostForKey(deps.hosts(), hostKey)
    return saved ? saved.label || saved.hostname : hostKey
  }

  /** The state a server shows when we hold no live login for it. */
  const signedOutState = (hostKey: string): Partial<HostEntry> => {
    const block = deps.coord.block(hostKey)
    if (block?.reason === 'kicked') {
      return { auth: 'kicked', authNote: authNoteFor(block, labelOf(hostKey)), presence: null }
    }
    if (!block && deps.coord.leaseHeldElsewhere(hostKey)) {
      return { auth: 'signing-in', authNote: null, presence: null }
    }
    return { auth: 'signin-required', authNote: authNoteFor(block, labelOf(hostKey)), presence: null }
  }

  /** MS71: was this login removed from the server by its owner? A 4001
   *  close is exact. A daemon from before the code closes with no status;
   *  then a close in the last 5 s plus a refused whoami counts as a kick. */
  const kickEvidence = (hostKey: string): boolean => {
    const recent = (closes.get(hostKey) ?? []).filter((c) => deps.now() - c.at <= KICK_EVIDENCE_WINDOW_MS)
    if (recent.some((c) => c.code === WS_CLOSE_KICKED)) return true
    if (recent.some((c) => c.code === WS_CLOSE_SESSION_REVOKED)) return false
    return recent.some((c) => c.code === WS_CLOSE_NO_STATUS)
  }

  /** Whoami with the current token: 'alive' + role, 'dead', or 'unknown'. */
  const whoami = async (
    hostKey: string,
    base: string,
    token: string,
  ): Promise<{ kind: 'alive'; role: string | null; mustChange: boolean } | { kind: 'dead' } | { kind: 'unknown' }> => {
    let res: Response
    try {
      res = await deps.http(hostKey, withToken(base, '/cli/auth/whoami', token), { method: 'GET' })
    } catch {
      return { kind: 'unknown' }
    }
    if (res.status === 401 || res.status === 403) return { kind: 'dead' }
    if (!res.ok) return { kind: 'unknown' }
    let body: { role?: unknown; mustChangePassword?: unknown }
    try {
      body = (await res.json()) as { role?: unknown; mustChangePassword?: unknown }
    } catch {
      return { kind: 'alive', role: null, mustChange: false }
    }
    return { kind: 'alive', role: str(body.role), mustChange: body.mustChangePassword === true }
  }

  /** The automatic-login half of a revive (also used when no token is held
   *  but a password is remembered). */
  const autoLogin = async (hostKey: string, saved: ConnectHost): Promise<ReviveResult> => {
    const label = labelOf(hostKey)
    const block = deps.coord.block(hostKey)
    if (block) {
      write(hostKey, signedOutState(hostKey))
      return block.reason === 'kicked' ? 'kicked' : 'signin-required'
    }
    if (!saved.username) {
      write(hostKey, { auth: 'signin-required', authNote: null, presence: null })
      return 'signin-required'
    }
    const password = await deps.resolvePassword(saved.id)
    if (!password) {
      write(hostKey, { auth: 'signin-required', authNote: null, presence: null })
      return 'signin-required'
    }
    const begin = await deps.coord.tryBegin(hostKey)
    if (!begin.ok) {
      if (begin.why === 'lease-held') {
        write(hostKey, { auth: 'signing-in', authNote: null })
        return 'waiting'
      }
      if (begin.why === 'blocked') {
        write(hostKey, signedOutState(hostKey))
        return begin.block.reason === 'kicked' ? 'kicked' : 'signin-required'
      }
      // Out of automatic budget: the user's click still works.
      write(hostKey, { auth: 'signin-required', authNote: null, presence: null })
      return 'signin-required'
    }
    write(hostKey, { auth: 'signing-in', authNote: null })
    let result: LoginResult
    try {
      result = await deps.login(saved, password)
    } finally {
      deps.coord.end(hostKey)
    }
    if (result.ok) {
      if (result.mustChangePassword) {
        write(hostKey, { auth: 'rotate-required', authNote: `${label} needs a new password.`, presence: null })
        return 'rotate-required'
      }
      write(hostKey, { auth: 'ok', authNote: null })
      return 'revived'
    }
    if (result.kind === 'auth' || result.kind === 'not-found') {
      // Never a second automatic wrong attempt (MS31).
      deps.coord.setBlock(hostKey, 'refused')
      deps.dropSessionInMemory(saved.id)
      write(hostKey, signedOutState(hostKey))
      return 'signin-required'
    }
    if (result.kind === 'throttled') {
      const waitMs = (result.retryAfterSec ?? 300) * 1000
      deps.coord.setBlock(hostKey, 'throttled', deps.now() + waitMs)
      write(hostKey, { auth: 'signin-required', authNote: result.reason, presence: null })
      return 'signin-required'
    }
    write(hostKey, { auth: 'signin-required', authNote: null })
    return 'unreachable'
  }

  const doRevive = async (hostKey: string): Promise<ReviveResult> => {
    if (hostKey === LOCAL_HOME_HOST || hostKey === deps.windowHostKey()) return 'not-applicable'
    const saved = savedHostForKey(deps.hosts(), hostKey)
    if (!saved) {
      write(hostKey, { saved: false, auth: 'signin-required', presence: null })
      return 'signin-required'
    }
    const base = baseOf(saved)
    if (saved.token.length > 0) {
      const who = await whoami(hostKey, base, saved.token)
      if (who.kind === 'unknown') return 'unreachable'
      if (who.kind === 'alive') {
        write(hostKey, {
          auth: who.mustChange ? 'rotate-required' : 'ok',
          authNote: who.mustChange ? `${labelOf(hostKey)} needs a new password.` : null,
          role: who.role,
        })
        return who.mustChange ? 'rotate-required' : 'still-valid'
      }
      // Dead. Was it a kick?
      if (kickEvidence(hostKey)) {
        deps.coord.setBlock(hostKey, 'kicked')
        deps.dropSessionInMemory(saved.id)
        write(hostKey, signedOutState(hostKey))
        return 'kicked'
      }
      deps.dropSessionInMemory(saved.id)
    }
    return autoLogin(hostKey, savedHostForKey(deps.hosts(), hostKey) ?? saved)
  }

  const revive = (hostKey: string): Promise<ReviveResult> => {
    const existing = reviving.get(hostKey)
    if (existing) return existing
    const p = doRevive(hostKey).finally(() => {
      reviving.delete(hostKey)
    })
    reviving.set(hostKey, p)
    return p
  }

  const loginHalf = async (hostKey: string, base: string, token: string, isLocal: boolean): Promise<void> => {
    const onDead = async (): Promise<void> => {
      if (isLocal) {
        write(hostKey, { auth: 'signin-required', presence: null })
        return
      }
      const outcome = await revive(hostKey)
      if (outcome !== 'revived') return
      const fresh = savedHostForKey(deps.hosts(), hostKey)
      if (!fresh || fresh.token.length === 0) return
      return loginHalf(hostKey, base, fresh.token, false)
    }
    // whoami: is the login alive, and its role there (MS83). An older or
    // flaky server that does not answer it still gets the summary read.
    const who = await whoami(hostKey, base, token)
    if (who.kind === 'dead') return onDead()
    if (who.kind === 'alive' && who.mustChange) {
      write(hostKey, {
        auth: 'rotate-required',
        authNote: `${labelOf(hostKey)} needs a new password.`,
        role: who.role,
        presence: null,
      })
      return
    }
    const role = who.kind === 'alive' ? who.role : read(hostKey).role
    let res: Response
    try {
      res = await deps.http(hostKey, withToken(base, '/cli/presence/summary', token), { method: 'GET' })
    } catch {
      // Network miss on the summary only: the login stands if whoami said so.
      write(hostKey, who.kind === 'alive' ? { auth: 'ok', authNote: null, role, presence: null } : { presence: null })
      return
    }
    if (res.status === 401 || res.status === 403) return onDead()
    write(hostKey, { auth: 'ok', authNote: null, role })
    if (!res.ok) {
      // 404 = a server older than the summary route: live, no presence.
      write(hostKey, { presence: null })
      return
    }
    let body: { workspaces?: unknown }
    try {
      body = (await res.json()) as { workspaces?: unknown }
    } catch {
      write(hostKey, { presence: null })
      return
    }
    write(hostKey, {
      presence: Array.isArray(body.workspaces) ? (body.workspaces as PresenceWorkspace[]) : null,
    })
  }

  const doCheck = async (hostKey: string, bootOnly: boolean): Promise<HostEntry> => {
    const isLocal = hostKey === LOCAL_HOME_HOST
    let base: string
    let token: string
    let saved: ConnectHost | null = null
    if (isLocal) {
      try {
        const c = await deps.localCreds()
        base = c.base
        token = c.token
      } catch {
        const prev = read(hostKey)
        return write(hostKey, { saved: true, reach: 'offline', offlineStreak: prev.offlineStreak + 1, checkedAt: deps.now() })
      }
    } else {
      saved = savedHostForKey(deps.hosts(), hostKey)
      if (!saved) return write(hostKey, { saved: false, hostId: null, reach: 'unknown', presence: null })
      base = baseOf(saved)
      token = saved.token
      const prev = read(hostKey)
      // MS61: the key now resolves to another saved entry (a re-add, or a
      // duplicate getting the login). Treat it as a new login: forget the
      // old role and auth before reading them again.
      if (prev.hostId !== null && prev.hostId !== saved.id) {
        write(hostKey, { hostId: saved.id, role: null, presence: null, ...signedOutState(hostKey) })
      } else if (prev.hostId === null) {
        write(hostKey, { hostId: saved.id, saved: true })
      }
    }

    const body = await deps.bootStatus(hostKey, base)
    const at = deps.now()
    if (!body) {
      const prev = read(hostKey)
      return write(hostKey, { reach: 'offline', offlineStreak: prev.offlineStreak + 1, checkedAt: at })
    }
    const boot = parseBoot(body, at)
    const prevBoot = read(hostKey).boot
    deps.noteVersion(hostKey, boot.version)
    write(hostKey, { boot, reach: boot.ready ? 'live' : 'starting', offlineStreak: 0, checkedAt: at })
    // MS29: a new instanceId means this server restarted. Only its own
    // listeners hear about it.
    if (prevBoot?.instanceId && boot.instanceId && prevBoot.instanceId !== boot.instanceId) {
      for (const fn of [...restartListeners]) fn(hostKey, prevBoot.instanceId, boot.instanceId)
    }
    if (bootOnly || !boot.ready) return read(hostKey)

    if (isLocal) {
      await loginHalf(hostKey, base, token, true)
      return read(hostKey)
    }
    if (token.length === 0) {
      // No login held. A remembered password may sign in on its own, under
      // the app-wide lease and budgets; otherwise the row says Sign in.
      if (saved) {
        const outcome = await autoLogin(hostKey, saved)
        if (outcome === 'revived') {
          const fresh = savedHostForKey(deps.hosts(), hostKey)
          if (fresh && fresh.token.length > 0) await loginHalf(hostKey, base, fresh.token, false)
        }
      }
      return read(hostKey)
    }
    await loginHalf(hostKey, base, token, false)
    return read(hostKey)
  }

  return {
    store,
    entry: (hostKey) => store.getState().entries[hostKey],
    check(hostKey, opts) {
      const existing = checking.get(hostKey)
      if (existing) return existing
      const p = doCheck(hostKey, opts?.bootOnly === true).finally(() => {
        checking.delete(hostKey)
      })
      checking.set(hostKey, p)
      return p
    },
    revive,
    noteSocketClose(hostKey, code) {
      const list = (closes.get(hostKey) ?? []).filter((c) => deps.now() - c.at <= KICK_EVIDENCE_WINDOW_MS)
      list.push({ code, at: deps.now() })
      closes.set(hostKey, list)
      if (code === WS_CLOSE_KICKED && hostKey !== LOCAL_HOME_HOST) {
        // Exact (MS44 a): stay signed out until a click, in every window.
        deps.coord.setBlock(hostKey, 'kicked')
        const saved = savedHostForKey(deps.hosts(), hostKey)
        if (saved && hostKey !== deps.windowHostKey()) deps.dropSessionInMemory(saved.id)
        write(hostKey, signedOutState(hostKey))
      }
    },
    noteSessionRefreshed(hostKey) {
      write(hostKey, { auth: 'ok', authNote: null })
    },
    noteSignedOut(hostKey) {
      write(hostKey, { ...signedOutState(hostKey), role: null })
    },
    forget(hostKey) {
      closes.delete(hostKey)
      store.setState((s) => {
        if (!(hostKey in s.entries)) return s
        const entries = { ...s.entries }
        delete entries[hostKey]
        return { entries }
      })
    },
    onRestart(fn) {
      restartListeners.add(fn)
      return () => {
        restartListeners.delete(fn)
      }
    },
    onAuth(fn) {
      authListeners.add(fn)
      return () => {
        authListeners.delete(fn)
      }
    },
  }
}
