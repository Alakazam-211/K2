// prd-voice-chat-only-members-v1 §7, §8.1, vs-live V3: the connection pool
// and a Voice & Chat only login. One fake daemon answers boot-status,
// whoami and the presence summary; it counts every request and login.

import { describe, it, expect, beforeEach } from 'vitest'
import { createHostPool, roleAllowsRoom, isVoiceChatOnly, type HostPool } from './host-pool'
import { createLoginCoordinator, type CoordStorage } from './host-login-coord'
import type { ConnectHost, LoginResult } from '@/stores/connect-host'

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

const KEY = 'box.k2.dev'
const HOST: ConnectHost = {
  id: 'id-box',
  label: 'Box',
  hostname: 'box.k2.dev',
  port: 443,
  secure: true,
  username: 'reviewer',
  token: 'tok-1',
  remember: true,
  lastConnectedAt: null,
}

interface Fake {
  role: string
  /** When set, whoami answers these roles in turn (the last repeats). */
  roles?: string[]
  /** What `/cli/presence/summary` answers. */
  summary: 'ok' | 'role_required' | 'dead'
  paths: string[]
  logins: number
}

let fake: Fake
let notedRoles: Array<[string, string | null]>
let pool: HostPool

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status })
}

function serve(url: string): Response {
  const u = new URL(url)
  fake.paths.push(u.pathname)
  if (u.pathname === '/boot-status') {
    return json(200, { version: '0.46.10', protocol: 1, phase: 'ready', instanceId: 'i-1', features: ['voice-rooms-v1', 'voice-chat-role-v1'] })
  }
  if (u.searchParams.get('token') !== 'tok-1') return json(403, { error: 'Invalid or missing auth token' })
  if (u.pathname === '/cli/auth/whoami') {
    const n = fake.paths.filter((p) => p === '/cli/auth/whoami').length - 1
    const role = fake.roles ? fake.roles[Math.min(n, fake.roles.length - 1)] : fake.role
    return json(200, { username: 'reviewer', owner: false, role, mustChangePassword: false, voiceChatOnly: role === 'voice_chat' })
  }
  if (u.pathname === '/cli/presence/summary') {
    if (fake.summary === 'role_required') {
      return json(403, { error: 'role_required', required: 'member', role: 'voice_chat', message: 'This server gives you Voice & Chat only' })
    }
    if (fake.summary === 'dead') return json(403, { error: 'Invalid or missing auth token' })
    return json(200, { workspaces: [{ id: 'w1' }], agentActivity: [], activeProjectIds: [] })
  }
  throw new Error(`unexpected request ${url}`)
}

beforeEach(() => {
  fake = { role: 'voice_chat', summary: 'ok', paths: [], logins: 0 }
  notedRoles = []
  const hosts: ConnectHost[] = [{ ...HOST }]
  pool = createHostPool({
    hosts: () => hosts,
    windowHostKey: () => 'local',
    localCreds: async () => {
      throw new Error('no local daemon in this test')
    },
    http: async (_key, url) => serve(url),
    bootStatus: async (_key, base, token) => (await serve(`${base}/boot-status?token=${token}`).json()) as Record<string, unknown>,
    resolvePassword: async () => 'pw',
    login: async (): Promise<LoginResult> => {
      fake.logins += 1
      return { ok: true, token: 'tok-1', mustChangePassword: false }
    },
    dropSessionInMemory: () => {
      throw new Error('a role refusal must never drop the login')
    },
    coord: createLoginCoordinator({ storage: new MemStorage(), now: () => 1_000, windowId: 'w', settle: async () => {} }),
    noteVersion: () => {},
    activate: async () => {
      throw new Error('not part of this test')
    },
    now: () => 1_000,
    noteRole: (hostKey, role) => notedRoles.push([hostKey, role]),
  })
})

describe('the pool and a Voice & Chat only login', () => {
  it('reads the role, never the presence summary, and the login stays ok', async () => {
    const e = await pool.check(KEY)
    expect(e.auth).toBe('ok')
    expect(e.role).toBe('voice_chat')
    expect(e.presence).toBeNull()
    expect(fake.paths).toEqual(['/boot-status', '/cli/auth/whoami'])
    expect(fake.logins).toBe(0)
    expect(notedRoles).toEqual([[KEY, 'voice_chat']])
    // Home and rooms: no access; the headset may still list it.
    expect(roleAllowsRoom(e.role)).toBe(false)
    expect(isVoiceChatOnly(e.role)).toBe(true)
  })

  it('a 403 role_required on the summary re-reads whoami: no revive, no login (V3)', async () => {
    // whoami still said member when the check began; the server already
    // refuses the summary (a downgrade landed in between).
    fake.roles = ['member', 'voice_chat']
    fake.summary = 'role_required'
    const e = await pool.check(KEY)
    expect(e.auth).toBe('ok')
    expect(e.role).toBe('voice_chat')
    expect(e.presence).toBeNull()
    expect(fake.logins).toBe(0)
    expect(fake.paths.filter((p) => p === '/cli/auth/whoami').length).toBe(2)
    expect(fake.paths.filter((p) => p === '/cli/presence/summary').length).toBe(1)
  })

  it('an invalid-token 403 on the summary is still a dead login (revive path unchanged)', async () => {
    fake.role = 'member'
    fake.summary = 'dead'
    // dropSessionInMemory throws in this harness: a dead login reaches it
    // only through a whoami refusal, which this fake never gives, so the
    // revive sees a live whoami and stops at 'still-valid'.
    const e = await pool.check(KEY)
    expect(e.auth).toBe('ok')
    expect(fake.logins).toBe(0)
  })

  it('refreshRole follows an upgrade back to member, live, with no sign-in', async () => {
    await pool.check(KEY)
    expect(pool.entry(KEY)?.role).toBe('voice_chat')
    fake.role = 'member'
    expect(await pool.refreshRole(KEY)).toBe('member')
    expect(pool.entry(KEY)?.role).toBe('member')
    expect(notedRoles.at(-1)).toEqual([KEY, 'member'])
    expect(fake.logins).toBe(0)
  })
})
