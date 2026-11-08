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

/** Cmd+L / the Switch Server menu item. Focus a visible browser address
 *  bar if there is one, otherwise open the switcher once. Never toggle: a
 *  second event in the same turn must not close what the first opened. */
export function applyCmdL(args: { focusAddress: () => boolean }): void {
  if (!args.focusAddress()) useServerSwitcherStore.getState().setOpen(true)
}
