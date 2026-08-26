import { createContext, useContext } from 'react'
import type { SessionViewTab } from './sessionViewTab'

/** Chrome around an agent session: tabs pick the compose send destination. */
export interface SessionViewChromeValue {
  viewTab: SessionViewTab
  overlayAddr: string
  conversationId: string | null
}

export const SessionViewChromeContext = createContext<SessionViewChromeValue | null>(null)

export function useSessionViewChrome(): SessionViewChromeValue | null {
  return useContext(SessionViewChromeContext)
}
