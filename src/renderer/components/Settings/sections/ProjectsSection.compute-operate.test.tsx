// @vitest-environment jsdom

// The Agent tab's "Allow this agent to operate compute nodes" (node admin
// Q14, 0156): the owner's standing approval for the operate class. One
// gesture = one agent-access write (toggle `compute_operate`, the same
// route as `k2 compute operate`). Owner/Admin only: a Member never sees it.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return { ...orig, setWorkspaceComputeOperate: vi.fn() }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn() }))

import * as api from './compute-api'
import * as presence from '@/components/Presence/PresenceKickButton'
import { AgentsOperateComputeToggle, operateComputeVisibility } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const m = vi.mocked(api)
const role = vi.mocked(presence.fetchViewerRole)
const NAME = 'Allow this agent to operate compute nodes'

function project(on: number): ProjectWithWorkspaces {
  return { id: 'p-it', name: 'it', path: '/w/it', agentsCanOperateCompute: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-it'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsOperateComputeToggle project={p} />
}

async function enabledSwitch(): Promise<HTMLElement> {
  const sw = await screen.findByRole('switch', { name: NAME })
  await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
  return sw
}

beforeEach(() => {
  vi.clearAllMocks()
  role.mockResolvedValue('owner')
  useProjectsStore.setState({ projects: [project(0)] } as never)
})

afterEach(() => cleanup())

describe('operateComputeVisibility', () => {
  it('owner and admin flip it, a member never sees it, unknown shows it disabled', () => {
    expect(operateComputeVisibility('owner')).toEqual({ show: true, canFlip: true })
    expect(operateComputeVisibility('admin')).toEqual({ show: true, canFlip: true })
    expect(operateComputeVisibility('member')).toEqual({ show: false, canFlip: false })
    expect(operateComputeVisibility(null)).toEqual({ show: true, canFlip: false })
  })
})

describe('Agent tab: Allow this agent to operate compute nodes', () => {
  it('on: one compute_operate write; the store follows', async () => {
    m.setWorkspaceComputeOperate.mockResolvedValue({ ok: true, workspace: 'it', toggle: 'compute_operate', agentsCanOperateCompute: true })
    render(<Harness />)
    const sw = await enabledSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/each of those still waits for your approval/)).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceComputeOperate).toHaveBeenCalledWith('/w/it', true))
    expect(m.setWorkspaceComputeOperate).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true'))
    expect(useProjectsStore.getState().projects[0].agentsCanOperateCompute).toBe(1)
  })

  it('off: one write with false (an admin login may flip it)', async () => {
    role.mockResolvedValue('admin')
    useProjectsStore.setState({ projects: [project(1)] } as never)
    m.setWorkspaceComputeOperate.mockResolvedValue({ ok: true, agentsCanOperateCompute: false })
    render(<Harness />)
    fireEvent.click(await enabledSwitch())
    await waitFor(() => expect(m.setWorkspaceComputeOperate).toHaveBeenCalledWith('/w/it', false))
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanOperateCompute).toBe(0))
  })

  it('a refusal rolls back and shows the daemon hint', async () => {
    m.setWorkspaceComputeOperate.mockRejectedValue(new Error(JSON.stringify({ error: { code: 'forbidden', hint: 'only the owner or an admin login may change agent access' } })))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(await enabledSwitch())
    await screen.findByText('only the owner or an admin login may change agent access (forbidden)')
    expect(useProjectsStore.getState().projects[0].agentsCanOperateCompute).toBe(0)
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('a Member login never sees it', async () => {
    role.mockResolvedValue('member')
    render(<Harness />)
    await waitFor(() => expect(role).toHaveBeenCalled())
    await waitFor(() => expect(screen.queryByRole('switch', { name: NAME })).toBeNull())
    expect(m.setWorkspaceComputeOperate).not.toHaveBeenCalled()
  })

  it('until the role is known it shows but cannot be flipped', async () => {
    role.mockResolvedValue(null)
    render(<Harness />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect(role).toHaveBeenCalled())
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(m.setWorkspaceComputeOperate).not.toHaveBeenCalled()
    expect(useProjectsStore.getState().projects[0].agentsCanOperateCompute).toBe(0)
  })
})
