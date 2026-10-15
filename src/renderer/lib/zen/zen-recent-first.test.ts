// @vitest-environment jsdom
//
// "Recent agents first" in Gardens (0.45.6, ported from K2 Companion's
// `recentFirst.test.ts`): the order, the cap, the per-computer setting (off
// by default), and that sends are recorded while it's off.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ZEN_RECENT_FIRST_DEFAULT,
  ZEN_RECENT_FIRST_KEY,
  ZEN_RECENT_SENT_KEY,
  ZEN_RECENT_SENT_MAX,
  orderZenRecentFirst,
  readZenRecentFirst,
  readZenRecentSent,
  trimZenRecentSent,
  useZenRecentFirstStore,
  zenRecentKey,
} from './zen-recent-first'

interface Row {
  address: string
  pinned?: boolean
}

const rows = (...addrs: string[]): Row[] => addrs.map((address) => ({ address }))
const key = (r: Row): string | null => zenRecentKey(r.address)
const order = (list: Row[]): string[] => list.map((r) => r.address)

beforeEach(() => {
  localStorage.clear()
  useZenRecentFirstStore.getState().reload()
})

afterEach(() => {
  vi.restoreAllMocks()
  localStorage.clear()
})

describe('orderZenRecentFirst', () => {
  it('puts the newest sends first and keeps every other row in its own order', () => {
    const list = rows('a::local', 'b::local', 'c::local', 'd::local', 'e::local')
    const sent = { 'd::local': 300, 'b::local': 100, 'e::local': 200 }
    expect(order(orderZenRecentFirst(list, sent, key))).toEqual(['d::local', 'e::local', 'b::local', 'a::local', 'c::local'])
  })

  it('keeps pinned rows first, in their order, even when never sent to', () => {
    const list: Row[] = [
      { address: 'a::local' },
      { address: 'p2::local', pinned: true },
      { address: 'b::local' },
      { address: 'p1::local', pinned: true },
    ]
    const sent = { 'b::local': 5, 'p1::local': 999 }
    expect(order(orderZenRecentFirst(list, sent, key, (r) => r.pinned === true))).toEqual([
      'p2::local',
      'p1::local',
      'b::local',
      'a::local',
    ])
  })

  it('ties on the same time keep the original order', () => {
    const list = rows('a::local', 'b::local', 'c::local')
    expect(order(orderZenRecentFirst(list, { 'c::local': 7, 'b::local': 7 }, key))).toEqual(['b::local', 'c::local', 'a::local'])
  })

  it('keys by host + handle: the same handle on two servers is two rows', () => {
    const list = rows('cortana::local', 'cortana::box.k2.dev')
    expect(order(orderZenRecentFirst(list, { 'cortana::box.k2.dev': 1 }, key))).toEqual(['cortana::box.k2.dev', 'cortana::local'])
    expect(zenRecentKey('  Cortana::Box.K2.dev ')).toBe('cortana::box.k2.dev')
    expect(zenRecentKey('')).toBeNull()
  })
})

describe('trimZenRecentSent', () => {
  it('keeps the newest 200', () => {
    const sent: Record<string, number> = {}
    for (let i = 0; i < ZEN_RECENT_SENT_MAX + 5; i++) sent[`a${i}::local`] = i
    const kept = trimZenRecentSent(sent)
    expect(ZEN_RECENT_SENT_MAX).toBe(200)
    expect(Object.keys(kept)).toHaveLength(200)
    for (let i = 0; i < 5; i++) expect(kept[`a${i}::local`]).toBeUndefined()
    expect(kept[`a${ZEN_RECENT_SENT_MAX + 4}::local`]).toBe(ZEN_RECENT_SENT_MAX + 4)
  })
})

describe('the setting (this computer)', () => {
  it('is off by default', () => {
    expect(ZEN_RECENT_FIRST_DEFAULT).toBe(false)
    expect(readZenRecentFirst()).toBe(false)
    expect(useZenRecentFirstStore.getState().on).toBe(false)
  })

  it('turns on and off, and is kept in localStorage', () => {
    useZenRecentFirstStore.getState().setOn(true)
    expect(useZenRecentFirstStore.getState().on).toBe(true)
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBe('1')
    useZenRecentFirstStore.getState().reload()
    expect(useZenRecentFirstStore.getState().on).toBe(true)
    useZenRecentFirstStore.getState().setOn(false)
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBe('0')
    expect(readZenRecentFirst()).toBe(false)
  })

  it('records sends while it is off, lowercased, capped', () => {
    expect(useZenRecentFirstStore.getState().on).toBe(false)
    useZenRecentFirstStore.getState().markSent('Cortana::Local', 42)
    expect(useZenRecentFirstStore.getState().sent).toEqual({ 'cortana::local': 42 })
    expect(readZenRecentSent()).toEqual({ 'cortana::local': 42 })
    useZenRecentFirstStore.getState().markSent('', 43)
    expect(readZenRecentSent()).toEqual({ 'cortana::local': 42 })
  })

  it('a garbage stored value reads as empty', () => {
    localStorage.setItem(ZEN_RECENT_SENT_KEY, '[1,2]')
    expect(readZenRecentSent()).toEqual({})
    localStorage.setItem(ZEN_RECENT_SENT_KEY, '{"a::local": "x", "b::local": 3}')
    expect(readZenRecentSent()).toEqual({ 'b::local': 3 })
  })

  it('storage that throws keeps the choice in memory', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError')
    })
    useZenRecentFirstStore.getState().setOn(true)
    useZenRecentFirstStore.getState().markSent('a::local', 1)
    expect(useZenRecentFirstStore.getState().on).toBe(true)
    expect(useZenRecentFirstStore.getState().sent).toEqual({ 'a::local': 1 })
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBeNull()
  })
})
