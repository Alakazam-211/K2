// Names for a DM call's people, from Home's people list. A DM call's room
// (`dm:<conversation>`) is on no voice rooms list, so the voice controller
// can't name its people from there; Home's people know every person on the
// server by their Sew member id, and the summary knows whose DM a
// conversation is. Pure: the caller passes the host's chat state.

import { convOfDmRoom } from './calls'
import type { HomeChatPeople, HomeChatSummary } from './api'

export interface CallNamesSource {
  people: HomeChatPeople | null
  summary: HomeChatSummary | null
  /** Person (external id) → their DM's conversation, once known here. */
  dms: Record<string, string>
}

/** The display name of the person with Sew member id `memberId`, or null. */
export function callPersonName(src: CallNamesSource | undefined, memberId: string): string | null {
  return src?.people?.people.find((p) => p.memberId === memberId)?.displayName ?? null
}

/** A DM call's name: the other person's display name, or null when its room
 *  isn't a DM's or the person isn't known (yet). */
export function dmCallRoomName(src: CallNamesSource | undefined, roomId: string): string | null {
  const conv = convOfDmRoom(roomId)
  if (!conv || !src) return null
  const ext = src.summary?.convs.find((c) => c.conv === conv)?.with ?? Object.entries(src.dms).find(([, c]) => c === conv)?.[0] ?? null
  if (!ext) return null
  return src.people?.people.find((p) => p.ext === ext)?.displayName ?? null
}
