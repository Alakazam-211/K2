// Shared open state for the top-bar ServerSwitcher so Cmd+L can open
// it without a mouse click. One boolean — every mounted switcher
// (gate, main chrome, Focus, Settings) follows the same dropdown.

import { create } from 'zustand'

interface ServerSwitcherState {
  open: boolean
  setOpen: (open: boolean) => void
  toggle: () => void
}

export const useServerSwitcherStore = create<ServerSwitcherState>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
  toggle: () => set((s) => ({ open: !s.open })),
}))
