// Settings → Logs → Timer. The timer's three settings in one card, then the
// History card: a date range and a workspace to filter by, the entries
// grouped by month, export, and delete with a confirmation.
import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { Button, Input, Toggle } from '@/components/ui'
import { useProjectsStore } from '@/stores/projects'
import { useTimerStore, formatTimestamp, type TimeEntry } from '@/stores/timer'
import {
  SETTING_FIELD_WIDTH,
  SETTING_ROW_HINT,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
} from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const TIMER_MANIFEST: SettingEntry[] = [
  { id: 'timer.visible', section: 'timer', label: 'Show the Timer in the Top Bar', description: 'The clock button that starts and stops the timer', keywords: ['timer', 'clock', 'show', 'hide'] },
  { id: 'timer.skip-memo', section: 'timer', label: 'Skip the Note When You Stop', description: 'Save each entry without asking what you worked on', keywords: ['memo', 'note', 'prompt'] },
  { id: 'timer.timezone', section: 'timer', label: 'Time Zone', description: 'Times in the history and exports', keywords: ['timezone', 'time', 'zone', 'tz'] },
  { id: 'timer.history', section: 'timer', label: 'Timer History', description: 'Past time entries, with CSV and JSON export', keywords: ['history', 'entries', 'export', 'csv', 'json'] },
]

const CONTENT_WIDTH = 'max-w-[52rem]'

// Common IANA timezones for the dropdown
const COMMON_TIMEZONES = [
  '',
  'UTC',
  'America/New_York',
  'America/Chicago',
  'America/Denver',
  'America/Los_Angeles',
  'America/Anchorage',
  'Pacific/Honolulu',
  'America/Toronto',
  'America/Vancouver',
  'America/Sao_Paulo',
  'America/Argentina/Buenos_Aires',
  'Europe/London',
  'Europe/Paris',
  'Europe/Berlin',
  'Europe/Moscow',
  'Asia/Dubai',
  'Asia/Kolkata',
  'Asia/Singapore',
  'Asia/Shanghai',
  'Asia/Tokyo',
  'Asia/Seoul',
  'Australia/Sydney',
  'Australia/Melbourne',
  'Pacific/Auckland',
]

type Range = 'all' | 'today' | 'week' | 'month' | 'last-month' | 'custom'

const RANGES: { value: Range; label: string }[] = [
  { value: 'all', label: 'All time' },
  { value: 'today', label: 'Today' },
  { value: 'week', label: 'Last 7 days' },
  { value: 'month', label: 'This month' },
  { value: 'last-month', label: 'Last month' },
  { value: 'custom', label: 'Pick dates…' },
]

/** How the footer names the range: "3 entries in the last 7 days in atlas". */
const RANGE_PHRASE: Record<Range, string> = {
  all: '',
  today: 'today',
  week: 'in the last 7 days',
  month: 'this month',
  'last-month': 'last month',
  custom: 'between the dates you picked',
}

const pad = (n: number): string => String(n).padStart(2, '0')

/** "1h 35m", "48m", "under a minute": the history reads in minutes. */
export function formatSpent(seconds: number): string {
  const mins = Math.round(seconds / 60)
  if (mins < 1) return 'under a minute'
  const h = Math.floor(mins / 60)
  const m = mins % 60
  if (h === 0) return `${m}m`
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

/** The range as unix seconds (start of the first day, end of the last). */
export function rangeBounds(range: Range, from: string, to: string, now = new Date()): { start?: number; end?: number } {
  const sec = (d: Date): number => Math.floor(d.getTime() / 1000)
  const day = (y: number, m: number, d: number): Date => new Date(y, m, d)
  const y = now.getFullYear()
  const m = now.getMonth()
  switch (range) {
    case 'all':
      return {}
    case 'today':
      return { start: sec(day(y, m, now.getDate())) }
    case 'week':
      return { start: sec(day(y, m, now.getDate() - 6)) }
    case 'month':
      return { start: sec(day(y, m, 1)) }
    case 'last-month':
      return { start: sec(day(y, m - 1, 1)), end: sec(day(y, m, 1)) - 1 }
    case 'custom':
      return {
        start: from ? sec(new Date(`${from}T00:00:00`)) : undefined,
        end: to ? sec(new Date(`${to}T23:59:59`)) : undefined,
      }
  }
}

function EntryRow({
  entry,
  projectName,
  timezone,
  onDelete,
}: {
  entry: TimeEntry
  projectName: string | null
  timezone: string
  onDelete: (id: string) => Promise<boolean>
}): React.JSX.Element {
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  const day = formatTimestamp(entry.startTime, timezone, { year: undefined, month: 'short', day: 'numeric', hour: undefined, minute: undefined })
  const clock = (t: number): string => formatTimestamp(t, timezone, { year: undefined, month: undefined, day: undefined, hour: 'numeric', minute: '2-digit' })
  const title = entry.memo?.trim() || 'No note'
  return (
    <li className="px-4 py-3" data-timer-entry={entry.id}>
      <div className="flex items-start justify-between gap-6">
        <div className="min-w-0 flex-1">
          <p className={`type-row truncate ${entry.memo?.trim() ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>{title}</p>
          <p className={SETTING_ROW_HINT}>
            {projectName ?? 'No workspace'} · {day}, {clock(entry.startTime)} – {clock(entry.endTime)}
          </p>
          {failed && (
            <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
              Couldn’t delete this entry. It’s still here; try again.
            </p>
          )}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          {confirming ? (
            <>
              <span className="type-hint text-[var(--color-text-primary)]">Delete this entry?</span>
              <Button variant="quiet" onClick={() => setConfirming(false)} disabled={busy}>
                Keep
              </Button>
              <Button
                variant="danger"
                disabled={busy}
                aria-label={`Delete the entry “${title}”`}
                onClick={async () => {
                  setBusy(true)
                  setFailed(false)
                  const ok = await onDelete(entry.id)
                  setBusy(false)
                  if (!ok) {
                    setFailed(true)
                    setConfirming(false)
                  }
                }}
              >
                {busy ? 'Deleting…' : 'Delete'}
              </Button>
            </>
          ) : (
            <>
              <span className="type-row text-[var(--color-text-primary)] tabular-nums">{formatSpent(entry.durationSeconds)}</span>
              <Button variant="quiet" aria-label={`Delete “${title}”`} onClick={() => setConfirming(true)}>
                Delete
              </Button>
            </>
          )}
        </div>
      </div>
    </li>
  )
}

export function TimerSection(): React.JSX.Element {
  const visible = useTimerStore((s) => s.visible)
  const skipMemo = useTimerStore((s) => s.skipMemo)
  const timezone = useTimerStore((s) => s.timezone)
  const entries = useTimerStore((s) => s.entries)
  const status = useTimerStore((s) => s.entriesStatus)
  const loadError = useTimerStore((s) => s.entriesError)
  const fetchEntries = useTimerStore((s) => s.fetchEntries)
  const deleteEntry = useTimerStore((s) => s.deleteEntry)
  const restoreEntry = useTimerStore((s) => s.restoreEntry)
  const exportEntries = useTimerStore((s) => s.exportEntries)
  const updateTimerSetting = useTimerStore((s) => s.updateTimerSetting)
  const projects = useProjectsStore((s) => s.projects)

  const showSave = useSaveStatus()
  const memoSave = useSaveStatus()
  const tzSave = useSaveStatus()

  const [range, setRange] = useState<Range>('all')
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const [project, setProject] = useState('')
  const [exportState, setExportState] = useState<null | { kind: 'saved' | 'error'; text: string }>(null)
  // The entry just deleted, so the page can say so and offer Undo.
  const [deleted, setDeleted] = useState<null | { entry: TimeEntry; undo: 'idle' | 'busy' | 'failed' }>(null)

  const deleteAndNote = useCallback(
    async (id: string): Promise<boolean> => {
      const entry = entries.find((e) => e.id === id)
      const ok = await deleteEntry(id)
      if (ok && entry) setDeleted({ entry, undo: 'idle' })
      return ok
    },
    [entries, deleteEntry],
  )

  const undoDelete = useCallback(async () => {
    if (!deleted) return
    setDeleted({ ...deleted, undo: 'busy' })
    const ok = await restoreEntry(deleted.entry)
    setDeleted(ok ? null : { ...deleted, undo: 'failed' })
  }, [deleted, restoreEntry])

  const bounds = useMemo(() => rangeBounds(range, from, to), [range, from, to])
  const filtered = range !== 'all' || project !== ''

  const load = useCallback(() => {
    void fetchEntries(bounds.start, bounds.end, project || undefined)
  }, [bounds.start, bounds.end, project, fetchEntries])

  // Loads on open and again whenever a filter changes.
  useEffect(() => {
    load()
  }, [load])

  const save = useCallback(
    async (key: 'visible' | 'skipMemo' | 'timezone', value: boolean | string, status: ReturnType<typeof useSaveStatus>) => {
      const ok = await updateTimerSetting(key, value)
      if (ok) status.saved()
      else status.failed('the server didn’t accept the change')
    },
    [updateTimerSetting],
  )

  const handleExport = useCallback(
    async (format: 'csv' | 'json') => {
      setExportState(null)
      const data = await exportEntries(format, bounds.start, bounds.end, project || undefined)
      if (!data) {
        setExportState({ kind: 'error', text: `Couldn’t export: this server sent no file. Nothing was saved; try again.` })
        return
      }
      const blob = new Blob([data], { type: format === 'csv' ? 'text/csv' : 'application/json' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `k2-time-entries.${format}`
      a.click()
      URL.revokeObjectURL(url)
      setExportState({ kind: 'saved', text: `Exported ${format.toUpperCase()} to your downloads.` })
    },
    [bounds.start, bounds.end, project, exportEntries],
  )

  // Group entries by month, newest first.
  const groupedEntries = useMemo(() => {
    const groups: Record<string, TimeEntry[]> = {}
    for (const entry of [...entries].sort((a, b) => b.startTime - a.startTime)) {
      const date = new Date(entry.startTime * 1000)
      const key = `${date.getFullYear()}-${pad(date.getMonth() + 1)}`
      if (!groups[key]) groups[key] = []
      groups[key].push(entry)
    }
    return Object.entries(groups).sort(([a], [b]) => b.localeCompare(a))
  }, [entries])

  const total = entries.reduce((n, e) => n + e.durationSeconds, 0)
  const detectedTz = Intl.DateTimeFormat().resolvedOptions().timeZone
  const projectName = (id: string | null): string | null => (id ? projects.find((p) => p.id === id)?.name ?? null : null)
  const clearFilters = (): void => {
    setRange('all')
    setFrom('')
    setTo('')
    setProject('')
  }

  const historyState =
    status === 'error' ? (
      <SettingsLoadState
        kind="error"
        title="Your time entries couldn’t load"
        detail={loadError}
        action={<Button onClick={load}>Try again</Button>}
        testId="timer-history-error"
      >
        K2 couldn’t read the history on this server. Nothing is lost; try again in a moment.
      </SettingsLoadState>
    ) : status === 'loading' ? (
      <SettingsLoadState kind="loading" title="Loading your time entries…" testId="timer-history-loading" />
    ) : undefined

  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`} data-testid="timer-page">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Timer</h2>
        <p className={SETTING_ROW_HINT}>
          Track the time you spend in each workspace. Start and stop it from the clock in the top bar; each stop saves an
          entry to the history below.
        </p>
      </header>

      <SettingsGroup card title="Settings" testId="timer-settings">
        <SettingRow
          settingId="timer.visible"
          label="Show the timer in the top bar"
          hint={
            <>
              The clock button that starts and stops the timer.
              <SaveError state={showSave.state} />
            </>
          }
        >
          <SaveNote state={showSave.state} />
          <Toggle size="sm" checked={visible} aria-label="Show the timer in the top bar" onChange={() => void save('visible', !visible, showSave)} />
        </SettingRow>
        <SettingRow
          settingId="timer.skip-memo"
          label="Skip the note when you stop"
          hint={
            <>
              Save each entry straight away instead of asking what you worked on.
              <SaveError state={memoSave.state} />
            </>
          }
        >
          <SaveNote state={memoSave.state} />
          <Toggle size="sm" checked={skipMemo} aria-label="Skip the note when you stop" onChange={() => void save('skipMemo', !skipMemo, memoSave)} />
        </SettingRow>
        <SettingRow
          settingId="timer.timezone"
          label="Time zone"
          hint={
            <>
              For the times in the history and in exports. Auto follows this computer ({detectedTz}).
              <SaveError state={tzSave.state} />
            </>
          }
        >
          <SaveNote state={tzSave.state} />
          <div className={SETTING_FIELD_WIDTH}>
            <SettingDropdown
              ariaLabel="Time zone"
              fullWidth
              value={timezone}
              options={COMMON_TIMEZONES.map((tz) => ({
                value: tz,
                label: tz === '' ? `Auto (${detectedTz})` : tz,
              }))}
              onChange={(v) => void save('timezone', v, tzSave)}
            />
          </div>
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup
        card
        title="History"
        hint="Every entry, newest first. Filter by dates or workspace, and export what you see."
        action={
          <div className="flex items-center gap-2">
            <Button variant="quiet" onClick={() => void handleExport('csv')} disabled={status !== 'ready' || entries.length === 0}>
              Export CSV
            </Button>
            <Button variant="quiet" onClick={() => void handleExport('json')} disabled={status !== 'ready' || entries.length === 0}>
              Export JSON
            </Button>
          </div>
        }
        testId="timer-history"
      >
        <div className="px-4 py-3 space-y-3" data-testid="timer-filters">
          <div className="flex flex-wrap items-end gap-3">
            <label className="flex flex-col gap-1">
              <span className="type-hint text-[var(--color-text-secondary)]">Dates</span>
              <div className="w-44">
                <SettingDropdown ariaLabel="Dates" menuAlign="left" value={range} options={RANGES} onChange={(v) => setRange(v as Range)} fullWidth />
              </div>
            </label>
            <label className="flex flex-col gap-1">
              <span className="type-hint text-[var(--color-text-secondary)]">Workspace</span>
              <div className={SETTING_FIELD_WIDTH}>
                <SettingDropdown
                  ariaLabel="Workspace"
                  menuAlign="left"
                  value={project}
                  fullWidth
                  options={[{ value: '', label: 'All workspaces' }, ...projects.map((p) => ({ value: p.id, label: p.name }))]}
                  onChange={setProject}
                />
              </div>
            </label>
            {filtered && (
              <Button variant="quiet" onClick={clearFilters}>
                Clear filters
              </Button>
            )}
          </div>
          {range === 'custom' && (
            <div className="flex flex-wrap items-end gap-3" data-testid="timer-custom-range">
              <label className="flex flex-col gap-1">
                <span className="type-hint text-[var(--color-text-secondary)]">From</span>
                <Input type="date" value={from} max={to || undefined} onChange={(e) => setFrom(e.target.value)} aria-label="From" className="w-44" />
              </label>
              <label className="flex flex-col gap-1">
                <span className="type-hint text-[var(--color-text-secondary)]">To</span>
                <Input type="date" value={to} min={from || undefined} onChange={(e) => setTo(e.target.value)} aria-label="To" className="w-44" />
              </label>
              {!from && !to && <p className="type-hint text-[var(--color-text-secondary)] pb-2">Pick a first day, a last day, or both.</p>}
            </div>
          )}
          {deleted && (
            <div className="flex items-center justify-between gap-4" data-testid="timer-deleted">
              <p
                role={deleted.undo === 'failed' ? 'alert' : 'status'}
                className={`type-hint min-w-0 ${deleted.undo === 'failed' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-ok-soft)]'}`}
              >
                {deleted.undo === 'failed'
                  ? 'Couldn’t bring the entry back. Try Undo again in a moment.'
                  : `Deleted the entry “${deleted.entry.memo?.trim() || 'No note'}” (${formatSpent(deleted.entry.durationSeconds)}${
                      projectName(deleted.entry.projectId) ? `, ${projectName(deleted.entry.projectId)}` : ''
                    }).`}
              </p>
              <div className="flex items-center gap-2 flex-shrink-0">
                <Button variant="quiet" onClick={() => setDeleted(null)} disabled={deleted.undo === 'busy'}>
                  Dismiss
                </Button>
                <Button onClick={() => void undoDelete()} disabled={deleted.undo === 'busy'}>
                  {deleted.undo === 'busy' ? 'Restoring…' : 'Undo'}
                </Button>
              </div>
            </div>
          )}
          {exportState && (
            <p
              role={exportState.kind === 'error' ? 'alert' : 'status'}
              className={`type-hint ${exportState.kind === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-ok-soft)]'}`}
            >
              {exportState.text}
            </p>
          )}
        </div>

        {historyState ? (
          <div className="p-3">{historyState}</div>
        ) : entries.length === 0 ? (
          <div className="px-4 py-6 flex items-center justify-between gap-6" data-testid="timer-history-empty">
            <div className="min-w-0">
              <p className="type-row text-[var(--color-text-primary)]">{filtered ? 'Nothing in this range' : 'No time entries yet'}</p>
              <p className={SETTING_ROW_HINT}>
                {filtered
                  ? 'No entries match these dates and workspace.'
                  : 'Start the timer from the clock in the top bar. When you stop it, the entry shows here.'}
              </p>
            </div>
            {filtered && <Button onClick={clearFilters}>Show all entries</Button>}
          </div>
        ) : (
          <>
            <div className="max-h-[36rem] overflow-y-auto" data-testid="timer-entries">
              {groupedEntries.map(([monthKey, monthEntries]) => {
                const [year, month] = monthKey.split('-')
                const monthLabel = new Date(Number(year), Number(month) - 1).toLocaleString('en-US', { month: 'long', year: 'numeric' })
                const monthTotal = monthEntries.reduce((n, e) => n + e.durationSeconds, 0)
                return (
                  <section key={monthKey} aria-label={monthLabel}>
                    <div className="px-4 pt-3 pb-1 flex items-baseline justify-between gap-4 border-b border-[var(--color-border)]">
                      <h4 className="type-name text-[var(--color-text-primary)]">{monthLabel}</h4>
                      <span className="type-hint text-[var(--color-text-secondary)] tabular-nums">{formatSpent(monthTotal)}</span>
                    </div>
                    <ul className="divide-y divide-[var(--color-border)] border-b border-[var(--color-border)]">
                      {monthEntries.map((entry) => (
                        <EntryRow key={entry.id} entry={entry} projectName={projectName(entry.projectId)} timezone={timezone} onDelete={deleteAndNote} />
                      ))}
                    </ul>
                  </section>
                )
              })}
            </div>
            <div className="px-4 py-2.5">
              <p className="type-hint text-[var(--color-text-secondary)]" data-testid="timer-total">
                {entries.length === 1 ? 'One entry' : `${entries.length} entries`}
                {filtered ? ` ${[RANGE_PHRASE[range], project ? `in ${projectName(project) ?? 'this workspace'}` : ''].filter(Boolean).join(' ')}` : ''}
                , {formatSpent(total)} in all.
              </p>
            </div>
          </>
        )}
      </SettingsGroup>
    </div>
  )
}
