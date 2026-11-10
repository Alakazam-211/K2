// Which screen share this window shows in the middle (lib/voice/call-share.ts
// has the rule). Per-window view state, kept per call on this client too, so
// popping the call out or back in keeps the same share in the middle. Never
// daemon state: the call is the voice and video stores'.

import { create } from 'zustand'
import { readSharePick, writeSharePick } from '@/lib/voice/call-view'

export const useCallShare = create<{ picked: string | null }>(() => ({ picked: null }))

/** Put this share in the middle (null: back to the rule's own choice). */
export function pickShare(callId: string | null, key: string | null): void {
  useCallShare.setState({ picked: key })
  writeSharePick(callId, key)
}

/** Take up the pick another window of this client made for this call. */
export function loadSharePick(callId: string | null): void {
  useCallShare.setState({ picked: readSharePick(callId) })
}

export function __resetCallShareForTests(): void {
  useCallShare.setState({ picked: null })
}
