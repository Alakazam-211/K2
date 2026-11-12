// The desktops this person may open on a scope's server
// (`GET /cli/desktop/list`, prd-linux-virtual-desktop-v1 §6): read while
// shown, and again on the daemon's `desktop_state` event. Only a server
// that lists `virtual-desktop-v1` is asked; elsewhere there is one desktop
// (This server) and no picker.

import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { onDesktopState } from '@/stores/session-events'
import { fetchDesktopList } from './desktop-api'
import type { DesktopListEntry } from './desktop-state'

export interface DesktopListView {
  /** null until the first answer (or when not asked). */
  list: DesktopListEntry[] | null
  /** The last read failed (the list stays as it was). */
  failed: boolean
}

export function useDesktopList(scope: ServerScope, visible: boolean, enabled: boolean): DesktopListView {
  const [list, setList] = useState<DesktopListEntry[] | null>(null)
  const [failed, setFailed] = useState(false)
  const [tick, setTick] = useState(0)

  useEffect(() => {
    if (!enabled || !visible) return
    let cancelled = false
    fetchDesktopList(scope).then(
      (next) => {
        if (cancelled) return
        setList(next)
        setFailed(false)
      },
      (err: unknown) => {
        if (cancelled) return
        console.warn('[desktop] desktop/list:', err instanceof Error ? err.message : String(err))
        setFailed(true)
      },
    )
    return () => {
      cancelled = true
    }
  }, [scope, enabled, visible, tick])

  useEffect(() => {
    if (!enabled || !visible) return
    return onDesktopState(scope, () => setTick((n) => n + 1))
  }, [scope, enabled, visible])

  return { list, failed }
}
