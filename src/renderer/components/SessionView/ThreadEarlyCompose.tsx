import { useMemo, type JSX } from 'react'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT } from './sessionViewTab'
import { SessionViewChromeContext, type SessionViewChromeValue } from './sessionViewChrome'

/**
 * The Thread's message box while the agent's session is still starting
 * (the pinned Chat's ensure, an agent tile still checking its session):
 * there is no terminal pane yet to draw it, so the Thread column draws this
 * one. A Thread post goes through the daemon's Thread route, which needs
 * the address and not a live session, so it sends as normal.
 *
 * Same draft as the terminal pane's box: `agentName` is the pane's v2 agent
 * name (TerminalPane's `attachAgentName`, the workspace id here), passed as
 * the bar's `draftId`, so its key is the pane's `agent:<draftId>` from the
 * first keystroke. When the session comes up the text, files, caret and
 * focus carry over (composeHandoff.ts); a refresh keeps them as before.
 */
export function ThreadEarlyCompose({
  addr,
  conversationId,
  agentName,
  projectPath,
  displayName,
}: {
  addr: string
  conversationId: string | null
  /** The v2 agent name the ready pane attaches as (its `attachAgentName`). */
  agentName: string
  projectPath: string
  displayName?: string
}): JSX.Element {
  const chrome = useMemo<SessionViewChromeValue>(
    () => ({
      viewTab: 'thread',
      splitLeft: DEFAULT_SPLIT_LEFT,
      splitRight: DEFAULT_SPLIT_RIGHT,
      overlayAddr: addr,
      conversationId,
      agentName,
      displayName,
      projectPath,
    }),
    [addr, conversationId, agentName, displayName, projectPath],
  )
  return (
    <SessionViewChromeContext.Provider value={chrome}>
      <TerminalComposeBar sessionId="" draftId={agentName} workspacePath={projectPath} sendDestination="thread" />
    </SessionViewChromeContext.Provider>
  )
}
