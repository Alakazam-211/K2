import { createContext, useContext } from 'react'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'
import type { ThreadPanelKind } from './threadPanels'

/** Chrome around an agent session: the view menu picks the compose send destination. */
export interface SessionViewChromeValue {
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  overlayAddr: string
  conversationId: string | null
  /** v2 agent name. The daemon resolves cwd. Not a transcript path. */
  agentName: string
  /** Human name for the agent (AGENT.md display name or sidecar handle). */
  displayName?: string
  /** Thread view's revealed terminal (the conversation's remembered state).
   *  Undefined: this chrome has no reveal (the Design tab's Thread). */
  terminalRevealed?: boolean
  /** The memory `terminalRevealed` was read from. A change of it (another
   *  conversation) never animates. */
  terminalRevealKey?: string | null
  /** A sidecar's header has no phase of its own to read: the pane reports
   *  whether it has a live terminal. */
  reportTerminalLive?: (live: boolean) => void
  /** Thread view's open slide-out panel (`threadPanels.ts`). Undefined: this
   *  chrome has no panels. */
  threadPanel?: ThreadPanelKind | null
  setThreadPanel?: (next: ThreadPanelKind | null) => void
  /** The workspace's path: the Heartbeats panel reads its fires by it. */
  projectPath?: string
}

export const SessionViewChromeContext = createContext<SessionViewChromeValue | null>(null)

export function useSessionViewChrome(): SessionViewChromeValue | null {
  return useContext(SessionViewChromeContext)
}
