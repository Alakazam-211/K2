import { create } from 'zustand'

interface GitInitDialogState {
  isOpen: boolean
  isPending: boolean
  path: string | null
  name: string | null
  error: string | null

  open: (path: string, name: string) => void
  close: () => void
  setIsPending: (pending: boolean) => void
  setError: (error: string) => void
}

export const useGitInitDialogStore = create<GitInitDialogState>((set) => ({
  isOpen: false,
  isPending: false,
  path: null,
  name: null,
  error: null,

  open: (path: string, name: string) =>
    set({ isOpen: true, path, name, error: null, isPending: false }),

  close: () =>
    set({ isOpen: false, isPending: false, path: null, name: null, error: null }),

  setIsPending: (isPending: boolean) => set({ isPending }),

  setError: (error: string) => set({ error, isPending: false })
}))
