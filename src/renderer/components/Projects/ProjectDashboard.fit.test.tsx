// @vitest-environment jsdom
// A dashboard in a narrow canvas (the project chat open in a small
// window): its tiles reflow so none is under MIN_TILE_W_PX, for display
// only. The saved layout is never written, and with room again it is drawn
// as saved.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'

const h = vi.hoisted(() => ({
  save: vi.fn(async () => ({ revision: 2 })),
  /** The default each tile's view was resolved against, by agent. */
  views: {} as Record<string, string | undefined>,
}))

vi.mock('./projects-api', async (orig) => ({
  ...(await orig<typeof import('./projects-api')>()),
  saveDashboardLayout: h.save,
}))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn(async () => ({})), daemonCliPost: vi.fn(async () => ({})) }))
vi.mock('./wake-member-session', () => ({
  resolveAllMemberSessions: vi.fn(async () => ({})),
  activateOnLiveSessionAttach: vi.fn(),
}))
vi.mock('./DashboardAgentTile', () => ({
  DashboardAgentTile: () => null,
  PHASE_CHECKING: { kind: 'checking' },
  PHASE_DORMANT: { kind: 'dormant' },
  TilePicture: () => null,
  TileViewMenu: () => null,
  TileViewPicker: () => null,
  useAgentTileAddr: () => '',
  useTileView: (_g: string, ws: string, dv?: string) => {
    h.views[ws] = dv
    return [dv ?? 'thread', () => {}]
  },
}))
vi.mock('./PaneChrome', () => ({
  PaneChrome: ({ title, shortcutNum }: { title: string; shortcutNum?: number }) => (
    <div data-testid="tile" data-shortcut={shortcutNum}>{title}</div>
  ),
}))
vi.mock('./TileBarTools', () => ({ TileBarTools: () => null, tileToolsPlace: () => null }))

import ProjectDashboard from './ProjectDashboard'
import { LAYOUT_SAVE_COALESCE_MS, MIN_TILE_W_PX } from './dashboard-layout'
import type { ProjectGroupDashboard, ProjectGroupShow } from './projects-api'

// The canvas's size, as the ResizeObserver would report it.
let canvasW = 0
let canvasH = 700
const observers: Array<() => void> = []
class FakeResizeObserver {
  constructor(private cb: () => void) {
    observers.push(() => this.cb())
  }
  observe(): void {}
  disconnect(): void {}
}
const resize = (w: number): void => {
  canvasW = w
  act(() => observers.forEach((fire) => fire()))
}

beforeEach(() => {
  observers.length = 0
  h.save.mockClear()
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
    return this.hasAttribute('data-dashboard-canvas') ? canvasW : 0
  })
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
    return this.hasAttribute('data-dashboard-canvas') ? canvasH : 0
  })
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

const term = (ws: string) => ({ type: 'pane', pane: { kind: 'terminal', workspaceId: ws } })
const col = (a: string, b: string) => ({ type: 'split', dir: 'col', children: [{ size: 50, node: term(a) }, { size: 50, node: term(b) }] })
// Three columns of two agents.
const layoutJson = JSON.stringify({
  version: 2,
  root: { type: 'split', dir: 'row', children: [{ size: 100 / 3, node: col('a', 'b') }, { size: 100 / 3, node: col('c', 'd') }, { size: 100 / 3, node: col('e', 'f') }] },
})
const member = (ws: string) => ({ workspaceId: ws, name: ws, path: `/w/${ws}`, agentName: ws.toUpperCase(), createdAt: 0 })
const dashboard: ProjectGroupDashboard = { id: 'd1', groupId: 'g1', name: 'Main', layoutJson, revision: 1, position: 0, createdAt: 0, updatedAt: 0 }
const show = {
  id: 'g1',
  name: 'Spring launch',
  pocWorkspaceId: 'a',
  members: ['a', 'b', 'c', 'd', 'e', 'f'].map(member),
  dashboards: [dashboard],
} as unknown as ProjectGroupShow

const panes = (): HTMLElement[] => Array.from(document.querySelectorAll<HTMLElement>('[data-pane-id]'))
const canvas = (): HTMLElement => document.querySelector('[data-dashboard-canvas]') as HTMLElement

describe('a dashboard in a narrow canvas', () => {
  it('draws the saved layout when every tile has room', () => {
    canvasW = 1200
    render(<ProjectDashboard show={show} dashboard={dashboard} />)
    resize(1200)
    expect(canvas().hasAttribute('data-dashboard-reflowed')).toBe(false)
    expect(panes()[0].style.width).toMatch(/^33\.3/)
    expect(panes().map((p) => p.dataset.paneId)).toEqual(['t:a', 't:b', 't:c', 't:d', 't:e', 't:f'])
  })

  it('reflows so no tile is under the minimum, never saves it, and restores the saved layout with room again', async () => {
    vi.useFakeTimers()
    try {
      canvasW = 1200
      render(<ProjectDashboard show={show} dashboard={dashboard} />)
      resize(380)
      expect(canvas().hasAttribute('data-dashboard-reflowed')).toBe(true)
      // One column: every tile the canvas's width, stacked, the same tiles in reading order.
      for (const p of panes()) expect(p.style.width).toBe('100%')
      expect(panes().map((p) => p.dataset.paneId)).toEqual(['t:a', 't:b', 't:c', 't:d', 't:e', 't:f'])
      // Too short stacked six high: the canvas scrolls down.
      const grid = panes()[0].parentElement as HTMLElement
      expect(parseFloat(grid.style.minHeight)).toBeGreaterThan(700)
      // No resize handles on a layout that isn't the saved one.
      expect(document.querySelector('.cursor-col-resize')).toBeNull()

      resize(600)
      // Two columns of 300px.
      for (const p of panes()) expect((parseFloat(p.style.width) / 100) * 600).toBeGreaterThanOrEqual(MIN_TILE_W_PX)

      resize(1200)
      expect(canvas().hasAttribute('data-dashboard-reflowed')).toBe(false)
      expect(panes()[0].style.width).toMatch(/^33\.3/)
      await act(async () => {
        vi.advanceTimersByTime(LAYOUT_SAVE_COALESCE_MS * 3)
      })
      expect(h.save).not.toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })
})

describe('each tile’s ⌘N hint', () => {
  it('numbers the tiles 1 to 6 in their reading order, saved or reflowed', () => {
    canvasW = 1200
    render(<ProjectDashboard show={show} dashboard={dashboard} />)
    const numbers = (): Array<[string | undefined, string | undefined]> =>
      panes().map((p) => [p.dataset.paneId, p.querySelector<HTMLElement>('[data-testid="tile"]')?.dataset.shortcut])
    const expected = [['t:a', '1'], ['t:b', '2'], ['t:c', '3'], ['t:d', '4'], ['t:e', '5'], ['t:f', '6']]
    resize(1200)
    expect(numbers()).toEqual(expected)
    resize(380)
    expect(numbers()).toEqual(expected)
  })
})

describe("each agent's tile opens in its own view", () => {
  it("the agent's own view, else the project's (a personal switch is useTileView's, on top)", () => {
    canvasW = 1200
    h.views = {}
    const withViews = {
      ...show,
      defaultView: 'chatter',
      members: [
        { ...member('a'), defaultView: 'terminal', ownDefaultView: 'terminal' },
        { ...member('b'), defaultView: 'chatter', ownDefaultView: null },
        // An older daemon's member row: the project's view.
        member('c'),
        member('d'),
        member('e'),
        member('f'),
      ],
    } as unknown as ProjectGroupShow
    render(<ProjectDashboard show={withViews} dashboard={dashboard} />)
    resize(1200)
    expect(h.views.a).toBe('terminal')
    expect(h.views.b).toBe('chatter')
    expect(h.views.c).toBe('chatter')
  })
})
