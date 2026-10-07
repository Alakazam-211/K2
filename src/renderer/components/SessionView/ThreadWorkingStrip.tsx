// The Thread working strip (prd-daemon-activity-and-thread-working-v1 S7:
// TW12–TW13; mockup-thread-working-v1). Live-only: it sits under the last
// message while the agent works on a Thread turn and goes away when the turn
// ends. Nothing in it is stored, and messages keep their own timestamps.
//
// It counts its own clock (1 Hz, from the daemon's `startedAt` /
// `phaseSince`, corrected by `serverNow`), so the Thread list around it
// doesn't re-render every second. Words: `threadStripView`.

import { useEffect, useState, type JSX } from 'react'
import type { ThreadTurn } from './overlayThread'
import { STOPPED_LINGER_MS, threadStripView, type ThreadStripTone } from './threadStrip'

const RULE: Record<ThreadStripTone, string> = {
  working: 'border-l-2 border-solid border-[var(--color-status-working)]',
  monitoring: 'border-l-2 border-solid border-[var(--color-accent)]',
  'needs-you':
    'border-l-2 border-solid border-[var(--color-status-warn-amber)] bg-[color-mix(in_srgb,var(--color-status-warn-amber)_7%,transparent)]',
  stopped: 'border-l-2 border-solid border-[var(--color-text-muted)]',
  // "No update in Nm": a dashed rule, never shown as done.
  unverifiable: 'border-l-2 border-dashed border-[var(--color-text-muted)]',
}

const LABEL_COLOR: Record<ThreadStripTone, string> = {
  working: 'text-[var(--color-status-working)]',
  monitoring: 'text-[var(--color-accent)]',
  'needs-you': 'text-[var(--color-status-warn-amber)]',
  stopped: 'text-[var(--color-text-secondary)]',
  unverifiable: 'text-[var(--color-text-secondary)]',
}

const MARK: Record<ThreadStripTone, string> = {
  working: 'bg-[var(--color-status-working)] thread-strip-pulse',
  monitoring: 'border-2 border-[var(--color-accent)]',
  'needs-you': 'bg-[var(--color-status-warn-amber)]',
  stopped: 'border border-[var(--color-border)]',
  unverifiable: 'border border-dashed border-[var(--color-text-muted)]',
}

/** A tool line: backtick spans become code. */
function StepText({ text }: { text: string }): JSX.Element {
  const parts = text.split('`')
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 && i < parts.length - 1 ? (
          <code
            key={i}
            className="px-1 font-mono text-[0.95em] bg-[var(--color-bg-elevated)] border border-[var(--color-border)]"
          >
            {p}
          </code>
        ) : (
          <span key={i}>{i % 2 === 1 ? `\`${p}` : p}</span>
        ),
      )}
    </>
  )
}

/** This client's clock, re-read every second while the turn runs, and once
 *  more when a "Stopped" linger runs out. */
function useStripNow(turn: ThreadTurn): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    setNow(Date.now())
    if (turn.end === null) {
      const id = setInterval(() => setNow(Date.now()), 1_000)
      return () => clearInterval(id)
    }
    if (turn.end?.reason === 'interrupted') {
      const left = turn.end.at + STOPPED_LINGER_MS - (Date.now() + turn.skewMs)
      if (left <= 0) return
      const id = setTimeout(() => setNow(Date.now()), left + 50)
      return () => clearTimeout(id)
    }
  }, [turn])
  return now
}

export function ThreadWorkingStrip({
  turn,
  onStop,
}: {
  turn: ThreadTurn
  /** Sends Esc to the agent's session. Absent: no Stop (a view that can't
   *  type into the session). */
  onStop?: () => void
}): JSX.Element | null {
  const now = useStripNow(turn)
  const view = threadStripView(turn, now)
  if (!view) return null
  return (
    <div
      role="status"
      data-testid="thread-working-strip"
      data-tone={view.tone}
      data-phase={turn.phase}
      className={`thread-strip flex flex-col gap-[3px] pl-2.5 pr-2 pt-1.5 pb-[7px] min-w-0 ${RULE[view.tone]}`}
    >
      <div className="flex items-center gap-2 min-w-0">
        <span aria-hidden className={`inline-block flex-none w-2 h-2 box-border ${MARK[view.tone]}`} />
        <span data-testid="thread-strip-state" className={`text-[11px] font-bold ${LABEL_COLOR[view.tone]}`}>
          {view.label}
        </span>
        <span
          data-testid="thread-strip-since"
          className="min-w-0 truncate text-[11px] text-[var(--color-text-muted)] tabular-nums"
        >
          · {view.since}
        </span>
        {view.stoppable && onStop && (
          <button
            type="button"
            data-testid="thread-strip-stop"
            title="Sends Esc to the agent"
            onClick={onStop}
            className="ml-auto flex-none px-1.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] cursor-pointer"
          >
            Stop
          </button>
        )}
      </div>
      {view.step && (
        <div data-testid="thread-strip-step" className="flex items-baseline gap-2 min-w-0 text-[11px] text-[var(--color-text-primary)]">
          {view.step.kind === 'needs' ? (
            <span aria-hidden className="flex-none w-[1ch] text-[var(--color-status-warn-amber)]">!</span>
          ) : (
            <span
              aria-hidden
              className={`braille-spinner flex-none font-mono ${
                view.tone === 'monitoring' ? 'text-[var(--color-accent)]' : 'text-[var(--color-status-working)]'
              }`}
            />
          )}
          <span
            data-testid="thread-strip-step-text"
            className={`min-w-0 overflow-hidden text-ellipsis whitespace-nowrap ${
              view.step.kind === 'thinking' ? 'thread-strip-shimmer' : ''
            }`}
          >
            {view.step.kind === 'line' ? <StepText text={view.step.text} /> : view.step.text}
          </span>
          {view.step.elapsed && (
            <span className="ml-auto flex-none text-[var(--color-text-muted)] tabular-nums">{view.step.elapsed}</span>
          )}
        </div>
      )}
      {view.tally.length > 0 && (
        <div data-testid="thread-strip-tally" className="text-[10px] text-[var(--color-text-muted)]">
          {view.tally.join(' · ')}
        </div>
      )}
    </div>
  )
}
