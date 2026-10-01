// Home M2 — app-wide coordination of AUTOMATIC logins (MS28, MS31, MS37).
//
// Every window of this app shares one origin, so localStorage is the one
// place every window can see. It holds, per server (host key):
//
//   - a LEASE `k2.hostLogin.lease.<hostKey>` = {holder, exp}: only the
//     holder posts an automatic login; other windows show "Signing in…"
//     and wait for the `k2:host-session` broadcast (one login, not N);
//   - a BLOCK `k2.hostLogin.block.<hostKey>` = {reason, at, until?}:
//       kicked     — B's owner removed this login. No automatic login until
//                    the user signs in (MS37, plan decision 4).
//       refused    — B refused the remembered password. Never a second
//                    automatic wrong attempt, so the client can never cause
//                    the 3-wrong-password lockout (MS31).
//       signed-out — the user signed out of B here. Respect it.
//       throttled  — B or the edge answered 429. Wait for `until`.
//     A login that lands (by click or automatic) clears the block;
//   - a shared ATTEMPT log `k2.hostLogin.attempts`: at most 1 automatic login
//     per server per 5 min and 3 across all servers per 5 min, which leaves
//     room for the user's own clicks under the 5-per-IP limits (MS31).
//
// Clicks never go through this module: a user gesture always may sign in.
// Local storage writes from two windows can race. `tryBegin` writes the
// lease, lets other windows' writes settle, and re-reads it before posting.

export const AUTO_LOGIN_WINDOW_MS = 5 * 60_000
export const AUTO_LOGINS_PER_HOST = 1
export const AUTO_LOGINS_GLOBAL = 3
export const LOGIN_LEASE_MS = 30_000

const LEASE_PREFIX = 'k2.hostLogin.lease.'
const BLOCK_PREFIX = 'k2.hostLogin.block.'
const ATTEMPTS_KEY = 'k2.hostLogin.attempts'

export type LoginBlockReason = 'kicked' | 'refused' | 'signed-out' | 'throttled'

export interface LoginBlock {
  reason: LoginBlockReason
  at: number
  /** 'throttled' only: when automatic logins may resume. */
  until: number | null
}

export type BeginResult =
  | { ok: true }
  | { ok: false; why: 'blocked'; block: LoginBlock }
  | { ok: false; why: 'lease-held' }
  | { ok: false; why: 'host-budget' }
  | { ok: false; why: 'global-budget' }

export interface CoordStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

export interface LoginCoordinatorDeps {
  storage: CoordStorage | null
  now: () => number
  /** This window's identity for the lease. */
  windowId: string
  /** Let other windows' lease writes land before re-reading it. */
  settle: () => Promise<void>
}

export interface LoginCoordinator {
  block(hostKey: string): LoginBlock | null
  setBlock(hostKey: string, reason: LoginBlockReason, until?: number | null): void
  clearBlock(hostKey: string): void
  /** Is another window's lease on this server live? */
  leaseHeldElsewhere(hostKey: string): boolean
  /** Take the lease and spend budget for ONE automatic login, or say why
   *  not. On `ok`, call `end(hostKey)` when the login settles. */
  tryBegin(hostKey: string): Promise<BeginResult>
  end(hostKey: string): void
  /** Automatic login POSTs in the last 5 min (test seam and diagnostics). */
  recentAttempts(hostKey?: string): number
}

interface Lease {
  holder: string
  exp: number
}

interface Attempt {
  h: string
  at: number
}

function readJson<T>(storage: CoordStorage | null, key: string): T | null {
  if (!storage) return null
  const raw = storage.getItem(key)
  if (raw === null) return null
  try {
    return JSON.parse(raw) as T
  } catch {
    return null
  }
}

function isLease(v: unknown): v is Lease {
  return (
    typeof v === 'object' &&
    v !== null &&
    typeof (v as Lease).holder === 'string' &&
    typeof (v as Lease).exp === 'number'
  )
}

function isBlock(v: unknown): v is LoginBlock {
  if (typeof v !== 'object' || v === null) return false
  const b = v as LoginBlock
  return (
    (b.reason === 'kicked' || b.reason === 'refused' || b.reason === 'signed-out' || b.reason === 'throttled') &&
    typeof b.at === 'number' &&
    (b.until === null || typeof b.until === 'number')
  )
}

export function createLoginCoordinator(deps: LoginCoordinatorDeps): LoginCoordinator {
  const { storage, now, windowId } = deps

  const write = (key: string, value: unknown): void => {
    if (!storage) return
    storage.setItem(key, JSON.stringify(value))
  }

  const block = (hostKey: string): LoginBlock | null => {
    const b = readJson<unknown>(storage, BLOCK_PREFIX + hostKey)
    if (!isBlock(b)) return null
    if (b.reason === 'throttled' && b.until !== null && b.until <= now()) return null
    return b
  }

  const attempts = (): Attempt[] => {
    const raw = readJson<unknown>(storage, ATTEMPTS_KEY)
    if (!Array.isArray(raw)) return []
    const cutoff = now() - AUTO_LOGIN_WINDOW_MS
    return raw.filter(
      (a): a is Attempt =>
        typeof a === 'object' &&
        a !== null &&
        typeof (a as Attempt).h === 'string' &&
        typeof (a as Attempt).at === 'number' &&
        (a as Attempt).at > cutoff,
    )
  }

  const lease = (hostKey: string): Lease | null => {
    const l = readJson<unknown>(storage, LEASE_PREFIX + hostKey)
    if (!isLease(l)) return null
    if (l.exp <= now()) return null
    return l
  }

  return {
    block,
    setBlock(hostKey, reason, until = null) {
      write(BLOCK_PREFIX + hostKey, { reason, at: now(), until } satisfies LoginBlock)
    },
    clearBlock(hostKey) {
      if (storage) storage.removeItem(BLOCK_PREFIX + hostKey)
    },
    leaseHeldElsewhere(hostKey) {
      const l = lease(hostKey)
      return l !== null && l.holder !== windowId
    },
    async tryBegin(hostKey) {
      const b = block(hostKey)
      if (b) return { ok: false, why: 'blocked', block: b }
      const held = lease(hostKey)
      if (held && held.holder !== windowId) return { ok: false, why: 'lease-held' }
      const recent = attempts()
      if (recent.filter((a) => a.h === hostKey).length >= AUTO_LOGINS_PER_HOST) {
        return { ok: false, why: 'host-budget' }
      }
      if (recent.length >= AUTO_LOGINS_GLOBAL) return { ok: false, why: 'global-budget' }
      write(LEASE_PREFIX + hostKey, { holder: windowId, exp: now() + LOGIN_LEASE_MS } satisfies Lease)
      await deps.settle()
      // Another window may have written its lease in the same instant. The
      // last write wins; only its holder goes on.
      const after = lease(hostKey)
      if (!after || after.holder !== windowId) return { ok: false, why: 'lease-held' }
      write(ATTEMPTS_KEY, [...attempts(), { h: hostKey, at: now() } satisfies Attempt])
      return { ok: true }
    },
    end(hostKey) {
      const l = lease(hostKey)
      if (l && l.holder === windowId && storage) storage.removeItem(LEASE_PREFIX + hostKey)
    },
    recentAttempts(hostKey) {
      const all = attempts()
      return hostKey === undefined ? all.length : all.filter((a) => a.h === hostKey).length
    },
  }
}

function newWindowId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return `w-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`
}

/** This webview's identity for leases and broadcasts. */
export const WINDOW_INSTANCE_ID = newWindowId()

function realStorage(): CoordStorage | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage
  } catch {
    return null
  }
}

/** The app's coordinator, over the real localStorage. */
export const loginCoordinator: LoginCoordinator = createLoginCoordinator({
  storage: realStorage(),
  now: () => Date.now(),
  windowId: WINDOW_INSTANCE_ID,
  settle: () => new Promise((resolve) => setTimeout(resolve, 40 + Math.floor(Math.random() * 80))),
})
