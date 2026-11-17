// A call's conversation, to go back to it from elsewhere in the app ("Back to
// call", Expand): a DM call's room is `dm:<conversation>`; the person on the
// other side is whoever Home's chat state says that conversation is with
// (the server's summary, or the DMs this window has opened). A voice room has
// no conversation.

import { convOfDmRoom } from '@/lib/home-chat/calls'
import { personAddress } from '@/lib/home-chat/address'
import { openHomeRow } from '@/lib/home-open'
import { useHomeChatStore } from '@/stores/home-chat'
import { callHasConversation, expandCallStage, useCallDocked } from '@/stores/call-place'
import type { VoiceCall } from '@/stores/voice-core'

/** The person row address of a DM call's conversation (`<ext>::<host>`),
 *  or null: not a DM, or this window doesn't know whose DM it is yet. */
export function useCallConversationAddress(call: { hostKey: string; roomId: string } | null): string | null {
  const conv = call ? convOfDmRoom(call.roomId) : null
  const host = useHomeChatStore((s) => (call ? s.byHost[call.hostKey] : undefined))
  if (!call || !conv || !host) return null
  const ext = host.summary?.convs.find((c) => c.conv === conv)?.with ?? Object.entries(host.dms).find(([, c]) => c === conv)?.[0] ?? null
  return ext ? personAddress(ext, call.hostKey) : null
}

/** Back to call: open the call's DM in this window, with its stage open. */
export function openCallConversation(address: string, label: string): void {
  expandCallStage()
  openHomeRow({ kind: 'person', address, workspaceId: null, label })
}

/** Expand, where this window can place the stage: a voice room's at the top
 *  of the page, a DM call's in its conversation (opened first when it isn't
 *  on screen). Null: a DM this window can't find, so never a dead button. */
export function useExpandCall(call: VoiceCall): (() => void) | null {
  const docked = useCallDocked(call)
  const address = useCallConversationAddress(call)
  if (!callHasConversation(call) || docked) return expandCallStage
  if (address) return () => openCallConversation(address, call.roomName)
  return null
}
