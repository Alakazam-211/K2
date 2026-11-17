// How this window shows the call's stage: open (the tiles and the call's
// controls), or folded into the slim in-call bar (Compact). One toggle,
// Compact / Expand; there is no floating tile, no corner and no hidden state.
// Per-window view state: the call itself is the voice and video stores'
// (the app's native engine), never this, and it is not daemon state.

import { create } from 'zustand'

export interface CallPlaceState {
  /** The stage is folded into the slim in-call bar. */
  compact: boolean
}

export const useCallPlace = create<CallPlaceState>(() => ({ compact: false }))

/** Fold the stage into the slim bar (no video is drawn while folded). */
export function compactCallStage(): void {
  useCallPlace.setState({ compact: true })
}

/** Open the stage again. */
export function expandCallStage(): void {
  useCallPlace.setState({ compact: false })
}

export function __resetCallPlaceForTests(): void {
  useCallPlace.setState({ compact: false })
}
