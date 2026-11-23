// @vitest-environment jsdom
//
// Home groups: what a swipe costs on the main thread, through the real
// AgentsShell + HomeSidebar with 12 agents a group (0.46.18; 0.46.17 felt
// laggy: the first sideways wheel event mounted the next group's live rows
// and the bar re-rendered every 5% of travel). The gate is what renders
// (React Profilers around the list and its neighbours), not wall time.
// K2_SWIPE_COST=1 prints the times too.

import { afterEach, beforeEach, describe, it, vi, expect } from 'vitest'
import { Profiler } from 'react'
import { act, cleanup, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({
  default: () => null,
  hostDisplayAddress: (h: { hostname: string; port: number; secure: boolean }) =>
    h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`,
}))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({
  default: ({ leading, children }: { leading?: React.ReactNode; children?: React.ReactNode }) => (
    <>
      {leading}
      {children}
    </>
  ),
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/lib/titlebar-drag', () => ({
  titleBarDragOnMouseDown: () => undefined,
  titleBarOnContextMenu: () => undefined,
  titleBarOnDoubleClick: () => undefined,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ found: false, dataUrl: null })),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
vi.mock('@/components/Terminal/TerminalArea', () => ({
  TerminalArea: ({ cwd }: { cwd: string }) => <div data-testid="terminal-area" data-cwd={cwd} />,
}))
vi.mock('@/components/FileTree/FileTree', () => ({ default: () => <div /> }))
vi.mock('@/components/AgentPanel/AgentPanel', () => ({ default: () => <div /> }))
vi.mock('@/components/ChatHistory/ChatHistory', () => ({ default: () => <div /> }))
vi.mock('@/components/WorkspacePanel/WorkspacePanel', () => ({ default: () => <div /> }))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))
vi.mock('@/components/Sidebar/Sidebar', async () => {
  const real = await vi.importActual<typeof import('@/components/Sidebar/Sidebar')>('@/components/Sidebar/Sidebar')
  return { ...real, default: () => <div data-testid="agents-sidebar" /> }
})

const byId = vi.hoisted(() => ({ sums: {} as Record<string, number> }))
vi.mock('./groups/GroupSwipeSlide', async () => {
  const React = await import('react')
  const real = await vi.importActual<typeof import('./groups/GroupSwipeSlide')>('./groups/GroupSwipeSlide')
  const on = (id: string, _p: string, ms: number): void => {
    byId.sums[id] = (byId.sums[id] ?? 0) + ms
  }
  const Strip = (p: Record<string, unknown>) => {
    const props = { ...p } as Record<string, unknown>
    const rn = p.renderNeighbour as (g: unknown) => React.ReactNode
    props.renderNeighbour = (g: unknown) => React.createElement(React.Profiler, { id: 'neighbour', onRender: on }, rn(g))
    props.children = React.createElement(React.Profiler, { id: 'list', onRender: on }, p.children as React.ReactNode)
    return React.createElement(real.GroupSwipeStrip as never, props as never)
  }
  return { ...real, GroupSwipeStrip: Strip }
})

const h = vi.hoisted(() => {
  function hookOf<T extends object>(state: T) {
    const hook = ((sel: (s: T) => unknown) => sel(state)) as ((sel: (s: T) => unknown) => unknown) & {
      getState: () => T
      setState: (p: Partial<T>) => void
    }
    hook.getState = () => state
    hook.setState = (p) => Object.assign(state, p)
    return hook
  }
  const activity = hookOf({ agents: new Map(), liveSessionCwds: new Set<string>() })
  const presence = hookOf({ roster: [] as unknown[], supported: true })
  return { activity, presence }
})

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  const projects = Array.from({ length: 24 }, (_, i) => ({
    id: `p${i}`,
    name: `Agent ${i}`,
    path: `/w/a${i}`,
    handle: `a${i}`,
    color: '#e06c75',
    iconUrl: null,
    workspaces: [{ id: `w${i}`, name: 'main', type: 'main', navVisible: 0, worktreePath: null }],
  }))
  const useProjectsStore = create<{
    projects: typeof projects
    activeProjectId: string | null
    activeWorkspaceId: string | null
    setActiveProject: (id: string) => void
    setActiveWorkspace: (projectId: string, workspaceId: string) => void
  }>((set) => ({
    projects,
    activeProjectId: 'p0',
    activeWorkspaceId: 'w0',
    setActiveProject: (id) => set({ activeProjectId: id }),
    setActiveWorkspace: (projectId, workspaceId) => set({ activeProjectId: projectId, activeWorkspaceId: workspaceId }),
  }))
  return { useProjectsStore }
})
vi.mock('@/stores/active-agents', () => ({
  useActiveAgentsStore: h.activity,
  projectHasLiveSession: (cwds: Set<string>, path: string) => cwds.has(path),
}))
vi.mock('@/stores/presence', async () => {
  const real = await vi.importActual<typeof import('@/stores/presence')>('@/stores/presence')
  return { ...real, usePresenceStore: h.presence }
})
vi.mock('@/stores/session-events', () => ({
  onAppHello: vi.fn(() => () => undefined),
  onPresenceChanged: vi.fn(() => () => undefined),
  onPersonPictureChanged: vi.fn(() => () => undefined),
  onFirstRunChanged: vi.fn(() => () => undefined),
  onLlmAccountsChanged: vi.fn(() => () => undefined),
  onProjectsChanged: vi.fn(() => () => undefined),
  onTunnelStatusChanged: vi.fn(() => () => undefined),
  onTunnelIdentityChanged: vi.fn(() => () => undefined),
  onPairCodeChanged: vi.fn(() => () => undefined),
}))

import AgentsShell from '@/components/Layout/AgentsShell'
import { HomeShellEffects } from './home-room'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore, __resetConnectHostStoreForTests } from '@/stores/connect-host'
import { useGroupSwipeStore } from '@/hooks/useGroupSwipe'

if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
Element.prototype.scrollIntoView = vi.fn()
vi.stubGlobal(
  'fetch',
  vi.fn(async () => new Response('{}', { status: 404 })),
)

function Shell(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const page = usePageViewStore((s) => s.page)
  const p = projects[0] as never as Parameters<typeof AgentsShell>[0]['activeProject']
  return (
    <>
      <AgentsShell activeProject={p} activeWorkspace={undefined} cwd="/w/a0" />
      {page === 'home' && <HomeShellEffects />}
    </>
  )
}

const NAV = 260
const FRAME = 16
let commits: number[] = []
const onRender = (_id: string, _phase: string, actual: number): void => {
  commits.push(actual)
}

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance', 'requestAnimationFrame', 'cancelAnimationFrame'] })
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(NAV)
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(NAV)
  __resetConnectHostStoreForTests()
  useConnectHostStore.setState({ connectionStatus: 'connected' })
  await useConnectHostStore.getState().hydrateFromDisk()
  const st = useHomesStore.getState()
  const before = st.homes.map((x) => x.id)
  const made = ['One', 'Two', 'Three'].map((n) => st.createHome(n) as string)
  for (const id of before) useHomesStore.getState().deleteHome(id)
  made.forEach((hid, g) => {
    for (let i = 0; i < 8; i++) {
      const n = g * 8 + i
      useHomesStore.getState().addRow(hid, { address: `a${n}::local`, workspaceId: `p${n}`, label: `Agent ${n}` })
    }
    for (let i = 0; i < 4; i++) {
      useHomesStore.getState().addRow(hid, { address: `r${g}${i}::b${g}.k2.dev`, workspaceId: `q${g}${i}`, label: `Remote ${g}${i}` })
    }
  })
  useHomesStore.getState().selectHome(made[0])
  usePageViewStore.getState().setPage('home')
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.useRealTimers()
  useGroupSwipeStore.setState({ swipe: null, release: null })
})

function stats(xs: number[]): string {
  if (xs.length === 0) return 'n=0'
  const s = [...xs].sort((p, q) => p - q)
  const sum = xs.reduce((p, q) => p + q, 0)
  return `n=${xs.length} max=${s[s.length - 1].toFixed(2)} p50=${s[Math.floor(s.length / 2)].toFixed(2)} sum=${sum.toFixed(1)}`
}

describe('a swipe through the real Home nav (12 agents a group)', () => {
  it('nothing renders a row while the fingers move: one small commit for the bar, the neighbours drawn ahead', () => {
    render(
      <Profiler id="shell" onRender={onRender}>
        <Shell />
      </Profiler>,
    )
    act(() => vi.advanceTimersByTime(500))
    const target = (): Element => {
      const el = document.querySelector('[data-home-list] [data-agent-row]')
      if (!el) throw new Error('no row')
      return el
    }
    const out: string[] = []
    for (let rep = 0; rep < 6; rep++) {
      const dir = rep % 2 === 0 ? 1 : -1
      const wall: number[] = []
      commits = []
      byId.sums = {}
      // 26 frames at 6 px: 156 px = 60% of the nav, then the fingers lift.
      for (let i = 0; i < 26; i++) {
        const t0 = process.hrtime.bigint()
        act(() => {
          vi.advanceTimersByTime(FRAME)
          target().dispatchEvent(new WheelEvent('wheel', { deltaX: 6 * dir, deltaY: 0, bubbles: true }))
        })
        wall.push(Number(process.hrtime.bigint() - t0) / 1e6)
      }
      // The bar's two refreshes (moving, then past the line); no list, no neighbour.
      expect(commits.length).toBeLessThanOrEqual(2)
      expect(byId.sums).toEqual({})
      const trackCommits = commits
      commits = []
      const t1 = process.hrtime.bigint()
      act(() => vi.advanceTimersByTime(600))
      const landWall = Number(process.hrtime.bigint() - t1) / 1e6
      const landIds = JSON.stringify(Object.fromEntries(Object.entries(byId.sums).map(([k, v]) => [k, +v.toFixed(1)])))
      byId.sums = {}
      expect(selectedHome(useHomesStore.getState()).name).toBe(dir > 0 ? 'Two' : 'One')
      out.push(`swipe ${rep}: per wheel event ${stats(wall)} ms | commits while moving ${stats(trackCommits)} | landing ${landWall.toFixed(1)} ms ${landIds}`)
    }
    // K2_SWIPE_COST=1 prints the times (jsdom: a main-thread proxy, not WebKit's).
    if (process.env.K2_SWIPE_COST) console.log('\n' + out.join('\n'))
  })
})
