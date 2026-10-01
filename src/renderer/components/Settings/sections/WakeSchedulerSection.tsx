// Settings is about the window's server: heartbeats open in the primary
// room's tabs.
import { useTabsStore } from '@/stores/tabs'
import React, { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore } from '@/stores/connect-host'
import { useToastStore } from '@/stores/toast'
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
import { describeHeartbeatWait } from '@/lib/heartbeat-wait'

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
}

/** Decision → terse one-letter chip color. `fired` is the happy path; all
 *  the `skipped_*` decisions are routine non-events; `error` is the only
 *  one that should pop on visual scan. */
function decisionStyle(decision: string): { label: string; color: string } {
  if (decision === 'fired') return { label: '●', color: 'text-[var(--color-good,#57c98a)]' }
  // Catch-up fires are successes for a previously-missed occurrence —
  // amber dot so a recovered miss is scannable.
  if (decision === 'fired_catchup') return { label: '●', color: 'text-[var(--color-status-warn-amber)]' }
  if (
    decision === 'error' ||
    decision === 'wakeup_file_missing' ||
    decision === 'schedule_invalid' ||
    decision === 'auto_disabled' ||
    decision === 'auto_disabled_failing' ||
    decision === 'tick_gap'
  )
    return { label: '!', color: 'text-[var(--color-bad,#ef6f6f)]' }
  if (decision === 'not_due') return { label: '·', color: 'text-[var(--color-text-muted)]' }
  return { label: '○', color: 'text-[var(--color-text-muted)]' }
}

/** "2026-05-19T07:01:23.456Z" → "07:01" today, "May 18 21:13" earlier, etc. */
function formatFiredAt(iso: string): string {
  const d = new Date(iso)
  if (isNaN(d.getTime())) return iso.slice(0, 16)
  const now = new Date()
  const sameDay = d.toDateString() === now.toDateString()
  const hh = d.getHours().toString().padStart(2, '0')
  const mm = d.getMinutes().toString().padStart(2, '0')
  if (sameDay) return `${hh}:${mm}`
  const mon = d.toLocaleString('en-US', { month: 'short' })
  return `${mon} ${d.getDate()} ${hh}:${mm}`
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
  const since = (iso: string | null | undefined): string =>
    iso ? `since ${new Date(iso).toLocaleString()}` : '— no check has ever been recorded'
  if (status.ticker) {
    const stale = status.ticker.staleSecs == null || status.ticker.staleSecs > 180
    if (!stale) return null
    return `Heartbeats are not being checked ${since(status.ticker.lastTickAt)}. The K2 daemon checks every minute while this computer is awake; it may be stopped or restarting. Missed heartbeats under 12 hours late fire once when it is back.`
  }
  const stale = status.staleSecs == null || status.staleSecs > 180
  if (!stale) return null
  return `Heartbeat ticks are not arriving ${since(status.lastTickAt)}. Update K2 on this server: newer versions check heartbeats on their own every minute.`
}

/** Same badge HeartbeatsSection shows: the shared wait formatter. */
function systemRowErrorBadge(row: SystemHeartbeatRow): string | null {
  return describeHeartbeatWait(row)
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
    case 'weekly': return `${(spec.days as string[] | undefined ?? []).join(', ') || '—'}${at}${win}`
    case 'monthly': return `Day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}${win}`
    case 'yearly': return `${(spec.months as string[] | undefined ?? []).join(', ')} day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}${win}`
    case 'hourly': {
      const every = (spec.every_seconds as number | undefined) ?? 3600
      const start = fmt12((spec.start as string | undefined) ?? '00:00')
      const end = fmt12((spec.end as string | undefined) ?? '23:59')
      return `Every ${Math.round(every / 60)}min ${start}–${end}`
    }
    default: return frequency
  }
}

export function WakeSchedulerSection(): React.JSX.Element {
  // 0.38.3 — system-wide heartbeat list rendered in the right column.
  // Loads on mount + refreshes after any per-row toggle so the on-disk
  // state and the visible state stay in lockstep.
  const [heartbeats, setHeartbeats] = useState<SystemHeartbeatRow[]>([])
  const [heartbeatsLoading, setHeartbeatsLoading] = useState(true)
  // When non-null, render the `WakeupEditor` overlay (AIFileEditor +
  // live preview, scoped to this heartbeat's WAKEUP.md). null = list view.
  const [editingHeartbeat, setEditingHeartbeat] = useState<SystemHeartbeatRow | null>(null)
  // 0.38.3 — universal audit log (right-most column). Most recent 100
  // fires across all workspaces. Polled every 5s so the user sees
  // newly-firing heartbeats live without having to reopen the page.
  const [fires, setFires] = useState<SystemFireRow[]>([])
  // Scheduler + wake status from the daemon (S2: ticker, wake, awake).
  // Polled every 15s; also refreshed right after the wake switch moves.
  const [schedulerStatus, setSchedulerStatus] = useState<SchedulerStatus | null>(null)
  // The wake switch is daemon truth (`wakeForHeartbeats`); this only
  // tracks a request in flight (the macOS admin dialog can take a while).
  const [wakeSaving, setWakeSaving] = useState(false)
  const toast = useToastStore((s) => s.addToast)

  const refreshStatus = useCallback(async () => {
    const status = await daemonCliGet<SchedulerStatus>(primaryScope(), 'heartbeat/scheduler-status', {})
    setSchedulerStatus(status)
  }, [])

  /** D8 / D11 — flip "Wake this computer for heartbeats" (or "Also on
   *  battery"). The daemon may show one admin dialog on its Mac; if it is
   *  declined the switch stays off and the daemon says why. */
  const handleSetWake = useCallback(
    async (enabled: boolean, onBattery?: boolean) => {
      setWakeSaving(true)
      try {
        const resp = await daemonCliPost<{
          success?: boolean
          wakeForHeartbeats?: boolean
          message?: string
        }>(primaryScope(), 'heartbeat/wake', { enabled, onBattery })
        if (resp?.success === false) {
          toast(resp.message ?? 'Wake stays off.', 'info', 10000)
        }
        await refreshStatus()
      } catch (err) {
        toast(`Failed to change wake: ${String(err)}`, 'error')
      } finally {
        setWakeSaving(false)
      }
    },
    [toast, refreshStatus],
  )

  // 0.38.3 — load system-wide heartbeats for the right-column list.
  // Sort case-insensitively by project name, then heartbeat name —
  // the backend ORDER BY uses default collation (case-sensitive ASCII)
  // so uppercase-prefixed workspaces clump above lowercase-prefixed
  // ones. Override here for natural alphabetical reading.
  const refreshHeartbeats = useCallback(async () => {
    setHeartbeatsLoading(true)
    try {
      // 0.40.48 host-aware: connected to a REMOTE host, the cross-project
      // roster comes from ITS new /cli/heartbeat/list-all route — this
      // page is the fleet-audit surface, so it must show the machine
      // you're connected to (needs the host on >=0.40.48; older hosts
      // error honestly instead of silently showing the wrong machine).
      // LOCAL keeps the in-process command: same machine, same DB, and
      // no dependency on the local daemon having the new route.
      const rows =
        useConnectHostStore.getState().activeHost !== 'local'
          ? await daemonCliGet<SystemHeartbeatRow[]>(primaryScope(), 'heartbeat/list-all')
          : await invoke<SystemHeartbeatRow[]>('k2so_heartbeat_list_all')
      const sorted = [...rows].sort((a, b) => {
        const byProj = a.projectName.toLowerCase().localeCompare(b.projectName.toLowerCase())
        if (byProj !== 0) return byProj
        return a.name.toLowerCase().localeCompare(b.name.toLowerCase())
      })
      setHeartbeats(sorted)
    } catch (err) {
      // Older-host signature: a pre-0.40.48 daemon has no list-all route,
      // so the request falls into its project-scoped catch-all and answers
      // "Missing 'project' (or project_path) parameter" — which reads like
      // a client bug. Translate it into what it actually means.
      const s = useConnectHostStore.getState()
      const oldHost =
        s.activeHost !== 'local' && /missing '?project/i.test(String(err))
      if (oldHost) {
        const v = s.serverVersion ? ` (it's on v${s.serverVersion})` : ''
        toast(
          `Auditing this host's heartbeats from here needs K2 0.40.48+ on the host${v}. ` +
            `Update the host, then reopen this page.`,
          'info',
          10000,
        )
      } else {
        toast(`Failed to load heartbeats: ${String(err)}`, 'error')
      }
      setHeartbeats([])
    } finally {
      setHeartbeatsLoading(false)
    }
  }, [toast])

  useEffect(() => {
    void refreshHeartbeats()
  }, [refreshHeartbeats])

  // 0.38.3 — fires audit log. Polled every 5s so newly-firing
  // heartbeats appear without page reload. Limit 100 keeps the list
  // bounded; older fires fall off the visible window but stay in SQL.
  useEffect(() => {
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        // 0.40.48 host-aware: audit the ACTIVE host's fires (new
        // /cli/heartbeat/fires-list-all route); local keeps in-process.
        const rows =
          useConnectHostStore.getState().activeHost !== 'local'
            ? await daemonCliGet<SystemFireRow[]>(primaryScope(), 'heartbeat/fires-list-all', { limit: 100 })
            : await invoke<SystemFireRow[]>('k2so_heartbeat_fires_list_all', { limit: 100 })
        if (!cancelled) setFires(rows)
      } catch {
        // Silent — the audit log is a read-only convenience; a transient
        // failure shouldn't toast.
      }
    }
    void tick()
    const id = window.setInterval(tick, 5000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const status = await daemonCliGet<SchedulerStatus>(primaryScope(), 'heartbeat/scheduler-status', {})
        if (!cancelled) setSchedulerStatus(status)
      } catch {
        // Silent — health polling must never toast on a transient miss.
      }
    }
    void tick()
    const id = window.setInterval(tick, 15000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [])

  const handleToggleEnabled = useCallback(
    async (row: SystemHeartbeatRow, next: boolean) => {
      // Optimistic — flip locally, fall back if the daemon rejects.
      setHeartbeats((rows) =>
        rows.map((r) => (r.id === row.id ? { ...r, enabled: next } : r)),
      )
      try {
        await daemonCliGet(primaryScope(), 'heartbeat/enable', {
          project: row.projectPath,
          name: row.name,
          enabled: next,
        })
      } catch (err) {
        toast(`Toggle failed for ${row.projectName}/${row.name}: ${String(err)}`, 'error')
        setHeartbeats((rows) =>
          rows.map((r) => (r.id === row.id ? { ...r, enabled: !next } : r)),
        )
      }
    },
    [toast],
  )

  // Delivery drop-down (replaces the pinned-chat checkbox) — persist
  // where this heartbeat's wakeup goes. Optimistic like
  // handleToggleEnabled: flip locally, revert on daemon rejection.
  const handleSetDelivery = useCallback(
    async (row: SystemHeartbeatRow, next: HeartbeatDeliveryTarget) => {
      setHeartbeats((rows) =>
        rows.map((r) => (r.id === row.id ? applyDeliveryTarget(r, next) : r)),
      )
      try {
        // Host-aware default (0.40.48): this page's roster now comes from
        // the ACTIVE host's list-all route, so delivery-target writes go
        // to the same host the rows came from.
        await setHeartbeatSession(row.projectPath, row.name, next)
      } catch (err) {
        toast(`Wakeup delivery change failed for ${row.projectName}/${row.name}: ${String(err)}`, 'error')
        setHeartbeats((rows) =>
          rows.map((r) => (r.id === row.id ? row : r)),
        )
      }
    },
    [toast],
  )

  const handleEditWakeup = useCallback((row: SystemHeartbeatRow) => {
    setEditingHeartbeat(row)
  }, [])

  // 0.38.3 — when the user clicks "Edit Wakeup" on a row, render the
  // WakeupEditor (AIFileEditor) takeover. The agent-name resolution
  // uses the project-name slug as a best-effort fallback; the proper
  // resolver lives in ProjectsSection (`primaryAgentName` useEffect)
  // and post-0.39.0f Phase 2.1 calls `k2so_workspace_agent_display_name`.
  // This panel renders the system-wide heartbeat list and doesn't have
  // a per-row resolved name handy, so the slug is the cheap fallback
  // for now — WakeupEditor's AGENT.md fetch fails-soft on mismatch.
  if (editingHeartbeat) {
    // Convert SystemHeartbeatRow → HeartbeatRow shape WakeupEditor expects
    const hb: HeartbeatRow = {
      id: editingHeartbeat.id,
      projectId: editingHeartbeat.projectId,
      name: editingHeartbeat.name,
      frequency: editingHeartbeat.frequency,
      specJson: editingHeartbeat.specJson,
      wakeupPath: editingHeartbeat.wakeupPath,
      enabled: editingHeartbeat.enabled,
      lastFired: editingHeartbeat.lastFired,
      createdAt: 0,
      useWorkspaceSession: editingHeartbeat.useWorkspaceSession,
      lastSessionId: editingHeartbeat.lastSessionId,
      sessionProvider: editingHeartbeat.sessionProvider,
      consecutiveFailures: editingHeartbeat.consecutiveFailures,
      nextRetryAt: null,
      disabledReason: editingHeartbeat.disabledReason,
      scheduleError: editingHeartbeat.scheduleError,
    }
    const slug = editingHeartbeat.projectName.toLowerCase().replace(/\s+/g, '-')
    const otherHeartbeats: HeartbeatRow[] = heartbeats
      .filter((r) => r.projectId === editingHeartbeat.projectId && r.id !== editingHeartbeat.id)
      .map((r) => ({
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
      }))
    return (
      <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
        <WakeupEditor
          projectPath={editingHeartbeat.projectPath}
          agentName={slug}
          heartbeat={hb}
          otherHeartbeats={otherHeartbeats}
          onClose={() => {
            setEditingHeartbeat(null)
            void refreshHeartbeats()
          }}
        />
      </div>
    )
  }

  return (
    // 0.38.3 — Two-column layout: left = how heartbeats fire and the
    // S2 wake switch; right = system-wide heartbeat list with
    // per-row enable toggle, pinned-chat checkbox, and edit-wakeup
    // button. The right column inherits the same parent container so
    // the page just spreads naturally on wider Settings panes.
    <div data-settings-id="heartbeats" className="flex flex-col gap-4">
      {/* Heartbeat S2 — the daemon's own 60 s check has stopped. Above
          both columns: nothing below fires while it is down. */}
      {tickerDownMessage(schedulerStatus) && (
        <div className="border border-[var(--color-bad,#ef6f6f)]/60 bg-[var(--color-bad,#ef6f6f)]/10 px-3 py-2 text-[11px] text-[var(--color-bad,#ef6f6f)] leading-relaxed">
          {tickerDownMessage(schedulerStatus)}
        </div>
      )}
      <div className="flex gap-8 items-start">
      {/* ── Left column: when heartbeats fire, and waking ──────────── */}
      <div className="w-1/3 min-w-[280px] max-w-[420px] flex-shrink-0">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        Heartbeats
        <span
          className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
          title="This feature is in beta — interface and behavior may change"
        >
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed">
        K2 checks your heartbeats every minute while this computer is awake, with or
        without the app open. A heartbeat missed while the computer slept fires once
        when it wakes, if it is less than 12 hours late; older ones are skipped and
        logged. Heartbeat schedules themselves are set per workspace in{' '}
        <span className="text-[var(--color-text-secondary)]">Workspaces → Heartbeats</span>.
      </p>

      {schedulerStatus && !schedulerStatus.wake && (
        <p className="text-[10px] text-[var(--color-text-muted)] py-2 border-b border-[var(--color-border)] leading-relaxed">
          This server runs an older K2 that cannot wake the computer for heartbeats.
          Update K2 on it to use this setting.
        </p>
      )}

      {schedulerStatus?.wake && (() => {
        const wake = schedulerStatus.wake
        const on = schedulerStatus.wakeForHeartbeats === true
        const onBattery = schedulerStatus.wakeOnBattery === true
        const floor = wake.batteryFloorPercent ?? 20
        const note = wakePlatformNote(wake.platform)
        return (
          <>
            <div
              data-settings-id="wake-scheduler.wake"
              className="flex items-start justify-between py-2 border-b border-[var(--color-border)]"
            >
              <div className="flex-1 min-w-0 mr-3">
                <span className="text-xs text-[var(--color-text-secondary)]">
                  Wake this computer for heartbeats
                </span>
                <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                  {on
                    ? 'On: K2 wakes this computer a minute before the next heartbeat.'
                    : 'Off: K2 does not wake this computer from sleep. Heartbeats still fire whenever it is awake.'}
                  {note ? ` ${note}` : ''}
                </p>
                {wakeSaving && wake.platform === 'macos' && !on && (
                  <p className="text-[10px] text-[var(--color-text-secondary)] mt-1">
                    Approve the admin dialog on this Mac to finish turning wake on.
                  </p>
                )}
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={on}
                aria-label="Wake this computer for heartbeats"
                disabled={wakeSaving}
                onClick={() => void handleSetWake(!on)}
                className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-40 disabled:cursor-wait ${
                  on ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                    on ? 'translate-x-3.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>

            {on && (
              <label
                data-settings-id="wake-scheduler.battery"
                className="flex items-start gap-2 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag"
              >
                <input
                  type="checkbox"
                  checked={onBattery}
                  disabled={wakeSaving}
                  onChange={(e) => void handleSetWake(true, e.target.checked)}
                  className="mt-0.5"
                />
                <div className="flex-1 min-w-0">
                  <span className="text-xs text-[var(--color-text-secondary)]">Also on battery</span>
                  <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                    Without this, K2 wakes the computer only while it is plugged in. Never below {floor}% battery.
                  </p>
                </div>
              </label>
            )}

            {on && (
              <div className="py-2 text-[10px] leading-relaxed">
                {wake.state === 'ok' && wake.nextWakeAt && (
                  <span className="text-[var(--color-text-secondary)]">
                    Next wake {formatClock(wake.nextWakeAt)}
                    {wake.nextFireAt ? `, for a heartbeat at ${formatClock(wake.nextFireAt)}` : ''}.
                  </span>
                )}
                {wake.state === 'ok' && !wake.nextWakeAt && (
                  <span className="text-[var(--color-text-muted)]">No heartbeat is scheduled, so no wake is set.</span>
                )}
                {(wake.state === 'unavailable' || wake.state === 'paused') && (
                  <span className="text-[var(--color-status-warn-amber)]">
                    {wake.state === 'paused' ? 'Paused: ' : 'Not available: '}
                    {wake.reason}
                  </span>
                )}
                {wake.notes?.wakeTimers && (
                  <p className="text-[var(--color-text-muted)] mt-1">
                    Wake timers in this power plan: plugged in {wake.notes.wakeTimers.pluggedIn.replace('_', ' ')},
                    on battery {wake.notes.wakeTimers.onBattery.replace('_', ' ')}.
                    {wake.notes.steps ? ` To allow them: ${wake.notes.steps}` : ''}
                  </p>
                )}
              </div>
            )}

            {schedulerStatus.awake?.held && (
              <p className="text-[10px] text-[var(--color-text-muted)] py-1">
                Keeping this computer awake now: {schedulerStatus.awake.reasons.join(', ')}.
              </p>
            )}
          </>
        )
      })()}
      </div>
      {/* ── Right column: every heartbeat across all workspaces ──────── */}
      <div className="flex-1 min-w-0 max-w-[640px]">
        <div className="flex items-baseline justify-between mb-1">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            All Heartbeats
          </h2>
          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">
            {heartbeatsLoading ? 'loading…' : `${heartbeats.length} total`}
          </span>
        </div>
        <p className="text-[10px] text-[var(--color-text-muted)] mb-3 leading-relaxed">
          Every active heartbeat across every workspace. Toggle to
          enable/disable, pick where each heartbeat&apos;s wake is
          delivered (the workspace&apos;s pinned chat, its own session,
          or a saved session), and click <span className="text-[var(--color-text-secondary)]">Edit Wakeup</span> to
          open the prompt template in your default editor.
        </p>
        {heartbeats.length === 0 && !heartbeatsLoading && (
          <div className="text-[10px] text-[var(--color-text-muted)] italic py-3">
            No heartbeats configured. Add one inside any workspace at
            <span className="text-[var(--color-text-secondary)]"> Workspaces → Heartbeats</span>.
          </div>
        )}
        <div className="space-y-1">
          {heartbeats.map((row) => (
            <div
              key={row.id}
              className="border border-[var(--color-border)] hover:border-[var(--color-text-secondary)] transition-colors px-3 py-2 flex items-center gap-3"
            >
              <div className="flex-1 min-w-0">
                <div className="text-xs text-[var(--color-text-secondary)] flex items-baseline gap-2">
                  <span className="text-[var(--color-text-primary)] font-medium truncate">
                    {row.projectName}
                  </span>
                  <span className="text-[var(--color-text-muted)]">/</span>
                  <span className="truncate">{row.name}</span>
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
                  {describeHeartbeatSpec(row.specJson, row.frequency)}
                </div>
                {systemRowErrorBadge(row) && (
                  <div
                    className="text-[10px] text-[var(--color-bad,#ef6f6f)] mt-0.5 truncate"
                    title={row.scheduleError ?? undefined}
                  >
                    {systemRowErrorBadge(row)}
                  </div>
                )}
              </div>
              {/* Delivery drop-down (replaces the pinned-chat checkbox)
                  — pinned chat / own session / a trained saved session
                  — plus an open-target button that jumps to the
                  heartbeat's chat (drawer-row-click flow). */}
              <div className="flex items-center gap-0.5 flex-shrink-0">
                <HeartbeatSessionPicker
                  projectPath={row.projectPath}
                  projectId={row.projectId}
                  value={deriveDeliveryTarget(row)}
                  onSelect={(next) => { void handleSetDelivery(row, next) }}
                />
                <button
                  type="button"
                  onClick={() => { void openHeartbeatTarget(useTabsStore, row.projectPath, row.name, deriveDeliveryTarget(row).mode) }}
                  title={
                    deriveDeliveryTarget(row).mode === 'pinned'
                      ? 'Open the workspace’s pinned chat tab'
                      : 'Open this heartbeat’s chat session'
                  }
                  aria-label="Open this heartbeat's wakeup destination"
                  className="inline-flex items-center justify-center h-4 w-4 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer flex-shrink-0"
                >
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M15 3h6v6" />
                    <path d="M10 14L21 3" />
                    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                  </svg>
                </button>
              </div>
              {/* Edit-wakeup */}
              <button
                type="button"
                onClick={() => handleEditWakeup(row)}
                className="text-[10px] px-2 py-1 border border-[var(--color-border)] hover:bg-white/[0.04] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag flex-shrink-0"
                title={`Open ${row.wakeupPath}`}
              >
                Edit Wakeup
              </button>
              {/* Enable toggle */}
              <button
                type="button"
                role="switch"
                aria-checked={row.enabled}
                onClick={() => handleToggleEnabled(row, !row.enabled)}
                title={row.enabled ? 'Enabled — click to disable' : 'Disabled — click to enable'}
                className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                  row.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
                }`}
              >
                <span
                  className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                    row.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
                  }`}
                />
              </button>
            </div>
          ))}
        </div>
      </div>
      {/* ── Right-most column: universal fire audit log ──────────────── */}
      <div className="flex-1 min-w-0 max-w-[420px]">
        <div className="flex items-baseline justify-between mb-1">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Recent Fires
          </h2>
          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">
            last {fires.length}
          </span>
        </div>
        <p className="text-[10px] text-[var(--color-text-muted)] mb-3 leading-relaxed">
          Live audit feed of every scheduler decision across all
          workspaces — fires, skips, errors. Updates every 5s. Hover
          a row for the full skip reason.
        </p>
        <div className="space-y-0.5 max-h-[600px] overflow-y-auto">
          {fires.length === 0 && (
            <div className="text-[10px] text-[var(--color-text-muted)] italic py-3">
              No fire decisions recorded yet. Heartbeats will appear
              here on their next scheduler tick.
            </div>
          )}
          {fires.map((fire) => {
            const style = decisionStyle(fire.decision)
            return (
              <div
                key={fire.id}
                className="px-2 py-1 flex items-center gap-2 hover:bg-white/[0.03] transition-colors text-[10px] font-mono"
                title={fire.reason ? `${fire.decision} — ${fire.reason}` : fire.decision}
              >
                <span className={`flex-shrink-0 ${style.color}`} aria-hidden="true">
                  {style.label}
                </span>
                <span className="flex-shrink-0 text-[var(--color-text-muted)] tabular-nums w-[68px]">
                  {formatFiredAt(fire.firedAt)}
                </span>
                <span className="flex-1 min-w-0 truncate text-[var(--color-text-secondary)]">
                  <span className="text-[var(--color-text-primary)]">{fire.projectName}</span>
                  {fire.scheduleName && (
                    <>
                      <span className="text-[var(--color-text-muted)]"> / </span>
                      <span>{fire.scheduleName}</span>
                    </>
                  )}
                </span>
                <span className="flex-shrink-0 text-[var(--color-text-muted)] uppercase tracking-wider text-[8px]">
                  {fire.decision === 'fired' ? 'fired' : fire.decision.replace(/^skipped_/, '')}
                </span>
              </div>
            )
          })}
        </div>
      </div>
      </div>
    </div>
  )
}
