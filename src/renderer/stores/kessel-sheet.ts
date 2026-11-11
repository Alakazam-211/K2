// The Kessel sheet's open state (prd-setup-checklist-v1 §10.1). Opened from the
// LLM sign-in sheet's "Meet Kessel" and from the setup checklist's `kessel_sheet`
// opener; drawn once per window by `KesselSheet` (mounted in App). The window's
// server only, like the checklist.

import { create } from 'zustand'

interface KesselSheetState {
  isOpen: boolean
  open: () => void
  close: () => void
}

export const useKesselSheetStore = create<KesselSheetState>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
}))

export function openKesselSheet(): void {
  useKesselSheetStore.getState().open()
}
