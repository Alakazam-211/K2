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
import { bootViewOf, isPartialBootView } from '@/lib/boot-status-view'
import type { LoginCoordinator, LoginBlock } from '@/lib/host-login-coord'
import type { PresenceActivity, PresenceWorkspace } from '@/lib/home-status'
import type { ActivityDisplay } from '@/stores/session-events'
import { isRoleRequired, isVoiceChatOnly } from '@/lib/voice-chat-only'

export { isVoiceChatOnly }

const DISPLAYS: ReadonlySet<string> = new Set(['working', 'monitoring', 'waiting', 'unverifiable', 'idle'])

/** Home 0.43.2 (Z23): the summary's `agentActivity`, keeping only rows the
 *  client understands. Absent (a server before 0.43.2) → null. A server
 *  with daemon activity (prd-daemon-activity-and-thread-working-v1 A31)
 *  also sends the real `display` (`monitoring` rows say `working`,
 *  `unverifiable` ones `idle`, in `status`); it is kept when known. */
/** plan-agent-status-unify-v1 D2: `heartbeat` / `live` / `busy` / `counts`
 *  on a summary entry (0.46.0+), kept only when well-formed. */
function d2Fields(r: object): Pick<PresenceActivity, 'heartbeat' | 'live' | 'busy' | 'counts'> {
  const { heartbeat, live, busy, counts } = r as { heartbeat?: unknown; live?: unknown; busy?: unknown; counts?: unknown }
  const out: Pick<PresenceActivity, 'heartbeat' | 'live' | 'busy' | 'counts'> = {}
  if (typeof heartbeat === 'boolean') out.heartbeat = heartbeat
  if (typeof live === 'boolean') out.live = live
  if (typeof busy === 'boolean') out.busy = busy
  if (counts && typeof counts === 'object') {
    const c: Partial<Record<ActivityDisplay, number>> = {}
    for (const [k, v] of Object.entries(counts as Record<string, unknown>)) {
      if (DISPLAYS.has(k) && typeof v === 'number' && Number.isFinite(v) && v >= 0) c[k as ActivityDisplay] = v
    }
    out.counts = c
  }
  return out
}

type InboxFields = Pick<PresenceActivity, 'unread' | 'waiting' | 'muted' | 'ticketsWaiting' | 'ticketsWaitingMe'>

/** agent-unread-tickets-v1 (0.46.4): the caller's unread / waiting and the
 *  agent's ticket counts on a summary entry, kept only when well-formed. */
function inboxFields(r: object): InboxFields {
  const v = r as Record<string, unknown>
  const out: InboxFields = {}
  const count = (x: unknown): number | null =>
    typeof x === 'number' && Number.isFinite(x) && x >= 0 ? Math.floor(x) : null
  const unread = count(v.unread)
  const waiting = count(v.waiting)
  const tickets = count(v.ticketsWaiting)
  const mine = count(v.ticketsWaitingMe)
  if (unread !== null) out.unread = unread
  if (waiting !== null) out.waiting = waiting
  if (typeof v.muted === 'boolean') out.muted = v.muted
  if (tickets !== null) out.ticketsWaiting = tickets
  if (mine !== null) out.ticketsWaitingMe = mine
  return out
}

export function parsePresenceActivity(raw: unknown): PresenceActivity[] | null {
  if (!Array.isArray(raw)) return null
  const out: PresenceActivity[] = []
  for (const r of raw) {
    if (!r || typeof r !== 'object') continue
    const { workspaceId, status, display } = r as { workspaceId?: unknown; status?: unknown; display?: unknown }
    if (typeof workspaceId !== 'string' || workspaceId.length === 0) continue
    const real = typeof display === 'string' && DISPLAYS.has(display) ? (display as ActivityDisplay) : null
    if (real) {
      out.push({
        workspaceId,
        status: status === 'permission' ? 'permission' : 'working',
        display: real,
        ...d2Fields(r),
        ...inboxFields(r),
      })
      continue
    }
    if (status !== 'working' && status !== 'permission') continue
    out.push({ workspaceId, status })
  }
  return out
}

export type PoolAuth = 'ok' | 'signing-in' | 'signin-required' | 'rotate-required' | 'kicked'
export type PoolReach = 'unknown' | 'live' | 'starting' | 'offline'

export interface PoolBoot {
  phase: string | null
  ready: boolean
  version: string | null
  protocol: number | null
  instanceId: string | null
  /** `/boot-status` `features` (REPORTED_FEATURES); `[]` from an older daemon. */
  features: string[]
  /** 0.46.3+: which view the server answered (`null` = an older daemon,
   *  always full). A `public` view without a known version is "sign in
   *  to see", not "an older K2". */
  view?: 'public' | 'compat' | 'full' | null
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
  /** Home 0.43.2 (Z23): the summary's `agentActivity` — that server's busy
   *  workspaces (working / permission). Null (or absent) when unread or the
   *  server is older than 0.43.2; cleared whenever `presence` is. */
  activity?: PresenceActivity[] | null
  /** Home 0.43.2 (Z23): the summary's `activeProjectIds` — that server's
   *  canonical Active set. Same lifetime as `activity`. */
  activeProjectIds?: string[] | null
  /** Offline checks in a row, for the 5 s / 15 s / 30 s probe backoff. */
  offlineStreak: number
  checkedAt: number | null
}

/** `/boot-status` body fields the pool reads. */
export interface BootBody {
  view?: unknown
  phase?: unknown
  ready?: unknown
  version?: unknown
  protocol?: unknown
  instanceId?: unknown
  features?: unknown
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
  /** `GET <base>/boot-status`, or null when unreachable. `token` (non-empty
   *  for a signed-in remote) rides `?token=` so the server answers its full
   *  body (PRD boot-status-ghost). */
  bootStatus: (hostKey: string, base: string, token: string) => Promise<BootBody | null>
  resolvePassword: (hostId: string) => Promise<string | null>
  /** `loginToHost`: commits the token to the store and the keychain, and
   *  fires the login-landed broadcast. */
  login: (host: ConnectHost, password: string) => Promise<LoginResult>
  /** MS36/MS59: forget a dead token in MEMORY only. */
  dropSessionInMemory: (hostId: string) => void
  /** Resolves once the saved logins are in memory (the app: the keychain
   *  tokens, `hydrateFromDisk`). A check of another server waits for it, so
   *  it never reads "no login held" from a list the keychain hasn't filled
   *  yet. Omitted: the list is already complete. */
  loginsReady?: () => Promise<void>
  coord: LoginCoordinator
  /** What `/boot-status` said: the version, and its `features` list (an
   *  older daemon without one reports `[]`). */
  noteVersion: (hostKey: string, version: string | null, features: readonly string[]) => void
  /** `POST projects/activate {projectId}` on `hostKey`, with THAT server's
   *  scope and login (MS39). Throws on failure. */
  activate: (hostKey: string, projectId: string) => Promise<void>
  now: () => number
  /** Voice & Chat only (§8.1): every role a whoami read for a saved server,
   *  so the app can keep the host's non-secret hint. Omitted: nothing kept. */
  noteRole?: (hostKey: string, role: string | null) => void
}

/** MS39: at most one `projects/activate` per (server, project) per 10 min. */
export const KEEP_ALIVE_DEDUPE_MS = 10 * 60_000

/** What `keepRoomAlive` did: posted it, skipped it (sent in the last
 *  10 min, or one is in flight), or the post failed (logged; the next
 *  open, focus or hourly tick tries again). Never throws. */
export type KeepAliveResult = 'sent' | 'deduped' | 'failed'

/** Copy for a row or room in each sign-in state (MS27, MS45). */
export function authNoteFor(block: LoginBlock | null, serverLabel: string, now: number): string | null {
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
      const min = Math.max(1, Math.ceil((block.until - now) / 60_000))
      return `Too many sign-ins from this network. Try again in ${min} min.`
    }
  }
}

/** Roles that may open an agent on another server (MS83). Anything else —
 *  Viewer, Voice & Chat only, or a role this app does not know — is "No
 *  access". */
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
   *  (the window's own server). Never throws. Single-flight per server: a
   *  request shares the check in flight only when that check covers it (a
   *  full check started with the same saved token covers either kind);
   *  otherwise it runs right after it. */
  check(hostKey: string, opts?: { bootOnly?: boolean }): Promise<HostEntry>
  /** One revive for a server whose login was refused (MS10). */
  revive(hostKey: string): Promise<ReviveResult>
  /** Voice & Chat only (§7): a route answered 403 `role_required`, so this
   *  login's role there changed. Re-read whoami with the saved token and
   *  keep the role; never a revive, never a sign-in (the login is alive).
   *  Single-flight per server. Works for the window's own server too.
   *  Resolves to the role read, or null when whoami didn't answer one. */
  refreshRole(hostKey: string): Promise<string | null>
  /** A daemon socket for this server closed with `code` (MS71). */
  noteSocketClose(hostKey: string, code: number): void
  /** Another window's login landed and this window re-read the token. */
  noteSessionRefreshed(hostKey: string): void
  /** The user signed out of this server (here or in another window). */
  noteSignedOut(hostKey: string): void
  /** A server left the saved list or changed host key: drop its entry. */
  forget(hostKey: string): void
  /**
   * MS39 / GH#22: tell `hostKey`'s daemon that someone is watching
   * `projectId` there, so its reaper (need clock, 24 h default) never kills
   * a session a room shows. `POST projects/activate` on THAT server with
   * its own scope and login; deduped per (server, project) to once per
   * 10 min, single-flight, and independent of the window's own activate
   * dedupe (`stores/projects.ts`). A restart of that server (new
   * `instanceId`) clears its dedupe so the next call re-sends.
   */
  keepRoomAlive(hostKey: string, projectId: string): Promise<KeepAliveResult>
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
    features: Array.isArray(b.features) ? b.features.filter((f): f is string => typeof f === 'string') : [],
    view: bootViewOf(b) ?? null,
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
  /** The check in flight per server: what kind it is, and the saved token
   *  it started with ('' for none, and for this computer). */
  const checking = new Map<string, { p: Promise<HostEntry>; bootOnly: boolean; token: string }>()
  const tokenFor = (hostKey: string): string =>
    hostKey === LOCAL_HOME_HOST ? '' : savedHostForKey(deps.hosts(), hostKey)?.token ?? ''
  const reviving = new Map<string, Promise<ReviveResult>>()
  const closes = new Map<string, Array<{ code: number; at: number }>>()
  const restartListeners = new Set<(k: string, a: string, b: string) => void>()
  const authListeners = new Set<(k: string, a: PoolAuth) => void>()
  /** `hostKey\0projectId` → when its last activate landed. */
  const keepAliveSent = new Map<string, number>()
  const keepAliveInflight = new Map<string, Promise<KeepAliveResult>>()
  const keepAliveKey = (hostKey: string, projectId: string): string => `${hostKey}\u0000${projectId}`
  const clearKeepAlive = (hostKey: string): void => {
    for (const k of [...keepAliveSent.keys()]) {
      if (k.startsWith(`${hostKey}\u0000`)) keepAliveSent.delete(k)
    }
  }

  const read = (hostKey: string): HostEntry => store.getState().entries[hostKey] ?? blankEntry(hostKey)

  /** Servers whose login state was actually read (whoami, a login, a
   *  sign-out, a kick...). Until then an entry's `auth` is provisional. */
  const authRead = new Set<string>()

  /** `auth` for an entry whose login hasn't been read yet: this computer
   *  and a server we hold (or remember) a login for are being signed in,
   *  not signed out. "Sign in" waits for a check that says so. */
  const pendingAuth = (hostKey: string, saved: ConnectHost | null): PoolAuth =>
    hostKey === LOCAL_HOME_HOST || (saved !== null && (saved.token.length > 0 || saved.remember))
      ? 'signing-in'
      : 'signin-required'

  const write = (hostKey: string, patch: Partial<HostEntry>, opts?: { provisional?: boolean }): HostEntry => {
    if ('auth' in patch && opts?.provisional !== true) authRead.add(hostKey)
    const prev = read(hostKey)
    const next: HostEntry = { ...prev, ...patch, hostKey }
    // Home 0.43.2: the summary's activity fields live and die with
    // `presence` (every "no summary" write clears presence only).
    if (patch.presence === null) {
      if (!('activity' in patch)) next.activity = null
      if (!('activeProjectIds' in patch)) next.activeProjectIds = null
    }
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
      return { auth: 'kicked', authNote: authNoteFor(block, labelOf(hostKey), deps.now()), presence: null }
    }
    if (!block && deps.coord.leaseHeldElsewhere(hostKey)) {
      return { auth: 'signing-in', authNote: null, presence: null }
    }
    return { auth: 'signin-required', authNote: authNoteFor(block, labelOf(hostKey), deps.now()), presence: null }
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
    const role = str(body.role)
    if (hostKey !== LOCAL_HOME_HOST && deps.noteRole) deps.noteRole(hostKey, role)
    return { kind: 'alive', role, mustChange: body.mustChangePassword === true }
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

  const refreshing = new Map<string, Promise<string | null>>()
  const refreshRole = (hostKey: string): Promise<string | null> => {
    const existing = refreshing.get(hostKey)
    if (existing) return existing
    const p = (async (): Promise<string | null> => {
      const saved = hostKey === LOCAL_HOME_HOST ? null : savedHostForKey(deps.hosts(), hostKey)
      if (!saved || saved.token.length === 0) return null
      const who = await whoami(hostKey, baseOf(saved), saved.token)
      if (who.kind !== 'alive') return null
      write(hostKey, { role: who.role, ...(isVoiceChatOnly(who.role) ? { presence: null } : {}) })
      return who.role
    })().finally(() => {
      refreshing.delete(hostKey)
    })
    refreshing.set(hostKey, p)
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
    // §8.1: a Voice & Chat only login reads no presence (the daemon answers
    // an empty compat body for older clients; a new client skips the call).
    if (isVoiceChatOnly(role)) {
      write(hostKey, { auth: 'ok', authNote: null, role, presence: null })
      return
    }
    let res: Response
    try {
      res = await deps.http(hostKey, withToken(base, '/cli/presence/summary', token), { method: 'GET' })
    } catch {
      // Network miss on the summary only: the login stands if whoami said so.
      write(hostKey, who.kind === 'alive' ? { auth: 'ok', authNote: null, role, presence: null } : { presence: null })
      return
    }
    if (res.status === 403) {
      // A role refusal is not a dead login (vs-live V3): re-read the role,
      // never revive.
      let text = ''
      try {
        text = await res.text()
      } catch {
        text = ''
      }
      if (isRoleRequired(res.status, text)) {
        const fresh = await refreshRole(hostKey)
        write(hostKey, { auth: 'ok', authNote: null, role: fresh ?? role, presence: null })
        return
      }
      return onDead()
    }
    if (res.status === 401) return onDead()
    write(hostKey, { auth: 'ok', authNote: null, role })
    if (!res.ok) {
      // 404 = a server older than the summary route: live, no presence.
      write(hostKey, { presence: null })
      return
    }
    let body: { workspaces?: unknown; agentActivity?: unknown; activeProjectIds?: unknown }
    try {
      body = (await res.json()) as { workspaces?: unknown; agentActivity?: unknown; activeProjectIds?: unknown }
    } catch {
      write(hostKey, { presence: null })
      return
    }
    write(hostKey, {
      presence: Array.isArray(body.workspaces) ? (body.workspaces as PresenceWorkspace[]) : null,
      activity: parsePresenceActivity(body.agentActivity),
      activeProjectIds: Array.isArray(body.activeProjectIds)
        ? body.activeProjectIds.filter((id): id is string => typeof id === 'string')
        : null,
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
      // A remembered login with no token in memory may only mean the
      // keychain hasn't answered yet (boot): wait for it before reading.
      const early = savedHostForKey(deps.hosts(), hostKey)
      if (deps.loginsReady && early && early.remember && early.token.length === 0) await deps.loginsReady()
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
        const pending = authRead.has(hostKey) ? {} : { auth: pendingAuth(hostKey, saved) }
        write(hostKey, { hostId: saved.id, saved: true, ...pending }, { provisional: true })
      }
    }

    // PRD boot-status-ghost: a remote server gets its own token so it
    // answers the full body (version + features) even when it hides them
    // from strangers. Local is loopback (always full): no token.
    const body = await deps.bootStatus(hostKey, base, isLocal ? '' : token)
    const at = deps.now()
    if (!body) {
      const prev = read(hostKey)
      return write(hostKey, { reach: 'offline', offlineStreak: prev.offlineStreak + 1, checkedAt: at })
    }
    const prevBoot = read(hostKey).boot
    // B3 no clobber: a stranger's view (public/compat) keeps the version
    // and features an earlier full body taught; it moves phase + instanceId.
    const partial = isPartialBootView(body)
    const parsed = parseBoot(body, at)
    const boot: PoolBoot = partial
      ? {
          ...parsed,
          version: parsed.version ?? prevBoot?.version ?? null,
          features: Array.isArray(body.features) ? parsed.features : prevBoot?.features ?? [],
        }
      : parsed
    if (!partial || parsed.version !== null) deps.noteVersion(hostKey, boot.version, boot.features)
    // A login nobody has read yet (a boot-only check, or before the login
    // half answers) reads as being signed in, never as "Sign in".
    const pending = authRead.has(hostKey) ? {} : { auth: pendingAuth(hostKey, saved) }
    write(
      hostKey,
      { boot, reach: boot.ready ? 'live' : 'starting', offlineStreak: 0, checkedAt: at, ...pending },
      { provisional: true },
    )
    // MS29: a new instanceId means this server restarted. Only its own
    // listeners hear about it.
    if (prevBoot?.instanceId && boot.instanceId && prevBoot.instanceId !== boot.instanceId) {
      // MS29: a restarted server re-hears every room's activate.
      clearKeepAlive(hostKey)
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
      const bootOnly = opts?.bootOnly === true
      const token = tokenFor(hostKey)
      const existing = checking.get(hostKey)
      // A check in flight answers this one only when it covers it: a full
      // check covers a boot-only one, and a full check covers another full
      // one when it started with the login held now. Otherwise this check
      // runs after it. (Voice's boot-only sweep at app start used to answer
      // Home's first full check, so every saved server read Sign in until
      // a login landed; a check begun before the keychain tokens arrived
      // did the same.)
      if (existing && (bootOnly || (!existing.bootOnly && existing.token === token))) return existing.p
      const run = (): Promise<HostEntry> => doCheck(hostKey, bootOnly)
      const p: Promise<HostEntry> = (existing ? existing.p.then(run, run) : run()).finally(() => {
        if (checking.get(hostKey)?.p === p) checking.delete(hostKey)
      })
      checking.set(hostKey, { p, bootOnly, token })
      return p
    },
    revive,
    refreshRole,
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
    keepRoomAlive(hostKey, projectId) {
      if (!projectId) return Promise.resolve('failed')
      const key = keepAliveKey(hostKey, projectId)
      const inflight = keepAliveInflight.get(key)
      if (inflight) return inflight.then((r) => (r === 'sent' ? 'deduped' : r))
      const last = keepAliveSent.get(key)
      if (last !== undefined && deps.now() - last < KEEP_ALIVE_DEDUPE_MS) return Promise.resolve('deduped')
      const p = deps
        .activate(hostKey, projectId)
        .then((): KeepAliveResult => {
          keepAliveSent.set(key, deps.now())
          return 'sent'
        })
        .catch((err: unknown): KeepAliveResult => {
          console.warn(`[host-pool] projects/activate on ${hostKey} failed:`, err)
          return 'failed'
        })
        .finally(() => {
          keepAliveInflight.delete(key)
        })
      keepAliveInflight.set(key, p)
      return p
    },
    forget(hostKey) {
      closes.delete(hostKey)
      authRead.delete(hostKey)
      clearKeepAlive(hostKey)
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
