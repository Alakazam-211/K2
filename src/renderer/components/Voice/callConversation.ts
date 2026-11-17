// A call's conversation, to go back to it from elsewhere in the app ("Back to
// call"): a DM call's room is `dm:<conversation>`; the person on the other
// side is whoever Home's chat state says that conversation is with (the
// server's summary, or the DMs this window has opened). A voice room has no
// conversation.

import { convOfDmRoom } from '@/lib/home-chat/calls'
import { personAddress } from '@/lib/home-chat/address'
import { openHomeRow } from '@/lib/home-open'
import { useHomeChatStore } from '@/stores/home-chat'
import { expandCallStage } from '@/stores/call-place'

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
