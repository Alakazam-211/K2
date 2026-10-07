import { useCallback, useState } from 'react'
import { emit } from '@tauri-apps/api/event'
import { openHeartbeatTarget } from '@/components/common/HeartbeatSessionPicker'
import { useRoom, useRoomSupports } from '@/components/Room/RoomContext'
import { daemonCliPostQuery } from '@/lib/daemon-cli'
import { deriveDeliveryTarget } from '@/lib/heartbeat-delivery'
import { launchHeartbeat } from '@/lib/heartbeat-launch'
import { HeartbeatStatusLine } from '@/components/common/HeartbeatStatusLine'
import { scopeMayWrite } from '@/kessel/server-scope'
import {
  type HeartbeatEntry,
} from '@/stores/heartbeat-sessions'
import { useToastStore } from '@/stores/toast'

/**
 * One row in the Workspace panel's Heartbeats section.
 *
 * Layout:
 *   [indicator]  <name>   <Daily 9 AM>
 *   Next run: in 12m 04s   (or the daemon's wait reason)
 *   [toggle]                          [Launch]
 *
 * Click semantics:
 *   - Click row body → the session this heartbeat is firing in.
 *     Pinned chat is decided first (`useWorkspaceSession`) and focuses
 *     the pinned Chat tab. Own session / a picked session focus an
 *     open tab or attach the live process.
 *   - Click `Launch` → `/cli/heartbeat/launch?force=1` (test-fire even
 *     when the toggle is off). Archived rows cannot launch.
 */
export function HeartbeatEntryRow({
  entry,
  projectPath,
}: {
  entry: HeartbeatEntry
  projectPath: string
}): React.JSX.Element {
  // Opening a heartbeat target opens it in THIS drawer's room.
  const room = useRoom()
  const [busy, setBusy] = useState(false)

  // Heartbeat S4 (HB28): the room's server decides whether rows carry the
  // daemon's `nextFireAt` / `waitReason` (its scope's version, never the
  // window's). An older server shows the schedule text only (HB30).
  const nextFire = useRoomSupports('heartbeat-next-fire')

  const handleClick = (): void => {
    if (!projectPath) {
      console.warn('[heartbeats-panel] click ignored — projectPath missing')
      return
    }
    // Pinned before any conversation search. Pinned mode leaves
    // last_session_id set; searching first would focus that leftover
    // chat and never reach the pinned Chat tab.
    const mode = deriveDeliveryTarget({
      useWorkspaceSession: !!entry.row.useWorkspaceSession,
      lastSessionId: entry.row.lastSessionId,
      sessionProvider: entry.row.sessionProvider,
    }).mode
    void openHeartbeatTarget(room.tabs, projectPath, entry.row.name, mode).catch((err) => {
      console.warn('[heartbeats-panel] open heartbeat failed:', err)
    })
  }

  const handleLaunch = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation()
    if (busy || !projectPath || entry.state === 'archived') return
    // Home M4/M5: the room's write rules (view-only fires nothing).
    if (!scopeMayWrite(room.scope, 'heartbeat/launch')) return
    setBusy(true)
    try {
      await launchHeartbeat(room, projectPath, entry.row.name)
    } finally {
      setBusy(false)
    }
  }, [busy, projectPath, entry.row.name, entry.state])

  const handleToggle = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation()
    if (busy || !projectPath || entry.state === 'archived') return
    // Home M4/M5: `heartbeat/enable` is a GET that writes: held to the
    // room's write rules.
    if (!scopeMayWrite(room.scope, 'heartbeat/enable')) return
    setBusy(true)
    try {
      await daemonCliPostQuery(room.scope, 'heartbeat/enable', {
        project: projectPath,
        name: entry.row.name,
        enabled: entry.row.enabled ? '0' : '1',
      })
      void room.heartbeats.getState().refresh(projectPath)
      // MS67: `sync:projects` tells THIS computer's windows; not from a room
      // on another server.
      if (room.localCommands) void emit('sync:projects').catch(() => {})
    } catch (err) {
      useToastStore.getState().addToast(`Toggle failed: ${String(err)}`, 'error', 4000)
    } finally {
      setBusy(false)
    }
  }, [busy, projectPath, entry.row.name, entry.row.enabled, entry.state])

  const archived = entry.state === 'archived'

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
          {describeSpec(entry.row.frequency, entry.row.specJson)}
        </span>
      </div>
      {/* Heartbeat S4 (HB26/HB27): the daemon's next fire or the reason it
          is waiting, through the shared formatter. No schedule math. */}
      {!archived && (
        <HeartbeatStatusLine
          row={entry.row}
          nextFire={nextFire}
          className="text-[9px] truncate pt-0.5"
        />
      )}
      {!archived && (
        <div className="flex items-center justify-between gap-2 pt-1">
          <button
            type="button"
            onClick={handleToggle}
            role="switch"
            aria-checked={entry.row.enabled}
            disabled={busy}
            className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
              entry.row.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
            title={entry.row.enabled ? 'Enabled — click to disable' : 'Disabled — click to enable'}
          >
            <span
              className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                entry.row.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
              }`}
            />
          </button>
          <button
            type="button"
            onClick={handleLaunch}
            disabled={busy}
            title={launchTooltip(entry)}
            className="px-2 py-0.5 text-[9px] font-medium text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed flex-shrink-0"
          >
            {busy ? '…' : 'Launch'}
          </button>
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
  if (!entry.row.enabled) return 'Fire now to test — stays off the schedule until you enable it.'
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
