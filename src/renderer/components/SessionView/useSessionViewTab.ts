import { useCallback, useEffect, useState } from 'react'
import { useConnectHostStore, activeHostKey } from '@/stores/connect-host'
import {
  SESSION_VIEW_TAB_DEFAULT,
  type SessionViewTab,
  readSessionViewTab,
  sessionViewTabStorageKey,
  writeSessionViewTab,
} from './sessionViewTab'

/** Remembered Thread vs Terminal for this window + named conversation (C8). */
export function useSessionViewTab(sessionKey: string | null): [SessionViewTab, (tab: SessionViewTab) => void] {
  const hostKey = useConnectHostStore((s) => activeHostKey(s.activeHost))
  const storageKey = sessionKey
    ? sessionViewTabStorageKey(hostKey, sessionKey)
    : null
  const [tab, setTab] = useState<SessionViewTab>(() =>
    storageKey ? readSessionViewTab(storageKey) : SESSION_VIEW_TAB_DEFAULT,
  )

  useEffect(() => {
    if (!storageKey) {
      setTab(SESSION_VIEW_TAB_DEFAULT)
      return
    }
    setTab(readSessionViewTab(storageKey))
  }, [storageKey])

  const onChange = useCallback(
    (next: SessionViewTab) => {
      setTab(next)
      if (storageKey) writeSessionViewTab(storageKey, next)
    },
    [storageKey],
  )

  return [tab, onChange]
}
