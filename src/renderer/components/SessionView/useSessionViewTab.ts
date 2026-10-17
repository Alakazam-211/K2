import { useCallback, useEffect, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { homeHostKey } from '@/lib/host-key'
import type { ServerScope } from '@/kessel/server-scope'
import {
  type SessionSplitSides,
  type SessionViewTab,
  type SplitPaneView,
  resolveSessionView,
  sessionViewSplitStorageKey,
  sessionViewTabStorageKey,
  writeSessionSplitSides,
  writeSessionViewTab,
} from './sessionViewTab'

export interface SessionViewChoice {
  viewTab: SessionViewTab
  setViewTab: (tab: SessionViewTab) => void
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  setSplitLeft: (view: SplitPaneView) => void
  setSplitRight: (view: SplitPaneView) => void
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
 *  including both split sides. `scope` is the room's scope (`useRoom().scope`). */
export function useSessionViewTab(
  sessionKey: string | null,
  scope: Pick<ServerScope, 'isPrimary' | 'hostKey'>,
): SessionViewChoice {
  // Subscribed so the primary room re-reads after a server switch (Home M1).
  const windowHostKey = useConnectHostStore((s) => homeHostKey(s.activeHost))
  const hostKey = sessionViewHostKey(scope, windowHostKey)
  const storageKey = sessionKey ? sessionViewTabStorageKey(hostKey, sessionKey) : null
  const splitKey = sessionKey ? sessionViewSplitStorageKey(hostKey, sessionKey) : null
  // A conversation this client has never shown opens in the
  // "New agents open in" choice (Settings → General → Workspaces); one with
  // a remembered view keeps it.
  const [viewTab, setViewTabState] = useState<SessionViewTab>(
    () => resolveSessionView(storageKey, splitKey).tab,
  )
  const [sides, setSides] = useState<SessionSplitSides>(
    () => resolveSessionView(storageKey, splitKey).sides,
  )

  useEffect(() => {
    const resolved = resolveSessionView(storageKey, splitKey)
    setViewTabState(resolved.tab)
    setSides(resolved.sides)
    // Remember the first view so a later change to the setting does not
    // move a conversation the user has already seen.
    if (resolved.fresh && storageKey && splitKey) {
      writeSessionViewTab(storageKey, resolved.tab)
      writeSessionSplitSides(splitKey, resolved.sides)
    }
  }, [storageKey, splitKey])

  const setViewTab = useCallback(
    (next: SessionViewTab) => {
      setViewTabState(next)
      if (storageKey) writeSessionViewTab(storageKey, next)
    },
    [storageKey],
  )

  const setSplitLeft = useCallback(
    (left: SplitPaneView) => {
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
      setSides((prev) => {
        const next = { left: prev.left, right }
        if (splitKey) writeSessionSplitSides(splitKey, next)
        return next
      })
    },
    [splitKey],
  )

  return {
    viewTab,
    setViewTab,
    splitLeft: sides.left,
    splitRight: sides.right,
    setSplitLeft,
    setSplitRight,
  }
}
