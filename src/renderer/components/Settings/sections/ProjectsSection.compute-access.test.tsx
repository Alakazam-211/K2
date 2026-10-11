// @vitest-environment jsdom

// The Agent tab's "Allow compute nodes" (Rosson 2026-10-11): the single
// per-agent compute control. One gesture = one daemon call; the daemon
// grants every machine, and the row says which ones.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return { ...orig, setWorkspaceComputeSwitch: vi.fn() }
})

import * as api from './compute-api'
import { AgentsUseComputeToggle } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const m = vi.mocked(api)

function project(on: number): ProjectWithWorkspaces {
  return { id: 'p-sales', name: 'sales', path: '/w/sales', agentsCanUseCompute: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-sales'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsUseComputeToggle project={p} />
}

beforeEach(() => {
  vi.clearAllMocks()
  useProjectsStore.setState({ projects: [project(0)] } as never)
})

afterEach(() => cleanup())

describe('Agent tab: Allow compute nodes', () => {
  it('on: one agent-access write; the row lists the machines granted', async () => {
    m.setWorkspaceComputeSwitch.mockResolvedValue({ ok: true, agentsCanUseCompute: true, granted: ['mini-1'], kept: ['mini-2'] })
    render(<Harness />)
    const sw = screen.getByRole('switch', { name: 'Allow compute nodes' })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/Allow this agent to use compute nodes: run builds and tests on the machines/)).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceComputeSwitch).toHaveBeenCalledWith('/w/sales', true))
    expect(m.setWorkspaceComputeSwitch).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(screen.getByTestId('agents-compute-result').textContent).toBe('Can use 2 machines: mini-1, mini-2.'))
    expect(screen.getByRole('switch', { name: 'Allow compute nodes' }).getAttribute('aria-checked')).toBe('true')
    expect(useProjectsStore.getState().projects[0].agentsCanUseCompute).toBe(1)
  })

  it('off: one write with false', async () => {
    useProjectsStore.setState({ projects: [project(1)] } as never)
    m.setWorkspaceComputeSwitch.mockResolvedValue({ ok: true, agentsCanUseCompute: false, grantsRemoved: 2 })
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: 'Allow compute nodes' }))
    await waitFor(() => expect(m.setWorkspaceComputeSwitch).toHaveBeenCalledWith('/w/sales', false))
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanUseCompute).toBe(0))
  })

  it('a refusal rolls back and shows the daemon hint', async () => {
    m.setWorkspaceComputeSwitch.mockRejectedValue(new Error(JSON.stringify({ error: { code: 'forbidden', hint: 'only the owner or an admin login may change agent access' } })))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: 'Allow compute nodes' }))
    await screen.findByText('only the owner or an admin login may change agent access (forbidden)')
    expect(useProjectsStore.getState().projects[0].agentsCanUseCompute).toBe(0)
    spy.mockRestore()
  })
})
