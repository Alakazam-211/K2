// @vitest-environment jsdom
//
// 0.46.4 — the person's order and pins in a Garden's Agents widget
// (`zen-agents-order.ts`): the pure helpers (order on top of the widget's,
// Garden pins vs workspace pins, moves, the merge that keeps hidden rows,
// the grid's drop gap) and the store (this computer's daemon is the truth:
// read, optimistic writes in order, a refused write reads back, the change
// event re-reads, an older daemon disables it). Only the daemon
// (`daemon-cli`) and the event bus are faked. Fail loudly.

import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  get: null as null | (() => unknown),
  post: null as null | ((body: Record<string, unknown>) => unknown),
  onChanged: [] as Array<() => void>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (route !== 'zen/agents/order' || !h.get) throw new Error(`unexpected GET ${route}`)
    return h.get()
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: Record<string, unknown>) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    if (route !== 'zen/agents/order/set' || !h.post) throw new Error(`unexpected POST ${route}`)
    return h.post(body ?? {})
  }),
}))

vi.mock('@/stores/session-events', () => ({
  onZenAgentsOrderChanged: vi.fn((_scope: unknown, fn: () => void) => {
    h.onChanged.push(fn)
    return () => {
      h.onChanged = h.onChanged.filter((f) => f !== fn)
    }
  }),
}))

import {
  __resetZenAgentsOrderForTests,
  __settleZenAgentsOrderForTests,
  loadZenAgentsOrder,
  mergeZenAgentsOrder,
  moveZenAddress,
  orderZenAgentRows,
  parseZenAgentsOrder,
  retainZenAgentsOrderLive,
  setZenAgentsView,
  toggleZenPin,
  useZenAgentsOrderStore,
  ZEN_AGENTS_ORDER_MAX,
  ZenAgentsOrderParseError,
  zenGridGapAt,
  zenRowPinned,
} from './zen-agents-order'

const r = (address: string, pinned = false) => ({ address, pinned })
const addrs = (rows: { address: string }[]) => rows.map((x) => x.address)

afterEach(() => {
  __resetZenAgentsOrderForTests()
  h.calls = []
  h.get = null
  h.post = null
  h.onChanged = []
})

describe('the helpers', () => {
  it('parses the daemon answer and throws on bad shapes', () => {
    expect(
      parseZenAgentsOrder({
        ok: true,
        gardens: [
          { id: 'g-1', name: 'Garden 1', order: ['b::local', 'a::local'], pins: null, updatedAt: '2026-10-24T10:00:00Z' },
          { id: 'g-2', name: 'Garden 2', order: null, pins: [], updatedAt: null },
        ],
      }),
    ).toEqual({ 'g-1': { order: ['b::local', 'a::local'], pins: null }, 'g-2': { order: null, pins: [] } })
    expect(() => parseZenAgentsOrder({ ok: true })).toThrow(ZenAgentsOrderParseError)
    expect(() => parseZenAgentsOrder({ gardens: [{ name: 'no id' }] })).toThrow('without an id')
    expect(() => parseZenAgentsOrder({ gardens: [{ id: 'g-1', order: 'a::local' }] })).toThrow('g-1.order is not a list')
    expect(() => parseZenAgentsOrder({ gardens: [{ id: 'g-1', pins: [1] }] })).toThrow('g-1.pins is not a list')
  })

  it('puts the listed rows first in the person’s order, the rest in their own; unknown addresses never add a row', () => {
    const rows = [r('a::local'), r('b::local'), r('c::box.example.test'), r('d::local')]
    expect(addrs(orderZenAgentRows(rows, null))).toEqual(['a::local', 'b::local', 'c::box.example.test', 'd::local'])
    expect(addrs(orderZenAgentRows(rows, ['C::BOX.example.test', 'ghost::local', 'b::local']))).toEqual([
      'c::box.example.test',
      'b::local',
      'a::local',
      'd::local',
    ])
    expect(orderZenAgentRows(rows, ['ghost::local'])).toHaveLength(4)
    expect(orderZenAgentRows([], ['a::local'])).toEqual([])
  })

  it('pins: the workspace pin until the Garden has its own list, then that list alone', () => {
    expect(zenRowPinned(r('a::local', true), null)).toBe(true)
    expect(zenRowPinned(r('a::local', false), null)).toBe(false)
    expect(zenRowPinned(r('a::local', true), [])).toBe(false)
    expect(zenRowPinned(r('a::local', false), [' A::Local '])).toBe(true)
  })

  it('moves a row before or after another; nothing moves → null', () => {
    const seq = ['a', 'b', 'c', 'd']
    expect(moveZenAddress(seq, 'd', { before: 'b' })).toEqual(['a', 'd', 'b', 'c'])
    expect(moveZenAddress(seq, 'a', { after: 'c' })).toEqual(['b', 'c', 'a', 'd'])
    expect(moveZenAddress(seq, 'a', { before: 'b' })).toBeNull()
    expect(moveZenAddress(seq, 'b', { after: 'a' })).toBeNull()
    expect(moveZenAddress(seq, 'ghost', { before: 'a' })).toBeNull()
    expect(moveZenAddress(seq, 'a', { before: 'ghost' })).toBeNull()
    expect(moveZenAddress(seq, 'a', { before: 'A' })).toBeNull()
  })

  it('a stored order keeps rows that aren’t drawn now right after the row they followed', () => {
    // offline1 followed b; offline0 led the list.
    const stored = ['offline0', 'a', 'b', 'offline1', 'c']
    expect(mergeZenAgentsOrder(stored, ['c', 'a', 'b'])).toEqual(['offline0', 'c', 'a', 'b', 'offline1'])
    expect(mergeZenAgentsOrder(null, ['c', 'a'])).toEqual(['c', 'a'])
    // Over the cap: hidden rows go first.
    const drawn = Array.from({ length: ZEN_AGENTS_ORDER_MAX }, (_, i) => `r${i}`)
    const merged = mergeZenAgentsOrder(['hidden', ...drawn], drawn)
    expect(merged).toHaveLength(ZEN_AGENTS_ORDER_MAX)
    expect(merged).not.toContain('hidden')
    expect(mergeZenAgentsOrder(null, [...drawn, 'r-extra'])).toHaveLength(ZEN_AGENTS_ORDER_MAX)
  })

  it('the first pin or unpin in a Garden starts from the workspace pins of the rows shown', () => {
    const rows = [r('a::local', true), r('b::local'), r('c::local', true)]
    expect(toggleZenPin(null, rows, 'b::local')).toEqual(['a::local', 'c::local', 'b::local'])
    expect(toggleZenPin(null, rows, 'a::local')).toEqual(['c::local'])
    expect(toggleZenPin(['B::LOCAL'], rows, 'b::local')).toEqual([])
    expect(toggleZenPin([], rows, 'a::local')).toEqual(['a::local'])
  })

  it('the grid’s drop gap follows reading order', () => {
    // Two rows of three 100×100 cells, 20px gaps.
    const cell = (i: number) => ({ left: (i % 3) * 120, width: 100, top: Math.floor(i / 3) * 120, bottom: Math.floor(i / 3) * 120 + 100 })
    const rects = [0, 1, 2, 3, 4].map(cell)
    expect(zenGridGapAt(rects, 10, 50)).toBe(0) // left half of the first cell
    expect(zenGridGapAt(rects, 70, 50)).toBe(1) // right half of the first cell
    expect(zenGridGapAt(rects, 330, 50)).toBe(3) // past the first row's end
    expect(zenGridGapAt(rects, 10, 170)).toBe(3) // second row, first cell's left half
    expect(zenGridGapAt(rects, 300, 170)).toBe(5) // past the last cell
  })
})

describe('the store', () => {
  const answer = (gardens: Array<{ id: string; order: string[] | null; pins: string[] | null }>) => ({
    ok: true,
    gardens: gardens.map((g) => ({ name: g.id, updatedAt: null, ...g })),
  })

  it('reads this computer’s daemon (never another server)', async () => {
    h.get = () => answer([{ id: 'g-1', order: ['b::local'], pins: null }])
    await loadZenAgentsOrder()
    const s = useZenAgentsOrderStore.getState()
    expect(s.status).toBe('ready')
    expect(s.byGarden['g-1']).toEqual({ order: ['b::local'], pins: null })
    expect(h.calls).toEqual([{ method: 'GET', hostKey: 'local', route: 'zen/agents/order', data: undefined }])
  })

  it('an older daemon (no route) turns it off; another failure keeps what is drawn', async () => {
    h.get = () => {
      throw new Error('unknown zen route')
    }
    await loadZenAgentsOrder()
    expect(useZenAgentsOrderStore.getState().status).toBe('unsupported')

    h.get = () => answer([{ id: 'g-1', order: ['a::local'], pins: null }])
    await loadZenAgentsOrder()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    h.get = () => {
      throw new Error('daemon unreachable')
    }
    await loadZenAgentsOrder()
    const s = useZenAgentsOrderStore.getState()
    expect(s.status).toBe('failed')
    expect(s.error).toBe('daemon unreachable')
    expect(s.byGarden['g-1']).toEqual({ order: ['a::local'], pins: null })
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
  })

  it('a change is drawn at once, sent in order, and the daemon’s copy wins', async () => {
    h.get = () => answer([{ id: 'g-1', order: null, pins: null }])
    await loadZenAgentsOrder()
    let release: () => void = () => undefined
    const gate = new Promise<void>((res) => (release = res))
    h.post = async (body) => {
      await gate
      return { ok: true, changed: true, garden: { id: body.garden, name: 'Garden 1', order: body.order ?? ['b::local', 'a::local'], pins: body.pins ?? null, updatedAt: 'x' } }
    }
    void setZenAgentsView('g-1', { order: ['b::local', 'a::local'] })
    void setZenAgentsView('g-1', { pins: ['a::local'] })
    // Drawn at once, before the daemon answers.
    expect(useZenAgentsOrderStore.getState().byGarden['g-1']).toEqual({ order: ['b::local', 'a::local'], pins: ['a::local'] })
    release()
    await __settleZenAgentsOrderForTests()
    const posts = h.calls.filter((c) => c.method === 'POST')
    expect(posts).toEqual([
      { method: 'POST', hostKey: 'local', route: 'zen/agents/order/set', data: { garden: 'g-1', order: ['b::local', 'a::local'] } },
      { method: 'POST', hostKey: 'local', route: 'zen/agents/order/set', data: { garden: 'g-1', pins: ['a::local'] } },
    ])
    expect(useZenAgentsOrderStore.getState().byGarden['g-1']).toEqual({ order: ['b::local', 'a::local'], pins: ['a::local'] })
    // null goes back to the default, and is sent as null.
    void setZenAgentsView('g-1', { order: null })
    await __settleZenAgentsOrderForTests()
    expect(h.calls[h.calls.length - 1]?.data).toEqual({ garden: 'g-1', order: null })
  })

  it('a refused write reads the daemon’s copy back', async () => {
    h.get = () => answer([{ id: 'g-1', order: ['a::local', 'b::local'], pins: null }])
    await loadZenAgentsOrder()
    h.post = () => {
      throw new Error('owner_only')
    }
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    void setZenAgentsView('g-1', { order: ['b::local', 'a::local'] })
    expect(useZenAgentsOrderStore.getState().byGarden['g-1'].order).toEqual(['b::local', 'a::local'])
    await __settleZenAgentsOrderForTests()
    expect(useZenAgentsOrderStore.getState().byGarden['g-1'].order).toEqual(['a::local', 'b::local'])
    expect(h.calls.map((c) => `${c.method} ${c.route}`)).toEqual([
      'GET zen/agents/order',
      'POST zen/agents/order/set',
      'GET zen/agents/order',
    ])
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
  })

  it('a read that lands while a change is on its way never undoes it', async () => {
    h.get = () => answer([{ id: 'g-1', order: null, pins: null }])
    await loadZenAgentsOrder()
    let release: () => void = () => undefined
    const gate = new Promise<void>((res) => (release = res))
    h.post = async (body) => {
      await gate
      return { ok: true, changed: true, garden: { id: 'g-1', name: 'Garden 1', order: body.order, pins: null, updatedAt: 'x' } }
    }
    void setZenAgentsView('g-1', { order: ['z::local'] })
    await loadZenAgentsOrder() // the old copy: dropped
    expect(useZenAgentsOrderStore.getState().byGarden['g-1'].order).toEqual(['z::local'])
    h.get = () => answer([{ id: 'g-1', order: ['z::local'], pins: null }])
    release()
    await __settleZenAgentsOrderForTests()
    expect(useZenAgentsOrderStore.getState().byGarden['g-1'].order).toEqual(['z::local'])
    expect(h.calls.filter((c) => c.method === 'GET')).toHaveLength(3) // first, dropped, after the write
  })

  it('stays live while retained: the change event and window focus re-read; released, neither does', async () => {
    h.get = () => answer([{ id: 'g-1', order: null, pins: null }])
    const releaseA = retainZenAgentsOrderLive()
    const releaseB = retainZenAgentsOrderLive()
    await vi.waitFor(() => expect(useZenAgentsOrderStore.getState().status).toBe('ready'))
    expect(h.onChanged).toHaveLength(1)
    h.get = () => answer([{ id: 'g-1', order: ['x::local'], pins: null }])
    h.onChanged[0]()
    await vi.waitFor(() => expect(useZenAgentsOrderStore.getState().byGarden['g-1'].order).toEqual(['x::local']))
    releaseA()
    expect(h.onChanged).toHaveLength(1)
    releaseB()
    expect(h.onChanged).toHaveLength(0)
  })

  it('needs a Garden and a change', () => {
    expect(() => setZenAgentsView('', { order: [] })).toThrow('needs a Garden id')
    expect(() => setZenAgentsView('g-1', {})).toThrow('needs order or pins')
  })
})
