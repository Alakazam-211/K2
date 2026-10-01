/**
 * Heartbeat status copy — the one formatter for when a heartbeat fires
 * next, or why it is not firing (`.k2/prds/prd-heartbeat-firing-v1.md`
 * HB26/HB27/HB30/HB31).
 *
 * The daemon decides everything: `nextFireAt` (UTC RFC3339), `waitReason`
 * (the fixed HB20 vocabulary, plus the read-time `no_ticks`),
 * `waitDetail`, `waitSince`, and `disabledReason`. This module only turns
 * them into words. There is no schedule math here (HB1): the only clock
 * work is the distance from now to a time the daemon sent.
 *
 * Used by the drawer row (`HeartbeatEntry`), Settings → Heartbeats and
 * the Wake Scheduler list. No path renders a bare "now" or "0m 0s".
 */

export interface HeartbeatWaitFields {
  enabled: boolean
  disabledReason?: string | null
  scheduleError?: string | null
  waitReason?: string | null
  waitDetail?: string | null
  waitSince?: string | null
  nextFireAt?: string | null
}

/** From `nextFireAt` to this long after it the row reads "firing…"
 *  (HB27). Past it the daemon's watchdog names the reason (HB22). */
export const FIRING_GRACE_MS = 120_000

/** One-shot refetch lands this long after the grace (HB29). The daemon's
 *  overdue / `no_ticks` checks are strict `>` 120 s, so give them a beat. */
export const REFETCH_SLACK_MS = 5_000

/** How the line should look. `next` = an upcoming fire; `firing` = due
 *  now; `waiting` = a designed wait; `problem` = needs attention;
 *  `disabled` = the user turned it off. */
export type HeartbeatStatusTone = 'next' | 'firing' | 'waiting' | 'problem' | 'disabled'

export interface HeartbeatStatus {
  text: string
  tone: HeartbeatStatusTone
  /** True when the text depends on the clock (re-render every second). */
  ticking: boolean
}

export interface HeartbeatStatusOptions {
  /** Epoch ms. */
  now: number
  /** Does the row's server send `nextFireAt` (`heartbeat-next-fire`)?
   *  When false the formatter never shows a time (HB30). */
  nextFire: boolean
}

const FIRING = '…'

/** Copy for every HB20 reason that is not time-based. */
const DISABLED_COPY: Record<string, string> = {
  failures: 'disabled: repeated failures',
  wakeup_missing: 'disabled: WAKEUP.md missing',
  wakeup_empty: 'disabled: WAKEUP.md is empty',
}

const STATIC_COPY: Record<string, { text: string; tone: HeartbeatStatusTone }> = {
  wakeup_empty: { text: 'waiting: WAKEUP.md is empty', tone: 'waiting' },
  no_agent: { text: 'waiting: no agent in this workspace', tone: 'waiting' },
  no_project: { text: 'waiting: workspace folder not found', tone: 'problem' },
}

function parseTime(iso: string | null | undefined): number | null {
  if (!iso) return null
  const t = Date.parse(iso)
  return Number.isNaN(t) ? null : t
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n)
}

/** "7:00 AM" in local time. Fixed format, so it reads the same on every
 *  locale and in tests. */
export function clockLabel(t: number): string {
  const d = new Date(t)
  let h = d.getHours()
  const ap = h >= 12 ? 'PM' : 'AM'
  if (h === 0) h = 12
  else if (h > 12) h -= 12
  return `${h}:${pad2(d.getMinutes())} ${ap}`
}

function sameLocalDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

/** "Today 7:00 AM", "Tomorrow 7:00 AM", "Mon 7:00 AM", "Oct 12 7:00 AM". */
export function calendarLabel(t: number, now: number): string {
  const when = new Date(t)
  const today = new Date(now)
  const time = clockLabel(t)
  if (sameLocalDay(when, today)) return `Today ${time}`
  const tomorrow = new Date(now)
  tomorrow.setDate(today.getDate() + 1)
  if (sameLocalDay(when, tomorrow)) return `Tomorrow ${time}`
  if (t - now < 7 * 24 * 3600_000) {
    return `${['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][when.getDay()]} ${time}`
  }
  const month = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][when.getMonth()]
  return `${month} ${when.getDate()} ${time}`
}

/** A future time: "in 45s" / "in 12m 04s" under an hour, else the
 *  calendar label. Only called with `t > now`, so never "0m 0s". */
function countdownLabel(t: number, now: number): string {
  const secs = Math.ceil((t - now) / 1000)
  if (secs < 60) return `in ${secs}s`
  if (secs < 3600) return `in ${Math.floor(secs / 60)}m ${pad2(secs % 60)}s`
  return calendarLabel(t, now)
}

/** A compact span: "45s", "6m", "8h", "3d". Rounds toward the larger
 *  unit only once it is reached, and never returns "0s" (min 1s). */
export function spanLabel(ms: number): string {
  const secs = Math.max(1, Math.round(Math.abs(ms) / 1000))
  if (secs < 60) return `${secs}s`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h`
  return `${Math.floor(hours / 24)}d`
}

/** The overdue gate (`wait_detail`, HB22) in plain words. The daemon
 *  writes `<reason>: <detail>` or `not_fired: …`. */
function gateLabel(detail: string | null | undefined): string {
  if (!detail) return 'reason unknown'
  if (detail.startsWith('no_ticks')) return 'scheduler not ticking'
  if (detail.startsWith('not_fired')) return 'scheduler ticking, slot not fired'
  return detail
}

function disabledText(row: HeartbeatWaitFields): HeartbeatStatus {
  const reason = row.waitReason
  if (reason && reason.startsWith('disabled:')) {
    const why = reason.slice('disabled:'.length)
    if (why === 'user') {
      // `disabled:user` with a detail = a system reason the vocabulary has
      // no slot for (the daemon keeps the raw value as detail).
      return row.waitDetail
        ? { text: `disabled: ${row.waitDetail}`, tone: 'problem', ticking: false }
        : { text: 'disabled', tone: 'disabled', ticking: false }
    }
    return { text: DISABLED_COPY[why] ?? `disabled: ${why}`, tone: 'problem', ticking: false }
  }
  // A server older than S3 sends `disabledReason` only.
  if (row.disabledReason) {
    return {
      text: DISABLED_COPY[row.disabledReason] ?? `disabled: ${row.disabledReason}`,
      tone: 'problem',
      ticking: false,
    }
  }
  return { text: 'disabled', tone: 'disabled', ticking: false }
}

/**
 * The status line for one heartbeat, or null when there is nothing to
 * say (an older server with a healthy row: the schedule text is enough,
 * HB30). Archived rows are the caller's to skip.
 */
export function describeHeartbeatStatus(
  row: HeartbeatWaitFields,
  opts: HeartbeatStatusOptions,
): HeartbeatStatus | null {
  const { now } = opts
  if (!row.enabled) return disabledText(row)

  const reason = row.waitReason ?? null
  const next = opts.nextFire ? parseTime(row.nextFireAt) : null

  if (reason === 'schedule_error' || (!reason && row.scheduleError)) {
    return {
      text: `invalid schedule: ${row.waitDetail || row.scheduleError || 'unknown error'}`,
      tone: 'problem',
      ticking: false,
    }
  }
  if (reason && reason.startsWith('disabled:')) return disabledText(row)
  if (reason && STATIC_COPY[reason]) return { ...STATIC_COPY[reason], ticking: false }

  switch (reason) {
    case 'in_flight':
      return { text: `firing${FIRING}`, tone: 'firing', ticking: false }
    case 'no_ticks': {
      const since = parseTime(row.waitSince)
      return {
        text:
          since === null
            ? 'waiting: scheduler not ticking'
            : `waiting: scheduler not ticking (${spanLabel(now - since)})`,
        tone: 'problem',
        ticking: since !== null,
      }
    }
    case 'overdue':
      return {
        text: next === null
          ? `overdue: ${gateLabel(row.waitDetail)}`
          : `overdue ${spanLabel(now - next)}: ${gateLabel(row.waitDetail)}`,
        tone: 'problem',
        ticking: next !== null,
      }
    case 'window_closed':
      if (next !== null && next > now) {
        const sameDay = sameLocalDay(new Date(next), new Date(now))
        return {
          text: `waiting: window opens ${sameDay ? clockLabel(next) : calendarLabel(next, now)}`,
          tone: 'waiting',
          ticking: true,
        }
      }
      if (next === null) return { text: 'waiting: firing window closed', tone: 'waiting', ticking: false }
      break
    case 'backoff':
      if (next !== null && next > now) {
        const left = spanLabel(next - now)
        return {
          text: row.waitDetail ? `retry in ${left} (${row.waitDetail})` : `retry in ${left}`,
          tone: 'waiting',
          ticking: true,
        }
      }
      if (next === null) {
        return {
          text: row.waitDetail ? `waiting to retry (${row.waitDetail})` : 'waiting to retry',
          tone: 'waiting',
          ticking: false,
        }
      }
      break
    case null:
    case 'scheduled':
      break
    default:
      // A reason this client has no copy for (a newer server). Say it.
      return {
        text: row.waitDetail ? `waiting: ${reason} (${row.waitDetail})` : `waiting: ${reason}`,
        tone: 'waiting',
        ticking: false,
      }
  }

  // Scheduled (or a timed wait whose time has come): read the clock.
  if (next === null) {
    if (!opts.nextFire) return null
    // A new server that has not filled the row yet (boot fill pending).
    return reason ? { text: 'waiting: next fire not computed yet', tone: 'waiting', ticking: false } : null
  }
  if (next > now) return { text: countdownLabel(next, now), tone: 'next', ticking: true }
  if (row.waitDetail && /catch-up/.test(row.waitDetail)) {
    return { text: 'catching up a missed run', tone: 'firing', ticking: false }
  }
  if (now - next <= FIRING_GRACE_MS) return { text: `firing${FIRING}`, tone: 'firing', ticking: true }
  // Past the grace with no reason yet: the one-shot refetch (HB29) is
  // about to bring the daemon's gate.
  return { text: `overdue ${spanLabel(now - next)}`, tone: 'problem', ticking: true }
}

/** Text only — for tests and plain-string callers. */
export function heartbeatStatusText(
  row: HeartbeatWaitFields,
  opts: HeartbeatStatusOptions,
): string | null {
  return describeHeartbeatStatus(row, opts)?.text ?? null
}

/**
 * HB29 — ms until the next one-shot refetch, or null for none. That is
 * the earliest `nextFireAt + 120 s` still in the future across enabled
 * rows. A deadline already behind `now` never re-arms, so a row that
 * stays stuck triggers exactly one refetch, not a poll.
 */
export function nextDueRefetchDelay(
  rows: ReadonlyArray<Pick<HeartbeatWaitFields, 'enabled' | 'nextFireAt'> & { archivedAt?: string | null }>,
  now: number,
): number | null {
  let best: number | null = null
  for (const r of rows) {
    if (!r.enabled || r.archivedAt) continue
    const t = parseTime(r.nextFireAt)
    if (t === null) continue
    const due = t + FIRING_GRACE_MS + REFETCH_SLACK_MS
    if (due <= now) continue
    if (best === null || due < best) best = due
  }
  // setTimeout overflows past ~24.8 days and would fire at once; a far
  // deadline re-arms after an early (harmless) refetch instead.
  return best === null ? null : Math.min(best - now, MAX_TIMER_MS)
}

/** Largest delay `setTimeout` honours (2^31 - 1 ms). */
export const MAX_TIMER_MS = 2_147_483_647
