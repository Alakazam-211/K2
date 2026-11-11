// Settings is about the window's server: heartbeats open in the primary
// room's tabs.
import { useTabsStore } from '@/stores/tabs'
import React, { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost, daemonCliPostQuery } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { heartbeatHistoryActorLine } from '@/lib/heartbeat-actor'
import type { SettingEntry } from '../searchManifest'
import { WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'
import { HeartbeatSessionPicker, openHeartbeatTarget } from '@/components/common/HeartbeatSessionPicker'
import {
  applyDeliveryTarget,
  deriveDeliveryTarget,
  setHeartbeatSession,
  type HeartbeatDeliveryTarget,
} from '@/lib/heartbeat-delivery'
import { primaryScope } from '@/kessel/server-scope'
import { spanLabel } from '@/lib/heartbeat-wait'
import { HeartbeatStatusLine } from '@/components/common/HeartbeatStatusLine'
import { useHeartbeatDueRefetch } from '@/hooks/useHeartbeatDueRefetch'
import { BetaTag, Button, Segmented, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'

export const WAKE_SCHEDULER_MANIFEST: SettingEntry[] = [
  {
    id: 'wake-scheduler.wake',
    section: 'wake-scheduler',
    label: 'Wake This Computer for Heartbeats',
    description: 'Wake from sleep a minute before the next heartbeat',
    keywords: ['wake', 'heartbeat', 'scheduler', 'sleep', 'lid', 'overnight'],
  },
  {
    id: 'wake-scheduler.battery',
    section: 'wake-scheduler',
    label: 'Also on Battery',
    description: 'Wake for heartbeats on battery too, never below 20%',
    keywords: ['battery', 'wake', 'lid', 'power'],
  },
]

/** Heartbeat S2 (W8) — what the daemon's power layer really has. */
interface WakeStatusInfo {
  state: 'off' | 'ok' | 'unavailable' | 'paused'
  reason: string
  nextWakeAt: string | null
  nextFireAt: string | null
  support?: { ready: boolean; reason?: string }
  platform?: string
  batteryFloorPercent?: number
  powerSource?: { onAc: boolean | null; batteryPercent: number | null }
  notes?: {
    wakeTimers?: { pluggedIn: string; onBattery: string }
    steps?: string
    lidClosed?: string
  } | null
}

/** Copy per platform for the wake switch (D8, W8, D11, D14). */
function wakePlatformNote(platform: string | undefined): string {
  switch (platform) {
    case 'macos':
      return 'The first time you turn this on, macOS asks once for an admin password to install a small helper. With the lid closed: on power it is best effort; on battery it is not supported, and the Mac may go back to sleep within a minute.'
    case 'linux':
      return 'Uses the wake alarm permission the K2 package sets up. Without it, K2 shows the one command to run.'
    case 'windows':
      return 'Windows wakes for this only when the power plan allows wake timers. K2 shows the setting below and never changes it.'
    default:
      return ''
  }
}

function formatClock(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (isNaN(d.getTime())) return iso
  const sameDay = d.toDateString() === new Date().toDateString()
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  return sameDay ? time : `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })} ${time}`
}

/** Row shape returned by `k2so_heartbeat_fires_list_all` — most recent
 *  fire decisions across all workspaces. Used by the audit log column. */
interface SystemFireRow {
  id: number
  projectId: string
  projectName: string
  agentName: string | null
  scheduleName: string | null
  firedAt: string
  mode: string
  decision: string
  reason: string | null
  durationMs: number | null
  /** 0125 (AH18): who did it; null for scheduler rows / older servers. */
  actor?: string | null
}


/** Row shape returned by the `k2so_heartbeat_list_all` Tauri command —
 *  every active heartbeat across all workspaces with the parent project's
 *  name + path joined in. */
interface SystemHeartbeatRow {
  id: string
  projectId: string
  projectName: string
  projectPath: string
  name: string
  frequency: string
  specJson: string
  wakeupPath: string
  enabled: boolean
  lastFired: string | null
  useWorkspaceSession: boolean
  // Delivery drop-down — the heartbeat's saved/trained session (see
  // HeartbeatRow + `lib/heartbeat-delivery.ts`; older daemons omit
  // `sessionProvider`).
  lastSessionId: string | null
  sessionProvider?: string | null
  // Reliability overhaul — failure/error visibility (see HeartbeatRow).
  consecutiveFailures: number
  disabledReason: string | null
  scheduleError: string | null
  // S5 — daemon-decided wait reason (older daemons omit both).
  waitReason?: string | null
  waitDetail?: string | null
  // Heartbeat S3/S4 (HB24): the daemon's next fire and when the reason
  // last changed. Rendered through the shared formatter (HB31).
  nextFireAt?: string | null
  waitSince?: string | null
}

/** Payload of `/cli/heartbeat/scheduler-status`. Heartbeat S2: the
 *  daemon ticks itself every 60 s (`ticker`); `wake` / `awake` say what
 *  the power layer really has. Older servers omit the S2 fields. */
interface SchedulerStatus {
  lastTickAt: string | null
  lastDaemonTickAt?: string | null
  staleSecs: number | null
  enabledCount: number
  transportInstalled: boolean
  wakeMode: string
  ticker?: { intervalSecs: number; lastTickAt: string | null; staleSecs: number | null }
  wakeForHeartbeats?: boolean
  wakeOnBattery?: boolean
  wake?: WakeStatusInfo
  awake?: { held: boolean; reasons: string[] }
}

/** "Not ticking" banner text, or null when healthy. S2 servers report
 *  their own 60 s loop (`ticker`); three missed ticks is a problem.
 *  Older servers fall back to the combined tick stamp. */
function tickerDownMessage(status: SchedulerStatus | null): string | null {
  if (!status || status.enabledCount === 0) return null
  // Same span words as the rows' "scheduler not ticking (8h)" (HB31).
  const since = (iso: string | null | undefined): string => {
    const t = iso ? Date.parse(iso) : NaN
    return Number.isNaN(t)
      ? 'yet: no check has been recorded'
      : `for ${spanLabel(Date.now() - t)} (since ${formatClock(iso)})`
  }
  if (status.ticker) {
    const stale = status.ticker.staleSecs == null || status.ticker.staleSecs > 180
    if (!stale) return null
    // HB9: the daemon's own tick stamp (`lastDaemonTickAt`).
    return `Heartbeats haven’t been checked ${since(status.ticker.lastTickAt ?? status.lastDaemonTickAt)}. K2 may be stopped or restarting on this server. When it’s back, any heartbeat less than 12 hours late runs once.`
  }
  const stale = status.staleSecs == null || status.staleSecs > 180
  if (!stale) return null
  return `Heartbeats haven’t been checked ${since(status.lastTickAt)}. Update K2 on this server so it checks them on its own every minute.`
}

/** Compact one-liner for the heartbeat list row — "Every day at 9 AM",
 *  "Every 5min 1 AM–11 PM", etc. Mirrors HeartbeatsSection's `describeSpec`
 *  but takes raw JSON instead of a typed row so we don't need to import
 *  it here. */
function describeHeartbeatSpec(specJson: string, frequency: string): string {
  let spec: Record<string, unknown> = {}
  try { spec = JSON.parse(specJson) } catch { /* fall through to frequency */ }
  const fmt12 = (t: unknown): string => {
    if (typeof t !== 'string' || !t.includes(':')) return ''
    const [hh, mm] = t.split(':')
    let h = parseInt(hh, 10); if (isNaN(h)) return t
    const ap = h >= 12 ? 'PM' : 'AM'
    if (h === 0) h = 12; else if (h > 12) h -= 12
    return mm === '00' ? `${h} ${ap}` : `${h}:${mm} ${ap}`
  }
  const at = typeof spec.time === 'string' ? ` at ${fmt12(spec.time)}` : ''
  // Non-hourly firing window (reliability overhaul) — hourly renders
  // its window inline in its own case below.
  const win =
    frequency !== 'hourly' && (typeof spec.start === 'string' || typeof spec.end === 'string')
      ? ` · window ${fmt12((spec.start as string | undefined) ?? '00:00')}–${fmt12((spec.end as string | undefined) ?? '23:59')}`
      : ''
  switch (frequency) {
    case 'daily': return `Every day${at}${win}`
    case 'weekly': {
      const days = (spec.days as string[] | undefined ?? []).map((d) => d.charAt(0).toUpperCase() + d.slice(1))
      return days.length ? `Every ${days.join(', ')}${at}${win}` : `Weekly${at}${win}`
    }
    case 'monthly': return `Day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}${win}`
    case 'yearly': return `${(spec.months as string[] | undefined ?? []).join(', ')} day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}${win}`
    case 'hourly': {
      const every = (spec.every_seconds as number | undefined) ?? 3600
      const start = fmt12((spec.start as string | undefined) ?? '00:00')
      const end = fmt12((spec.end as string | undefined) ?? '23:59')
      return `Every ${Math.round(every / 60)} min, ${start}–${end}`
    }
    default: return frequency
  }
}

const CONTENT_WIDTH = 'max-w-[52rem]'

/** A list reply from the daemon or the in-process command. Anything else
 *  (null, an error envelope, an object) is a failed read, not an empty list:
 *  the page shows its error card instead of crashing on `.length`. */
export function listReply<T>(raw: unknown): T[] {
  if (Array.isArray(raw)) return raw as T[]
  throw new Error('the reply wasn’t a list')
}

/** The scheduler status is an object; anything else is a failed read. */
function statusReply(raw: unknown): SchedulerStatus {
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) return raw as SchedulerStatus
  throw new Error('the reply wasn’t a status')
}

function errorText(err: unknown): string {
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
  return raw.trim()
}

type Tone = 'ok' | 'late' | 'problem' | 'skip'

/** A run's decision in plain words, and how it reads at a glance. */
export function describeDecision(decision: string): { text: string; tone: Tone } {
  switch (decision) {
    case 'fired':
      return { text: 'Ran', tone: 'ok' }
    case 'fired_catchup':
      return { text: 'Ran late, catching up a missed time', tone: 'late' }
    case 'error':
      return { text: 'Failed', tone: 'problem' }
    case 'wakeup_file_missing':
      return { text: 'Failed: its wakeup file is missing', tone: 'problem' }
    case 'schedule_invalid':
      return { text: 'Failed: its schedule isn’t valid', tone: 'problem' }
    case 'auto_disabled':
    case 'auto_disabled_failing':
      return { text: 'Paused by K2 after failing several times', tone: 'problem' }
    case 'tick_gap':
      return { text: 'Missed: K2 wasn’t checking then', tone: 'problem' }
    case 'skipped_missed':
      return { text: 'Skipped: more than 12 hours late', tone: 'problem' }
    case 'not_due':
      return { text: 'Not due yet', tone: 'skip' }
  }
  const SKIPS: Record<string, string> = {
    skipped_locked: 'another check was already running',
    skipped_already_fired: 'it already ran for this time',
    skipped_overlap: 'its last run was still going',
    skipped_archived: 'the workspace is archived',
    skipped_user_session: 'you were using its chat',
    skipped_quality_gate: 'held back by a quality check',
    skipped_no_primary: 'there was no agent to wake',
    skipped_empty: 'its wakeup file is empty',
    skipped_custom_timing: 'it wasn’t its time yet',
    skipped_busy: 'the agent was busy',
  }
  if (decision.startsWith('skipped_')) {
    return { text: `Skipped: ${SKIPS[decision] ?? decision.slice('skipped_'.length).replace(/_/g, ' ')}`, tone: 'skip' }
  }
  return { text: decision.replace(/_/g, ' '), tone: 'skip' }
}

const DOT: Record<Tone, string> = {
  ok: 'bg-[var(--color-status-ok)]',
  late: 'bg-[var(--color-status-warn)]',
  problem: 'bg-[var(--color-status-error)]',
  skip: 'bg-[var(--color-text-muted)]',
}

function Dot({ tone }: { tone: Tone }): React.JSX.Element {
  return <span aria-hidden="true" className={`mt-1.5 inline-block h-2 w-2 flex-shrink-0 rounded-[var(--radius-status)] ${DOT[tone]}`} />
}

/** What a row says after its last change: paused / resumed for a moment,
 *  or why the change didn't take. */
type RowNote = { kind: 'saved' | 'error'; text: string }

function FireRow({ fire, open, onToggle }: { fire: SystemFireRow; open: boolean; onToggle: () => void }): React.JSX.Element {
  const d = describeDecision(fire.decision)
  const actor = heartbeatHistoryActorLine(fire)
  const when = new Date(fire.firedAt)
  const fields: Array<[string, string | null | undefined]> = [
    ['When', Number.isNaN(when.getTime()) ? fire.firedAt : when.toLocaleString()],
    ['Result', fire.decision],
    ['Reason', fire.reason],
    ['Took', fire.durationMs != null ? `${(fire.durationMs / 1000).toFixed(1)} s` : null],
    ['How', fire.mode],
    ['Who', actor || null],
  ]
  const detailId = `heartbeat-run-${fire.id}`
  return (
    <li data-fire-row="" data-tone={d.tone}>
      <button
        type="button"
        data-k2-focus=""
        aria-expanded={open}
        aria-controls={detailId}
        onClick={onToggle}
        className="group relative w-full grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-start gap-x-3 px-4 py-3 text-left no-drag cursor-pointer outline-none transition-colors hover:bg-[var(--color-bg-hover)]"
      >
        <Dot tone={d.tone} />
        <span className="min-w-0">
          <span className="block type-row text-[var(--color-text-primary)] truncate">
            <span className="type-name">{fire.scheduleName ?? 'Heartbeat'}</span>
            <span className="text-[var(--color-text-secondary)]"> in {fire.projectName}</span>
          </span>
          <span className={`block type-hint mt-0.5 ${d.tone === 'problem' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}`}>
            {d.text}
          </span>
          {fire.reason && !open && (
            <span className="block type-hint text-[var(--color-text-secondary)] mt-0.5 truncate">{fire.reason}</span>
          )}
        </span>
        <span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap">{formatClock(fire.firedAt)}</span>
        <span aria-hidden="true" className={`type-row text-[var(--color-text-muted)] group-hover:text-[var(--color-text-primary)] transition-transform ${open ? 'rotate-90' : ''}`}>
          ›
        </span>
      </button>
      {open && (
        <dl id={detailId} className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-4 gap-y-1 px-4 pb-3 pl-9">
          {fields
            .filter(([, v]) => v != null && v !== '')
            .map(([k, v]) => (
              <React.Fragment key={k}>
                <dt className="type-hint text-[var(--color-text-secondary)]">{k}</dt>
                <dd className={`type-hint text-[var(--color-text-primary)] break-words ${k === 'Result' ? 'font-mono' : ''}`}>{v}</dd>
              </React.Fragment>
            ))}
        </dl>
      )}
    </li>
  )
}

export function WakeSchedulerSection(): React.JSX.Element {
  // Every heartbeat on this server, across workspaces. null = not read yet.
  const [heartbeats, setHeartbeats] = useState<SystemHeartbeatRow[] | null>(null)
  const [heartbeatsLoading, setHeartbeatsLoading] = useState(true)
  const [heartbeatsError, setHeartbeatsError] = useState<{ detail: string; oldHost: boolean } | null>(null)
  // When non-null, render the `WakeupEditor` overlay (AIFileEditor +
  // live preview, scoped to this heartbeat's WAKEUP.md). null = list view.
  const [editingHeartbeat, setEditingHeartbeat] = useState<SystemHeartbeatRow | null>(null)
  // The newest 100 runs across all workspaces, polled every 5 s.
  const [fires, setFires] = useState<SystemFireRow[] | null>(null)
  const [firesError, setFiresError] = useState<string | null>(null)
  const [firesAttempt, setFiresAttempt] = useState(0)
  const [firesView, setFiresView] = useState<'all' | 'problems'>('all')
  const [openFire, setOpenFire] = useState<number | null>(null)
  // Scheduler + wake status from the daemon (S2: ticker, wake, awake),
  // polled every 15 s and right after the wake switch moves.
  const [schedulerStatus, setSchedulerStatus] = useState<SchedulerStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [statusAttempt, setStatusAttempt] = useState(0)
  // The wake switch is daemon truth (`wakeForHeartbeats`); this only
  // tracks a request in flight (the macOS admin dialog can take a while).
  const [wakeSaving, setWakeSaving] = useState(false)
  const [wakeNote, setWakeNote] = useState<RowNote | null>(null)
  const [rowNotes, setRowNotes] = useState<Record<string, RowNote>>({})
  const noteTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({})

  useEffect(() => () => Object.values(noteTimers.current).forEach(clearTimeout), [])

  const noteRow = useCallback((id: string, note: RowNote | null) => {
    clearTimeout(noteTimers.current[id])
    setRowNotes((n) => {
      const next = { ...n }
      if (note) next[id] = note
      else delete next[id]
      return next
    })
    // A confirmation fades; an error stays until the next try.
    if (note?.kind === 'saved') noteTimers.current[id] = setTimeout(() => noteRow(id, null), 6000)
  }, [])

  const refreshStatus = useCallback(async () => {
    const status = statusReply(await daemonCliGet<unknown>(primaryScope(), 'heartbeat/scheduler-status', {}))
    setSchedulerStatus(status)
    setStatusError(null)
  }, [])

  /** D8 / D11 — flip "Wake this computer for heartbeats" (or "Also on
   *  battery"). The daemon may show one admin dialog on its Mac; if it is
   *  declined the switch stays off and the daemon says why. */
  const handleSetWake = useCallback(
    async (enabled: boolean, onBattery?: boolean) => {
      setWakeSaving(true)
      setWakeNote(null)
      try {
        const resp = await daemonCliPost<{
          success?: boolean
          wakeForHeartbeats?: boolean
          message?: string
        }>(primaryScope(), 'heartbeat/wake', { enabled, onBattery })
        if (resp?.success === false) {
          setWakeNote({ kind: 'error', text: `Wake stays off. ${resp.message ?? 'The server didn’t say why.'}` })
        } else {
          setWakeNote({ kind: 'saved', text: 'Saved' })
        }
        await refreshStatus()
      } catch (err) {
        setWakeNote({
          kind: 'error',
          text: `Couldn’t change wake: ${errorText(err).replace(/[.\s]+$/, '') || 'no answer from this server'}. Nothing changed; try again.`,
        })
      } finally {
        setWakeSaving(false)
      }
    },
    [refreshStatus],
  )

  // Sort case-insensitively by workspace, then heartbeat name — the
  // backend ORDER BY is case-sensitive ASCII.
  const refreshHeartbeats = useCallback(async (opts?: { quiet?: boolean }) => {
    if (!opts?.quiet) setHeartbeatsLoading(true)
    try {
      // Host-aware (0.40.48): connected to a REMOTE host, the roster comes
      // from ITS /cli/heartbeat/list-all route; LOCAL keeps the in-process
      // command (same machine, same DB).
      const raw =
        useConnectHostStore.getState().activeHost !== 'local'
          ? await daemonCliGet<unknown>(primaryScope(), 'heartbeat/list-all')
          : await invoke<unknown>('k2so_heartbeat_list_all')
      const sorted = [...listReply<SystemHeartbeatRow>(raw)].sort((a, b) => {
        const byProj = a.projectName.toLowerCase().localeCompare(b.projectName.toLowerCase())
        if (byProj !== 0) return byProj
        return a.name.toLowerCase().localeCompare(b.name.toLowerCase())
      })
      setHeartbeats(sorted)
      setHeartbeatsError(null)
    } catch (err) {
      // An older host has no list-all route: its project-scoped catch-all
      // answers "Missing 'project' parameter", which means "update the host".
      const s = useConnectHostStore.getState()
      const oldHost = s.activeHost !== 'local' && /missing '?project/i.test(String(err))
      setHeartbeatsError({ detail: oldHost ? '' : errorText(err), oldHost })
    } finally {
      setHeartbeatsLoading(false)
    }
  }, [])

  useEffect(() => {
    void refreshHeartbeats()
  }, [refreshHeartbeats])

  // Heartbeat S4: one quiet refetch when a row's fire time + 120 s passes
  // (HB29), so a fired or overdue row never sits stale.
  const nextFire = primaryScope().serverSupports('heartbeat-next-fire')
  useHeartbeatDueRefetch(heartbeats ?? [], () => void refreshHeartbeats({ quiet: true }), nextFire)

  // Recent runs, polled every 5 s. A failed poll keeps the last good list
  // and says it stopped updating; a failed first read shows the error card.
  useEffect(() => {
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const raw =
          useConnectHostStore.getState().activeHost !== 'local'
            ? await daemonCliGet<unknown>(primaryScope(), 'heartbeat/fires-list-all', { limit: 100 })
            : await invoke<unknown>('k2so_heartbeat_fires_list_all', { limit: 100 })
        const rows = listReply<SystemFireRow>(raw)
        if (!cancelled) {
          setFires(rows)
          setFiresError(null)
        }
      } catch (err) {
        if (!cancelled) setFiresError(errorText(err) || 'no answer from this server')
      }
    }
    void tick()
    const id = window.setInterval(tick, 5000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [firesAttempt])

  useEffect(() => {
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const status = statusReply(await daemonCliGet<unknown>(primaryScope(), 'heartbeat/scheduler-status', {}))
        if (!cancelled) {
          setSchedulerStatus(status)
          setStatusError(null)
        }
      } catch (err) {
        if (!cancelled) setStatusError(errorText(err) || 'no answer from this server')
      }
    }
    void tick()
    const id = window.setInterval(tick, 15000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [statusAttempt])

  const handleToggleEnabled = useCallback(
    async (row: SystemHeartbeatRow, next: boolean) => {
      // Optimistic — flip locally, fall back if the daemon rejects.
      setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? { ...r, enabled: next } : r)))
      noteRow(row.id, null)
      try {
        await daemonCliPostQuery(primaryScope(), 'heartbeat/enable', {
          project: row.projectPath,
          name: row.name,
          enabled: next,
        })
        noteRow(row.id, {
          kind: 'saved',
          text: next ? 'Resumed. It runs on its schedule again.' : 'Paused. It won’t run until you turn it back on.',
        })
      } catch (err) {
        setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? { ...r, enabled: !next } : r)))
        noteRow(row.id, {
          kind: 'error',
          text: `Couldn’t ${next ? 'resume' : 'pause'} it: ${errorText(err).replace(/[.\s]+$/, '') || 'no answer from this server'}. Nothing changed; try again.`,
        })
      }
    },
    [noteRow],
  )

  // Where each heartbeat's wakeup goes. Optimistic like the switch.
  const handleSetDelivery = useCallback(
    async (row: SystemHeartbeatRow, next: HeartbeatDeliveryTarget) => {
      setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? applyDeliveryTarget(r, next) : r)))
      noteRow(row.id, null)
      try {
        // Writes go to the same host the rows came from (0.40.48).
        await setHeartbeatSession(row.projectPath, row.name, next)
        noteRow(row.id, { kind: 'saved', text: 'Saved. The next run goes there.' })
      } catch (err) {
        setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? row : r)))
        noteRow(row.id, {
          kind: 'error',
          text: `Couldn’t change where it runs: ${errorText(err).replace(/[.\s]+$/, '') || 'no answer from this server'}. Nothing changed; try again.`,
        })
      }
    },
    [noteRow],
  )

  // "Edit wakeup" opens the WakeupEditor takeover. The agent name falls back
  // to the workspace slug; WakeupEditor's AGENT.md read fails soft.
  if (editingHeartbeat) {
    const toRow = (r: SystemHeartbeatRow): HeartbeatRow => ({
      id: r.id,
      projectId: r.projectId,
      name: r.name,
      frequency: r.frequency,
      specJson: r.specJson,
      wakeupPath: r.wakeupPath,
      enabled: r.enabled,
      lastFired: r.lastFired,
      createdAt: 0,
      useWorkspaceSession: r.useWorkspaceSession,
      lastSessionId: r.lastSessionId,
      sessionProvider: r.sessionProvider,
      consecutiveFailures: r.consecutiveFailures,
      nextRetryAt: null,
      disabledReason: r.disabledReason,
      scheduleError: r.scheduleError,
    })
    const slug = editingHeartbeat.projectName.toLowerCase().replace(/\s+/g, '-')
    const otherHeartbeats = (heartbeats ?? [])
      .filter((r) => r.projectId === editingHeartbeat.projectId && r.id !== editingHeartbeat.id)
      .map(toRow)
    return (
      <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
        <WakeupEditor
          projectPath={editingHeartbeat.projectPath}
          agentName={slug}
          heartbeat={toRow(editingHeartbeat)}
          otherHeartbeats={otherHeartbeats}
          onClose={() => {
            setEditingHeartbeat(null)
            void refreshHeartbeats()
          }}
        />
      </div>
    )
  }

  const tickerDown = tickerDownMessage(schedulerStatus)
  const lastCheck = schedulerStatus?.ticker?.lastTickAt ?? schedulerStatus?.lastDaemonTickAt ?? schedulerStatus?.lastTickAt ?? null

  // ── How heartbeats run: status, wake, battery ──────────────────────
  const statusState =
    schedulerStatus === null ? (
      statusError ? (
        <SettingsLoadState
          kind="error"
          title="K2 couldn’t read the scheduler"
          detail={statusError}
          action={<Button onClick={() => setStatusAttempt((n) => n + 1)}>Try again</Button>}
          testId="heartbeats-status-error"
        >
          Whether heartbeats are being checked is unknown right now. Your heartbeats are unchanged.
        </SettingsLoadState>
      ) : (
        <SettingsLoadState kind="loading" title="Checking the scheduler…" testId="heartbeats-status-loading" />
      )
    ) : undefined

  const wake = schedulerStatus?.wake
  const wakeOn = schedulerStatus?.wakeForHeartbeats === true
  const onBattery = schedulerStatus?.wakeOnBattery === true
  const note = wakePlatformNote(wake?.platform)

  const statusGroup = (
    <SettingsGroup card title="How heartbeats run" state={statusState} testId="heartbeats-status">
      <div className="px-4 py-3 flex items-start gap-3" data-testid="heartbeats-checking">
        <Dot tone={tickerDown ? 'problem' : 'ok'} />
        <div className="min-w-0 flex-1">
          <p className={SETTING_ROW_LABEL}>{tickerDown ? 'Heartbeats aren’t being checked' : 'Checking every minute'}</p>
          <p className={SETTING_ROW_HINT}>
            {tickerDown ??
              `${lastCheck ? `Last check ${formatClock(lastCheck)}. ` : ''}K2 checks while this computer is awake, with or without the app open. A heartbeat missed while it slept runs once when it wakes, if it is less than 12 hours late.`}
          </p>
        </div>
      </div>

      {schedulerStatus && !wake && (
        <div className="px-4 py-3">
          <p className={SETTING_ROW_LABEL}>Wake this computer for heartbeats</p>
          <p className={SETTING_ROW_HINT}>This server runs an older K2 that can’t wake the computer. Update K2 on it to use this.</p>
        </div>
      )}

      {wake && (
        <div className="px-4 py-3" data-settings-id="wake-scheduler.wake">
          <div className="flex items-start justify-between gap-6">
            <div className="min-w-0 flex-1">
              <p className={SETTING_ROW_LABEL}>Wake this computer for heartbeats</p>
              <p className={SETTING_ROW_HINT}>
                {wakeOn
                  ? 'K2 wakes this computer a minute before the next heartbeat.'
                  : 'Off: K2 doesn’t wake this computer from sleep. Heartbeats still fire whenever it is awake.'}
                {note ? ` ${note}` : ''}
              </p>
              {wakeSaving && wake.platform === 'macos' && !wakeOn && (
                <p className="type-hint text-[var(--color-text-primary)] mt-1" role="status">
                  Approve the admin dialog on this Mac to finish turning wake on.
                </p>
              )}
              {wakeOn && wake.state === 'ok' && (
                <p className="type-hint text-[var(--color-text-primary)] mt-1">
                  {wake.nextWakeAt
                    ? `Next wake ${formatClock(wake.nextWakeAt)}${wake.nextFireAt ? `, for a heartbeat at ${formatClock(wake.nextFireAt)}` : ''}.`
                    : 'No heartbeat is scheduled, so no wake is set.'}
                </p>
              )}
              {wakeOn && (wake.state === 'unavailable' || wake.state === 'paused') && (
                <p className="type-hint text-[var(--color-status-warn-amber-soft)] mt-1">
                  {wake.state === 'paused' ? 'Paused: ' : 'Not available: '}
                  {wake.reason}
                </p>
              )}
              {wakeOn && wake.notes?.wakeTimers && (
                <p className={SETTING_ROW_HINT}>
                  Wake timers in this power plan: plugged in {wake.notes.wakeTimers.pluggedIn.replace('_', ' ')}, on battery{' '}
                  {wake.notes.wakeTimers.onBattery.replace('_', ' ')}.
                  {wake.notes.steps ? ` To allow them: ${wake.notes.steps}` : ''}
                </p>
              )}
              {schedulerStatus?.awake?.held && (
                <p className={SETTING_ROW_HINT}>Keeping this computer awake now: {schedulerStatus.awake.reasons.join(', ')}.</p>
              )}
              {wakeNote?.kind === 'error' && (
                <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1 max-w-[75ch]">
                  {wakeNote.text}
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {wakeNote?.kind === 'saved' && !wakeSaving && (
                <span role="status" className="type-hint text-[var(--color-status-ok-soft)]">
                  Saved
                </span>
              )}
              <Toggle
                size="sm"
                checked={wakeOn}
                onChange={() => void handleSetWake(!wakeOn)}
                aria-label="Wake this computer for heartbeats"
                disabled={wakeSaving}
                style={wakeSaving ? { cursor: 'wait' } : undefined}
              />
            </div>
          </div>
        </div>
      )}

      {wake && wakeOn && (
        <div className="px-4 py-3 flex items-start justify-between gap-6" data-settings-id="wake-scheduler.battery">
          <div className="min-w-0 flex-1">
            <p className={SETTING_ROW_LABEL}>Also on battery</p>
            <p className={SETTING_ROW_HINT}>
              Without this, K2 wakes the computer only while it is plugged in. Never below {wake.batteryFloorPercent ?? 20}% battery.
            </p>
          </div>
          <Toggle
            size="sm"
            checked={onBattery}
            disabled={wakeSaving}
            onChange={() => void handleSetWake(true, !onBattery)}
            aria-label="Also wake on battery"
          />
        </div>
      )}
    </SettingsGroup>
  )

  // ── Your heartbeats ────────────────────────────────────────────────
  const listState = heartbeatsError ? (
    <SettingsLoadState
      kind="error"
      title={heartbeatsError.oldHost ? 'This server can’t list its heartbeats here yet' : 'Your heartbeats couldn’t load'}
      detail={heartbeatsError.detail || null}
      action={<Button onClick={() => void refreshHeartbeats()}>Try again</Button>}
      testId="heartbeats-list-error"
    >
      {heartbeatsError.oldHost
        ? 'Update K2 on the server, then try again.'
        : 'K2 couldn’t read the heartbeats on this server. They still run on their schedules; try again in a moment.'}
    </SettingsLoadState>
  ) : heartbeats === null && heartbeatsLoading ? (
    <SettingsLoadState kind="loading" title="Loading your heartbeats…" testId="heartbeats-list-loading" />
  ) : heartbeats !== null && heartbeats.length === 0 ? (
    <SettingsLoadState kind="empty" title="No heartbeats yet" testId="heartbeats-list-empty">
      A heartbeat wakes a workspace’s agent on a schedule. Add one in a workspace’s settings, under Workspaces → Heartbeats.
    </SettingsLoadState>
  ) : undefined

  const listGroup = (
    <SettingsGroup
      card
      title="Your heartbeats"
      hint="Every heartbeat in every workspace. Pause one to stop it running until you turn it back on."
      state={listState}
      testId="heartbeats-list"
    >
      {(heartbeats ?? []).map((row) => {
        const target = deriveDeliveryTarget(row)
        const rowNote = rowNotes[row.id]
        return (
          <div key={row.id} className="px-4 py-3" data-testid={`heartbeat-row-${row.id}`}>
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0 flex-1">
                <p className="type-row text-[var(--color-text-primary)] truncate">
                  <span className="type-name">{row.name}</span>
                  <span className="text-[var(--color-text-secondary)]"> in {row.projectName}</span>
                </p>
                <p className={SETTING_ROW_HINT}>{describeHeartbeatSpec(row.specJson, row.frequency)}</p>
                {/* Heartbeat S4 (HB31): next run or the daemon's wait reason. */}
                {/* Paused by a person reads plainly; paused by K2 keeps the
                    formatter's reason. */}
                {!row.enabled && !row.disabledReason && !row.waitDetail && (!row.waitReason || row.waitReason === 'disabled:user') ? (
                  <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">Paused. Turn it on to resume.</p>
                ) : (
                  <HeartbeatStatusLine row={row} nextFire={nextFire} nextPrefix="Next run: " className="type-hint mt-0.5" />
                )}
                {rowNote && (
                  <p
                    role={rowNote.kind === 'error' ? 'alert' : 'status'}
                    data-row-note={rowNote.kind}
                    className={`type-hint mt-1 max-w-[75ch] ${rowNote.kind === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-ok-soft)]'}`}
                  >
                    {rowNote.text}
                  </p>
                )}
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                <HeartbeatSessionPicker
                  variant="field"
                  ariaLabel={`Where ${row.name} runs`}
                  projectPath={row.projectPath}
                  projectId={row.projectId}
                  value={target}
                  onSelect={(next) => {
                    void handleSetDelivery(row, next)
                  }}
                />
                <Button
                  variant="quiet"
                  onClick={() => {
                    void openHeartbeatTarget(useTabsStore, row.projectPath, row.name, target.mode)
                  }}
                  title={target.mode === 'pinned' ? 'Open the workspace’s pinned chat' : 'Open this heartbeat’s chat'}
                  aria-label={`Open the chat ${row.name} runs in`}
                >
                  Open chat
                </Button>
                <Button onClick={() => setEditingHeartbeat(row)} title={`Edit ${row.wakeupPath}`} aria-label={`Edit the wakeup for ${row.name}`}>
                  Edit wakeup
                </Button>
                <Toggle
                  size="sm"
                  checked={row.enabled}
                  onChange={() => void handleToggleEnabled(row, !row.enabled)}
                  aria-label={`Run ${row.name} in ${row.projectName}`}
                  title={row.enabled ? 'On: turn off to pause it' : 'Paused: turn on to resume it'}
                />
              </div>
            </div>
          </div>
        )
      })}
    </SettingsGroup>
  )

  // ── Recent runs ────────────────────────────────────────────────────
  const shown = (fires ?? []).filter((f) => firesView === 'all' || describeDecision(f.decision).tone === 'problem')
  const problemCount = (fires ?? []).filter((f) => describeDecision(f.decision).tone === 'problem').length
  const firesState =
    fires === null ? (
      firesError ? (
        <SettingsLoadState
          kind="error"
          title="Recent runs couldn’t load"
          detail={firesError}
          action={<Button onClick={() => setFiresAttempt((n) => n + 1)}>Try again</Button>}
          testId="heartbeats-runs-error"
        >
          K2 couldn’t read what the scheduler did. Your heartbeats still run; try again in a moment.
        </SettingsLoadState>
      ) : (
        <SettingsLoadState kind="loading" title="Loading recent runs…" testId="heartbeats-runs-loading" />
      )
    ) : fires.length === 0 ? (
      <SettingsLoadState kind="empty" title="No runs yet" testId="heartbeats-runs-empty">
        Each time a heartbeat is due, what K2 did shows here: ran, skipped or failed, and why.
      </SettingsLoadState>
    ) : undefined

  const runsGroup = (
    <SettingsGroup
      card
      title="Recent runs"
      hint="What K2 did each time a heartbeat was due, newest first. Open a run to see why."
      action={
        fires && fires.length > 0 ? (
          <Segmented<'all' | 'problems'>
            aria-label="Show runs"
            value={firesView}
            onChange={(next) => {
              setFiresView(next)
              setOpenFire(null)
            }}
            options={[
              { value: 'all', label: 'All' },
              { value: 'problems', label: problemCount > 0 ? `Problems (${problemCount})` : 'Problems' },
            ]}
          />
        ) : undefined
      }
      state={firesState}
      testId="heartbeats-runs"
    >
      {shown.length === 0 ? (
        <div className="px-4 py-6">
          <p className={SETTING_ROW_LABEL}>No problems</p>
          <p className={SETTING_ROW_HINT}>Every run in the last {fires?.length ?? 0} ran or was skipped on purpose.</p>
        </div>
      ) : (
        <ul className="max-h-[32rem] overflow-y-auto divide-y divide-[var(--color-border)]" data-testid="heartbeats-runs-rows">
          {shown.map((fire) => (
            <FireRow key={fire.id} fire={fire} open={openFire === fire.id} onToggle={() => setOpenFire((id) => (id === fire.id ? null : fire.id))} />
          ))}
        </ul>
      )}
      <div className="px-4 py-2.5">
        {firesError ? (
          <p role="alert" className="type-hint text-[var(--color-text-primary)] flex items-center gap-2">
            <span aria-hidden="true" className="inline-block h-2 w-2 rounded-[var(--radius-status)] bg-[var(--color-status-error)]" />
            <span>
              Stopped updating; showing the last runs K2 read.{' '}
              <span className="text-[var(--color-text-secondary)]">The server said: {firesError.replace(/[.\s]+$/, '')}.</span>
            </span>
          </p>
        ) : (
          <p className="type-hint text-[var(--color-text-secondary)]">
            {firesView === 'problems' ? `${shown.length} of the last ${fires?.length ?? 0} runs` : `The last ${fires?.length ?? 0} runs`}. Updates every few
            seconds.
          </p>
        )}
      </div>
    </SettingsGroup>
  )

  return (
    <div data-settings-id="heartbeats" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
          Heartbeats
          <BetaTag />
        </h2>
        <p className={SETTING_ROW_HINT}>
          Heartbeats wake your agents on a schedule. Here you see every one on this server, pause or resume them, and
          find out why one didn’t run.
        </p>
      </header>
      {statusGroup}
      {listGroup}
      {runsGroup}
    </div>
  )
}
