import { createContext, useContext, useLayoutEffect, useMemo, useState, type ReactNode } from 'react'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { shouldShowTerminalComposeBar } from '@/components/Terminal/terminalCompose'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'
import { ChatterOverlayPane } from './ChatterOverlayPane'
import { SplitSessionColumns } from './SplitSessionColumns'
import { ThreadDrawerPanel } from './ThreadDrawerPanel'
import { useThreadTextCapPx } from './useThreadTextCap'
import { useTerminalRevealPresence } from './threadReveal'
import { threadDrawerOpen, type ThreadDrawerKind } from './threadPanels'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT, overlayViewer, type SessionViewTab, type SplitPaneView } from './sessionViewTab'
import type { SessionViewChromeValue } from './sessionViewChrome'

/**
 * An agent session's columns: the terminal, and beside it the Thread (and
 * Chatter, and Thread view's slide-out drawers), as the view asks.
 *
 * The Thread doesn't need the terminal: the session runs headless in the
 * daemon and a Thread post goes through the daemon's Thread route. So a
 * session body (`PinnedSessionBody`) draws these columns itself, around
 * the terminal pane it is given, and a respawn (refresh, switch, continue)
 * rebuilds only the pane: the Thread, its scroll and its message box stay.
 * The pane reports what the Thread's box and Stop need from the PTY
 * (`TerminalThreadHandle`) through `SessionColumnsContext`.
 *
 * A TerminalPane with a session chrome but no body around it draws these
 * columns around its own grid, as it always has.
 */

/** What the view shows, and Thread view's drawer (its terminal reveal and
 *  panels). The terminal pane reads it too: its painter, paint pause and
 *  focus follow whether its column is on screen. */
export interface SessionColumnsState {
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  showSplit: boolean
  showThread: boolean
  showChatter: boolean
  hidePty: boolean
  showThreadOnly: boolean
  showChatterOnly: boolean
  /** The drawer the person asked for, or null. */
  drawerOpen: ThreadDrawerKind | null
  /** The drawer laid out: the last one open while it slides shut. */
  drawerShown: ThreadDrawerKind
  drawerLaidOut: boolean
  /** The terminal's column is the drawer, and laid out. */
  revealShown: boolean
  onRevealClosed: () => void
}

export function useSessionColumnsState(chrome: SessionViewChromeValue | null): SessionColumnsState {
  const viewTab = chrome?.viewTab ?? 'terminal'
  const splitLeft = chrome?.splitLeft ?? DEFAULT_SPLIT_LEFT
  const splitRight = chrome?.splitRight ?? DEFAULT_SPLIT_RIGHT
  const { thread, chatter, hidePty } = overlayViewer(viewTab, { left: splitLeft, right: splitRight })
  const showThreadOnly = viewTab === 'thread'
  // Thread view's revealed terminal (`threadReveal.ts`). `drawerOpen` is what
  // the person asked for; the column stays laid out while it slides shut.
  // Keyed during render so the commit that closes it still has it on screen.
  // Chatter and Heartbeats (`threadPanels.ts`) slide out of the same column.
  const drawerOpen = showThreadOnly
    ? threadDrawerOpen(!!chrome?.terminalRevealed, chrome?.threadPanel ?? null)
    : null
  const { shown: drawerLaidOut, onClosed: onRevealClosed } = useTerminalRevealPresence(drawerOpen !== null, showThreadOnly)
  const [lastDrawer, setLastDrawer] = useState<ThreadDrawerKind>(drawerOpen ?? 'terminal')
  if (drawerOpen && drawerOpen !== lastDrawer) setLastDrawer(drawerOpen)
  const drawerShown: ThreadDrawerKind = drawerOpen ?? lastDrawer
  // One object while nothing in it changes: the pane reads it from context.
  return useMemo(
    () => ({
      viewTab,
      splitLeft,
      splitRight,
      showSplit: viewTab === 'split',
      showThread: thread,
      showChatter: chatter,
      hidePty,
      showThreadOnly,
      showChatterOnly: viewTab === 'chatter',
      drawerOpen,
      drawerShown,
      drawerLaidOut,
      revealShown: drawerLaidOut && drawerShown === 'terminal',
      onRevealClosed,
    }),
    [viewTab, splitLeft, splitRight, thread, chatter, hidePty, showThreadOnly, drawerOpen, drawerShown, drawerLaidOut, onRevealClosed],
  )
}

/** What the Thread's message box and Stop need from the terminal pane. */
export interface TerminalThreadHandle {
  /** The PTY's session id ('' before one is known). */
  sessionId: string
  /** The pane's phase (`idle`, `ready`, `exited`, …). */
  phaseKind: string
  /** Into this PTY (Esc and Ctrl+C from the box, the strip's Stop). */
  sendInput: (data: string) => void
  /** A view-only room never types: no Stop. */
  readOnly: boolean
  showComposeBar: boolean
}

interface SessionColumnsHost {
  state: SessionColumnsState
  setHandle: (next: TerminalThreadHandle | null) => void
}

/** Set by a session body that draws the columns: the terminal pane inside
 *  then draws only its grid, and reports its handle here. */
export const SessionColumnsContext = createContext<SessionColumnsHost | null>(null)

export function useSessionColumnsHost(): SessionColumnsHost | null {
  return useContext(SessionColumnsContext)
}

/** The terminal pane's side: report `handle` to the body around it, and
 *  take it back on unmount (a respawn's new pane reports its own in the
 *  same commit). */
export function useReportTerminalThreadHandle(host: SessionColumnsHost | null, handle: TerminalThreadHandle): void {
  // `setHandle` is a state setter: stable for the body's life.
  const setHandle = host?.setHandle
  useLayoutEffect(() => {
    setHandle?.(handle)
  }, [setHandle, handle])
  useLayoutEffect(() => {
    if (!setHandle) return
    return () => setHandle(null)
  }, [setHandle])
}

/** A session body's side: the columns' state and the pane's handle, and the
 *  context value the pane reads. */
export function useSessionColumnsHostValue(chrome: SessionViewChromeValue): {
  state: SessionColumnsState
  handle: TerminalThreadHandle | null
  host: SessionColumnsHost
} {
  const state = useSessionColumnsState(chrome)
  const [handle, setHandle] = useState<TerminalThreadHandle | null>(null)
  const host = useMemo<SessionColumnsHost>(() => ({ state, setHandle }), [state])
  return { state, handle, host }
}

export function SessionColumns({
  chrome,
  state,
  terminal,
  handle,
  draftId,
  cwd,
}: {
  chrome: SessionViewChromeValue | null
  state: SessionColumnsState
  /** The terminal pane (or its grid): the first column, never remounted by a view change. */
  terminal: ReactNode
  /** The pane's PTY side; null while no pane has reported (between a
   *  respawn's panes): the box stays, with nothing to send Esc into. */
  handle: TerminalThreadHandle | null
  /** The message box's draft: the pane's agent, not the PTY. */
  draftId: string
  cwd: string
}): React.JSX.Element {
  const threadTextCap = useThreadTextCapPx()
  const sendInput = handle?.sendInput
  const readOnly = handle?.readOnly ?? false
  // The Thread working strip's Stop (prd-daemon-activity-and-thread-working-
  // v1 S7): the same Esc the box sends, into this PTY. A view-only room never
  // types, so it gets no Stop.
  const stop = useMemo(() => (sendInput && !readOnly ? (): void => sendInput('\x1b') : undefined), [sendInput, readOnly])
  const composeShown =
    (handle?.showComposeBar ?? true) && shouldShowTerminalComposeBar({ kind: handle?.phaseKind ?? 'idle' })
  const sessionId = handle?.sessionId ?? ''
  const compose = (destination: 'thread' | 'pty'): ReactNode =>
    composeShown ? (
      <TerminalComposeBar
        sessionId={sessionId}
        workspacePath={cwd}
        onInjectInput={sendInput}
        draftId={draftId}
        sendDestination={destination}
      />
    ) : null
  const { showSplit, hidePty, splitLeft, splitRight, showThreadOnly, drawerShown, drawerOpen, drawerLaidOut } = state
  return (
    <SplitSessionColumns
      showSplit={showSplit}
      hidePty={hidePty}
      left={splitLeft}
      right={splitRight}
      reveal={
        showThreadOnly && chrome?.terminalRevealed !== undefined
          ? {
              kind: drawerShown,
              open: drawerOpen !== null,
              shown: drawerLaidOut,
              motionKey: chrome.terminalRevealKey ?? null,
              onClosed: state.onRevealClosed,
              ...(chrome.revealThreadMinPx !== undefined ? { threadMinPx: chrome.revealThreadMinPx } : {}),
              ...(chrome.revealDrawerMinPx !== undefined ? { drawerMinPx: chrome.revealDrawerMinPx } : {}),
            }
          : null
      }
      panel={
        drawerLaidOut && drawerShown !== 'terminal' && chrome?.threadPanel !== undefined ? (
          <ThreadDrawerPanel kind={drawerShown} active={drawerOpen === drawerShown} />
        ) : null
      }
      terminal={terminal}
      sharedTerminal={
        <div className="flex-1 min-w-0 min-h-0 flex flex-col" data-testid="split-terminal-shared">
          <div className="flex-1 min-h-0" />
          {compose('pty')}
        </div>
      }
      renderView={(view: Exclude<SplitPaneView, 'terminal'>) => {
        if (!chrome) return null
        if (view === 'thread') {
          return (
            <ThreadOverlayColumn
              addr={chrome.overlayAddr}
              conversationId={chrome.conversationId}
              active
              composeBar={compose('thread')}
              agentName={chrome.displayName || chrome.overlayAddr}
              centreEmpty
              onStop={stop}
            />
          )
        }
        return (
          <div className="flex-1 min-w-0 min-h-0 flex flex-col" data-testid="agent-session-chatter">
            <div className="flex-1 min-h-0 min-w-0">
              <ChatterOverlayPane addr={chrome.overlayAddr} conversationId={chrome.conversationId} active />
            </div>
          </div>
        )
      }}
      single={
        <>
          {chrome && state.showThread && (
            <ThreadOverlayColumn
              addr={chrome.overlayAddr}
              conversationId={chrome.conversationId}
              active={state.showThread}
              composeBar={compose('thread')}
              agentName={chrome.displayName || chrome.overlayAddr}
              centreEmpty
              onStop={stop}
              textCapPx={threadTextCap}
            />
          )}
          {chrome && state.showChatter && (
            <div className="flex-1 min-w-0 min-h-0 flex flex-col" data-testid="agent-session-chatter">
              <div className="flex-1 min-h-0 min-w-0">
                <ChatterOverlayPane
                  addr={chrome.overlayAddr}
                  conversationId={chrome.conversationId}
                  active={state.showChatter}
                  textCapPx={threadTextCap}
                />
              </div>
            </div>
          )}
        </>
      }
    />
  )
}
