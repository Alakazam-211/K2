// How this window shows the call, by what it carries:
// - audio only (nobody's camera, no share): the top bar's slim bar only;
// - video (anyone's camera or a share): the floating tile, in its corner;
// - the stage in the call's place, once the person chose Expand: a DM
//   call's at the top of its conversation, a voice room's (which has no view
//   of its own) at the top of the page. The choice holds through the video
//   stopping and starting again, until Float or the call ends.
// Pop out (the call's own window) is stores/call-popout.ts.
//
// Per-window view state: the call itself is the voice and video stores'
// (the app's native engine), never this, and it is not daemon state.

import { useEffect } from 'react'
import { create } from 'zustand'
import { convOfDmRoom } from '@/lib/home-chat/calls'

/** `auto`: the call follows what it carries (the tile with video, the top
 *  bar without); `stage`: the person chose Expand. */
export type CallChoice = 'auto' | 'stage'

export interface CallPlaceState {
  choice: CallChoice
  /** The conversations on screen in this window that are a call's place,
   *  by call place key (how many views of each are mounted). */
  docks: Record<string, number>
}

export const useCallPlace = create<CallPlaceState>(() => ({ choice: 'auto', docks: {} }))

/** A call's place key: its server and room. Known before the call has an
 *  id, and the same in every window. */
export function callPlaceKey(call: { hostKey: string; roomId: string }): string {
  return `${call.hostKey}|${call.roomId}`
}

/** A DM call lives in its conversation; a voice room has no view of its own. */
export function callHasConversation(call: { roomId: string }): boolean {
  return convOfDmRoom(call.roomId) !== null
}

/** Expand: the stage in the call's place, kept until Float or the call ends. */
export function expandCallStage(): void {
  useCallPlace.setState({ choice: 'stage' })
}

/** Float (and every call's start): the call follows what it carries again. */
export function floatCall(): void {
  useCallPlace.setState({ choice: 'auto' })
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
  useCallPlace.setState({ choice: 'auto', docks: {} })
}
