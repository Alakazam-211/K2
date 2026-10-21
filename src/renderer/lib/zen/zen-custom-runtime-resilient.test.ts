// 0.46.1 (catalog 11): the frame runtime's typed errors, handler-object
// subscriptions that re-subscribe on their own, shape guards and bad_args.
// The real k2-frame.js text evaluated in a fresh realm with a fake window, a
// real MessageChannel and timers the test runs by hand.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { createContext, runInContext } from 'node:vm'
import { MessageChannel, type MessagePort } from 'node:worker_threads'
import { afterEach, describe, expect, it } from 'vitest'
import { k2FrameScript } from './zen-custom-prelude'

const catalogJson = JSON.parse(readFileSync(resolve(__dirname, '../../../../crates/k2-core/src/contract/catalog.json'), 'utf8')) as unknown

type Listener = (e: unknown) => void

interface Realm {
  k2: Record<string, any>
  fire(type: string, e: unknown): void
  parent: object
  /** Run the timers due now (in order), returning their delays. */
  runTimers(): number[]
  pending(): number
  /** Every `documentElement.style.setProperty` the runtime made. */
  rootStyle(): Array<[string, string, string | undefined]>
  /** The root's attributes (the prelude's scheme script sets them). */
  rootAttrs: Record<string, string>
}

const ports: MessagePort[] = []

function realm(): Realm {
  const listeners = new Map<string, Listener[]>()
  const rootStyle: Array<[string, string, string | undefined]> = []
  const rootAttrs: Record<string, string> = {}
  let timers: Array<{ id: number; f: () => void; ms: number }> = []
  let nextTimer = 1
  const parent = {}
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: (f: () => void, ms: number) => {
      const id = nextTimer++
      timers.push({ id, f, ms: ms ?? 0 })
      return id
    },
    clearTimeout: (id: number) => {
      timers = timers.filter((t) => t.id !== id)
    },
    parent,
    navigator: { platform: 'MacIntel' },
    document: {
      documentElement: {
        style: { setProperty: (k: string, v: string, p?: string) => rootStyle.push([k, v, p]) },
        setAttribute: (k: string, v: string) => void (rootAttrs[k] = v),
        getAttribute: (k: string) => rootAttrs[k] ?? null,
      },
    },
    console,
  }
  g.self = g
  runInContext(k2FrameScript(), createContext(g))
  return {
    k2: g.k2 as Record<string, any>,
    fire: (type, e) => {
      for (const f of listeners.get(type) ?? []) f(e)
    },
    parent,
    runTimers: () => {
      // Timer 1 is the runtime's 10 s hello timeout; the test never fires it.
      const due = timers.filter((t) => t.id !== 1)
      timers = timers.filter((t) => t.id === 1)
      for (const t of due) t.f()
      return due.map((t) => t.ms)
    },
    pending: () => timers.filter((t) => t.id !== 1).length,
    rootStyle: () => rootStyle,
    rootAttrs,
  }
}

function connect(r: Realm): { host: MessagePort; heard: Array<Record<string, any>> } {
  const ch = new MessageChannel()
  ports.push(ch.port1, ch.port2)
  const heard: Array<Record<string, any>> = []
  ch.port1.on('message', (m) => heard.push(m as Record<string, any>))
  r.fire('message', {
    source: r.parent,
    data: { k2: 'hello', v: 1, caps: ['sources:read', 'store:read'], features: ['zen-v1'], widget: { id: 'w', name: 'W', garden: 'g' }, config: {} },
    ports: [ch.port2],
  })
  return { host: ch.port1, heard }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 15))
const subsOf = (heard: Array<Record<string, any>>, verb: string): number[] =>
  heard.filter((m) => m.verb === verb && typeof m.sub === 'number').map((m) => m.sub as number)

const VIEW = {
  name: 'k2-ops-boxes',
  label: 'Boxes',
  health: 'warn',
  phase: 'live',
  display: 'warn',
  fields: { target_version: '0.46.0' },
  items: [{ key: 'ohana', health: 'warn', fields: { version: '0.44.4' }, soft: { count: 1, of: 3, level: 'crit' } }],
  counts: { ok: 0, warn: 1, crit: 0, unknown: 0 },
  soft: { count: 0, of: 3 },
  serverNow: 1000,
  updatedAt: 990,
}

afterEach(() => {
  for (const p of ports.splice(0)) p.close()
})

describe('0.46.1: typed errors', () => {
  it('K2Error carries retryable from the catalog; k2.retryable lists the codes', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const retry = (catalogJson as { errors: Array<{ code: string; retryable?: boolean }> }).errors.filter((e) => e.retryable).map((e) => e.code)
    expect(retry).toEqual(['not_bound', 'rate_limited', 'unreachable', 'busy'])
    expect([...r.k2.retryable]).toEqual(retry)
    const p = r.k2.sources.read('fleet')
    await tick()
    host.postMessage({ id: heard.find((m) => m.verb === 'sources.read')!.id, ok: false, error: { code: 'unreachable', message: 'down', last: VIEW } })
    const err = await p.then(
      () => {
        throw new Error('resolved')
      },
      (e: any) => e,
    )
    expect(err).toBeInstanceOf(r.k2.K2Error)
    expect([err.name, err.code, err.retryable, err.verb]).toEqual(['K2Error', 'unreachable', true, 'sources.read'])
    expect(err.last.name).toBe('k2-ops-boxes')
    const p2 = r.k2.agents.list()
    await tick()
    host.postMessage({ id: heard.find((m) => m.verb === 'agents.list')!.id, ok: false, error: { code: 'cap_not_granted', message: 'no' } })
    expect((await p2.catch((e: any) => e)).retryable).toBe(false)
  })

  it('bad_args: arguments that aren’t plain data are refused in the frame and never sent', async () => {
    const r = realm()
    const { heard } = connect(r)
    await tick()
    const before = heard.length
    class Box {}
    const cyc: Record<string, unknown> = {}
    cyc.self = cyc
    for (const bad of [() => 1, new Box(), cyc, Number.NaN, Symbol('s')]) {
      const e = await r.k2.call('store.get', bad).catch((x: any) => x)
      expect([e.name, e.code, e.retryable]).toEqual(['K2Error', 'bad_args', false])
    }
    expect(() => r.k2.sources.subscribe(new Box(), () => undefined)).toThrow(/plain data/)
    await tick()
    expect(heard.length).toBe(before)
    // Plain data still goes.
    r.k2.call('store.get', 'a/b', { x: [1, null, 'y', { z: true }] })
    await tick()
    expect(heard.at(-1)).toMatchObject({ verb: 'store.get', args: ['a/b', { x: [1, null, 'y', { z: true }] }] })
  })
})

describe('0.46.1: handler-object subscriptions re-subscribe on their own', () => {
  it('not_bound, then the placement binds it: retries with backoff, then data (normalised), then live', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const got: unknown[] = []
    const errors: any[] = []
    const states: string[] = []
    r.k2.sources.subscribe('k2-ops-boxes', { data: (v: unknown) => got.push(v), error: (e: unknown) => errors.push(e), state: (s: string) => states.push(s) })
    await tick()
    const [s1] = subsOf(heard, 'sources.subscribe')
    expect(heard.find((m) => m.sub === s1)!.args).toEqual(['k2-ops-boxes'])
    host.postMessage({ sub: s1, error: { code: 'not_bound', message: 'not in config' } })
    await tick()
    expect(errors.map((e) => [e.code, e.retryable, e.retrying, e.attempt])).toEqual([['not_bound', true, true, 1]])
    expect(states).toEqual(['retrying'])
    expect(r.runTimers()).toEqual([1000])
    await tick()
    const [, s2] = subsOf(heard, 'sources.subscribe')
    expect(s2).toBeGreaterThan(s1)
    host.postMessage({ sub: s2, error: { code: 'not_bound', message: 'still' } })
    await tick()
    expect(r.runTimers()).toEqual([2000])
    await tick()
    const s3 = subsOf(heard, 'sources.subscribe')[2]
    host.postMessage({ sub: s3, value: VIEW })
    await tick()
    expect(got).toHaveLength(1)
    expect((got[0] as any).items[0].soft).toEqual({ count: 1, of: 3, level: 'crit' })
    expect(states).toEqual(['retrying', 'live'])
    // A later refusal starts the backoff again from 1 s.
    host.postMessage({ sub: s3, error: { code: 'unreachable', message: 'down', last: { health: 'ok', display: 'stale', items: 'nope' } } })
    await tick()
    const last = errors.at(-1)
    expect([last.code, last.attempt]).toEqual(['unreachable', 1])
    expect(last.last.items).toEqual([])
    expect(r.runTimers()).toEqual([1000])
  })

  it('backoff is capped at 30 s and retries keep going while the code is retryable', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    r.k2.sources.subscribe('x', { data: () => undefined, error: () => undefined })
    const delays: number[] = []
    for (let i = 0; i < 8; i++) {
      await tick()
      const subs = subsOf(heard, 'sources.subscribe')
      host.postMessage({ sub: subs.at(-1), error: { code: 'rate_limited', message: 'slow' } })
      await tick()
      delays.push(...r.runTimers())
    }
    expect(delays).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000])
  })

  it('a refusal that isn’t retryable ends it: error with retrying false, state ended, no new subscribe', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const errors: any[] = []
    const states: string[] = []
    r.k2.sources.subscribe('x', { data: () => undefined, error: (e: unknown) => errors.push(e), state: (s: string) => states.push(s) })
    await tick()
    host.postMessage({ sub: subsOf(heard, 'sources.subscribe')[0], error: { code: 'cap_not_granted', message: 'no', cap: 'sources:read' } })
    await tick()
    expect(errors.map((e) => [e.code, e.retrying])).toEqual([['cap_not_granted', false]])
    expect(states).toEqual(['ended'])
    expect(r.pending()).toBe(0)
    expect(subsOf(heard, 'sources.subscribe')).toHaveLength(1)
  })

  it('nothing is called after the unsubscribe: not while waiting to retry, not for late values', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const calls: string[] = []
    const off = r.k2.sources.subscribe('x', {
      data: () => calls.push('data'),
      error: () => calls.push('error'),
      state: (s: string) => calls.push(s),
    })
    await tick()
    const [s1] = subsOf(heard, 'sources.subscribe')
    host.postMessage({ sub: s1, error: { code: 'not_bound', message: 'x' } })
    await tick()
    expect(calls).toEqual(['error', 'retrying'])
    off()
    off()
    expect(r.pending()).toBe(0)
    r.runTimers()
    await tick()
    expect(subsOf(heard, 'sources.subscribe')).toHaveLength(1)
    host.postMessage({ sub: s1, value: VIEW })
    await tick()
    expect(calls).toEqual(['error', 'retrying'])
    // Live, then off: the unsub goes out and late values are dropped.
    const off2 = r.k2.sources.subscribe('y', { data: () => calls.push('data2') })
    await tick()
    const s2 = subsOf(heard, 'sources.subscribe').at(-1)
    host.postMessage({ sub: s2, value: VIEW })
    await tick()
    off2()
    await tick()
    expect(heard.at(-1)).toEqual({ unsub: s2 })
    host.postMessage({ sub: s2, value: VIEW })
    await tick()
    expect(calls.filter((c) => c === 'data2')).toHaveLength(1)
  })

  it('bad_shape: a value that isn’t a card view reaches error, data keeps the last good one', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const got: unknown[] = []
    const errors: any[] = []
    r.k2.sources.subscribe('x', { data: (v: unknown) => got.push(v), error: (e: unknown) => errors.push(e) })
    await tick()
    const [s1] = subsOf(heard, 'sources.subscribe')
    for (const bad of [null, 42, 'ok', [VIEW], { items: [] }]) host.postMessage({ sub: s1, value: bad })
    host.postMessage({ sub: s1, value: { ...VIEW, items: [null, 3, { key: 7, fields: 'x' }], fields: [1], counts: null } })
    await tick()
    expect(errors.map((e) => e.code)).toEqual(['bad_shape', 'bad_shape', 'bad_shape', 'bad_shape', 'bad_shape'])
    expect(got).toHaveLength(1)
    const v = got[0] as any
    expect(v.items).toEqual([{ key: '7', fields: {}, health: 'unknown', reason: null, soft: { count: 0, of: 1, level: null } }])
    expect(v.fields).toEqual({})
    expect(v.counts).toEqual({ ok: 0, warn: 0, crit: 0, unknown: 0 })
  })

  it('store.subscribe with handlers: deliveries are guarded too', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const got: unknown[] = []
    const errors: any[] = []
    r.k2.store.subscribe({ prefix: 'holds/', from: 'fleet-ops-lists' }, { data: (v: unknown) => got.push(v), error: (e: unknown) => errors.push(e) })
    await tick()
    const m = heard.find((x) => x.verb === 'store.changes')!
    expect(m.args).toEqual([{ prefix: 'holds/', from: 'fleet-ops-lists' }])
    host.postMessage({ sub: m.sub, value: [{ key: 'holds/a', rev: 1, value: 1, by: 'me' }, 'junk'] })
    host.postMessage({ sub: m.sub, value: { resync: true, items: [] } })
    host.postMessage({ sub: m.sub, value: { resync: true } })
    await tick()
    expect(got).toEqual([[{ key: 'holds/a', rev: 1, value: 1, by: 'me' }], { resync: true, items: [] }])
    expect(errors.map((e) => e.code)).toEqual(['bad_shape'])
  })

})

describe('0.46.3: a refused (cb, onError) source subscription is retried too', () => {
  it('not_bound: onError hears it with retrying, K2 subscribes again, raw values flow when bound', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const errors: any[] = []
    const got: unknown[] = []
    r.k2.sources.subscribe('x', (v: unknown) => got.push(v), (e: unknown) => errors.push(e))
    await tick()
    const [s1] = subsOf(heard, 'sources.subscribe')
    host.postMessage({ sub: s1, value: { anything: true } })
    host.postMessage({ sub: s1, error: { code: 'not_bound', message: 'x' } })
    await tick()
    expect(got, 'the old form gets raw values: no shape guard').toEqual([{ anything: true }])
    expect(errors.map((e) => [e.code, e.retryable, e.retrying, e.attempt])).toEqual([['not_bound', true, true, 1]])
    expect(r.runTimers()).toEqual([1000])
    await tick()
    const [, s2] = subsOf(heard, 'sources.subscribe')
    expect(heard.find((m) => m.sub === s2)!.args).toEqual(['x'])
    host.postMessage({ sub: s2, value: VIEW })
    await tick()
    expect(got.at(-1)).toEqual(VIEW)
  })

  it('a refusal that isn’t retryable still ends it once; rate_limited is never retried in the old form', async () => {
    for (const code of ['cap_not_granted', 'rate_limited']) {
      const r = realm()
      const { host, heard } = connect(r)
      const errors: any[] = []
      r.k2.sources.subscribe('x', () => undefined, (e: unknown) => errors.push(e))
      await tick()
      host.postMessage({ sub: subsOf(heard, 'sources.subscribe')[0], error: { code, message: 'no' } })
      await tick()
      expect(errors.map((e) => [e.code, e.retrying])).toEqual([[code, false]])
      expect(r.pending()).toBe(0)
      expect(subsOf(heard, 'sources.subscribe')).toHaveLength(1)
    }
  })

  it('a card that re-subscribes in its own onError replaces K2’s retry: no pile-up', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    let offs = 0
    const sub = (): void => {
      r.k2.sources.subscribe('x', () => undefined, () => {
        offs += 1
        sub()
      })
    }
    sub()
    for (let i = 0; i < 5; i++) {
      await tick()
      host.postMessage({ sub: subsOf(heard, 'sources.subscribe').at(-1), error: { code: 'not_bound', message: 'x' } })
      await tick()
    }
    expect(offs).toBe(5)
    // Each card re-subscribe stopped the retry K2 had waiting: no timers left.
    expect(r.pending()).toBe(0)
    expect(subsOf(heard, 'sources.subscribe')).toHaveLength(6)
  })

  it('a second live subscription to the same source is never stopped by a new one', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const a: unknown[] = []
    const b: unknown[] = []
    r.k2.sources.subscribe('x', (v: unknown) => a.push(v))
    r.k2.sources.subscribe('x', (v: unknown) => b.push(v))
    await tick()
    const [s1, s2] = subsOf(heard, 'sources.subscribe')
    host.postMessage({ sub: s1, value: 1 })
    host.postMessage({ sub: s2, value: 2 })
    await tick()
    expect([a, b]).toEqual([[1], [2]])
    expect(heard.filter((m) => typeof m.unsub === 'number')).toEqual([])
  })

  it('with no onError K2 hears of a run of refusals once, not per retry', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    r.k2.sources.subscribe('x', () => undefined)
    for (let i = 0; i < 4; i++) {
      await tick()
      host.postMessage({ sub: subsOf(heard, 'sources.subscribe').at(-1), error: { code: 'unreachable', message: 'down' } })
      await tick()
      r.runTimers()
    }
    await tick()
    expect(heard.filter((m) => m.error && typeof m.error.message === 'string' && m.error.message.includes('sources.subscribe'))).toHaveLength(1)
    expect(subsOf(heard, 'sources.subscribe')).toHaveLength(5)
  })

  it('other verbs keep the old form exactly: one refusal ends it', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const errors: any[] = []
    r.k2.subscribe('presence.subscribe', 'ada::rosson.k2.dev', () => undefined, (e: unknown) => errors.push(e))
    await tick()
    host.postMessage({ sub: subsOf(heard, 'presence.subscribe')[0], error: { code: 'not_bound', message: 'x' } })
    await tick()
    expect(errors.map((e) => [e.code, e.retrying])).toEqual([['not_bound', undefined]])
    expect(r.pending()).toBe(0)
  })
})

describe('0.46.3: the Garden’s scheme wins in the frame', () => {
  it('a theme switch sets color-scheme !important on the root, over any widget rule', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    await tick()
    const themeSub = subsOf(heard, 'theme.changed')[0]
    host.postMessage({ sub: themeSub, value: { theme: { scheme: 'dark', vars: { '--zen-text': '#eee' } } } })
    await tick()
    expect(r.rootStyle()).toEqual([
      ['--zen-text', '#eee', undefined],
      ['color-scheme', 'dark', 'important'],
    ])
  })

  it('a widget whose manifest says colorScheme "own" keeps its own scheme on a switch', async () => {
    const r = realm()
    r.rootAttrs['data-k2-scheme'] = 'own'
    const { host, heard } = connect(r)
    await tick()
    host.postMessage({ sub: subsOf(heard, 'theme.changed')[0], value: { theme: { scheme: 'dark', vars: { '--zen-text': '#eee' } } } })
    await tick()
    expect(r.rootStyle()).toEqual([['--zen-text', '#eee', undefined]])
  })
})

describe('0.46.1: k2.shape', () => {
  it('guards never throw and always give the shape', () => {
    const { shape } = realm().k2
    expect(Object.isFrozen(shape)).toBe(true)
    expect(shape.view(null)).toBeNull()
    expect(shape.view({ items: [] })).toBeNull()
    const v = shape.view({ health: 'crit' })
    expect([v.display, v.phase, v.items, v.fields, v.label, v.updatedAt]).toEqual(['crit', 'pending', [], {}, '', null])
    expect(shape.view({ display: 'stale', health: 'purple', extra: 1 })).toMatchObject({ health: 'unknown', display: 'stale', extra: 1 })
    expect(shape.list('x')).toEqual([])
    expect(shape.list([1])).toEqual([1])
    expect(shape.obj([1])).toEqual({})
    expect(shape.num('3')).toBe(0)
    expect(shape.num(Number.POSITIVE_INFINITY, 7)).toBe(7)
    expect(shape.str(5, 'n/a')).toBe('n/a')
    expect(shape.str('abcdef', '', 3)).toBe('abc')
  })
})
