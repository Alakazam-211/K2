import { useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { emit } from '@tauri-apps/api/event'
// Plan B — projects_update is host-aware daemon data; route through the
// `/cli/projects/update` HTTP layer and re-emit `sync:projects`. The
// `k2so_agents_*` calls are agent-control (out of scope), stay on invoke.
import { daemonCliPost } from '@/lib/daemon-cli'
import { useHeartbeatScheduleStore } from '@/stores/heartbeat-schedule'
import { useProjectsStore } from '@/stores/projects'

// ── Dropdown (matches Settings SettingDropdown style) ──────────────────

function Dropdown({
  value, options, onChange, className,
}: {
  value: string
  options: { value: string; label: string }[]
  onChange: (value: string) => void
  className?: string
}): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)
  const selected = options.find((o) => o.value === value) ?? options[0]

  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) setIsOpen(false)
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  return (
    <div ref={containerRef} className={`relative no-drag ${className ?? ''}`}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-2 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-primary)] transition-colors cursor-pointer"
      >
        <span className="truncate">{selected?.label ?? ''}</span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      {isOpen && (
        <div className="absolute top-full left-0 z-50 mt-0.5 min-w-full bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-xl max-h-60 overflow-y-auto">
          {options.map((option) => {
            const isActive = option.value === value
            return (
              <button
                key={option.value}
                onClick={() => { onChange(option.value); setIsOpen(false) }}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors cursor-pointer whitespace-nowrap ${
                  isActive
                    ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
                }`}
              >
                <span className="truncate flex-1">{option.label}</span>
                {isActive && (
                  <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

// ── Types ──────────────────────────────────────────────────────────────

type ScheduleTab = 'scheduled' | 'hourly'
type Frequency = 'daily' | 'weekly' | 'monthly' | 'yearly'

interface ScheduledState {
  frequency: Frequency
  interval: number
  time: string
  days: string[]         // weekly: ['mon','wed','fri']
  daysOfMonth: number[]  // monthly: [1, 15]
  ordinal: string        // 'first' | 'second' | 'third' | 'fourth' | 'last'
  ordinalDay: string     // 'day' | 'weekday' | 'monday' etc.
  months: string[]       // yearly: ['jan', 'jul']
}

interface HourlyState {
  start: string
  end: string
  everyValue: number
  everyUnit: 'minutes' | 'hours'
}

const WEEKDAYS = [
  { key: 'mon', label: 'M' },
  { key: 'tue', label: 'T' },
  { key: 'wed', label: 'W' },
  { key: 'thu', label: 'T' },
  { key: 'fri', label: 'F' },
  { key: 'sat', label: 'S' },
  { key: 'sun', label: 'S' },
]

const MONTHS = [
  { key: 'jan', label: 'Jan' }, { key: 'feb', label: 'Feb' }, { key: 'mar', label: 'Mar' },
  { key: 'apr', label: 'Apr' }, { key: 'may', label: 'May' }, { key: 'jun', label: 'Jun' },
  { key: 'jul', label: 'Jul' }, { key: 'aug', label: 'Aug' }, { key: 'sep', label: 'Sep' },
  { key: 'oct', label: 'Oct' }, { key: 'nov', label: 'Nov' }, { key: 'dec', label: 'Dec' },
]

const ORDINALS = ['first', 'second', 'third', 'fourth', 'last']
const ORDINAL_DAYS = ['day', 'weekday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday']

// ── Helpers ────────────────────────────────────────────────────────────

function parseExistingSchedule(mode: string, json: string | null): { tab: ScheduleTab; scheduled: ScheduledState; hourly: HourlyState } {
  const defaultScheduled: ScheduledState = {
    frequency: 'daily', interval: 1, time: '09:00',
    days: ['mon', 'tue', 'wed', 'thu', 'fri'],
    daysOfMonth: [], ordinal: 'first', ordinalDay: 'day', months: [],
  }
  const defaultHourly: HourlyState = { start: '09:00', end: '17:00', everyValue: 30, everyUnit: 'minutes' }

  if (!json) return { tab: mode === 'hourly' ? 'hourly' : 'scheduled', scheduled: defaultScheduled, hourly: defaultHourly }

  try {
    const v = JSON.parse(json)
    if (mode === 'hourly') {
      const secs = v.every_seconds ?? 1800
      return {
        tab: 'hourly',
        scheduled: defaultScheduled,
        hourly: {
          start: v.start ?? '09:00',
          end: v.end ?? '17:00',
          everyValue: secs >= 3600 ? Math.round(secs / 3600) : Math.round(secs / 60),
          everyUnit: secs >= 3600 ? 'hours' : 'minutes',
        },
      }
    }
    // scheduled
    return {
      tab: 'scheduled',
      scheduled: {
        frequency: v.frequency ?? 'daily',
        interval: v.interval ?? 1,
        time: v.time ?? '09:00',
        days: v.days ?? ['mon', 'tue', 'wed', 'thu', 'fri'],
        daysOfMonth: v.days_of_month ?? [],
        ordinal: v.ordinal ?? 'first',
        ordinalDay: v.ordinal_day ?? 'day',
        months: v.months ?? [],
      },
      hourly: defaultHourly,
    }
  } catch {
    return { tab: 'scheduled', scheduled: defaultScheduled, hourly: defaultHourly }
  }
}

function buildScheduleJson(tab: ScheduleTab, scheduled: ScheduledState, hourly: HourlyState): string {
  if (tab === 'hourly') {
    const secs = hourly.everyUnit === 'hours' ? hourly.everyValue * 3600 : hourly.everyValue * 60
    return JSON.stringify({ start: hourly.start, end: hourly.end, every_seconds: secs })
  }
  const base: Record<string, unknown> = { frequency: scheduled.frequency, interval: scheduled.interval, time: scheduled.time }
  if (scheduled.frequency === 'weekly') base.days = scheduled.days
  if (scheduled.frequency === 'monthly') {
    if (scheduled.daysOfMonth.length > 0) {
      base.days_of_month = scheduled.daysOfMonth
    } else {
      base.ordinal = scheduled.ordinal
      base.ordinal_day = scheduled.ordinalDay
    }
  }
  if (scheduled.frequency === 'yearly') {
    base.months = scheduled.months
    if (scheduled.ordinal) { base.ordinal = scheduled.ordinal; base.ordinal_day = scheduled.ordinalDay }
  }
  return JSON.stringify(base)
}

// ── Component ──────────────────────────────────────────────────────────

export default function HeartbeatScheduleDialog(): React.JSX.Element | null {
  const { isOpen, projectId, close } = useHeartbeatScheduleStore()
  const projects = useProjectsStore((s) => s.projects)
  const project = projects.find((p) => p.id === projectId)

  const [tab, setTab] = useState<ScheduleTab>('scheduled')
  const [scheduled, setScheduled] = useState<ScheduledState>({
    frequency: 'daily', interval: 1, time: '09:00',
    days: ['mon', 'tue', 'wed', 'thu', 'fri'],
    daysOfMonth: [], ordinal: 'first', ordinalDay: 'day', months: [],
  })
  const [hourly, setHourly] = useState<HourlyState>({ start: '09:00', end: '17:00', everyValue: 30, everyUnit: 'minutes' })
  const [preview, setPreview] = useState<string[]>([])
  const [saving, setSaving] = useState(false)

  // Load existing schedule when dialog opens
  useEffect(() => {
    if (!isOpen || !project) return
    const parsed = parseExistingSchedule(project.heartbeatMode, project.heartbeatSchedule)
    setTab(parsed.tab)
    setScheduled(parsed.scheduled)
    setHourly(parsed.hourly)
  }, [isOpen, project?.id])

  // Compute preview whenever schedule changes
  const scheduleJson = useMemo(() => buildScheduleJson(tab, scheduled, hourly), [tab, scheduled, hourly])
  useEffect(() => {
    const mode = tab === 'hourly' ? 'hourly' : 'scheduled'
    invoke<string[]>('k2so_agents_preview_schedule', { mode, scheduleJson, count: 5 })
      .then(setPreview)
      .catch(() => setPreview([]))
  }, [tab, scheduleJson])

  // Close on Escape
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') close() }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isOpen, close])

  const handleSave = useCallback(async () => {
    if (!projectId) return
    setSaving(true)
    try {
      const mode = tab === 'hourly' ? 'hourly' : 'scheduled'
      await daemonCliPost('projects/update', { id: projectId, heartbeatMode: mode, heartbeatSchedule: scheduleJson })
      void emit('sync:projects').catch(() => {})
      await invoke('k2so_agents_update_heartbeat_projects')
      try { await invoke('k2so_agents_install_heartbeat') } catch { /* may already be installed */ }

      // Optimistic local update
      useProjectsStore.setState({
        projects: useProjectsStore.getState().projects.map((p) =>
          p.id === projectId ? { ...p, heartbeatMode: mode, heartbeatSchedule: scheduleJson } : p
        ),
      })
      close()
    } catch (err) {
      console.error('[heartbeat-schedule] Save failed:', err)
    } finally {
      setSaving(false)
    }
  }, [projectId, tab, scheduleJson, close])

  const handleTurnOff = useCallback(async () => {
    if (!projectId) return
    await daemonCliPost('projects/update', { id: projectId, heartbeatMode: 'off', heartbeatSchedule: '' })
    void emit('sync:projects').catch(() => {})
    await invoke('k2so_agents_update_heartbeat_projects')
    useProjectsStore.setState({
      projects: useProjectsStore.getState().projects.map((p) =>
        p.id === projectId ? { ...p, heartbeatMode: 'off', heartbeatSchedule: null } : p
      ),
    })
    close()
  }, [projectId, close])

  if (!isOpen) return null

  const frequencyLabel = { daily: 'day', weekly: 'week', monthly: 'month', yearly: 'year' }[scheduled.frequency] || 'day'

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      onClick={(e) => { if (e.target === e.currentTarget) close() }}
    >
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" />

      <div className="relative w-[380px] flex flex-col bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl overflow-hidden">
        {/* Header */}
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center justify-between">
          <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">Heartbeat Schedule</h2>
          <button onClick={close} className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors">
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Tab bar */}
        <div className="flex border-b border-[var(--color-border)]">
          {(['scheduled', 'hourly'] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={`flex-1 px-3 py-2 text-xs font-medium transition-colors cursor-pointer ${
                tab === t
                  ? 'text-[var(--color-accent)] border-b-2 border-[var(--color-accent)] bg-[var(--color-accent)]/5'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {t === 'scheduled' ? 'Scheduled' : 'Hourly'}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="px-4 py-4 space-y-4">
          {tab === 'scheduled' ? (
            <ScheduledForm state={scheduled} onChange={setScheduled} frequencyLabel={frequencyLabel} />
          ) : (
            <HourlyForm state={hourly} onChange={setHourly} />
          )}
        </div>

        {/* Preview */}
        {preview.length > 0 && (
          <div className="px-4 py-2.5 border-t border-[var(--color-border)] bg-[var(--color-bg)]/50">
            <div className="text-[10px] text-[var(--color-text-muted)] mb-1 font-medium">Next runs:</div>
            {preview.slice(0, 4).map((t, i) => (
              <div key={i} className="text-[10px] text-[var(--color-text-secondary)] font-mono">{t}</div>
            ))}
          </div>
        )}

        {/* Footer */}
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex items-center justify-between">
          <button
            onClick={handleTurnOff}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-red-400 transition-colors cursor-pointer"
          >
            Turn off
          </button>
          <div className="flex gap-2">
            <button
              onClick={close}
              className="px-3 py-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors cursor-pointer"
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="px-4 py-1.5 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors cursor-pointer disabled:opacity-50"
            >
              {saving ? 'Saving...' : 'Save'}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

// ── Scheduled Form ──────────────────────────────────────────────────────

function ScheduledForm({
  state, onChange, frequencyLabel,
}: {
  state: ScheduledState
  onChange: (s: ScheduledState) => void
  frequencyLabel: string
}): React.JSX.Element {
  const update = (partial: Partial<ScheduledState>) => onChange({ ...state, ...partial })

  return (
    <>
      {/* Frequency dropdown */}
      <div className="flex items-center gap-3">
        <label className="text-xs text-[var(--color-text-muted)] w-20 flex-shrink-0">Frequency:</label>
        <Dropdown
          value={state.frequency}
          onChange={(v) => update({ frequency: v as Frequency })}
          options={[
            { value: 'daily', label: 'Daily' },
            { value: 'weekly', label: 'Weekly' },
            { value: 'monthly', label: 'Monthly' },
            { value: 'yearly', label: 'Yearly' },
          ]}
          className="flex-1"
        />
      </div>

      {/* Interval */}
      <div className="flex items-center gap-2">
        <span className="text-xs text-[var(--color-text-muted)]">Every</span>
        <input
          type="number"
          min={1}
          max={99}
          value={state.interval}
          onChange={(e) => update({ interval: Math.max(1, parseInt(e.target.value) || 1) })}
          className="w-12 px-2 py-1.5 text-xs text-center bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
        />
        <span className="text-xs text-[var(--color-text-muted)]">{frequencyLabel}{state.interval !== 1 ? 's' : ''}</span>
        {(state.frequency === 'daily' || state.frequency === 'weekly') && (
          <>
            <span className="text-xs text-[var(--color-text-muted)]">at</span>
            <input
              type="time"
              value={state.time}
              onChange={(e) => update({ time: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
          </>
        )}
      </div>

      {/* Weekly: day toggles */}
      {state.frequency === 'weekly' && (
        <div className="flex items-center gap-2">
          <span className="text-xs text-[var(--color-text-muted)] flex-shrink-0">On:</span>
          <div className="flex gap-1">
            {WEEKDAYS.map((d) => (
              <button
                key={d.key}
                onClick={() => {
                  const next = state.days.includes(d.key)
                    ? state.days.filter((x) => x !== d.key)
                    : [...state.days, d.key]
                  update({ days: next })
                }}
                className={`w-7 h-7 text-[10px] font-medium transition-colors cursor-pointer ${
                  state.days.includes(d.key)
                    ? 'bg-[var(--color-accent)] text-white'
                    : 'bg-[var(--color-bg)] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
                }`}
              >
                {d.label}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Monthly: date grid */}
      {state.frequency === 'monthly' && (
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <span className="text-xs text-[var(--color-text-muted)]">at</span>
            <input
              type="time"
              value={state.time}
              onChange={(e) => update({ time: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
          </div>
          <div>
            <span className="text-[10px] text-[var(--color-text-muted)] mb-1.5 block">Select days:</span>
            <div className="grid grid-cols-7 gap-1">
              {Array.from({ length: 31 }, (_, i) => i + 1).map((d) => (
                <button
                  key={d}
                  onClick={() => {
                    const next = state.daysOfMonth.includes(d)
                      ? state.daysOfMonth.filter((x) => x !== d)
                      : [...state.daysOfMonth, d]
                    update({ daysOfMonth: next })
                  }}
                  className={`w-full aspect-square text-[10px] font-medium transition-colors cursor-pointer ${
                    state.daysOfMonth.includes(d)
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg)] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
                  }`}
                >
                  {d}
                </button>
              ))}
            </div>
          </div>
          {/* Ordinal alternative */}
          {state.daysOfMonth.length === 0 && (
            <div className="flex items-center gap-2">
              <span className="text-[10px] text-[var(--color-text-muted)]">On the</span>
              <Dropdown
                value={state.ordinal}
                onChange={(v) => update({ ordinal: v })}
                options={ORDINALS.map((o) => ({ value: o, label: o }))}
              />
              <Dropdown
                value={state.ordinalDay}
                onChange={(v) => update({ ordinalDay: v })}
                options={ORDINAL_DAYS.map((d) => ({ value: d, label: d }))}
              />
            </div>
          )}
        </div>
      )}

      {/* Yearly: month grid */}
      {state.frequency === 'yearly' && (
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <span className="text-xs text-[var(--color-text-muted)]">at</span>
            <input
              type="time"
              value={state.time}
              onChange={(e) => update({ time: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
          </div>
          <div>
            <span className="text-[10px] text-[var(--color-text-muted)] mb-1.5 block">In months:</span>
            <div className="grid grid-cols-4 gap-1">
              {MONTHS.map((m) => (
                <button
                  key={m.key}
                  onClick={() => {
                    const next = state.months.includes(m.key)
                      ? state.months.filter((x) => x !== m.key)
                      : [...state.months, m.key]
                    update({ months: next })
                  }}
                  className={`px-2 py-1.5 text-[10px] font-medium transition-colors cursor-pointer ${
                    state.months.includes(m.key)
                      ? 'bg-[var(--color-accent)] text-white'
                      : 'bg-[var(--color-bg)] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-secondary)]'
                  }`}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>
          {/* Ordinal */}
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-[var(--color-text-muted)]">On the</span>
            <Dropdown
              value={state.ordinal}
              onChange={(v) => update({ ordinal: v })}
              options={ORDINALS.map((o) => ({ value: o, label: o }))}
            />
            <Dropdown
              value={state.ordinalDay}
              onChange={(v) => update({ ordinalDay: v })}
              options={ORDINAL_DAYS.map((d) => ({ value: d, label: d }))}
            />
          </div>
        </div>
      )}
    </>
  )
}

// ── Hourly Form ──────────────────────────────────────────────────────────

function HourlyForm({
  state, onChange,
}: {
  state: HourlyState
  onChange: (s: HourlyState) => void
}): React.JSX.Element {
  const update = (partial: Partial<HourlyState>) => onChange({ ...state, ...partial })

  return (
    <>
      <div className="space-y-3">
        <div>
          <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">Work hours:</label>
          <div className="flex items-center gap-2">
            <input
              type="time"
              value={state.start}
              onChange={(e) => update({ start: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
            <span className="text-xs text-[var(--color-text-muted)]">to</span>
            <input
              type="time"
              value={state.end}
              onChange={(e) => update({ end: e.target.value })}
              className="px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
          </div>
        </div>

        <div>
          <label className="text-xs text-[var(--color-text-muted)] block mb-1.5">Wake every:</label>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min={1}
              max={999}
              value={state.everyValue}
              onChange={(e) => update({ everyValue: Math.max(1, parseInt(e.target.value) || 1) })}
              className="w-16 px-2 py-1.5 text-xs text-center bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag"
            />
            <Dropdown
              value={state.everyUnit}
              onChange={(v) => update({ everyUnit: v as 'minutes' | 'hours' })}
              options={[
                { value: 'minutes', label: 'minutes' },
                { value: 'hours', label: 'hours' },
              ]}
            />
          </div>
        </div>
      </div>
    </>
  )
}
