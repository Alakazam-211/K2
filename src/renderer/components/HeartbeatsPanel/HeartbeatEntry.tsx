import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import {
  type HeartbeatEntry,
  useHeartbeatSessionsStore,
} from '@/stores/heartbeat-sessions'
import { useTabsStore } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'

/**
 * One row in the Workspace panel's Heartbeats section.
 *
 * Layout (single line):
 *   [indicator]  <name>   <Daily 9 AM>           [Launch]
 *
 * Indicators (squares, not circles, per K2 status convention):
 *   - 'live'       : braille spinner (animated)
 *   - 'resumable'  : filled square
 *   - 'scheduled'  : hollow square
 *   - 'archived'   : muted hollow square
 *
 * Click semantics:
 *   - Click row body → openHeartbeatTab (focus live, spawn-and-resume
 *     otherwise) — connects the user to the actual chat session.
 *   - Click `Launch` → k2so_heartbeat_force_fire (spawns a fresh fire
 *     using the heartbeat's WAKEUP.md, regardless of whether a live
 *     session exists). The agent-lock check still prevents
 *     double-spawn against an already-running session.
 */
export function HeartbeatEntryRow({
  entry,
  projectPath,
}: {
  entry: HeartbeatEntry
  projectPath: string
}): React.JSX.Element {
  const openHeartbeatTab = useTabsStore((s) => s.openHeartbeatTab)
  const [busy, setBusy] = useState(false)

  // 1Hz re-render so the "Next run: in Xs" countdown ticks smoothly.
  // describeNextRun reads `new Date()` on every render and the
  // computation is cheap (one JSON.parse + arithmetic), so doing this
  // once a second per row is well within the per-frame budget. The
  // useState bump is intentionally discarded — we only need the
  // re-render side effect.
  const [, setTick] = useState(0)
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), 1000)
    return () => clearInterval(id)
  }, [])

  const handleClick = (): void => {
    if (!projectPath) {
      console.warn('[heartbeats-panel] click ignored — projectPath missing')
      return
    }
    // The store handles all four states:
    //  - live      : focus existing tab via existingTerminalId match
    //  - resumable : build_launch reads agent_heartbeats.last_session_id
    //  - scheduled : build_launch finds no resume target, spawns fresh
    //  - archived  : build_launch resumes if the saved session still exists
    openHeartbeatTab(projectPath, entry.row.name, {
      existingTerminalId: entry.liveTerminalId ?? undefined,
    }).catch((err) => {
      console.warn('[heartbeats-panel] openHeartbeatTab failed:', err)
    })
  }

  const handleLaunch = useCallback(async (e: React.MouseEvent) => {
    // Stop the row click from also firing — Launch is its own action.
    e.stopPropagation()
    if (busy || !projectPath) return
    setBusy(true)
    const toast = useToastStore.getState()
    try {
      // Single thin invoke into the daemon. The smart-launch decision
      // tree (fresh-fire / inject-into-live / resume-and-fire) lives
      // in `crates/k2so-daemon/src/heartbeat_launch.rs` so the cron
      // tick, the CLI, and this Launch button all converge on the
      // same path. Tauri being open is no longer a precondition.
      const resp = await invoke<string>('k2so_heartbeat_smart_launch', {
        projectPath,
        name: entry.row.name,
      })
      // Daemon returns a JSON string mirroring the heartbeat_fires
      // audit decision; surface a friendly toast based on the
      // branch it took.
      // 0.37.8: when the heartbeat has `use_workspace_session = true`,
      // the branch is tagged `workspace_session:<inner>` (e.g.
      // `workspace_session:fresh_fire`) — the inner cascade is the
      // chat-tab `deliver_live` cascade, not the heartbeat-keyed one.
      type LaunchResp = {
        success: boolean
        decision: string
        branch?: string
        reason?: string
      }
      const parsed: LaunchResp = JSON.parse(resp)
      if (!parsed.success) {
        toast.addToast(
          `Launch failed: ${parsed.reason ?? parsed.decision}`,
          'error',
          4000,
        )
        return
      }
      const branchLabel: Record<string, string> = {
        fresh_fire: 'Fired',
        injected: 'Sent wakeup to running session for',
        resume_and_fire: 'Resumed + fired',
      }
      let verb: string
      if (parsed.branch && parsed.branch.startsWith('workspace_session:')) {
        verb = 'Sent wakeup to pinned chat for'
      } else if (parsed.branch && parsed.branch in branchLabel) {
        verb = branchLabel[parsed.branch]
      } else {
        verb = 'Fired'
      }
      toast.addToast(`${verb} "${entry.row.name}"`, 'success', 2500)
      // Refetch heartbeat rows so the "Next run" line picks up the
      // freshly-stamped last_fired immediately. Without this, the
      // store's cached row still shows the OLD last_fired and the
      // countdown looks frozen until the next periodic refresh.
      //
      // The schedule logic does the right thing per frequency type:
      //   - hourly (every_seconds): last_fired bumps → next run is
      //     `every_seconds` from now → countdown resets
      //   - daily/weekly/monthly/yearly: last_fired = today → the
      //     "already fired today" guard kicks in → next run rolls
      //     forward to tomorrow / next-matching-day at the same
      //     wall-clock time. NEVER drifts mid-day.
      void useHeartbeatSessionsStore.getState().refresh(projectPath)
    } catch (err) {
      toast.addToast(`Launch failed: ${String(err)}`, 'error', 4000)
    } finally {
      setBusy(false)
    }
  }, [busy, projectPath, entry.row.name])

  const archivedOrDisabled = entry.state === 'archived' || !entry.row.enabled

  // Row is a div, not a button — the Launch button nests inside, and
  // HTML5 disallows nested interactive elements (browsers eject the
  // inner <button> during parsing, which broke the row click target
  // in some renderers and explained why clicking the row name did
  // nothing while Launch still worked). div + role="button" gives us
  // a clean click surface that can host the inner Launch button.
  const handleKey = (e: React.KeyboardEvent): void => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      handleClick()
    }
  }

  const nextRun = entry.row.enabled && entry.state !== 'archived'
    ? describeNextRun(entry.row.frequency, entry.row.specJson, entry.row.lastFired)
    : null

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleClick}
      onKeyDown={handleKey}
      className="w-full px-1 py-1 flex flex-col text-left hover:bg-white/[0.04] cursor-pointer no-drag transition-colors focus:outline-none focus:bg-white/[0.04]"
      title={`${entry.row.name} — ${entry.state}${entry.row.enabled ? '' : ' (disabled)'}`}
    >
      <div className="flex items-center gap-2">
        {/* Indicator only renders for the 'live' state (braille spinner —
            real-time activity that the section header can't convey).
            Resumable / Scheduled / Archived are already communicated by
            the collapsible section the row sits under, so the
            filled/hollow square just duplicated that grouping. */}
        {(() => {
          const indicator = indicatorFor(entry)
          return indicator ? <span className="flex-shrink-0">{indicator}</span> : null
        })()}
        <span
          className={`text-[11px] font-mono truncate flex-shrink-0 ${
            entry.state === 'archived'
              ? 'text-[var(--color-text-muted)] line-through'
              : entry.row.enabled
                ? 'text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-muted)]'
          }`}
        >
          {entry.row.name}
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)] truncate flex-1">
          {entry.row.enabled ? describeSpec(entry.row.frequency, entry.row.specJson) : 'Disabled'}
        </span>
        <button
          onClick={handleLaunch}
          disabled={busy || archivedOrDisabled}
          title={launchTooltip(entry)}
          className="px-2 py-0.5 text-[9px] font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed flex-shrink-0"
        >
          {busy ? '…' : 'Launch'}
        </button>
      </div>
      {nextRun && (
        <div className="text-[9px] text-[var(--color-text-muted)] truncate pt-0.5">
          Next run: {nextRun}
        </div>
      )}
    </div>
  )
}

/**
 * Tooltip shown on hover over the Launch button. Communicates how
 * a manual click affects the schedule for this row's frequency type:
 *
 *   - Hourly (`every_seconds`): manual fire stamps `last_fired` so
 *     the next cron tick is `every_seconds` from now — the countdown
 *     visibly resets. Useful for spreading load if the user notices
 *     two heartbeats clustered at the same moment.
 *
 *   - Scheduled (daily/weekly/monthly/yearly): manual fire stamps
 *     today's date but the next cron tick stays anchored to the
 *     calendar slot (next 6 AM, next Monday, etc.). Mid-day clicks
 *     never drift the schedule to a random new wall-clock time.
 */
function launchTooltip(entry: HeartbeatEntry): string {
  if (entry.state === 'archived') return 'Restore from archive before launching'
  if (!entry.row.enabled) return 'Enable this heartbeat before launching'
  let mode: string
  try {
    const v = JSON.parse(entry.row.specJson) as { frequency?: string }
    mode = v.frequency ?? entry.row.frequency
  } catch {
    mode = entry.row.frequency
  }
  if (mode === 'hourly') {
    return 'Fire now. The cron timer resets — next scheduled fire will be the full interval from this click.'
  }
  if (mode === 'daily' || mode === 'weekly' || mode === 'monthly' || mode === 'yearly') {
    return 'Fire now. The next scheduled fire stays on its calendar slot — daily/weekly/monthly schedules don\'t drift.'
  }
  return 'Fire this heartbeat now.'
}

function indicatorFor(entry: HeartbeatEntry): React.ReactNode {
  // Only the 'live' state gets an indicator — the braille spinner
  // signals real-time activity (claude is processing right now)
  // which isn't conveyed by the collapsible section header.
  //
  // Resumable / Scheduled / Archived used to render filled / hollow /
  // muted squares, but those duplicated information already shown by
  // the section the row lives under. Removed to reduce visual noise.
  if (entry.state === 'live') {
    return <span className="braille-spinner text-[10px] text-[var(--color-accent)]" />
  }
  return null
}

/**
 * Compact schedule summary derived from the heartbeat row's specJson.
 * Mirrors the formatter used in HeartbeatsSection's table so users see
 * the same "Daily 9 AM" / "Weekly Mon/Wed 7 AM" / "Every 30m" labels
 * everywhere a heartbeat surfaces.
 */
function describeSpec(frequency: string, specJson: string): string {
  let v: {
    frequency?: string
    time?: string
    days?: string[]
    days_of_month?: number[]
    months?: string[]
    every_seconds?: number
  } = {}
  try {
    v = JSON.parse(specJson)
  } catch {
    return frequency
  }
  const freq = v.frequency ?? frequency
  const at = v.time ? ` ${fmt12h(v.time)}` : ''
  if (freq === 'daily') return `Daily${at}`
  if (freq === 'weekly') {
    const days = (v.days ?? [])
      .map((d) => d.charAt(0).toUpperCase() + d.slice(1, 3))
      .join('/')
    return days ? `${days}${at}` : `Weekly${at}`
  }
  if (freq === 'monthly') {
    const days = (v.days_of_month ?? []).join(',')
    return days ? `Day ${days}${at}` : `Monthly${at}`
  }
  if (freq === 'yearly') {
    const months = (v.months ?? []).join(',')
    return months ? `${months}${at}` : `Yearly${at}`
  }
  if (freq === 'hourly') {
    const mins = Math.round((v.every_seconds ?? 3600) / 60)
    return `Every ${mins}m`
  }
  return freq
}

/** Strip a leading YAML frontmatter block (if any) from a markdown
 *  body. Mirrors wake.rs::strip_frontmatter so the wakeup body the
 *  Launch button pastes into a running session is the same content
 *  scheduled fires send via --append-system-prompt. */
function stripFrontmatter(content: string): string {
  if (!content.startsWith('---')) return content.trim()
  const end = content.slice(3).indexOf('---')
  if (end < 0) return content.trim()
  return content.slice(3 + end + 3).trim()
}

/**
 * Compute and format the next scheduled fire time for a heartbeat.
 *
 * Mirrors the daemon's schedule resolution in `crates/k2so-core/src/scheduler.rs`
 * — keeping the two in lockstep matters because the user expects the
 * "Next run" hint to match what cron actually does. We accept the same
 * frequency-mode aliasing the daemon does (daily/weekly/monthly/yearly
 * → scheduled).
 *
 * Returns a compact human label like:
 *   - "in 1m 47s"           (hourly, sub-minute precision near 0)
 *   - "Today 9 AM"          (scheduled, today still in window)
 *   - "Tomorrow 9 AM"       (scheduled, today already fired or past)
 *   - "Mon 9 AM"            (weekly, in the next 7 days)
 *   - "Apr 27 9 AM"         (further out)
 */
function describeNextRun(
  frequency: string,
  specJson: string,
  lastFired: string | null,
): string | null {
  let v: {
    frequency?: string
    time?: string
    days?: string[]
    days_of_month?: number[]
    months?: string[]
    every_seconds?: number
    start?: string
    end?: string
  } = {}
  try {
    v = JSON.parse(specJson)
  } catch {
    return null
  }
  const mode = v.frequency ?? frequency
  const now = new Date()

  // Hourly with every_seconds — relative time until next fire.
  if (mode === 'hourly') {
    const everySecs = v.every_seconds ?? 3600
    const last = lastFired ? new Date(lastFired) : null
    const nextAt = last
      ? new Date(last.getTime() + everySecs * 1000)
      : now
    const deltaSec = Math.max(0, Math.round((nextAt.getTime() - now.getTime()) / 1000))
    if (deltaSec === 0) return 'now'
    if (deltaSec < 60) return `in ${deltaSec}s`
    const m = Math.floor(deltaSec / 60)
    const s = deltaSec % 60
    return s === 0 ? `in ${m}m` : `in ${m}m ${s}s`
  }

  // scheduled / daily / weekly / monthly / yearly — find next occurrence.
  const time = v.time ?? '09:00'
  const [hStr, mStr] = time.split(':')
  const hour = parseInt(hStr, 10)
  const minute = parseInt(mStr ?? '0', 10)
  if (isNaN(hour) || isNaN(minute)) return null

  const lastDate = lastFired ? new Date(lastFired) : null
  const firedToday = lastDate
    ? isSameLocalDay(lastDate, now)
    : false

  // Build a candidate "today at HH:MM" and see if it's still in the future.
  const todayAtTime = new Date(now)
  todayAtTime.setHours(hour, minute, 0, 0)

  // Look up to 366 days ahead for a matching day.
  for (let offset = 0; offset < 366; offset++) {
    const candidate = new Date(todayAtTime)
    candidate.setDate(todayAtTime.getDate() + offset)

    // If today, skip if we've already fired today or the time has passed.
    if (offset === 0) {
      if (firedToday) continue
      if (candidate.getTime() <= now.getTime()) continue
    }

    if (!matchesScheduleDay(mode, candidate, v)) continue

    return formatNextLabel(candidate, now)
  }

  return null
}

function isSameLocalDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  )
}

function matchesScheduleDay(
  mode: string,
  candidate: Date,
  spec: { days?: string[]; days_of_month?: number[]; months?: string[] },
): boolean {
  const dowShort = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'][candidate.getDay()]
  const monthShort = [
    'jan','feb','mar','apr','may','jun','jul','aug','sep','oct','nov','dec',
  ][candidate.getMonth()]

  if (mode === 'daily' || mode === 'scheduled') return true
  if (mode === 'weekly') {
    const days = spec.days ?? []
    return days.length === 0 || days.includes(dowShort)
  }
  if (mode === 'monthly') {
    const dom = spec.days_of_month ?? []
    return dom.length === 0 || dom.includes(candidate.getDate())
  }
  if (mode === 'yearly') {
    const months = spec.months ?? []
    return months.length === 0 || months.includes(monthShort)
  }
  return false
}

/** Format the next-run label relative to `now`.
 *  Today/Tomorrow/<weekday> within 7 days, absolute date otherwise. */
function formatNextLabel(when: Date, now: Date): string {
  const time = fmt12h(`${when.getHours().toString().padStart(2, '0')}:${when.getMinutes().toString().padStart(2, '0')}`)
  if (isSameLocalDay(when, now)) return `Today ${time}`
  const tomorrow = new Date(now)
  tomorrow.setDate(now.getDate() + 1)
  if (isSameLocalDay(when, tomorrow)) return `Tomorrow ${time}`
  // Within a week → weekday name. Beyond → MMM DD.
  const daysAhead = Math.round((when.getTime() - now.getTime()) / (24 * 60 * 60 * 1000))
  if (daysAhead < 7) {
    const dow = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'][when.getDay()]
    return `${dow} ${time}`
  }
  const month = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'][when.getMonth()]
  return `${month} ${when.getDate()} ${time}`
}

/** Convert "HH:MM" → "h AM/PM" (minute elided when 00 to keep the
 *  schedule line tight in the narrow Workspace panel). */
function fmt12h(time: string): string {
  const [hStr, mStr] = time.split(':')
  let h = parseInt(hStr, 10)
  if (isNaN(h)) return time
  const m = mStr ?? '00'
  const ampm = h >= 12 ? 'PM' : 'AM'
  if (h === 0) h = 12
  else if (h > 12) h -= 12
  return m === '00' ? `${h} ${ampm}` : `${h}:${m} ${ampm}`
}
