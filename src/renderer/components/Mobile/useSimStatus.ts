// The workspace's sim session as the daemon reports it (`GET /cli/sim/status`).
//
// Read while the tab is shown: often while the device is changing state
// (booting, stopping), slowly otherwise, and at once after a gesture
// (`refresh`), and at once on the daemon's `sim_state` / `sim_hold` events
// for this workspace's device. Hidden tabs do not poll.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { onSimChanged } from '@/stores/session-events'
import { fetchSimStatus, type SimStatus } from './simApi'

export const SIM_STATUS_FAST_MS = 1500
export const SIM_STATUS_SLOW_MS = 10_000

export interface SimStatusView {
  status: SimStatus | null
  error: string | null
  refresh(): void
}

export function useSimStatus(scope: ServerScope, workspace: string | null, visible: boolean): SimStatusView {
  const [status, setStatus] = useState<SimStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [tick, setTick] = useState(0)
  const refresh = useCallback(() => setTick((n) => n + 1), [])
  const statusRef = useRef<SimStatus | null>(null)

  useEffect(() => {
    if (!workspace || !visible) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | null = null
    const load = async (): Promise<void> => {
      try {
        const next = await fetchSimStatus(scope, workspace)
        if (cancelled) return
        statusRef.current = next
        setStatus(next)
        setError(null)
      } catch (err) {
        if (cancelled) return
        const msg = err instanceof Error ? err.message : String(err)
        console.warn('[mobile] sim/status:', msg)
        setError(msg)
      }
      if (cancelled) return
      const st = statusRef.current?.sim?.state
      const changing = st === 'installing' || st === 'booting' || st === 'stopping'
      timer = setTimeout(() => void load(), changing ? SIM_STATUS_FAST_MS : SIM_STATUS_SLOW_MS)
    }
    void load()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [scope, workspace, visible, tick])

  // The daemon says this device changed: read it now.
  useEffect(() => {
    if (!workspace || !visible) return
    return onSimChanged(scope, (e) => {
      const mine = statusRef.current?.sim
      if (e.workspace === workspace || (mine && (e.sim === mine.sim || e.workspace === mine.workspace))) refresh()
    })
  }, [scope, workspace, visible, refresh])

  return { status, error, refresh }
}
