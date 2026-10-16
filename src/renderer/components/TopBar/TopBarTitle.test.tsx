// @vitest-environment jsdom
//
// The top bar's centre title (Rosson 2026-10-16). Local agent: `agent /
// branch` as before. An agent on ANOTHER server (Home): `[server icon with
// that server's status LEDs] server / agent / branch`, the server drawn like
// the switcher draws one (picker label, ellipsis, full name in the tooltip).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) => sel({ openSettings: () => {} }),
}))
vi.mock('@/lib/remote-session', () => ({ reviveRemoteSession: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { __resetServerScopesForTests } from '@/kessel/server-scope'
import { setPoolStatusSource, type PoolHostStatus } from '@/lib/pool-hooks'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { Room } from '@/stores/room'
import TopBarTitle, { remoteServerIndicator, roomWorkspaceName, useRoomWorkspaceName } from './TopBarTitle'
import { SERVER_ICON_PX } from './ServerSwitcher'

const LONG = 'a-really-long-customer-server-name.example.k2.dev'
const B: ConnectHost = {
  id: 'id-b',
  label: 'z3thon.k2.dev',
  hostname: '127.0.0.1',
  port: 59_998,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}
const B_KEY = '127.0.0.1:59998'
const GREEN = '#3fb950'
const AMBER = '#d29922'
const RED = '#f85149'

const poolStore = createStore<{ entries: Record<string, PoolHostStatus> }>(() => ({ entries: {} }))
function setPool(status: Partial<PoolHostStatus> | null): void {
  act(() =>
    poolStore.setState({
      entries: status ? { [B_KEY]: { reach: 'live', auth: 'ok', role: 'member', checkedAt: 1, ...status } } : {},
    }),
  )
}

function parts(): string[] {
  const title = screen.getByTestId('top-bar-title')
  return Array.from(title.children).map((el) => el.textContent ?? '')
}

beforeEach(() => {
  __resetServerScopesForTests()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
  setPoolStatusSource(poolStore)
  setPool(null)
})

afterEach(() => {
  cleanup()
  setPoolStatusSource(null)
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' } as never)
})

describe('local agent (the window’s server)', () => {
  it('shows agent / branch and no server crumb', () => {
    render(<TopBarTitle agent="cortana" branch="main" />)
    expect(parts()).toEqual(['cortana', '/', 'main'])
    expect(screen.queryByTestId('top-bar-title-server')).toBe(null)
    expect(screen.queryByTestId('server-status-icon')).toBe(null)
  })

  it('a worktree shows its branch the same way', () => {
    render(<TopBarTitle agent="cortana" branch="feat/icons" />)
    expect(screen.getByTestId('top-bar-title-branch').textContent).toBe('feat/icons')
  })

  it('no agent says so', () => {
    render(<TopBarTitle />)
    expect(screen.getByTestId('top-bar-title').textContent).toBe('No workspace selected')
  })
})

describe('agent on another server', () => {
  it('server / agent / branch, the server first with its icon and picker label', () => {
    render(<TopBarTitle agent="nora" branch="feat/press" remoteServer={{ hostKey: B_KEY, phase: 'open' }} />)
    expect(parts()).toEqual(['z3thon.k2.dev', '/', 'nora', '/', 'feat/press'])
    const server = screen.getByTestId('top-bar-title-server')
    const icon = server.firstElementChild
    expect(icon?.getAttribute('data-testid')).toBe('server-status-icon')
    expect(icon?.getAttribute('width')).toBe(String(SERVER_ICON_PX))
    expect(icon?.getAttribute('data-led-color')).toBe(GREEN)
    expect(server.getAttribute('title')).toBe('z3thon.k2.dev — Connection: connected')
    expect(server.getAttribute('aria-label')).toBe('z3thon.k2.dev — Connection: connected')
  })

  it('without a known branch: server / agent', () => {
    render(<TopBarTitle agent="nora" remoteServer={{ hostKey: B_KEY, phase: 'open' }} />)
    expect(parts()).toEqual(['z3thon.k2.dev', '/', 'nora'])
    expect(screen.queryByTestId('top-bar-title-branch')).toBe(null)
  })

  it('a long server name truncates with an ellipsis; the full name is in the tooltip', () => {
    useConnectHostStore.setState({ hosts: [{ ...B, label: LONG }] } as never)
    render(<TopBarTitle agent="nora" remoteServer={{ hostKey: B_KEY, phase: 'open' }} />)
    const name = screen.getByTestId('top-bar-title-server-name')
    expect(name.textContent).toBe(LONG)
    expect(name.className).toContain('truncate')
    expect(name.className).toMatch(/max-w-\[\d+px\]/)
    expect(screen.getByTestId('top-bar-title-server').getAttribute('title')).toBe(`${LONG} — Connection: connected`)
  })

  it('the LEDs follow THAT server: offline red, sign-in red, back to green', () => {
    render(<TopBarTitle agent="nora" remoteServer={{ hostKey: B_KEY, phase: 'open' }} />)
    const led = (): string | null => screen.getByTestId('server-status-icon').getAttribute('data-led-color')
    expect(led()).toBe(GREEN)
    setPool({ reach: 'offline' })
    expect(led()).toBe(RED)
    expect(screen.getByTestId('top-bar-title-server').getAttribute('title')).toBe('z3thon.k2.dev — Connection: offline')
    setPool({ auth: 'signin-required' })
    expect(led()).toBe(RED)
    expect(screen.getByTestId('top-bar-title-server').getAttribute('title')).toBe('z3thon.k2.dev — Sign-in required')
    setPool({})
    expect(led()).toBe(GREEN)
  })

  it('remoteServerIndicator: connecting while the room is read, red on error', () => {
    expect(remoteServerIndicator('resolving', 'ok')).toEqual({ color: AMBER, title: 'Connection: connecting' })
    expect(remoteServerIndicator('error', 'ok').color).toBe(RED)
    expect(remoteServerIndicator('not-found', 'ok').color).toBe(RED)
    expect(remoteServerIndicator('open', 'ok')).toEqual({ color: GREEN, title: 'Connection: connected' })
    expect(remoteServerIndicator('open', 'offline').color).toBe(RED)
    expect(remoteServerIndicator('resolving', 'signin').title).toBe('Sign-in required')
  })
})

describe('the remote room’s branch name', () => {
  const projects = [
    {
      id: 'p1',
      name: 'nora',
      workspaces: [
        { id: 'w1', name: 'main', branch: 'main' },
        { id: 'w2', name: 'feat/press', branch: 'feat/press' },
      ],
    },
  ] as unknown as ProjectWithWorkspaces[]

  it('roomWorkspaceName reads the workspace from the room id and its server’s list', () => {
    expect(roomWorkspaceName(`${B_KEY}|p1:w2`, projects)).toBe('feat/press')
    expect(roomWorkspaceName(`${B_KEY}|p1:w1`, projects)).toBe('main')
    expect(roomWorkspaceName(`${B_KEY}|p1:nope`, projects)).toBe(null)
    expect(roomWorkspaceName(null, projects)).toBe(null)
  })

  it('useRoomWorkspaceName follows the room’s list live', () => {
    const list = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects: [] }))
    const room = { roomId: () => `${B_KEY}|p1:w2`, projects: list } as unknown as Room
    function Probe(): React.JSX.Element {
      return <span data-testid="probe">{useRoomWorkspaceName(room) ?? 'none'}</span>
    }
    render(<Probe />)
    expect(screen.getByTestId('probe').textContent).toBe('none')
    act(() => list.setState({ projects }))
    expect(screen.getByTestId('probe').textContent).toBe('feat/press')
  })
})
