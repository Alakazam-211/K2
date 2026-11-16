// The real timeline's connection driver on a scripted docs socket: attach on
// the first welcome, reconnect with backoff on a drop the daemon says can be
// retried, Retry now at once, the can't-be-live card on a refusal at open,
// failed on any other end, and stop.

import { describe, expect, it } from 'vitest'
import { LiveConnectError, type LiveClose, type LiveTransport, type LiveWelcome } from '@/lib/docs-live-transport'
import { backoffMs, connectTimeline } from './connect'

const WELCOME = { root: '', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'time_position'] } as unknown as LiveWelcome

function fake(results: (LiveWelcome | LiveClose)[]) {
  const frames = new Set<(f: unknown) => void>()
  const closes = new Set<(c: LiveClose) => void>()
  const sent: Record<string, unknown>[] = []
  const calls: { prev?: number }[] = []
  let closed = false
  const t = {
    open: true,
    async connect(o: { prev?: number }) {
      calls.push({ ...(o.prev !== undefined ? { prev: o.prev } : {}) })
      const r = results.shift()
      if (!r) throw new LiveConnectError({ reason: 'network', retry: true })
      if ('reason' in r) throw new LiveConnectError(r)
      return r
    },
    send: (f: unknown) => void sent.push(f as Record<string, unknown>),
    onFrame: (cb: (f: unknown) => void) => (frames.add(cb), () => frames.delete(cb)),
    onClose: (cb: (c: LiveClose) => void) => (closes.add(cb), () => closes.delete(cb)),
    close: () => void (closed = true),
  }
  return {
    t: t as unknown as LiveTransport,
    sent,
    calls,
    drop: (c: LiveClose) => [...closes].forEach((cb) => cb(c)),
    closed: () => closed,
  }
}

function timers() {
  const q: { fn: () => void; ms: number; live: boolean }[] = []
  return {
    q,
    schedule: (fn: () => void, ms: number) => {
      const e = { fn, ms, live: true }
      q.push(e)
      return () => void (e.live = false)
    },
    fire: () => {
      const e = q.filter((x) => x.live).shift()
      if (e) {
        e.live = false
        e.fn()
      }
    },
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('connectTimeline', () => {
  it('waits on a spinner, then attaches on the first welcome and asks for the watch', async () => {
    const f = fake([WELCOME])
    const c = connectTimeline(f.t, { post: async () => undefined, schedule: timers().schedule })
    expect(c.source.timeline().state.loading?.phase).toBe('connect')
    await flush()
    expect(c.source.timeline().state.loading?.phase).toBe('welcome')
    expect(f.sent).toContainEqual({ type: 'watch' })
    expect(c.source.kind).toBe('live')
    c.stop()
    expect(f.closed()).toBe(true)
  })

  it('a drop it can retry: Reconnecting, a backoff, then the new welcome (prev: the last client) clears it', async () => {
    const f = fake([WELCOME, { ...WELCOME, client: 2 }])
    const tm = timers()
    const c = connectTimeline(f.t, { post: async () => undefined, schedule: tm.schedule })
    await flush()
    f.drop({ reason: 'network', retry: true })
    expect(c.source.timeline().state.conn).toBe('reconnecting')
    expect(tm.q.at(-1)?.ms).toBe(1000)
    tm.fire()
    await flush()
    expect(f.calls.at(-1)).toEqual({ prev: 1 })
    expect(c.source.timeline().state.conn).toBeNull()
    c.stop()
  })

  it('backs off 1, 2, 4… s, at most 30 s, and Retry now dials at once', async () => {
    expect([0, 1, 2, 3, 10].map(backoffMs)).toEqual([1000, 2000, 4000, 8000, 30000])
    const f = fake([WELCOME])
    const tm = timers()
    const c = connectTimeline(f.t, { post: async () => undefined, schedule: tm.schedule })
    await flush()
    f.drop({ reason: 'cut_off', retry: true })
    tm.fire()
    await flush()
    expect(tm.q.at(-1)?.ms).toBe(2000)
    const n = f.calls.length
    c.source.retry()
    await flush()
    expect(f.calls.length).toBe(n + 1)
    c.stop()
  })

  it('a refusal at open is the can’t-be-live card in the pack’s words, not a retry', async () => {
    const f = fake([{ reason: 'declined', retry: false, error: { id: 'MPC-E201', message: 'a collection', details: { code: 'live_many_timelines' } } as never }])
    const tm = timers()
    const c = connectTimeline(f.t, { post: async () => undefined, schedule: tm.schedule })
    await flush()
    const S = c.source.timeline().state
    expect(S.tier).toBe('declined')
    expect(S.decline?.title).toBe('This file holds more than one timeline')
    expect(tm.q.length).toBe(0)
    c.stop()
  })

  it('any other end says the connection failed', async () => {
    const f = fake([WELCOME])
    const c = connectTimeline(f.t, { post: async () => undefined, schedule: timers().schedule })
    await flush()
    f.drop({ reason: 'kicked', retry: false })
    expect(c.source.timeline().state.conn).toBe('failed')
    c.stop()
  })
})
