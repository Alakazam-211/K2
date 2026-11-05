// The role gate behind Settings → General's Update and Restart rows
// (Rosson 2026-11-05: "it often leaves the button greyed out and says you
// need to be an owner or an admin, even though I am logged in as an owner
// or an admin"). Only a role the host REPORTED below Admin refuses.
import { describe, expect, it } from 'vitest'
import {
  hostActionGate,
  isNotSentError,
  refusalCopy,
  roleFromWhoami,
  roleRetryDelayMs,
  roleStateFrom,
  shouldRecheckRole,
  type HostRoleState,
} from './host-role'

function named(name: string, message = name): Error {
  const e = new Error(message)
  e.name = name
  return e
}

function withStatus(message: string, status: number): Error {
  return Object.assign(new Error(message), { status })
}

describe('roleFromWhoami', () => {
  it('reads the role the daemon names', () => {
    expect(roleFromWhoami({ username: 'rosson', owner: false, role: 'admin' })).toBe('admin')
    expect(roleFromWhoami({ username: null, owner: true, role: 'owner' })).toBe('owner')
    expect(roleFromWhoami({ username: 'm', owner: false, role: 'member' })).toBe('member')
  })
  it('an older host without a role: owner:true is the owner', () => {
    expect(roleFromWhoami({ owner: true })).toBe('owner')
  })
  it('a role this app does not know is below the admin tier', () => {
    expect(roleFromWhoami({ role: 'viewer' })).toBe('member')
  })
  it('no role and not the owner is not a whoami answer', () => {
    expect(roleFromWhoami({ owner: false })).toBeNull()
    expect(roleFromWhoami('<html>')).toBeNull()
    expect(roleFromWhoami(null)).toBeNull()
  })
})

describe('roleStateFrom: one whoami attempt', () => {
  it('a request the gate never sent (connection recovering, host switched) is still checking', () => {
    const recovering = named('RecoveringError', 'host "Rosson" is reconnecting — request skipped until recovery completes')
    expect(isNotSentError(recovering)).toBe(true)
    expect(roleStateFrom({ err: recovering })).toEqual({ kind: 'checking' })
    expect(roleStateFrom({ err: named('HostSwitchedError') })).toEqual({ kind: 'checking' })
  })
  it('a 401/403 on whoami is a login the host no longer accepts', () => {
    expect(roleStateFrom({ err: withStatus('invalid or missing token', 403) })).toEqual({
      kind: 'signed-out',
      reason: 'invalid or missing token',
    })
    expect(roleStateFrom({ err: withStatus('unauthorized', 401) }).kind).toBe('signed-out')
  })
  it('a network failure or 5xx is unreadable, never a role', () => {
    expect(roleStateFrom({ err: new TypeError('Load failed') })).toEqual({ kind: 'unreadable', reason: 'Load failed' })
    expect(roleStateFrom({ err: withStatus('daemon starting', 503) }).kind).toBe('unreadable')
  })
  it('an answer without a role is unreadable', () => {
    expect(roleStateFrom({ body: '<html>' })).toEqual({ kind: 'unreadable', reason: 'its reply did not name a role' })
  })
  it('an answer with a role is known', () => {
    expect(roleStateFrom({ body: { role: 'owner' } })).toEqual({ kind: 'known', role: 'owner' })
  })
})

describe('re-reading', () => {
  it('backs off 1 s, 3 s, 10 s, then every 30 s', () => {
    expect([0, 1, 2, 3, 4, 9].map(roleRetryDelayMs)).toEqual([1000, 3000, 10000, 30000, 30000, 30000])
  })
  it('reads again until a role is known', () => {
    expect(shouldRecheckRole({ kind: 'checking' })).toBe(true)
    expect(shouldRecheckRole({ kind: 'unreadable', reason: 'x' })).toBe(true)
    expect(shouldRecheckRole({ kind: 'signed-out', reason: 'x' })).toBe(true)
    expect(shouldRecheckRole({ kind: 'known', role: 'member' })).toBe(false)
  })
})

describe('hostActionGate', () => {
  const gate = (role: HostRoleState, over: Partial<{ isRemote: boolean; supported: boolean }> = {}) =>
    hostActionGate({ isRemote: true, supported: true, role, hostLabel: 'Rosson', action: 'update', ...over })

  it('this computer, or a server without the route: hidden', () => {
    expect(gate({ kind: 'known', role: 'owner' }, { isRemote: false }).show).toBe(false)
    expect(gate({ kind: 'known', role: 'owner' }, { supported: false }).show).toBe(false)
  })
  it('owner and admin: enabled, nothing to explain', () => {
    for (const role of ['owner', 'admin'] as const) {
      expect(gate({ kind: 'known', role })).toEqual({ show: true, enabled: true, note: null, title: undefined })
    }
  })
  it('not read yet: off for the moment, and never the owner/admin refusal', () => {
    const g = gate({ kind: 'checking' })
    expect(g.show).toBe(true)
    expect(g.enabled).toBe(false)
    expect(g.note).toBeNull()
    expect(g.title).toBe('Checking your access on Rosson…')
    expect(g.title).not.toMatch(/owner|admin/i)
  })
  it('a Member: off, and says so on the row', () => {
    const g = gate({ kind: 'known', role: 'member' })
    expect(g.enabled).toBe(false)
    expect(g.note).toBe("You're signed in to Rosson as a Member. Only the host owner or an admin can update it.")
  })
  it('an expired sign-in: off, and says sign in again (not "owner or admin")', () => {
    const g = gate({ kind: 'signed-out', reason: 'invalid or missing token' })
    expect(g.enabled).toBe(false)
    expect(g.note).toBe('Your sign-in to Rosson has expired. Sign in to Rosson again to update it.')
  })
  it('unreadable: stays on (the host decides) with a note', () => {
    const g = gate({ kind: 'unreadable', reason: 'Load failed' })
    expect(g.enabled).toBe(true)
    expect(g.note).toBe("Couldn't read your access on Rosson (Load failed). Still checking…")
  })
})

describe('refusalCopy: a 403 from an update route, read against whoami', () => {
  const msg = 'invalid or missing token'
  it('whoami says Member: the Member copy', () => {
    expect(refusalCopy('Rosson', 'update', msg, { kind: 'known', role: 'member' })).toBe(
      "You're signed in to Rosson as a Member. Only the host owner or an admin can update it.",
    )
  })
  it('whoami refuses the login too: sign in again', () => {
    expect(refusalCopy('Rosson', 'restart', msg, { kind: 'signed-out', reason: msg })).toBe(
      'Your sign-in to Rosson has expired. Sign in to Rosson again to restart it.',
    )
  })
  it('whoami says owner/admin: never "only the owner or an admin"', () => {
    const owner = refusalCopy('Rosson', 'update', msg, { kind: 'known', role: 'owner' })
    expect(owner).toContain("although you're the owner there")
    expect(owner).not.toMatch(/only the host owner/i)
    expect(refusalCopy('Rosson', 'update', msg, { kind: 'known', role: 'admin' })).toContain("you're an Admin there")
  })
  it('whoami unreadable: quotes the host, no role claim', () => {
    expect(refusalCopy('Rosson', 'update', msg, { kind: 'unreadable', reason: 'x' })).toBe(
      "Rosson turned down the update: invalid or missing token. Couldn't confirm your access there right now.",
    )
  })
})
