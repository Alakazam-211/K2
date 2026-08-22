// P2 (#632) — data-plane tests for the host-aware daemon-cli layer.
//
// daemon-cli.ts is the single renderer→daemon `/cli/*` chokepoint (Plan B,
// #622): every remote data read/write flows through daemonCliGet/Post,
// which resolve the ACTIVE host's creds at call time and fire `fetch`.
// daemon-ws.test.ts covers the URL HELPERS in isolation; this file covers
// the integration THROUGH daemon-cli — the part that was untested:
//   1. a call hits the active host's base URL and carries its token
//   2. the SAME call site follows the active host across a switch (the
//      "talked to the NEW host" property the host-switch reset relies on)
//   3. a 401 from the active REMOTE host expires its session (local 401
//      does not)
//   4. a connection-level fetch failure (ECONNREFUSED / "Load failed")
//      invalidates creds and retries once; an app-level non-2xx does NOT
//   5. secure/443 URL building end-to-end + `{"error":...}` surfacing
//
// We mock daemon-ws's getDaemonWs/invalidateDaemonWs (so creds are
// scripted) but keep the REAL daemonHttpBase (no URL-shape drift), use the
// REAL connect-host store (so expireSession wiring is exercised), and stub
// global `fetch`.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// localStorage stub so the connect-host store is happy in node.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// Scripted creds + a spy on cache invalidation, hoisted so the vi.mock
// factory can reference them safely.
const { getDaemonWsMock, invalidateDaemonWsMock } = vi.hoisted(() => ({
  getDaemonWsMock: vi.fn(),
  invalidateDaemonWsMock: vi.fn(),
}))

// Keep the REAL daemonHttpBase (pure URL builder) — only override the
// creds resolver + invalidation so we control them deterministically.
vi.mock('@/kessel/daemon-ws', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/kessel/daemon-ws')>()
  return {
    ...actual,
    getDaemonWs: (...a: unknown[]) => getDaemonWsMock(...a),
    invalidateDaemonWs: (...a: unknown[]) => invalidateDaemonWsMock(...a),
  }
})

// Tauri invoke is reached by connect-host (forgetToken on expireSession,
// persistHosts on addHost, resolvePassword during session revival). A fake
// keychain map lets the revival tests seed a remembered password.
const { fakeKeychain } = vi.hoisted(() => ({ fakeKeychain: new Map<string, string>() }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'k2_secret_get':
        return fakeKeychain.get(`${args!.service}:${args!.account}`) ?? null
      case 'k2_secret_set':
        fakeKeychain.set(`${args!.service}:${args!.account}`, args!.secret as string)
        return null
      case 'k2_secret_delete':
        fakeKeychain.delete(`${args!.service}:${args!.account}`)
        return null
      default:
        return null // connect_hosts_{read,write} etc. — inert in node
    }
  }),
}))

import { daemonCliGet, daemonCliGetText, daemonCliPost, RecoveringError } from './daemon-cli'
import * as connectionGateProbe from './connection-gate-probe'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  type ConnectHost,
} from '@/stores/connect-host'
import { __resetRemoteSessionForTests } from './remote-session'

const LOCAL_CREDS = { port: 47800, token: 'local-tok', host: '127.0.0.1', secure: false }
const SECURE_CREDS = { port: 443, token: 'remote-tok', host: 'rosson.k2.dev', secure: true }

/** A minimal Response stand-in covering exactly what daemon-cli reads. */
function fakeRes({
  status = 200,
  body = '',
  url = 'http://127.0.0.1:47800/cli/x',
}: {
  status?: number
  body?: string
  url?: string
} = {}): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    url,
    text: async () => body,
  } as unknown as Response
}

function makeRemoteHost(): ConnectHost {
  return {
    id: 'r1',
    label: 'Hosted',
    hostname: 'rosson.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'remote-tok',
    remember: false,
    lastConnectedAt: null,
  }
}

function lastFetchUrl(fetchMock: ReturnType<typeof vi.fn>): string {
  return fetchMock.mock.calls.at(-1)![0] as string
}

describe('daemonCliGet — hits the active host + carries its token', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  it('local: GETs http://127.0.0.1:<port>/cli/<route> with params + token', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => fakeRes({ body: JSON.stringify({ ok: 1 }) }))
    vi.stubGlobal('fetch', fetchMock)

    const out = await daemonCliGet('fs/read-dir', { path: '/x', n: 2 })
    expect(out).toEqual({ ok: 1 })

    const [url, opts] = fetchMock.mock.calls[0]
    expect(url as string).toContain('http://127.0.0.1:47800/cli/fs/read-dir?')
    expect(url as string).toContain('path=%2Fx')
    expect(url as string).toContain('n=2')
    expect(url as string).toContain('token=local-tok')
    expect(opts).toMatchObject({ method: 'GET' })
  })

  it('secure remote: GETs https://<host>/cli/<route> (443 omitted) with the remote token', async () => {
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    const fetchMock = vi.fn(async () => fakeRes({ body: JSON.stringify([]) }))
    vi.stubGlobal('fetch', fetchMock)

    await daemonCliGet('projects/list')

    const url = lastFetchUrl(fetchMock)
    expect(url).toContain('https://rosson.k2.dev/cli/projects/list?')
    expect(url).toContain('token=remote-tok')
    expect(url).not.toContain(':443')
    expect(url).not.toContain('local-tok')
  })

  it('the SAME call site follows the active host across a switch', async () => {
    // local → one URL; flip creds to the remote → the next identical call
    // targets the new host. This is the "talked to the NEW host" property
    // the #625 host-switch re-fetch wiring depends on.
    const fetchMock = vi.fn(async () => fakeRes({ body: '[]' }))
    vi.stubGlobal('fetch', fetchMock)

    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    await daemonCliGet('projects/list')
    expect(lastFetchUrl(fetchMock)).toContain('http://127.0.0.1:47800/cli/projects/list')
    expect(lastFetchUrl(fetchMock)).toContain('token=local-tok')

    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    await daemonCliGet('projects/list')
    expect(lastFetchUrl(fetchMock)).toContain('https://rosson.k2.dev/cli/projects/list')
    expect(lastFetchUrl(fetchMock)).toContain('token=remote-tok')
  })

  it('daemonCliGetText returns the RAW body (no JSON parse)', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    // A JSON-array STRING that must reach the caller verbatim (timer export).
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes({ body: '[1,2,3]' })))
    const out = await daemonCliGetText('timer/entries-export', { format: 'json' })
    expect(out).toBe('[1,2,3]')
  })
})

describe('daemonCliPost — body in body, token in query', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  it('POSTs JSON body with Content-Type and ?token= (params NOT in URL)', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => fakeRes({ body: JSON.stringify({ delivered: true }) }))
    vi.stubGlobal('fetch', fetchMock)

    const out = await daemonCliPost('workspace/msg', { workspace: 'k2', body: 'hi' })
    expect(out).toEqual({ delivered: true })

    const [url, opts] = fetchMock.mock.calls[0] as [string, RequestInit]
    // Token rides in the query; the message body does NOT (out of URL logs).
    expect(url).toBe('http://127.0.0.1:47800/cli/workspace/msg?token=local-tok')
    expect(opts.method).toBe('POST')
    expect(opts.headers).toMatchObject({ 'Content-Type': 'application/json' })
    expect(JSON.parse(opts.body as string)).toEqual({ workspace: 'k2', body: 'hi' })
  })
})

describe('remote auth failures — session revival / expiry through daemon-cli', () => {
  beforeEach(() => {
    mem.clear()
    fakeKeychain.clear()
    __resetConnectHostStoreForTests()
    __resetRemoteSessionForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  /** Mirror the REAL getDaemonWs remote behavior: derive creds from the
   *  store at call time, so a revival-committed token reaches the replay. */
  function credsFollowStore(): void {
    getDaemonWsMock.mockImplementation(async () => {
      const active = useConnectHostStore.getState().activeHost
      if (active === 'local') return LOCAL_CREDS
      return { port: active.port, token: active.token, host: active.hostname, secure: active.secure }
    })
  }

  it('401 from the active REMOTE host with NO remembered password drops its token + raises sign-in', async () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    // Every fetch 401s — the request itself AND the revival's whoami probe,
    // so the session is confirmed dead; with no keychain password the
    // revival expires it (classic path).
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes({ status: 401, body: JSON.stringify({ error: 'session expired' }) })))

    // The call still rejects with the daemon's message...
    await expect(daemonCliGet('projects/list')).rejects.toThrow('session expired')

    // ...and the session was expired: token cleared, sign-in pending.
    const active = useConnectHostStore.getState().activeHost
    expect(active).not.toBe('local')
    expect((active as ConnectHost).token).toBe('')
    expect(useConnectHostStore.getState().pendingSignIn?.id).toBe('r1')
  })

  it('401 while LOCAL is active does NOT raise a remote sign-in', async () => {
    // active stays 'local' (the default after reset)
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn(async () => fakeRes({ status: 401, body: JSON.stringify({ error: 'nope' }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toThrow('nope')
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
    expect(fetchMock).toHaveBeenCalledTimes(1) // no probe, no replay
  })

  it("the stale-session 403 ('Invalid or missing auth token') silently re-logs-in and REPLAYS the request", async () => {
    // The owner's bug: a remote restart wipes connect-sessions; every /cli/*
    // then 403s with the token-gate body (NOT 401). With a remembered
    // password the call must self-heal: whoami-confirm → re-login → replay
    // with the fresh token — no sign-in overlay, no error surfaced.
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    credsFollowStore()
    fakeKeychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:r1`, 'hunter2')

    const staleBody = JSON.stringify({ error: 'Invalid or missing auth token' })
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes('/cli/auth/whoami')) return fakeRes({ status: 403, body: staleBody })
      if (url.includes('/cli/auth/login')) {
        return {
          ...fakeRes({ status: 200, body: '' }),
          json: async () => ({ token: 'fresh-tok', username: 'rosson', expiresAt: 'x' }),
        } as unknown as Response
      }
      // The data route: stale token → 403; fresh token → 200.
      if (url.includes('token=fresh-tok')) return fakeRes({ body: JSON.stringify({ ok: 1 }) })
      return fakeRes({ status: 403, body: staleBody })
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).resolves.toEqual({ ok: 1 })

    // The replay carried the REVIVED token, and no sign-in was raised.
    const dataCalls = fetchMock.mock.calls
      .map(([u]) => u as string)
      .filter((u) => u.includes('projects/list'))
    expect(dataCalls.length).toBe(2)
    expect(dataCalls[0]).toContain('token=remote-tok')
    expect(dataCalls[1]).toContain('token=fresh-tok')
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('fresh-tok')
  })

  it('a 403 whose body is NOT the token gate (role denial etc.) is surfaced without probing', async () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    const fetchMock = vi.fn(async () => fakeRes({ status: 403, body: JSON.stringify({ error: 'peer not allowed' }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toThrow('peer not allowed')
    expect(fetchMock).toHaveBeenCalledTimes(1) // no whoami, no replay
    expect((useConnectHostStore.getState().activeHost as ConnectHost).token).toBe('remote-tok')
  })
})

describe('withConnRetry — connection-level failures retry (shared withRemoteRetry backoff)', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  it('a "Load failed" fetch error invalidates creds and retries → success', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error('Load failed')) // WKWebView ECONNREFUSED shape
      .mockResolvedValueOnce(fakeRes({ body: JSON.stringify({ ok: 2 }) }))
    vi.stubGlobal('fetch', fetchMock)

    const out = await daemonCliGet('projects/list')
    expect(out).toEqual({ ok: 2 })
    expect(fetchMock).toHaveBeenCalledTimes(2)
    // Creds were invalidated before the retry so the second attempt re-reads
    // the daemon's (possibly rotated) port/token.
    expect(invalidateDaemonWsMock).toHaveBeenCalledTimes(1)
  })

  it('a connection error that fails every attempt rejects after the connected one-shot', async () => {
    // Connected `/cli/*` uses delaysMs [0] → 1 initial try + 1 eviction retry.
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn().mockRejectedValue(new Error('Failed to fetch'))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toThrow('Failed to fetch')
    expect(fetchMock).toHaveBeenCalledTimes(2)
    // invalidateDaemonWs fired before the delay-0 retry.
    expect(invalidateDaemonWsMock).toHaveBeenCalledTimes(1)
  })

  it('exhausted REMOTE connection failure kicks a health probe; LOCAL does not', async () => {
    const probe = vi.spyOn(connectionGateProbe, 'forceSoftHealthProbe').mockImplementation(() => {})
    try {
      const host = makeRemoteHost()
      useConnectHostStore.getState().addHost(host)
      useConnectHostStore.getState().selectHost(host)
      getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
      const remoteFetch = vi.fn().mockRejectedValue(new Error('Load failed'))
      vi.stubGlobal('fetch', remoteFetch)
      await expect(daemonCliGet('terminal/send-message')).rejects.toThrow('Load failed')
      expect(remoteFetch).toHaveBeenCalledTimes(2)
      expect(probe).toHaveBeenCalledTimes(1)

      probe.mockClear()
      __resetConnectHostStoreForTests()
      getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
      const localFetch = vi.fn().mockRejectedValue(new Error('Load failed'))
      vi.stubGlobal('fetch', localFetch)
      await expect(daemonCliGet('projects/list')).rejects.toThrow('Load failed')
      expect(localFetch).toHaveBeenCalledTimes(2)
      expect(probe).not.toHaveBeenCalled()
    } finally {
      probe.mockRestore()
    }
  })

  it('remote ACAO TypeError is 1 fetch then probe once', async () => {
    const probe = vi.spyOn(connectionGateProbe, 'forceSoftHealthProbe').mockImplementation(() => {})
    try {
      const host = makeRemoteHost()
      useConnectHostStore.getState().addHost(host)
      useConnectHostStore.getState().selectHost(host)
      getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
      const fetchMock = vi.fn().mockRejectedValue(
        new TypeError(
          'Origin tauri://localhost is not allowed by Access-Control-Allow-Origin. Status code: 404',
        ),
      )
      vi.stubGlobal('fetch', fetchMock)

      await expect(daemonCliGet('projects/list')).rejects.toThrow(/access-control-allow-origin/i)
      expect(fetchMock).toHaveBeenCalledTimes(1)
      expect(invalidateDaemonWsMock).not.toHaveBeenCalled()
      expect(probe).toHaveBeenCalledTimes(1)
    } finally {
      probe.mockRestore()
    }
  })

  it('an APP-level non-2xx is NOT retried — it surfaces the daemon error verbatim', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn(async () => fakeRes({ status: 400, body: JSON.stringify({ error: 'bad request' }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toThrow('bad request')
    expect(fetchMock).toHaveBeenCalledTimes(1) // no retry
    expect(invalidateDaemonWsMock).not.toHaveBeenCalled()
  })
})

// 0.40.48 storm killer — while the ACTIVE remote host is recovering
// (ConnectionGate flipped recovery off 'connected'), every /cli/* call must
// FAIL FAST with RecoveringError instead of issuing a fetch: during the
// wedge incident a dozen retry loops stacked ~11 req/s of guaranteed
// failures through the poisoned pool. Recovery's own traffic (boot-status /
// whoami / login) never rides daemon-cli, so nothing here can starve it.
describe('recovery gate — fail-fast while the active remote is recovering', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  function activateRemote(): void {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
  }

  it.each([
    [{ kind: 'reconnecting', bootPhase: null } as const],
    [{ kind: 'reconnecting', bootPhase: 'migrating' } as const],
    [{ kind: 'reauthenticating' } as const],
    [{ kind: 'signin-required' } as const],
    [{ kind: 'wedged' } as const],
  ])('active REMOTE in %j → RecoveringError, ZERO network calls', async (recovery) => {
    activateRemote()
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    useConnectHostStore.getState().setRecovery(recovery)
    const fetchMock = vi.fn(async () => fakeRes({ body: '{}' }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toBeInstanceOf(RecoveringError)
    await expect(daemonCliPost('workspace/msg', { a: 1 })).rejects.toBeInstanceOf(RecoveringError)
    await expect(daemonCliGetText('timer/entries-export')).rejects.toBeInstanceOf(RecoveringError)
    expect(fetchMock).not.toHaveBeenCalled()
    // The fail-fast must NOT be classified as a connection error, so the
    // shared retry never burns its backoff on it (a single throw, no timers).
    expect(invalidateDaemonWsMock).not.toHaveBeenCalled()
  })

  it('recovery returning to connected immediately unblocks traffic (no restart needed)', async () => {
    activateRemote()
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
    useConnectHostStore.getState().setRecovery({ kind: 'reconnecting', bootPhase: null })
    const fetchMock = vi.fn(async () => fakeRes({ body: JSON.stringify({ ok: 1 }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toBeInstanceOf(RecoveringError)
    useConnectHostStore.getState().setRecovery({ kind: 'connected' })
    await expect(daemonCliGet('projects/list')).resolves.toEqual({ ok: 1 })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('LOCAL host is NEVER gated — its recovery field is meaningless', async () => {
    // Active stays 'local'; even a weird lingering recovery state (e.g. left
    // over from a host switch race) must not block loopback traffic.
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    useConnectHostStore.getState().setRecovery({ kind: 'reconnecting', bootPhase: null })
    const fetchMock = vi.fn(async () => fakeRes({ body: JSON.stringify({ ok: 3 }) }))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).resolves.toEqual({ ok: 3 })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})
