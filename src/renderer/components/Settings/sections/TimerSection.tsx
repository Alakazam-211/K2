import React from 'react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  useTimerStore,
  formatTimestamp,
  formatDuration,
  type TimeEntry,
  type CountdownThemeConfig,
} from '@/stores/timer'
import { SettingDropdown } from '../controls/SettingControls'
import { BUILT_IN_THEMES } from '@shared/constants/timer-themes'
import type { SettingEntry } from '../searchManifest'

export const TIMER_MANIFEST: SettingEntry[] = [
  { id: 'timer.visible', section: 'timer', label: 'Show Timer Button', description: 'Display the timer icon in the top bar', keywords: ['timer', 'clock', 'show', 'hide'] },
  { id: 'timer.countdown', section: 'timer', label: 'Countdown Before Start', description: 'Animated 3-2-1 countdown before the timer begins', keywords: ['countdown', 'intro', 'start'] },
  { id: 'timer.countdown-theme', section: 'timer', label: 'Countdown Theme', description: 'Visual theme for the countdown animation', keywords: ['theme', 'countdown', 'rocket', 'matrix', 'retro'] },
  { id: 'timer.custom-themes', section: 'timer', label: 'Custom Themes', description: 'Upload your own countdown theme JSON', keywords: ['custom', 'theme', 'upload', 'json'] },
  { id: 'timer.skip-memo', section: 'timer', label: 'Skip Memo on Stop', description: 'Save entries without asking for a note', keywords: ['memo', 'note', 'prompt'] },
  { id: 'timer.timezone', section: 'timer', label: 'Timezone', description: 'Display times in this IANA timezone', keywords: ['timezone', 'time', 'zone', 'tz'] },
  { id: 'timer.history', section: 'timer', label: 'Timer History', description: 'Past time entries with CSV/JSON export', keywords: ['history', 'entries', 'export', 'csv', 'json'] },
]

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

export function TimerSection(): React.JSX.Element {
  const visible = useTimerStore((s) => s.visible)
  const countdownEnabled = useTimerStore((s) => s.countdownEnabled)
  const countdownTheme = useTimerStore((s) => s.countdownTheme)
  const skipMemo = useTimerStore((s) => s.skipMemo)
  const timezone = useTimerStore((s) => s.timezone)
  const customThemes = useTimerStore((s) => s.customThemes)
  const entries = useTimerStore((s) => s.entries)
  const fetchEntries = useTimerStore((s) => s.fetchEntries)
  const deleteEntry = useTimerStore((s) => s.deleteEntry)
  const exportEntries = useTimerStore((s) => s.exportEntries)
  const updateTimerSetting = useTimerStore((s) => s.updateTimerSetting)

  const projects = useProjectsStore((s) => s.projects)

  // Filter state
  const [filterStart, setFilterStart] = useState('')
  const [filterEnd, setFilterEnd] = useState('')
  const [filterProject, setFilterProject] = useState('')

  // Load entries on mount
  useEffect(() => {
    fetchEntries()
  }, [fetchEntries])

  const handleFilter = useCallback(() => {
    const start = filterStart ? Math.floor(new Date(filterStart).getTime() / 1000) : undefined
    const end = filterEnd ? Math.floor(new Date(filterEnd + 'T23:59:59').getTime() / 1000) : undefined
    fetchEntries(start, end, filterProject || undefined)
  }, [filterStart, filterEnd, filterProject, fetchEntries])

  // Re-fetch when filters change
  useEffect(() => {
    handleFilter()
  }, [handleFilter])

  const handleExport = useCallback(async (format: 'csv' | 'json') => {
    const start = filterStart ? Math.floor(new Date(filterStart).getTime() / 1000) : undefined
    const end = filterEnd ? Math.floor(new Date(filterEnd + 'T23:59:59').getTime() / 1000) : undefined
    const data = await exportEntries(format, start, end, filterProject || undefined)
    if (!data) return

    // Download as file
    const blob = new Blob([data], { type: format === 'csv' ? 'text/csv' : 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `k2so-time-entries.${format}`
    a.click()
    URL.revokeObjectURL(url)
  }, [filterStart, filterEnd, filterProject, exportEntries])

  const themeInputRef = useRef<HTMLInputElement>(null)

  const handleUploadTheme = useCallback(() => {
    themeInputRef.current?.click()
  }, [])

  const handleThemeFileChange = useCallback(async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (!file) return
    // Reset input so the same file can be re-selected
    e.target.value = ''

    try {
      const text = await file.text()
      const parsed = JSON.parse(text) as CountdownThemeConfig

      // Basic validation
      if (!parsed.name || !parsed.backgroundColor || !parsed.textColor || !parsed.finalText) {
        console.error('[timer] Invalid theme: must have name, backgroundColor, textColor, and finalText')
        return
      }
      if (!parsed.countdownTexts || !Array.isArray(parsed.countdownTexts)) {
        parsed.countdownTexts = ['3', '2', '1']
      }
      if (!parsed.animationPreset) {
        parsed.animationPreset = 'fade'
      }
      if (!parsed.flowTitles || !Array.isArray(parsed.flowTitles)) {
        parsed.flowTitles = []
      }

      const updated = [...customThemes, parsed]
      await updateTimerSetting('customThemes', updated)
    } catch (err) {
      console.error('[timer] Failed to upload theme:', err)
    }
  }, [customThemes, updateTimerSetting])

  const handleDownloadReference = useCallback(() => {
    const reference: CountdownThemeConfig = {
      name: 'My Custom Theme',
      backgroundColor: '#0a0a2e',
      textColor: '#00ff88',
      fontFamily: 'monospace',
      countdownTexts: ['Ready...', 'Set...', 'Go!'],
      finalText: 'FLOW TIME!',
      animationPreset: 'fade',
      flowTitles: [
        "You're on fire!",
        "Keep that momentum going!",
        "Built different.",
        "The keyboard is smoking.",
        "Locked in.",
      ],
    }
    const blob = new Blob([JSON.stringify(reference, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'k2so-theme-reference.json'
    a.click()
    URL.revokeObjectURL(url)
  }, [])

  const handleDeleteTheme = useCallback(async (name: string) => {
    const updated = customThemes.filter((t) => t.name !== name)
    await updateTimerSetting('customThemes', updated)
    // If the deleted theme was active, fall back to rocket
    if (countdownTheme === name) {
      await updateTimerSetting('countdownTheme', 'rocket')
    }
  }, [customThemes, countdownTheme, updateTimerSetting])

  // Group entries by month
  const groupedEntries = useMemo(() => {
    const groups: Record<string, TimeEntry[]> = {}
    for (const entry of entries) {
      const date = new Date(entry.startTime * 1000)
      const key = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`
      if (!groups[key]) groups[key] = []
      groups[key].push(entry)
    }
    return Object.entries(groups).sort(([a], [b]) => b.localeCompare(a))
  }, [entries])

  const detectedTz = Intl.DateTimeFormat().resolvedOptions().timeZone

  return (
    <div>
      <div className="max-w-xl">
      <h2 className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">Timer</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-6">
        Track work sessions. Click the clock icon in the top bar to start.
      </p>

      {/* Show timer button */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Show timer button</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Display the timer icon in the top bar</div>
        </div>
        <button
          onClick={() => updateTimerSetting('visible', !visible)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            visible ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              visible ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Enable countdown */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Countdown before start</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Show a themed 3-2-1 countdown before the timer begins</div>
        </div>
        <button
          onClick={() => updateTimerSetting('countdownEnabled', !countdownEnabled)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            countdownEnabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              countdownEnabled ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Countdown theme */}
      {countdownEnabled && (
        <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
          <div>
            <div className="text-xs text-[var(--color-text-primary)]">Countdown theme</div>
          </div>
          <SettingDropdown
            value={countdownTheme}
            options={[
              ...BUILT_IN_THEMES.map((t) => ({ value: t.value, label: t.label })),
              ...customThemes.map((t) => ({ value: t.name, label: t.name })),
            ]}
            onChange={(v) => updateTimerSetting('countdownTheme', v)}
          />
        </div>
      )}

      {/* Custom themes */}
      {countdownEnabled && (
        <div className="py-2 border-b border-[var(--color-border)]">
          <input
            ref={themeInputRef}
            type="file"
            accept=".json"
            className="hidden"
            onChange={handleThemeFileChange}
          />
          <div className="flex items-center justify-between mb-2">
            <div className="text-xs text-[var(--color-text-primary)]">Custom themes</div>
            <div className="flex items-center gap-3">
              <button
                onClick={handleDownloadReference}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer"
              >
                Download reference
              </button>
              <button
                onClick={handleUploadTheme}
                className="text-[10px] text-[var(--color-accent)] hover:underline cursor-pointer"
              >
                Upload .json
              </button>
            </div>
          </div>
          {customThemes.length === 0 ? (
            <div className="text-[10px] text-[var(--color-text-muted)]">No custom themes uploaded</div>
          ) : (
            <div className="space-y-1">
              {customThemes.map((t) => (
                <div key={t.name} className="flex items-center justify-between text-xs">
                  <div className="flex items-center gap-2">
                    <div
                      className="w-3 h-3 border border-[var(--color-border)]"
                      style={{ backgroundColor: t.backgroundColor }}
                    />
                    <span className="text-[var(--color-text-secondary)]">{t.name}</span>
                  </div>
                  <button
                    onClick={() => handleDeleteTheme(t.name)}
                    className="text-[10px] text-red-400 hover:text-red-300 cursor-pointer"
                  >
                    Remove
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Skip memo */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Skip memo on stop</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">Save time entries without asking for a note</div>
        </div>
        <button
          onClick={() => updateTimerSetting('skipMemo', !skipMemo)}
          className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
            skipMemo ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-3 h-3 bg-white block transition-transform ${
              skipMemo ? 'translate-x-4.5' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* Timezone */}
      <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
        <div>
          <div className="text-xs text-[var(--color-text-primary)]">Timezone</div>
          <div className="text-[10px] text-[var(--color-text-muted)]">
            Times displayed in this timezone (detected: {detectedTz})
          </div>
        </div>
        <SettingDropdown
          value={timezone}
          options={COMMON_TIMEZONES.map((tz) => ({
            value: tz,
            label: tz === '' ? `Auto (${detectedTz})` : tz,
          }))}
          onChange={(v) => updateTimerSetting('timezone', v)}
        />
      </div>

      </div>{/* end max-w-xl */}

      {/* Timer History */}
      <div className="mt-6">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-sm font-semibold text-[var(--color-text-primary)]">History</h3>
          <div className="flex gap-2">
            <button
              onClick={() => handleExport('csv')}
              className="text-[10px] px-2 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            >
              Export CSV
            </button>
            <button
              onClick={() => handleExport('json')}
              className="text-[10px] px-2 py-0.5 border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            >
              Export JSON
            </button>
          </div>
        </div>

        {/* Filters */}
        <div className="flex gap-2 mb-3 items-end">
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">From</label>
            <input
              type="date"
              value={filterStart}
              onChange={(e) => setFilterStart(e.target.value)}
              className="text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-2 py-1 outline-none"
            />
          </div>
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">To</label>
            <input
              type="date"
              value={filterEnd}
              onChange={(e) => setFilterEnd(e.target.value)}
              className="text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] px-2 py-1 outline-none"
            />
          </div>
          <div>
            <label className="text-[10px] text-[var(--color-text-muted)] block mb-0.5">Project</label>
            <SettingDropdown
              value={filterProject}
              options={[
                { value: '', label: 'All projects' },
                ...projects.map((p) => ({ value: p.id, label: p.name })),
              ]}
              onChange={setFilterProject}
            />
          </div>
          {(filterStart || filterEnd || filterProject) && (
            <button
              onClick={() => { setFilterStart(''); setFilterEnd(''); setFilterProject('') }}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer pb-1"
            >
              Clear
            </button>
          )}
        </div>

        {/* Entries */}
        {entries.length === 0 ? (
          <div className="text-xs text-[var(--color-text-muted)] py-4 text-center">
            No time entries yet. Click the timer button to start tracking.
          </div>
        ) : (
          <div className="max-h-[600px] overflow-y-auto">
            {/* Column headers */}
            <div className="grid gap-x-3 px-2 py-1.5 border-b border-[var(--color-border)] sticky top-0 bg-[var(--color-bg)] z-10 text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider"
              style={{ gridTemplateColumns: '190px 190px 80px 100px 1fr 20px' }}
            >
              <span>Start</span>
              <span>End</span>
              <span>Duration</span>
              <span>Project</span>
              <span>Memo</span>
              <span />
            </div>

            <div className="space-y-3">
              {groupedEntries.map(([monthKey, monthEntries]) => {
                const [year, month] = monthKey.split('-')
                const monthLabel = new Date(Number(year), Number(month) - 1).toLocaleString('en-US', { month: 'long', year: 'numeric' })
                return (
                  <div key={monthKey}>
                    <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1 sticky top-[28px] bg-[var(--color-bg)] py-1 px-2 z-[5]">
                      {monthLabel}
                    </div>
                    <div>
                      {monthEntries.map((entry) => {
                        const project = projects.find((p) => p.id === entry.projectId)
                        const timeOpts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }
                        return (
                          <div
                            key={entry.id}
                            className="grid items-center gap-x-3 py-1.5 px-2 hover:bg-[var(--color-bg-elevated)] group text-xs font-mono"
                            style={{ gridTemplateColumns: '190px 190px 80px 100px 1fr 20px' }}
                          >
                            <span className="text-[var(--color-text-muted)] truncate">
                              {formatTimestamp(entry.startTime, timezone, timeOpts)}
                            </span>
                            <span className="text-[var(--color-text-muted)] truncate">
                              {formatTimestamp(entry.endTime, timezone, timeOpts)}
                            </span>
                            <span className="text-[var(--color-accent)]">
                              {formatDuration(entry.durationSeconds)}
                            </span>
                            <span className="text-[var(--color-text-muted)] truncate font-sans">
                              {project?.name || '—'}
                            </span>
                            <span className="text-[var(--color-text-secondary)] truncate font-sans">
                              {entry.memo || '—'}
                            </span>
                            <button
                              onClick={() => deleteEntry(entry.id)}
                              className="text-red-400/0 group-hover:text-red-400/60 hover:!text-red-400 transition-colors cursor-pointer text-center"
                              title="Delete entry"
                            >
                              ×
                            </button>
                          </div>
                        )
                      })}
                    </div>
                  </div>
                )
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
