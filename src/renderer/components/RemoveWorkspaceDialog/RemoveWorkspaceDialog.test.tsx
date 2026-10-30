// @vitest-environment jsdom
// Remove Workspace (0.46.7, App-governed Spaces S7 follow-up): a workspace
// that maintains Apps shows them before anything changes; "Uninstall its
// Apps and remove" uninstalls them in one all-or-nothing call; if people
// would lose access the dialog lists every App and what it'd lose, and
// "Remove anyway" repeats the call with dropAccess, then removes. Never a
// raw refusal, never a silent failure.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  calls: [] as string[],
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  apps: [] as unknown[],
  release: [] as unknown[],
  removeError: null as unknown,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    h.calls.push(`invoke ${cmd}`)
    return []
  }),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.calls.push(`GET ${route} ${JSON.stringify(params)}`)
      if (route !== 'app/list') throw new Error(`unexpected GET ${route}`)
      return { ok: true, apps: h.apps }
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.calls.push(`POST ${route}`)
      h.posts.push({ route, body })
      if (route === 'app/release') {
        const next = h.release.shift()
        if (next) throw next
        return { ok: true }
      }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('../../stores/projects', () => ({
  useProjectsStore: (sel: (s: { removeProject: (id: string) => Promise<void> }) => unknown) =>
    sel({
      removeProject: async (id: string) => {
        h.calls.push(`remove ${id}`)
        if (h.removeError) throw h.removeError
      },
    }),
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useRemoveWorkspaceDialogStore } from '../../stores/remove-workspace-dialog'
import RemoveWorkspaceDialog from './RemoveWorkspaceDialog'

beforeEach(() => {
  h.calls = []
  h.posts = []
  h.apps = []
  h.release = []
  h.removeError = null
  useRemoveWorkspaceDialogStore.getState().open({ projectId: 'ws-sales', projectName: 'Sales', projectPath: '/ws/sales' })
})
afterEach(() => {
  cleanup()
  useRemoveWorkspaceDialogStore.getState().close()
})

describe('Remove Workspace with Apps', () => {
  it('no Apps: removes and closes, as before', async () => {
    render(<RemoveWorkspaceDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Remove workspace' }))
    await waitFor(() => expect(useRemoveWorkspaceDialogStore.getState().isOpen).toBe(false))
    expect(h.calls).toEqual(['GET app/list {"workspace":"ws-sales"}', 'remove ws-sales'])
  })

  it('Apps that would drop access: lists them, then "Remove anyway" uninstalls with dropAccess and removes', async () => {
    h.apps = [{ appId: 'jobs' }, { appId: 'crm' }]
    h.release = [
      new DaemonHttpError('nothing was released: …', 409, 'uninstall_drops_access', {
        code: 'uninstall_drops_access',
        released: [],
        apps: [{ app: 'jobs', wouldDrop: { logins: 40, spaces: 1 } }],
      }),
    ]
    render(<RemoveWorkspaceDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Remove workspace' }))
    const step = await screen.findByTestId('remove-workspace-apps-step')
    expect(step.textContent).toContain('jobs, crm')
    expect(h.calls).toEqual(['GET app/list {"workspace":"ws-sales"}'])

    fireEvent.click(screen.getByRole('button', { name: 'Uninstall its Apps and remove' }))
    await waitFor(() => expect(within(screen.getByTestId('remove-workspace-apps-step')).queryAllByRole('listitem')).toHaveLength(1))
    expect(screen.getByTestId('remove-workspace-apps-step').textContent).toContain('jobs: 40 App logins, 1 chat Space bound to it')
    expect(screen.queryByText(/uninstall_drops_access/)).toBeNull()
    expect(h.posts).toEqual([{ route: 'app/release', body: { workspace: 'ws-sales', apps: 'uninstall', forRemoval: true } }])
    expect(h.calls).not.toContain('remove ws-sales')

    fireEvent.click(screen.getByRole('button', { name: 'Remove anyway' }))
    await waitFor(() => expect(useRemoveWorkspaceDialogStore.getState().isOpen).toBe(false))
    expect(h.posts[1]).toEqual({ route: 'app/release', body: { workspace: 'ws-sales', apps: 'uninstall', forRemoval: true, dropAccess: true } })
    expect(h.calls.slice(-2)).toEqual(['POST app/release', 'remove ws-sales'])
  })

  it('a teardown mode never runs before the Apps are dealt with', async () => {
    h.apps = [{ appId: 'jobs' }]
    render(<RemoveWorkspaceDialog />)
    fireEvent.click(screen.getByText('Keep current context'))
    fireEvent.click(screen.getByRole('button', { name: 'Freeze context & remove' }))
    await screen.findByTestId('remove-workspace-apps-step')
    expect(h.calls.some((c) => c.startsWith('invoke'))).toBe(false)
  })

  it('any other refusal shows its message, and the dialog stays open', async () => {
    h.apps = [{ appId: 'jobs' }]
    h.release = [new DaemonHttpError('Sales is the Point of Contact for: Q4. Reassign the PoC first.', 409, 'poc_of_projects', null)]
    render(<RemoveWorkspaceDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Remove workspace' }))
    await screen.findByTestId('remove-workspace-apps-step')
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall its Apps and remove' }))
    await screen.findByText('Sales is the Point of Contact for: Q4. Reassign the PoC first.')
    expect(useRemoveWorkspaceDialogStore.getState().isOpen).toBe(true)
    expect(h.calls).not.toContain('remove ws-sales')
  })

  it('a removal refused for Apps (the list was empty or unreadable) shows the Apps step, not an error', async () => {
    h.removeError = new DaemonHttpError('Sales maintains an App…', 409, 'workspace_has_apps', {
      code: 'workspace_has_apps',
      apps: [{ app: 'jobs', state: 'running' }],
    })
    render(<RemoveWorkspaceDialog />)
    fireEvent.click(screen.getByRole('button', { name: 'Remove workspace' }))
    const step = await screen.findByTestId('remove-workspace-apps-step')
    expect(step.textContent).toContain('jobs')
    expect(useRemoveWorkspaceDialogStore.getState().error).toBeNull()
  })
})
