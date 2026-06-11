// Coverage for the clone-to-dialog store's `openResult` action (GitHub #19
// item 4): after a successful clone, "Open on <host>" jumps into the freshly-
// cloned workspace on the still-active destination host. It must select the
// project (and dismiss the modal), fetching the project list first only if the
// post-clone refresh hasn't landed yet.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// The store imports the projects store (which has an import-time fetchProjects
// side effect), so mock it before importing the store under test. Lazy arrow
// wrappers dodge the vi.mock hoisting TDZ.
const fetchProjects = vi.fn()
const setActiveProject = vi.fn()
let projectsList: { id: string }[] = []
vi.mock('@/stores/projects', () => ({
  useProjectsStore: {
    getState: () => ({
      projects: projectsList,
      fetchProjects: (...a: unknown[]) => fetchProjects(...a),
      setActiveProject: (...a: unknown[]) => setActiveProject(...a),
    }),
  },
}))

import { useCloneToDialogStore } from './clone-to-dialog'

const RESULT = { project: { id: 'p1', name: 'ws' }, dest_path: '/home/x/ws' }

beforeEach(() => {
  fetchProjects.mockReset().mockResolvedValue(undefined)
  setActiveProject.mockReset()
  projectsList = []
  useCloneToDialogStore.getState().close()
})

describe('clone-to-dialog openResult', () => {
  it('selects the cloned project WITHOUT re-fetching when it is already listed', async () => {
    projectsList = [{ id: 'p1' }] // post-clone refresh already landed
    useCloneToDialogStore.setState({ isOpen: true, stage: 'done', result: RESULT })

    await useCloneToDialogStore.getState().openResult()

    expect(fetchProjects).not.toHaveBeenCalled()
    expect(setActiveProject).toHaveBeenCalledWith('p1')
    expect(useCloneToDialogStore.getState().isOpen).toBe(false) // modal dismissed
  })

  it('fetches first when the cloned project is not yet in the store (refresh race)', async () => {
    projectsList = [] // refresh still in flight
    useCloneToDialogStore.setState({ isOpen: true, stage: 'done', result: RESULT })

    await useCloneToDialogStore.getState().openResult()

    expect(fetchProjects).toHaveBeenCalledTimes(1)
    expect(setActiveProject).toHaveBeenCalledWith('p1')
    expect(useCloneToDialogStore.getState().isOpen).toBe(false)
  })

  it('still dismisses (but selects nothing) when the result has no project id', async () => {
    useCloneToDialogStore.setState({
      isOpen: true,
      stage: 'done',
      result: { project: {}, dest_path: '/home/x/ws' },
    })

    await useCloneToDialogStore.getState().openResult()

    expect(setActiveProject).not.toHaveBeenCalled()
    expect(useCloneToDialogStore.getState().isOpen).toBe(false)
  })
})
