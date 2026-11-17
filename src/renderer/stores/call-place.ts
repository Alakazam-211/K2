// Where this window shows the call, and how. A call lives in its place: a
// DM call in its conversation (the DM's view, its stage at the top and the
// messages below), a voice room (which has no view of its own) at the top
// of the page. The stage is open (the tiles and the call's controls) or
// folded into the slim in-call bar (Compact): one toggle, Compact / Expand.
// There is no floating tile, no corner and no hidden state.
//
// Per-window view state: the call itself is the voice and video stores'
// (the app's native engine), never this, and it is not daemon state.

import { useEffect } from 'react'
import { create } from 'zustand'
import { convOfDmRoom } from '@/lib/home-chat/calls'

export interface CallPlaceState {
  /** The stage is folded into the slim in-call bar. */
  compact: boolean
  /** The conversations on screen in this window that are a call's place,
   *  by call place key (how many views of each are mounted). */
  docks: Record<string, number>
}

export const useCallPlace = create<CallPlaceState>(() => ({ compact: false, docks: {} }))

/** A call's place key: its server and room. Known before the call has an
 *  id, and the same in every window. */
export function callPlaceKey(call: { hostKey: string; roomId: string }): string {
  return `${call.hostKey}|${call.roomId}`
}

/** A DM call lives in its conversation; a voice room has no view of its own. */
export function callHasConversation(call: { roomId: string }): boolean {
  return convOfDmRoom(call.roomId) !== null
}

/** Fold the stage into the slim bar (no video is drawn while folded). */
export function compactCallStage(): void {
  useCallPlace.setState({ compact: true })
}

/** Open the stage again. */
export function expandCallStage(): void {
  useCallPlace.setState({ compact: false })
}

/** While mounted: the conversation `key` (a call place key) is on screen in
 *  this window. Null: nothing. */
export function useCallDock(key: string | null): void {
  useEffect(() => {
    if (!key) return
    useCallPlace.setState((s) => ({ docks: { ...s.docks, [key]: (s.docks[key] ?? 0) + 1 } }))
    return () =>
      useCallPlace.setState((s) => {
        const n = (s.docks[key] ?? 0) - 1
        const docks = { ...s.docks }
        if (n > 0) docks[key] = n
        else delete docks[key]
        return { docks }
      })
  }, [key])
}

/** The call's conversation is on screen in this window. */
export function useCallDocked(call: { hostKey: string; roomId: string } | null): boolean {
  const key = call ? callPlaceKey(call) : null
  return useCallPlace((s) => (key ? (s.docks[key] ?? 0) > 0 : false))
}

export function __resetCallPlaceForTests(): void {
  useCallPlace.setState({ compact: false, docks: {} })
}
