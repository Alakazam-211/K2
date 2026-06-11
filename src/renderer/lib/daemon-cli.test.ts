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
// persistHosts on addHost). Inert in node.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import { daemonCliGet, daemonCliGetText, daemonCliPost } from './daemon-cli'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

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
    const fetchMock = vi.fn(async () => fakeRes({ body: JSON.stringify({ ok: 1 }) }))
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
    const fetchMock = vi.fn(async () => fakeRes({ body: JSON.stringify({ delivered: true }) }))
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

describe('handleRemoteUnauthorized — a remote 401 expires the session', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
  })

  it('401 from the active REMOTE host drops its token + raises sign-in', async () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    getDaemonWsMock.mockResolvedValue(SECURE_CREDS)
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
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes({ status: 401, body: JSON.stringify({ error: 'nope' }) })))

    await expect(daemonCliGet('projects/list')).rejects.toThrow('nope')
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
  })
})

describe('withConnRetry — connection-level failures retry once', () => {
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

  it('a connection error that fails BOTH times rejects (one retry only)', async () => {
    getDaemonWsMock.mockResolvedValue(LOCAL_CREDS)
    const fetchMock = vi.fn().mockRejectedValue(new Error('Failed to fetch'))
    vi.stubGlobal('fetch', fetchMock)

    await expect(daemonCliGet('projects/list')).rejects.toThrow('Failed to fetch')
    expect(fetchMock).toHaveBeenCalledTimes(2)
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
