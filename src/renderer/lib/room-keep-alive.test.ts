// Home M4 — projects/activate keep-alive (MS39, GH#22): the pool's
// per-(server, project) dedupe, and the room cadence (open, focus, input
// after an idle hour, hourly while hot).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createHostPool, KEEP_ALIVE_DEDUPE_MS, type HostPool } from './host-pool'
import { createLoginCoordinator } from './host-login-coord'
import { createRoomKeepAlive, KEEP_ALIVE_HOT_INTERVAL_MS, KEEP_ALIVE_IDLE_MS } from './room-keep-alive'
import type { ConnectHost } from '@/stores/connect-host'

const MIN = 60_000

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: 'b.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: true,
  lastConnectedAt: null,
}

let now: number
let posts: string[]
let failNext: boolean
let gate: Promise<void> | null
let instanceId: string
let pool: HostPool

function makePool(): HostPool {
  const mem = new Map<string, string>()
  return createHostPool({
    hosts: () => [B],
    windowHostKey: () => 'local',
    localCreds: async () => {
      throw new Error('no local daemon in this test')
    },
    http: async () => {
      throw new Error('http is not part of this test')
    },
    bootStatus: async () => ({ phase: 'ready', version: '0.41.6', protocol: 1, instanceId }),
    resolvePassword: async () => null,
    login: async () => {
      throw new Error('login is not part of this test')
    },
    dropSessionInMemory: () => {
      throw new Error('dropSessionInMemory is not part of this test')
    },
    coord: createLoginCoordinator({
      storage: { getItem: (k) => mem.get(k) ?? null, setItem: (k, v) => void mem.set(k, v), removeItem: (k) => void mem.delete(k) },
      now: () => now,
      windowId: 'w1',
      settle: async () => {},
    }),
    noteVersion: () => {},
    activate: async (hostKey, projectId) => {
      if (gate) await gate
      posts.push(`${hostKey} ${projectId}`)
      if (failNext) {
        failNext = false
        throw new Error('503 from the server')
      }
    },
    now: () => now,
  })
}

beforeEach(() => {
  now = 1_000_000
  posts = []
  failNext = false
  gate = null
  instanceId = 'inst-1'
  pool = makePool()
})

describe('hostPool.keepRoomAlive', () => {
  it('posts on that server, then dedupes per (server, project) for 10 min', async () => {
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('deduped')
    // Another project on the same server, and the same id on another server,
    // are separate keys.
    expect(await pool.keepRoomAlive('b.k2.dev', 'p2')).toBe('sent')
    expect(await pool.keepRoomAlive('c.k2.dev', 'p1')).toBe('sent')
    expect(posts).toEqual(['b.k2.dev p1', 'b.k2.dev p2', 'c.k2.dev p1'])
    now += KEEP_ALIVE_DEDUPE_MS - 1
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('deduped')
    now += 1
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    expect(posts.filter((p) => p === 'b.k2.dev p1')).toHaveLength(2)
  })

  it('is single-flight: calls while one is in flight post once', async () => {
    let release!: () => void
    gate = new Promise<void>((r) => {
      release = r
    })
    const a = pool.keepRoomAlive('b.k2.dev', 'p1')
    const b = pool.keepRoomAlive('b.k2.dev', 'p1')
    release()
    expect(await a).toBe('sent')
    expect(await b).toBe('deduped')
    expect(posts).toEqual(['b.k2.dev p1'])
  })

  it('a failed post is not remembered: the next call tries again', async () => {
    failNext = true
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('failed')
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    expect(posts).toEqual(['b.k2.dev p1', 'b.k2.dev p1'])
  })

  it('a restart of that server (new instanceId) re-sends; another server keeps its dedupe', async () => {
    await pool.check('b.k2.dev', { bootOnly: true })
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    expect(await pool.keepRoomAlive('c.k2.dev', 'p1')).toBe('sent')
    instanceId = 'inst-2'
    await pool.check('b.k2.dev', { bootOnly: true })
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    expect(await pool.keepRoomAlive('c.k2.dev', 'p1')).toBe('deduped')
  })

  it('forget() drops that server’s dedupe', async () => {
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
    pool.forget('b.k2.dev')
    expect(await pool.keepRoomAlive('b.k2.dev', 'p1')).toBe('sent')
  })

  it('an empty project id posts nothing', async () => {
    expect(await pool.keepRoomAlive('b.k2.dev', '')).toBe('failed')
    expect(posts).toEqual([])
  })
})

describe('room keep-alive cadence', () => {
  let calls: string[]
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-01T00:00:00Z'))
    calls = []
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function make() {
    return createRoomKeepAlive({
      hostKey: 'b.k2.dev',
      projectId: 'p1',
      keepAlive: async (hostKey, projectId) => {
        calls.push(`${hostKey} ${projectId}`)
        return 'sent'
      },
      now: () => Date.now(),
      setInterval: (fn, ms) => setInterval(fn, ms),
      clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
    })
  }

  it('sends on open and on focus', async () => {
    const ka = make()
    await ka.opened()
    await ka.focused()
    expect(calls).toEqual(['b.k2.dev p1', 'b.k2.dev p1'])
    ka.dispose()
  })

  it('input sends only after an idle hour, and each input resets the clock', async () => {
    const ka = make()
    await ka.opened()
    vi.advanceTimersByTime(30 * MIN)
    expect(ka.input()).toBeNull()
    vi.advanceTimersByTime(KEEP_ALIVE_IDLE_MS - 1)
    expect(ka.input()).toBeNull()
    expect(calls).toHaveLength(1)
    vi.advanceTimersByTime(KEEP_ALIVE_IDLE_MS)
    const sent = ka.input()
    if (!sent) throw new Error('input after an idle hour must send')
    expect(await sent).toBe('sent')
    expect(calls).toHaveLength(2)
    ka.dispose()
  })

  it('re-sends hourly while hot, stops when not hot or disposed', () => {
    const ka = make()
    ka.setHot(true)
    ka.setHot(true) // idempotent: one interval
    vi.advanceTimersByTime(KEEP_ALIVE_HOT_INTERVAL_MS * 2)
    expect(calls).toHaveLength(2)
    ka.setHot(false)
    vi.advanceTimersByTime(KEEP_ALIVE_HOT_INTERVAL_MS * 3)
    expect(calls).toHaveLength(2)
    ka.setHot(true)
    vi.advanceTimersByTime(KEEP_ALIVE_HOT_INTERVAL_MS)
    expect(calls).toHaveLength(3)
    ka.dispose()
    vi.advanceTimersByTime(KEEP_ALIVE_HOT_INTERVAL_MS * 3)
    expect(calls).toHaveLength(3)
    expect(vi.getTimerCount()).toBe(0)
    expect(() => ka.opened()).toThrow(/after dispose/)
  })
})
