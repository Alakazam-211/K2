/**
 * Heartbeat wait copy — the one formatter for why a heartbeat is not
 * firing (`.k2/prds/prd-heartbeat-firing-v1.md` HB26/HB27/HB31).
 *
 * The daemon decides the reason (`waitReason` / `waitDetail` on
 * `heartbeat/list` and `heartbeat/list-all`, plus `disabledReason`).
 * This module only turns it into words. No schedule math here (HB1).
 *
 * S5 ships the creation-trap reasons: `wakeup_empty` and
 * `schedule_error`, plus the `disabled:*` copy. S4 extends the table
 * with the rest of the HB20 vocabulary and the countdown.
 */

export interface HeartbeatWaitFields {
  enabled: boolean
  disabledReason?: string | null
  scheduleError?: string | null
  waitReason?: string | null
  waitDetail?: string | null
}

const DISABLED_COPY: Record<string, string> = {
  failures: 'disabled: repeated failures',
  wakeup_missing: 'disabled: WAKEUP.md missing',
  wakeup_empty: 'disabled: WAKEUP.md is empty',
}

/** Copy for the daemon's wait state, or null when nothing is in the way. */
export function describeHeartbeatWait(row: HeartbeatWaitFields): string | null {
  if (!row.enabled) {
    return row.disabledReason ? (DISABLED_COPY[row.disabledReason] ?? `disabled: ${row.disabledReason}`) : null
  }
  if (row.waitReason === 'wakeup_empty') return 'waiting: WAKEUP.md is empty'
  if (row.waitReason === 'schedule_error') {
    return `invalid schedule: ${row.waitDetail || row.scheduleError || 'unknown error'}`
  }
  // A daemon older than S5 sends no waitReason but does store the error.
  if (row.scheduleError) return `invalid schedule: ${row.scheduleError}`
  return null
}
