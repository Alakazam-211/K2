import { createContext, useContext } from 'react'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

/** Chrome around an agent session: the view menu picks the compose send destination. */
export interface SessionViewChromeValue {
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  overlayAddr: string
  conversationId: string | null
  /** Provider conversation id for the chat face. Never the PTY uuid. */
  chatConversationId: string | null
  /** `claude` | `codex` | `grok` | `gemini`, or null when Chat is not v1. */
  chatProvider: string | null
  /** v2 agent name. The daemon resolves cwd. Not a transcript path. */
  agentName: string
}

export const SessionViewChromeContext = createContext<SessionViewChromeValue | null>(null)

export function useSessionViewChrome(): SessionViewChromeValue | null {
  return useContext(SessionViewChromeContext)
}
