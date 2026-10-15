// @vitest-environment jsdom
//
// Garden 1 from K2 Companion (0.45.6, Rosson 2026-10-15), through the real
// Agents and Conversation widgets with a fake bridge:
//   - `pins`: pinned agents in a three-across grid at the top, every pin
//     drawn (no paging), each agent once; off, the list is unchanged;
//   - `settings`: the cog opens the Garden's own settings; its "Recent
//     agents first" switch is off by default and reorders the list below
//     the pins when on; off, no cog;
//   - a send from the Garden's box records the agent for recent-first;
//   - `activity-drawer`: the live turn shows in the drawer above the box,
//     instead of the typing dots; off, the dots stay.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import type { ThreadTurn } from '@/components/SessionView/overlayThread'
import { ZEN_RECENT_FIRST_KEY, useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversation } from './ZenConversationWidget'
import { __resetZenDraftsForTests } from './ZenCompose'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function row(handle: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address: `${handle}::local`,
    label: handle,
    index: 0,
    hostKey: 'local',
    server: null,
    role: null,
    reach: 'live',
    auth: null,
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
    pinned: false,
    ...over,
  }
}

interface Fake {
  bridge: ZenWidgetBridge
  opened: string[]
  posts: string[]
  pushThread(view: ZenThreadView): void
}

function fakeBridge(rows: ZenAgentRow[]): Fake {
  const opened: string[] = []
  const posts: string[] = []
  let threadListener: ((v: ZenThreadView) => void) | null = null
  const bridge = {
    widgetId: 'agents',
    caps: new Set<string>(['agents:read', 'thread:read', 'thread:post']),
    call(verb: string, ...args: unknown[]): unknown {
      switch (verb) {
        case 'agents.list':
          return rows
        case 'agents.subscribe':
          return () => {}
        case 'agents.home':
          return 'home-1'
        case 'conversation.open':
          opened.push(String(args[0]))
          return Promise.resolve()
        case 'thread.subscribe':
          threadListener = args[1] as (v: ZenThreadView) => void
          return () => {
            threadListener = null
          }
        case 'thread.post':
          posts.push(String(args[1]))
          return Promise.resolve()
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    gardens: { list: () => [], current: () => ({ id: 'g-1', name: 'Garden 1' }), switch: () => {}, create: () => Promise.reject(new Error('no')) },
    homes: { list: () => [{ id: 'home-1', name: 'Home', rows: rows.length }] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return {
    bridge,
    opened,
    posts,
    pushThread(view) {
      const fn = threadListener
      if (!fn) throw new Error('nobody subscribed to the thread')
      act(() => fn(view))
    },
  }
}

function agentsDecl(props: Record<string, unknown>): ZenWidgetDecl {
  return { id: 'agents', kind: 'agents', column: 0, props: { 'home-picker': false, ...props }, caps: ['agents:read'], source: 'builtin' } as ZenWidgetDecl
}

const pinCells = (): string[] => Array.from(document.querySelectorAll('[data-zen-pin]')).map((e) => e.getAttribute('data-zen-pin') ?? '')
const listRows = (): string[] => Array.from(document.querySelectorAll('[data-zen-agent-row]')).map((e) => e.getAttribute('data-zen-agent-row') ?? '')

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

beforeEach(() => {
  localStorage.clear()
  useZenRecentFirstStore.getState().reload()
  __resetZenDraftsForTests()
})

afterEach(() => {
  cleanup()
  localStorage.clear()
  useZenRecentFirstStore.getState().reload()
})

const ROWS = (): ZenAgentRow[] => [
  row('alpha'),
  row('pinA', { pinned: true }),
  row('beta'),
  row('pinB', { pinned: true, activity: 'working', working: true }),
  row('gamma'),
]

describe('Garden 1 pin grid (`pins`)', () => {
  it('draws pinned agents in the grid and the rest in the list, each once', async () => {
    const fb = fakeBridge(ROWS())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ pins: true })} />)
    expect(pinCells()).toEqual(['pinA::local', 'pinB::local'])
    expect(listRows()).toEqual(['alpha::local', 'beta::local', 'gamma::local'])
    const grid = el('[data-zen-pins]')
    // Two pins: centred cells of the three-column width.
    expect(grid.style.justifyContent).toBe('center')
    expect(grid.style.gridTemplateColumns).toContain('repeat(2, calc(')
    expect(el('[data-zen-pin="pinB::local"] [data-zen-pin-activity]').textContent).toBe('working')
    await act(async () => void fireEvent.click(el('[data-zen-pin="pinA::local"]')))
    expect(fb.opened).toEqual(['pinA::local'])
  })

  it('keeps adding rows past nine pins: no paging', () => {
    const rows = Array.from({ length: 11 }, (_, i) => row(`p${i}`, { pinned: true }))
    const fb = fakeBridge(rows)
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ pins: true })} />)
    expect(pinCells()).toHaveLength(11)
    expect(el('[data-zen-pins]').getAttribute('data-zen-pin-count')).toBe('11')
    expect(el('[data-zen-pins]').style.gridTemplateColumns).toBe('repeat(3, minmax(0, 1fr))')
    expect(listRows()).toEqual([])
  })

  it('off (an own copy, any other Garden): no grid, the list unchanged', () => {
    const fb = fakeBridge(ROWS())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({})} />)
    expect(document.querySelector('[data-zen-pins]')).toBeNull()
    expect(listRows()).toEqual(['alpha::local', 'pinA::local', 'beta::local', 'pinB::local', 'gamma::local'])
    expect(document.querySelector('[data-zen-agents-settings]')).toBeNull()
  })
})

describe('Garden settings (`settings`): Recent agents first', () => {
  it('the cog opens the Garden’s own settings; the switch is off, and on reorders the list below the pins', () => {
    useZenRecentFirstStore.getState().markSent('gamma::local', 200)
    useZenRecentFirstStore.getState().markSent('beta::local', 100)
    useZenRecentFirstStore.getState().markSent('pinA::local', 300)
    const fb = fakeBridge(ROWS())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ pins: true, settings: true })} />)
    // Off by default: the Home's order.
    expect(listRows()).toEqual(['alpha::local', 'beta::local', 'gamma::local'])
    act(() => void fireEvent.click(el('[data-zen-agents-settings]')))
    const dialog = el('[data-testid="zen-garden-settings"]')
    expect(dialog.getAttribute('aria-label')).toBe('Garden settings')
    const sw = el('[data-zen-recent-first-switch]')
    expect(sw.getAttribute('aria-checked')).toBe('false')
    act(() => void fireEvent.click(sw))
    expect(el('[data-zen-recent-first-switch]').getAttribute('aria-checked')).toBe('true')
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBe('1')
    // Pins stay in the grid; the list leads with the newest sends.
    expect(pinCells()).toEqual(['pinA::local', 'pinB::local'])
    expect(listRows()).toEqual(['gamma::local', 'beta::local', 'alpha::local'])
    act(() => void fireEvent.click(el('[data-zen-garden-settings-done]')))
    expect(document.querySelector('[data-testid="zen-garden-settings"]')).toBeNull()
    // Turned back off: the Home's order again.
    act(() => useZenRecentFirstStore.getState().setOn(false))
    expect(listRows()).toEqual(['alpha::local', 'beta::local', 'gamma::local'])
  })

  it('with no grid, recent first still keeps pinned agents first', () => {
    useZenRecentFirstStore.getState().setOn(true)
    useZenRecentFirstStore.getState().markSent('gamma::local', 5)
    const fb = fakeBridge(ROWS())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ settings: true })} />)
    expect(listRows()).toEqual(['pinA::local', 'pinB::local', 'gamma::local', 'alpha::local', 'beta::local'])
  })
})

function liveTurn(): ThreadTurn {
  const now = Date.now()
  return {
    turnId: 't1',
    state: 'working',
    phase: 'tool',
    startedAt: now - 8_000,
    phaseSince: now - 3_000,
    line: 'Read `src/main.ts`',
    subagents: 1,
    subagentsDone: 0,
    background: 0,
    tally: { read: 2, search: 0, cmd: 1, edit: 0 },
    waitingOn: null,
    end: null,
    rev: 1,
    skewMs: 0,
  }
}

function threadView(over: Partial<ZenThreadView> = {}): ZenThreadView {
  return {
    address: 'cortana::local',
    phase: 'ready',
    note: null,
    items: [],
    loaded: true,
    hasMore: false,
    loadingOlder: false,
    error: null,
    turn: { state: 'working', since: Date.now() - 8_000 },
    liveTurn: liveTurn(),
    ...over,
  }
}

describe('the activity drawer (`activity-drawer`)', () => {
  it('on: the live turn shows above the box instead of the dots; a click expands it', () => {
    const fb = fakeBridge([])
    const r = row('cortana', { activity: 'working', working: true, selected: true })
    render(<ZenConversation bridge={fb.bridge} row={r} options={{ compose: true, attachments: true, loadOlder: true, activityDrawer: true }} />)
    fb.pushThread(threadView())
    const drawer = el('[data-zen-compose] [data-zen-drawer]')
    expect(drawer.getAttribute('data-tone')).toBe('working')
    expect(document.querySelector('[data-zen-typing]')).toBeNull()
    expect(el('[data-zen-drawer-step]').textContent).toBe('Read src/main.ts')
    expect(Array.from(document.querySelectorAll('[data-zen-drawer-count]')).map((c) => c.getAttribute('data-zen-drawer-count'))).toEqual([
      'subagents',
      'read',
      'cmd',
    ])
    expect(el('[data-zen-drawer-clock]').textContent).toBe('0:08')
    act(() => void fireEvent.click(el('[data-zen-drawer-tile]')))
    expect(el('[data-zen-drawer-head]').textContent).toBe('Working · 8s since your message')
    expect(el('[data-zen-drawer-tally]').textContent).toBe('Read 2 files · ran 1 command')
    expect(localStorage.getItem('k2.zen.activityDrawerExpanded')).toBe('1')
  })

  it('off: no drawer, the typing dots stay', () => {
    const fb = fakeBridge([])
    const r = row('cortana', { activity: 'working', working: true, selected: true })
    render(<ZenConversation bridge={fb.bridge} row={r} options={{ compose: true, attachments: true, loadOlder: true }} />)
    fb.pushThread(threadView())
    expect(document.querySelector('[data-zen-drawer]')).toBeNull()
    expect(document.querySelector('[data-zen-typing]')).not.toBeNull()
  })

  it('a send from the Garden’s box records the agent for Recent agents first', async () => {
    const fb = fakeBridge([])
    const r = row('cortana', { selected: true })
    render(<ZenConversation bridge={fb.bridge} row={r} options={{ compose: true, attachments: true, loadOlder: true, activityDrawer: true }} />)
    fb.pushThread(threadView({ turn: null, liveTurn: null }))
    expect(useZenRecentFirstStore.getState().on).toBe(false)
    const input = el('[data-zen-compose-input]') as HTMLTextAreaElement
    act(() => void fireEvent.change(input, { target: { value: 'hello' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    expect(fb.posts).toEqual(['hello'])
    expect(Object.keys(useZenRecentFirstStore.getState().sent)).toEqual(['cortana::local'])
  })
})
