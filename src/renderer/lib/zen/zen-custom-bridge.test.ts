// prd-zen-user-widgets-v2 TUW3.3 (caps, binding, refusals), TUW3.4 (rows by
// cap), TUW3.5 (limits), TUWA7 (projection), TUWB4 (runaway guard), UWB9
// (Sending) — the custom layer, with K2's own verbs faked behind the inner
// bridge. Fixtures are made up (example.test, g-test0001).
import { describe, expect, it, vi } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import { ZenBridgeError } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import type { ZenAgentRow, ZenThreadView } from './zen-data'
import { createZenCustomLayer, zenPlain, ZEN_RUNAWAY, type ZenCustomLayerDeps } from './zen-custom-bridge'
import { projectZenRow, projectZenThreadItem, ZEN_WIDGET_ROW_KEYS } from './zen-custom-projection'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

function row(address: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address,
    label: address.split('::')[0],
    index: 0,
    hostKey: address.split('::')[1],
    server: null,
    role: 'owner',
    reach: 'live',
    auth: 'ok',
    activity: 'working',
    working: true,
    needsYou: false,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: { text: 'hi', at: 1, from: 'alice', mine: false, seq: 3 },
    people: [{ user: 'bob', name: 'Bob' }],
    selected: false,
    openable: true,
    avatarUrl: 'data:image/png;base64,AAAA',
    ...over,
  }
}

const ALICE = row('alice::local')
const BOB = row('bob::box.example.test')

function payload(over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id: 'arcade',
    kind: 'custom',
    widget: 'agent-arcade',
    column: 0,
    props: { config: {} },
    caps: ['agents:read', 'thread:read', 'thread:post'],
    requested: ['agents:read', 'thread:read', 'thread:post'],
    source: 'user',
    name: 'Agent Arcade',
    description: null,
    reasons: {},
    libs: [],
    hash: 'h1',
    state: 'ok',
    errors: [],
    warnings: [],
    grant: {
      state: 'granted',
      caps: ['agents:read', 'thread:read', 'thread:post'],
      granted: ['agents:read', 'thread:read', 'thread:post'],
      scope: { home: 'home-work' },
      entries: [{ server: 'local', room: 'alice' }],
      sending: true,
      paused: null,
      grantedAt: 'x',
      widgetHash: 'h1',
    },
    ...over,
  }
}

interface Harness {
  deps: ZenCustomLayerDeps
  calls: Array<{ verb: string; args: unknown[] }>
  pushes: Array<{ sub: number; value: unknown }>
  stops: string[]
  runaway: number
  clock: { t: number }
  widget: { current: ZenCustomWidgetPayload }
}

function harness(over: Partial<ZenCustomLayerDeps> = {}, impls: Record<string, (...a: unknown[]) => unknown> = {}): Harness {
  const h: Harness = {
    calls: [],
    pushes: [],
    stops: [],
    runaway: 0,
    clock: { t: 1_000_000 },
    widget: { current: payload() },
  } as unknown as Harness
  const inner = {
    widgetId: 'arcade',
    caps: new Set<string>(),
    call: (verb: string, ...args: unknown[]) => {
      h.calls.push({ verb, args })
      const impl = impls[verb]
      if (impl) return impl(...args)
      switch (verb) {
        case 'gardens.list':
          return [
            { id: 'g-test0001', name: 'Garden 1', index: 1 },
            { id: 'g-test0002', name: 'Garden 2', index: 2 },
          ]
        case 'agents.list':
          return [ALICE, BOB]
        case 'thread.post':
          return Promise.resolve({ id: 'm1', seq: 9 })
        case 'conversation.open':
          return Promise.resolve(row(String(args[0]), { selected: true }))
        default:
          return null
      }
    },
  } as unknown as ZenWidgetBridge
  h.deps = {
    widget: () => h.widget.current,
    gardenId: () => 'g-test0001',
    inner,
    boundRows: () => [ALICE, BOB],
    ensureConversation: async () => undefined,
    push: (sub, value) => h.pushes.push({ sub, value }),
    stop: (reason) => h.stops.push(reason),
    sendingOffForRunaway: async () => {
      h.runaway += 1
    },
    theme: () => ({ vars: {}, scheme: 'light' }),
    onThemeChange: () => () => undefined,
    now: () => h.clock.t,
    ...over,
  }
  return h
}

async function call(layer: ReturnType<typeof createZenCustomLayer>, verb: string, ...args: unknown[]): Promise<ZenFrameReply> {
  const r = await layer.handle({ id: 1, verb, args })
  if (!r) throw new Error('no reply')
  return r
}

function code(r: ZenFrameReply): string {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (r.ok) throw new Error(`ok: ${JSON.stringify(r.value)}`)
  return r.error.code
}

describe('TUW3.3: caps, binding and refusals', () => {
  it('no grant: every capped verb is cap_not_granted (with the cap); no-cap verbs still work', async () => {
    const h = harness()
    h.widget.current = payload({ caps: [], grant: null })
    const layer = createZenCustomLayer(h.deps)
    for (const verb of ['agents.list', 'thread.read', 'thread.post', 'compose.draft', 'presence.get', 'conversation.open']) {
      const r = await call(layer, verb, 'alice::local', 'x')
      expect(code(r), verb).toBe('cap_not_granted')
    }
    const r = (await call(layer, 'agents.list')) as { error: { cap: string } }
    expect(r.error.cap).toBe('agents:read')
    expect(await call(layer, 'gardens.list')).toMatchObject({ ok: true })
    expect(h.calls.map((c) => c.verb)).toEqual(['gardens.list'])
  })

  it('caps are intersected again with the request and the four widget caps (UW26)', async () => {
    const h = harness()
    h.widget.current = payload({ caps: ['agents:read', 'gardens:manage' as never], requested: ['agents:read'] })
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'thread.read', 'alice::local'))).toBe('cap_not_granted')
    expect(await call(layer, 'agents.list')).toMatchObject({ ok: true })
  })

  it('an address outside the bound rows is not_bound, and nothing reaches K2', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (const verb of ['conversation.open', 'thread.read', 'thread.post', 'thread.answer', 'compose.draft']) {
      const r = await call(layer, verb, 'carol::other.example.test', 'x', 'y')
      expect(code(r), verb).toBe('not_bound')
      expect((r as { error: { room?: string } }).error.room).toBe('carol')
    }
    expect(h.calls).toEqual([])
  })

  it('thread verbs take any bound address (UWB10), not just the open conversation', async () => {
    const ensured: string[] = []
    const h = harness({ ensureConversation: async (a) => void ensured.push(a) })
    const layer = createZenCustomLayer(h.deps)
    expect(await call(layer, 'thread.post', 'bob::box.example.test', 'hello')).toEqual({ id: 1, ok: true, value: { id: 'm1', seq: 9 } })
    expect(ensured).toEqual(['bob::box.example.test'])
    expect(h.calls).toEqual([
      { verb: 'thread.post', args: ['bob::box.example.test', 'hello', { origin: { widget: 'agent-arcade', garden: 'g-test0001' } }] },
    ])
  })

  it('thread.post with paths or files is refused before any upload; a secret answer is refused', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'thread.post', 'alice::local', 'see', { paths: ['/etc/hosts'] }))).toBe('failed')
    expect(code(await call(layer, 'thread.post', 'alice::local', 'see', { files: [] }))).toBe('failed')
    expect(code(await call(layer, 'thread.answer', 'alice::local', 'card1', { secret: 'hunter2' }))).toBe('failed')
    expect(code(await call(layer, 'thread.post', 'alice::local', 'x'.repeat(4001)))).toBe('too_large')
    expect(h.calls).toEqual([])
  })

  it('K2-only verbs are not_exposed; a made-up verb is unknown_verb (UWA10)', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (const verb of ['zen.exit', 'controls.bind', 'agents.local', 'homes.list', 'thread.void', 'thread.markRead', 'gardens.create', 'app.open']) {
      expect(code(await call(layer, verb)), verb).toBe('not_exposed')
    }
    expect(code(await call(layer, 'foo.bar'))).toBe('unknown_verb')
    expect(code(await call(layer, 42 as unknown as string))).toBe('unknown_verb')
    expect(h.calls).toEqual([])
  })

  it('conversation.open never passes where:"agents" through', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    await call(layer, 'conversation.open', 'alice::local', { where: 'agents' })
    expect(h.calls).toEqual([{ verb: 'conversation.open', args: ['alice::local'] }])
  })

  it('inner errors map onto the shared list', async () => {
    const h = harness({}, {
      'presence.get': () => {
        throw new ZenBridgeError('verb_unavailable', 'presence.get')
      },
    })
    h.widget.current = payload({ caps: ['agents:read', 'presence:read'], requested: ['agents:read', 'presence:read'] })
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'presence.get', 'alice::local'))).toBe('verb_unavailable')
  })
})

describe('UWB9: Sending and the runaway guard (TUWB4)', () => {
  it('Sending off → sending_off for posts and answers; drafts still work', async () => {
    const h = harness()
    const g = payload().grant!
    h.widget.current = payload({ grant: { ...g, sending: false } })
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'thread.post', 'alice::local', 'hi'))).toBe('sending_off')
    expect(code(await call(layer, 'thread.answer', 'alice::local', 'c1', 'Yes'))).toBe('sending_off')
    expect(await call(layer, 'compose.draft', 'alice::local', 'hi')).toMatchObject({ ok: true })
  })

  it(`the ${ZEN_RUNAWAY.posts + 1}st post in 10 minutes turns sending off and unloads the frame`, async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (let i = 0; i < ZEN_RUNAWAY.posts; i++) {
      h.clock.t += 2_000 // never over the call budget
      expect(await call(layer, 'thread.post', i % 2 ? 'alice::local' : 'bob::box.example.test', `msg ${i}`)).toMatchObject({ ok: true })
    }
    h.clock.t += 2_000
    expect(code(await call(layer, 'thread.post', 'alice::local', 'one more'))).toBe('sending_off')
    expect(h.runaway).toBe(1)
    expect(h.stops).toEqual(['runaway'])
    // Stopped: the layer answers nothing more.
    expect(await layer.handle({ id: 2, verb: 'gardens.list', args: [] })).toBeNull()
  })

  it(`the ${ZEN_RUNAWAY.identical + 1}st identical text to one agent trips it; posts older than 10 minutes don't count`, async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (let i = 0; i < ZEN_RUNAWAY.identical; i++) {
      h.clock.t += 1_000
      await call(layer, 'thread.post', 'alice::local', 'ping')
    }
    // The same text to another agent is fine.
    expect(await call(layer, 'thread.post', 'bob::box.example.test', 'ping')).toMatchObject({ ok: true })
    h.clock.t += ZEN_RUNAWAY.windowMs + 1
    expect(await call(layer, 'thread.post', 'alice::local', 'ping')).toMatchObject({ ok: true })
    for (let i = 0; i < ZEN_RUNAWAY.identical - 1; i++) {
      h.clock.t += 1_000
      await call(layer, 'thread.post', 'alice::local', 'ping')
    }
    h.clock.t += 1_000
    expect(code(await call(layer, 'thread.post', 'alice::local', 'ping'))).toBe('sending_off')
    expect(h.stops).toEqual(['runaway'])
  })
})

describe('TUW3.5: limits', () => {
  it('gardens.switch: not in the first 2 s, then once every 2 s; ten rate_limited in a minute unload', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'gardens.switch', 'g-test0002'))).toBe('rate_limited')
    h.clock.t += 2_001
    expect(await call(layer, 'gardens.switch', 'g-test0002')).toMatchObject({ ok: true })
    expect(code(await call(layer, 'gardens.switch', 'g-test0001'))).toBe('rate_limited')
    expect(code(await call(layer, 'gardens.switch', 'g-nope'))).toBe('rate_limited')
    for (let i = 0; i < 7; i++) await call(layer, 'gardens.switch', 'g-test0001')
    expect(h.stops).toEqual(['rate'])
  })

  it('60 calls a second with a burst of 120', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    const codes: string[] = []
    for (let i = 0; i < 121; i++) {
      const r = await call(layer, 'gardens.current')
      codes.push('ok' in r && r.ok ? 'ok' : code(r))
    }
    expect(codes.filter((c) => c === 'ok')).toHaveLength(120)
    expect(codes[120]).toBe('rate_limited')
    h.clock.t += 1_000
    expect(await call(layer, 'gardens.current')).toMatchObject({ ok: true })
  })

  it('a message over 64 KB is too_large', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'compose.draft', 'alice::local', 'x'.repeat(70_000)))).toBe('too_large')
  })

  it('16 live subscriptions; theme.changed doesn’t count; unsub frees one', async () => {
    const h = harness({}, { 'agents.subscribe': () => () => undefined })
    const layer = createZenCustomLayer(h.deps)
    await layer.handle({ sub: 100, verb: 'theme.changed', args: [] })
    for (let i = 0; i < 16; i++) await layer.handle({ sub: i + 1, verb: 'agents.subscribe', args: [] })
    expect(layer.subscriptions).toBe(17)
    await layer.handle({ sub: 50, verb: 'agents.subscribe', args: [] })
    expect(layer.subscriptions).toBe(17)
    await layer.handle({ unsub: 1 })
    await layer.handle({ sub: 51, verb: 'agents.subscribe', args: [] })
    expect(layer.subscriptions).toBe(17)
  })
})

describe('TUW3.4 / TUWA7: the guest projection', () => {
  it('rows carry exactly the schema keys; preview with thread:read, people with presence:read', async () => {
    const keys = (caps: string[]): string[] => Object.keys(projectZenRow(ALICE, new Set(caps))).sort()
    expect(keys(['agents:read'])).toEqual([...ZEN_WIDGET_ROW_KEYS].sort())
    expect(keys(['agents:read', 'thread:read'])).toEqual([...ZEN_WIDGET_ROW_KEYS, 'preview'].sort())
    expect(keys(['agents:read', 'presence:read'])).toEqual([...ZEN_WIDGET_ROW_KEYS, 'people'].sort())
    const h = harness()
    h.widget.current = payload({ caps: ['agents:read'], requested: ['agents:read'] })
    const r = (await call(createZenCustomLayer(h.deps), 'agents.list')) as { value: Array<Record<string, unknown>> }
    expect(Object.keys(r.value[0]).sort()).toEqual([...ZEN_WIDGET_ROW_KEYS].sort())
    for (const banned of ['hostKey', 'role', 'reach', 'auth', 'avatarUrl']) expect(banned in r.value[0], banned).toBe(false)
  })

  it('avatar: data: passes, any other URL is null; a new row key never crosses', () => {
    expect(projectZenRow(ALICE, new Set()).avatar).toBe('data:image/png;base64,AAAA')
    expect(projectZenRow(row('x::local', { avatarUrl: 'http://127.0.0.1:38472/icon.png' }), new Set()).avatar).toBeNull()
    const sneaky = { ...ALICE, sessionId: 'abc', projectPath: '/x' } as ZenAgentRow
    const p = projectZenRow(sneaky, new Set(['thread:read', 'presence:read'])) as unknown as Record<string, unknown>
    expect('sessionId' in p).toBe(false)
    expect('projectPath' in p).toBe(false)
    expect(projectZenRow(row('x::h', { state: 'offline', activity: null }), new Set())).toMatchObject({ state: 'unreachable', activity: 'unreachable' })
  })

  it('Thread items: no conversation_id; a secret keeps only id and kind; other kinds are dropped', () => {
    const base: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 'm4',
      conversation_id: 'session-123',
      doc: { id: 'm4', kind: 'text', from: 'alice', to: 'you', created_at: 5, body: 'hi', via: 'agent' },
    }
    expect(projectZenThreadItem(base)).toEqual({
      seq: 4,
      id: 'm4',
      doc: { id: 'm4', kind: 'text', from: 'alice', to: 'you', created_at: 5, body: 'hi', via: 'agent', choice: null },
    })
    expect(projectZenThreadItem({ ...base, doc: { id: 's', kind: 'secret', from: 'alice', secret: { name: 'API', status: 'open' } } })).toEqual({
      seq: 4,
      id: 'm4',
      doc: { id: 's', kind: 'secret' },
    })
    expect(projectZenThreadItem({ ...base, doc: { ...base.doc, kind: 'tool' } })).toBeNull()
  })

  it('subscriptions push projected, de-duplicated values as plain JSON', async () => {
    let feed: ((v: ZenThreadView) => void) | null = null
    const h = harness({}, {
      'thread.subscribe': (_a, cb) => {
        feed = cb as (v: ZenThreadView) => void
        return () => undefined
      },
    })
    const layer = createZenCustomLayer(h.deps)
    await layer.handle({ sub: 7, verb: 'thread.subscribe', args: ['alice::local'] })
    const view: ZenThreadView = {
      address: 'alice::local',
      phase: 'ready',
      note: null,
      items: [{ collection: 'thread', seq: 1, id: 'a', conversation_id: 'sess', doc: { id: 'a', kind: 'text', from: 'alice', body: 'yo' } }],
      loaded: true,
      hasMore: false,
      loadingOlder: false,
      error: null,
      turn: { state: 'working', since: 10 },
    }
    feed!(view)
    feed!({ ...view })
    expect(h.pushes).toHaveLength(1)
    expect(h.pushes[0]).toEqual({
      sub: 7,
      value: {
        address: 'alice::local',
        phase: 'ready',
        note: null,
        items: [{ seq: 1, id: 'a', doc: { id: 'a', kind: 'text', from: 'alice', to: null, created_at: null, body: 'yo', via: null, choice: null } }],
        hasMore: false,
        turn: { state: 'working', since: 10 },
      },
    })
  })

  it('TUW3.2: functions in a result are dropped', () => {
    expect(zenPlain({ a: 1, f: () => 2, nested: { g: vi.fn() } })).toEqual({ a: 1, nested: {} })
  })
})
