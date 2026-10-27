// The Design tab's left column: the agent's conversation in Thread view
// (Rosson 2026-10-16).
//
// Its OWN Thread instance, bound to the workspace's address in the room's
// scope: the overlay Thread list (daemon-backed over the overlay
// WebSocket) plus the Thread message box. It never touches the pinned Chat
// tab's retained `AgentChatPane` (no retained-chat slot, no portal move)
// and never writes the pinned Chat's remembered view, so the Chat tab stays
// in whatever view it is in. Design notes sent "via Design" land in this
// Thread, so the person sees them appear.

import { useEffect, useMemo, useState } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { ThreadOverlayColumn } from '@/components/SessionView/ThreadOverlayColumn'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { resolvePinnedChatCopyableAddress } from '@/lib/chat-session-tab'
import { roomProjectForCwd } from '@/stores/room'

export interface DesignThreadColumnProps {
  /** The workspace's folder (the pinned Chat's project path). */
  projectPath: string
  /** The agent's name (empty state, address fallback). */
  agentName: string
  /** The pinned Chat's conversation, when known (the Thread resolves the
   *  live one from the address either way). */
  conversationId?: string | null
  /** The Design tab is shown in this window. */
  visible: boolean
}

export function DesignThreadColumn({
  projectPath,
  agentName,
  conversationId = null,
  visible,
}: DesignThreadColumnProps): React.JSX.Element {
  const room = useRoom()
  const [addr, setAddr] = useState('')
  useEffect(() => {
    let cancelled = false
    const projectId = roomProjectForCwd(room, projectPath)?.id ?? null
    resolvePinnedChatCopyableAddress(room.scope, projectPath, projectId)
      .then((a) => {
        if (!cancelled && a?.clipboard) setAddr(a.clipboard)
      })
      .catch((e: unknown) => console.warn('[design] thread address:', e))
    return () => {
      cancelled = true
    }
  }, [room, projectPath])

  // A Thread-only view chrome for the message box: it sends to this
  // Thread. Local to this column; nothing here is saved as a view.
  const chrome = useMemo<SessionViewChromeValue>(
    () => ({
      viewTab: 'thread',
      splitLeft: 'thread',
      splitRight: 'terminal',
      overlayAddr: addr,
      conversationId,
      agentName,
      displayName: agentName,
    }),
    [addr, conversationId, agentName],
  )

  return (
    <div className="h-full min-h-0 flex flex-col" data-testid="design-thread-column">
      {addr ? (
        <SessionViewChromeContext.Provider value={chrome}>
          <ThreadOverlayColumn
            addr={addr}
            conversationId={conversationId}
            active={visible}
            composeBar={
              <TerminalComposeBar
                sessionId={`design:${projectPath}`}
                workspacePath={projectPath}
                sendDestination="thread"
              />
            }
            agentName={agentName}
          />
        </SessionViewChromeContext.Provider>
      ) : (
        <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
          Opening the Thread…
        </div>
      )}
    </div>
  )
}
