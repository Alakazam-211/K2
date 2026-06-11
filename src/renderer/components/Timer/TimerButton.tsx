import { useState, useEffect } from 'react'
import { useTimerStore, getElapsedMs, getRemainingMs, formatElapsed } from '@/stores/timer'

const DURATION_PRESETS = [
  { label: '5m', ms: 5 * 60 * 1000 },
  { label: '10m', ms: 10 * 60 * 1000 },
  { label: '15m', ms: 15 * 60 * 1000 },
  { label: '30m', ms: 30 * 60 * 1000 },
  { label: '45m', ms: 45 * 60 * 1000 },
  { label: '1h', ms: 60 * 60 * 1000 },
  { label: '2h', ms: 2 * 60 * 60 * 1000 },
  { label: '3h', ms: 3 * 60 * 60 * 1000 },
]

export default function TimerButton(): React.JSX.Element | null {
  const status = useTimerStore((s) => s.status)
  const visible = useTimerStore((s) => s.visible)
  const pausedElapsed = useTimerStore((s) => s.pausedElapsed)
  const resumeTime = useTimerStore((s) => s.resumeTime)
  const targetDurationMs = useTimerStore((s) => s.targetDurationMs)
  const pauseTimer = useTimerStore((s) => s.pauseTimer)
  const resumeTimer = useTimerStore((s) => s.resumeTimer)
  const stopTimer = useTimerStore((s) => s.stopTimer)
  const startWithDuration = useTimerStore((s) => s.startWithDuration)
  const showExtend = useTimerStore((s) => s.showExtend)
  const showMemoDialog = useTimerStore((s) => s.showMemoDialog)

  // Re-render every second when running
  const [, setTick] = useState(0)
  useEffect(() => {
    if (status !== 'running') return
    const interval = setInterval(() => setTick((t) => t + 1), 1000)
    return () => clearInterval(interval)
  }, [status])

  // Show extend dialog when countdown reaches zero
  useEffect(() => {
    if (status !== 'running' || targetDurationMs == null) return
    const remaining = getRemainingMs({ status, pausedElapsed, resumeTime, targetDurationMs })
    if (remaining <= 0) {
      showExtend()
    }
  }, [status, targetDurationMs, pausedElapsed, resumeTime, showExtend])

  if (!visible) return null

  // Timer is stopped and waiting for memo — hide controls
  // (the MemoDialog handles the rest of the flow)
  if (showMemoDialog) return null

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const noDrag = { WebkitAppRegion: 'no-drag' } as any

  const btnClass =
    'flex h-6 items-center justify-center transition-colors'

  // Idle state: clock icon + duration presets
  if (status === 'idle') {
    return (
      <div className="flex items-center gap-1 no-drag">
        {/* Clock icon */}
        <svg
          className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0"
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

        {/* Duration presets */}
        <div className="flex items-center gap-px">
          {DURATION_PRESETS.map((preset) => (
            <button
              key={preset.label}
              onClick={() => startWithDuration(preset.ms)}
              className="px-1.5 py-0.5 text-[10px] font-mono tabular-nums text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
              style={noDrag}
              title={`Start ${preset.label} countdown`}
            >
              {preset.label}
            </button>
          ))}
        </div>
      </div>
    )
  }

  // Running or paused: show controls + countdown remaining
  const elapsed = getElapsedMs({ status, pausedElapsed, resumeTime })
  const isCountdown = targetDurationMs != null
  const displayMs = isCountdown
    ? Math.max(0, targetDurationMs - elapsed)
    : elapsed
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
        className={`${btnClass} w-5 text-red-400 hover:text-red-300`}
        style={noDrag}
        title="Stop timer"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
          <rect x="4" y="4" width="16" height="16" rx="2" />
        </svg>
      </button>

      {/* Countdown / elapsed display */}
      <span
        className={`text-[11px] font-mono tabular-nums px-1 select-none ${
          status === 'paused' ? 'text-red-400/60 animate-pulse' : 'text-red-400'
        }`}
      >
        {displayText}
      </span>
    </div>
  )
}
