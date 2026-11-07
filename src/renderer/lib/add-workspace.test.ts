// 0.46.10: the add-workspace confirm step treats the ingest as best-effort.
// A lost `agents/run-workspace-ingest` request (WebKit: "Load failed",
// logged as CORS) must not abort the add, hide the new workspace or keep
// the dialog waiting.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return { daemonCliPost: primaryOnly((...args: unknown[]) => daemonCliPost(...args)) }
})

const addProject = vi.fn()
vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ addProject }) },
}))

import { addWorkspaceWithIngest, runWorkspaceIngestBestEffort } from './add-workspace'

const ADDED = { id: 'p9', path: '/srv/new', name: 'new', workspaces: [{ id: 'w9' }] }

let warn: ReturnType<typeof vi.spyOn>

beforeEach(() => {
  daemonCliPost.mockReset()
  addProject.mockReset()
  warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
})

afterEach(() => {
  warn.mockRestore()
})

describe('add workspace + best-effort ingest', () => {
  it('a failed ingest does not abort the add: the added workspace comes back, a warning is logged', async () => {
    addProject.mockResolvedValue(ADDED)
    daemonCliPost.mockRejectedValue(new TypeError('Load failed'))

    await expect(addWorkspaceWithIngest('/srv/new', { seedWiki: true })).resolves.toBe(ADDED)
    expect(addProject).toHaveBeenCalledWith('/srv/new', { seedWiki: true })

    // The ingest ran for the added path and its failure only warned.
    await vi.waitFor(() => expect(warn).toHaveBeenCalledTimes(1))
    expect(daemonCliPost).toHaveBeenCalledWith('agents/run-workspace-ingest', { project_path: '/srv/new' })
    expect(String(warn.mock.calls[0][0])).toContain('[add-workspace] run ingest failed')
  })

  it('the add does not wait for the ingest (a hung ingest never holds the dialog)', async () => {
    addProject.mockResolvedValue(ADDED)
    daemonCliPost.mockReturnValue(new Promise(() => {}))

    await expect(addWorkspaceWithIngest('/srv/new', {})).resolves.toBe(ADDED)
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
  })

  it('no ingest when the folder needs git first (nothing was added yet)', async () => {
    addProject.mockResolvedValue(null)

    await expect(addWorkspaceWithIngest('/srv/new', {})).resolves.toBe(null)
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a failed ADD still throws (the store toasted it)', async () => {
    addProject.mockRejectedValue(new Error('workspace already exists'))

    await expect(addWorkspaceWithIngest('/srv/new', {})).rejects.toThrow('workspace already exists')
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('runWorkspaceIngestBestEffort resolves on failure and on success', async () => {
    daemonCliPost.mockRejectedValueOnce(new TypeError('Load failed'))
    await expect(runWorkspaceIngestBestEffort('/srv/a')).resolves.toBeUndefined()
    expect(warn).toHaveBeenCalledTimes(1)

    daemonCliPost.mockResolvedValueOnce({ success: true })
    await expect(runWorkspaceIngestBestEffort('/srv/b')).resolves.toBeUndefined()
    expect(warn).toHaveBeenCalledTimes(1)
  })
})
