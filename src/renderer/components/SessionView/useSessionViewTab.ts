import { useCallback, useEffect, useRef, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { homeHostKey } from '@/lib/host-key'
import type { ServerScope } from '@/kessel/server-scope'
import {
  type SessionSplitSides,
  type SessionViewTab,
  type SplitPaneView,
  readSessionTerminalRevealed,
  resolveSessionView,
  sessionViewSplitStorageKey,
  sessionViewTabStorageKey,
  sessionViewTerminalStorageKey,
  writeSessionSplitSides,
  writeSessionTerminalRevealed,
  writeSessionViewTab,
} from './sessionViewTab'
import type { ThreadPanelKind } from './threadPanels'
import { sessionViewRequestFor, useSessionViewRequestStore } from '@/stores/session-view-request'

export interface SessionViewChoice {
  viewTab: SessionViewTab
  setViewTab: (tab: SessionViewTab) => void
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  setSplitLeft: (view: SplitPaneView) => void
  setSplitRight: (view: SplitPaneView) => void
  /** Thread view's terminal is revealed in this conversation. */
  terminalRevealed: boolean
  setTerminalRevealed: (next: boolean) => void
  /** Which conversation's memory `terminalRevealed` is. */
  terminalRevealKey: string | null
  /** Thread view's open slide-out panel (Chatter or Heartbeats), or null.
   *  Not remembered: a new window or conversation opens with none. Opening
   *  one hides the terminal, and revealing the terminal closes it. */
  threadPanel: ThreadPanelKind | null
  setThreadPanel: (next: ThreadPanelKind | null) => void
}

/** Which server's view memory a room reads and writes: the ROOM's server.
 *  The primary room follows the window's server (`windowHostKey`, so a
 *  server switch re-reads); a room pinned from Home uses its own server's
 *  host key. Agent A on server X therefore shares one entry whether it was
 *  opened from the server switcher or from Home. */
export function sessionViewHostKey(
  scope: Pick<ServerScope, 'isPrimary' | 'hostKey'>,
  windowHostKey: string,
): string {
  return scope.isPrimary ? windowHostKey : scope.hostKey
}

/** Remembered view for this client + the room's server + named conversation,
 *  including both split sides. `scope` is the room's scope (`useRoom().scope`).
 *  `pinnedProjectId`: the workspace whose pinned Chat this is; given, an
 *  opener's one-shot view ask for it (`stores/session-view-request.ts`, the
 *  window's server only) wins over the remembered view once.
 *
 *  `carryOnRekey`: the key is this pane's conversation, which can be named
 *  late or change under the same pane (an asleep agent woken, a sidecar's
 *  conversation adopted, a respawn). Once the person has changed the view
 *  here (the view, a Split side, the terminal peek, a panel), a new key
 *  keeps what is on screen and remembers it for that conversation, instead
 *  of reading that conversation's memory: one click on the terminal opens
 *  it and it stays open through the wake. Off for a deliberate change of
 *  conversation (the session dropdown), which reads its own memory. A view
 *  nobody touched still reads the new key's memory, as before. */
export function useSessionViewTab(
  sessionKey: string | null,
  scope: Pick<ServerScope, 'isPrimary' | 'hostKey'>,
  pinnedProjectId?: string | null,
  opts?: { carryOnRekey?: boolean },
): SessionViewChoice {
  // Subscribed so the primary room re-reads after a server switch (Home M1).
  const windowHostKey = useConnectHostStore((s) => homeHostKey(s.activeHost))
  const hostKey = sessionViewHostKey(scope, windowHostKey)
  const storageKey = sessionKey ? sessionViewTabStorageKey(hostKey, sessionKey) : null
  const splitKey = sessionKey ? sessionViewSplitStorageKey(hostKey, sessionKey) : null
  const terminalKey = sessionKey ? sessionViewTerminalStorageKey(hostKey, sessionKey) : null
  // A conversation this client has never shown opens in the
  // "New agents open in" choice (Settings → Workspaces / Agents → Workspace settings); one with
  // a remembered view keeps it.
  const [viewTab, setViewTabState] = useState<SessionViewTab>(
    () => resolveSessionView(storageKey, splitKey).tab,
  )
  const [sides, setSides] = useState<SessionSplitSides>(
    () => resolveSessionView(storageKey, splitKey).sides,
  )

  // The person changed the view in this pane (see `carryOnRekey`).
  const touchedRef = useRef(false)
  // The key the memos below were last keyed to, and whether its change
  // carries the view over (the effect below writes it for the new key).
  const lastHostRef = useRef(hostKey)
  const carryPendingRef = useRef(false)
  const shownRef = useRef({ tab: viewTab, sides })
  shownRef.current = { tab: viewTab, sides }

  // Keyed during render, so a change of conversation lands in one commit
  // with its own memory (and never reads as a toggle: no animation).
  const [revealMemo, setRevealMemo] = useState(() => ({
    key: terminalKey,
    revealed: readSessionTerminalRevealed(terminalKey),
  }))
  // The open panel belongs to this conversation too (keyed the same way).
  const [panelMemo, setPanelMemo] = useState<{ key: string | null; panel: ThreadPanelKind | null }>(() => ({
    key: terminalKey,
    panel: null,
  }))
  if (revealMemo.key !== terminalKey) {
    // Same server, same pane, a view the person set: it stays on screen.
    const carry = opts?.carryOnRekey === true && touchedRef.current && lastHostRef.current === hostKey && terminalKey !== null
    carryPendingRef.current = carry
    lastHostRef.current = hostKey
    setRevealMemo({ key: terminalKey, revealed: carry ? revealMemo.revealed : readSessionTerminalRevealed(terminalKey) })
    setPanelMemo({ key: terminalKey, panel: carry ? panelMemo.panel : null })
  } else if (panelMemo.key !== terminalKey) {
    setPanelMemo({ key: terminalKey, panel: null })
  }
  const revealedRef = useRef(revealMemo.revealed)
  revealedRef.current = revealMemo.revealed

  useEffect(() => {
    if (carryPendingRef.current) {
      // Carried over: what is shown is now this conversation's memory.
      carryPendingRef.current = false
      if (storageKey) writeSessionViewTab(storageKey, shownRef.current.tab)
      if (splitKey) writeSessionSplitSides(splitKey, shownRef.current.sides)
      if (terminalKey) writeSessionTerminalRevealed(terminalKey, revealedRef.current)
      return
    }
    const resolved = resolveSessionView(storageKey, splitKey)
    setViewTabState(resolved.tab)
    setSides(resolved.sides)
    // Remember the first view so a later change to the setting does not
    // move a conversation the user has already seen. A saved Chat view
    // (retired) was read as the Thread; write that back so `chat` is gone.
    if ((resolved.fresh || resolved.migrated) && storageKey && splitKey) {
      writeSessionViewTab(storageKey, resolved.tab)
      writeSessionSplitSides(splitKey, resolved.sides)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey, splitKey])

  // An opener asked for this workspace's Chat in a set view (Kessel opens in
  // the Thread). Declared after the read above, so in the commit where the
  // conversation becomes known it lands last. Shown at once; taken and
  // remembered once there is a conversation to remember it for.
  const viewRequest = useSessionViewRequestStore((s) =>
    scope.isPrimary ? sessionViewRequestFor(s.pending, pinnedProjectId) : null,
  )
  useEffect(() => {
    if (!viewRequest) return
    setViewTabState(viewRequest.view)
    if (storageKey) {
      writeSessionViewTab(storageKey, viewRequest.view)
      useSessionViewRequestStore.getState().clear(viewRequest.projectId)
    }
  }, [viewRequest, storageKey])

  const setViewTab = useCallback(
    (next: SessionViewTab) => {
      touchedRef.current = true
      setViewTabState(next)
      if (storageKey) writeSessionViewTab(storageKey, next)
      // Split view has no drawers (it picks its sides instead): an open
      // Chatter or Heartbeats panel closes rather than wait for Thread view.
      if (next === 'split') setPanelMemo((prev) => (prev.panel ? { ...prev, panel: null } : prev))
    },
    [storageKey],
  )

  const setSplitLeft = useCallback(
    (left: SplitPaneView) => {
      touchedRef.current = true
      setSides((prev) => {
        const next = { left, right: prev.right }
        if (splitKey) writeSessionSplitSides(splitKey, next)
        return next
      })
    },
    [splitKey],
  )

  const setSplitRight = useCallback(
    (right: SplitPaneView) => {
      touchedRef.current = true
      setSides((prev) => {
        const next = { left: prev.left, right }
        if (splitKey) writeSessionSplitSides(splitKey, next)
        return next
      })
    },
    [splitKey],
  )

  const setTerminalRevealed = useCallback(
    (revealed: boolean) => {
      touchedRef.current = true
      setRevealMemo({ key: terminalKey, revealed })
      if (terminalKey) writeSessionTerminalRevealed(terminalKey, revealed)
      // One drawer at a time.
      if (revealed) setPanelMemo({ key: terminalKey, panel: null })
    },
    [terminalKey],
  )

  const terminalRevealedNow = revealMemo.revealed
  const setThreadPanel = useCallback(
    (panel: ThreadPanelKind | null) => {
      touchedRef.current = true
      setPanelMemo({ key: terminalKey, panel })
      if (panel && terminalRevealedNow) {
        setRevealMemo({ key: terminalKey, revealed: false })
        if (terminalKey) writeSessionTerminalRevealed(terminalKey, false)
      }
    },
    [terminalKey, terminalRevealedNow],
  )

  return {
    viewTab,
    setViewTab,
    splitLeft: sides.left,
    splitRight: sides.right,
    setSplitLeft,
    setSplitRight,
    terminalRevealed: revealMemo.revealed,
    setTerminalRevealed,
    terminalRevealKey: terminalKey,
    threadPanel: panelMemo.panel,
    setThreadPanel,
  }
}
