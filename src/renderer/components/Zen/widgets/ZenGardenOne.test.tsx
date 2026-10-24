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

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import type { ThreadTurn } from '@/components/SessionView/overlayThread'
import { COUNTER_SPINNER_EM, COUNTER_SPINNER_INK_EM, COUNTER_SPINNER_SVG_EM } from '@/components/SessionView/threadStrip'
import { ZEN_RECENT_FIRST_KEY, useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversation } from './ZenConversationWidget'
import { ZenCompose, __resetZenDraftsForTests, zenComposeRowChrome } from './ZenCompose'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

// This computer's daemon, for the Garden settings' Theme row: the theme
// list (`themeList: null` = an older daemon without it) and theme set.
const h = vi.hoisted(() => ({
  gets: [] as { route: string; params: Record<string, unknown> }[],
  posts: [] as { route: string; body: unknown }[],
  themeList: null as unknown,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    // 0.46.4: the Agents widget reads the person's order and pins on mount
    // (none set here); this file's GET checks are about the theme list.
    if (route === 'zen/agents/order') return { ok: true, gardens: [] }
    h.gets.push({ route, params: params ?? {} })
    if (route !== 'zen/theme/list') throw new Error(`unexpected GET ${route}`)
    if (h.themeList === null) throw new Error('unknown zen route')
    return h.themeList
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
    h.posts.push({ route, body })
    if (route !== 'zen/theme/set') throw new Error(`unexpected POST ${route}`)
    return { ok: true }
  }),
}))

const THEME_LIST = {
  ok: true,
  active: 'glass',
  scope: 'garden',
  global: 'basic',
  garden: 'g-1',
  gardenTheme: null,
  themes: [
    { name: 'basic', label: 'Basic', builtin: true, user: false, active: false },
    { name: 'glass', label: 'Glass', builtin: true, user: false, active: true },
    { name: 'paper', label: 'Paper', builtin: true, user: false, active: false },
  ],
}

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
  h.gets.length = 0
  h.posts.length = 0
  h.themeList = THEME_LIST
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
  it('the cog opens the Garden’s own settings; the switch is off, and on reorders the list below the pins', async () => {
    useZenRecentFirstStore.getState().markSent('gamma::local', 200)
    useZenRecentFirstStore.getState().markSent('beta::local', 100)
    useZenRecentFirstStore.getState().markSent('pinA::local', 300)
    const fb = fakeBridge(ROWS())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ pins: true, settings: true })} />)
    // Off by default: the Home's order.
    expect(listRows()).toEqual(['alpha::local', 'beta::local', 'gamma::local'])
    // The cog is K2's one Settings icon, style A3 (Lucide `settings`, round caps).
    const cog = el('[data-zen-agents-settings] svg')
    expect(cog.getAttribute('data-k2-icon')).toBe('settings')
    expect(cog.getAttribute('width')).toBe('16')
    expect(cog.getAttribute('stroke-width')).toBe('1.75')
    expect(cog.getAttribute('stroke-linecap')).toBe('round')
    await act(async () => void fireEvent.click(el('[data-zen-agents-settings]')))
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

// Eleven pinned agents among three unpinned ones.
const ROWS11 = (selected?: string): ZenAgentRow[] => {
  const out: ZenAgentRow[] = [row('alpha')]
  for (let i = 0; i <= 10; i++) {
    out.push(row(`p${i}`, { pinned: true, selected: `p${i}` === selected }))
    if (i === 4) out.push(row('beta'))
  }
  out.push(row('gamma'))
  return out
}
const nine = Array.from({ length: 9 }, (_, i) => `p${i}::local`)

describe('Garden 1 from the 0.46.0 daemon (`pin-limit` 9, `recent-first` true)', () => {
  const garden1 = { pins: true, settings: true, 'pin-limit': 9, 'recent-first': true }

  it('nine pins in the grid, the rest head the list; recent first is on by default', () => {
    useZenRecentFirstStore.getState().markSent('gamma::local', 200)
    useZenRecentFirstStore.getState().markSent('beta::local', 100)
    const fb = fakeBridge(ROWS11())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl(garden1)} />)
    expect(pinCells()).toEqual(nine)
    expect(el('[data-zen-pins]').getAttribute('data-zen-pin-count')).toBe('9')
    expect(listRows()).toEqual(['p9::local', 'p10::local', 'gamma::local', 'beta::local', 'alpha::local'])
    expect(el('[role="listbox"][aria-label="Agents"]').getAttribute('data-zen-agents-order')).toBe('recent-first')
  })

  it('a stored choice wins over the Garden’s default', () => {
    localStorage.setItem(ZEN_RECENT_FIRST_KEY, '0')
    useZenRecentFirstStore.getState().reload()
    useZenRecentFirstStore.getState().markSent('gamma::local', 200)
    const fb = fakeBridge(ROWS11())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl(garden1)} />)
    // Off: overflow pins still head the list, then the Home's order.
    expect(listRows()).toEqual(['p9::local', 'p10::local', 'alpha::local', 'beta::local', 'gamma::local'])
  })

  it('⌘↓ from the last grid pin goes to the first overflow pin in the list', async () => {
    const fb = fakeBridge(ROWS11('p8'))
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl(garden1)} />)
    await act(async () => void fireEvent.keyDown(window, { key: 'ArrowDown', metaKey: true }))
    expect(fb.opened).toEqual(['p9::local'])
  })

  it('settings: Appearance, Pinned agents and Lists; the switch shows the Garden’s default; a theme pick is for this Garden', async () => {
    const fb = fakeBridge(ROWS11())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl(garden1)} />)
    await act(async () => void fireEvent.click(el('[data-zen-agents-settings]')))
    const dialog = el('[data-testid="zen-garden-settings"]')
    expect(dialog.querySelector('[data-zen-garden-settings-appearance]')).not.toBeNull()
    expect(dialog.querySelector('[data-zen-garden-settings-pins]')).not.toBeNull()
    expect(dialog.querySelector('[data-zen-garden-settings-lists]')).not.toBeNull()
    expect(Array.from(dialog.querySelectorAll('section > span:first-child')).map((s) => s.textContent)).toEqual([
      'Appearance',
      'Pinned agents',
      'Lists',
    ])

    // Theme: this Garden's list, its active theme selected.
    expect(h.gets).toEqual([{ route: 'zen/theme/list', params: { garden: 'g-1' } }])
    const select = el('[data-zen-garden-theme-select]') as HTMLSelectElement
    expect(select.getAttribute('data-zen-garden-theme-select')).toBe('ready')
    expect(select.disabled).toBe(false)
    expect(select.value).toBe('glass')
    expect(Array.from(select.options).map((o) => o.textContent)).toEqual(['Basic', 'Glass', 'Paper'])
    await act(async () => void fireEvent.change(select, { target: { value: 'paper' } }))
    expect(h.posts).toEqual([{ route: 'zen/theme/set', body: { name: 'paper', garden: 'g-1' } }])
    expect((el('[data-zen-garden-theme-select]') as HTMLSelectElement).value).toBe('paper')
    expect(document.querySelector('[data-zen-garden-theme-error]')).toBeNull()

    // Pinned agents: fixed, read-only.
    expect(el('[data-zen-pin-grid-text]').textContent).toBe('3 × 3 (9)')
    expect(dialog.querySelector('[data-zen-garden-settings-pins] button, [data-zen-garden-settings-pins] input')).toBeNull()

    // Lists: on from the Garden until the person picks; their pick is stored.
    expect(el('[data-zen-recent-first-switch]').getAttribute('aria-checked')).toBe('true')
    expect(el('[data-zen-recent-first-note]').getAttribute('data-zen-recent-first-source')).toBe('garden')
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBeNull()
    act(() => void fireEvent.click(el('[data-zen-recent-first-switch]')))
    expect(el('[data-zen-recent-first-switch]').getAttribute('aria-checked')).toBe('false')
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBe('0')
    expect(el('[data-zen-recent-first-note]').getAttribute('data-zen-recent-first-source')).toBe('stored')
    act(() => void fireEvent.click(el('[data-zen-recent-first-switch]')))
    expect(localStorage.getItem(ZEN_RECENT_FIRST_KEY)).toBe('1')
  })
})

describe('Garden 1 on an older daemon (no `pin-limit`, `recent-first`, or theme list)', () => {
  it('every pin in the grid, recent first off, the settings’ Theme row disabled with a note', async () => {
    h.themeList = null
    useZenRecentFirstStore.getState().markSent('gamma::local', 200)
    const fb = fakeBridge(ROWS11())
    render(<ZenAgentsWidget bridge={fb.bridge} decl={agentsDecl({ pins: true, settings: true })} />)
    expect(pinCells()).toHaveLength(11)
    expect(listRows()).toEqual(['alpha::local', 'beta::local', 'gamma::local'])
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await act(async () => void fireEvent.click(el('[data-zen-agents-settings]')))
    expect(warn).toHaveBeenCalledTimes(1)
    warn.mockRestore()
    const select = el('[data-zen-garden-theme-select]') as HTMLSelectElement
    expect(select.getAttribute('data-zen-garden-theme-select')).toBe('failed')
    expect(select.disabled).toBe(true)
    expect(el('[data-zen-garden-theme-note]').getAttribute('data-zen-garden-theme-note')).toBe('failed')
    expect(el('[data-zen-pin-grid-text]').textContent).toBe('3 across, no limit')
    expect(el('[data-zen-recent-first-switch]').getAttribute('aria-checked')).toBe('false')
    expect(el('[data-zen-recent-first-note]').getAttribute('data-zen-recent-first-source')).toBe('default')
    expect(h.posts).toEqual([])
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
    // Running subagents spin the app's narrow 2×3 cell grid in the working
    // colour (plan-agent-status-unify-v1; Rosson: narrow spots get 2×3),
    // not the braille.
    const spinner = el('[data-zen-drawer-count="subagents"] [data-zen-drawer-spinner]')
    expect(spinner.style.color).toBe('var(--zen-working)')
    const svg = spinner.querySelector('svg[data-agent-working]') as SVGElement
    expect([svg.getAttribute('data-cols'), svg.querySelectorAll('rect').length]).toEqual(['2', 6])
    // Sized in em of the count (follows the text size): the grid's ink
    // (14 of its 24 units tall, 9 wide) is 0.8em − 2px at 12px, centred on
    // the digits (no raise), with 2px between it and the count and 1px
    // more than the icon's usual gap between the agent icon and it.
    expect([svg.getAttribute('width'), svg.getAttribute('height')]).toEqual([
      `${Number(((COUNTER_SPINNER_SVG_EM * 19) / 24).toFixed(3))}em`,
      `${COUNTER_SPINNER_SVG_EM}em`,
    ])
    // The layout box is the ink's width (2 columns), not the old 0.8em.
    expect(COUNTER_SPINNER_EM).toBe(0.41)
    expect(Math.abs((COUNTER_SPINNER_SVG_EM * 14) / 24 - COUNTER_SPINNER_INK_EM)).toBeLessThan(0.001)
    expect(COUNTER_SPINNER_SVG_EM).toBe(1.085)
    expect(Math.abs(COUNTER_SPINNER_INK_EM * 12 - (0.8 * 12 - 2))).toBeLessThan(0.1)
    const centred = spinner.querySelector('[data-zen-drawer-spinner-grid]') as HTMLElement
    expect([centred.style.transform, spinner.style.verticalAlign]).toEqual(['translate(-50%, -50%)', '-0.14em'])
    expect([spinner.style.width, spinner.style.height, spinner.style.marginLeft, spinner.style.marginRight]).toEqual([
      `${COUNTER_SPINNER_EM}em`,
      '1em',
      '1px',
      '2px',
    ])
    // The agent icon before it keeps its 2px gap, so 3px in all.
    const agentIcon = spinner.parentElement?.previousElementSibling as SVGElement
    expect([agentIcon.tagName.toLowerCase(), agentIcon.style.marginRight]).toEqual(['svg', '2px'])
    expect(document.querySelector('[data-zen-drawer] .zen-braille, [data-zen-drawer] .braille-spinner')).toBeNull()
    expect(el('[data-zen-drawer-count="subagents"]').textContent).not.toMatch(/[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/)
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
    expect(useZenRecentFirstStore.getState().stored).toBeNull()
    const input = el('[data-zen-compose-input]') as HTMLTextAreaElement
    act(() => void fireEvent.change(input, { target: { value: 'hello' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    expect(fb.posts).toEqual(['hello'])
    expect(Object.keys(useZenRecentFirstStore.getState().sent)).toEqual(['cortana::local'])
  })
})

// 0.46.0 (Rosson 2026-10-16): the Garden's box stacks like the desktop
// Thread's past 3 lines (the shared rule in ThreadZenKit), back at 2.
describe('the stacked message box', () => {
  // 20px lines, 12px of padding: N lines = 12 + 20N. 300px wide.
  const LINE = 20
  let height = 32
  function renderBox(opts: { stack?: boolean } = {}): { ta: HTMLTextAreaElement; box: HTMLElement } {
    const real = window.getComputedStyle.bind(window)
    vi.spyOn(window, 'getComputedStyle').mockImplementation((elt: Element, pseudo?: string | null) =>
      elt instanceof HTMLTextAreaElement ? ({ lineHeight: `${LINE}px`, fontSize: '14px' } as CSSStyleDeclaration) : real(elt, pseudo),
    )
    const proto = HTMLTextAreaElement.prototype
    vi.spyOn(proto, 'scrollHeight', 'get').mockImplementation(() => height)
    vi.spyOn(proto, 'clientWidth', 'get').mockImplementation(() => 300)
    const fb = fakeBridge([])
    render(<ZenCompose bridge={fb.bridge} address="cortana::local" label="cortana" disabled={false} {...opts} />)
    return { ta: el('[data-zen-compose-input]') as HTMLTextAreaElement, box: el('[data-zen-compose-box]') }
  }
  const type = (ta: HTMLTextAreaElement, lines: number, text: string): void => {
    height = 12 + lines * LINE
    act(() => void fireEvent.change(ta, { target: { value: text } }))
  }

  beforeEach(() => {
    height = 32
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('stacks past 3 lines with the same textarea, focus and caret kept', () => {
    expect(zenComposeRowChrome(true)).toBe(80)
    expect(zenComposeRowChrome(false)).toBe(40)
    const { ta, box } = renderBox()
    ta.focus()
    type(ta, 1, 'a'.repeat(40))
    expect(box.hasAttribute('data-stacked')).toBe(false)
    type(ta, 3, 'a'.repeat(150))
    expect(box.hasAttribute('data-stacked')).toBe(false)
    type(ta, 5, 'a'.repeat(300))
    expect(box.getAttribute('data-stacked')).toBe('true')
    expect(el('[data-zen-compose-input]')).toBe(ta)
    expect(document.activeElement).toBe(ta)
    expect(ta.selectionStart).toBe(300)
    // The text first across the full width; "+" and Send under it, Send right.
    expect(ta.style.order).toBe('-1')
    expect(ta.style.flexBasis).toBe('100%')
    expect(box.style.flexWrap).toBe('wrap')
    expect(Array.from(box.children).map((c) => c.getAttribute('data-zen-attach') !== null ? 'attach' : c.hasAttribute('data-thread-compose-spacer') ? 'spacer' : c.hasAttribute('data-zen-send') ? 'send' : c.tagName.toLowerCase())).toEqual([
      'attach',
      'textarea',
      'spacer',
      'send',
    ])
  })

  it('unstacks at 2 lines or fewer, with no flapping at exactly 3', () => {
    const { ta, box } = renderBox()
    type(ta, 6, 'a'.repeat(400))
    expect(box.getAttribute('data-stacked')).toBe('true')
    // Exactly 3 lines while stacked: stays, and stays across more edits.
    for (let i = 0; i < 4; i++) {
      type(ta, 3, 'a'.repeat(200 + i))
      expect(box.getAttribute('data-stacked')).toBe('true')
    }
    // 2 lines stacked would be 3 in the one-row field (300 → 220px): stays.
    type(ta, 2, 'a'.repeat(130))
    expect(box.getAttribute('data-stacked')).toBe('true')
    // 1 line: back to one row, the same textarea.
    type(ta, 1, 'short')
    expect(box.hasAttribute('data-stacked')).toBe(false)
    expect(ta.style.order).toBe('')
    expect(document.querySelector('[data-thread-compose-spacer]')).toBeNull()
    expect(el('[data-zen-compose-input]')).toBe(ta)
    // And at exactly 3 from one row: stays one row.
    type(ta, 3, 'a'.repeat(150))
    expect(box.hasAttribute('data-stacked')).toBe(false)
  })

  it('reduced motion: the switch has no transition', () => {
    const mm = vi.fn((q: string) => ({ matches: q.includes('prefers-reduced-motion'), media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, onchange: null, dispatchEvent: () => false }))
    vi.stubGlobal('matchMedia', mm)
    const { ta, box } = renderBox()
    type(ta, 5, 'a'.repeat(300))
    expect(box.getAttribute('data-stacked')).toBe('true')
    expect(box.style.transition).toBe('')
    expect(ta.style.transition).toBe('')
    expect(el('[data-thread-compose-spacer]').style.transition).toBe('')
    vi.unstubAllGlobals()
  })

  it('`stack` off: never stacks', () => {
    const { ta, box } = renderBox({ stack: false })
    type(ta, 8, 'a'.repeat(600))
    expect(box.hasAttribute('data-stacked')).toBe(false)
    expect(ta.style.order).toBe('')
  })
})
