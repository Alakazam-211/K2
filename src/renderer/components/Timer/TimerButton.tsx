import { useState, useEffect } from 'react'
import { useTimerStore, getElapsedMs, formatElapsed } from '@/stores/timer'

export default function TimerButton(): React.JSX.Element | null {
  const status = useTimerStore((s) => s.status)
  const visible = useTimerStore((s) => s.visible)
  const pausedElapsed = useTimerStore((s) => s.pausedElapsed)
  const resumeTime = useTimerStore((s) => s.resumeTime)
  const startTimer = useTimerStore((s) => s.startTimer)
  const pauseTimer = useTimerStore((s) => s.pauseTimer)
  const resumeTimer = useTimerStore((s) => s.resumeTimer)
  const stopTimer = useTimerStore((s) => s.stopTimer)
  const showMemoDialog = useTimerStore((s) => s.showMemoDialog)

  // Re-render every second when running
  const [, setTick] = useState(0)
  useEffect(() => {
    if (status !== 'running') return
    const interval = setInterval(() => setTick((t) => t + 1), 1000)
    return () => clearInterval(interval)
  }, [status])

  if (!visible) return null

  // Timer is stopped and waiting for memo — hide controls
  // (the MemoDialog handles the rest of the flow)
  if (showMemoDialog) return null

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const noDrag = { WebkitAppRegion: 'no-drag' } as any

  const btnClass =
    'flex h-6 items-center justify-center transition-colors'

  // Idle state: single start button (clock icon) — starts the stopwatch
  if (status === 'idle') {
    return (
      <div className="flex items-center no-drag">
        <button
          onClick={startTimer}
          className={`${btnClass} w-5 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]`}
          style={noDrag}
          title="Start stopwatch"
        >
          <svg
            className="w-3.5 h-3.5 flex-shrink-0"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="12" cy="12" r="10" />
            <polyline points="12 6 12 12 16 14" />
          </svg>
        </button>
      </div>
    )
  }

  // Running or paused: show controls + elapsed readout
  const displayMs = getElapsedMs({ status, pausedElapsed, resumeTime })
  const displayText = formatElapsed(displayMs)

  return (
    <div className="flex items-center gap-0.5 no-drag">
      {/* Pause / Resume */}
      {status === 'running' ? (
        <button
          onClick={pauseTimer}
          className={`${btnClass} w-5 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]`}
          style={noDrag}
          title="Pause timer"
        >
          <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <rect x="5" y="3" width="4" height="18" rx="1" />
            <rect x="15" y="3" width="4" height="18" rx="1" />
          </svg>
        </button>
      ) : (
        <button
          onClick={resumeTimer}
          className={`${btnClass} w-5 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]`}
          style={noDrag}
          title="Resume timer"
        >
          <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <polygon points="5,3 19,12 5,21" />
          </svg>
        </button>
      )}

      {/* Stop */}
      <button
        onClick={stopTimer}
        className={`${btnClass} w-5 text-[var(--color-status-error-soft)] hover:text-[var(--color-status-error-bright)]`}
        style={noDrag}
        title="Stop timer"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
          <rect x="4" y="4" width="16" height="16" rx="2" />
        </svg>
      </button>

      {/* Elapsed display */}
      <span
        className={`text-[11px] font-mono tabular-nums px-1 select-none ${
          status === 'paused'
            ? 'text-[var(--color-text-muted)] animate-pulse'
            : 'text-[var(--color-text-secondary)]'
        }`}
      >
        {displayText}
      </span>
    </div>
  )
}
