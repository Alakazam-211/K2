// @vitest-environment jsdom
// "Allow a virtual desktop" on an agent's settings (prd-linux-virtual-desktop-v1
// LD-38, 0301):
//   - only on a server that has virtual desktops (`virtual-desktop-v1`, Linux);
//   - a Member never sees it; the Owner and an Admin turn it on and off;
//   - its state is the agent desktop's `agentsAllowed` in desktop/status
//     (?workspace=<project id>);
//   - a flip posts agent-access/set {toggle: 'desktop', workspace, value};
//   - a refusal puts it back with the daemon's words.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  allowed: false,
  gets: [] as { route: string; params: unknown }[],
  posts: [] as { route: string; body: unknown }[],
  fail: null as Error | null,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params: unknown) => {
    cli.gets.push({ route, params })
    if (route !== 'desktop/status') throw new Error(`unexpected GET ${route}`)
    return { desktop: 'virtual:ada', kind: 'agent', agentsAllowed: cli.allowed }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (cli.fail) throw cli.fail
    return {}
  }),
  isHostSwitchedError: () => false,
}))
const role = vi.hoisted(() => ({ current: 'owner' as string | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => role.current) }))
vi.mock('./PermissionCardsSection', () => ({ AskedChip: ({ row }: { row: string }) => <span data-testid="asked-chip" data-row={row} /> }))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AgentsVirtualDesktopToggle, VIRTUAL_DESKTOP_LABEL, VIRTUAL_DESKTOP_ROW } from './AgentsVirtualDesktopToggle'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const project = { id: 'ada', name: 'Ada', path: '/ws/ada', handle: 'ada' } as unknown as ProjectWithWorkspaces

function scope(features: string[]): ServerScope {
  return { serverSupports: (f: string) => features.includes(f) } as unknown as ServerScope
}

function mount(features = ['virtual-desktop-v1']): void {
  render(<AgentsVirtualDesktopToggle project={project} scope={scope(features)} />)
}

afterEach(() => {
  cleanup()
  cli.allowed = false
  cli.gets = []
  cli.posts = []
  cli.fail = null
  role.current = 'owner'
})

describe('Allow a virtual desktop (per agent)', () => {
  it('is not there on a server without virtual desktops (a Mac), and asks it nothing', async () => {
    mount(['remote-desktop-v1'])
    await new Promise((r) => setTimeout(r, 0))
    expect(screen.queryByRole('switch', { name: VIRTUAL_DESKTOP_LABEL })).toBeNull()
    expect(cli.gets).toEqual([])
  })

  it('reads agentsAllowed for the agent by its project id; the Owner turns it on with agent-access/set', async () => {
    mount()
    const sw = await screen.findByRole('switch', { name: VIRTUAL_DESKTOP_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(cli.gets).toEqual([{ route: 'desktop/status', params: { workspace: 'ada' } }])
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(sw.getAttribute('data-settings-id')).toBe(VIRTUAL_DESKTOP_ROW)
    fireEvent.click(sw)
    expect(sw.getAttribute('aria-checked')).toBe('true')
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'agent-access/set', body: { toggle: 'desktop', workspace: '/ws/ada', value: true } }]))
  })

  it('an Admin turns it off too', async () => {
    role.current = 'admin'
    cli.allowed = true
    mount()
    const sw = await screen.findByRole('switch', { name: VIRTUAL_DESKTOP_LABEL })
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('true'))
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'agent-access/set', body: { toggle: 'desktop', workspace: '/ws/ada', value: false } }]))
  })

  it('a Member never sees it', async () => {
    role.current = 'member'
    mount()
    await new Promise((r) => setTimeout(r, 0))
    await waitFor(() => expect(screen.queryByRole('switch', { name: VIRTUAL_DESKTOP_LABEL })).toBeNull())
  })

  it("a refusal puts it back and says the daemon's words", async () => {
    cli.fail = new Error('only the owner or an admin login may change agent access')
    mount()
    const sw = await screen.findByRole('switch', { name: VIRTUAL_DESKTOP_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await screen.findByText(/only the owner or an admin login may change agent access/)
    expect(sw.getAttribute('aria-checked')).toBe('false')
  })
})
