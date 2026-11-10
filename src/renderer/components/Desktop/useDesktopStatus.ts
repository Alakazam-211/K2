// The machine's Remote desktop status (`GET /cli/desktop/status`) on a
// scope's server, read while shown: once, then slowly, and at once on the
// daemon's `desktop_state` event or a gesture (`refresh`). A server without
// `remote-desktop-v1` is never asked (`supported: false`).

import { useCallback, useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { onDesktopState } from '@/stores/session-events'
import { fetchDesktopStatus } from './desktop-api'
import { DESKTOP_FEATURE, type DesktopStatus } from './desktop-state'

export const DESKTOP_STATUS_SLOW_MS = 15_000

export interface DesktopStatusView {
  supported: boolean
  status: DesktopStatus | null
  error: string | null
  refresh(): void
}

export function useDesktopStatus(scope: ServerScope, visible: boolean): DesktopStatusView {
  const supported = scope.serverSupports(DESKTOP_FEATURE)
  const [status, setStatus] = useState<DesktopStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [tick, setTick] = useState(0)
  const refresh = useCallback(() => setTick((n) => n + 1), [])

  useEffect(() => {
    if (!supported || !visible) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | null = null
    const load = async (): Promise<void> => {
      try {
        const next = await fetchDesktopStatus(scope)
        if (cancelled) return
        setStatus(next)
        setError(null)
      } catch (err) {
        if (cancelled) return
        const msg = err instanceof Error ? err.message : String(err)
        console.warn('[desktop] desktop/status:', msg)
        setError(msg)
      }
      if (!cancelled) timer = setTimeout(() => void load(), DESKTOP_STATUS_SLOW_MS)
    }
    void load()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [scope, supported, visible, tick])

  useEffect(() => {
    if (!supported || !visible) return
    return onDesktopState(scope, () => refresh())
  }, [scope, supported, visible, refresh])

  return { supported, status, error, refresh }
}
