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
import { describeHeartbeatStatus, spanLabel } from '@/lib/heartbeat-wait'
import { localWhen } from '../shared/local-time'
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
      return 'The first time, macOS asks once for an admin password to add a small helper. With the lid closed, it wakes only while plugged in.'
    case 'linux':
      return 'Uses the wake alarm permission the K2 package sets up. Without it, K2 shows the one command to run.'
    case 'windows':
      return 'Windows wakes for this only when the power plan allows wake timers. K2 shows the setting below and never changes it.'
    default:
      return ''
  }
}

/** Every time on this page reads the same way ("Nov 10, 5:39 PM"). */
function formatClock(iso: string | null | undefined): string {
  return iso ? localWhen(iso) : ''
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
/** A reply K2 couldn't read (the wrong shape). The page says so in its own
 *  words; there is no server reason to show. */
class UnreadableReply extends Error {}

export function listReply<T>(raw: unknown): T[] {
  if (Array.isArray(raw)) return raw as T[]
  throw new UnreadableReply('K2 couldn’t read the list')
}

/** The scheduler status is an object; anything else is a failed read. */
function statusReply(raw: unknown): SchedulerStatus {
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) return raw as SchedulerStatus
  throw new UnreadableReply('K2 couldn’t read the status')
}

/** The server's own reason, or '' when there is none to show. */
function errorText(err: unknown): string {
  if (err instanceof UnreadableReply) return ''
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
  return raw.trim().replace(/[.\s]+$/, '')
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

/** A run's reason in plain words, with the next step where there is one.
 *  The scheduler's own words stay in the opened run. */
export function explainRun(fire: { decision: string; reason: string | null }): { text: string; next?: string; missingFile?: boolean } | null {
  const reason = (fire.reason ?? '').trim()
  const hours = /(\d+)\s*h(?:ours?)?\s+late/i.exec(reason)?.[1]
  const late = hours ? `${hours} ${hours === '1' ? 'hour' : 'hours'} late` : 'late'
  const slept = /slept|sleep|asleep/i.test(reason)
  if (fire.decision === 'wakeup_file_missing' || /WAKEUP\.md (not found|missing)|wakeup file is missing/i.test(reason))
    return { text: 'Its instructions file is missing.', next: 'Edit its instructions to add them.', missingFile: true }
  if (fire.decision === 'skipped_empty' || /WAKEUP\.md is empty/i.test(reason))
    return { text: 'Its instructions file is empty.', next: 'Edit its instructions to add some.', missingFile: true }
  if (fire.decision === 'fired_catchup')
    return { text: `Ran ${late}${slept ? ' because the computer was asleep' : ''}.` }
  if (fire.decision === 'skipped_missed')
    return { text: `Skipped: it was due while the computer ${slept ? 'was asleep' : 'was off'} and is more than 12 hours late.`, next: 'It runs again at its next time.' }
  if (/session exited|exited before/i.test(reason))
    return { text: 'Failed: the agent’s chat closed before it answered.', next: 'It tries again at its next time. If it keeps failing, open its chat to see why.' }
  if (fire.decision === 'skipped_busy')
    return { text: reason ? `Skipped: ${reason.replace(/[.\s]+$/, '')}.` : 'Skipped: the agent was busy.', next: 'It runs again at its next time.' }
  return null
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
type RowNote = { kind: 'saved' | 'error'; text: string; reason?: string }

/** A change that didn't take: what failed, the next step, then the
 *  server's reason quietly. */
function failNote(what: string, err: unknown): RowNote {
  const reason = errorText(err)
  return { kind: 'error', text: `${what}. Nothing changed; try again in a moment.`, ...(reason ? { reason } : {}) }
}

/** Paused by a person reads one way everywhere on this page. */
export const PAUSED_TEXT = 'Paused. It won’t run until you turn it back on.'

const PAUSED_BY_K2: Record<string, string> = {
  failures: 'Paused by K2 after failing several times. See Recent runs, then turn it back on.',
  wakeup_missing: 'Paused by K2: its instructions file is missing. Edit its instructions, then turn it back on.',
  wakeup_empty: 'Paused by K2: its instructions file is empty. Edit its instructions, then turn it back on.',
}

/** The row's status line when this page words it itself: paused by a
 *  person, paused by K2 for a known reason, or the next run in the page's
 *  time format. null = the shared formatter's line (a wait or a problem). */
function rowStatus(row: SystemHeartbeatRow): { text: string; tone: 'next' | 'paused' | 'problem' } | null {
  if (!row.enabled) {
    const why = row.waitReason?.startsWith('disabled:') ? row.waitReason.slice('disabled:'.length) : row.disabledReason
    if (!why || why === 'user') return why === 'user' && row.waitDetail ? null : { text: PAUSED_TEXT, tone: 'paused' }
    return PAUSED_BY_K2[why] ? { text: PAUSED_BY_K2[why]!, tone: 'problem' } : null
  }
  const st = describeHeartbeatStatus(row, { now: Date.now(), nextFire: true })
  if (st?.tone === 'next' && row.nextFireAt) return { text: `Next run: ${localWhen(row.nextFireAt)}`, tone: 'next' }
  return null
}

function NoteReason({ reason }: { reason?: string }): React.JSX.Element | null {
  if (!reason) return null
  return <span className="block mt-1 text-[var(--color-text-muted)]">The server said: {reason}.</span>
}

function FireRow({
  fire,
  open,
  onToggle,
  onEdit,
}: {
  fire: SystemFireRow
  open: boolean
  onToggle: () => void
  /** Opens the heartbeat's instructions, when it still exists. */
  onEdit?: () => void
}): React.JSX.Element {
  const d = describeDecision(fire.decision)
  const why = explainRun(fire)
  const actor = heartbeatHistoryActorLine(fire)
  const headline = why?.text ?? d.text
  const fields: Array<{ label: string; value: string | null | undefined; raw?: boolean }> = [
    { label: 'When', value: formatClock(fire.firedAt) },
    { label: 'Result', value: why ? `${d.text}. ${why.text}` : d.text },
    { label: 'Took', value: fire.durationMs != null ? `${(fire.durationMs / 1000).toFixed(1)} seconds` : null },
    { label: 'Started by', value: actor || null },
    { label: 'Log line', value: [fire.decision, fire.reason].filter(Boolean).join(': '), raw: true },
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
            {headline}
          </span>
          {why?.next && <span className="block type-hint text-[var(--color-text-secondary)] mt-0.5">{why.next}</span>}
        </span>
        <span className="type-hint text-[var(--color-text-secondary)] tabular-nums whitespace-nowrap">{formatClock(fire.firedAt)}</span>
        <span aria-hidden="true" className={`type-row text-[var(--color-text-muted)] group-hover:text-[var(--color-text-primary)] transition-transform ${open ? 'rotate-90' : ''}`}>
          ›
        </span>
      </button>
      {open && (
        <div id={detailId} className="px-4 pb-3 pl-9 space-y-2" data-fire-details="">
          <dl className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-4 gap-y-1">
            {fields
              .filter((f) => f.value != null && f.value !== '')
              .map((f) => (
                <React.Fragment key={f.label}>
                  <dt className="type-hint text-[var(--color-text-secondary)]">{f.label}</dt>
                  <dd className={`type-hint break-words ${f.raw ? 'font-mono text-[var(--color-text-muted)]' : 'text-[var(--color-text-primary)]'}`}>{f.value}</dd>
                </React.Fragment>
              ))}
          </dl>
          {why?.missingFile && onEdit && <Button onClick={onEdit}>Edit instructions</Button>}
        </div>
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
  // Each heartbeat row, so the one that just changed can be brought into
  // view with its result.
  const rowEls = useRef<Record<string, HTMLDivElement | null>>({})
  const [noteFor, setNoteFor] = useState<{ id: string; n: number } | null>(null)
  useEffect(() => {
    if (!noteFor) return
    rowEls.current[noteFor.id]?.scrollIntoView?.({ block: 'nearest' })
  }, [noteFor])

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
    if (note) setNoteFor((f) => ({ id, n: (f?.n ?? 0) + 1 }))
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
          setWakeNote({ kind: 'error', text: 'Wake stays off. Try again; if it keeps failing, check the reason below.', reason: (resp.message ?? '').trim().replace(/[.\s]+$/, '') || undefined })
        } else {
          setWakeNote({ kind: 'saved', text: 'Saved' })
        }
        await refreshStatus()
      } catch (err) {
        setWakeNote(failNote('Couldn’t change wake', err))
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
        if (!cancelled) setFiresError(errorText(err))
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
        if (!cancelled) setStatusError(errorText(err))
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
        noteRow(row.id, { kind: 'saved', text: 'Saved' })
      } catch (err) {
        setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? { ...r, enabled: !next } : r)))
        noteRow(row.id, failNote(`Couldn’t ${next ? 'resume' : 'pause'} ${row.name}`, err))
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
        noteRow(row.id, { kind: 'saved', text: 'Saved' })
      } catch (err) {
        setHeartbeats((rows) => rows && rows.map((r) => (r.id === row.id ? row : r)))
        noteRow(row.id, failNote(`Couldn’t change where ${row.name} runs`, err))
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

  /** The heartbeat a run belongs to, to open its instructions. */
  const editFor = (fire: SystemFireRow): (() => void) | undefined => {
    const row = (heartbeats ?? []).find((r) => r.projectId === fire.projectId && r.name === fire.scheduleName)
    return row ? () => setEditingHeartbeat(row) : undefined
  }

  const tickerDown = tickerDownMessage(schedulerStatus)
  const lastCheck = schedulerStatus?.ticker?.lastTickAt ?? schedulerStatus?.lastDaemonTickAt ?? schedulerStatus?.lastTickAt ?? null

  // ── How heartbeats run: status, wake, battery ──────────────────────
  const statusState =
    schedulerStatus === null ? (
      statusError !== null ? (
        <SettingsLoadState
          kind="error"
          title="The scheduler’s status couldn’t load"
          detail={statusError || null}
          action={<Button onClick={() => setStatusAttempt((n) => n + 1)}>Try again</Button>}
          testId="heartbeats-status-error"
        >
          K2 couldn’t tell whether heartbeats are being checked. Your heartbeats are unchanged. Try again in a moment.
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
                  <NoteReason reason={wakeNote.reason} />
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
              Wake it on battery too, with the lid open, and never below {wake.batteryFloorPercent ?? 20}%. Off: only while it is
              plugged in.
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
        : 'K2 couldn’t read the list of heartbeats on this server. They still run on their schedules. Try again in a moment.'}
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
      hint="Every heartbeat in every workspace. Runs in picks the chat it wakes; Edit instructions changes what it tells the agent; the switch pauses it."
      state={listState}
      testId="heartbeats-list"
    >
      {(heartbeats ?? []).map((row) => {
        const target = deriveDeliveryTarget(row)
        const rowNote = rowNotes[row.id]
        const status = rowStatus(row)
        return (
          <div
            key={row.id}
            ref={(el) => {
              rowEls.current[row.id] = el
            }}
            className="px-4 py-3 scroll-my-6"
            data-testid={`heartbeat-row-${row.id}`}
          >
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0 flex-1">
                <p className="type-row text-[var(--color-text-primary)] truncate">
                  <span className="type-name">{row.name}</span>
                  <span className="text-[var(--color-text-secondary)]"> in {row.projectName}</span>
                </p>
                <p className={SETTING_ROW_HINT}>{describeHeartbeatSpec(row.specJson, row.frequency)}</p>
                {/* Paused by a person, or the next run, read in this page's
                    words and time format; anything else (a wait, a problem)
                    keeps the shared formatter's line. */}
                {status ? (
                  <p
                    className={`type-hint mt-0.5 ${status.tone === 'problem' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}`}
                    data-heartbeat-status={status.tone}
                  >
                    {status.text}
                  </p>
                ) : (
                  <HeartbeatStatusLine row={row} nextFire={nextFire} nextPrefix="Next run: " className="type-hint mt-0.5" />
                )}
                {rowNote?.kind === 'error' && (
                  <p role="alert" data-row-note="error" className="type-hint mt-1 max-w-[75ch] text-[var(--color-status-error-soft)]">
                    {rowNote.text}
                    <NoteReason reason={rowNote.reason} />
                  </p>
                )}
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                <HeartbeatSessionPicker
                  variant="field"
                  prefix="Runs in"
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
                <Button
                  onClick={() => setEditingHeartbeat(row)}
                  title="Edit what it tells the agent each time it runs"
                  aria-label={`Edit the instructions for ${row.name}`}
                >
                  Edit instructions
                </Button>
                {rowNote && (
                  <span
                    role={rowNote.kind === 'saved' ? 'status' : undefined}
                    data-row-note={rowNote.kind === 'saved' ? 'saved' : undefined}
                    className={`type-hint whitespace-nowrap ${rowNote.kind === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-ok-soft)]'}`}
                  >
                    {rowNote.kind === 'error' ? 'Not saved' : rowNote.text}
                  </span>
                )}
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
      firesError !== null ? (
        <SettingsLoadState
          kind="error"
          title="Recent runs couldn’t load"
          detail={firesError || null}
          action={<Button onClick={() => setFiresAttempt((n) => n + 1)}>Try again</Button>}
          testId="heartbeats-runs-error"
        >
          K2 couldn’t read the list of recent runs. Your heartbeats still run. Try again in a moment.
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
            <FireRow
              key={fire.id}
              fire={fire}
              open={openFire === fire.id}
              onToggle={() => setOpenFire((id) => (id === fire.id ? null : fire.id))}
              onEdit={editFor(fire)}
            />
          ))}
        </ul>
      )}
      <div className="px-4 py-2.5">
        {firesError !== null ? (
          <p role="alert" className="type-hint text-[var(--color-text-primary)] flex items-center gap-2">
            <span aria-hidden="true" className="inline-block h-2 w-2 rounded-[var(--radius-status)] bg-[var(--color-status-error)]" />
            <span>
              Stopped updating; showing the last runs K2 read.{' '}
              {firesError && <span className="text-[var(--color-text-muted)]">The server said: {firesError}.</span>}
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
