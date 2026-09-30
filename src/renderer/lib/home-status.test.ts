// Home P1 — row status resolver (ready / offline / sign-in / presence) and
// the one-server probe (public /boot-status, then the signed-in presence
// summary). Plan decisions 1 and 5; vs-live H14 / H19 f.

import { describe, it, expect, vi } from 'vitest'
import {
  bootIsReady,
  personInitials,
  presenceForRow,
  probeHost,
  resolveRowStatus,
  type HostProbe,
  type PresenceWorkspace,
  type ProbeDeps,
} from './home-status'
import type { RosterUser } from '@/stores/presence'

const row = { address: 'bee::b.k2.dev', workspaceId: 'pb' }

function probe(p: Partial<HostProbe>): HostProbe {
  return { reach: 'live', auth: 'ok', presence: [], at: 1, ...p }
}

const summary: PresenceWorkspace[] = [
  {
    workspaceId: 'pb',
    handle: 'bee',
    name: 'Bee',
    path: '/srv/bee',
    count: 3,
    people: [
      { user: 'owner', name: 'Rosson', role: 'owner' },
      { user: 'anna', name: 'anna', role: 'member' },
      { user: 'me', name: 'me', role: 'member' },
    ],
  },
  { workspaceId: 'pz', handle: 'zed', name: 'Zed', path: '/srv/zed', count: 1, people: [{ user: 'x', name: 'x', role: 'member' }] },
]

describe('bootIsReady', () => {
  it('accepts phase:"ready" or ready:true, nothing else', () => {
    expect(bootIsReady({ phase: 'ready' })).toBe(true)
    expect(bootIsReady({ ready: true, version: '0.41.6' } as { ready: boolean })).toBe(true)
    expect(bootIsReady({ phase: 'migrating' })).toBe(false)
    expect(bootIsReady({ ready: false })).toBe(false)
    expect(bootIsReady({ ready: 'true' })).toBe(false)
    expect(bootIsReady(null)).toBe(false)
  })
})

describe('resolveRowStatus — another saved server', () => {
  const base = { where: 'other' as const, row, saved: true, hasLogin: true, self: 'me' }

  it('ready → live, with the others on that agent (never you)', () => {
    const s = resolveRowStatus({ ...base, probe: probe({ presence: summary }) })
    expect(s.kind).toBe('live')
    expect(s.label).toBe('Live')
    expect(s.people.map((p) => p.user)).toEqual(['owner', 'anna'])
  })

  it('unreachable / not ready → offline', () => {
    expect(resolveRowStatus({ ...base, probe: probe({ reach: 'offline', presence: null }) }).kind).toBe('offline')
  })

  it('no login held → "Sign in" instead of offline, even with no probe', () => {
    const s = resolveRowStatus({ ...base, hasLogin: false, probe: undefined })
    expect(s.kind).toBe('sign-in')
    expect(s.label).toBe('Sign in')
    expect(resolveRowStatus({ ...base, hasLogin: false, probe: probe({ reach: 'offline' }) }).kind).toBe('sign-in')
  })

  it('the server refused the saved login → "Sign in"', () => {
    expect(resolveRowStatus({ ...base, probe: probe({ auth: 'rejected', presence: null }) }).kind).toBe('sign-in')
  })

  it('not probed yet → checking; not a saved server → offline', () => {
    expect(resolveRowStatus({ ...base, probe: undefined }).kind).toBe('checking')
    expect(resolveRowStatus({ ...base, saved: false, probe: undefined }).kind).toBe('offline')
  })

  it('live on an older server with no summary → live, nobody shown', () => {
    const s = resolveRowStatus({ ...base, probe: probe({ presence: null }) })
    expect(s.kind).toBe('live')
    expect(s.people).toEqual([])
  })
})

describe('resolveRowStatus — the connected server', () => {
  const roster: RosterUser[] = [
    { user: 'owner', role: 'owner', windowCount: 1, workspaces: ['/srv/bee'], grantedEdit: false, connectedAt: 1 },
    { user: 'anna', role: 'member', windowCount: 1, workspaces: ['/srv/bee/sub'], grantedEdit: false, connectedAt: 1 },
    { user: 'zoe', role: 'viewer', windowCount: 1, workspaces: ['/srv/other'], grantedEdit: false, connectedAt: 1 },
  ]
  const base = {
    where: 'connected' as const,
    row,
    workspace: { id: 'pb', path: '/srv/bee' },
    roster,
    rosterSupported: true,
    self: 'owner',
  }

  it('shows working / idle from the activity store, presence from the live roster', () => {
    const w = resolveRowStatus({ ...base, activity: 'working' })
    expect(w.kind).toBe('working')
    expect(w.people.map((p) => p.user)).toEqual(['anna'])
    expect(resolveRowStatus({ ...base, activity: 'idle' }).kind).toBe('idle')
    expect(resolveRowStatus({ ...base, activity: 'permission' }).label).toBe('Needs you')
  })

  it('never offline; a missing workspace is not-found', () => {
    expect(resolveRowStatus({ ...base, workspace: null, activity: 'idle' }).kind).toBe('not-found')
  })

  it('presence unsupported → nobody shown', () => {
    expect(resolveRowStatus({ ...base, rosterSupported: false, activity: 'idle' }).people).toEqual([])
  })
})

describe('presenceForRow', () => {
  it('matches by id, then by handle, else nobody', () => {
    expect(presenceForRow(summary, { address: 'renamed::b.k2.dev', workspaceId: 'pb' }, 'me')).toHaveLength(2)
    expect(presenceForRow(summary, { address: 'zed::b.k2.dev', workspaceId: null }, 'me').map((p) => p.user)).toEqual(['x'])
    expect(presenceForRow(summary, { address: 'nope::b.k2.dev', workspaceId: 'p404' }, 'me')).toEqual([])
    expect(presenceForRow(null, row, 'me')).toEqual([])
  })

  it('initials', () => {
    expect(personInitials('anna')).toBe('A')
    expect(personInitials('Rosson Long')).toBe('RL')
    expect(personInitials('sum_member')).toBe('SM')
    expect(personInitials('  ')).toBe('?')
  })
})

describe('probeHost', () => {
  const creds = { base: 'https://b.k2.dev', token: 'tok' }

  function deps(boot: unknown, fetchImpl: ProbeDeps['fetch']): ProbeDeps {
    return { bootStatus: vi.fn(async () => boot as { phase?: unknown }), fetch: fetchImpl, now: () => 42 }
  }

  it('ready + login → reads the summary with that server’s token', async () => {
    const f = vi.fn(async () => new Response(JSON.stringify({ online: 3, workspaces: summary }), { status: 200 }))
    const p = await probeHost(creds, deps({ phase: 'ready' }, f as unknown as typeof fetch))
    expect(p).toEqual({ reach: 'live', auth: 'ok', presence: summary, at: 42 })
    expect(f).toHaveBeenCalledTimes(1)
    const firstCall = f.mock.calls[0] as unknown as [string]
    expect(firstCall[0]).toBe('https://b.k2.dev/cli/presence/summary?token=tok')
  })

  it('dark-tunnel body {ready:true} counts as ready', async () => {
    const f = vi.fn(async () => new Response(JSON.stringify({ workspaces: [] }), { status: 200 }))
    const p = await probeHost(creds, deps({ ready: true, version: '0.41.6' }, f as unknown as typeof fetch))
    expect(p.reach).toBe('live')
    expect(p.presence).toEqual([])
  })

  it('unreachable → offline, and the summary is never asked', async () => {
    const f = vi.fn()
    const p = await probeHost(creds, deps(null, f as unknown as typeof fetch))
    expect(p).toEqual({ reach: 'offline', auth: 'ok', presence: null, at: 42 })
    expect(f).not.toHaveBeenCalled()
  })

  it('no token → live, auth none, no summary call', async () => {
    const f = vi.fn()
    const p = await probeHost({ base: creds.base, token: '' }, deps({ phase: 'ready' }, f as unknown as typeof fetch))
    expect(p).toEqual({ reach: 'live', auth: 'none', presence: null, at: 42 })
    expect(f).not.toHaveBeenCalled()
  })

  it('401 / 403 → the saved login was refused', async () => {
    for (const status of [401, 403]) {
      const f = vi.fn(async () => new Response('{"error":"invalid or missing token"}', { status }))
      const p = await probeHost(creds, deps({ phase: 'ready' }, f as unknown as typeof fetch))
      expect(p.auth).toBe('rejected')
      expect(p.reach).toBe('live')
    }
  })

  it('404 (older server) → live, no presence', async () => {
    const f = vi.fn(async () => new Response('{"error":"route not found"}', { status: 404 }))
    const p = await probeHost(creds, deps({ phase: 'ready' }, f as unknown as typeof fetch))
    expect(p).toEqual({ reach: 'live', auth: 'ok', presence: null, at: 42 })
  })
})
