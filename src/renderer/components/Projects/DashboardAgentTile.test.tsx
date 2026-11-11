// @vitest-environment jsdom
// prd-projects-threads-v1 §3: an agent tile's views. The terminal stays
// mounted in every view of a live tile, the default comes from the project,
// an asleep agent's Thread has a Wake bar, and there is no Split.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { useEffect, type ReactNode } from 'react'

const mounts = vi.hoisted(() => ({ terminal: 0, unmounts: 0 }))

vi.mock('@/kessel-term/TerminalPane', async () => {
  const { useSessionViewChrome } = await import('@/components/SessionView/sessionViewChrome')
  return {
    TerminalPane: (props: { terminalId: string; attachAgentName?: string }) => {
      const chrome = useSessionViewChrome()
      useEffect(() => {
        mounts.terminal++
        return () => {
          mounts.unmounts++
        }
      }, [])
      return (
        <div
          data-testid="mock-terminal"
          data-view={chrome?.viewTab}
          data-view-switch={chrome?.viewSwitch ? '' : undefined}
          data-revealed={String(chrome?.terminalRevealed)}
          data-panel={String(chrome?.threadPanel)}
          data-attach={props.attachAgentName}
        />
      )
    },
  }
})
vi.mock('@/components/SessionView/ThreadOverlayColumn', () => ({
  ThreadOverlayColumn: ({ addr, composeBar }: { addr: string; composeBar: ReactNode }) => (
    <div data-testid="thread-column" data-addr={addr}>
      <div data-testid="agent-session-thread-compose-slot">{composeBar}</div>
    </div>
  ),
}))
vi.mock('@/components/SessionView/ChatterOverlayPane', () => ({
  ChatterOverlayPane: ({ addr }: { addr: string }) => <div data-testid="chatter-pane" data-addr={addr} />,
}))
vi.mock('@/components/SessionView/ThreadDrawerPanel', () => ({
  InputsList: ({ cwd, active }: { cwd: string; active: boolean }) => (
    <div data-testid="heartbeats-list" data-cwd={cwd} data-active={String(active)} />
  ),
}))
vi.mock('@/components/SessionView/ThreadFloatBell', () => ({ ThreadFloatBell: () => <button type="button" data-testid="float-bell" /> }))
vi.mock('@/components/SessionView/ThreadFloatSearch', () => ({ ThreadFloatSearch: () => <button type="button" data-testid="float-search" /> }))
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({ AgentAvatar: () => <span /> }))
vi.mock('@/stores/projects', () => ({ activateProject: vi.fn() }))
const wake = vi.hoisted(() => ({ calls: [] as string[] }))
vi.mock('./wake-member-session', () => ({
  resolveAllMemberSessions: vi.fn(async () => ({})),
  activateOnLiveSessionAttach: vi.fn(),
  wakeCanonicalMemberSession: vi.fn(async (ws: string) => {
    wake.calls.push(ws)
  }),
}))

import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { DashboardAgentTile, TileViewPicker, tileSessionViewTab, useTileView, type TermPhase } from './DashboardAgentTile'
import type { ProjectDefaultView, ProjectGroupMemberInfo } from './projects-api'

const room = testRoom({
  tabs: {},
  scope: primaryScope(),
  projects: [{ id: 'ws-sales', path: '/w/sales', handle: 'sales' } as unknown as ProjectWithWorkspaces],
})
const member: ProjectGroupMemberInfo = {
  workspaceId: 'ws-sales',
  name: 'sales',
  path: '/w/sales',
  agentName: 'sales',
  createdAt: 0,
}
const LIVE: TermPhase = { kind: 'live', sessionId: 's1' }
const DORMANT: TermPhase = { kind: 'dormant' }

/** The tile as ProjectDashboard draws it: the picker over the body. */
function Tile({ defaultView, phase = LIVE }: { defaultView?: ProjectDefaultView; phase?: TermPhase }) {
  const [view, setView] = useTileView('g1', 'ws-sales', defaultView)
  return (
    <div data-tile-view={view} data-testid="tile">
      <TileViewPicker view={view} onView={setView} />
      <DashboardAgentTile workspaceId="ws-sales" member={member} dashboardId="d1" initialPhase={phase} view={view} addr="sales" />
    </div>
  )
}

const pick = (v: string): void => {
  fireEvent.click(screen.getByTestId(`tile-view-${v}`))
}
const tileView = (): string | null => screen.getByTestId('tile').getAttribute('data-tile-view')

beforeEach(() => {
  mounts.terminal = 0
  mounts.unmounts = 0
  wake.calls = []
  useProjectGroupsStore.setState({ tileViews: {} })
})
afterEach(() => cleanup())

describe("the tile's view picker", () => {
  it('is the small capsule (size="sm")', () => {
    renderInRoom(room, <Tile />)
    expect(screen.getByTestId('tile-view-picker').getAttribute('data-tab-size')).toBe('sm')
  })
})

describe('the default view', () => {
  it("comes from the project's defaultView", () => {
    renderInRoom(room, <Tile defaultView="chatter" />)
    expect(tileView()).toBe('chatter')
    expect(screen.getByTestId('mock-terminal').getAttribute('data-view')).toBe('chatter')
  })

  it('is Thread when the project has no defaultView (an older daemon)', () => {
    renderInRoom(room, <Tile defaultView={undefined} />)
    expect(tileView()).toBe('thread')
    expect(screen.getByTestId('mock-terminal').getAttribute('data-view')).toBe('thread')
  })

  it('a switch holds, and a default change resets it', () => {
    const { rerender } = renderInRoom(room, <Tile defaultView="thread" />)
    pick('terminal')
    expect(tileView()).toBe('terminal')
    rerender(<Tile defaultView="thread" />)
    expect(tileView()).toBe('terminal')
    rerender(<Tile defaultView="chatter" />)
    expect(tileView()).toBe('chatter')
    // Back to the old default: the switch is gone, not revived.
    rerender(<Tile defaultView="thread" />)
    expect(tileView()).toBe('thread')
    expect(useProjectGroupsStore.getState().tileViews).toEqual({})
  })
})

describe('a live tile', () => {
  it('keeps one terminal mounted across every view', () => {
    renderInRoom(room, <Tile />)
    expect(mounts.terminal).toBe(1)
    for (const v of ['terminal', 'chatter', 'heartbeats', 'thread', 'terminal']) {
      pick(v)
      expect(tileView()).toBe(v)
      expect(screen.getByTestId('mock-terminal')).toBeTruthy()
    }
    expect(mounts.terminal).toBe(1)
    expect(mounts.unmounts).toBe(0)
  })

  it('tells the terminal its view, with no drawers and no Split', () => {
    renderInRoom(room, <Tile />)
    const term = (): HTMLElement => screen.getByTestId('mock-terminal')
    expect(term().hasAttribute('data-view-switch')).toBe(true)
    expect(term().getAttribute('data-revealed')).toBe('false')
    expect(term().getAttribute('data-panel')).toBe('undefined')
    expect(term().getAttribute('data-attach')).toBe('ws-sales')
    for (const v of ['terminal', 'thread', 'chatter', 'heartbeats']) {
      pick(v)
      expect(term().getAttribute('data-view')).not.toBe('split')
    }
    expect(tileSessionViewTab('heartbeats')).toBe('chatter')
  })

  it('floats search and the bell over the Thread, search alone over Chatter, none in Terminal', () => {
    renderInRoom(room, <Tile />)
    expect(screen.getByTestId('float-search')).toBeTruthy()
    expect(screen.getByTestId('float-bell')).toBeTruthy()
    pick('chatter')
    expect(screen.getByTestId('float-search')).toBeTruthy()
    expect(screen.queryByTestId('float-bell')).toBeNull()
    pick('terminal')
    expect(screen.queryByTestId('tile-floats')).toBeNull()
  })

  it('Heartbeats shows the workspace fires full-tile and hides the session under it', () => {
    renderInRoom(room, <Tile />)
    pick('heartbeats')
    const list = screen.getByTestId('heartbeats-list')
    expect(list.getAttribute('data-cwd')).toBe('/w/sales')
    expect(list.getAttribute('data-active')).toBe('true')
    expect((screen.getByTestId('tile-session') as HTMLElement).style.display).toBe('none')
    expect(screen.queryByTestId('tile-floats')).toBeNull()
    pick('thread')
    expect(screen.queryByTestId('heartbeats-list')).toBeNull()
    expect((screen.getByTestId('tile-session') as HTMLElement).style.display).toBe('')
  })
})

describe('an asleep tile', () => {
  it("shows the Thread's history with a Wake bar where the box goes", async () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    expect(screen.getByTestId('thread-column').getAttribute('data-addr')).toBe('sales')
    expect(screen.getByTestId('tile-wake-bar').textContent).toContain('sales is asleep.')
    await act(async () => {
      fireEvent.click(screen.getByTestId('tile-wake'))
    })
    expect(wake.calls).toEqual(['ws-sales'])
  })

  it('still checking its session: the message box is there at once, then a Wake bar if it is asleep', () => {
    const r = renderInRoom(room, <Tile phase={{ kind: 'checking' }} />)
    const box = screen.getByTestId('agent-session-thread-compose-slot').querySelector('textarea') as HTMLTextAreaElement
    expect(box).not.toBeNull()
    expect(box.disabled).toBe(false)
    fireEvent.change(box, { target: { value: 'morning' } })
    expect(box.value).toBe('morning')
    expect(screen.queryByTestId('tile-wake-bar')).toBeNull()
    r.rerender(
      <div data-tile-view="thread" data-testid="tile">
        <DashboardAgentTile workspaceId="ws-sales" member={member} dashboardId="d1" initialPhase={DORMANT} view="thread" addr="sales" />
      </div>,
    )
    expect(screen.getByTestId('tile-wake-bar').textContent).toContain('sales is asleep.')
  })

  it('Terminal view offers Wake too; Heartbeats reads without a session', () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    pick('terminal')
    expect(screen.getByText('sales is asleep')).toBeTruthy()
    expect(screen.getByText('Wake')).toBeTruthy()
    pick('heartbeats')
    expect(screen.getByTestId('heartbeats-list')).toBeTruthy()
    expect(screen.queryByTestId('mock-terminal')).toBeNull()
  })

  it("both Wake buttons take Glass's button look", () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    // The Wake bar's Wake is the shared accent Button (pill, edge, fill).
    const barWake = screen.getByTestId('tile-wake')
    expect(barWake.getAttribute('data-k2-chamfer')).toBe('control')
    expect(barWake.className).toContain('bg-[var(--color-accent-fill)]')
    expect(barWake.className).toContain('[box-shadow:var(--ring-field)]')
    // Terminal view's Wake keeps Terminal's wash; Glass styles it by its hook.
    pick('terminal')
    const termWake = screen.getByRole('button', { name: 'Wake' })
    expect(termWake.getAttribute('data-pane-action')).toBe('accent')
    expect(termWake.className).toContain('bg-[var(--color-accent)]/15')
  })
})

describe('the Glass hook for a tile body action', () => {
  it('gives data-pane-action the pill, edge and accent fill, in Glass only and never in a Garden', async () => {
    const { readFileSync } = await import('node:fs')
    const { join } = await import('node:path')
    const css = readFileSync(join(process.cwd(), 'styles', 'glass', 'overrides.css'), 'utf8')
    const rule = (sel: string): string => {
      const at = css.indexOf(`${sel} {`)
      expect(at, sel).toBeGreaterThan(-1)
      return css.slice(at, css.indexOf('}', at))
    }
    const shape = rule('[data-style="glass"] button[data-pane-action]:not([data-zen-root] *)')
    expect(shape).toContain('border-radius: var(--radius-field)')
    expect(shape).toContain('box-shadow: var(--ring-field)')
    const fill = rule('[data-style="glass"] button[data-pane-action="accent"]:not([data-zen-root] *)')
    expect(fill).toContain('background: var(--color-accent-fill)')
    expect(fill).toContain('color: var(--color-on-accent)')
  })
})

describe('switching a tile', () => {
  it("never touches the Agents page's view memory", () => {
    const before = JSON.stringify({ ...localStorage })
    renderInRoom(room, <Tile />)
    pick('terminal')
    pick('chatter')
    expect(JSON.stringify({ ...localStorage })).toBe(before)
  })
})
