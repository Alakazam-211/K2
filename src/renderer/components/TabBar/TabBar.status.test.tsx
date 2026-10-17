// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 S5 (mock-up "Tab strip", AFTER): session tabs
// draw the shared status model. Status slot = the ranked status, else
// working (no status, Q7 = B), else live / reaped; the working grid gets
// its own slot only next to a status, and the title takes that slot when
// it is free. Hovering the tab swaps only the status slot for ×. The
// pinned Chat swaps its icon for the grid while working and wears its
// status as a corner badge (no underline). Done and failed are never the
// needs-you orange.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup } from '@testing-library/react'

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
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow, type UnseenDone } from '@/stores/activity'
import type { ActivityDisplay } from '@/stores/session-events'

function row(sessionId: string, agentName: string, display: ActivityDisplay, extra: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId,
    agentName,
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

function termTab(id: string, title: string): Tab {
  const item: Item = {
    id: `${id}-item`,
    type: 'terminal',
    data: { terminalId: `t-${id}`, cwd: '/ws', command: 'claude', sessionId: `s-${id}` },
  } as Item
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

function fileTab(id: string): Tab {
  const item: Item = { id: `${id}-item`, type: 'file-viewer', data: { filePath: `/ws/${id}.md` }, pinned: false }
  const pg: PaneGroup = { id: `${id}-pg`, items: [item], activeItemIndex: 0 }
  return { id, title: `${id}.md`, mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
}

function chatTab(): Tab {
  const item: Item = { id: 'chat-item', type: 'agent', data: { agentName: 'manager', projectPath: '/ws', section: 'chat' } } as Item
  const pg: PaneGroup = { id: 'chat-pg', items: [item], activeItemIndex: 0 }
  return { id: 'chat', title: 'Chat', mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]), isSystemAgent: true }
}

function setTabs(tabs: Tab[]): void {
  useTabsStore.setState({ tabs, activeTabId: tabs[0]?.id ?? null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
}

function seed(rows: ActivityRow[], unseen: Array<[string, UnseenDone]> = []): void {
  __seedActivityForTests(primaryScope(), { rows, supported: true, unseenDone: new Map(unseen) })
}

function unseenFor(sid: string, agentName: string, outcome: 'success' | 'failure'): [string, UnseenDone] {
  return [sid, { at: 1, agentName, projectId: 'p1', workspacePath: '/ws', outcome }]
}

function tabEl(id: string): HTMLElement {
  const el = document.querySelector(`[data-tab-id="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`tab ${id} did not render`)
  return el
}

/** What the tab shows at rest: [status slot's mark, working-slot use]. */
function restOf(id: string): { slot: string; workingSlot: string | null; title: string | null } {
  const tab = tabEl(id)
  const rest = tab.querySelector('[data-tab-slot-rest]')
  let slot = 'close-only'
  if (rest) {
    const status = rest.querySelector('[data-slot="status"]')
    const working = rest.querySelector('[data-slot="working"][data-working]')
    slot = status ? `status:${status.getAttribute('data-agent-status')}` : working ? 'working' : '?'
  }
  const grid = tab.querySelector('[data-tab-title-grid]')
  const title = tab.querySelector('[data-tab-title]')
  return {
    slot,
    workingSlot: grid?.getAttribute('data-working-slot') ?? null,
    title: title instanceof HTMLElement ? title.style.gridColumn : null,
  }
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

describe('session tabs: one status model (S5)', () => {
  it('working, no status: the grid sits in the status slot; the title takes the free working slot', () => {
    setTabs([termTab('a', 'Refactor auth')])
    seed([row('s-a', 'tab-t-a', 'working')])
    render()
    expect(restOf('a')).toEqual({ slot: 'working', workingSlot: 'free', title: '1 / span 2' })
  })

  it('working + needs you: the grid takes its own slot, the status slot shows needs you', () => {
    setTabs([termTab('a', 'Fix flaky test')])
    seed([row('s-a', 'tab-t-a', 'waiting', { children: { subagents: 1, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 } })])
    render()
    expect(restOf('a')).toEqual({ slot: 'status:needs-you', workingSlot: 'used', title: '1' })
    const grid = tabEl('a').querySelector('[data-tab-title-grid] [data-slot="working"] [data-agent-working]')
    expect(grid).toBeTruthy()
  })

  it('needs you alone (lead waiting, nothing running): no working grid', () => {
    setTabs([termTab('a', 'Release notes')])
    seed([row('s-a', 'tab-t-a', 'waiting')])
    render()
    expect(restOf('a')).toEqual({ slot: 'status:needs-you', workingSlot: 'free', title: '1 / span 2' })
  })

  it('done and failed (not viewed in this window) are ✓ and ×, ranked failed > done', () => {
    setTabs([termTab('a', 'Docs pass'), termTab('b', 'Migrate 0162')])
    seed([row('s-a', 'tab-t-a', 'idle'), row('s-b', 'tab-t-b', 'idle')], [
      unseenFor('s-a', 'tab-t-a', 'success'),
      unseenFor('s-b', 'tab-t-b', 'failure'),
    ])
    render()
    expect(restOf('a').slot).toBe('status:done')
    expect(restOf('b').slot).toBe('status:failed')
    for (const id of ['a', 'b']) {
      const html = tabEl(id).innerHTML
      expect([id, html.includes('status-warn-amber'), html.includes('status-needs-you')]).toEqual([id, false, false])
    }
  })

  it('monitoring and no update show their marks', () => {
    setTabs([termTab('a', 'k2/calendars'), termTab('b', 'quiet')])
    seed([row('s-a', 'tab-t-a', 'monitoring'), row('s-b', 'tab-t-b', 'unverifiable', { staleSince: 1 })])
    render()
    expect(restOf('a').slot).toBe('status:monitoring')
    expect(restOf('b').slot).toBe('status:no-update')
  })

  it('no status: live (a session row) is the green square, no row is reaped (grey)', () => {
    setTabs([termTab('a', 'zsh'), termTab('b', 'Old review')])
    seed([row('s-a', 'tab-t-a', 'idle')])
    render()
    expect(restOf('a').slot).toBe('status:live')
    expect(restOf('b').slot).toBe('status:reaped')
    expect(tabEl('a').querySelector('[data-live-square="live"]')).toBeTruthy()
    expect(tabEl('b').querySelector('[data-live-square="reaped"]')).toBeTruthy()
  })

  it('a row whose PTY exited is reaped, not live', () => {
    setTabs([termTab('a', 'gone')])
    seed([row('s-a', 'tab-t-a', 'idle', { reason: 'pty_exited' })])
    render()
    expect(restOf('a').slot).toBe('status:reaped')
  })

  it('hovering the tab swaps only the status slot for ×: the rest hides on group hover, × shows', () => {
    setTabs([termTab('a', 'Deploy')])
    seed([row('s-a', 'tab-t-a', 'waiting', { lead: { state: 'working', outcome: 'none', since: 0, promptId: null } })])
    render()
    const slot = tabEl('a').querySelector('[data-tab-close-slot="close"]')
    if (!(slot instanceof HTMLElement)) throw new Error('no status/close slot')
    expect(slot.querySelector('[data-tab-slot-rest]')?.className).toContain('group-hover:hidden')
    const hover = slot.querySelector('[data-tab-slot-hover]')
    expect(hover?.className).toContain('group-hover:flex')
    expect(hover?.querySelector('svg line')).toBeTruthy()
    // The tab itself is the hover group (not the 14px slot).
    expect(tabEl('a').className.split(' ')).toContain('group')
    // The working grid is outside the slot, so hovering never hides it.
    expect(slot.querySelector('[data-agent-working]')).toBeNull()
  })

  it('the slot is 14px with the cluster gap, and the tab sizes as title + working slot either way', () => {
    setTabs([termTab('a', 'Investigate websocket reconnect flake'), termTab('b', 'Investigate websocket reconnect flake')])
    seed([row('s-a', 'tab-t-a', 'idle'), row('s-b', 'tab-t-b', 'waiting', { lead: { state: 'working', outcome: 'none', since: 0, promptId: null } })])
    render()
    for (const id of ['a', 'b']) {
      const slot = tabEl(id).querySelector('[data-tab-close-slot]') as HTMLElement
      expect([id, slot.style.width, slot.style.height, slot.style.marginLeft]).toEqual([id, '14px', '14px', 'var(--clu-gap)'])
      const grid = tabEl(id).querySelector('[data-tab-title-grid]') as HTMLElement
      expect([id, grid.style.gridTemplateColumns]).toEqual([id, 'minmax(0, max-content) 14px'])
      // The hidden sizer holds column 1 at the title's width.
      const sizer = grid.firstElementChild as HTMLElement
      expect([id, sizer.getAttribute('aria-hidden'), sizer.textContent]).toEqual([id, 'true', 'Investigate websocket reconnect flake'])
    }
    expect(restOf('a').workingSlot).toBe('free')
    expect(restOf('b').workingSlot).toBe('used')
  })

  it('session tabs keep the working grid at 14px (only the pinned Chat draws it larger)', () => {
    setTabs([termTab('a', 'Deploy'), termTab('b', 'Build')])
    seed([
      row('s-a', 'tab-t-a', 'working'),
      row('s-b', 'tab-t-b', 'waiting', { lead: { state: 'working', outcome: 'none', since: 0, promptId: null } }),
    ])
    render()
    for (const id of ['a', 'b']) {
      const slot = tabEl(id).querySelector('[data-slot="working"][data-working]') as HTMLElement
      const grid = slot.querySelector('svg[data-agent-working]') as SVGElement
      expect([id, slot.style.width, slot.style.height, grid.getAttribute('width'), grid.getAttribute('height')]).toEqual([
        id,
        '14px',
        '14px',
        '14',
        '14',
      ])
      expect([id, grid.querySelectorAll('rect').length]).toEqual([id, 9])
    }
  })

  it('file tabs keep only the close slot', () => {
    setTabs([fileTab('f')])
    render()
    expect(tabEl('f').querySelector('[data-tab-title-grid]')).toBeNull()
    expect(tabEl('f').querySelector('[data-tab-slot-rest]')).toBeNull()
    expect(tabEl('f').querySelector('[data-tab-close-slot="close"] svg line')).toBeTruthy()
  })

  it('no braille spinner and no ActivityMark squares on any tab', () => {
    setTabs([chatTab(), termTab('a', 'w'), termTab('b', 'n')])
    seed([row('s-a', 'tab-t-a', 'working'), row('s-b', 'tab-t-b', 'waiting'), row('chat-s', 'p1', 'working')])
    render()
    expect(document.querySelector('.braille-spinner')).toBeNull()
    expect(document.querySelector('[data-activity]')).toBeNull()
  })
})

describe('the pinned Chat tab', () => {
  it('working: the grid replaces the icon; the status (needs you) is a 10px corner badge; no underline', () => {
    setTabs([chatTab()])
    seed([row('chat-s', 'p1', 'waiting', { lead: { state: 'working', outcome: 'none', since: 0, promptId: null } })])
    render()
    const tab = tabEl('chat')
    expect(tab.getAttribute('data-tab-status')).toBe('needs-you')
    expect(tab.getAttribute('data-tab-working')).toBe('')
    expect(tab.querySelector(':scope > [data-slot="working"] [data-agent-working]')).toBeTruthy()
    expect(tab.querySelector(':scope > svg')).toBeNull()
    // Drawn at the icon's size (20px box ≈ the 16px icon's 12px ink), not the 14px slot.
    const slot = tab.querySelector(':scope > [data-slot="working"]') as HTMLElement
    const grid = slot.querySelector('svg[data-agent-working]') as SVGElement
    expect([slot.style.width, slot.style.height, grid.getAttribute('width'), grid.getAttribute('height')]).toEqual([
      '20px',
      '20px',
      '20',
      '20',
    ])
    // The full 3×3 here (only narrow spots use the 2×3).
    expect(grid.querySelectorAll('rect')).toHaveLength(9)
    const badge = tab.querySelector('[data-pinned-chat-badge] [data-slot="status"]') as HTMLElement
    expect([badge.getAttribute('data-agent-status'), badge.style.width]).toEqual(['needs-you', '10px'])
    expect(tab.querySelector('.rounded-full')).toBeNull()
  })

  it('idle with a live session: its icon, and the live square as the badge', () => {
    setTabs([chatTab()])
    seed([row('chat-s', 'p1', 'idle')])
    render()
    const tab = tabEl('chat')
    expect(tab.querySelector(':scope > svg')).toBeTruthy()
    expect(tab.querySelector('[data-pinned-chat-badge] [data-live-square="live"]')).toBeTruthy()
  })

  it('done, not viewed: ✓ badge, not an amber square', () => {
    setTabs([chatTab()])
    seed([row('chat-s', 'p1', 'idle')], [unseenFor('chat-s', 'p1', 'success')])
    render()
    expect(tabEl('chat').getAttribute('data-tab-status')).toBe('done')
    expect(tabEl('chat').innerHTML).not.toContain('status-warn-amber')
  })
})
