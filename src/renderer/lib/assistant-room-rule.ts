// The Workspace Assistant (Cmd+Shift+L) and rooms on other servers.
//
// Rosson, answer Q4 (2026-09-30): the assistant follows the server
// switcher. It acts only on Home agents that live on the window's connected
// server — this computer when the switcher is on This computer, server X
// when it is on X. In a room for any other server it still CHATS, but it
// refuses actions (tabs, splits, terminals, documents, merges) and says
// why. This is the rule, not an M4 stopgap (prd-home-multi-server-client
// MS22).
//
// Pure: no stores. The callers (`lib/workspace-ops-router.ts` for the
// `workspace:*` Tauri ops, `AssistantBar`'s tool-call dispatch) pass the
// focused room's server and the window's server.

export interface AssistantServer {
  /** Host key (`local`, `dtl.k2.dev`, `ip:port`). */
  hostKey: string
  /** Label for copy (`This computer`, the saved server's label). */
  label: string
}

/** May the assistant act on a room on `room`, from a window on `window`? */
export function assistantMayActOnServer(room: Pick<AssistantServer, 'hostKey'>, window: Pick<AssistantServer, 'hostKey'>): boolean {
  return room.hostKey === window.hostKey
}

/**
 * The refusal the assistant gives for an action in a room on another
 * server, or null when it may act. It explains why and what would let it.
 */
export function assistantActionRefusal(room: AssistantServer, window: AssistantServer): string | null {
  if (assistantMayActOnServer(room, window)) return null
  return (
    `This agent is on ${room.label}. The assistant only changes tabs and terminals ` +
    `for agents on ${window.label}, the server this window is connected to. ` +
    `Switch this window to ${room.label} to let it act here. You can still chat with it.`
  )
}
