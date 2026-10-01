// Home M2 — the per-server connection pool (MS23, MS26–MS29, MS31, MS36,
// MS37, MS52 m/n/o, MS61, MS71, MS81), with several "windows" side by side.
//
// Each fake window has its own saved-server list (tokens live in memory per
// window), its own pool and its own cross-window sync. They share what the
// real windows share: localStorage (the login lease, budgets and blocks),
// the keychain, and the Tauri event bus. One fake daemon B answers boot-
// status, whoami, the presence summary and login, and counts login POSTs.

import { describe, it, expect, beforeEach } from 'vitest'
import { createHostPool, nextCheckDelayMs, sameServerPairs, type HostPool, type HostEntry } from './host-pool'
import { createLoginCoordinator, type CoordStorage } from './host-login-coord'
import { createHostSessionSync, type HostSessionEvent, type HostSessionSync } from './host-session-sync'
import type { ConnectHost, LoginResult } from '@/stores/connect-host'
import type { LoginLanded } from './connect-host-hooks'

class MemStorage implements CoordStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
  removeItem(k: string): void {
    this.map.delete(k)
  }
}

interface FakeDaemon {
  base: string
  up: boolean
  phase: string
  version: string
  instanceId: string
  password: string
  role: string
  mustChange: boolean
  tokens: Set<string>
  loginPosts: number
  nextLoginStatus: number | null
  retryAfter: string | null
  minted: number
}

function daemon(base: string): FakeDaemon {
  return {
    base,
    up: true,
    phase: 'ready',
    version: '0.41.6',
    instanceId: `inst-${base}`,
    password: 'pw',
    role: 'member',
    mustChange: false,
    tokens: new Set(['tok-0']),
    loginPosts: 0,
    nextLoginStatus: null,
    retryAfter: null,
    minted: 0,
  }
}

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status, headers })
}

let now = 1_000_000
let shared: MemStorage
let keychain: Map<string, string>
let passwords: Map<string, string>
let daemons: Map<string, FakeDaemon>
let windows: FakeWindow[]

function serve(url: string, init?: RequestInit): Response {
  const u = new URL(url)
  const d = daemons.get(u.origin)
  if (!d || !d.up) throw new TypeError('Load failed')
  const token = u.searchParams.get('token') ?? ''
  if (u.pathname === '/boot-status') {
    return json(200, { version: d.version, protocol: 1, phase: d.phase, instanceId: d.instanceId })
  }
  if (u.pathname === '/cli/auth/whoami') {
    if (!d.tokens.has(token)) return json(403, { error: 'Invalid or missing auth token' })
    return json(200, { username: 'rosson', owner: false, role: d.role, mustChangePassword: d.mustChange })
  }
  if (u.pathname === '/cli/presence/summary') {
    if (!d.tokens.has(token)) return json(403, { error: 'Invalid or missing auth token' })
    return json(200, { online: 1, workspaces: [] })
  }
  throw new Error(`unexpected ${init?.method ?? 'GET'} ${url}`)
}

interface FakeWindow {
  id: string
  hosts: ConnectHost[]
  pool: HostPool
  sync: HostSessionSync
  primaryKey: string
  overlays: number
}

function hostFor(base: string, over: Partial<ConnectHost> = {}): ConnectHost {
  const u = new URL(base)
  return {
    id: `id-${u.hostname}`,
    label: u.hostname.split('.')[0]!.toUpperCase(),
    hostname: u.hostname,
    port: 443,
    secure: true,
    username: 'rosson',
    token: 'tok-0',
    remember: true,
    lastConnectedAt: null,
    ...over,
  }
}

function makeWindow(id: string, savedHosts: ConnectHost[]): FakeWindow {
  const w: FakeWindow = {
    id,
    hosts: savedHosts.map((h) => ({ ...h })),
    pool: undefined as unknown as HostPool,
    sync: undefined as unknown as HostSessionSync,
    primaryKey: 'local',
    overlays: 0,
  }
  const coord = createLoginCoordinator({ storage: shared, now: () => now, windowId: id, settle: async () => {} })
  const setToken = (hostId: string, token: string): void => {
    w.hosts = w.hosts.map((h) => (h.id === hostId ? { ...h, token } : h))
  }
  const login = async (host: ConnectHost, password: string): Promise<LoginResult> => {
    const d = daemons.get(new URL(`https://${host.hostname}`).origin)
    if (!d || !d.up) return { ok: false, kind: 'unreachable', reason: 'down' }
    d.loginPosts += 1
    if (d.nextLoginStatus === 429) {
      return { ok: false, kind: 'throttled', reason: 'Too many sign-ins from this network. Try again in 2 min.', retryAfterSec: 120 }
    }
    if (password !== d.password) return { ok: false, kind: 'auth', reason: 'Invalid username or password.' }
    d.minted += 1
    const token = `tok-${d.minted}`
    d.tokens.add(token)
    setToken(host.id, token)
    if (host.remember) keychain.set(host.id, token)
    const landed: LoginLanded = { host: { ...host, token }, token, mustChangePassword: d.mustChange }
    w.sync.loginLanded(landed)
    return { ok: true, token, mustChangePassword: d.mustChange }
  }
  w.pool = createHostPool({
    hosts: () => w.hosts,
    windowHostKey: () => w.primaryKey,
    localCreds: async () => {
      throw new Error('no local daemon in this test')
    },
    http: async (_key, url, init) => serve(url, init),
    bootStatus: async (_key, base) => {
      try {
        const res = serve(`${base}/boot-status`)
        return (await res.json()) as Record<string, unknown>
      } catch {
        return null
      }
    },
    resolvePassword: async (hostId) => passwords.get(hostId) ?? null,
    login,
    dropSessionInMemory: (hostId) => setToken(hostId, ''),
    coord,
    noteVersion: () => {},
    now: () => now,
  })
  w.sync = createHostSessionSync({
    windowId: id,
    emit: async (_event, payload) => {
      // The Tauri bus delivers to every window, the sender included.
      await Promise.all(windows.map((x) => x.sync.apply(payload)))
    },
    listen: async () => () => {},
    hosts: () => w.hosts,
    windowHostKey: () => w.primaryKey,
    resolveToken: async (hostId) => keychain.get(hostId) ?? null,
    setHostToken: setToken,
    dropSessionInMemory: (hostId) => setToken(hostId, ''),
    coord,
    pool: w.pool,
  })
  windows.push(w)
  return w
}

const B_BASE = 'https://b.k2.dev'
const C_BASE = 'https://c.k2.dev'

beforeEach(() => {
  now = 1_000_000
  shared = new MemStorage()
  keychain = new Map()
  passwords = new Map()
  daemons = new Map([
    [B_BASE, daemon(B_BASE)],
    [C_BASE, daemon(C_BASE)],
  ])
  windows = []
})

function B(): FakeDaemon {
  return daemons.get(B_BASE) as FakeDaemon
}

function entryOf(w: FakeWindow, key: string): HostEntry {
  const e = w.pool.entry(key)
  if (!e) throw new Error(`no pool entry for ${key}`)
  return e
}

describe('pool state transitions', () => {
  it('ok → live with version, instanceId and role from that server', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE)])
    const e = await w.pool.check('b.k2.dev')
    expect(e.reach).toBe('live')
    expect(e.auth).toBe('ok')
    expect(e.role).toBe('member')
    expect(e.boot?.version).toBe('0.41.6')
    expect(e.boot?.instanceId).toBe(`inst-${B_BASE}`)
    expect(e.presence).toEqual([])
    expect(e.hostId).toBe('id-b.k2.dev')
  })

  it('not ready → starting; unreachable → offline with the 5 s / 15 s / 30 s probe backoff', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE)])
    B().phase = 'migrating'
    expect((await w.pool.check('b.k2.dev')).reach).toBe('starting')
    B().up = false
    const delays: number[] = []
    for (let i = 0; i < 4; i++) delays.push(nextCheckDelayMs(await w.pool.check('b.k2.dev')))
    expect(delays).toEqual([5_000, 15_000, 30_000, 30_000])
    B().up = true
    B().phase = 'ready'
    const back = await w.pool.check('b.k2.dev')
    expect(back.reach).toBe('live')
    expect(back.offlineStreak).toBe(0)
    expect(nextCheckDelayMs(back)).toBe(30_000)
  })

  it('MS52 o: a new instanceId fires only that server’s restart listeners', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE), hostFor(C_BASE)])
    const heard: string[] = []
    w.pool.onRestart((key, prev, next) => heard.push(`${key}:${prev}->${next}`))
    await w.pool.check('b.k2.dev')
    await w.pool.check('c.k2.dev')
    B().instanceId = 'inst-b-2'
    await w.pool.check('b.k2.dev')
    await w.pool.check('c.k2.dev')
    expect(heard).toEqual([`b.k2.dev:inst-${B_BASE}->inst-b-2`])
  })

  it('no login and no remembered password → signin-required, and no login POST', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE, { token: '' })])
    const e = await w.pool.check('b.k2.dev')
    expect(e.reach).toBe('live')
    expect(e.auth).toBe('signin-required')
    expect(B().loginPosts).toBe(0)
  })

  it('a temporary password → rotate-required, never an overlay', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE)])
    B().mustChange = true
    const e = await w.pool.check('b.k2.dev')
    expect(e.auth).toBe('rotate-required')
    expect(e.authNote).toBe('B needs a new password.')
    expect(w.overlays).toBe(0)
  })

  it('MS61: the key resolving to another saved entry resets role and auth', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE)])
    await w.pool.check('b.k2.dev')
    expect(entryOf(w, 'b.k2.dev').role).toBe('member')
    // Removed and re-added: same address, a new client id, no login yet.
    w.hosts = [hostFor(B_BASE, { id: 'id-b-readded', token: '' })]
    const e = await w.pool.check('b.k2.dev')
    expect(e.hostId).toBe('id-b-readded')
    expect(e.role).toBeNull()
    expect(e.auth).toBe('signin-required')
  })
})

describe('automatic login (MS28, MS31, MS36)', () => {
  it('an expired token with a remembered password signs in once and stays ok', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w = makeWindow('w1', [hostFor(B_BASE)])
    B().tokens.clear() // the session expired on B
    const e = await w.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(1)
    expect(e.auth).toBe('ok')
    expect(e.role).toBe('member')
    expect(w.hosts[0]!.token).toBe('tok-1')
    expect(keychain.get('id-b.k2.dev')).toBe('tok-1')
  })

  it('MS52 n: 3 windows × 3 rooms after token expiry → exactly 1 login POST, every window gets the token', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const ws = [makeWindow('w1', [hostFor(B_BASE)]), makeWindow('w2', [hostFor(B_BASE)]), makeWindow('w3', [hostFor(B_BASE)])]
    B().tokens.clear()
    const outcomes = await Promise.all(ws.flatMap((w) => [w.pool.revive('b.k2.dev'), w.pool.revive('b.k2.dev'), w.pool.revive('b.k2.dev')]))
    expect(B().loginPosts).toBe(1)
    expect(outcomes.filter((o) => o === 'revived')).toHaveLength(3) // one window's three rooms share its one revive
    expect(outcomes.filter((o) => o === 'waiting')).toHaveLength(6)
    for (const w of ws) {
      expect([w.id, w.hosts[0]!.token]).toEqual([w.id, 'tok-1'])
      expect([w.id, entryOf(w, 'b.k2.dev').auth]).toEqual([w.id, 'ok'])
    }
    // Later checks in every window find the login alive: still one POST.
    for (const w of ws) await w.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(1)
  })

  it('a refused remembered password is never tried again automatically, in any window', async () => {
    passwords.set('id-b.k2.dev', 'wrong')
    const w1 = makeWindow('w1', [hostFor(B_BASE)])
    const w2 = makeWindow('w2', [hostFor(B_BASE)])
    B().tokens.clear()
    expect(await w1.pool.revive('b.k2.dev')).toBe('signin-required')
    expect(B().loginPosts).toBe(1)
    now += 10 * 60_000 // past every budget window
    expect(await w2.pool.revive('b.k2.dev')).toBe('signin-required')
    await w1.pool.check('b.k2.dev')
    await w2.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(1)
    expect(entryOf(w2, 'b.k2.dev').authNote).toBe('B refused the saved password. Sign in again.')
  })

  it('MS36: background failures never delete the keychain token or the password', async () => {
    passwords.set('id-b.k2.dev', 'wrong')
    keychain.set('id-b.k2.dev', 'tok-0')
    const w = makeWindow('w1', [hostFor(B_BASE)])
    B().tokens.clear()
    await w.pool.check('b.k2.dev')
    expect(entryOf(w, 'b.k2.dev').auth).toBe('signin-required')
    expect(keychain.get('id-b.k2.dev')).toBe('tok-0')
    expect(passwords.get('id-b.k2.dev')).toBe('wrong')
    expect(w.hosts[0]!.token).toBe('') // dropped in memory only
  })

  it('a 429 holds automatic logins until Retry-After, with the throttle copy', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w = makeWindow('w1', [hostFor(B_BASE)])
    B().tokens.clear()
    B().nextLoginStatus = 429
    expect(await w.pool.revive('b.k2.dev')).toBe('signin-required')
    expect(entryOf(w, 'b.k2.dev').authNote).toBe('Too many sign-ins from this network. Try again in 2 min.')
    B().nextLoginStatus = null
    now += 60_000
    await w.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(1)
    // Still held: the row keeps saying how long.
    expect(entryOf(w, 'b.k2.dev').authNote).toBe('Too many sign-ins from this network. Try again in 1 min.')
    now += 6 * 60_000 // past Retry-After and the per-server budget
    await w.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(2)
    expect(entryOf(w, 'b.k2.dev').auth).toBe('ok')
  })

  it('MS31: at most 1 automatic login per server and 3 across servers per 5 min', async () => {
    const bases = ['https://d1.k2.dev', 'https://d2.k2.dev', 'https://d3.k2.dev', 'https://d4.k2.dev']
    for (const b of bases) daemons.set(b, daemon(b))
    const hosts = bases.map((b) => hostFor(b, { token: '' }))
    for (const h of hosts) passwords.set(h.id, 'wrong-then-right')
    for (const b of bases) (daemons.get(b) as FakeDaemon).password = 'wrong-then-right'
    const w = makeWindow('w1', hosts)
    for (const b of bases) {
      const key = new URL(b).hostname
      await w.pool.check(key)
    }
    const posts = bases.map((b) => (daemons.get(b) as FakeDaemon).loginPosts)
    expect(posts).toEqual([1, 1, 1, 0])
    expect(entryOf(w, 'd4.k2.dev').auth).toBe('signin-required')
    // A second check of a server inside the window never posts again.
    w.hosts = w.hosts.map((h) => (h.hostname === 'd1.k2.dev' ? { ...h, token: '' } : h))
    await w.pool.check('d1.k2.dev')
    expect((daemons.get('https://d1.k2.dev') as FakeDaemon).loginPosts).toBe(1)
  })
})

describe('kick (MS37, MS52 m, MS71)', () => {
  it('a 4001 close marks kicked in every window and no login is posted afterwards', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w1 = makeWindow('w1', [hostFor(B_BASE)])
    const w2 = makeWindow('w2', [hostFor(B_BASE)])
    w1.pool.noteSocketClose('b.k2.dev', 4001)
    B().tokens.clear()
    expect(entryOf(w1, 'b.k2.dev').auth).toBe('kicked')
    expect(entryOf(w1, 'b.k2.dev').authNote).toBe('Removed from B. Sign in again.')
    expect(await w2.pool.revive('b.k2.dev')).toBe('kicked')
    now += 30 * 60_000
    await w1.pool.check('b.k2.dev')
    await w2.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(0)
    expect(entryOf(w2, 'b.k2.dev').auth).toBe('kicked')
  })

  it('an older daemon (no close code) is inferred: a close in the last 5 s plus a refused whoami', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w = makeWindow('w1', [hostFor(B_BASE)])
    w.pool.noteSocketClose('b.k2.dev', 1005)
    B().tokens.clear()
    expect(await w.pool.revive('b.k2.dev')).toBe('kicked')
    expect(B().loginPosts).toBe(0)
  })

  it('a 4003 session-revoked close is not a kick: the remembered password signs in', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w = makeWindow('w1', [hostFor(B_BASE)])
    w.pool.noteSocketClose('b.k2.dev', 4003)
    w.pool.noteSocketClose('b.k2.dev', 1005)
    B().tokens.clear()
    expect(await w.pool.revive('b.k2.dev')).toBe('revived')
    expect(B().loginPosts).toBe(1)
  })

  it('a click-in (any login landing) clears the kick for every window', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w1 = makeWindow('w1', [hostFor(B_BASE)])
    const w2 = makeWindow('w2', [hostFor(B_BASE)])
    w1.pool.noteSocketClose('b.k2.dev', 4001)
    // The user signs in by hand in window 2 (RemoteSignIn → loginToHost).
    const landed: LoginLanded = { host: { ...w2.hosts[0]!, token: 'tok-click' }, token: 'tok-click', mustChangePassword: false }
    B().tokens.add('tok-click')
    keychain.set('id-b.k2.dev', 'tok-click')
    w2.hosts = w2.hosts.map((h) => ({ ...h, token: 'tok-click' }))
    w2.sync.loginLanded(landed)
    await Promise.resolve()
    await new Promise((r) => setTimeout(r, 0))
    expect(w1.hosts[0]!.token).toBe('tok-click')
    expect(entryOf(w1, 'b.k2.dev').auth).toBe('ok')
    expect((await w1.pool.check('b.k2.dev')).auth).toBe('ok')
    expect(B().loginPosts).toBe(0)
  })
})

describe('cross-window login broadcast (MS28, MS62)', () => {
  it('one window’s login reaches the others through the keychain, with no login POST of their own', async () => {
    passwords.set('id-b.k2.dev', 'pw')
    const w1 = makeWindow('w1', [hostFor(B_BASE, { token: '' })])
    // Window 2 saved the same server under its own id (map by host key).
    const w2 = makeWindow('w2', [hostFor(B_BASE, { token: '' })])
    const seen: HostSessionEvent[] = []
    const realApply = w2.sync.apply
    w2.sync.apply = async (e) => {
      seen.push(e)
      await realApply(e)
    }
    await w1.pool.check('b.k2.dev')
    expect(B().loginPosts).toBe(1)
    expect(seen).toEqual([{ hostKey: 'b.k2.dev', hostId: 'id-b.k2.dev', kind: 'signed-in', from: 'w1' }])
    expect(w2.hosts[0]!.token).toBe('tok-1')
    expect((await w2.pool.check('b.k2.dev')).auth).toBe('ok')
    expect(B().loginPosts).toBe(1)
  })

  it('a server without “remember” has no keychain token: the others keep Sign in and post nothing', async () => {
    const w1 = makeWindow('w1', [hostFor(B_BASE, { token: '', remember: false })])
    const w2 = makeWindow('w2', [hostFor(B_BASE, { token: '', remember: false })])
    // Window 1 signs in by click (no remembered password). loginToHost
    // writes the keychain even without "remember"; the other windows still
    // must not pick it up (MS28).
    const d = B()
    d.minted += 1
    const res = `tok-${d.minted}`
    d.tokens.add(res)
    keychain.set('id-b.k2.dev', res)
    w1.hosts = w1.hosts.map((h) => ({ ...h, token: res }))
    w1.sync.loginLanded({ host: w1.hosts[0]!, token: res, mustChangePassword: false })
    await new Promise((r) => setTimeout(r, 0))
    expect(res).toBe('tok-1')
    expect(w2.hosts[0]!.token).toBe('')
    expect((await w2.pool.check('b.k2.dev')).auth).toBe('signin-required')
    expect(B().loginPosts).toBe(0)
  })

  it('signing out in one window drops the others’ in-memory token (not the window on that server)', async () => {
    const w1 = makeWindow('w1', [hostFor(B_BASE)])
    const w2 = makeWindow('w2', [hostFor(B_BASE)])
    const w3 = makeWindow('w3', [hostFor(B_BASE)])
    w3.primaryKey = 'b.k2.dev'
    w1.sync.signedOut(w1.hosts[0]!)
    await new Promise((r) => setTimeout(r, 0))
    expect(w2.hosts[0]!.token).toBe('')
    expect(entryOf(w2, 'b.k2.dev').auth).toBe('signin-required')
    expect(w3.hosts[0]!.token).toBe('tok-0')
  })
})

describe('same server at two addresses (MS81, answer Q2(a))', () => {
  it('a later key with the same instanceId gets a note naming the earlier one', () => {
    const boot = (id: string): Pick<HostEntry, 'boot'> => ({
      boot: { phase: 'ready', ready: true, version: '0.41.6', protocol: 1, instanceId: id, at: 1 },
    })
    const pairs = sameServerPairs(['local', 'rosson.k2.dev', 'b.k2.dev', '192.168.1.20:38471'], {
      local: boot('i-1'),
      'rosson.k2.dev': boot('i-1'),
      'b.k2.dev': boot('i-2'),
      '192.168.1.20:38471': boot('i-1'),
    })
    expect(pairs).toEqual({ 'rosson.k2.dev': 'local', '192.168.1.20:38471': 'local' })
    expect(sameServerPairs(['a', 'b'], { a: boot('x'), b: { boot: null } })).toEqual({})
  })

  it('the pool keeps each address as its own entry', async () => {
    const w = makeWindow('w1', [hostFor(B_BASE), hostFor(C_BASE)])
    daemons.get(C_BASE)!.instanceId = B().instanceId
    await w.pool.check('b.k2.dev')
    await w.pool.check('c.k2.dev')
    expect(entryOf(w, 'b.k2.dev').boot?.instanceId).toBe(entryOf(w, 'c.k2.dev').boot?.instanceId)
    expect(sameServerPairs(['b.k2.dev', 'c.k2.dev'], w.pool.store.getState().entries)).toEqual({ 'c.k2.dev': 'b.k2.dev' })
  })
})
