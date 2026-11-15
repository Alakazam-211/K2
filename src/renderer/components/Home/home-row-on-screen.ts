// Which Home row is on screen: the one answer the nav's rows and the
// collapsed rail's icons both paint as selected.
//
// Rosson 2026-11-14: with another server's room on screen, the rail lit the
// window's own agent behind it (it read only the active workspace) and gave
// the room's icon no selected state at all, so clicking it changed nothing
// you could see. The nav already had the full rule; now both read this.
//
//   - a person or channel row: its chat is open;
//   - a Grok Bot row: its chat is open;
//   - a row on the connected server (`ws`, its workspace there): that
//     workspace is the window's active one and no other server's room
//     covers the main area (Home M4);
//   - a row on another server: its room is the one on screen (with "Open
//     other servers' agents in Home" on).
//
// A row is only ever compared with what is on screen, so a group that does
// not hold the agent on screen lights nothing, never a stale pick.

import { useProjectsStore } from '@/stores/projects'
import { useShownHomeRoom } from '@/stores/home-rooms'
import { useHomeChatStore } from '@/stores/home-chat'
import { useGrokBotsStore } from '@/stores/grokbots'
import type { HomeRow } from '@/stores/homes'
import { useRemoteRoomsPreview } from '@/lib/remote-rooms-preview'
import { grokBotIdFromAddress } from '@/lib/grokbots'
import { isHomeChatRow } from '@/lib/home-chat/address'
import { parseHomeAddress } from '@/lib/home-address'

export interface OnScreen {
  /** The window's active workspace (its own room). */
  activeProjectId: string | null
  /** The other server's room on screen, by row address; null = the window's own. */
  shown: string | null
  /** "Open other servers' agents in Home". */
  preview: boolean
  /** The open person or channel chat, by row address. */
  chat: string | null
  /** The open Grok Bot chat. */
  grok: { hostKey: string; agentId: string } | null
}

export function homeRowOnScreen(row: HomeRow, ws: { id: string } | null, now: OnScreen): boolean {
  if (isHomeChatRow(row)) return now.chat === row.address
  const grokId = grokBotIdFromAddress(row.address)
  if (grokId !== null) {
    return (
      now.grok !== null &&
      now.grok.hostKey === parseHomeAddress(row.address)?.host &&
      now.grok.agentId.toLowerCase() === grokId
    )
  }
  if (ws) return ws.id === now.activeProjectId && now.shown === null
  return now.preview && now.shown === row.address
}

/** Is this row the one on screen? `ws` is its workspace when the row is on
 *  the connected server (null otherwise). */
export function useHomeRowOnScreen(row: HomeRow, ws: { id: string } | null): boolean {
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const shown = useShownHomeRoom()
  const preview = useRemoteRoomsPreview()
  const chat = useHomeChatStore((s) => s.open?.address ?? null)
  const grok = useGrokBotsStore((s) => s.open)
  return homeRowOnScreen(row, ws, { activeProjectId, shown, preview, chat, grok })
}
