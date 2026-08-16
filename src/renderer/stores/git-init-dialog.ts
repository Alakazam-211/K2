import { create } from 'zustand'

interface GitInitDialogState {
  isOpen: boolean
  isPending: boolean
  path: string | null
  name: string | null
  error: string | null
  seedWiki: boolean
  seedAgentsMd: boolean
  fanout: boolean

  open: (
    path: string,
    name: string,
    seedWiki?: boolean,
    seedAgentsMd?: boolean,
    fanout?: boolean,
  ) => void
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
  seedAgentsMd: true,
  fanout: false,

  open: (path: string, name: string, seedWiki = true, seedAgentsMd = true, fanout = false) =>
    set({ isOpen: true, path, name, seedWiki, seedAgentsMd, fanout, error: null, isPending: false }),

  close: () =>
    set({
      isOpen: false,
      isPending: false,
      path: null,
      name: null,
      error: null,
      seedWiki: true,
      seedAgentsMd: true,
      fanout: false,
    }),

  setIsPending: (isPending: boolean) => set({ isPending }),

  setError: (error: string) => set({ error, isPending: false })
}))
