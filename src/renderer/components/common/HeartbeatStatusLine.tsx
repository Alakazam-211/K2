import { useEffect, useState } from 'react'
import {
  describeHeartbeatStatus,
  type HeartbeatStatusTone,
  type HeartbeatWaitFields,
} from '@/lib/heartbeat-wait'

/**
 * One heartbeat's "next fire / why it is waiting" line, rendered from the
 * daemon's `nextFireAt` + `waitReason` through the shared formatter
 * (prd-heartbeat-firing-v1 HB26/HB27/HB31). Shared by the drawer row,
 * Settings → Heartbeats and the Wake Scheduler list.
 *
 * Re-renders once a second only while the text depends on the clock (a
 * countdown, "firing…", an overdue span). That tick is for the text
 * only; it never fetches (HB29).
 */
const TONE_CLASS: Record<HeartbeatStatusTone, string> = {
  next: 'text-[var(--color-text-muted)]',
  firing: 'text-[var(--color-accent)]',
  waiting: 'text-[var(--color-status-warn-amber)]',
  problem: 'text-[var(--color-status-error-soft)]',
  disabled: 'text-[var(--color-text-muted)]',
}

export function HeartbeatStatusLine({
  row,
  nextFire,
  className,
  nextPrefix = 'Next run: ',
}: {
  row: HeartbeatWaitFields
  /** The row's server sends `nextFireAt` (`heartbeat-next-fire`). */
  nextFire: boolean
  className?: string
  /** Prefix for an upcoming fire ("Next run: in 12m 04s"). */
  nextPrefix?: string
}): React.JSX.Element | null {
  // The clock is read on every render (a fresh row never paints against
  // a stale `now`); the state bump only schedules the next render.
  const [, setTick] = useState(0)
  const status = describeHeartbeatStatus(row, { now: Date.now(), nextFire })
  const ticking = status?.ticking ?? false

  useEffect(() => {
    if (!ticking) return
    const id = setInterval(() => setTick((t) => t + 1), 1000)
    return () => clearInterval(id)
  }, [ticking])

  if (!status) return null
  const text = status.tone === 'next' ? `${nextPrefix}${status.text}` : status.text
  return (
    <div
      className={`${className ?? ''} ${TONE_CLASS[status.tone]}`}
      title={row.waitDetail ? `${text} — ${row.waitDetail}` : text}
      data-heartbeat-status={status.tone}
    >
      {text}
    </div>
  )
}
