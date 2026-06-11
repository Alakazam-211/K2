import { create } from 'zustand'

interface AddWorkspaceDialogState {
  isOpen: boolean
  isPending: boolean
  path: string | null
  error: string | null
  /** Fires when the user confirms. The caller passes in a handler that
   * completes the add (projects_add_from_path + k2so_agents_run_workspace_ingest). */
  onConfirm: (() => Promise<void>) | null

  open: (args: { path: string; onConfirm: () => Promise<void> }) => void
  close: () => void
  setIsPending: (pending: boolean) => void
  setError: (error: string | null) => void
}

export const useAddWorkspaceDialogStore = create<AddWorkspaceDialogState>((set) => ({
  isOpen: false,
  isPending: false,
  path: null,
  error: null,
  onConfirm: null,

  open: ({ path, onConfirm }) =>
    set({ isOpen: true, path, onConfirm, error: null, isPending: false }),

  close: () =>
    set({ isOpen: false, isPending: false, path: null, onConfirm: null, error: null }),

  setIsPending: (isPending: boolean) => set({ isPending }),

  setError: (error: string | null) => set({ error, isPending: false })
}))
