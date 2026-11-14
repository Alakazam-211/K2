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
  const { ThreadOverlayColumn } = await import('@/components/SessionView/ThreadOverlayColumn')
  const { TerminalComposeBar } = await import('@/components/Terminal/TerminalComposeBar')
  return {
    TerminalPane: (props: { terminalId: string; attachAgentName?: string; cwd: string; sessionId?: string; holdSpawn?: boolean }) => {
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
          data-sessionid={props.sessionId ?? 'NONE'}
          data-hold={String(props.holdSpawn === true)}
        >
          {/* Thread view: the Thread and its message box, where the real pane draws them. */}
          {chrome?.viewTab === 'thread' ? (
            <ThreadOverlayColumn
              addr={chrome.overlayAddr}
              conversationId={chrome.conversationId}
              active
              composeBar={
                <TerminalComposeBar sessionId="" draftId={String(props.attachAgentName)} workspacePath={props.cwd} sendDestination="thread" />
              }
            />
          ) : null}
        </div>
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
  // Draws its `empty` (the list is empty in these tests).
  InputsList: ({ cwd, active, empty }: { cwd: string; active: boolean; empty?: ReactNode }) => (
    <div data-testid="heartbeats-list" data-cwd={cwd} data-active={String(active)}>
      {empty}
    </div>
  ),
}))
const nav = vi.hoisted(() => ({ agents: [] as string[] }))
vi.mock('@/lib/page-selection', () => ({
  openOnAgentsPage: (id: string) => {
    nav.agents.push(id)
  },
}))
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
import { DashboardAgentTile, TileViewMenu, TileViewPicker, tileSessionViewTab, useTileView, type TermPhase } from './DashboardAgentTile'
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
  nav.agents = []
  useProjectGroupsStore.setState({ tileViews: {} })
})
afterEach(() => cleanup())

describe("the tile's view picker", () => {
  it('is the small capsule (size="sm")', () => {
    renderInRoom(room, <Tile />)
    expect(screen.getByTestId('tile-view-picker').getAttribute('data-tab-size')).toBe('sm')
  })
})

describe("the picker folded to one button (a narrow tile's bar)", () => {
  it('names the current view, opens the four views with it checked, and switches', () => {
    const picked: string[] = []
    renderInRoom(room, <TileViewMenu view="thread" onView={(v) => picked.push(v)} />)
    const b = screen.getByRole('button', { name: 'View: Thread' })
    expect(b.getAttribute('aria-haspopup')).toBe('menu')
    // The shared button's focus ring (Button: k2-focus, the control cut).
    expect(b.className).toContain('k2-focus')
    expect(b.getAttribute('data-k2-chamfer')).toBe('control')
    fireEvent.click(b)
    const radios = screen.getAllByRole('menuitemradio')
    expect(radios.map((r) => r.textContent)).toEqual(['Terminal', 'Thread', 'Chatter', 'Heartbeats'])
    expect(radios.map((r) => r.getAttribute('aria-checked'))).toEqual(['false', 'true', 'false', 'false'])
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Thread' }))
    expect(picked).toEqual([])
    fireEvent.click(b)
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Chatter' }))
    expect(picked).toEqual(['chatter'])
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

  it('floats nothing over the Thread or Chatter: the bell and search are in the bar (TileBarTools)', () => {
    renderInRoom(room, <Tile />)
    for (const v of ['thread', 'chatter', 'terminal', 'heartbeats']) {
      pick(v)
      expect(document.querySelector('[data-thread-floats]')).toBeNull()
      expect(document.querySelector('[data-thread-floats-host]')).toBeNull()
      expect(screen.queryByTestId('thread-search-button')).toBeNull()
      expect(screen.queryByTestId('workspace-completion-bell')).toBeNull()
    }
  })

  it('Heartbeats shows the workspace fires full-tile and hides the session under it', () => {
    renderInRoom(room, <Tile />)
    pick('heartbeats')
    const list = screen.getByTestId('heartbeats-list')
    expect(list.getAttribute('data-cwd')).toBe('/w/sales')
    expect(list.getAttribute('data-active')).toBe('true')
    expect((screen.getByTestId('tile-session') as HTMLElement).style.display).toBe('none')
    pick('thread')
    expect(screen.queryByTestId('heartbeats-list')).toBeNull()
    expect((screen.getByTestId('tile-session') as HTMLElement).style.display).toBe('')
  })
})

describe("the bar's asleep hint", () => {
  it('is told when the agent is asleep, and when it wakes', async () => {
    const told: boolean[] = []
    const onAsleep = (a: boolean): void => {
      told.push(a)
    }
    const { rerender } = renderInRoom(
      room,
      <DashboardAgentTile workspaceId="ws-sales" member={member} dashboardId="d1" initialPhase={DORMANT} view="thread" addr="sales" onAsleep={onAsleep} />,
    )
    expect(told.at(-1)).toBe(true)
    rerender(<DashboardAgentTile workspaceId="ws-sales" member={member} dashboardId="d1" initialPhase={LIVE} view="thread" addr="sales" onAsleep={onAsleep} />)
    expect(told.at(-1)).toBe(false)
  })
})

describe('an asleep tile', () => {
  it("shows the Thread's history with a Wake bar where the box goes", async () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    expect(screen.getByTestId('thread-column').getAttribute('data-addr')).toBe('sales')
    expect(screen.getByTestId('tile-wake-bar').textContent).toContain('sales is asleep.')
    // Under the Thread's box, not in it: inside, the box's own corners win
    // and Terminal's cut never reached the bar or its Wake.
    expect(screen.getByTestId('thread-column').contains(screen.getByTestId('tile-wake-bar'))).toBe(false)
    await act(async () => {
      fireEvent.click(screen.getByTestId('tile-wake'))
    })
    expect(wake.calls).toEqual(['ws-sales'])
  })

  it('still checking its session: the message box is there at once, then a Wake bar if it is asleep', () => {
    const r = renderInRoom(room, <Tile phase={{ kind: 'checking' }} />)
    // The live tile's pane, held: nothing spawns while the check runs.
    expect(screen.getByTestId('mock-terminal').getAttribute('data-hold')).toBe('true')
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
    // Asleep: the held pane went without ever spawning.
    expect(screen.queryByTestId('mock-terminal')).toBeNull()
  })

  it('still checking, then live: the same pane attaches, and the Thread and its box stay', () => {
    const tile = (phase: TermPhase): React.JSX.Element => (
      <div data-tile-view="thread" data-testid="tile">
        <DashboardAgentTile workspaceId="ws-sales" member={member} dashboardId="d1" initialPhase={phase} view="thread" addr="sales" />
      </div>
    )
    const r = renderInRoom(room, tile({ kind: 'checking' }))
    const column = screen.getByTestId('thread-column')
    const box = screen.getByTestId('agent-session-thread-compose-slot').querySelector('textarea') as HTMLTextAreaElement
    box.focus()
    fireEvent.change(box, { target: { value: 'morning' } })
    expect(mounts.terminal).toBe(1)
    r.rerender(tile(LIVE))
    const term = screen.getByTestId('mock-terminal')
    expect(term.getAttribute('data-hold')).toBe('false')
    expect(term.getAttribute('data-sessionid')).toBe('s1')
    expect(mounts.terminal).toBe(1)
    expect(mounts.unmounts).toBe(0)
    expect(screen.getByTestId('thread-column')).toBe(column)
    expect(screen.getByTestId('agent-session-thread-compose-slot').querySelector('textarea')).toBe(box)
    expect(box.value).toBe('morning')
    expect(document.activeElement).toBe(box)
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

  it('both Wake buttons are the shared accent Button', () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    // The Wake bar's Wake is the shared accent Button (pill, edge, fill).
    const barWake = screen.getByTestId('tile-wake')
    expect(barWake.getAttribute('data-k2-chamfer')).toBe('control')
    expect(barWake.className).toContain('bg-[var(--color-accent-fill)]')
    expect(barWake.className).toContain('[box-shadow:var(--ring-field)]')
    // Terminal view's Wake is the same Button (0.46.12: one button look in every tile state).
    pick('terminal')
    const termWake = screen.getByRole('button', { name: 'Wake' })
    expect(termWake.getAttribute('data-k2-chamfer')).toBe('control')
    expect(termWake.className).toContain('bg-[var(--color-accent-fill)]')
  })
})

describe('tile states', () => {
  it('Chatter view of an asleep agent shows the Wake bar under the mailbox, and Wake wakes it', async () => {
    renderInRoom(room, <Tile phase={DORMANT} />)
    pick('chatter')
    expect(screen.getByTestId('chatter-pane')).toBeTruthy()
    expect(screen.getByTestId('tile-wake-bar').textContent).toContain('sales is asleep.')
    await act(async () => {
      fireEvent.click(screen.getByTestId('tile-wake'))
    })
    expect(wake.calls).toEqual(['ws-sales'])
  })

  it('Chatter view of a live agent has no Wake bar', () => {
    renderInRoom(room, <Tile phase={LIVE} />)
    pick('chatter')
    expect(screen.queryByTestId('tile-wake-bar')).toBeNull()
  })

  it("a failed session read says whose session, the reason quietly, and Try again checks again (busy while it does)", async () => {
    const { resolveAllMemberSessions } = await import('./wake-member-session')
    let answer: (v: unknown) => void = () => {}
    vi.mocked(resolveAllMemberSessions).mockImplementationOnce(
      () => new Promise((r) => (answer = r as (v: unknown) => void)) as never,
    )
    renderInRoom(room, <Tile phase={{ kind: 'error', message: 'session registry is not answering' }} />)
    const state = screen.getByTestId('tile-error')
    expect(state.getAttribute('role')).toBe('alert')
    expect(state.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(state.textContent).toContain('Couldn’t reach sales’s session')
    expect(state.querySelector('[data-load-detail]')?.textContent).toBe('session registry is not answering')
    expect(state.querySelector('[data-load-detail]')?.className).not.toContain('font-mono')
    const again = screen.getByRole('button', { name: 'Try again' })
    expect(again.getAttribute('data-k2-chamfer')).toBe('control')
    await act(async () => {
      fireEvent.click(again)
    })
    // Still the failure, its button busy, until the read answers.
    const busy = screen.getByRole('button', { name: 'Trying again…' }) as HTMLButtonElement
    expect(busy.disabled).toBe(true)
    expect(busy.getAttribute('aria-busy')).toBe('true')
    expect(screen.getByTestId('tile-error')).toBeTruthy()
    await act(async () => {
      answer({ 'ws-sales': { kind: 'dormant' } })
    })
    expect(resolveAllMemberSessions).toHaveBeenCalledWith(['ws-sales'], expect.any(Function))
    expect(screen.queryByTestId('tile-error')).toBeNull()
  })

  it('a failure names one more next step: open the agent on the Agents page', () => {
    renderInRoom(room, <Tile phase={{ kind: 'error', message: 'session registry is not answering' }} />)
    const state = screen.getByTestId('tile-error')
    expect(state.textContent).toContain('Still failing? Open sales on the Agents page')
    const link = screen.getByRole('button', { name: 'Open sales on the Agents page' })
    expect(link.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(link)
    expect(nav.agents).toEqual(['ws-sales'])
  })

  it('checking and waking are the compact state card with the name, in type roles', () => {
    renderInRoom(room, <Tile phase={{ kind: 'checking' }} />)
    pick('terminal')
    const line = screen.getByText('Checking sales’s session…')
    const card = line.closest('[data-tile-state]')
    expect(card?.getAttribute('data-tile-state')).toBe('busy')
    expect(card?.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(line.closest('p')?.className).toContain('type-name')
    expect(line.closest('p')?.className).not.toMatch(/text-\[\d+px\]|text-xs/)
  })

  it('Heartbeats with nothing yet: the compact card says what shows there and how to set one up', async () => {
    const { useSettingsStore } = await import('@/stores/settings')
    const opened: unknown[][] = []
    const real = useSettingsStore.getState().openSettings
    useSettingsStore.setState({ openSettings: ((...a: unknown[]) => void opened.push(a)) as never })
    try {
      renderInRoom(room, <Tile phase={LIVE} />)
      pick('heartbeats')
      const card = screen.getByTestId('tile-heartbeats-empty')
      expect(card.textContent).toContain('No heartbeats yet')
      expect(card.textContent).toContain('A heartbeat wakes sales on a schedule. Each run shows here.')
      fireEvent.click(screen.getByRole('button', { name: 'Set up heartbeats' }))
      expect(opened).toEqual([['projects', 'ws-sales', 'schedule']])
    } finally {
      useSettingsStore.setState({ openSettings: real })
    }
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
