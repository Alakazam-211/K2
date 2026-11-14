import { create } from 'zustand'

interface CommandPaletteState {
  isOpen: boolean
  /** The text it opens with: what was typed into an entry field that
   *  opened it (the Home nav's search field); '' otherwise. */
  prefill: string
  open: (prefill?: string) => void
  close: () => void
  toggle: () => void
}

export const useCommandPaletteStore = create<CommandPaletteState>((set) => ({
  isOpen: false,
  prefill: '',
  open: (prefill = '') => set({ isOpen: true, prefill }),
  close: () => set({ isOpen: false, prefill: '' }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen, prefill: '' }))
}))
