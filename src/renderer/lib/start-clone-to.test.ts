// Coverage for the post-clone project refresh (GitHub #19 item 1 / #18): once
// a clone unpacks + registers on the remote daemon, the renderer's project
// list (already pointed at that host) must re-fetch so the cloned workspace
// appears WITHOUT a manual window reload — and must NOT re-fetch on failure.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const fetchProjects = vi.fn()
vi.mock('@/stores/projects', () => ({
  useProjectsStore: {
    getState: () => ({
      projects: [],
      fetchProjects: (...a: unknown[]) => fetchProjects(...a),
      setActiveProject: vi.fn(),
    }),
  },
}))

const cloneWorkspaceTo = vi.fn()
vi.mock('./clone-to', () => ({
  cloneWorkspaceTo: (...a: unknown[]) => cloneWorkspaceTo(...a),
  defaultCloneDeps: () => ({}),
}))

import { startCloneTo } from './start-clone-to'
import { useCloneToDialogStore } from '@/stores/clone-to-dialog'
import type { ConnectHost } from '@/stores/connect-host'

const HOST = { id: 'h1', label: 'Hetzner box', hostname: '1.2.3.4' } as unknown as ConnectHost

// Let the async runClone chain (cloneWorkspaceTo → fetchProjects, both mocked
// resolved) settle: a macrotask fires after the microtask queue drains.
const flush = (): Promise<void> => new Promise((r) => setTimeout(r, 0))

beforeEach(() => {
  fetchProjects.mockReset().mockResolvedValue(undefined)
  cloneWorkspaceTo.mockReset().mockResolvedValue({ project: { id: 'p1' }, dest_path: '/x' })
  useCloneToDialogStore.getState().close()
})

describe('startCloneTo → runClone post-clone refresh', () => {
  it('refreshes the project list after a successful clone (workspace appears without reload)', async () => {
    startCloneTo('/local/ws', 'ws', HOST)
    const onConfirm = useCloneToDialogStore.getState().onConfirm
    expect(onConfirm).toBeTruthy()

    onConfirm?.(true, true)
    await flush()

    expect(cloneWorkspaceTo).toHaveBeenCalledTimes(1)
    expect(fetchProjects).toHaveBeenCalledTimes(1)
  })

  it('does NOT refresh when the clone fails', async () => {
    cloneWorkspaceTo.mockRejectedValueOnce(new Error('bundle failed'))
    startCloneTo('/local/ws', 'ws', HOST)
    const onConfirm = useCloneToDialogStore.getState().onConfirm

    onConfirm?.(true, true)
    await flush()

    expect(fetchProjects).not.toHaveBeenCalled()
  })
})
