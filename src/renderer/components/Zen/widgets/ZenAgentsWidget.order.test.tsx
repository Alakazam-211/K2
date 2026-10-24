// @vitest-environment jsdom
//
// 0.46.4 (Baden via Appa 2026-10-22; prd-garden-column-collapse-and-agent-
// list-v1 §3, Q3): the person's order and pins in the Agents widget, per
// Garden, on this computer — through the real widget with a fake bridge and
// a fake daemon (`daemon-cli`):
//   - the person's order sits on top of the widget's own (the Home's, or a
//     list-from list's), and unknown addresses never add a row;
//   - before the person pins or unpins in a Garden, the workspace pins; after,
//     that Garden's own pins only;
//   - Alt+Down moves a row and sends the whole order; the row's menu
//     (right-click, Shift+F10) pins in this Garden and sends the pins;
//   - Recent agents first: the list doesn't move; one-agent mode and an older
//     daemon (no route) have no drag and no menu.
// Fixtures are made up (example.test, g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'

const h = vi.hoisted(() => ({
  view: { order: null as string[] | null, pins: null as string[] | null },
  missing: false,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route !== 'zen/agents/order') throw new Error(`unexpected GET ${route}`)
    if (h.missing) throw new Error('unknown zen route')
    return { ok: true, gardens: [{ id: 'g-test0001', name: 'Board', ...h.view, updatedAt: null }] }
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    if (route !== 'zen/agents/order/set') throw new Error(`unexpected POST ${route}`)
    h.posts.push({ route, body })
    if ('order' in body) h.view.order = body.order as string[] | null
    if ('pins' in body) h.view.pins = body.pins as string[] | null
    return { ok: true, changed: true, garden: { id: 'g-test0001', name: 'Board', ...h.view, updatedAt: 'x' } }
  }),
}))

import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { __resetZenAgentsOrderForTests, __settleZenAgentsOrderForTests, useZenAgentsOrderStore } from '@/lib/zen/zen-agents-order'
import { ZenAgentsWidget } from './ZenAgentsWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function row(address: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address,
    label: address.split('::')[0],
    index: 0,
    hostKey: address.split('::')[1],
    server: null,
    role: null,
    reach: 'live',
    auth: 'ok',
    activity: 'idle',
    counts: null,
    working: false,
    needsYou: false,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: null,
    people: [],
    selected: false,
    openable: true,
    avatarUrl: null,
    color: null,
    image: null,
    ...over,
  }
}

function fakeBridge(rows: ZenAgentRow[]): ZenWidgetBridge {
  const call = (verb: string): unknown => {
    switch (verb) {
      case 'agents.list':
        return rows
      case 'agents.subscribe':
        return () => undefined
      case 'agents.home':
        return 'home-work'
      case 'gardens.current':
        return { id: 'g-test0001', name: 'Board', index: 1 }
      case 'homes.list':
        return [{ id: 'home-work', name: 'Work' }]
      case 'conversation.open':
        return Promise.resolve(null)
      default:
        throw new Error(`unexpected verb ${verb}`)
    }
  }
  return {
    widgetId: 'crew',
    caps: new Set(['agents:read']),
    call,
    gardens: { current: () => call('gardens.current'), list: () => [] },
    homes: { list: () => call('homes.list') },
  } as unknown as ZenWidgetBridge
}

function decl(props: Record<string, unknown>): ZenWidgetDecl {
  return { id: 'crew', kind: 'agents', column: 1, props, caps: ['agents:read'], source: 'builtin' }
}

const HOME = { mode: 'home', pins: true, preview: true, 'server-tag': true, unread: true, 'recent-first': false }
const ROWS = () => [
  row('ana::local', { index: 0, pinned: true }),
  row('bo::local', { index: 1 }),
  row('cy::box.example.test', { index: 2, server: 'Box' }),
  row('di::local', { index: 3 }),
]

const listed = (c: HTMLElement) =>
  [...c.querySelectorAll('[data-zen-agents-list] [data-zen-agent-row]')].map((b) => b.getAttribute('data-zen-agent-row'))
const pinned = (c: HTMLElement) => [...c.querySelectorAll('[data-zen-pin]')].map((b) => b.getAttribute('data-zen-pin'))
const listRow = (c: HTMLElement, address: string) => {
  const el = c.querySelector(`[data-zen-agents-list] [data-zen-agent-row="${address}"]`)
  if (!el) throw new Error(`no list row ${address}`)
  return el as HTMLElement
}

async function mount(rows: ZenAgentRow[], props: Record<string, unknown>) {
  const out = render(<ZenAgentsWidget bridge={fakeBridge(rows)} decl={decl(props)} />)
  await vi.waitFor(() => expect(useZenAgentsOrderStore.getState().status).not.toBe('loading'))
  await act(async () => undefined)
  return out
}

beforeEach(() => {
  h.view = { order: null, pins: null }
  h.missing = false
  h.posts = []
})

afterEach(() => {
  cleanup()
  __resetZenAgentsOrderForTests()
  useZenRecentFirstStore.setState({ stored: null })
})

describe('the person’s order and pins in the Agents widget', () => {
  it('nothing set: the Home’s order and the workspace pins, as before', async () => {
    const { container } = await mount(ROWS(), HOME)
    expect(useZenAgentsOrderStore.getState().status).toBe('ready')
    expect(pinned(container)).toEqual(['ana::local'])
    expect(listed(container)).toEqual(['bo::local', 'cy::box.example.test', 'di::local'])
    const root = container.querySelector('[data-zen-widget="agents"]')!
    expect(root.hasAttribute('data-zen-agents-editable')).toBe(true)
    expect(root.hasAttribute('data-zen-agents-own-order')).toBe(false)
    expect(listRow(container, 'bo::local').getAttribute('aria-keyshortcuts')).toBe('Alt+ArrowUp Alt+ArrowDown Shift+F10')
  })

  it('the person’s order beats the Home’s; an address the widget doesn’t show adds nothing', async () => {
    h.view = { order: ['di::local', 'ghost::local', 'cy::box.example.test'], pins: null }
    const { container } = await mount(ROWS(), HOME)
    expect(listed(container)).toEqual(['di::local', 'cy::box.example.test', 'bo::local'])
    expect(container.querySelectorAll('[data-zen-agent-row="ghost::local"]')).toHaveLength(0)
  })

  it('the person’s order beats a list-from list’s order (Q3)', async () => {
    h.view = { order: ['cy::box.example.test'], pins: null }
    const list = { mode: 'list', 'list-from': 'board', pins: false, preview: true }
    const { container } = await mount(ROWS(), list)
    expect(listed(container)).toEqual(['cy::box.example.test', 'ana::local', 'bo::local', 'di::local'])
  })

  it('a Garden’s own pins replace the workspace pins ([] = none here)', async () => {
    h.view = { order: null, pins: ['di::local', 'cy::box.example.test'] }
    const { container } = await mount(ROWS(), HOME)
    expect(pinned(container)).toEqual(['cy::box.example.test', 'di::local'])
    expect(listed(container)).toEqual(['ana::local', 'bo::local'])
    cleanup()
    __resetZenAgentsOrderForTests()
    h.view = { order: null, pins: [] }
    const again = await mount(ROWS(), HOME)
    expect(pinned(again.container)).toEqual([])
  })

  it('Alt+Down moves a row and sends the whole order', async () => {
    const { container } = await mount(ROWS(), HOME)
    const bo = listRow(container, 'bo::local')
    bo.focus()
    await act(async () => void fireEvent.keyDown(bo, { key: 'ArrowDown', altKey: true }))
    expect(listed(container)).toEqual(['cy::box.example.test', 'bo::local', 'di::local'])
    await act(async () => __settleZenAgentsOrderForTests())
    expect(h.posts).toEqual([
      {
        route: 'zen/agents/order/set',
        body: { garden: 'g-test0001', order: ['ana::local', 'cy::box.example.test', 'bo::local', 'di::local'] },
      },
    ])
    // The first row can't go up: nothing is sent.
    const cy = listRow(container, 'cy::box.example.test')
    await act(async () => void fireEvent.keyDown(cy, { key: 'ArrowUp', altKey: true }))
    await act(async () => __settleZenAgentsOrderForTests())
    expect(h.posts).toHaveLength(1)
    expect(container.querySelector('[data-zen-agents-spoken]')?.textContent).toBe('bo moved down')
  })

  it('the row menu pins in this Garden (starting from the workspace pins) and sends the pins', async () => {
    const { container } = await mount(ROWS(), HOME)
    await act(async () => void fireEvent.contextMenu(listRow(container, 'di::local')))
    const menu = document.querySelector('[data-zen-agent-menu]')
    expect(menu).not.toBeNull()
    const ids = [...menu!.querySelectorAll('[data-zen-agent-menu-item]')].map((b) => b.getAttribute('data-zen-agent-menu-item'))
    expect(ids).toEqual(['pin', 'up', 'down'])
    expect((menu!.querySelector('[data-zen-agent-menu-item="down"]') as HTMLButtonElement).disabled).toBe(true)
    await act(async () => void fireEvent.click(menu!.querySelector('[data-zen-agent-menu-item="pin"]')!))
    await act(async () => __settleZenAgentsOrderForTests())
    expect(h.posts).toEqual([{ route: 'zen/agents/order/set', body: { garden: 'g-test0001', pins: ['ana::local', 'di::local'] } }])
    expect(pinned(container)).toEqual(['ana::local', 'di::local'])
    expect(document.querySelector('[data-zen-agent-menu]')).toBeNull()
    // Shift+F10 opens it from the keyboard; now it offers the workspace pins back.
    const pin = container.querySelector('[data-zen-pin="di::local"]') as HTMLElement
    await act(async () => void fireEvent.keyDown(pin, { key: 'F10', shiftKey: true }))
    const again = [...document.querySelectorAll('[data-zen-agent-menu] [data-zen-agent-menu-item]')].map((b) =>
      b.getAttribute('data-zen-agent-menu-item'),
    )
    expect(again).toEqual(['unpin', 'up', 'down', 'workspace-pins'])
    await act(async () => void fireEvent.keyDown(document.querySelector('[data-zen-agent-menu]')!, { key: 'Escape' }))
    expect(document.querySelector('[data-zen-agent-menu]')).toBeNull()
  })

  it('with Recent agents first on, the list doesn’t move (the grid still does)', async () => {
    useZenRecentFirstStore.setState({ stored: true })
    const rows = [...ROWS(), row('ed::local', { index: 4, pinned: true })]
    const { container } = await mount(rows, HOME)
    const bo = listRow(container, 'bo::local')
    expect(bo.hasAttribute('data-row-drag-handle')).toBe(false)
    await act(async () => void fireEvent.keyDown(bo, { key: 'ArrowDown', altKey: true }))
    await act(async () => __settleZenAgentsOrderForTests())
    expect(h.posts).toEqual([])
    const pin = container.querySelector('[data-zen-pin="ana::local"]') as HTMLElement
    expect(pin.hasAttribute('data-row-drag-handle')).toBe(true)
  })

  it('one-agent mode and an older daemon: no drag, no menu, nothing sent', async () => {
    const one = await mount(ROWS(), { agent: 'bo', preview: true })
    expect(one.container.querySelector('[data-zen-widget="agents"]')?.hasAttribute('data-zen-agents-editable')).toBe(false)
    cleanup()
    __resetZenAgentsOrderForTests()
    h.missing = true
    const { container } = await mount(ROWS(), HOME)
    expect(useZenAgentsOrderStore.getState().status).toBe('unsupported')
    const bo = listRow(container, 'bo::local')
    expect(bo.hasAttribute('data-row-drag-handle')).toBe(false)
    expect(bo.hasAttribute('aria-keyshortcuts')).toBe(false)
    await act(async () => void fireEvent.contextMenu(bo))
    expect(document.querySelector('[data-zen-agent-menu]')).toBeNull()
    await act(async () => void fireEvent.keyDown(bo, { key: 'ArrowDown', altKey: true }))
    expect(h.posts).toEqual([])
    expect(listed(container)).toEqual(['bo::local', 'cy::box.example.test', 'di::local'])
  })
})
