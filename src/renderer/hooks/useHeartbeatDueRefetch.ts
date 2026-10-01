import { useEffect, useRef } from 'react'
import { nextDueRefetchDelay, type HeartbeatWaitFields } from '@/lib/heartbeat-wait'

/**
 * Heartbeat S4 (HB29) for the Settings lists: one refetch when the
 * earliest row's `nextFireAt + 120 s` passes, so a stale list picks up
 * the daemon's reason (or the fire that already happened). Not a poll:
 * re-armed only when the rows change, and a deadline already behind us
 * never re-arms. The drawer's per-room store does the same internally.
 */
export function useHeartbeatDueRefetch(
  rows: ReadonlyArray<Pick<HeartbeatWaitFields, 'enabled' | 'nextFireAt'> & { archivedAt?: string | null }>,
  refetch: () => void,
  enabled: boolean,
): void {
  const refetchRef = useRef(refetch)
  refetchRef.current = refetch
  useEffect(() => {
    if (!enabled) return
    const delay = nextDueRefetchDelay(rows, Date.now())
    if (delay === null) return
    const id = setTimeout(() => refetchRef.current(), delay)
    return () => clearTimeout(id)
  }, [rows, enabled])
}
