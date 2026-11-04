// @vitest-environment jsdom
//
// A board file's row in the Boards Garden opens its Board tab: out of Zen, onto the Agents page, in the file's own
// workspace. A window on another server, or a workspace this window doesn't list, opens nothing and says false.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  remote: false,
  known: new Set<string>(),
  opened: [] as string[],
  shownFor: [] as string[],
  exits: 0,
}))

vi.mock('@/stores/room', () => ({
  primaryRoom: () => ({
    isPrimary: true,
    scope: { isRemote: h.remote },
    tabs: { getState: () => ({ openFileAsTab: (p: string) => { h.opened.push(p); return 'opened' } }) },
  }),
  primaryProjectForRoot: (root: string) => (h.known.has(root) ? { projectId: 'p1', workspaceId: 'w1', name: 'alpha', showing: false, show: (f: () => void) => f() } : null),
}))
vi.mock('@/lib/zen/zen-view', () => ({ exitZen: () => { h.exits++ } }))
vi.mock('@/lib/thread-file-link-open', () => ({
  withThreadWorkspaceShowing: (_room: unknown, root: string, open: () => void) => {
    h.shownFor.push(root)
    open()
  },
}))

import { usePageViewStore } from '@/stores/page-view'
import { openBoardFileRowInK2 } from './board-file-list'

const ROW = { path: '/w/a/docs/Plan.k5board', rel: 'docs/Plan.k5board', name: 'Plan', workspace: '/w/a', workspaceName: 'alpha' }

beforeEach(() => {
  h.remote = false
  h.known = new Set(['/w/a'])
  h.opened = []
  h.shownFor = []
  h.exits = 0
  usePageViewStore.getState().setPage('home')
})

describe('openBoardFileRowInK2', () => {
  it('leaves Zen for the Agents page and opens the file’s tab in its workspace', () => {
    expect(openBoardFileRowInK2(ROW)).toBe(true)
    expect(h.exits).toBe(1)
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(h.shownFor).toEqual(['/w/a'])
    expect(h.opened).toEqual(['/w/a/docs/Plan.k5board'])
  })

  it('opens nothing for a workspace this window doesn’t list, or a window on another server', () => {
    h.known = new Set()
    expect(openBoardFileRowInK2(ROW)).toBe(false)
    h.known = new Set(['/w/a'])
    h.remote = true
    expect(openBoardFileRowInK2(ROW)).toBe(false)
    expect(h.exits).toBe(0)
    expect(h.opened).toEqual([])
    expect(usePageViewStore.getState().page).toBe('home')
  })
})
