// prd-garden-column-collapse-and-agent-list-v1 A1 (CA10) — `k2.layout.*`
// through the custom layer: frame-only and cap-free, the caller's own page
// (the `deps.layout` the page hands its frames), each verb at most 4 a
// second, a fold only within 5 s of the person's press in the frame (the
// page then saves it like the person's click: zen-columns-page.test), and
// the subscription budget. K2's own verbs are faked behind the
// inner bridge. Fixtures are made up (g-test0001).
import { describe, expect, it } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import { createZenCustomLayer, ZEN_LAYOUT_PER_SECOND, type ZenCustomLayerDeps, type ZenCustomLayout } from './zen-custom-bridge'
import { zenColumnFoldRefusal, zenLayoutColumns, type ZenLayoutColumn } from './zen-columns'
import type { ZenColumnFold } from './zen-page'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'

function payload(over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id: 'board',
    kind: 'custom',
    widget: 'board-lite',
    column: 0,
    props: { config: {} },
    // No caps at all: the layout verbs need none.
    caps: [],
    requested: [],
    source: 'user',
    name: 'Board',
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

/** A page with a fixed Board column and a collapsible Chat column. */
class FakePage implements ZenCustomLayout {
  folds: ZenColumnFold[] = [
    { collapsible: false, collapsed: false, label: 'Board' },
    { collapsible: true, collapsed: false, label: 'Chat' },
  ]
  collapsed = [false, false]
  widgetFolds: Array<[number, boolean]> = []
  press = false
  listeners = new Set<(c: ZenLayoutColumn[]) => void>()
  columns(): ZenLayoutColumn[] {
    return zenLayoutColumns(this.folds, this.collapsed)
  }
  setByWidget(index: number, collapsed: boolean) {
    const r = zenColumnFoldRefusal(this.folds, this.collapsed, index, collapsed)
    if (r) return r
    this.widgetFolds.push([index, collapsed])
    this.collapsed = this.collapsed.map((c, i) => (i === index ? collapsed : c))
    for (const fn of this.listeners) fn(this.columns())
    return null
  }
  subscribe(fn: (c: ZenLayoutColumn[]) => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }
  pressed(): boolean {
    return this.press
  }
}

function harness(layout: ZenCustomLayout | undefined) {
  const clock = { t: 1_000_000 }
  const pushes: Array<{ sub: number; value: unknown }> = []
  const stops: string[] = []
  const inner = { widgetId: 'board', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge
  const deps: ZenCustomLayerDeps = {
    widget: () => payload(),
    gardenId: () => 'g-test0001',
    inner,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: (sub, value) => pushes.push({ sub, value }),
    stop: (reason) => stops.push(reason),
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => clock.t,
    ...(layout ? { layout } : {}),
  }
  return { layer: createZenCustomLayer(deps), clock, pushes, stops }
}

async function call(layer: ReturnType<typeof createZenCustomLayer>, verb: string, ...args: unknown[]): Promise<ZenFrameReply> {
  const r = await layer.handle({ id: 1, verb, args })
  if (!r) throw new Error('no reply')
  return r
}

function ok(r: ZenFrameReply): unknown {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (!r.ok) throw new Error(`refused: ${JSON.stringify(r.error)}`)
  return r.value
}

function code(r: ZenFrameReply): string {
  if (!('ok' in r)) throw new Error('not a call reply')
  if (r.ok) throw new Error(`ok: ${JSON.stringify(r.value)}`)
  return r.error.code
}

describe('k2.layout.* rows (catalog 20, CA10)', () => {
  it('no cap, frame-only, widget-exposed', () => {
    expect(ZEN_CUSTOM_VERBS['layout.get']).toEqual({ cap: null, reach: 'local', kind: 'call' })
    expect(ZEN_CUSTOM_VERBS['layout.set']).toEqual({ cap: null, reach: 'local', kind: 'call' })
    expect(ZEN_CUSTOM_VERBS['layout.toggle']).toEqual({ cap: null, reach: 'local', kind: 'call' })
    expect(ZEN_CUSTOM_VERBS['layout.subscribe'].cap).toBeNull()
  })
})

describe('k2.layout.* through the custom layer (A1, CA10)', () => {
  it('without deps.layout every verb answers verb_unavailable', async () => {
    const { layer } = harness(undefined)
    expect(code(await call(layer, 'layout.get'))).toBe('verb_unavailable')
    expect(code(await call(layer, 'layout.set', { column: 1, collapsed: true }))).toBe('verb_unavailable')
    expect(code(await call(layer, 'layout.toggle', 1))).toBe('verb_unavailable')
    expect(await layer.handle({ sub: 1, verb: 'layout.subscribe', args: [] })).toEqual({
      sub: 1,
      error: expect.objectContaining({ code: 'verb_unavailable' }),
    })
  })

  it('get answers the page’s columns; a widget with no caps may call it', async () => {
    const page = new FakePage()
    const { layer } = harness(page)
    expect(ok(await call(layer, 'layout.get'))).toEqual({
      columns: [
        { index: 0, label: 'Board', collapsible: false, collapsed: false },
        { index: 1, label: 'Chat', collapsible: true, collapsed: false },
      ],
    })
  })

  it('without a press: failed "needs a click" and the page is never asked; after a press: folds and answers the column', async () => {
    const page = new FakePage()
    const { layer, clock } = harness(page)
    const refused = await call(layer, 'layout.set', { column: 1, collapsed: true })
    expect(code(refused)).toBe('failed')
    expect('ok' in refused && !refused.ok && refused.error.message).toMatch(/needs a click/)
    expect(page.collapsed).toEqual([false, false])
    expect(page.widgetFolds).toEqual([])

    page.press = true
    clock.t += 300
    expect(ok(await call(layer, 'layout.set', { column: 1, collapsed: true }))).toEqual({
      index: 1,
      label: 'Chat',
      collapsible: true,
      collapsed: true,
    })
    expect(page.collapsed).toEqual([false, true])
    expect(page.widgetFolds).toEqual([[1, true]])

    clock.t += 300
    expect(ok(await call(layer, 'layout.toggle', 1))).toMatchObject({ index: 1, collapsed: false })
    expect(page.collapsed).toEqual([false, false])
    // toggle without a press is refused too.
    page.press = false
    clock.t += 300
    expect(code(await call(layer, 'layout.toggle', 1))).toBe('failed')
  })

  it('a fixed column and the last open one answer failed; a bad index or bad args answer bad_args', async () => {
    const page = new FakePage()
    page.press = true
    const { layer, clock } = harness(page)
    const step = async (verb: string, ...args: unknown[]) => {
      clock.t += 300
      return call(layer, verb, ...args)
    }
    expect(code(await step('layout.set', { column: 0, collapsed: true }))).toBe('failed')
    expect(code(await step('layout.toggle', 0))).toBe('failed')
    // Both collapsible, one already folded: the open one is the last.
    page.folds = [
      { collapsible: true, collapsed: false, label: 'Board' },
      { collapsible: true, collapsed: false, label: 'Chat' },
    ]
    page.collapsed = [false, true]
    const last = await step('layout.set', { column: 0, collapsed: true })
    expect(code(last)).toBe('failed')
    expect('ok' in last && !last.ok && last.error.message).toMatch(/only open column/)
    expect(page.collapsed).toEqual([false, true])

    expect(code(await step('layout.set', { column: 2, collapsed: true }))).toBe('bad_args')
    expect(code(await step('layout.set', { column: -1, collapsed: true }))).toBe('bad_args')
    expect(code(await step('layout.set', { column: 1.5, collapsed: true }))).toBe('bad_args')
    expect(code(await step('layout.set', { column: 1 }))).toBe('bad_args')
    expect(code(await step('layout.set', { column: 1, collapsed: 'yes' }))).toBe('bad_args')
    expect(code(await step('layout.set', { column: 1, collapsed: false, label: 'x' }))).toBe('bad_args')
    expect(code(await step('layout.set', 1))).toBe('bad_args')
    expect(code(await step('layout.toggle', '1'))).toBe('bad_args')
    expect(code(await step('layout.toggle'))).toBe('bad_args')
  })

  it('each verb at most 4 a second: the 5th is rate_limited, the others still answer, and it counts toward UW29', async () => {
    expect(ZEN_LAYOUT_PER_SECOND).toBe(4)
    const page = new FakePage()
    page.press = true
    const { layer, clock, stops } = harness(page)
    for (let i = 0; i < 4; i++) ok(await call(layer, 'layout.get'))
    expect(code(await call(layer, 'layout.get'))).toBe('rate_limited')
    // Per verb: toggle has its own four.
    ok(await call(layer, 'layout.toggle', 1))
    // A second later get answers again.
    clock.t += 1_000
    ok(await call(layer, 'layout.get'))
    // Ten rate_limited answers in a minute unload the widget (UW29).
    for (let i = 0; i < 3; i++) ok(await call(layer, 'layout.get'))
    for (let i = 0; i < 9; i++) expect(code(await call(layer, 'layout.get'))).toBe('rate_limited')
    expect(stops).toEqual(['rate'])
  })

  it('subscribe pushes the columns now and on every move; it counts toward the 16; unsub stops it', async () => {
    const page = new FakePage()
    page.press = true
    const { layer, clock, pushes } = harness(page)
    expect(await layer.handle({ sub: 1, verb: 'layout.subscribe', args: [] })).toBeNull()
    expect(pushes).toEqual([{ sub: 1, value: { columns: page.columns() } }])
    clock.t += 300
    ok(await call(layer, 'layout.set', { column: 1, collapsed: true }))
    expect(pushes[1]).toEqual({ sub: 1, value: { columns: zenLayoutColumns(page.folds, [false, true]) } })
    await layer.handle({ unsub: 1 })
    expect(page.listeners.size).toBe(0)

    // 16 live subscriptions; the 17th is refused (spaced out past the 4/s limit).
    for (let i = 0; i < 16; i++) {
      clock.t += 300
      expect(await layer.handle({ sub: 10 + i, verb: 'layout.subscribe', args: [] })).toBeNull()
    }
    expect(layer.subscriptions).toBe(16)
    clock.t += 300
    expect(await layer.handle({ sub: 99, verb: 'layout.subscribe', args: [] })).toEqual({
      sub: 99,
      error: expect.objectContaining({ code: 'rate_limited' }),
    })
    expect(page.listeners.size).toBe(16)
  })

  it('0.46.5: layout.subscribe is not held to 4 a second (tiles subscribing at once, or re-subscribing as they redraw); the 16 live still bound it', async () => {
    const page = new FakePage()
    const { layer, clock, stops } = harness(page)
    // Six tiles subscribe in the same millisecond, as a canvas does on load.
    for (let i = 0; i < 6; i++) {
      expect(await layer.handle({ sub: 1 + i, verb: 'layout.subscribe', args: [] })).toBeNull()
    }
    expect(page.listeners.size).toBe(6)
    // Each redraw drops its subscription and takes a new one, many times a second.
    for (let round = 0; round < 5; round++) {
      for (let i = 0; i < 6; i++) {
        await layer.handle({ unsub: 1 + i + round * 6 })
        expect(await layer.handle({ sub: 1 + i + (round + 1) * 6, verb: 'layout.subscribe', args: [] })).toBeNull()
      }
    }
    expect(page.listeners.size).toBe(6)
    expect(stops).toEqual([])
    // The 16 live subscriptions still bound it, in the same second.
    for (let i = 0; i < 10; i++) {
      expect(await layer.handle({ sub: 100 + i, verb: 'layout.subscribe', args: [] })).toBeNull()
    }
    expect(await layer.handle({ sub: 200, verb: 'layout.subscribe', args: [] })).toEqual({
      sub: 200,
      error: expect.objectContaining({ code: 'rate_limited', message: 'At most 16 live subscriptions.' }),
    })
    // The call verbs keep their 4 a second.
    for (let i = 0; i < 4; i++) ok(await call(layer, 'layout.get'))
    expect(code(await call(layer, 'layout.get'))).toBe('rate_limited')
    expect(clock.t).toBeGreaterThan(0)
  })

  it('a frame reload is a new layer: nothing it did before counts against the new one', async () => {
    const page = new FakePage()
    const first = harness(page)
    for (let i = 0; i < 4; i++) ok(await call(first.layer, 'layout.get'))
    expect(code(await call(first.layer, 'layout.get'))).toBe('rate_limited')
    first.layer.dispose()
    expect(page.listeners.size).toBe(0)
    const second = harness(page)
    for (let i = 0; i < 4; i++) ok(await call(second.layer, 'layout.get'))
    expect(await second.layer.handle({ sub: 1, verb: 'layout.subscribe', args: [] })).toBeNull()
  })

  it('a call verb sent as a subscription, and a subscription sent as a call, are refused', async () => {
    const { layer } = harness(new FakePage())
    expect(await layer.handle({ sub: 3, verb: 'layout.get', args: [] })).toEqual({
      sub: 3,
      error: expect.objectContaining({ code: 'failed' }),
    })
    expect(code(await call(layer, 'layout.subscribe'))).toBe('failed')
  })
})
