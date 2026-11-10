// @vitest-environment jsdom
// "Manage the org chart" on an agent's settings (prd-org-chart-v1 §7):
//   - only while the server's Org Chart is on; a Member never sees it;
//   - its state is that agent's `canManage` on the chart;
//   - the Owner turns it on and off; an Admin may only turn it off;
//   - a flip posts agent-access/set {toggle: 'org_chart_manage', workspace, value};
//   - a refusal puts it back with the daemon's words;
//   - a held `organization.manage` card shows on this row.
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  canManage: false,
  posts: [] as { route: string; body: unknown }[],
  fail: null as Error | null,
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { orgChartFixture } = await import('@/components/OrgChart/org-chart-fixture')
  return {
    daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
      if (route !== 'organization/chart') throw new Error(`unexpected GET ${route}`)
      const c = orgChartFixture()
      return { ...c, agents: c.agents.map((a) => (a.id === 'ada' ? { ...a, canManage: cli.canManage } : a)) }
    }),
    daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
      cli.posts.push({ route, body })
      if (cli.fail) throw cli.fail
      return {}
    }),
    isHostSwitchedError: () => false,
  }
})
const role = vi.hoisted(() => ({ current: 'owner' as string | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => role.current) }))
vi.mock('./PermissionCardsSection', () => ({ AskedChip: ({ row }: { row: string }) => <span data-testid="asked-chip" data-row={row} /> }))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AgentsManageOrgChartToggle, MANAGE_ORG_CHART_LABEL, MANAGE_ORG_CHART_ROW, manageOrgChartRights } from './AgentsManageOrgChartToggle'
import { PERMISSION_ACTION_ROWS } from './permission-cards-api'
import { useSettingsStore } from '@/stores/settings'
import { primaryScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const project = { id: 'ada', name: 'Ada', path: '/ws/ada', handle: 'ada' } as unknown as ProjectWithWorkspaces

function mount(): void {
  render(<AgentsManageOrgChartToggle project={project} scope={primaryScope()} />)
}

afterEach(() => {
  cleanup()
  cli.canManage = false
  cli.posts = []
  cli.fail = null
  role.current = 'owner'
  act(() => useSettingsStore.setState({ orgChart: false }))
})

describe('Manage the org chart (per agent)', () => {
  it('is not there while the Org Chart is off', () => {
    mount()
    expect(screen.queryByRole('switch', { name: MANAGE_ORG_CHART_LABEL })).toBeNull()
  })

  it("reads the agent's canManage from the chart; the Owner turns it on with agent-access/set", async () => {
    act(() => useSettingsStore.setState({ orgChart: true }))
    mount()
    const sw = await screen.findByRole('switch', { name: MANAGE_ORG_CHART_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    fireEvent.click(sw)
    expect(sw.getAttribute('aria-checked')).toBe('true')
    await waitFor(() =>
      expect(cli.posts).toEqual([{ route: 'agent-access/set', body: { toggle: 'org_chart_manage', workspace: '/ws/ada', value: true } }]),
    )
  })

  it("a refusal puts it back and says the daemon's words", async () => {
    cli.fail = new Error('only the server\'s owner turns on "Manage the org chart"')
    act(() => useSettingsStore.setState({ orgChart: true }))
    mount()
    const sw = await screen.findByRole('switch', { name: MANAGE_ORG_CHART_LABEL })
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    expect(await screen.findByTestId('agents-manage-org-chart-error')).toHaveProperty(
      'textContent',
      'only the server\'s owner turns on "Manage the org chart"',
    )
    expect(sw.getAttribute('aria-checked')).toBe('false')
  })

  it('an Admin may turn it off but not on', async () => {
    role.current = 'admin'
    act(() => useSettingsStore.setState({ orgChart: true }))
    mount()
    const off = await screen.findByRole('switch', { name: MANAGE_ORG_CHART_LABEL })
    await waitFor(() => expect(off.title).toBe("Only the server's owner turns this on"))
    expect((off as HTMLButtonElement).disabled).toBe(true)
    cleanup()
    cli.canManage = true
    mount()
    const on = await screen.findByRole('switch', { name: MANAGE_ORG_CHART_LABEL })
    await waitFor(() => expect(on.getAttribute('aria-checked')).toBe('true'))
    await waitFor(() => expect((on as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(on)
    await waitFor(() =>
      expect(cli.posts).toEqual([{ route: 'agent-access/set', body: { toggle: 'org_chart_manage', workspace: '/ws/ada', value: false } }]),
    )
  })

  it('a Member never sees it', async () => {
    role.current = 'member'
    act(() => useSettingsStore.setState({ orgChart: true }))
    mount()
    await waitFor(() => expect(cli.posts).toEqual([]))
    await new Promise((r) => setTimeout(r, 0))
    expect(screen.queryByRole('switch', { name: MANAGE_ORG_CHART_LABEL })).toBeNull()
  })

  it('the rights, and a held organization.manage card lands on this row', () => {
    expect(manageOrgChartRights('owner', false)).toEqual({ show: true, canFlip: true })
    expect(manageOrgChartRights('admin', false)).toEqual({ show: true, canFlip: false })
    expect(manageOrgChartRights('admin', true)).toEqual({ show: true, canFlip: true })
    expect(manageOrgChartRights('member', true)).toEqual({ show: false, canFlip: false })
    expect(PERMISSION_ACTION_ROWS['organization.manage']).toBe(MANAGE_ORG_CHART_ROW)
  })
})
