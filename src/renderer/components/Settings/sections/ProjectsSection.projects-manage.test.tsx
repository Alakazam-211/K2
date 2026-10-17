// @vitest-environment jsdom

// The Agent tab's "Manage K2 projects" (0162, Rosson 2026-10-14): projects
// are open by default, with a per-agent switch. One gesture = one
// agent-access write (toggle `projects_manage`, Owner/Admin route); the
// store patches optimistically and rolls back on a refusal.

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
import { AgentsManageProjectsToggle } from './ProjectsSection'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'

const post = vi.mocked(cli.daemonCliPost)
const NAME = 'Manage K2 projects'

function project(on: number | undefined): ProjectWithWorkspaces {
  return { id: 'p-pm', name: 'pm', path: '/w/pm', agentsCanManageProjects: on, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function Harness(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects.find((x) => x.id === 'p-pm'))
  if (!p) throw new Error('project missing from the store')
  return <AgentsManageProjectsToggle project={p} />
}

beforeEach(() => {
  vi.clearAllMocks()
  useProjectsStore.setState({ projects: [project(1)] } as never)
})

afterEach(() => cleanup())

describe('Agent tab: Manage K2 projects', () => {
  it('is on by default, including on a daemon that does not send the field yet', () => {
    useProjectsStore.setState({ projects: [project(undefined)] } as never)
    render(<Harness />)
    const sw = screen.getByRole('switch', { name: NAME })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(/On by default/)).toBeTruthy()
    expect(screen.getByText(/it can still read projects and post in\s+its project chat/)).toBeTruthy()
  })

  it('off: one projects_manage write through agent-access/set; the store follows', async () => {
    post.mockResolvedValue({ ok: true, toggle: 'projects_manage', agentsCanManageProjects: false } as never)
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: NAME }))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    const [, route, body] = post.mock.calls[0]
    expect(route).toBe('agent-access/set')
    expect(body).toEqual({ workspace: '/w/pm', toggle: 'projects_manage', value: false })
    await waitFor(() => expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('false'))
    expect(useProjectsStore.getState().projects[0].agentsCanManageProjects).toBe(0)
  })

  it('on again: one write with true', async () => {
    useProjectsStore.setState({ projects: [project(0)] } as never)
    post.mockResolvedValue({ ok: true } as never)
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: NAME }))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0][2]).toEqual({ workspace: '/w/pm', toggle: 'projects_manage', value: true })
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanManageProjects).toBe(1))
  })

  it('a refusal rolls the switch back', async () => {
    post.mockRejectedValue(new Error('403 forbidden'))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Harness />)
    fireEvent.click(screen.getByRole('switch', { name: NAME }))
    await waitFor(() => expect(spy).toHaveBeenCalled())
    await waitFor(() => expect(useProjectsStore.getState().projects[0].agentsCanManageProjects).toBe(1))
    expect(screen.getByRole('switch', { name: NAME }).getAttribute('aria-checked')).toBe('true')
    spy.mockRestore()
  })

  it('sits in the Agents group with a settings-search entry, never workspace/set or a refetch', () => {
    const src = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'ProjectsSection.tsx'), 'utf8')
    expect(src).toContain("id: 'projects.agents-can-manage-projects'")
    expect(src).toContain('data-settings-id="projects.agents-can-manage-projects"')
    expect(src.indexOf('<AgentsManageProjectsToggle project={project} />')).toBeGreaterThan(
      src.indexOf('<AgentsOperateComputeToggle project={project} />'),
    )
    // Slice to this component's own closing brace, not the next component's
    // comment: MailAgentSendControl's comment ("no fetchProjects() here")
    // sits between them.
    const start = src.indexOf('export function AgentsManageProjectsToggle(')
    expect(start).toBeGreaterThan(0)
    const end = src.indexOf('\n}\n', start)
    expect(end).toBeGreaterThan(start)
    const toggle = src.slice(start, end + 2)
    expect(toggle).toContain("toggle: 'projects_manage'")
    expect(toggle).not.toContain('fetchProjects(')
    expect(toggle).not.toContain('workspace/set')
  })
})
