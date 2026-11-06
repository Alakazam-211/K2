// Whether this window shows the call's video view (VideoStage) or its
// floating tile. Per-window view state, as K2 Companion's `useCallScreen`:
// the call itself is the voice and video stores' (the app's native engine),
// never this. Not daemon state and not saved.

import { create } from 'zustand'

export interface CallScreen {
  /** The call view is open over the current page. */
  open: boolean
  /** The floating tile's body was clicked: open the call view. */
  ask(): void
  /** The call view's Hide: back to the page, the tile comes back. */
  close(): void
}

export const useCallScreen = create<CallScreen>((set) => ({
  open: false,
  ask: () => set({ open: true }),
  close: () => set({ open: false }),
}))
