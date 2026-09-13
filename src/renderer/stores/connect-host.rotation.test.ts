// connect-host — edge login URL (D1/D4), the D3 404 verdict, and the
// switcher path's forced rotation (W4 via `pickHost`).
//
// The ServerSwitcher component defers every pick to the store's `pickHost`
// (ServerSwitcher.tsx `pick` → `pickHost(h)`), so this file IS the
// switcher-path test: a remembered TEMPORARY password must open the
// rotation step and must NOT activate the host.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

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
  useConnectHostStore,
  loginToHost,
  hostBaseUrl,
  __resetConnectHostStoreForTests,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  type ConnectHost,
} from './connect-host'
import { LOGIN_404_K2DEV_MESSAGE } from '@/lib/login-url'

function makeHost(overrides?: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'rpm',
    label: 'RPM box',
    hostname: 'rpm.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: '',
    remember: true,
    lastConnectedAt: null,
    ...overrides,
  }
}

function fakeRes(status: number, jsonBody?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (jsonBody === undefined ? '' : JSON.stringify(jsonBody)),
    json: async () => jsonBody,
  } as unknown as Response
}

beforeEach(() => {
  mem.clear()
  fakeKeychain.clear()
  __resetConnectHostStoreForTests()
  vi.unstubAllGlobals()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
    setItem: (k: string, v: string) => void mem.set(k, v),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
})

describe('loginToHost — login URL rule (D1)', () => {
  it('hosted apex host POSTs to the .app.k2.dev edge; body unchanged (desktop)', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    const fetchMock = vi.fn(async () =>
      fakeRes(200, { token: 'tok', username: 'rosson', expiresAt: 'x', mustChangePassword: false }),
    )
    vi.stubGlobal('fetch', fetchMock)
    const r = await loginToHost(host, 'hunter2')
    expect(r).toEqual({ ok: true, token: 'tok', mustChangePassword: false })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://rpm.app.k2.dev/cli/auth/login')
    expect(JSON.parse(init.body as string)).toEqual({ username: 'rosson', password: 'hunter2' })
    // The host's own base is untouched — only the login POST moved.
    expect(hostBaseUrl(host)).toBe('https://rpm.k2.dev')
  })

  it('LAN / self-host keeps its own origin', async () => {
    const host = makeHost({ id: 'lan', hostname: '192.168.1.50', port: 60710, secure: false })
    useConnectHostStore.getState().addHost(host)
    const fetchMock = vi.fn(async (_url: string) => fakeRes(200, { token: 'tok', username: 'rosson', expiresAt: 'x' }))
    vi.stubGlobal('fetch', fetchMock)
    await expect(loginToHost(host, 'pw')).resolves.toEqual({ ok: true, token: 'tok', mustChangePassword: false })
    expect(fetchMock.mock.calls[0][0]).toBe('http://192.168.1.50:60710/cli/auth/login')
  })

  it('an older daemon body without mustChangePassword reads as no rotation', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(200, { token: 'tok', username: 'rosson', expiresAt: 'x' })))
    const r = await loginToHost(host, 'pw')
    expect(r.ok && r.mustChangePassword).toBe(false)
  })
})

describe('loginToHost — 404 verdict (D3)', () => {
  it('hosted .k2.dev host: not-found with the D3 copy; nothing committed', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(404, { error: 'not_found' })))
    await expect(loginToHost(host, 'pw')).resolves.toEqual({
      ok: false,
      kind: 'not-found',
      reason: LOGIN_404_K2DEV_MESSAGE,
    })
    expect(useConnectHostStore.getState().hosts.find((h) => h.id === 'rpm')?.token).toBe('')
  })

  it('non-hosted host: not-found with the generic copy', async () => {
    const host = makeHost({ id: 'lan', hostname: '10.0.0.5', port: 8080, secure: false })
    useConnectHostStore.getState().addHost(host)
    vi.stubGlobal('fetch', vi.fn(async () => fakeRes(404)))
    const r = await loginToHost(host, 'pw')
    expect(r.ok).toBe(false)
    if (!r.ok) {
      expect(r.kind).toBe('not-found')
      expect(r.reason).toBe('Server returned 404. It may not be a K2 server.')
    }
  })
})

describe('pickHost — switcher path with a remembered TEMPORARY password (W4)', () => {
  it('mustChangePassword → rotation step requested, host NOT activated, restricted token kept', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    fakeKeychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:rpm`, 'temp-pw')
    const fetchMock = vi.fn(async (_url: string) =>
      fakeRes(200, { token: 'restricted', username: 'rosson', expiresAt: 'x', mustChangePassword: true }),
    )
    vi.stubGlobal('fetch', fetchMock)

    useConnectHostStore.getState().pickHost(host)
    await vi.waitFor(() => {
      expect(useConnectHostStore.getState().signInRotate).toBe(true)
    })
    const s = useConnectHostStore.getState()
    expect(s.activeHost).toBe('local') // NOT signed in / switched
    expect(s.pendingSignIn?.id).toBe('rpm')
    expect(s.pendingSignIn?.token).toBe('restricted') // the step needs it
    expect(s.signInActivate).toBe(true) // completes with a switch
    expect(fetchMock.mock.calls[0][0]).toBe('https://rpm.app.k2.dev/cli/auth/login')
  })

  it('normal remembered password → silent switch, no rotation', async () => {
    const host = makeHost()
    useConnectHostStore.getState().addHost(host)
    fakeKeychain.set(`${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:rpm`, 'hunter2')
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => fakeRes(200, { token: 'tok', username: 'rosson', expiresAt: 'x', mustChangePassword: false })),
    )
    useConnectHostStore.getState().pickHost(host)
    await vi.waitFor(() => {
      expect(useConnectHostStore.getState().activeHost).not.toBe('local')
    })
    const s = useConnectHostStore.getState()
    expect((s.activeHost as ConnectHost).token).toBe('tok')
    expect(s.signInRotate).toBe(false)
    expect(s.pendingSignIn).toBeNull()
  })

  it('requestPasswordRotation is idempotent for the same host and reset by requestSignIn', () => {
    const host = makeHost({ token: 'restricted' })
    const s = useConnectHostStore.getState()
    s.requestPasswordRotation(host)
    const first = useConnectHostStore.getState().pendingSignIn
    s.requestPasswordRotation({ ...host, label: 'changed' })
    expect(useConnectHostStore.getState().pendingSignIn).toBe(first)
    s.requestSignIn(host)
    expect(useConnectHostStore.getState().signInRotate).toBe(false)
    s.requestPasswordRotation(host, { activate: false })
    expect(useConnectHostStore.getState().signInActivate).toBe(false)
    s.cancelSignIn()
    expect(useConnectHostStore.getState().signInRotate).toBe(false)
    expect(useConnectHostStore.getState().pendingSignIn).toBeNull()
  })
})
