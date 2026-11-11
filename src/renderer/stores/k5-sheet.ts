// The K5 sheet's open state (prd-setup-checklist-v1 §10.1). Opened from the
// LLM sign-in sheet's "Meet K5" and from the setup checklist's `k5_sheet`
// opener; drawn once per window by `K5Sheet` (mounted in App). The window's
// server only, like the checklist.

import { create } from 'zustand'

interface K5SheetState {
  isOpen: boolean
  open: () => void
  close: () => void
}

export const useK5SheetStore = create<K5SheetState>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
}))

export function openK5Sheet(): void {
  useK5SheetStore.getState().open()
}
