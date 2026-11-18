// The desktop app after a LOCAL daemon restart (0.46.15).
//
// The daemon mints a new owner token on every boot and usually keeps the
// same port. The renderer caches the port + token it read at start, so
// after an update (which restarts the daemon) every local request used to
// be refused with 403 "Invalid or missing auth token" until the app was
// restarted. These tests drive the REAL daemon-ws cache and the REAL
// daemon-cli layer against a scripted daemon: Tauri's `daemon_ws_url`
// returns whatever token the daemon wrote last, and `fetch` refuses any
// other token the way the dispatcher's token gate does.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

/** What `~/.k2/heartbeat.{port,token}` hold (what `daemon_ws_url` reads). */
const files = vi.hoisted(() => ({ port: 47800, token: 'tok-A', reads: 0 }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      files.reads += 1
      return { state: 'available', port: files.port, token: files.token }
    }
    return null
  }),
}))

import { daemonCliGet, daemonCliPost, localDaemonCliGet, isLocalTokenRefusal } from './daemon-cli'
import { getDaemonWs, invalidateDaemonWs, __resetLocalDaemonWsForTests } from '@/kessel/daemon-ws'
import { primaryScope } from '@/kessel/server-scope'
import { __resetConnectHostStoreForTests } from '@/stores/connect-host'
import { openQueuedWebSocket, openQueuedGridWebSocket, resetGridDialQueueForTests } from './grid-dial-queue'

const TOKEN_REFUSAL = JSON.stringify({ error: 'Invalid or missing auth token' })

/** The daemon: the token it accepts, and every request it was sent. */
const daemon = { token: 'tok-A', seen: [] as { method: string; token: string | null; path: string }[] }

function res(status: number, body: string): Response {
  return { ok: status >= 200 && status < 300, status, url: 'http://127.0.0.1:47800/cli/x', text: async () => body } as unknown as Response
}

/** A daemon whose token gate runs first (dispatcher.rs, the `/cli/*` arm). */
function tokenGatedFetch(handler: (path: string) => Response = () => res(200, '{"ok":true}')) {
  return vi.fn(async (url: string, init?: RequestInit) => {
    const u = new URL(url)
    const token = u.searchParams.get('token')
    daemon.seen.push({ method: init?.method ?? 'GET', token, path: u.pathname })
    if (token !== daemon.token) return res(403, TOKEN_REFUSAL)
    return handler(u.pathname)
  })
}

/** The daemon restarts: a new token on disk, the old one refused. */
function restartDaemon(token: string): void {
  daemon.token = token
  files.token = token
}

beforeEach(async () => {
  mem.clear()
  __resetConnectHostStoreForTests()
  invalidateDaemonWs()
  __resetLocalDaemonWsForTests()
  resetGridDialQueueForTests()
  files.port = 47800
  files.token = 'tok-A'
  files.reads = 0
  daemon.token = 'tok-A'
  daemon.seen = []
  // The app read the creds once at start (prewarm).
  await getDaemonWs(primaryScope())
  expect(files.reads).toBe(1)
})

describe('a local token refusal after a daemon restart', () => {
  it('re-reads the creds once and retries once: the call succeeds', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    restartDaemon('tok-B')

    await expect(daemonCliGet(primaryScope(), 'projects/list')).resolves.toEqual({ ok: true })
    expect(files.reads).toBe(2)
    expect(daemon.seen.map((s) => s.token)).toEqual(['tok-A', 'tok-B'])
  })

  it('a POST refused at the token gate is sent again once, with the new token', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    restartDaemon('tok-B')

    await expect(daemonCliPost(primaryScope(), 'thread/post', { text: 'hi' })).resolves.toEqual({ ok: true })
    expect(files.reads).toBe(2)
    expect(daemon.seen).toEqual([
      { method: 'POST', token: 'tok-A', path: '/cli/thread/post' },
      { method: 'POST', token: 'tok-B', path: '/cli/thread/post' },
    ])
  })

  it('the next call uses the new token straight away (no second refusal)', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    restartDaemon('tok-B')
    await daemonCliGet(primaryScope(), 'projects/list')
    daemon.seen = []

    await daemonCliGet(primaryScope(), 'projects/list')
    expect(daemon.seen.map((s) => s.token)).toEqual(['tok-B'])
    expect(files.reads).toBe(2)
  })

  it('many calls refused at once share ONE re-read', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    restartDaemon('tok-B')

    const calls = Array.from({ length: 6 }, (_, i) => daemonCliGet(primaryScope(), `r/${i}`))
    await expect(Promise.all(calls)).resolves.toHaveLength(6)
    expect(files.reads).toBe(2)
    expect(daemon.seen.filter((s) => s.token === 'tok-A')).toHaveLength(6)
    expect(daemon.seen.filter((s) => s.token === 'tok-B')).toHaveLength(6)
  })

  it('refused again with the new token: the refusal surfaces, no loop', async () => {
    // The files hold a token the daemon still refuses.
    vi.stubGlobal('fetch', tokenGatedFetch())
    daemon.token = 'tok-D'
    files.token = 'tok-C'

    const err = await daemonCliGet(primaryScope(), 'projects/list').then(
      () => null,
      (e: unknown) => e,
    )
    expect(err).toBeInstanceOf(Error)
    expect((err as Error).message).toBe('Invalid or missing auth token')
    expect((err as { status?: number }).status).toBe(403)
    expect(files.reads).toBe(2)
    expect(daemon.seen.map((s) => s.token)).toEqual(['tok-A', 'tok-C'])
  })

  it('the re-read finds the SAME creds: no retry (the refusal is real)', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    daemon.token = 'tok-other' // refuses tok-A, and tok-A is still on disk

    await expect(daemonCliGet(primaryScope(), 'projects/list')).rejects.toThrow('Invalid or missing auth token')
    expect(files.reads).toBe(2)
    expect(daemon.seen).toHaveLength(1)
  })

  it('localDaemonCliGet (clone-to pull) recovers the same way', async () => {
    vi.stubGlobal('fetch', tokenGatedFetch())
    restartDaemon('tok-B')

    await expect(localDaemonCliGet('vault/carry-recipient')).resolves.toEqual({ ok: true })
    expect(files.reads).toBe(2)
    expect(daemon.seen.map((s) => s.token)).toEqual(['tok-A', 'tok-B'])
  })
})

describe('a permission refusal is not a dead token', () => {
  const refusals: [string, number, string][] = [
    ['owner_only permission card', 403, JSON.stringify({ error: { code: 'owner_only' }, ok: false, permission: { requestable: false, where: 'x' } })],
    ['the owner gate text WITH a permission card', 403, JSON.stringify({ error: 'invalid or missing token', permission: { requestable: false, where: 'x' } })],
    ['Voice & Chat only role', 403, JSON.stringify({ error: { code: 'role_required', hint: 'needs Member' } })],
    ['password rotation', 403, JSON.stringify({ error: 'password_change_required' })],
    ['plain 401 without the token text', 401, JSON.stringify({ error: 'nope' })],
  ]
  for (const [name, status, body] of refusals) {
    it(`${name}: no re-read, no retry`, async () => {
      vi.stubGlobal('fetch', tokenGatedFetch(() => res(status, body)))

      await expect(daemonCliGet(primaryScope(), 'tunnel/start')).rejects.toBeInstanceOf(Error)
      expect(files.reads).toBe(1)
      expect(daemon.seen).toHaveLength(1)
    })
  }

  it('isLocalTokenRefusal matches the daemon token-gate bodies only', () => {
    expect(isLocalTokenRefusal(403, '{"error":"Invalid or missing auth token"}')).toBe(true)
    expect(isLocalTokenRefusal(403, '{"error":"invalid or missing token"}')).toBe(true)
    expect(isLocalTokenRefusal(401, '{"error":{"code":"unauthorized","hint":"invalid or missing auth token"}}')).toBe(true)
    expect(isLocalTokenRefusal(403, '{"error":{"code":"invalid_token","hint":"invalid or missing token"}}')).toBe(true)
    expect(isLocalTokenRefusal(404, '{"error":"invalid or missing token"}')).toBe(false)
    expect(isLocalTokenRefusal(403, '{"error":{"code":"owner_only"},"ok":false}')).toBe(false)
    expect(isLocalTokenRefusal(403, '{"error":"invalid or missing token","permission":{"requestable":false}}')).toBe(false)
  })
})

/** A socket double with real event listeners (what grid-dial-queue watches). */
class FakeWebSocket extends EventTarget {
  static all: FakeWebSocket[] = []
  readyState = 0
  binaryType = ''
  onopen: ((ev?: Event) => void) | null = null
  onerror: ((ev?: Event) => void) | null = null
  onclose: ((ev?: Event) => void) | null = null
  constructor(public url: string) {
    super()
    FakeWebSocket.all.push(this)
  }
  open(): void {
    this.readyState = 1
    this.onopen?.(new Event('open'))
    this.dispatchEvent(new Event('open'))
  }
  serverClose(code: number): void {
    this.readyState = 3
    const ev = Object.assign(new Event('close'), { code })
    this.onclose?.(ev)
    this.dispatchEvent(ev)
  }
  close(): void {
    this.readyState = 3
  }
}

async function eventsUrl(): Promise<string> {
  const creds = await getDaemonWs(primaryScope())
  return `ws://127.0.0.1:${creds.port}/cli/sessions/events?token=${creds.token}`
}

describe('local WebSocket reconnects after a daemon restart', () => {
  beforeEach(() => {
    FakeWebSocket.all = []
    vi.stubGlobal('WebSocket', FakeWebSocket)
  })

  it('a socket the daemon dropped reconnects with the NEW token', async () => {
    const first = await openQueuedWebSocket(primaryScope(), await eventsUrl())
    ;(first as unknown as FakeWebSocket).open()
    expect(first.url).toContain('token=tok-A')

    restartDaemon('tok-B')
    ;(first as unknown as FakeWebSocket).serverClose(1006)

    const again = await openQueuedWebSocket(primaryScope(), await eventsUrl())
    expect(again.url).toContain('token=tok-B')
    expect(files.reads).toBe(2)
  })

  it('a grid socket refused before it opened marks the creds stale too', async () => {
    const dial = openQueuedGridWebSocket(primaryScope(), (await eventsUrl()).replace('sessions/events', 'grid'))
    restartDaemon('tok-B')
    await vi.waitFor(() => expect(FakeWebSocket.all).toHaveLength(1))
    FakeWebSocket.all[0]!.serverClose(1006) // the upgrade got a 403
    await expect(dial).rejects.toThrow('grid-dial-failed')

    expect(await eventsUrl()).toContain('token=tok-B')
    expect(files.reads).toBe(2)
  })

  it('a clean close keeps the cached creds (no re-read)', async () => {
    const first = await openQueuedWebSocket(primaryScope(), await eventsUrl())
    ;(first as unknown as FakeWebSocket).open()
    ;(first as unknown as FakeWebSocket).serverClose(1000)

    expect(await eventsUrl()).toContain('token=tok-A')
    expect(files.reads).toBe(1)
  })

  it('many sockets dropped at once share one re-read', async () => {
    const socks = await Promise.all([1, 2, 3].map(async () => openQueuedWebSocket(primaryScope(), await eventsUrl())))
    for (const s of socks) (s as unknown as FakeWebSocket).open()
    restartDaemon('tok-B')
    for (const s of socks) (s as unknown as FakeWebSocket).serverClose(1006)

    const urls = await Promise.all([eventsUrl(), eventsUrl(), eventsUrl()])
    expect(urls.every((u) => u.includes('token=tok-B'))).toBe(true)
    expect(files.reads).toBe(2)
  })
})
