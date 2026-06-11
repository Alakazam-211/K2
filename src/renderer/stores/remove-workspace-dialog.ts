import { create } from 'zustand'

export type RemoveWorkspaceMode = 'keep_current' | 'restore_original' | 'deregister_only'

export interface RemoveWorkspaceResult {
  action: string
  path: string
  note: string
}

interface RemoveWorkspaceDialogState {
  isOpen: boolean
  isPending: boolean
  projectId: string | null
  projectName: string | null
  projectPath: string | null
  results: RemoveWorkspaceResult[]
  error: string | null

  open: (args: { projectId: string; projectName: string; projectPath: string }) => void
  close: () => void
  setIsPending: (pending: boolean) => void
  setResults: (results: RemoveWorkspaceResult[]) => void
  setError: (error: string) => void
}

export const useRemoveWorkspaceDialogStore = create<RemoveWorkspaceDialogState>((set) => ({
  isOpen: false,
  isPending: false,
  projectId: null,
  projectName: null,
  projectPath: null,
  results: [],
  error: null,

  open: ({ projectId, projectName, projectPath }) =>
    set({
      isOpen: true,
      projectId,
      projectName,
      projectPath,
      results: [],
      error: null,
      isPending: false,
    }),

  close: () =>
    set({
      isOpen: false,
      isPending: false,
      projectId: null,
      projectName: null,
      projectPath: null,
      results: [],
      error: null,
    }),

  setIsPending: (isPending: boolean) => set({ isPending }),
  setResults: (results: RemoveWorkspaceResult[]) => set({ results }),
  setError: (error: string) => set({ error, isPending: false }),
}))
