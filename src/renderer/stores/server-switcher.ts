// Shared open state for the top-bar ServerSwitcher so Cmd+L can open
// it without a mouse click. One boolean — every mounted switcher
// (gate, main chrome, Focus, Settings) follows the same dropdown.

import { create } from 'zustand'

interface ServerSwitcherState {
  open: boolean
  /** Where keyboard focus was when the menu opened; Esc puts it back. */
  returnFocus: HTMLElement | null
  setOpen: (open: boolean) => void
  toggle: () => void
}

/** The focused element at the moment the menu opens (null when nothing
 *  real is focused). Read before the menu's search field takes focus. */
function focusedNow(): HTMLElement | null {
  if (typeof document === 'undefined') return null
  const el = document.activeElement
  return el instanceof HTMLElement && el !== document.body ? el : null
}

export const useServerSwitcherStore = create<ServerSwitcherState>((set) => ({
  open: false,
  returnFocus: null,
  setOpen: (open) =>
    set((s) => (open && !s.open ? { open, returnFocus: focusedNow() } : { open })),
  toggle: () =>
    set((s) => (s.open ? { open: false } : { open: true, returnFocus: focusedNow() })),
}))

/** Esc: close the menu and put focus back where it was when it opened,
 *  if that element is still on the page. */
export function closeServerSwitcherRestoringFocus(): void {
  const { returnFocus } = useServerSwitcherStore.getState()
  useServerSwitcherStore.setState({ open: false, returnFocus: null })
  if (returnFocus?.isConnected) returnFocus.focus()
}

/** Cmd+L / the Switch Server menu item. Focus a visible browser address
 *  bar if there is one, otherwise open the switcher once. Never toggle: a
 *  second event in the same turn must not close what the first opened. */
export function applyCmdL(args: { focusAddress: () => boolean }): void {
  if (!args.focusAddress()) useServerSwitcherStore.getState().setOpen(true)
}
