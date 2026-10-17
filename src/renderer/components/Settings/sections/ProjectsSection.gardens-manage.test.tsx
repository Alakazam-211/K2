// @vitest-environment jsdom

// The Agent tab's "Manage Gardens and data sources" (0181, Rosson
// 2026-10-16): off by default; one gesture = one agent-access write (toggle
// `gardens_manage`, the Owner/Admin route); the store patches
// optimistically and rolls back on a refusal.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const orig = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...orig, daemonCliPost: vi.fn() }
})

import * as cli from '@/lib/daemon-cli'
import { AgentsManageGardensToggle } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const post = vi.mocked(cli.daemonCliPost)
const NAME = 'Manage Gardens and data sources'

function project(on: number | undefined): ProjectWithWorkspaces {
  return { id: 'p-gm', name: 'gm', path: '/w/gm', agentsCanManageGardens: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-gm'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsManageGardensToggle project={p} />
}

beforeEach(() => {
  vi.clearAllMocks()
  useProjectsStore.setState({ projects: [project(0)] } as never)
})

afterEach(() => cleanup())

describe('Agent tab: Manage Gardens and data sources', () => {
  it('is off by default, including on a daemon that does not send the field yet', () => {
    useProjectsStore.setState({ projects: [project(undefined)] } as never)
    render(<Harness />)
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(/Every act is logged in Access Audit/)).toBeTruthy()
  })

  it('on: one gardens_manage write through agent-access/set; the store follows', async () => {
    post.mockResolvedValue({ ok: true, toggle: 'gardens_manage', agentsCanManageGardens: true } as never)
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: NAME }))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    const [, route, body] = post.mock.calls[0]
    expect(route).toBe('agent-access/set')
    expect(body).toEqual({ workspace: '/w/gm', toggle: 'gardens_manage', value: true })
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanManageGardens).toBe(1))
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true')
  })

  it('a refusal rolls the switch back', async () => {
    post.mockRejectedValue(new Error('403 forbidden'))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: NAME }))
    await waitFor(() => expect(spy).toHaveBeenCalled())
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanManageGardens).toBe(0))
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false')
    spy.mockRestore()
  })

  it('sits in the Agents group with a settings-search entry, never workspace/set or a refetch', () => {
    const src = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'), 'utf8')
    expect(src).toContain("id: 'projects.agents-can-manage-gardens'")
    expect(src).toContain('data-settings-id="projects.agents-can-manage-gardens"')
    expect(src.indexOf('<AgentsManageGardensToggle project={project} />')).toBeGreaterThan(
      src.indexOf('<AgentsManageProjectsToggle project={project} />'),
    )
    const start = src.indexOf('export function AgentsManageGardensToggle')
    expect(start).toBeGreaterThan(0)
    const toggle = src.slice(start, src.indexOf('// Per-workspace agents-may-manage-hosted-mail', start))
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
    expect(toggle).not.toContain('backdrop-blur')
  })
})
