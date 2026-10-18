// @vitest-environment jsdom
//
// 0.46.1 polish: the Split view's small (24px) pane tabs wear the same
// status marks as the room tab strip (TabBar.status.test.tsx): the status
// in one 14px slot (working = the 3×3 grid with the status as a corner
// badge), × on hover. Both strips fold through `terminalsStatus` and draw
// `TabSlotRest`, so they cannot disagree.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

import { PaneTabBar } from '@/components/PaneLayout/PaneTabBar'
import { TabBar } from '@/components/TabBar/TabBar'
import { useTabsStore, type Item, type PaneGroup, type Tab } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow, type UnseenDone } from '@/stores/activity'
import type { ActivityDisplay } from '@/stores/session-events'

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

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

function seed(rows: ActivityRow[], unseen: Array<[string, UnseenDone]> = []): void {
  __seedActivityForTests(primaryScope(), { rows, supported: true, unseenDone: new Map(unseen) })
}

type PaneItem = Parameters<typeof PaneTabBar>[0]['items'][number]

function term(id: string): PaneItem {
  return { id, type: 'terminal', data: { terminalId: `t-${id}`, cwd: '/ws', command: 'claude', sessionId: `s-${id}` } as never }
}

function renderPane(items: PaneItem[], active = 0): void {
  renderInPrimaryRoom(<PaneTabBar items={items} activeItemIndex={active} onActivate={() => {}} onClose={() => {}} />)
}

function paneTab(id: string): HTMLElement {
  const el = document.querySelector(`[data-pane-tab-id="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`pane tab ${id} did not render`)
  return el
}

/** The pane tab's resting mark: slot kind and corner badge. */
function restOf(root: Element): { slot: string; badge: string | null } {
  const rest = root.querySelector('[data-tab-slot-rest]')
  if (!rest) return { slot: 'close-only', badge: null }
  const working = rest.querySelector(':scope > [data-tab-slot-working] > [data-slot="working"][data-working]')
  const status = rest.querySelector(':scope > [data-slot="status"]')
  const slot = working ? 'working' : status ? `status:${status.getAttribute('data-agent-status')}` : '?'
  const b = rest.querySelector('[data-tab-badge] [data-slot="status"]')
  return { slot, badge: b ? `status:${b.getAttribute('data-agent-status')}` : null }
}

beforeEach(() => {
  localStorage.clear()
  __resetActivityForTests()
  useProjectsStore.setState({ projects: [{ id: 'p1', path: '/ws', name: 'ws' }] as never })
})

afterEach(() => {
  cleanup()
  __resetActivityForTests()
  useTabsStore.setState({ tabs: [], activeTabId: null, splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
})

describe('Split view pane tabs: status marks', () => {
  it('working: the grid takes the slot and live is its corner badge', () => {
    seed([row('s-a', 'tab-t-a', 'working')])
    renderPane([term('a')])
    expect(restOf(paneTab('a'))).toEqual({ slot: 'working', badge: 'status:live' })
    expect(paneTab('a').getAttribute('data-pane-tab-status')).toBe('live')
  })

  it('waiting (needs you) alone: the needs-you mark in the slot, in its orange', () => {
    seed([row('s-a', 'tab-t-a', 'waiting')])
    renderPane([term('a')])
    expect(restOf(paneTab('a'))).toEqual({ slot: 'status:needs-you', badge: null })
    const mark = paneTab('a').querySelector('[data-tab-slot-rest] [data-slot="status"]') as HTMLElement
    expect(mark.style.color).toContain('status-needs-you')
  })

  it('working + needs you: grid in the slot, needs you as the badge', () => {
    seed([row('s-a', 'tab-t-a', 'waiting', { children: { subagents: 1, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 } })])
    renderPane([term('a')])
    expect(restOf(paneTab('a'))).toEqual({ slot: 'working', badge: 'status:needs-you' })
  })

  it('done and failed (not viewed) show ✓ and ×; each pane tab folds only its own session', () => {
    seed([row('s-a', 'tab-t-a', 'idle'), row('s-b', 'tab-t-b', 'idle'), row('s-c', 'tab-t-c', 'working')], [
      ['s-a', { at: 1, agentName: 'tab-t-a', projectId: 'p1', workspacePath: '/ws', outcome: 'success' }],
      ['s-b', { at: 1, agentName: 'tab-t-b', projectId: 'p1', workspacePath: '/ws', outcome: 'failure' }],
    ])
    renderPane([term('a'), term('b'), term('c')])
    expect(restOf(paneTab('a')).slot).toBe('status:done')
    expect(restOf(paneTab('b')).slot).toBe('status:failed')
    expect(restOf(paneTab('c')).slot).toBe('working')
  })

  it('non-terminal pane tabs (files, browser) keep the plain ×', () => {
    seed([])
    renderPane([
      { id: 'f', type: 'file-viewer', data: { filePath: '/ws/notes.md' } },
      { id: 'b', type: 'browser', data: { url: 'https://example.com' } },
    ])
    for (const id of ['f', 'b']) {
      expect(paneTab(id).querySelector('[data-tab-slot-rest]')).toBeNull()
      expect(paneTab(id).querySelector('[data-pane-tab-close-slot] svg line')).toBeTruthy()
      expect(paneTab(id).hasAttribute('data-pane-tab-status')).toBe(false)
    }
  })

  it('hover swaps the status slot for ×; the badge sits on the pane tab background', () => {
    seed([row('s-a', 'tab-t-a', 'working'), row('s-b', 'tab-t-b', 'working')])
    renderPane([term('a'), term('b')], 0)
    const slot = paneTab('a').querySelector('[data-pane-tab-close-slot]') as HTMLElement
    expect([slot.style.width, slot.style.height]).toEqual(['14px', '14px'])
    expect(slot.querySelector('[data-tab-slot-rest]')?.className).toContain('group-hover/panetab:hidden')
    const hover = slot.querySelector('[data-tab-slot-hover]')
    expect(hover?.className).toContain('group-hover/panetab:flex')
    expect(hover?.querySelector('svg line')).toBeTruthy()
    expect(paneTab('a').className.split(' ')).toContain('group/panetab')
    expect(paneTab('a').style.getPropertyValue('--tab-bg')).toBe('var(--color-bg-inset)')
    expect(paneTab('b').style.getPropertyValue('--tab-bg')).toBe('var(--color-bg-stripe)')
  })

  it('the same session draws the same mark in the room strip and the pane strip', () => {
    seed([row('s-a', 'tab-t-a', 'waiting', { children: { subagents: 1, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 } })])
    const item = term('a') as unknown as Item
    const pg: PaneGroup = { id: 'a-pg', items: [item], activeItemIndex: 0 }
    const tab: Tab = { id: 'a', title: 'Fix', mosaicTree: pg.id, paneGroups: new Map([[pg.id, pg]]) }
    useTabsStore.setState({ tabs: [tab], activeTabId: 'a', splitCount: 1, extraGroups: [], activeGroupIndex: 0 })
    renderInPrimaryRoom(<TabBar cwd="/ws" />)
    const roomTab = document.querySelector('[data-tab-id="a"]') as HTMLElement
    const roomMark = restOf(roomTab)
    cleanup()
    renderPane([term('a')])
    expect(restOf(paneTab('a'))).toEqual(roomMark)
    expect(roomMark).toEqual({ slot: 'working', badge: 'status:needs-you' })
  })
})
