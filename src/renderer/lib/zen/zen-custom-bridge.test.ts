// prd-zen-user-widgets-v2 TUW3.3 (caps, binding, refusals), TUW3.4 (rows by
// cap), TUW3.5 (limits), TUWA7 (projection), TUWB4 (runaway guard as changed
// 2026-10-08: it pauses posting, never the frame) — the custom layer, with
// K2's own verbs faked behind the inner bridge. No permissions: a new widget
// posts with zero clicks. Fixtures are made up (example.test, g-test0001).
import { describe, expect, it, vi } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import { ZenBridgeError } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import type { ZenAgentRow, ZenThreadView } from './zen-data'
import { createZenCustomLayer, zenEffectiveCaps, zenPlain, ZEN_RUNAWAY, type ZenCustomLayerDeps } from './zen-custom-bridge'
import { createZenSourcesReader } from './zen-sources'
import { fakeSources, fixtureView, settle } from './zen-sources-fixtures'
import { projectZenRow, projectZenThreadItem, ZEN_WIDGET_ROW_KEYS } from './zen-custom-projection'
import { ZenCanvasError } from './zen-canvas'
import { ZenWidgetError } from './zen-widget-routes'
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
    counts: { subagents: 2, tools: 14, commands: 5 },
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
    color: null,
    image: null,
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
    origin: 'local',
    paused: null,
    ...over,
  }
}

interface Harness {
  deps: ZenCustomLayerDeps
  calls: Array<{ verb: string; args: unknown[] }>
  pushes: Array<{ sub: number; value: unknown }>
  stops: string[]
  runaway: number
  paused: boolean
  clock: { t: number }
  widget: { current: ZenCustomWidgetPayload }
}

function harness(over: Partial<ZenCustomLayerDeps> = {}, impls: Record<string, (...a: unknown[]) => unknown> = {}): Harness {
  const h: Harness = {
    calls: [],
    pushes: [],
    stops: [],
    runaway: 0,
    paused: false,
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
    paused: () => h.paused,
    pauseForRunaway: () => {
      h.runaway += 1
      h.paused = true
    },
    theme: () => ({ theme: { scheme: 'light', vars: {} }, chrome: { corners: null, stoplights: null }, motion: { reduced: false } }),
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
  it('a widget that asks for no caps: every capped verb is cap_not_granted (with the cap); no-cap verbs still work', async () => {
    const h = harness()
    h.widget.current = payload({ caps: [], requested: [] })
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
      {
        verb: 'thread.post',
        args: [
          'bob::box.example.test',
          'hello',
          { origin: { widget: 'agent-arcade', garden: 'g-test0001', placement: 'arcade', hash: 'h1' } },
        ],
      },
    ])
  })

  // 0.46.1: every widget post and card answer names its placement and the
  // hash its frame runs, so the daemon holding the Garden checks thread:post.
  it('posts and answers carry the placement and the frame hash (the frame never sets them)', async () => {
    const h = harness({ frameHash: () => 'framehash2' })
    const layer = createZenCustomLayer(h.deps)
    const origin = { widget: 'agent-arcade', garden: 'g-test0001', placement: 'arcade', hash: 'framehash2' }
    // A frame's own `origin` option is never passed through (options are refused).
    expect(code(await call(layer, 'thread.post', 'alice::local', 'hi', { origin: { placement: 'x', hash: 'y' } }))).toBe('failed')
    expect(await call(layer, 'thread.post', 'alice::local', 'hi')).toMatchObject({ ok: true })
    expect(await call(layer, 'thread.answer', 'alice::local', 'c1', 'Yes')).toMatchObject({ ok: true })
    expect(h.calls).toEqual([
      { verb: 'thread.post', args: ['alice::local', 'hi', { origin }] },
      { verb: 'thread.answer', args: ['alice::local', 'c1', { answer: 'Yes', origin }] },
    ])
    // Without a loaded frame hash, the placement's hash.
    const h2 = harness()
    await call(createZenCustomLayer(h2.deps), 'thread.post', 'alice::local', 'hi')
    expect(h2.calls[0].args[2]).toEqual({ origin: { ...origin, hash: 'h1' } })
  })

  it("the daemon's widget_cap_missing reaches the widget as cap_not_granted", async () => {
    const h = harness({}, {
      'thread.post': () => Promise.reject(new ZenWidgetError('widget_cap_missing', 'needs thread:post')),
    })
    const r = await call(createZenCustomLayer(h.deps), 'thread.post', 'alice::local', 'hi')
    expect(r).toMatchObject({ ok: false, error: { code: 'cap_not_granted', cap: 'thread:post' } })
    const h2 = harness({}, {
      'thread.post': () => Promise.reject(new ZenWidgetError('widget_not_approved', 'stale')),
    })
    expect(code(await call(createZenCustomLayer(h2.deps), 'thread.post', 'alice::local', 'hi'))).toBe('failed')
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

  it('0.46.1: arguments that aren’t a list are bad_args, a missing list is none; nothing runs', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (const args of ['alice::local', { 0: 'alice::local' }, 7, null]) {
      const r = (await layer.handle({ id: 9, verb: 'agents.list', args })) as ZenFrameReply
      expect(code(r), JSON.stringify(args)).toBe('bad_args')
    }
    const sub = await layer.handle({ sub: 3, verb: 'agents.subscribe', args: 'x' })
    expect(sub).toEqual({ sub: 3, error: expect.objectContaining({ code: 'bad_args' }) })
    expect(h.calls).toEqual([])
    const none = (await layer.handle({ id: 10, verb: 'agents.list' })) as ZenFrameReply
    expect('ok' in none && (none.ok || none.error.code !== 'bad_args')).toBe(true)
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

describe('No permissions, and the runaway guard (TUWB4, Rosson 2026-10-08)', () => {
  it('a new widget posts with zero clicks: no grant, no Sending switch', async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    expect(await call(layer, 'thread.post', 'alice::local', 'hello')).toEqual({ id: 1, ok: true, value: { id: 'm1', seq: 9 } })
    expect(await call(layer, 'thread.answer', 'alice::local', 'c1', 'Yes')).toMatchObject({ ok: true })
    expect(await call(layer, 'agents.list')).toMatchObject({ ok: true })
    expect(h.runaway).toBe(0)
  })

  it('a widget that is not from your own Garden gets no caps (the v4 seam)', async () => {
    const h = harness()
    h.widget.current = payload({ origin: 'other', caps: [] })
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'thread.post', 'alice::local', 'hi'))).toBe('cap_not_granted')
    expect(h.calls).toEqual([])
  })

  it('while paused (here or by the daemon) posts and answers are sending_off; reads and drafts still work', async () => {
    const h = harness()
    h.paused = true
    const layer = createZenCustomLayer(h.deps)
    const r = await call(layer, 'thread.post', 'alice::local', 'hi')
    expect(code(r)).toBe('sending_off')
    expect((r as { error: { message: string } }).error.message).toContain('Paused: too many posts')
    expect(code(await call(layer, 'thread.answer', 'alice::local', 'c1', 'Yes'))).toBe('sending_off')
    expect(await call(layer, 'compose.draft', 'alice::local', 'hi')).toMatchObject({ ok: true })
    expect(await call(layer, 'agents.list')).toMatchObject({ ok: true })
    expect(h.calls.some((c) => c.verb === 'thread.post')).toBe(false)
  })

  it(`the ${ZEN_RUNAWAY.posts + 1}st post in 10 minutes pauses posting; the frame keeps running; Resume lets it post again`, async () => {
    const h = harness()
    const layer = createZenCustomLayer(h.deps)
    for (let i = 0; i < ZEN_RUNAWAY.posts; i++) {
      h.clock.t += 2_000 // never over the call budget
      expect(await call(layer, 'thread.post', i % 2 ? 'alice::local' : 'bob::box.example.test', `msg ${i}`)).toMatchObject({ ok: true })
    }
    h.clock.t += 2_000
    expect(code(await call(layer, 'thread.post', 'alice::local', 'one more'))).toBe('sending_off')
    expect(h.runaway).toBe(1)
    expect(h.stops).toEqual([])
    // Still running: reads answer, posts stay refused, the guard trips once.
    expect(await layer.handle({ id: 2, verb: 'gardens.list', args: [] })).toMatchObject({ ok: true })
    h.clock.t += 2_000
    expect(code(await call(layer, 'thread.post', 'alice::local', 'and another'))).toBe('sending_off')
    expect(h.runaway).toBe(1)
    // The person clicks Resume: posting starts over with a fresh count.
    h.paused = false
    for (let i = 0; i < ZEN_RUNAWAY.posts; i++) {
      h.clock.t += 2_000
      expect(await call(layer, 'thread.post', 'alice::local', `after ${i}`)).toMatchObject({ ok: true })
    }
    expect(h.runaway).toBe(1)
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
    expect(h.runaway).toBe(1)
    expect(h.stops).toEqual([])
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

  it('a refused subscription is answered once with {sub, error}, never dropped (B4 Q2)', async () => {
    const h = harness({}, { 'agents.subscribe': () => () => undefined })
    h.widget.current = payload({ caps: [], requested: [] })
    const noCap = createZenCustomLayer(h.deps)
    expect(await noCap.handle({ sub: 7, verb: 'agents.subscribe', args: [] })).toEqual({
      sub: 7,
      error: expect.objectContaining({ code: 'cap_not_granted', cap: 'agents:read' }),
    })
    expect(await noCap.handle({ sub: 8, verb: 'made.up', args: [] })).toEqual({
      sub: 8,
      error: expect.objectContaining({ code: 'unknown_verb' }),
    })
    // A call verb sent as a subscription.
    expect(await noCap.handle({ sub: 9, verb: 'gardens.list', args: [] })).toEqual({
      sub: 9,
      error: expect.objectContaining({ code: 'failed' }),
    })
    expect(noCap.subscriptions).toBe(0)

    const h2 = harness({}, { 'agents.subscribe': () => () => undefined })
    const layer = createZenCustomLayer(h2.deps)
    for (let i = 0; i < 16; i++) expect(await layer.handle({ sub: i + 1, verb: 'agents.subscribe', args: [] })).toBeNull()
    expect(await layer.handle({ sub: 17, verb: 'agents.subscribe', args: [] })).toEqual({
      sub: 17,
      error: expect.objectContaining({ code: 'rate_limited' }),
    })
    // A repeated sub id is the frame's own bug: ignored, not refused.
    expect(await layer.handle({ sub: 1, verb: 'agents.subscribe', args: [] })).toBeNull()
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

  it('counts: exactly three numbers cross; null when unreachable or unknown', () => {
    expect(projectZenRow(ALICE, new Set()).counts).toEqual({ subagents: 2, tools: 14, commands: 5 })
    const sneaky = row('x::local', {
      counts: { subagents: 1, tools: 2, commands: 0, toolName: 'Bash' } as unknown as ZenAgentRow['counts'],
    })
    expect(projectZenRow(sneaky, new Set()).counts).toEqual({ subagents: 1, tools: 2, commands: 0 })
    expect(projectZenRow(row('x::h', { state: 'offline', activity: null }), new Set()).counts).toBeNull()
    expect(projectZenRow(row('x::local', { counts: null }), new Set()).counts).toBeNull()
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

  it('color and image: the agent look crosses only under the App surface rules', () => {
    // 1x1 PNG.
    const png =
      'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII='
    expect(projectZenRow(ALICE, new Set())).toMatchObject({ color: null, image: null })
    expect(projectZenRow(row('x::local', { color: '#3366AA', image: png }), new Set())).toMatchObject({
      color: '#3366aa',
      image: png,
    })
    // Never SVG, never a URL, never bytes that lie about their type.
    const svg = `data:image/svg+xml;base64,${btoa('<svg xmlns="http://www.w3.org/2000/svg"/>')}`
    const lying = `data:image/png;base64,${btoa('GIF89a-not-a-png')}`
    for (const bad of [svg, lying, 'http://127.0.0.1:38472/icon.png', 'file:///x.png']) {
      expect(projectZenRow(row('x::local', { image: bad }), new Set()).image, bad).toBeNull()
    }
    expect(projectZenRow(row('x::local', { color: 'red' }), new Set()).color).toBeNull()
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

  it('choice cards carry answeredBy and expiresAt (catalog v7), null when absent', () => {
    const card = (choice: Record<string, unknown>): OverlayThreadItem => ({
      collection: 'thread',
      seq: 9,
      id: 'c9',
      conversation_id: 'session-123',
      doc: {
        id: 'c9',
        kind: 'choice',
        from: 'alice',
        choice: { prompt: 'Ship?', options: [{ label: 'Go' }], allow_custom: false, ...choice } as never,
      },
    })
    const answered = projectZenThreadItem(
      card({ status: 'answered', answer: 'Go', answeredBy: { kind: 'owner', login: 'owner', name: 'Rosson' }, answeredAt: 40, expiresAt: 99 }),
    )
    expect(answered?.doc).toMatchObject({
      choice: { status: 'answered', answer: 'Go', answeredBy: { kind: 'owner', login: 'owner', name: 'Rosson' }, expiresAt: 99 },
    })
    // answeredAt is not part of the widget shape (closed: only catalog fields cross).
    const c = (answered?.doc as unknown as { choice: Record<string, unknown> }).choice
    expect(Object.keys(c).sort()).toEqual(['allow_custom', 'answer', 'answeredBy', 'expiresAt', 'options', 'prompt', 'status'])
    // 0.46.3: an answered settings card's result line crosses (catalog field
    // resultMessage); a card without one keeps the exact shape above.
    const kept = projectZenThreadItem(card({ status: 'answered', answer: 'Go', resultMessage: 'Kept as yours.' }))
    const kc = (kept?.doc as unknown as { choice: Record<string, unknown> }).choice
    expect(kc.resultMessage).toBe('Kept as yours.')
    expect(Object.keys(kc).sort()).toEqual(['allow_custom', 'answer', 'answeredBy', 'expiresAt', 'options', 'prompt', 'resultMessage', 'status'])
    expect(projectZenThreadItem(card({ status: 'answered', answeredBy: { kind: 'guest', login: 'patty', name: 'Spoof' } }))?.doc).toMatchObject({
      choice: { answeredBy: { kind: 'guest', login: 'patty', name: null } },
    })
    expect(projectZenThreadItem(card({ status: 'pending' }))?.doc).toMatchObject({ choice: { answer: null, answeredBy: null, expiresAt: null } })
    expect(projectZenThreadItem(card({ status: 'pending', expiresAt: 1.5, answeredBy: { kind: '', login: 'x' } }))?.doc).toMatchObject({
      choice: { answeredBy: null, expiresAt: null },
    })
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
        items: [{ seq: 1, id: 'a', doc: { id: 'a', kind: 'text', from: 'alice', to: null, body: 'yo', via: null, choice: null } }],
        hasMore: false,
        turn: { state: 'working', since: 10 },
      },
    })
  })

  it('TUW3.2: functions in a result are dropped', () => {
    expect(zenPlain({ a: 1, f: () => 2, nested: { g: vi.fn() } })).toEqual({ a: 1, nested: {} })
  })
})

describe('0.45.3: the whole canvas through the layer', () => {
  function canvasHarness() {
    const regions: unknown[] = []
    let drags = 0
    let press = false
    const h = harness({
      canvas: {
        setHitRegions: (rects) => void regions.push(rects),
        startDrag: () => {
          if (!press) throw new ZenCanvasError('failed', 'window.startDrag needs a mouse button down in this widget.')
          press = false
          drags += 1
        },
      },
    })
    return { h, regions, drags: () => drags, press: () => void (press = true) }
  }

  it('a widget with no caps may call both (no data, no cap)', async () => {
    const c = canvasHarness()
    c.h.widget.current = payload({ caps: [], requested: [] })
    const layer = createZenCustomLayer(c.h.deps)
    expect(await call(layer, 'canvas.setHitRegions', [{ x: 1, y: 2, width: 3, height: 4 }])).toEqual({ id: 1, ok: true, value: null })
    expect(c.regions).toEqual([[{ left: 1, top: 2, width: 3, height: 4 }]])
    expect(await call(layer, 'canvas.setHitRegions', [])).toMatchObject({ ok: true })
    expect(c.regions[1]).toEqual([])
    // Nothing reached K2's own verbs.
    expect(c.h.calls).toEqual([])
  })

  it('bad input is refused before anything changes: failed, or too_large over the cap', async () => {
    const c = canvasHarness()
    const layer = createZenCustomLayer(c.h.deps)
    expect(code(await call(layer, 'canvas.setHitRegions', 'all of it'))).toBe('failed')
    expect(code(await call(layer, 'canvas.setHitRegions', [{ x: 1, y: 2, width: Number.NaN, height: 4 }]))).toBe('failed')
    expect(code(await call(layer, 'canvas.setHitRegions', Array(33).fill({ x: 0, y: 0, width: 1, height: 1 })))).toBe('too_large')
    expect(c.regions).toEqual([])
  })

  it('window.startDrag: refused (failed) with no press; honoured once per press', async () => {
    const c = canvasHarness()
    const layer = createZenCustomLayer(c.h.deps)
    expect(code(await call(layer, 'window.startDrag'))).toBe('failed')
    c.press()
    expect(await call(layer, 'window.startDrag')).toMatchObject({ ok: true, value: null })
    expect(code(await call(layer, 'window.startDrag'))).toBe('failed')
    expect(c.drags()).toBe(1)
  })

  it('without a canvas host both are verb_unavailable', async () => {
    const layer = createZenCustomLayer(harness().deps)
    expect(code(await call(layer, 'canvas.setHitRegions', []))).toBe('verb_unavailable')
    expect(code(await call(layer, 'window.startDrag'))).toBe('verb_unavailable')
  })
})

// ── Garden v3 data sources (contract-sources-v1 §6, §12.3 "Verbs") ──────

function sourcesHarness(bound: string[] = ['fleet', 'deploys::akzm.k2.dev']): {
  h: Harness
  f: ReturnType<typeof fakeSources>
  asks: Array<[string, string | null]>
  errors: Array<{ sub: number; error: unknown }>
} {
  const f = fakeSources(['akzm.k2.dev'])
  f.servers.get('local').views.set('fleet', fixtureView())
  f.servers.get('akzm.k2.dev').views.set('deploys', fixtureView({ name: 'deploys', label: 'Deploys' }))
  const reader = createZenSourcesReader(f.deps)
  const asks: Array<[string, string | null]> = []
  const errors: Array<{ sub: number; error: unknown }> = []
  const h = harness({
    sources: {
      bound: () => bound,
      reader,
      ask: async (address, key) => {
        asks.push([address, key])
        return true
      },
    },
    pushError: (sub, error) => errors.push({ sub, error }),
  })
  h.widget.current = payload({
    caps: ['sources:read'] as never,
    requested: ['sources:read', 'thread:post'] as never,
    sourceAddresses: bound,
  })
  return { h, f, asks, errors }
}

describe('Garden v3: sources.* verbs', () => {
  it('0.46.1: a sources reader posts to the Thread when it asked for thread:post, and is refused when it did not', async () => {
    const { h } = sourcesHarness()
    h.widget.current = payload({
      caps: ['thread:post', 'sources:read'] as never,
      requested: ['thread:post', 'sources:read'] as never,
      sourceAddresses: ['fleet'],
    })
    const layer = createZenCustomLayer(h.deps)
    expect(await call(layer, 'thread.post', 'alice::local', 'box ohana is behind target')).toMatchObject({ ok: true })
    const posts = () => h.calls.filter((c) => c.verb === 'thread.post')
    expect(posts().map((c) => c.args[1])).toEqual(['box ohana is behind target'])
    expect(await call(layer, 'sources.read', 'fleet')).toMatchObject({ ok: true })
    expect(zenEffectiveCaps({ caps: ['thread:post', 'sources:read'] as never, requested: ['thread:post', 'sources:read'] as never })).toEqual(
      new Set(['thread:post', 'sources:read']),
    )
    // Outside its caps: no thread:post in the manifest → refused, nothing sent.
    h.widget.current = payload({ caps: ['sources:read'] as never, requested: ['sources:read'] as never, sourceAddresses: ['fleet'] })
    const plain = createZenCustomLayer(h.deps)
    expect(code(await call(plain, 'thread.post', 'alice::local', 'hi'))).toBe('cap_not_granted')
    expect(code(await call(plain, 'compose.draft', 'alice::local', 'hi'))).toBe('cap_not_granted')
    // A cap the daemon sent but the manifest never asked for is dropped too.
    h.widget.current = payload({ caps: ['thread:post', 'sources:read'] as never, requested: ['sources:read'] as never })
    expect(code(await call(createZenCustomLayer(h.deps), 'thread.post', 'alice::local', 'hi'))).toBe('cap_not_granted')
    expect(posts()).toHaveLength(1)
  })

  it('sources.read: a bound address answers its card view; others are not_bound', async () => {
    const { h } = sourcesHarness()
    const layer = createZenCustomLayer(h.deps)
    const r = await call(layer, 'sources.read', 'fleet')
    expect(r).toMatchObject({ ok: true, value: { address: 'fleet', reach: 'ok', label: 'Fleet boxes', serverNow: 1_760_423_520 } })
    const remote = await call(layer, 'sources.read', 'deploys::AKZM.k2.dev')
    expect(remote).toMatchObject({ ok: true, value: { address: 'deploys::akzm.k2.dev' } })
    const nb = await call(layer, 'sources.read', 'money')
    expect(nb).toMatchObject({ ok: false, error: { code: 'not_bound', room: 'money' } })
    expect(code(await call(layer, 'sources.read', 'Not An Address'))).toBe('failed')
  })

  it('cap_not_granted without sources:read; verb_unavailable without a sources host', async () => {
    const { h } = sourcesHarness()
    h.widget.current = payload({ caps: ['agents:read'], requested: ['agents:read'] })
    expect(code(await call(createZenCustomLayer(h.deps), 'sources.read', 'fleet'))).toBe('cap_not_granted')
    const bare = harness()
    bare.widget.current = payload({ caps: ['sources:read'] as never, requested: ['sources:read'] as never })
    expect(code(await call(createZenCustomLayer(bare.deps), 'sources.read', 'fleet'))).toBe('verb_unavailable')
  })

  it('unreachable carries the last view greyed; server_not_saved; removed', async () => {
    const { h, f } = sourcesHarness(['fleet', 'deploys::akzm.k2.dev', 'money::nope.k2.dev'])
    const layer = createZenCustomLayer(h.deps)
    await call(layer, 'sources.read', 'deploys::akzm.k2.dev')
    f.servers.get('akzm.k2.dev').down = true
    const r = await call(layer, 'sources.read', 'deploys::akzm.k2.dev')
    expect(r).toMatchObject({ ok: false, error: { code: 'unreachable', last: { phase: 'stale', reach: 'unreachable' } } })
    expect(code(await call(layer, 'sources.read', 'money::nope.k2.dev'))).toBe('server_not_saved')
    f.servers.get('local').removed.add('fleet')
    expect(code(await call(layer, 'sources.read', 'fleet'))).toBe('removed')
  })

  it('sources.list answers every bound address with its reach', async () => {
    const { h } = sourcesHarness(['fleet', 'money::nope.k2.dev'])
    const r = await call(createZenCustomLayer(h.deps), 'sources.list')
    expect(r).toMatchObject({
      ok: true,
      value: [
        { address: 'fleet', reach: 'ok' },
        { address: 'money::nope.k2.dev', reach: 'server_not_saved' },
      ],
    })
  })

  it('sources.history and sources.refresh, with no_history and rate_limited', async () => {
    const { h, f } = sourcesHarness()
    f.servers.get('local').views.set('fleet', fixtureView({ keep: 'states', fields: {} }))
    const layer = createZenCustomLayer(h.deps)
    expect(code(await call(layer, 'sources.history', 'fleet', 'count'))).toBe('no_history')
    const ok = await call(layer, 'sources.history', 'deploys::akzm.k2.dev', 'count', { fromAt: 10.7 })
    expect(ok).toMatchObject({ ok: true, value: { res: 'raw', points: [{ t: 1, v: 2 }, { t: 2, v: null }] } })
    expect(code(await call(layer, 'sources.history', 'fleet'))).toBe('failed')
    expect(await call(layer, 'sources.refresh', 'fleet')).toMatchObject({ ok: true, value: { queued: true, nextAt: 1_760_423_521 } })
    expect(code(await call(layer, 'sources.refresh', 'fleet'))).toBe('rate_limited')
  })

  it('sources.ask opens K2’s box with the address and key only (no text from the widget)', async () => {
    const { h, asks } = sourcesHarness()
    const layer = createZenCustomLayer(h.deps)
    expect(await call(layer, 'sources.ask', 'fleet', 'alpha')).toMatchObject({ ok: true, value: { opened: true } })
    expect(await call(layer, 'sources.ask', 'fleet')).toMatchObject({ ok: true, value: { opened: true } })
    expect(code(await call(layer, 'sources.ask', 'fleet', { text: 'post this' }))).toBe('failed')
    expect(code(await call(layer, 'sources.ask', 'money'))).toBe('not_bound')
    expect(asks).toEqual([
      ['fleet', 'alpha'],
      ['fleet', null],
    ])
  })

  it('sources.subscribe pushes the view, counts toward 16, and an async refusal ends it', async () => {
    const { h, f, errors } = sourcesHarness(['fleet', 'money::akzm.k2.dev'])
    f.servers.get('akzm.k2.dev').error = { code: 'cap_not_granted', role: 'admin' }
    const layer = createZenCustomLayer(h.deps)
    expect(await layer.handle({ sub: 1, verb: 'sources.subscribe', args: ['fleet'] })).toBeNull()
    expect(await layer.handle({ sub: 2, verb: 'sources.subscribe', args: ['money::akzm.k2.dev'] })).toBeNull()
    await settle()
    expect(h.pushes.find((p) => p.sub === 1)?.value).toMatchObject({ address: 'fleet', reach: 'ok' })
    expect(errors).toEqual([{ sub: 2, error: expect.objectContaining({ code: 'cap_not_granted', room: 'money::akzm.k2.dev' }) }])
    expect(layer.subscriptions).toBe(1)
    const nb = await layer.handle({ sub: 3, verb: 'sources.subscribe', args: ['deploys'] })
    expect(nb).toMatchObject({ sub: 3, error: { code: 'not_bound' } })
    for (let i = 10; i < 25; i++) await layer.handle({ sub: i, verb: 'sources.subscribe', args: ['fleet'] })
    expect(layer.subscriptions).toBe(16)
    const over = await layer.handle({ sub: 99, verb: 'sources.subscribe', args: ['fleet'] })
    expect(over).toMatchObject({ sub: 99, error: { code: 'rate_limited' } })
  })

  // 0.46.1 S3: a placement change re-subscribes a not_bound subscription.
  describe('not_bound source subscriptions wait for a rebind', () => {
    function timed(bound: string[]): {
      layer: ReturnType<typeof createZenCustomLayer>
      h: Harness
      bound: string[]
      errors: Array<{ sub: number; error: unknown }>
      fire: () => void
      timers: Map<number, () => void>
    } {
      const s = sourcesHarness(bound)
      const timers = new Map<number, () => void>()
      let next = 1
      s.h.deps.setTimer = (fn) => {
        const id = next++
        timers.set(id, fn)
        return id
      }
      s.h.deps.clearTimer = (t) => {
        timers.delete(t as number)
      }
      const fire = (): void => {
        const fns = [...timers.values()]
        timers.clear()
        for (const fn of fns) fn()
      }
      return { layer: createZenCustomLayer(s.h.deps), h: s.h, bound, errors: s.errors, fire, timers }
    }

    it('is held (no answer), then starts under its own id when the placement binds the address', async () => {
      const t = timed(['fleet'])
      t.h.widget.current = payload({ caps: ['sources:read'] as never, requested: ['sources:read'] as never, sourceAddresses: ['fleet'] })
      expect(await t.layer.handle({ sub: 7, verb: 'sources.subscribe', args: ['deploys::akzm.k2.dev'] })).toBeNull()
      expect(t.layer.held).toBe(1)
      expect(t.layer.subscriptions).toBe(1)
      expect(t.timers.size).toBe(1)
      // An unrelated change: still held.
      t.layer.boundChanged()
      expect(t.layer.held).toBe(1)
      // The approval lands: the placement now binds it.
      t.bound.push('deploys::akzm.k2.dev')
      t.layer.boundChanged()
      expect(t.layer.held).toBe(0)
      expect(t.timers.size).toBe(0)
      await settle()
      expect(t.h.pushes.find((p) => p.sub === 7)?.value).toMatchObject({ address: 'deploys::akzm.k2.dev', reach: 'ok' })
      expect(t.errors).toEqual([])
    })

    it('answers the refusal through pushError when no rebind comes in time', async () => {
      const t = timed(['fleet'])
      expect(await t.layer.handle({ sub: 8, verb: 'sources.subscribe', args: ['money'] })).toBeNull()
      t.fire()
      expect(t.errors).toEqual([{ sub: 8, error: expect.objectContaining({ code: 'not_bound', room: 'money' }) }])
      expect(t.layer.held).toBe(0)
      expect(t.layer.subscriptions).toBe(0)
      // A late rebind does nothing for an answered subscription.
      t.bound.push('money')
      t.layer.boundChanged()
      await settle()
      expect(t.h.pushes.find((p) => p.sub === 8)).toBeUndefined()
    })

    it('an unsubscribe or dispose drops the hold; other verbs and refusals answer at once', async () => {
      const t = timed(['fleet'])
      await t.layer.handle({ sub: 9, verb: 'sources.subscribe', args: ['money'] })
      await t.layer.handle({ unsub: 9 })
      expect(t.layer.held).toBe(0)
      expect(t.timers.size).toBe(0)
      t.fire()
      expect(t.errors).toEqual([])
      // sources.read is a call: not_bound answers at once.
      expect(await call(t.layer, 'sources.read', 'money')).toMatchObject({ ok: false, error: { code: 'not_bound' } })
      await t.layer.handle({ sub: 10, verb: 'sources.subscribe', args: ['money'] })
      t.layer.dispose()
      expect(t.layer.held).toBe(0)
      expect(t.timers.size).toBe(0)
    })
  })
})
