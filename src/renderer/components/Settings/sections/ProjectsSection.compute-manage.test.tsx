// @vitest-environment jsdom

// The Agent tab's "Allow managing compute nodes" (`compute_manage`, 0194):
// a local agent may add, confirm, rename, pause, retire and route machines
// and grant them to any workspace. One gesture = one agent-access write
// (toggle `compute_manage`, the same route as `k2 compute manage`). The
// daemon lets only the owner flip it, so an Admin sees it read-only and a
// Member never sees it. The renderer only shows the switch.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return { ...orig, setWorkspaceComputeManage: vi.fn() }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn() }))

import * as api from './compute-api'
import * as presence from '@/components/Presence/PresenceKickButton'
import { AgentsManageComputeToggle, PROJECTS_MANIFEST, manageComputeVisibility } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const m = vi.mocked(api)
const role = vi.mocked(presence.fetchViewerRole)
const NAME = 'Allow managing compute nodes'

function project(on: number): ProjectWithWorkspaces {
  return { id: 'p-cm', name: 'ops', path: '/w/ops', agentsCanManageCompute: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-cm'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsManageComputeToggle project={p} />
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

describe('manageComputeVisibility', () => {
  it('only the owner flips it; an admin sees it read-only; a member never sees it', () => {
    expect(manageComputeVisibility('owner')).toEqual({ show: true, canFlip: true })
    expect(manageComputeVisibility('admin')).toEqual({ show: true, canFlip: false })
    expect(manageComputeVisibility('member')).toEqual({ show: false, canFlip: false })
    expect(manageComputeVisibility(null)).toEqual({ show: true, canFlip: false })
  })
})

describe('Agent tab: Allow managing compute nodes', () => {
  it('defaults off; on is one compute_manage write and the store follows', async () => {
    m.setWorkspaceComputeManage.mockResolvedValue({ ok: true, workspace: 'ops', toggle: 'compute_manage', agentsCanManageCompute: true })
    render(<Harness />)
    const sw = await enabledSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/removing a\s+machine stay yours/)).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceComputeManage).toHaveBeenCalledWith('/w/ops', true))
    expect(m.setWorkspaceComputeManage).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true'))
    expect(useProjectsStore.getState().projects[0].agentsCanManageCompute).toBe(1)
  })

  it('off: one write with false', async () => {
    useProjectsStore.setState({ projects: [project(1)] } as never)
    m.setWorkspaceComputeManage.mockResolvedValue({ ok: true, agentsCanManageCompute: false })
    render(<Harness />)
    const sw = await enabledSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceComputeManage).toHaveBeenCalledWith('/w/ops', false))
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanManageCompute).toBe(0))
  })

  it('a refusal rolls back and shows the daemon hint', async () => {
    m.setWorkspaceComputeManage.mockRejectedValue(
      new Error(JSON.stringify({ error: { code: 'owner_only', hint: 'only the server\'s owner may turn this on or off' } })),
    )
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(await enabledSwitch())
    await screen.findByText('only the server\'s owner may turn this on or off (owner_only)')
    expect(useProjectsStore.getState().projects[0].agentsCanManageCompute).toBe(0)
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('an Admin login sees it but cannot flip it', async () => {
    role.mockResolvedValue('admin')
    render(<Harness />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect(role).toHaveBeenCalled())
    await waitFor(() => expect(sw.getAttribute('title')).toBe('Only the server\'s owner can change this'))
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(m.setWorkspaceComputeManage).not.toHaveBeenCalled()
    expect(useProjectsStore.getState().projects[0].agentsCanManageCompute).toBe(0)
  })

  it('a Member login never sees it', async () => {
    role.mockResolvedValue('member')
    render(<Harness />)
    await waitFor(() => expect(role).toHaveBeenCalled())
    await waitFor(() => expect(screen.queryByRole('switch', { name: NAME })).toBeNull())
    expect(m.setWorkspaceComputeManage).not.toHaveBeenCalled()
  })

  it('until the role is known it shows but cannot be flipped', async () => {
    role.mockResolvedValue(null)
    render(<Harness />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect(role).toHaveBeenCalled())
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(m.setWorkspaceComputeManage).not.toHaveBeenCalled()
  })
})

describe('wiring', () => {
  it('the write rides agent-access/set with toggle compute_manage', () => {
    const src = readFileSync(join(__dirname, 'compute-api.ts'), 'utf8')
    expect(src).toContain("'agent-access/set', { toggle: 'compute_manage', value, workspace }")
  })

  it('sits in Settings → Workspaces → Agents right after the operate switch, with a search entry', () => {
    const src = readFileSync(join(__dirname, 'ProjectsSection.tsx'), 'utf8')
    const operate = src.indexOf('<AgentsOperateComputeToggle project={project} />')
    const manage = src.indexOf('<AgentsManageComputeToggle project={project} />')
    expect(operate).toBeGreaterThan(-1)
    expect(manage).toBeGreaterThan(operate)
    expect(PROJECTS_MANIFEST.some((e) => e.id === 'projects.agents-can-manage-compute')).toBe(true)
  })
})
