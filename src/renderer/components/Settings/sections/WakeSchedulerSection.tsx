import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useToastStore } from '@/stores/toast'
// Phase 2 Unit 7a — settings live in the daemon.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import type { SettingEntry } from '../searchManifest'
import { WakeupEditor, type HeartbeatRow } from './HeartbeatsSection'

export const WAKE_SCHEDULER_MANIFEST: SettingEntry[] = [
  {
    id: 'wake-scheduler.mode',
    section: 'wake-scheduler',
    label: 'Heartbeat Wake Mode',
    description: 'Off / on-demand / scheduled heartbeat wakes',
    keywords: ['wake', 'heartbeat', 'scheduler', 'launchd', 'sleep', 'lid'],
  },
  {
    id: 'wake-scheduler.interval',
    section: 'wake-scheduler',
    label: 'Wake Interval',
    description: 'Minutes between scheduled heartbeat fires',
    keywords: ['interval', 'minutes', 'cadence', 'frequency'],
  },
  {
    id: 'wake-scheduler.wake-system',
    section: 'wake-scheduler',
    label: 'Wake System From Sleep',
    description: 'Let launchd wake a sleeping laptop to fire heartbeats (lid-closed overnight work)',
    keywords: ['wake', 'sleep', 'lid', 'overnight', 'battery', 'wakesystem'],
  },
]

type WakeMode = 'off' | 'on_demand' | 'heartbeat'

interface WakeSchedulerSettings {
  mode: WakeMode
  intervalMinutes: number
  wakeSystem: boolean
}

interface AppSettingsShape {
  wakeScheduler?: WakeSchedulerSettings
}

const DEFAULT_SETTINGS: WakeSchedulerSettings = {
  mode: 'on_demand',
  intervalMinutes: 5,
  wakeSystem: false,
}

const MODE_DESCRIPTIONS: Record<WakeMode, string> = {
  off:
    'No launchd plist. Heartbeats only fire while K2 is open. Agents sit idle when you quit.',
  on_demand:
    'Heartbeats fire while K2 is open. The daemon stays running in the background after you quit, but the system will not wake itself.',
  heartbeat:
    'launchd fires scheduled heartbeats every N minutes. With "Wake System From Sleep" on, the laptop wakes from sleep (lid closed, on battery) to run agents — the configuration that makes overnight agent work possible.',
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
  if (decision === 'error') return { label: '!', color: 'text-[var(--color-bad,#ef6f6f)]' }
  if (decision === 'wakeup_file_missing') return { label: '!', color: 'text-[var(--color-bad,#ef6f6f)]' }
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
  switch (frequency) {
    case 'daily': return `Every day${at}`
    case 'weekly': return `${(spec.days as string[] | undefined ?? []).join(', ') || '—'}${at}`
    case 'monthly': return `Day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}`
    case 'yearly': return `${(spec.months as string[] | undefined ?? []).join(', ')} day(s) ${(spec.days_of_month as number[] | undefined ?? []).join(', ') || '—'}${at}`
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
  const [loaded, setLoaded] = useState(false)
  const [settings, setSettings] = useState<WakeSchedulerSettings>(DEFAULT_SETTINGS)
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
  // Last successfully-persisted snapshot. `dirty` is computed from
  // deep-equality against this — there's no separate `setDirty` flag
  // that can drift out of sync with the actual on-disk state. Apply
  // updates this AFTER the invoke succeeds; any later edit that
  // matches the snapshot byte-for-byte clears the indicator.
  const [lastApplied, setLastApplied] = useState<WakeSchedulerSettings>(DEFAULT_SETTINGS)
  const [applying, setApplying] = useState(false)
  const toast = useToastStore((s) => s.push)

  const dirty = useMemo(
    () => JSON.stringify(settings) !== JSON.stringify(lastApplied),
    [settings, lastApplied],
  )

  // Load settings on mount.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const app = (await settingsGet()) as unknown as AppSettingsShape
        if (cancelled) return
        const cur = app.wakeScheduler ?? DEFAULT_SETTINGS
        const normalized: WakeSchedulerSettings = {
          mode: (cur.mode as WakeMode) ?? 'on_demand',
          intervalMinutes: cur.intervalMinutes ?? 5,
          wakeSystem: cur.wakeSystem ?? false,
        }
        setSettings(normalized)
        setLastApplied(normalized)
      } catch {
        setSettings(DEFAULT_SETTINGS)
        setLastApplied(DEFAULT_SETTINGS)
      } finally {
        if (!cancelled) setLoaded(true)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const update = useCallback((patch: Partial<WakeSchedulerSettings>) => {
    setSettings((s) => ({ ...s, ...patch }))
  }, [])

  // 0.38.3 — load system-wide heartbeats for the right-column list.
  // Sort case-insensitively by project name, then heartbeat name —
  // the backend ORDER BY uses default collation (case-sensitive ASCII)
  // so uppercase-prefixed workspaces clump above lowercase-prefixed
  // ones. Override here for natural alphabetical reading.
  const refreshHeartbeats = useCallback(async () => {
    setHeartbeatsLoading(true)
    try {
      const rows = await invoke<SystemHeartbeatRow[]>('k2so_heartbeat_list_all')
      const sorted = [...rows].sort((a, b) => {
        const byProj = a.projectName.toLowerCase().localeCompare(b.projectName.toLowerCase())
        if (byProj !== 0) return byProj
        return a.name.toLowerCase().localeCompare(b.name.toLowerCase())
      })
      setHeartbeats(sorted)
    } catch (err) {
      toast(`Failed to load heartbeats: ${String(err)}`, 'error')
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
        const rows = await invoke<SystemFireRow[]>('k2so_heartbeat_fires_list_all', { limit: 100 })
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

  const handleToggleEnabled = useCallback(
    async (row: SystemHeartbeatRow, next: boolean) => {
      // Optimistic — flip locally, fall back if the daemon rejects.
      setHeartbeats((rows) =>
        rows.map((r) => (r.id === row.id ? { ...r, enabled: next } : r)),
      )
      try {
        await daemonCliGet('heartbeat/enable', {
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

  const handleToggleUseWorkspaceSession = useCallback(
    async (row: SystemHeartbeatRow, next: boolean) => {
      setHeartbeats((rows) =>
        rows.map((r) =>
          r.id === row.id ? { ...r, useWorkspaceSession: next } : r,
        ),
      )
      try {
        await daemonCliGet('heartbeat/set-use-workspace-session', {
          project: row.projectPath,
          name: row.name,
          enabled: next,
        })
      } catch (err) {
        toast(`Pinned-chat toggle failed: ${String(err)}`, 'error')
        setHeartbeats((rows) =>
          rows.map((r) =>
            r.id === row.id ? { ...r, useWorkspaceSession: !next } : r,
          ),
        )
      }
    },
    [toast],
  )

  const handleEditWakeup = useCallback((row: SystemHeartbeatRow) => {
    setEditingHeartbeat(row)
  }, [])

  const handleApply = useCallback(async () => {
    setApplying(true)
    try {
      await settingsUpdate({
        wakeScheduler: {
          mode: settings.mode,
          intervalMinutes: settings.intervalMinutes,
          wakeSystem: settings.wakeSystem,
        },
      })
      const msg = await invoke<string>('k2so_agents_apply_wake_scheduler')
      toast(msg, 'success')
      // Snapshot the just-persisted shape. Any subsequent edit that
      // happens to bring `settings` back to this value clears the
      // dirty indicator — the previous boolean-flag implementation
      // couldn't do that.
      setLastApplied(settings)
    } catch (err) {
      toast(`Failed to apply: ${String(err)}`, 'error')
    } finally {
      setApplying(false)
    }
  }, [settings, toast])

  if (!loaded) {
    return <div className="text-[10px] text-[var(--color-text-muted)]">Loading…</div>
  }

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
    // 0.38.3 — Two-column layout: left = wake-scheduler launchd mode
    // (the existing settings); right = system-wide heartbeat list with
    // per-row enable toggle, pinned-chat checkbox, and edit-wakeup
    // button. The right column inherits the same parent container so
    // the page just spreads naturally on wider Settings panes.
    <div data-settings-id="heartbeats" className="flex gap-8 items-start">
      {/* ── Left column: launchd plist mode ─────────────────────────── */}
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
        How launchd fires heartbeats — and whether it wakes your laptop from sleep
        to do it. Configures{' '}
        <code className="text-[var(--color-text-secondary)] font-mono">
          ~/Library/LaunchAgents/com.k2so.agent-heartbeat.plist
        </code>
        . Heartbeat schedules themselves are configured per-workspace in{' '}
        <span className="text-[var(--color-text-secondary)]">Workspaces → Heartbeats</span>.
      </p>

      {/* Mode — bottom-border only renders when the heartbeat-only
          Interval + Wake-system rows appear below, so we don't get
          a stray divider above the Apply button when those rows
          are hidden. The radio cards already have their own outer
          borders, so no separator is needed at the bottom of the
          Mode group itself. */}
      <div
        data-settings-id="wake-scheduler.mode"
        className={`py-2 ${settings.mode === 'heartbeat' ? 'border-b border-[var(--color-border)]' : ''}`}
      >
        <div className="text-xs text-[var(--color-text-secondary)] mb-2">Mode</div>
        <div className="space-y-1">
          {(['off', 'on_demand', 'heartbeat'] as WakeMode[]).map((mode) => (
            <label
              key={mode}
              className={`flex cursor-pointer items-start gap-2 px-2 py-2 border transition-colors no-drag ${
                settings.mode === mode
                  ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/8'
                  : 'border-[var(--color-border)] hover:border-[var(--color-text-secondary)]'
              }`}
            >
              <input
                type="radio"
                name="wake-mode"
                checked={settings.mode === mode}
                onChange={() => update({ mode })}
                className="mt-0.5"
              />
              <div className="flex-1 min-w-0">
                <div className="text-xs text-[var(--color-text-secondary)]">
                  {mode === 'off' && 'Off'}
                  {mode === 'on_demand' && 'On-demand while app open'}
                  {mode === 'heartbeat' && 'Heartbeat every N minutes'}
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                  {MODE_DESCRIPTIONS[mode]}
                </div>
              </div>
            </label>
          ))}
        </div>
      </div>

      {/* Interval — only when mode=heartbeat */}
      {settings.mode === 'heartbeat' && (
        <>
          <div
            data-settings-id="wake-scheduler.interval"
            className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
          >
            <div className="flex-1 min-w-0 mr-3">
              <span className="text-xs text-[var(--color-text-secondary)]">Interval</span>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                Minutes between fires (1–1440). Lower intervals burn more battery; 1–5 minutes balances responsiveness and power.
              </p>
            </div>
            <input
              type="number"
              min={1}
              max={1440}
              value={settings.intervalMinutes}
              onChange={(e) =>
                update({
                  intervalMinutes: Math.max(
                    1,
                    Math.min(1440, parseInt(e.target.value, 10) || 1),
                  ),
                })
              }
              className="w-20 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-primary)] no-drag"
            />
          </div>

          {/* Wake system from sleep */}
          <div
            data-settings-id="wake-scheduler.wake-system"
            className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
          >
            <div className="flex-1 min-w-0 mr-3">
              <span className="text-xs text-[var(--color-text-secondary)]">
                Wake system from sleep
              </span>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed">
                Adds <code className="font-mono text-[var(--color-text-secondary)]">WakeSystem: true</code> to the plist (the same mechanism Time Machine uses for battery-powered hourly backups). When off, scheduled fires run on the next user-initiated wake.
              </p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={settings.wakeSystem}
              onClick={() => update({ wakeSystem: !settings.wakeSystem })}
              className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 ${
                settings.wakeSystem
                  ? 'bg-[var(--color-accent)]'
                  : 'bg-[var(--color-border)]'
              }`}
            >
              <span
                className={`w-2.5 h-2.5 bg-white block transition-transform ${
                  settings.wakeSystem ? 'translate-x-3.5' : 'translate-x-0.5'
                }`}
              />
            </button>
          </div>
        </>
      )}

      {/* Apply — no extra border on this row; the preceding setting
          row already carries its own bottom border, so adding a
          top-border here would render as a doubled separator. */}
      <div className="flex items-center gap-3 mt-4">
        <button
          type="button"
          onClick={handleApply}
          disabled={!dirty || applying}
          className="px-3 py-1 text-xs font-medium text-white bg-[var(--color-accent)] hover:opacity-90 transition-opacity cursor-pointer no-drag disabled:opacity-30 disabled:cursor-not-allowed"
        >
          {applying ? 'Applying…' : 'Apply'}
        </button>
        {dirty && !applying && (
          <span className="text-[10px] text-[var(--color-text-muted)]">Unsaved changes</span>
        )}
      </div>
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
          enable/disable, check the box to route this heartbeat&apos;s
          wake into the workspace&apos;s pinned chat tab instead of its
          own session, and click <span className="text-[var(--color-text-secondary)]">Edit Wakeup</span> to
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
              </div>
              {/* Pinned-chat checkbox — themed to match the rest of
                  settings (the native input's blue square didn't fit
                  the dark + accent palette). Native input is the source
                  of truth; the visible square is a sibling that flips
                  state via the `peer-checked` Tailwind variant. */}
              <label
                className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)] no-drag cursor-pointer select-none flex-shrink-0"
                title="Deliver this heartbeat's wakeup into the workspace's pinned chat session instead of its own"
              >
                <input
                  type="checkbox"
                  checked={row.useWorkspaceSession}
                  onChange={(e) => handleToggleUseWorkspaceSession(row, e.target.checked)}
                  className="peer sr-only"
                />
                <span
                  aria-hidden="true"
                  className="
                    w-3 h-3 flex items-center justify-center border transition-colors
                    border-[var(--color-border)] bg-[var(--color-bg-elevated)]
                    peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)]
                    peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)]
                  "
                >
                  {row.useWorkspaceSession && (
                    <svg
                      viewBox="0 0 12 12"
                      className="w-2.5 h-2.5"
                      fill="none"
                      stroke="white"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <path d="M2.5 6.5 L5 9 L9.5 3.5" />
                    </svg>
                  )}
                </span>
                Pinned&nbsp;chat
              </label>
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
                  className={`w-2.5 h-2.5 bg-white block transition-transform ${
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
  )
}
