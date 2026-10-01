// Home M4 — room failure states (MS27, MS30, MS45, MS83): one state per
// pool entry, each with one action, never a fallback.

import { describe, it, expect } from 'vitest'
import { roomFailure, HOME_ROOM_MIN_VERSION } from './room-failure'
import type { HostEntry } from './host-pool'

function entry(patch: Partial<HostEntry> = {}): HostEntry {
  return {
    hostKey: 'dtl.k2.dev',
    saved: true,
    hostId: 'id-dtl',
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.41.6', protocol: 1, instanceId: 'i1', at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
    ...patch,
  }
}

const base = { serverLabel: 'dtl', saved: true }

describe('roomFailure', () => {
  it('a live, signed-in Member on a new enough server: no failure', () => {
    expect(roomFailure({ ...base, entry: entry() })).toBeNull()
    expect(roomFailure({ ...base, entry: entry({ role: 'owner' }) })).toBeNull()
    expect(roomFailure({ ...base, entry: entry({ role: 'admin' }) })).toBeNull()
  })

  it('the read floor is 0.40.39 (MS30)', () => {
    expect(HOME_ROOM_MIN_VERSION).toBe('0.40.39')
  })

  it('B offline: says so, Retry, with the next try time; input off', () => {
    const f = roomFailure({
      ...base,
      entry: entry({ reach: 'offline', boot: null, offlineStreak: 1, checkedAt: 10_000 }),
      nextCheckAt: 15_000,
      now: 10_000,
    })
    expect(f).toEqual({
      kind: 'offline',
      title: 'dtl is offline. Retrying…',
      detail: 'Next try in 5 s.',
      action: 'retry',
      actionLabel: 'Retry',
      inputOff: true,
    })
  })

  it('B restarting (phase not ready): Retry', () => {
    const f = roomFailure({ ...base, entry: entry({ reach: 'starting' }) })
    expect(f?.kind).toBe('restarting')
    expect(f?.title).toBe('dtl is restarting…')
    expect(f?.action).toBe('retry')
  })

  it('never checked: Connecting, Retry', () => {
    expect(roomFailure({ ...base, entry: undefined })?.kind).toBe('connecting')
    expect(roomFailure({ ...base, entry: entry({ reach: 'unknown' }) })?.action).toBe('retry')
  })

  it('sign-in needed, new password, signing in: Sign in to B, with the pool’s note', () => {
    const s = roomFailure({ ...base, entry: entry({ auth: 'signin-required', authNote: 'dtl refused the saved password. Sign in again.' }) })
    expect(s).toEqual({
      kind: 'signin-required',
      title: 'Sign in to dtl.',
      detail: 'dtl refused the saved password. Sign in again.',
      action: 'sign-in',
      actionLabel: 'Sign in to dtl',
      inputOff: true,
    })
    expect(roomFailure({ ...base, entry: entry({ auth: 'rotate-required' }) })?.title).toBe('dtl needs a new password.')
    expect(roomFailure({ ...base, entry: entry({ auth: 'signing-in' }) })?.title).toBe('Signing in to dtl…')
  })

  it('kicked: Removed from B. Sign in again. — a click, never automatic', () => {
    const f = roomFailure({ ...base, entry: entry({ auth: 'kicked' }) })
    expect(f?.kind).toBe('kicked')
    expect(f?.title).toBe('Removed from dtl. Sign in again.')
    expect(f?.action).toBe('sign-in')
  })

  it('version too old: names both versions; Open B’s server', () => {
    const f = roomFailure({
      ...base,
      entry: entry({ boot: { phase: 'ready', ready: true, version: '0.40.30', protocol: 1, instanceId: 'i', at: 1 } }),
    })
    expect(f).toEqual({
      kind: 'version-too-old',
      title: 'dtl is on v0.40.30. This room needs v0.40.39 or newer.',
      detail: 'Update dtl, or open its server in this window.',
      action: 'open-server',
      actionLabel: 'Open dtl’s server',
      inputOff: true,
    })
    // Exactly the floor opens.
    expect(
      roomFailure({
        ...base,
        entry: entry({ boot: { phase: 'ready', ready: true, version: '0.40.39', protocol: 1, instanceId: 'i', at: 1 } }),
      }),
    ).toBeNull()
    // Unknown version is too old (MS11: unknown means no).
    expect(
      roomFailure({
        ...base,
        entry: entry({ boot: { phase: 'ready', ready: true, version: null, protocol: 1, instanceId: 'i', at: 1 } }),
      })?.title,
    ).toBe('dtl is on an unknown version. This room needs v0.40.39 or newer.')
  })

  it('too old wins over a login state: the room cannot open either way', () => {
    const f = roomFailure({
      ...base,
      entry: entry({ auth: 'signin-required', boot: { phase: 'ready', ready: true, version: '0.40.1', protocol: 1, instanceId: 'i', at: 1 } }),
    })
    expect(f?.kind).toBe('version-too-old')
  })

  it('no access: a Viewer or an unknown role (MS83); Open B’s server', () => {
    const v = roomFailure({ ...base, entry: entry({ role: 'viewer' }) })
    expect(v?.kind).toBe('no-access')
    expect(v?.title).toBe('You don’t have access to this agent on dtl.')
    expect(v?.action).toBe('open-server')
    expect(roomFailure({ ...base, entry: entry({ role: 'guest' }) })?.kind).toBe('no-access')
  })

  it('offline wins over a login state: a down server cannot check a login', () => {
    expect(roomFailure({ ...base, entry: entry({ reach: 'offline', auth: 'kicked' }) })?.kind).toBe('offline')
  })

  it('a removed server: says so and offers nothing to fall back to', () => {
    const f = roomFailure({ serverLabel: 'dtl', saved: false, entry: entry() })
    expect(f).toEqual({
      kind: 'removed',
      title: 'This server was removed from this computer.',
      detail: null,
      action: null,
      actionLabel: null,
      inputOff: true,
    })
  })
})
