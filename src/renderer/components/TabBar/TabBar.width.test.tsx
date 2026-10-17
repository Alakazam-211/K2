// @vitest-environment jsdom
//
// Tab width (Rosson 2026-10-16, 0.46.0): a tab is as wide as its content
// (padding + icon + title + working slot + status slot), capped at
// TAB_MAX_PX where the title truncates, never below TAB_MIN_PX. A short
// title gives a narrow tab; the working / status slot changing never moves
// the width (the title grid's hidden sizer). The pinned Chat keeps its own
// fixed size.
//
// jsdom has no layout, so `intrinsicWidth` models the browser's max-content
// sizing over the REAL rendered DOM: every child of the tab must be one of
// the known pieces (anything else throws, e.g. a hover-only control that
// would take room), text is CHAR_PX per character, and the result is clamped
// by the tab's own min / max width styles.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import { TabBar } from '@/components/TabBar/TabBar'
import { TAB_MAX_PX, TAB_MIN_PX } from '@/components/TabBar/TabStatus'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'
import type { ActivityDisplay } from '@/stores/session-events'

const CHAR_PX = 7
/** px-3 on both sides. */
const PADDING_PX = 24
/** The icon span: a 14px icon + mr-1.5. */
const ICON_PX = 14 + 6
/** var(--clu-gap) in the model. */
const GAP_PX = 4

function row(sessionId: string, display: ActivityDisplay, extra: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId,
    agentName: `tab-${sessionId}`,
    projectId: 'p1',
    workspacePath: '/ws',
    harness: 'claude',
    display,
    lead: { state: display === 'working' ? 'working' : display === 'waiting' ? 'waiting' : 'idle', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: null,
    evidenceSource: null,
    reason: 'idle',
    staleSince: null,
    confirmed: true,
    rev: 1,
    ...extra,
  }
}

const workingLead = { lead: { state: 'working' as const, outcome: 'none' as const, since: 0, promptId: null } }

function termTab(id: string, title: string, extra?: Partial<Tab>): Tab {
  const item: Item = {
    id: `${id}-item`,
    type: 'terminal',
    data: { terminalId: `t-${id}`, cwd: '/ws', command: 'claude', sessionId: `s-${id}` },
  } as Item
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), ...extra }
}

function fileTab(id: string, title: string): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath: `/ws/${title}` }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

function chatTab(): Tab {
  const item: Item = { id: 'chat-item', type: 'agent', data: { agentName: 'manager', projectPath: '/ws', section: 'chat' } } as Item
  const pg: PaneGroup = { id: 'chat-pg', items: [item], activeItemIndex: 0 }
  return { id: 'chat', title: 'Chat', mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), isSystemAgent: true }
}

function setTabs(tabs: Tab[]): void {
  useTabsStore.setState({ tabs, activeTabId: tabs[0]?.id ?? null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
}

function seed(rows: ActivityRow[]): void {
  act(() => {
    __seedActivityForTests(primaryScope(), { rows, supported: true, unseenDone: new Map() })
  })
}

function tabEl(id: string): HTMLElement {
  const el = document.querySelector(`[data-tab-id="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`tab ${id} did not render`)
  return el
}

function px(value: string, what: string): number {
  if (value === 'var(--clu-gap)') return GAP_PX
  const n = Number.parseFloat(value)
  if (!Number.isFinite(n)) throw new Error(`${what}: not a px value: "${value}"`)
  return n
}

function childWidth(child: HTMLElement): number {
  // Absolutely positioned decorations (drop lines, sandbox bar) take no room.
  if (child.classList.contains('absolute')) return 0
  if (child.matches('span.mr-1\\.5')) return ICON_PX
  if (child.matches('[data-tab-title-grid]')) {
    const cols = child.style.gridTemplateColumns
    const m = /^minmax\(0, max-content\) (\d+)px$/.exec(cols)
    if (!m) throw new Error(`title grid columns changed: "${cols}"`)
    const sizer = child.firstElementChild
    if (!(sizer instanceof HTMLElement) || sizer.getAttribute('aria-hidden') !== 'true') throw new Error('title grid lost its hidden sizer')
    // Column 1 = the sizer's text; then the gap and the working slot column.
    return (sizer.textContent ?? '').length * CHAR_PX + GAP_PX + Number(m[1])
  }
  if (child.matches('span.truncate')) return (child.textContent ?? '').length * CHAR_PX
  if (child.matches('[data-tab-close-slot], [data-tab-status-slot]')) {
    return px(child.style.width, 'slot width') + px(child.style.marginLeft, 'slot margin')
  }
  throw new Error(`unexpected tab child takes room: <${child.tagName.toLowerCase()} class="${child.className}">`)
}

/** The tab's width as the browser sizes it: max-content of its children,
 *  clamped by its min / max width. Fails if the tab has a fixed width. */
function intrinsicWidth(id: string): number {
  const tab = tabEl(id)
  if (tab.style.width !== '') throw new Error(`tab ${id} has a fixed width ${tab.style.width}`)
  if (/(^|\s)w-/.test(tab.className)) throw new Error(`tab ${id} has a fixed width class: ${tab.className}`)
  if (!tab.className.split(' ').includes('flex-shrink-0')) throw new Error(`tab ${id} can be squeezed below its content`)
  let content = PADDING_PX
  for (const child of Array.from(tab.children)) content += childWidth(child as HTMLElement)
  const min = px(tab.style.minWidth, 'minWidth')
  const max = px(tab.style.maxWidth, 'maxWidth')
  return Math.min(max, Math.max(min, content))
}

function render(): void {
  renderInPrimaryRoom(<TabBar cwd="/ws" />)
}

beforeEach(() => {
  localStorage.clear()
  __resetActivityForTests()
  useProjectsStore.setState({ projects: [{ id: 'p1', path: '/ws', name: 'ws' }] as never })
  setTabs([])
})

afterEach(() => {
  cleanup()
  __resetActivityForTests()
  setTabs([])
})

describe('tab width follows the title', () => {
  it('every regular tab carries the same default (max) width and floor', () => {
    setTabs([termTab('a', 'zsh'), fileTab('f', 'a.md')])
    render()
    for (const id of ['a', 'f']) {
      expect([id, tabEl(id).style.minWidth, tabEl(id).style.maxWidth]).toEqual([id, `${TAB_MIN_PX}px`, `${TAB_MAX_PX}px`])
    }
    expect(TAB_MIN_PX).toBe(56)
    expect(TAB_MAX_PX).toBe(200)
  })

  it('a short-title tab is narrower than a long-title one', () => {
    setTabs([termTab('s', 'zsh'), termTab('l', 'Investigate websocket'), fileTab('fs', 'a.md'), fileTab('fl', 'deployment-notes.md')])
    seed([row('s-s', 'idle'), row('s-l', 'idle')])
    render()
    const short = intrinsicWidth('s')
    const long = intrinsicWidth('l')
    expect(short).toBeLessThan(long)
    expect(short).toBeLessThan(TAB_MAX_PX)
    expect(intrinsicWidth('fs')).toBeLessThan(intrinsicWidth('fl'))
    // Short tabs are not padded up to the old shared 100px floor.
    expect(intrinsicWidth('fs')).toBeLessThan(100)
  })

  it('a long title caps at the default width and truncates with an ellipsis', () => {
    const title = 'Investigate the websocket reconnect flake on the relay pool'
    setTabs([termTab('a', title), fileTab('f', `${title}.md`)])
    seed([row('s-a', 'idle')])
    render()
    expect(intrinsicWidth('a')).toBe(TAB_MAX_PX)
    expect(intrinsicWidth('f')).toBe(TAB_MAX_PX)
    const shown = tabEl('a').querySelector('[data-tab-title]') as HTMLElement
    expect(shown.className.split(' ')).toEqual(expect.arrayContaining(['truncate', 'min-w-0']))
    expect(shown.textContent).toBe(title)
    expect(tabEl('f').querySelector('span.truncate')?.textContent).toBe(`${title}.md`)
  })

  it('the width does not jump when working or the status toggles', () => {
    setTabs([termTab('a', 'Refactor auth')])
    seed([row('s-a', 'idle')])
    render()
    const grid = (): HTMLElement => tabEl('a').querySelector('[data-tab-title-grid]') as HTMLElement
    const widths: number[] = []
    const slots: string[] = []
    const record = (): void => {
      widths.push(intrinsicWidth('a'))
      slots.push(grid().getAttribute('data-working-slot') ?? '?')
    }
    record() // idle: the title runs into the free working slot
    seed([row('s-a', 'working')])
    record() // working, no status: grid in the status slot
    seed([row('s-a', 'waiting', workingLead)])
    record() // working + a status: grid takes its own slot
    seed([row('s-a', 'idle')])
    record()
    // The toggles really happened...
    expect(slots).toEqual(['free', 'free', 'used', 'free'])
    // ...and the width never moved.
    expect(new Set(widths).size).toBe(1)
    expect(widths[0]).toBeLessThan(TAB_MAX_PX)
  })

  it('a pinned tab sizes the same way (its static pin rests in the status slot)', () => {
    setTabs([termTab('p', 'zsh', { isPinnedFile: true }), termTab('a', 'zsh')])
    seed([row('s-p', 'idle'), row('s-a', 'idle')])
    render()
    expect(intrinsicWidth('p')).toBe(intrinsicWidth('a'))
  })

  it('the pinned Chat keeps its own fixed size', () => {
    setTabs([chatTab(), termTab('a', 'zsh')])
    render()
    const chat = tabEl('chat')
    expect(chat.className.split(' ')).toEqual(expect.arrayContaining(['w-9', 'flex-shrink-0']))
    expect(chat.style.maxWidth).toBe('')
  })
})
