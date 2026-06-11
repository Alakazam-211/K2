import { create } from 'zustand'

interface RunningAgentsState {
  isOpen: boolean
  open: () => void
  close: () => void
  toggle: () => void
}

export const useRunningAgentsStore = create<RunningAgentsState>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
}))
