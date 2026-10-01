import { useCallback, useEffect, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import { homeHostKey } from '@/lib/host-key'
import {
  DEFAULT_SPLIT_SIDES,
  SESSION_VIEW_TAB_DEFAULT,
  type SessionSplitSides,
  type SessionViewTab,
  type SplitPaneView,
  readSessionSplitSides,
  readSessionViewTab,
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

/** Remembered view for this window + named conversation, including both split sides. */
export function useSessionViewTab(sessionKey: string | null): SessionViewChoice {
  // The primary server's host key (Home M1): follows a server switch.
  const hostKey = useConnectHostStore((s) => homeHostKey(s.activeHost))
  const storageKey = sessionKey ? sessionViewTabStorageKey(hostKey, sessionKey) : null
  const splitKey = sessionKey ? sessionViewSplitStorageKey(hostKey, sessionKey) : null
  const [viewTab, setViewTabState] = useState<SessionViewTab>(() =>
    storageKey ? readSessionViewTab(storageKey) : SESSION_VIEW_TAB_DEFAULT,
  )
  const [sides, setSides] = useState<SessionSplitSides>(() =>
    splitKey ? readSessionSplitSides(splitKey) : DEFAULT_SPLIT_SIDES,
  )

  useEffect(() => {
    if (!storageKey || !splitKey) {
      setViewTabState(SESSION_VIEW_TAB_DEFAULT)
      setSides(DEFAULT_SPLIT_SIDES)
      return
    }
    setViewTabState(readSessionViewTab(storageKey))
    setSides(readSessionSplitSides(splitKey))
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
