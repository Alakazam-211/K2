// homes-sync-v1 — hostKey ⇄ serverId at the sync edge.

import { describe, it, expect, beforeEach } from 'vitest'
import {
  fromWireAddress,
  hostKeyForServerId,
  isServerId,
  noteServerId,
  noteServerMap,
  resetServerIdsForTests,
  serverIdForHost,
  toWireAddress,
} from './server-ids'

const LAPTOP = '1111111111111111'
const Z = '2222222222222222'

describe('server-ids', () => {
  beforeEach(() => resetServerIdsForTests())

  it('notes ids from /boot-status and ignores junk', () => {
    expect(isServerId(LAPTOP)).toBe(true)
    expect(isServerId('ABCDEF0123456789')).toBe(false)
    noteServerId('local', LAPTOP)
    noteServerId('box.lan:38471', 'not-an-id')
    noteServerId('old.k2.dev', undefined)
    expect(serverIdForHost('local')).toBe(LAPTOP)
    expect(serverIdForHost('box.lan:38471')).toBeNull()
    expect(serverIdForHost('old.k2.dev')).toBeNull()
  })

  it('the laptop’s local and its own tunnel host are one server: local wins on the way back', () => {
    noteServerId('local', LAPTOP)
    noteServerId('rosson.k2.dev', LAPTOP)
    expect(toWireAddress('atlas::local')).toBe(`atlas::${LAPTOP}`)
    expect(toWireAddress('atlas::rosson.k2.dev')).toBe(`atlas::${LAPTOP}`)
    expect(fromWireAddress(`atlas::${LAPTOP}`)).toBe('atlas::local')
  })

  it('migration: a host-key row goes on the wire by id once the id is known; until then by host key', () => {
    expect(toWireAddress('pi::z3mbpz.k2.dev')).toBe('pi::z3mbpz.k2.dev')
    noteServerId('z3mbpz.k2.dev', Z)
    expect(toWireAddress('pi::z3mbpz.k2.dev')).toBe(`pi::${Z}`)
    expect(fromWireAddress(`pi::${Z}`)).toBe('pi::z3mbpz.k2.dev')
    // An unknown id (a server this device hasn't saved) stays as is, and round-trips.
    expect(fromWireAddress('pi::3333333333333333')).toBe('pi::3333333333333333')
    expect(toWireAddress('pi::3333333333333333')).toBe('pi::3333333333333333')
    // Only saved hosts are used on the way back.
    expect(fromWireAddress(`pi::${Z}`, () => false)).toBe(`pi::${Z}`)
  })

  it('a home server’s servers map fills gaps but never overrides a host’s own answer', () => {
    noteServerId('z3mbpz.k2.dev', Z)
    noteServerMap({ 'z3mbpz.k2.dev': '9999999999999999', 'nsi.k2.dev': '4444444444444444', bad: 'x' })
    expect(serverIdForHost('z3mbpz.k2.dev')).toBe(Z)
    expect(serverIdForHost('nsi.k2.dev')).toBe('4444444444444444')
    expect(serverIdForHost('bad')).toBeNull()
    expect(hostKeyForServerId('4444444444444444')).toBe('nsi.k2.dev')
  })

  it('a migrated box (new identity key) keeps resolving its old id to the host, so rows move to the new id', () => {
    noteServerId('box.k2.dev', Z)
    noteServerId('box.k2.dev', '5555555555555555')
    expect(fromWireAddress(`pi::${Z}`)).toBe('pi::box.k2.dev')
    expect(toWireAddress('pi::box.k2.dev')).toBe('pi::5555555555555555')
  })
})
