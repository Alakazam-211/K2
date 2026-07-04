// Presence S3 — kick-button logic coverage, no DOM harness: the pure
// `canKick` matrix (client-side mirror of the daemon's `handle_kick`
// gate) plus the whoami single-flight cache (one fetch shared across the
// modal's row buttons; host switch invalidates).
//
// Boundary mocks follow presence.test.ts: daemon-cli is mocked BEFORE the
// module import (the component wires `onActiveHostChange` at module
// scope); the connect-host store is REAL so invalidation is proven
// through the actual seam.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// Tauri core — connect-host imports `invoke` at module scope.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

const cli = vi.hoisted(() => ({
  whoami: { role: 'owner', owner: true } as unknown,
  getCalls: [] as string[],
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    cli.getCalls.push(route)
    return cli.whoami
  }),
  daemonCliPost: vi.fn(async () => ({ success: true })),
}))

import {
  canKick,
  fetchViewerRole,
  __resetWhoamiCacheForTests,
} from './PresenceKickButton'
import { daemonCliGet } from '@/lib/daemon-cli'

describe('canKick — the client-side kick matrix (PRD §4)', () => {
  it('owner kicks any non-owner row', () => {
    expect(canKick('owner', 'admin')).toBe(true)
    expect(canKick('owner', 'member')).toBe(true)
    expect(canKick('owner', 'viewer')).toBe(true)
  })

  it('nobody kicks the owner row', () => {
    expect(canKick('owner', 'owner')).toBe(false)
    expect(canKick('admin', 'owner')).toBe(false)
    expect(canKick('member', 'owner')).toBe(false)
  })

  it('admin kicks member/viewer rows only — never fellow admins', () => {
    expect(canKick('admin', 'member')).toBe(true)
    expect(canKick('admin', 'viewer')).toBe(true)
    expect(canKick('admin', 'admin')).toBe(false)
  })

  it('member / viewer / unresolved viewers kick nobody', () => {
    expect(canKick('member', 'member')).toBe(false)
    expect(canKick('viewer', 'member')).toBe(false)
    expect(canKick(null, 'member')).toBe(false)
  })
})

describe('fetchViewerRole — whoami single-flight cache', () => {
  beforeEach(() => {
    __resetWhoamiCacheForTests()
    cli.getCalls.length = 0
    cli.whoami = { role: 'owner', owner: true }
    vi.mocked(daemonCliGet).mockClear()
  })

  it('all row buttons opening together share ONE whoami fetch', async () => {
    const [a, b, c] = await Promise.all([
      fetchViewerRole(),
      fetchViewerRole(),
      fetchViewerRole(),
    ])
    expect([a, b, c]).toEqual(['owner', 'owner', 'owner'])
    expect(cli.getCalls).toEqual(['auth/whoami'])
  })

  it('cache invalidation forces a fresh fetch (the host-switch path)', async () => {
    expect(await fetchViewerRole()).toBe('owner')
    cli.whoami = { role: 'member' }
    // Same TTL window → still the cached owner.
    expect(await fetchViewerRole()).toBe('owner')
    __resetWhoamiCacheForTests()
    expect(await fetchViewerRole()).toBe('member')
    expect(cli.getCalls).toEqual(['auth/whoami', 'auth/whoami'])
  })

  it('a pre-roles daemon (no role field) resolves owner:true → owner, else null', async () => {
    cli.whoami = { owner: true }
    expect(await fetchViewerRole()).toBe('owner')
    __resetWhoamiCacheForTests()
    cli.whoami = { owner: false }
    expect(await fetchViewerRole()).toBeNull()
  })

  it('a whoami failure resolves null (button hides; daemon still enforces)', async () => {
    vi.mocked(daemonCliGet).mockRejectedValueOnce(new Error('boom'))
    expect(await fetchViewerRole()).toBeNull()
  })
})
