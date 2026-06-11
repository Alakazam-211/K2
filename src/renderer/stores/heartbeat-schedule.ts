import { create } from 'zustand'

interface HeartbeatScheduleState {
  isOpen: boolean
  projectId: string | null
  open: (projectId: string) => void
  close: () => void
}

export const useHeartbeatScheduleStore = create<HeartbeatScheduleState>((set) => ({
  isOpen: false,
  projectId: null,
  open: (projectId: string) => set({ isOpen: true, projectId }),
  close: () => set({ isOpen: false, projectId: null }),
}))
