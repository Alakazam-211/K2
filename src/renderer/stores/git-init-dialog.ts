import { create } from 'zustand'

interface GitInitDialogState {
  isOpen: boolean
  isPending: boolean
  path: string | null
  name: string | null
  error: string | null
  seedWiki: boolean

  open: (path: string, name: string, seedWiki?: boolean) => void
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
  seedWiki: true,

  open: (path: string, name: string, seedWiki = true) =>
    set({ isOpen: true, path, name, seedWiki, error: null, isPending: false }),

  close: () =>
    set({ isOpen: false, isPending: false, path: null, name: null, error: null, seedWiki: true }),

  setIsPending: (isPending: boolean) => set({ isPending }),

  setError: (error: string) => set({ error, isPending: false })
}))
