// Unit tests for runtime session revival (lib/remote-session.ts) — the fix
// for the "stale remote-host credentials after a remote restart/update"
// dead end (only Cmd+Q recovered).
//
// Coverage:
//   - isPossibleAuthFailure: 401 always; 403 only with the daemon's
//     token-gate body; never for network-shaped/other errors.
//   - reviveRemoteSession:
//       * dead session + remembered password → re-login, token committed to
//         the store (host list AND activeHost), 'revived'
//       * whoami says alive → 'still-valid', NO login attempt
//       * no remembered password → token dropped + RemoteSignIn raised
//         (active host), 'signin-required'
//       * re-login 401 → 'signin-required' (never loops a rejected password)
//       * single-flight: concurrent callers share ONE whoami + ONE login
//       * backoff: a failed attempt gates the next (cooldown) until the
//         window elapses; `force` bypasses the gate but not the flight
//
// Follows connect-host.test.ts's harness: in-memory localStorage +
// mocked Tauri invoke (fake keychain), stubbed global fetch.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── In-memory localStorage stub (store module reads it at import) ───────
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// ── Mock the Tauri invoke bridge: fake keychain + hosts file ─────────────
const fakeKeychain = new Map<string, string>()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'connect_hosts_read':
        return '[]'
      case 'connect_hosts_write':
        return undefined
      case 'k2_secret_set':
        fakeKeychain.set(`${args!.service}:${args!.account}`, args!.secret as string)
        return undefined
      case 'k2_secret_get':
        return fakeKeychain.get(`${args!.service}:${args!.account}`) ?? null
      case 'k2_secret_delete':
        fakeKeychain.delete(`${args!.service}:${args!.account}`)
        return undefined
      default:
        throw new Error(`unexpected invoke: ${cmd}`)
    }
  }),
}))

import {
  isPossibleAuthFailure,
  reviveRemoteSession,
  reviveBackoffMs,
  REVIVE_BACKOFF_MS,
  __resetRemoteSessionForTests,
} from './remote-session'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  type ConnectHost,
} from '@/stores/connect-host'

function makeHost(overrides?: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'rpm',
    label: 'RPM box',
    hostname: 'rpm.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'stale-tok',
    remember: true,
    lastConnectedAt: null,
    ...overrides,
  }
}

/** Minimal Response stand-in for the whoami probe + loginToHost. */
function fakeRes(status: number, jsonBody?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (jsonBody === undefined ? '' : JSON.stringify(jsonBody)),
    json: async () => jsonBody,
  } as unknown as Response
}

/** Seed the store with `host` as a saved + ACTIVE remote. */
function seedActive(host: ConnectHost): void {
  useConnectHostStore.getState().addHost(host)
  useConnectHostStore.getState().selectHost(host)
}

/** Remember a password in the fake keychain the way rememberPassword does. */
function seedPassword(hostId: string, pw: string): void {
  fakeKeychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:${hostId}`, pw)
}

beforeEach(() => {
  mem.clear()
  fakeKeychain.clear()
  __resetConnectHostStoreForTests()
  __resetRemoteSessionForTests()
  vi.unstubAllGlobals()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
})

describe('isPossibleAuthFailure — auth vs everything else', () => {
  it('401 is auth regardless of body', () => {
    expect(isPossibleAuthFailure(401, '')).toBe(true)
    expect(isPossibleAuthFailure(401, 'anything')).toBe(true)
  })

  it("403 with the daemon's token-gate bodies is auth", () => {
    // /cli/* dispatch gate (cli_response.rs::forbidden)
    expect(isPossibleAuthFailure(403, '{"error":"Invalid or missing auth token"}')).toBe(true)
    // require_owner_or_admin / WS-upgrade gates
    expect(isPossibleAuthFailure(403, '{"error":"invalid or missing token"}')).toBe(true)
    // Already-extracted message (daemon-cli surfaces the error field verbatim)
    expect(isPossibleAuthFailure(403, 'Invalid or missing auth token')).toBe(true)
  })

  it('403 with any other body is NOT auth', () => {
    expect(isPossibleAuthFailure(403, '{"error":"peer not allowed"}')).toBe(false)
    expect(isPossibleAuthFailure(403, '')).toBe(false)
  })

  it('non-auth statuses are never auth', () => {
    expect(isPossibleAuthFailure(200, 'Invalid or missing auth token')).toBe(false)
    expect(isPossibleAuthFailure(400, 'Invalid or missing auth token')).toBe(false)
    expect(isPossibleAuthFailure(500, 'Invalid or missing auth token')).toBe(false)
  })
})

describe('reviveBackoffMs — the plaintext-login pacing rule', () => {
  it('first attempt is never gated; failures widen to the 30s cap', () => {
    expect(reviveBackoffMs(0)).toBe(0)
    expect(reviveBackoffMs(1)).toBe(REVIVE_BACKOFF_MS[0])
    expect(reviveBackoffMs(2)).toBe(REVIVE_BACKOFF_MS[1])
    expect(reviveBackoffMs(REVIVE_BACKOFF_MS.length)).toBe(30000)
    // Beyond the schedule it stays capped, never grows or wraps.
    expect(reviveBackoffMs(99)).toBe(30000)
  })
})

describe('reviveRemoteSession — dead session + remembered password', () => {
  it('whoami dead → login → fresh token committed to host list AND activeHost → revived', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')

    const fetchMock = vi.fn(async (url: string, _init?: RequestInit) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      if (url.includes('/cli/auth/login')) {
        return fakeRes(200, { token: 'fresh-tok', username: 'rosson', expiresAt: '2026-08-01T00:00:00+00:00' })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(reviveRemoteSession('rpm')).resolves.toBe('revived')

    // Fresh token visible to every getDaemonWs() consumer: activeHost swapped
    // synchronously + host list updated.
    const s = useConnectHostStore.getState()
    expect((s.activeHost as ConnectHost).token).toBe('fresh-tok')
    expect(s.hosts.find((h) => h.id === 'rpm')?.token).toBe('fresh-tok')
    // No sign-in raised — the revival was silent.
    expect(s.pendingSignIn).toBeNull()

    // The login body carried the credentials in the BODY (never the URL).
    const loginCall = fetchMock.mock.calls.find(([u]) => (u as string).includes('/cli/auth/login'))!
    expect(loginCall[0]).toBe('https://rpm.k2.dev/cli/auth/login')
    const init = loginCall[1] as RequestInit
    expect(JSON.parse(init.body as string)).toEqual({ username: 'rosson', password: 'hunter2' })
  })

  it('whoami alive → still-valid, no login attempt, token untouched', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')

    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(200, { username: 'rosson' })
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(reviveRemoteSession('rpm')).resolves.toBe('still-valid')
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('stale-tok')
    expect(fetchMock.mock.calls.every(([u]) => (u as string).includes('whoami'))).toBe(true)
  })

  it('whoami blip (timeout/5xx) → unreachable, token untouched, no login', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(502)))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('unreachable')
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('stale-tok')
  })

  it('unknown host id → not-applicable', async () => {
    await expect(reviveRemoteSession('nope')).resolves.toBe('not-applicable')
  })
})

describe('reviveRemoteSession — the cases that need the user', () => {
  it('dead session + NO remembered password → token dropped + sign-in raised', async () => {
    const host = makeHost({ remember: false })
    seedActive(host)
    // No seedPassword — the keychain has nothing for this host.
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      throw new Error(`unexpected fetch: ${url}`)
    }))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    const s = useConnectHostStore.getState()
    expect((s.activeHost as ConnectHost).token).toBe('')
    expect(s.pendingSignIn?.id).toBe('rpm')
  })

  it('remembered password REJECTED (login 401) → signin-required, not an endless retry', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'rotated-away')
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      if (url.includes('/cli/auth/login')) return fakeRes(401, { error: 'invalid username or password' })
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    const s = useConnectHostStore.getState()
    expect((s.activeHost as ConnectHost).token).toBe('')
    expect(s.pendingSignIn?.id).toBe('rpm')
    // Exactly one login attempt — a rejected password is never re-fired here.
    expect(fetchMock.mock.calls.filter(([u]) => (u as string).includes('login')).length).toBe(1)
  })

  it('legacy raw-token host (no username) → signin-required (no login flow to run)', async () => {
    const host = makeHost({ username: undefined })
    seedActive(host)
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      throw new Error(`unexpected fetch: ${url}`)
    }))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('')
  })

  it('a NON-active host clears its token WITHOUT hijacking the UI', async () => {
    const host = makeHost({ remember: false })
    useConnectHostStore.getState().addHost(host) // saved but NOT active
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      throw new Error(`unexpected fetch: ${url}`)
    }))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    const s = useConnectHostStore.getState()
    expect(s.activeHost).toBe('local')
    expect(s.pendingSignIn).toBeNull() // no overlay for a background tile
    expect(s.hosts.find((h) => h.id === 'rpm')?.token).toBe('')
  })
})

describe('reviveRemoteSession — the three-state recovery surface (active host)', () => {
  it("paints 'reauthenticating' while in flight and 'connected' on success", async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')

    let releaseWhoami!: () => void
    const whoamiGate = new Promise<void>((r) => { releaseWhoami = r })
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) {
        await whoamiGate
        return fakeRes(403, { error: 'Invalid or missing auth token' })
      }
      return fakeRes(200, { token: 'fresh-tok', username: 'rosson', expiresAt: 'x' })
    }))

    const p = reviveRemoteSession('rpm')
    // Mid-flight: state 2 of the owner contract.
    expect(useConnectHostStore.getState().recovery).toEqual({ kind: 'reauthenticating' })
    releaseWhoami()
    await expect(p).resolves.toBe('revived')
    // Auto-reconnect kept its promise: healthy baseline, no user action.
    expect(useConnectHostStore.getState().recovery).toEqual({ kind: 'connected' })
  })

  it("a failed re-login lands on 'signin-required' — the only user state", async () => {
    const host = makeHost({ remember: false })
    seedActive(host)
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      throw new Error(`unexpected fetch: ${url}`)
    }))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    expect(useConnectHostStore.getState().recovery).toEqual({ kind: 'signin-required' })
  })

  it("a NON-active host's revival never repaints the active indicator", async () => {
    const host = makeHost({ remember: false })
    useConnectHostStore.getState().addHost(host) // saved but NOT active
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      throw new Error(`unexpected fetch: ${url}`)
    }))

    await expect(reviveRemoteSession('rpm')).resolves.toBe('signin-required')
    expect(useConnectHostStore.getState().recovery).toEqual({ kind: 'connected' })
  })
})

describe('reviveRemoteSession — single-flight + backoff', () => {
  it('concurrent callers share ONE whoami and ONE login', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')

    // Deferred whoami so both calls arrive while the first is in flight.
    let releaseWhoami!: () => void
    const whoamiGate = new Promise<void>((r) => { releaseWhoami = r })
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) {
        await whoamiGate
        return fakeRes(403, { error: 'Invalid or missing auth token' })
      }
      if (url.includes('/cli/auth/login')) {
        return fakeRes(200, { token: 'fresh-tok', username: 'rosson', expiresAt: 'x' })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal('fetch', fetchMock)

    const a = reviveRemoteSession('rpm')
    const b = reviveRemoteSession('rpm')
    releaseWhoami()
    await expect(a).resolves.toBe('revived')
    await expect(b).resolves.toBe('revived')

    const whoamiCalls = fetchMock.mock.calls.filter(([u]) => (u as string).includes('whoami'))
    const loginCalls = fetchMock.mock.calls.filter(([u]) => (u as string).includes('login'))
    expect(whoamiCalls.length).toBe(1)
    expect(loginCalls.length).toBe(1)
  })

  it('a failed attempt gates the next call (cooldown); force bypasses it', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')
    // Everything unreachable-shaped: whoami 502 → 'unreachable' failure.
    const fetchMock = vi.fn(async () => fakeRes(502))
    vi.stubGlobal('fetch', fetchMock)

    await expect(reviveRemoteSession('rpm')).resolves.toBe('unreachable')
    const callsAfterFirst = fetchMock.mock.calls.length

    // Immediately again — inside the 1s window → cooldown, NO network.
    await expect(reviveRemoteSession('rpm')).resolves.toBe('cooldown')
    expect(fetchMock.mock.calls.length).toBe(callsAfterFirst)

    // Forced (the user's explicit reconnect gesture) → probes again.
    await expect(reviveRemoteSession('rpm', { force: true })).resolves.toBe('unreachable')
    expect(fetchMock.mock.calls.length).toBeGreaterThan(callsAfterFirst)
  })

  it('a success RESETS the failure backoff', async () => {
    const host = makeHost()
    seedActive(host)
    seedPassword('rpm', 'hunter2')

    // First attempt: unreachable (failure recorded).
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(502)))
    await expect(reviveRemoteSession('rpm')).resolves.toBe('unreachable')

    // Forced success clears the failure count…
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(403, { error: 'Invalid or missing auth token' })
      return fakeRes(200, { token: 'fresh-tok', username: 'rosson', expiresAt: 'x' })
    }))
    await expect(reviveRemoteSession('rpm', { force: true })).resolves.toBe('revived')

    // …so the next unforced call is NOT in cooldown.
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes(200, { username: 'rosson' })
      throw new Error(`unexpected fetch: ${url}`)
    }))
    await expect(reviveRemoteSession('rpm')).resolves.toBe('still-valid')
  })
})
