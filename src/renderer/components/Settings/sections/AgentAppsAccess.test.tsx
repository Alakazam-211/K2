// @vitest-environment jsdom
// The workspace card's App permissions (prd-agent-app-permissions-v1 rev 2,
// slice H): switch 1 "Build and run Apps", the nested switch 2 "Manage its
// Apps end to end", "Loosening the wall always asks you", and a checkbox per
// core. The owner turns things on; an Admin only off (the rest disabled,
// with the reason); a Member reads. Writes go to /cli/agent-access/set on
// the window's server, patch the store at once and roll back on a refusal;
// a change from elsewhere (the projects store re-read) shows live.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  fail: null as Error | null,
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string) => {
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (h.fail) throw h.fail
      if (body.toggle === 'app_core') {
        return { ok: true, cores: body.value ? [body.core] : [] }
      }
      return { ok: true }
    }),
    daemonCliPostQuery: vi.fn(),
  }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => h.role) }))

import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { AgentAppsAccess } from './AgentAppsAccess'
import { canSet, lockedReason, parseAppCores } from './app-access-api'

function project(over: Partial<ProjectWithWorkspaces> = {}): ProjectWithWorkspaces {
  return {
    id: 'p1',
    name: 'sales',
    path: '/ws/sales',
    color: '#3b82f6',
    iconUrl: null,
    handle: 'sales',
    workspaces: [],
    agentsCanManageSkin: 0,
    agentsCanManageAppsFull: 0,
    appCores: '[]',
    ...over,
  } as ProjectWithWorkspaces
}

function Card(): React.JSX.Element {
  const p = useProjectsStore((s) => s.projects[0])
  return <AgentAppsAccess project={p} />
}

function mount(over: Partial<ProjectWithWorkspaces> = {}): void {
  useProjectsStore.setState({ projects: [project(over)] })
  render(<Card />)
}

const sw = (id: string): HTMLButtonElement => screen.getByTestId(id) as HTMLButtonElement
const box = (core: string): HTMLInputElement => screen.getByTestId(`apps-core-${core}`) as HTMLInputElement
const stored = (): ProjectWithWorkspaces => useProjectsStore.getState().projects[0]

beforeEach(() => {
  h.posts = []
  h.fail = null
  h.role = 'owner'
})
afterEach(() => cleanup())

describe('the rules', () => {
  it('owner raises and lowers, an admin only lowers, a member and an unknown role change nothing', () => {
    expect([canSet('owner', false, true), canSet('owner', true, false)]).toEqual([true, true])
    expect([canSet('admin', false, true), canSet('admin', true, false)]).toEqual([false, true])
    expect([canSet('member', true, false), canSet(null, true, false)]).toEqual([false, false])
    expect(canSet('owner', true, true)).toBe(false)
    expect(lockedReason('admin', false)).toBe('Only the server’s owner can turn this on')
    expect(lockedReason('admin', true)).toBeNull()
    expect(lockedReason('owner', false)).toBeNull()
  })

  it('reads projects.app_cores in display order and ignores unknown cores and bad JSON', () => {
    expect(parseAppCores('["sew","db","postgres","thread"]')).toEqual(['db', 'thread', 'sew'])
    expect(parseAppCores('not json')).toEqual([])
    expect(parseAppCores(undefined)).toEqual([])
  })
})

describe('the workspace card', () => {
  it('shows both switches nested, the wall line and five cores, Mail and Sew marked for later', async () => {
    mount()
    await waitFor(() => expect(sw('apps-switch-build').disabled).toBe(false))
    expect(sw('apps-switch-build').getAttribute('aria-checked')).toBe('false')
    expect(sw('apps-switch-full').disabled).toBe(true)
    expect(sw('apps-switch-full').title).toBe('Turn on Build and run Apps first')
    expect(screen.getByTestId('apps-wall-line').textContent).toBe('Loosening the wall always asks you.')
    expect(screen.getByText('Manage its Apps end to end')).toBeTruthy()
    // Its users are their own switch, in the same box (0225).
    expect(sw('apps-switch-users').getAttribute('aria-checked')).toBe('false')
    expect(screen.getByTestId('apps-switch-users-row').textContent).toContain('Never a box login')
    for (const c of ['db', 'files', 'thread', 'mail', 'sew']) expect(box(c).checked).toBe(false)
    expect(screen.getByTestId('apps-core-mail-later').textContent).toBe('Takes effect when App mail lands')
    // Sew for Apps A1 (0.46.6): the Sew tick is enforced (the chat caps).
    expect(screen.queryByTestId('apps-core-sew-later')).toBeNull()
    expect(screen.queryByTestId('apps-core-db-later')).toBeNull()
  })

  it('the owner turns switch 1 then switch 2 on and ticks a core: one route, the store patched at once', async () => {
    mount()
    await waitFor(() => expect(sw('apps-switch-build').disabled).toBe(false))
    fireEvent.click(sw('apps-switch-build'))
    expect(stored().agentsCanManageSkin).toBe(1)
    await waitFor(() => expect(sw('apps-switch-full').disabled).toBe(false))
    fireEvent.click(sw('apps-switch-full'))
    expect(stored().agentsCanManageAppsFull).toBe(1)
    await waitFor(() => expect(box('db').disabled).toBe(false))
    fireEvent.click(box('db'))
    expect(stored().appCores).toBe('["db"]')
    await waitFor(() => expect(h.posts).toHaveLength(3))
    expect(h.posts).toEqual([
      { route: 'agent-access/set', body: { workspace: '/ws/sales', toggle: 'apps_build', value: true } },
      { route: 'agent-access/set', body: { workspace: '/ws/sales', toggle: 'apps_manage_full', value: true } },
      { route: 'agent-access/set', body: { workspace: '/ws/sales', toggle: 'app_core', core: 'db', value: true } },
    ])
  })

  it('only the owner flips "Let this agent manage its Apps’ users", both ways, on its own', async () => {
    mount({ agentsCanManageAppUsers: 0 })
    await waitFor(() => expect(sw('apps-switch-users').disabled).toBe(false))
    fireEvent.click(sw('apps-switch-users'))
    expect(stored().agentsCanManageAppUsers).toBe(1)
    await waitFor(() => expect(h.posts).toHaveLength(1))
    expect(h.posts[0]).toEqual({ route: 'agent-access/set', body: { workspace: '/ws/sales', toggle: 'app_users', value: true } })
    cleanup()
    h.role = 'admin'
    mount({ agentsCanManageAppUsers: 1 })
    await waitFor(() => expect(sw('apps-switch-users').title).toBe('Only the server’s owner can change this'))
    expect(sw('apps-switch-users').disabled).toBe(true)
  })

  it('switch 1 off takes switch 2 with it', async () => {
    mount({ agentsCanManageSkin: 1, agentsCanManageAppsFull: 1 })
    await waitFor(() => expect(sw('apps-switch-build').disabled).toBe(false))
    expect(sw('apps-switch-full').getAttribute('aria-checked')).toBe('true')
    fireEvent.click(sw('apps-switch-build'))
    expect([stored().agentsCanManageSkin, stored().agentsCanManageAppsFull]).toEqual([0, 0])
    expect(sw('apps-switch-full').getAttribute('aria-checked')).toBe('false')
    await waitFor(() => expect(h.posts).toHaveLength(1))
  })

  it('an Admin may only turn things off: what is off is disabled with the reason', async () => {
    h.role = 'admin'
    mount({ agentsCanManageSkin: 1, appCores: '["thread"]' })
    await waitFor(() => expect(screen.getByTestId('apps-access-locked').textContent).toContain('Only the server’s owner turns them on'))
    expect(sw('apps-switch-build').disabled).toBe(false)
    expect(sw('apps-switch-full').disabled).toBe(true)
    expect(sw('apps-switch-full').title).toBe('Only the server’s owner can turn this on')
    expect(box('db').disabled).toBe(true)
    expect(box('thread').disabled).toBe(false)
    fireEvent.click(box('db'))
    await act(async () => {})
    expect(h.posts).toEqual([])
    fireEvent.click(box('thread'))
    await waitFor(() =>
      expect(h.posts).toEqual([
        { route: 'agent-access/set', body: { workspace: '/ws/sales', toggle: 'app_core', core: 'thread', value: false } },
      ]),
    )
  })

  it('a Member reads: every control disabled', async () => {
    h.role = 'member'
    mount({ agentsCanManageSkin: 1, appCores: '["db"]' })
    await waitFor(() => expect(screen.getByTestId('apps-access-locked')).toBeTruthy())
    expect(sw('apps-switch-build').disabled).toBe(true)
    expect(box('db').disabled).toBe(true)
    expect(box('db').checked).toBe(true)
  })

  it('a refusal rolls the store back and shows the daemon’s words', async () => {
    h.fail = new Error(JSON.stringify({ error: { code: 'owner_only', hint: 'only the server’s owner ticks an App core' } }))
    mount({ agentsCanManageSkin: 1 })
    await waitFor(() => expect(box('files').disabled).toBe(false))
    fireEvent.click(box('files'))
    await waitFor(() => expect(screen.getByTestId('apps-access-error').textContent).toBe('only the server’s owner ticks an App core'))
    expect(stored().appCores).toBe('[]')
    expect(box('files').checked).toBe(false)
  })

  it('a change from another client (the store re-read) shows live', async () => {
    mount()
    await waitFor(() => expect(sw('apps-switch-build').disabled).toBe(false))
    act(() => {
      useProjectsStore.setState({ projects: [project({ agentsCanManageSkin: 1, appCores: '["files","thread"]' })] })
    })
    expect(sw('apps-switch-build').getAttribute('aria-checked')).toBe('true')
    expect([box('files').checked, box('thread').checked, box('db').checked]).toEqual([true, true, false])
  })
})
