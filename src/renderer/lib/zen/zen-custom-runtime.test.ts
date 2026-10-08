// prd-zen-user-widgets-v2 TUWA6 — K2's frame runtime, the real
// k2-frame.js text (table + sdk/k2-runtime.js) evaluated in a fresh realm
// with a fake window and a real MessageChannel.
import { runInContext, createContext } from 'node:vm'
import { MessageChannel, type MessagePort } from 'node:worker_threads'
import { afterEach, describe, expect, it } from 'vitest'
import { K2_FRAME_MAX_BYTES, k2FrameScript, zenFramePrelude, zenScriptText } from './zen-custom-prelude'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'

type Listener = (e: unknown) => void

interface Realm {
  k2: Record<string, any>
  fire(type: string, e: unknown): void
  parent: object
  runTimers(): void
  rootStyle: Record<string, string>
  rootAttrs: Record<string, string>
}

const ports: MessagePort[] = []

function realm(platform = 'MacIntel'): Realm {
  const listeners = new Map<string, Listener[]>()
  const timers: Array<() => void> = []
  const parent = {}
  const rootStyle: Record<string, string> = {}
  const rootAttrs: Record<string, string> = {}
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: (f: () => void) => {
      timers.push(f)
      return timers.length
    },
    parent,
    navigator: { platform },
    document: {
      documentElement: {
        style: { setProperty: (k: string, v: string) => void (rootStyle[k] = v) },
        setAttribute: (k: string, v: string) => void (rootAttrs[k] = v),
      },
    },
    console,
  }
  g.self = g
  const ctx = createContext(g)
  runInContext(k2FrameScript(), ctx)
  return {
    k2: g.k2 as Record<string, any>,
    fire: (type, e) => {
      for (const f of listeners.get(type) ?? []) f(e)
    },
    parent,
    runTimers: () => {
      for (const t of timers.splice(0)) t()
    },
    rootStyle,
    rootAttrs,
  }
}

/** Connect: K2's hello with one port; returns the host's end and what it hears. */
function connect(r: Realm, over: Record<string, unknown> = {}): { host: MessagePort; heard: unknown[] } {
  const ch = new MessageChannel()
  ports.push(ch.port1, ch.port2)
  const heard: unknown[] = []
  ch.port1.on('message', (m) => heard.push(m))
  r.fire('message', {
    source: r.parent,
    data: {
      k2: 'hello',
      v: 1,
      caps: ['agents:read'],
      features: ['zen-v1', 'zen-gardens-v1', 'zen-widgets-v1'],
      widget: { id: 'arcade', name: 'Agent Arcade', garden: 'g-test0001' },
      config: { speed: 2 },
      motion: { reduced: true },
      ...over,
    },
    ports: [ch.port2],
  })
  return { host: ch.port1, heard }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 15))

afterEach(() => {
  for (const p of ports.splice(0)) p.close()
})

describe('TUWA6: the frame runtime', () => {
  it('fits the budget and escapes nothing it doesn’t need to', () => {
    const text = k2FrameScript()
    expect(Buffer.byteLength(text, 'utf8')).toBeLessThanOrEqual(K2_FRAME_MAX_BYTES)
    expect(text).not.toMatch(/<\/script/i)
    expect(zenScriptText('a="</script>"; b=/<\\/SCRIPT/')).toBe('a="<\\/script>"; b=/<\\/SCRIPT/')
  })

  it('k2 is frozen and fixed; named helpers exist per table row', () => {
    const r = realm()
    expect(Object.isFrozen(r.k2)).toBe(true)
    expect(Object.isFrozen(r.k2.agents)).toBe(true)
    for (const verb of Object.keys(ZEN_CUSTOM_VERBS)) {
      const [surface, action] = verb.split('.')
      expect(typeof r.k2[surface][action], verb).toBe('function')
    }
    expect(r.k2.on).toBe(r.k2.subscribe)
    expect(r.k2.zen).toBeUndefined()
    expect(r.k2.controls).toBeUndefined()
  })

  it('before the hello a call waits; with no hello in 10 s it rejects failed "not connected"', async () => {
    const r = realm()
    const p = r.k2.agents.list()
    r.runTimers()
    const err = await p.then(
      () => {
        throw new Error('resolved')
      },
      (e: any) => e,
    )
    expect(err.name).toBe('K2Error')
    expect(err.code).toBe('failed')
    expect(err.message).toBe('not connected')
    expect(err.verb).toBe('agents.list')
    // After that, calls reject at once.
    await expect(r.k2.call('gardens.list')).rejects.toMatchObject({ code: 'failed', message: 'not connected' })
  })

  it('a hello from anyone but the parent is ignored; calls made before it go out with it, in order', async () => {
    const r = realm()
    const early = r.k2.gardens.list()
    const ch = new MessageChannel()
    ports.push(ch.port1, ch.port2)
    const strangerHeard: unknown[] = []
    ch.port1.on('message', (m) => strangerHeard.push(m))
    r.fire('message', { source: {}, data: { k2: 'hello', v: 1 }, ports: [ch.port2] })
    const { heard, host } = connect(r)
    await tick()
    expect(strangerHeard).toEqual([])
    expect(heard).toEqual([
      { sub: expect.any(Number), verb: 'theme.changed', args: [] },
      { id: expect.any(Number), verb: 'gardens.list', args: [] },
    ])
    const id = (heard[1] as { id: number }).id
    host.postMessage({ id, ok: true, value: [{ id: 'g-test0001', name: 'Garden 1', index: 1 }] })
    await expect(early).resolves.toEqual([{ id: 'g-test0001', name: 'Garden 1', index: 1 }])
  })

  it('every named helper sends the same message as k2.call; subscribe returns an unsubscribe', async () => {
    const r = realm()
    const { heard } = connect(r)
    await tick()
    heard.length = 0
    void r.k2.thread.post('alice::local', 'hi')
    void r.k2.call('thread.post', 'alice::local', 'hi')
    const cb = (): void => undefined
    const off = r.k2.agents.subscribe(cb)
    const off2 = r.k2.subscribe('agents.subscribe', cb)
    expect(typeof off).toBe('function')
    off()
    off()
    off2()
    await tick()
    const [a, b, s1, s2, u1, u2] = heard as Array<Record<string, unknown>>
    expect({ verb: a.verb, args: a.args }).toEqual({ verb: b.verb, args: b.args })
    expect(a).toMatchObject({ verb: 'thread.post', args: ['alice::local', 'hi'] })
    expect(s1).toMatchObject({ verb: 'agents.subscribe', args: [] })
    expect(s2).toMatchObject({ verb: 'agents.subscribe', args: [] })
    expect(u1).toEqual({ unsub: s1.sub })
    expect(u2).toEqual({ unsub: s2.sub })
    expect(heard).toHaveLength(6)
  })

  it('pushes reach the subscriber; ping answers pong; hello fields show; ready is a protocol message', async () => {
    const r = realm()
    const { heard, host } = connect(r)
    await tick()
    const got: unknown[] = []
    r.k2.agents.subscribe((v: unknown) => got.push(v))
    await tick()
    const sub = (heard.find((m) => (m as { verb?: string }).verb === 'agents.subscribe') as { sub: number }).sub
    host.postMessage({ sub, value: [{ address: 'alice::local' }] })
    host.postMessage({ ping: 7 })
    r.k2.ready()
    await tick()
    expect(got).toEqual([[{ address: 'alice::local' }]])
    expect(heard).toContainEqual({ pong: 7 })
    expect(heard).toContainEqual({ ready: true })
    expect(r.k2.widget).toEqual({ id: 'arcade', name: 'Agent Arcade', garden: 'g-test0001' })
    expect(r.k2.config).toEqual({ speed: 2 })
    expect(r.k2.motion).toEqual({ reduced: true })
  })

  it('k2.can: cap and feature from the hello; never a verb outside the table', async () => {
    const r = realm()
    expect(r.k2.can('gardens.list')).toBe(false) // not connected yet
    connect(r)
    expect(r.k2.can('gardens.list')).toBe(true)
    expect(r.k2.can('agents.list')).toBe(true)
    expect(r.k2.can('thread.post')).toBe(false)
    expect(r.k2.can('zen.exit')).toBe(false)
    expect(r.k2.can('foo.bar')).toBe(false)
  })

  it('a refused call rejects with K2Error carrying the wire fields', async () => {
    const r = realm()
    const { heard, host } = connect(r)
    await tick()
    const p = r.k2.thread.read('carol::local')
    await tick()
    const req = heard.find((m) => (m as { verb?: string }).verb === 'thread.read') as { id: number }
    host.postMessage({ id: req.id, ok: false, error: { code: 'cap_not_granted', message: 'needs thread:read', cap: 'thread:read', room: 'carol' } })
    const err = await p.catch((e: unknown) => e)
    expect(err).toMatchObject({ name: 'K2Error', code: 'cap_not_granted', cap: 'thread:read', room: 'carol', verb: 'thread.read' })
    const p2 = r.k2.call('agents.list')
    await tick()
    const req2 = heard.find((m) => (m as { id?: number; verb?: string }).verb === 'agents.list') as { id: number }
    host.postMessage({ id: req2.id, ok: false, error: { code: 'verb_unavailable', message: 'x', feature: 'zen-widgets-v1' } })
    await expect(p2).rejects.toMatchObject({ code: 'verb_unavailable', feature: 'zen-widgets-v1' })
  })

  it('a refused subscription ({sub, error}) ends and reaches onError; without onError K2 hears it (B4 Q2)', async () => {
    const r = realm()
    const { host, heard } = connect(r)
    const got: unknown[] = []
    const errs: any[] = []
    r.k2.thread.subscribe('alice::local', (v: unknown) => got.push(v), (e: unknown) => errs.push(e))
    r.k2.agents.subscribe((v: unknown) => got.push(v))
    await tick()
    const subOf = (verb: string): number =>
      (heard.find((m) => (m as { verb?: string }).verb === verb) as { sub: number; args: unknown[] }).sub
    const threadMsg = heard.find((m) => (m as { verb?: string }).verb === 'thread.subscribe') as { args: unknown[] }
    expect(threadMsg.args).toEqual(['alice::local'])
    const tSub = subOf('thread.subscribe')
    const aSub = subOf('agents.subscribe')
    host.postMessage({ sub: tSub, error: { code: 'cap_not_granted', message: 'thread.subscribe needs thread:read.', cap: 'thread:read' } })
    host.postMessage({ sub: aSub, error: { code: 'rate_limited', message: 'At most 16 live subscriptions.' } })
    await tick()
    expect(errs).toHaveLength(1)
    expect(errs[0].name).toBe('K2Error')
    expect(errs[0].code).toBe('cap_not_granted')
    expect(errs[0].cap).toBe('thread:read')
    expect(errs[0].verb).toBe('thread.subscribe')
    expect(heard).toContainEqual({ error: { message: 'k2: agents.subscribe refused (rate_limited): At most 16 live subscriptions.' } })
    // The subscriptions are over: a late push reaches no one.
    host.postMessage({ sub: tSub, value: 'late' })
    host.postMessage({ sub: aSub, value: 'late' })
    await tick()
    expect(got).toEqual([])
  })

  it('a subscription made before a hello that never comes reaches onError "not connected"', () => {
    const r = realm()
    const errs: any[] = []
    r.k2.agents.subscribe(() => undefined, (e: unknown) => errs.push(e))
    r.runTimers()
    expect(errs).toHaveLength(1)
    expect(errs[0]).toMatchObject({ code: 'failed', message: 'not connected', verb: 'agents.subscribe' })
  })

  it('theme.changed: K2 applies --zen-* variables and the scheme on :root, nothing else', async () => {
    const r = realm()
    const { heard, host } = connect(r)
    await tick()
    const sub = (heard[0] as { sub: number }).sub
    host.postMessage({ sub, value: { theme: { vars: { '--zen-text': '#111', color: 'red' }, scheme: 'dark' }, chrome: {}, motion: { reduced: false } } })
    await tick()
    expect(r.rootStyle).toEqual({ '--zen-text': '#111' })
    expect(r.rootAttrs).toEqual({ 'data-zen-scheme': 'dark' })
  })

  it('errors and forwarded chords reach K2 (UW30, UW33)', async () => {
    const r = realm('Linux x86_64')
    const { heard } = connect(r)
    await tick()
    heard.length = 0
    r.fire('error', { message: 'boom', error: { stack: 'at x' } })
    r.fire('unhandledrejection', { reason: { message: 'nope' } })
    let prevented = 0
    const key = (o: Record<string, unknown>): void =>
      r.fire('keydown', { code: '', isTrusted: true, repeat: false, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, getModifierState: () => false, preventDefault: () => void prevented++, ...o })
    key({ code: 'KeyZ', ctrlKey: true, altKey: true })
    key({ code: 'Digit3', metaKey: true, altKey: true })
    key({ code: 'Period', ctrlKey: true, altKey: true, shiftKey: true })
    key({ code: 'KeyA', ctrlKey: true })
    // Mirrors zenForwardedChordForKey: untrusted, repeats and Ctrl+Alt+digit are not chords.
    key({ code: 'KeyZ', ctrlKey: true, altKey: true, isTrusted: false })
    key({ code: 'KeyZ', ctrlKey: true, altKey: true, repeat: true })
    key({ code: 'Digit4', ctrlKey: true, altKey: true })
    await tick()
    expect(heard).toEqual([
      { error: { message: 'boom', stack: 'at x' } },
      { error: { message: 'nope', stack: undefined } },
      { chord: 'zen-exit' },
      { chord: 'garden-3' },
      { chord: 'theme-prev' },
    ])
    expect(prevented).toBe(3)
  })
})

describe('the frame prelude', () => {
  it('theme style, then the nonced runtime, then each library, all nonced; no theme value can break out', () => {
    const html = zenFramePrelude({
      nonce: 'AAAAAAAAAAAAAAAAAAAAAA',
      theme: { vars: { '--zen-text': '#111', '--zen-x': 'red}</style><script>alert(1)</script>', bad: 'x' }, scheme: 'light' },
      libs: ['var LIB = "</script>";'],
    })
    expect(html.startsWith('<style data-k2-theme="">:root{--zen-text:#111;color-scheme:light;')).toBe(true)
    expect(html).not.toContain('alert(1)')
    const scripts = [...html.matchAll(/<script nonce="AAAAAAAAAAAAAAAAAAAAAA">/g)]
    expect(scripts).toHaveLength(3)
    expect(html.indexOf('__K2_VERBS__')).toBeLessThan(html.indexOf('var LIB'))
    expect(html).toContain('var LIB = "<\\/script>";')
    expect(html.match(/<\/script>/g)).toHaveLength(3)
  })
})
