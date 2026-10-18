// Garden data in the renderer (contract-garden-data-v1 §7.1, §7.3, §12.2):
// the store's calls, `by` in the person's words, push that never shows half
// a rev, resync, the one stale reload, and the custom layer's verbs (every
// verb's success and refusal paths, the Sources taint, 16 subscriptions).
// The daemon is faked: `call` answers like `/cli/zen/call`, throwing the
// error envelope text as the daemon client does.
import { describe, expect, it } from 'vitest'
import type { ZenWidgetBridge } from './zen-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import { createZenCustomLayer, type ZenCustomLayerDeps } from './zen-custom-bridge'
import {
  createZenCustomStore,
  zenDataSpace,
  zenStoreBy,
  zenStoreErrorFrom,
  ZEN_STORE_VERBS,
  type ZenCustomStore,
  type ZenCustomStoreDeps,
} from './zen-custom-store'

const settle = (): Promise<void> => new Promise((r) => setTimeout(r, 0))
const flush = async (): Promise<void> => {
  for (let i = 0; i < 20; i++) await settle()
}

function envelope(code: string, hint = code, extra: Record<string, unknown> = {}): Error {
  return new Error(JSON.stringify({ error: { code, hint, ...extra } }))
}

type Page = { head: number; items: unknown[]; next: { since: number; after?: string } | null; partial: boolean }

interface Fake {
  deps: ZenCustomStoreDeps
  calls: Array<{ verb: string; args: unknown[] }>
  changed: Array<(space: string, rev: number) => void>
  stale: number
  timers: Array<() => void>
  answer: (verb: string, args: unknown[]) => unknown
}

function fake(answer: (verb: string, args: unknown[]) => unknown): Fake {
  const f: Fake = { calls: [], changed: [], stale: 0, timers: [], answer } as unknown as Fake
  f.deps = {
    call: async (verb, args) => {
      f.calls.push({ verb, args })
      if (verb === 'viewer.get') return { kind: 'owner', name: 'Baden' }
      return f.answer(verb, args)
    },
    onChanged: (cb) => {
      f.changed.push(cb)
      return () => {
        f.changed = f.changed.filter((x) => x !== cb)
      }
    },
    space: 'data:board',
    onStale: () => {
      f.stale += 1
    },
    setTimer: (fn) => {
      f.timers.push(fn)
      return f.timers.length
    },
    clearTimer: () => undefined,
    now: () => 0,
  }
  return f
}

function change(key: string, rev: number, by = 'agent:board-keeper'): Record<string, unknown> {
  return { key, rev, deleted: false, value: { k: key }, by, at: 1 }
}

describe('the store: calls and names', () => {
  it('names "you" with the person’s name, everywhere a `by` comes back', async () => {
    expect(zenStoreBy('you', 'Baden')).toBe('Baden')
    expect(zenStoreBy('you via board', 'Baden')).toBe('Baden via board')
    expect(zenStoreBy('agent:board-keeper', 'Baden')).toBe('agent:board-keeper')
    const f = fake((verb) => {
      if (verb === 'store.get') return { key: 'a', rev: 1, value: 1, updatedAt: 1, by: 'you via board' }
      if (verb === 'store.list') return { items: [{ key: 'a', rev: 1, value: 1, updatedAt: 1, by: 'you' }], next: null }
      return null
    })
    const s = createZenCustomStore(f.deps)
    await expect(s.call('store.get', ['a'])).resolves.toMatchObject({ by: 'Baden via board' })
    await expect(s.call('store.list', [{}])).resolves.toMatchObject({ items: [{ by: 'Baden' }] })
    await s.call('store.get', ['a'])
    expect(f.calls.filter((c) => c.verb === 'viewer.get')).toHaveLength(1)
  })

  it('turns the daemon’s envelope into the shared error list, and reloads a stale widget once', async () => {
    expect(zenStoreErrorFrom(envelope('not_granted', 'no grant')).wire).toEqual({ code: 'not_granted', message: 'no grant' })
    const c = zenStoreErrorFrom(envelope('conflict', 'obj/7 changed', { key: 'obj/7', rev: 42 }))
    expect(c.wire).toEqual({ code: 'conflict', message: 'obj/7 changed (now rev 42)', last: { key: 'obj/7', rev: 42 } })
    expect(zenStoreErrorFrom(envelope('widget_not_found')).wire.code).toBe('failed')
    expect(zenStoreErrorFrom(new Error('boom')).wire).toEqual({ code: 'failed', message: 'boom' })
    const f = fake(() => {
      throw envelope('stale_widget', 'the widget changed')
    })
    const s = createZenCustomStore(f.deps)
    await expect(s.call('store.put', ['k', 1])).rejects.toMatchObject({ wire: { code: 'stale_widget' } })
    await expect(s.call('store.put', ['k', 1])).rejects.toMatchObject({ wire: { code: 'stale_widget' } })
    expect(f.stale).toBe(1)
    await expect(s.call('thread.post', [])).rejects.toMatchObject({ wire: { code: 'unknown_verb' } })
    expect(zenDataSpace('k2:diary@2')).toBe('data:k2:diary')
    expect(zenDataSpace('board')).toBe('data:board')
  })
})

describe('the store: subscribe', () => {
  it('delivers the whole state first, then each rev from any writer; never half a rev', async () => {
    const pages: Page[] = [
      { head: 3, items: [change('obj/1', 3, 'you')], next: null, partial: false },
      // rev 7 is split over two pages: nothing is delivered until both are in.
      { head: 7, items: [change('obj/2', 7)], next: { since: 7, after: 'obj/2' }, partial: true },
      { head: 7, items: [change('obj/3', 7)], next: null, partial: false },
    ]
    const f = fake((verb, args) => {
      if (verb !== 'store.changes') throw new Error(verb)
      expect((args[0] as { prefix: string }).prefix).toBe('obj/')
      return pages.shift()
    })
    const s = createZenCustomStore(f.deps)
    const got: unknown[] = []
    const off = s.subscribe('obj/', (v) => got.push(v), (e) => got.push({ failed: e }))
    await flush()
    expect(got).toEqual([[{ key: 'obj/1', rev: 3, value: { k: 'obj/1' }, by: 'Baden' }]])
    expect(f.calls.filter((c) => c.verb === 'store.changes')[0].args[0]).toEqual({ since: 0, prefix: 'obj/' })
    for (const cb of f.changed) cb('data:other', 9)
    for (const cb of f.changed) cb('data:board', 7)
    await flush()
    expect(got).toHaveLength(2)
    expect((got[1] as Array<{ key: string }>).map((r) => r.key)).toEqual(['obj/2', 'obj/3'])
    const asked = f.calls.filter((c) => c.verb === 'store.changes').map((c) => c.args[0])
    expect(asked.slice(1)).toEqual([
      { since: 3, prefix: 'obj/' },
      { since: 7, after: 'obj/2', prefix: 'obj/' },
    ])
    off()
    expect(f.changed).toHaveLength(0)
  })

  it('resync lists everything again and hands {resync, items}; then follows from the kept history', async () => {
    let n = 0
    const f = fake((verb, args) => {
      if (verb === 'store.list') return { items: [{ key: 'a', rev: 9, value: 1, updatedAt: 1, by: 'you' }], next: null }
      n += 1
      if (n === 1) throw envelope('resync', 'older than kept history', { reapedRev: 8 })
      expect(args[0]).toMatchObject({ since: 8 })
      return { head: 9, items: [change('a', 9)], next: null, partial: false }
    })
    const s = createZenCustomStore(f.deps)
    const got: unknown[] = []
    s.subscribe({ since: 2 }, (v) => got.push(v), (e) => got.push({ failed: e }))
    await flush()
    expect(got[0]).toEqual({ resync: true, items: [{ key: 'a', rev: 9, value: 1, updatedAt: 1, by: 'Baden' }] })
    expect(got[1]).toEqual([{ key: 'a', rev: 9, value: { k: 'a' }, by: 'agent:board-keeper' }])
  })

  it('a refusal ends the subscription; rate_limited retries later', async () => {
    let mode = 'limited'
    const f = fake(() => {
      if (mode === 'limited') throw envelope('rate_limited', 'too many', { retryAfterSecs: 1 })
      if (mode === 'ok') return { head: 1, items: [change('x', 1)], next: null, partial: false }
      throw envelope('not_granted', 'no grant')
    })
    const s = createZenCustomStore(f.deps)
    const got: unknown[] = []
    s.subscribe('', (v) => got.push(v), (e) => got.push({ failed: e.code }))
    await flush()
    expect(got).toEqual([])
    expect(f.timers).toHaveLength(1)
    mode = 'ok'
    f.timers.shift()!()
    await flush()
    expect(got).toHaveLength(1)
    mode = 'refuse'
    for (const cb of f.changed) cb('data:board', 2)
    await flush()
    expect(got[1]).toEqual({ failed: 'not_granted' })
    expect(f.changed).toHaveLength(0)
    expect(() => s.subscribe({ since: -1 }, () => undefined, () => undefined)).toThrow(/since/)
  })
})

// ── through the custom layer ─────────────────────────────────────────────

function payload(over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id: 'b1',
    kind: 'custom',
    widget: 'board',
    column: 0,
    props: { config: {} },
    caps: ['store:read', 'store:write'],
    requested: ['store:read', 'store:write'],
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

function layerWith(store: ZenCustomStore | undefined, w: ZenCustomWidgetPayload = payload()) {
  const pushes: Array<{ sub: number; value: unknown }> = []
  const errors: Array<{ sub: number; error: unknown }> = []
  const deps: ZenCustomLayerDeps = {
    widget: () => w,
    gardenId: () => 'g-test0001',
    inner: { widgetId: 'b1', caps: new Set<string>(), call: () => null } as unknown as ZenWidgetBridge,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push: (sub, value) => pushes.push({ sub, value }),
    pushError: (sub, error) => errors.push({ sub, error }),
    store,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({}),
    onThemeChange: () => () => undefined,
    now: () => 1_000_000,
  }
  return { layer: createZenCustomLayer(deps), pushes, errors }
}

async function ask(layer: ReturnType<typeof createZenCustomLayer>, verb: string, ...args: unknown[]): Promise<ZenFrameReply> {
  const r = await layer.handle({ id: 1, verb, args })
  if (!r) throw new Error('no reply')
  return r
}

describe('the custom layer: Garden data verbs', () => {
  it('every garden-local verb reaches the store and answers; refusals come back as the shared list', async () => {
    const seen: string[] = []
    const f = fake((verb) => {
      seen.push(verb)
      if (verb === 'store.put') throw envelope('not_granted', 'no grant for this key', { key: 'x' })
      return { verb }
    })
    const { layer } = layerWith(createZenCustomStore(f.deps))
    for (const verb of ZEN_STORE_VERBS) {
      const r = await ask(layer, verb, 'k', 1)
      if (verb === 'store.put') expect(r).toMatchObject({ ok: false, error: { code: 'not_granted', message: 'no grant for this key' } })
      else if (verb === 'viewer.get') expect(r).toMatchObject({ ok: true, value: { kind: 'owner', name: 'Baden' } })
      else expect(r, verb).toMatchObject({ ok: true, value: { verb } })
    }
    expect(new Set(seen)).toEqual(new Set([...ZEN_STORE_VERBS].filter((v) => v !== 'viewer.get')))
  })

  it('caps: no store caps → cap_not_granted (saves still work); sources:read keeps store:write it asked for; no store → verb_unavailable', async () => {
    const f = fake(() => null)
    const none = layerWith(createZenCustomStore(f.deps), payload({ caps: [], requested: [] }))
    expect(await ask(none.layer, 'store.get', 'k')).toMatchObject({ ok: false, error: { code: 'cap_not_granted', cap: 'store:read' } })
    expect(await ask(none.layer, 'save.set', 'k', 1)).toMatchObject({ ok: true })
    const reader = layerWith(
      createZenCustomStore(f.deps),
      payload({ caps: ['store:read', 'store:write', 'sources:read'], requested: ['store:read', 'store:write', 'sources:read'] }),
    )
    expect(await ask(reader.layer, 'store.put', 'k', 1)).toMatchObject({ ok: true })
    expect(await ask(reader.layer, 'store.get', 'k')).toMatchObject({ ok: true })
    const readOnly = layerWith(
      createZenCustomStore(f.deps),
      payload({ caps: ['store:read', 'sources:read'], requested: ['store:read', 'sources:read'] }),
    )
    expect(await ask(readOnly.layer, 'store.put', 'k', 1)).toMatchObject({ ok: false, error: { code: 'cap_not_granted', cap: 'store:write' } })
    const bare = layerWith(undefined)
    expect(await ask(bare.layer, 'store.get', 'k')).toMatchObject({ ok: false, error: { code: 'verb_unavailable' } })
  })

  it('subscribe rides store.changes, pushes plain values, ends on a refusal, and counts toward the 16', async () => {
    let refuse = false
    const f = fake(() => {
      if (refuse) throw envelope('author_unknown', 'nobody signed this widget')
      return { head: 1, items: [change('obj/1', 1)], next: null, partial: false }
    })
    const { layer, pushes, errors } = layerWith(createZenCustomStore(f.deps))
    expect(await layer.handle({ sub: 1, verb: 'store.changes', args: ['obj/'] })).toBeNull()
    await flush()
    expect(pushes).toEqual([{ sub: 1, value: [{ key: 'obj/1', rev: 1, value: { k: 'obj/1' }, by: 'agent:board-keeper' }] }])
    refuse = true
    for (const cb of f.changed) cb('data:board', 2)
    await flush()
    expect(errors).toEqual([{ sub: 1, error: { code: 'author_unknown', message: 'nobody signed this widget' } }])
    expect(layer.subscriptions).toBe(0)
    refuse = false
    for (let sub = 10; sub < 26; sub++) expect(await layer.handle({ sub, verb: 'store.changes', args: [''] })).toBeNull()
    expect(layer.subscriptions).toBe(16)
    expect(await layer.handle({ sub: 99, verb: 'store.changes', args: [''] })).toMatchObject({ sub: 99, error: { code: 'rate_limited' } })
    // A call row other than store.changes still can't be subscribed.
    expect(await layer.handle({ sub: 100, verb: 'store.get', args: ['k'] })).toMatchObject({ sub: 100, error: { code: 'failed' } })
    layer.dispose()
  })
})
