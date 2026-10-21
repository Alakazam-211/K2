// @vitest-environment jsdom

// The Agent tab's "Allow this agent to manage Postgres versions"
// (`db_upgrade`, 0201): a local agent may move its own and its Apps'
// databases to a newer Postgres, create on a chosen version and roll back.
// One gesture = one agent-access write (toggle `db_upgrade`). The daemon
// lets the owner raise it and the owner or an Admin lower it, so an Admin
// sees it flippable only while it is on and a Member never sees it.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('./data-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./data-api')>()
  return { ...orig, setWorkspaceDbUpgrade: vi.fn() }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn() }))

import * as api from './data-api'
import * as presence from '@/components/Presence/PresenceKickButton'
import { AgentsManagePostgresToggle, PROJECTS_MANIFEST, dbUpgradeVisibility } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const m = vi.mocked(api)
const role = vi.mocked(presence.fetchViewerRole)
const NAME = 'Allow this agent to manage Postgres versions'

function project(on: number): ProjectWithWorkspaces {
  return { id: 'p-pg', name: 'ops', path: '/w/ops', agentsCanUpgradePostgres: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-pg'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsManagePostgresToggle project={p} />
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

describe('dbUpgradeVisibility', () => {
  it('the owner flips it both ways; an admin only lowers; a member never sees it', () => {
    expect(dbUpgradeVisibility('owner', false)).toEqual({ show: true, canFlip: true })
    expect(dbUpgradeVisibility('owner', true)).toEqual({ show: true, canFlip: true })
    expect(dbUpgradeVisibility('admin', false)).toEqual({ show: true, canFlip: false })
    expect(dbUpgradeVisibility('admin', true)).toEqual({ show: true, canFlip: true })
    expect(dbUpgradeVisibility('member', true)).toEqual({ show: false, canFlip: false })
    expect(dbUpgradeVisibility(null, true)).toEqual({ show: true, canFlip: false })
  })
})

describe('Agent tab: Allow this agent to manage Postgres versions', () => {
  it('defaults off; on is one db_upgrade write and the store follows', async () => {
    m.setWorkspaceDbUpgrade.mockResolvedValue({ ok: true, workspace: 'ops', toggle: 'db_upgrade', agentsCanUpgradePostgres: true })
    render(<Harness />)
    const sw = await enabledSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/Deleting the old copy stays with you/)).toBeTruthy()
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceDbUpgrade).toHaveBeenCalledWith('/w/ops', true))
    expect(m.setWorkspaceDbUpgrade).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true'))
    expect(useProjectsStore.getState().projects[0].agentsCanUpgradePostgres).toBe(1)
  })

  it('a refusal rolls back and shows the daemon hint', async () => {
    m.setWorkspaceDbUpgrade.mockRejectedValue(
      new Error(JSON.stringify({ error: { code: 'owner_only', hint: 'only the server\'s owner turns this on' } })),
    )
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(await enabledSwitch())
    await screen.findByText('only the server\'s owner turns this on (owner_only)')
    expect(useProjectsStore.getState().projects[0].agentsCanUpgradePostgres).toBe(0)
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('an Admin login cannot raise it', async () => {
    role.mockResolvedValue('admin')
    render(<Harness />)
    const sw = await screen.findByRole('switch', { name: NAME })
    await waitFor(() => expect(role).toHaveBeenCalled())
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(true))
    fireEvent.click(sw)
    expect(m.setWorkspaceDbUpgrade).not.toHaveBeenCalled()
    expect(useProjectsStore.getState().projects[0].agentsCanUpgradePostgres).toBe(0)
  })

  it('an Admin login lowers it: one write with false', async () => {
    role.mockResolvedValue('admin')
    useProjectsStore.setState({ projects: [project(1)] } as never)
    m.setWorkspaceDbUpgrade.mockResolvedValue({ ok: true, agentsCanUpgradePostgres: false })
    render(<Harness />)
    const sw = await enabledSwitch()
    expect(sw.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw)
    await waitFor(() => expect(m.setWorkspaceDbUpgrade).toHaveBeenCalledWith('/w/ops', false))
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanUpgradePostgres).toBe(0))
  })

  it('a Member login never sees it', async () => {
    role.mockResolvedValue('member')
    render(<Harness />)
    await waitFor(() => expect(role).toHaveBeenCalled())
    await waitFor(() => expect(screen.queryByRole('switch', { name: NAME })).toBeNull())
    expect(m.setWorkspaceDbUpgrade).not.toHaveBeenCalled()
  })
})

describe('wiring', () => {
  it('the write rides agent-access/set with toggle db_upgrade', () => {
    const src = readFileSync(join(__dirname, 'data-api.ts'), 'utf8')
    expect(src).toContain("'agent-access/set', { toggle: 'db_upgrade', value, workspace }")
  })

  it('sits in Settings → Workspaces → Agents right after the compute switch, with a search entry', () => {
    const src = readFileSync(join(__dirname, 'ProjectsSection.tsx'), 'utf8')
    const compute = src.indexOf('<AgentsManageComputeToggle project={project} />')
    const pg = src.indexOf('<AgentsManagePostgresToggle project={project} />')
    expect(compute).toBeGreaterThan(-1)
    expect(pg).toBeGreaterThan(compute)
    expect(PROJECTS_MANIFEST.some((e) => e.id === 'projects.agents-can-upgrade-postgres')).toBe(true)
  })
})
