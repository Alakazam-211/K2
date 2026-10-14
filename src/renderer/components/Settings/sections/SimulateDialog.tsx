// Simulate… (contract-sources-v1 §4.3, §8; PRD §9.2). Owner: B2.
//
// A fake state for testing cards: every card shows a striped TEST badge,
// triggers never fire for it, nothing is written to history, and real
// checks keep running underneath. Off by itself after the time chosen
// (1 h by default).

import React, { useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SimulateState, SourceSimulated } from '@/lib/sources/sources-types'
import { simulateSource } from '@/lib/sources/sources-api'
import { btn, btnPrimary, danger, hhmm, muted, sourcesErrorText } from './data-sources-ui'
import { SquareRadio } from '@/components/ui'

const STATES: Array<{ state: SimulateState; label: string }> = [
  { state: 'crit', label: 'Red' },
  { state: 'warn', label: 'Amber' },
  { state: 'ok', label: 'Green' },
  { state: 'unknown', label: 'Unknown' },
  { state: 'stale', label: 'Stale (no good check)' },
  { state: 'slow', label: 'Slow (updates arrive 20 s late)' },
]

const DURATIONS: Array<{ secs: number; label: string }> = [
  { secs: 600, label: '10 minutes' },
  { secs: 1800, label: '30 minutes' },
  { secs: 3600, label: '1 hour' },
  { secs: 14_400, label: '4 hours' },
]

export function SimulateDialog({
  scope,
  name,
  current,
  onDone,
}: {
  scope: ServerScope
  name: string
  current: SourceSimulated | null
  onDone: () => void
}): React.JSX.Element {
  const [state, setState] = useState<SimulateState>(current?.state ?? 'crit')
  const [secs, setSecs] = useState(3600)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const send = async (s: SimulateState | 'off'): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await simulateSource(scope, name, s, s === 'off' ? undefined : secs)
      onDone()
    } catch (e) {
      setError(sourcesErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-1.5 border border-[var(--color-border)] p-2 max-w-md" data-testid="sources-simulate">
      <p className={`text-[11px] ${muted}`}>
        Cards show this state with a TEST badge. Triggers never fire for it and nothing goes into history. Real checks keep
        running.
      </p>
      {current && (
        <p className="text-[11px] text-[var(--color-text-primary)]">
          Now: TEST {current.state} until {hhmm(current.until)}
          {current.by ? ` (by ${current.by})` : ''}.
        </p>
      )}
      <fieldset className="space-y-0.5">
        {STATES.map((s) => (
          <label key={s.state} className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
            <SquareRadio name={`sim-${name}`} checked={state === s.state} onChange={() => setState(s.state)} />
            {s.label}
          </label>
        ))}
      </fieldset>
      <label className={`block text-[11px] ${muted}`}>
        For
        <select
          className="ml-1.5 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)]"
          value={secs}
          onChange={(e) => setSecs(Number(e.currentTarget.value))}
          data-testid="sources-simulate-for"
        >
          {DURATIONS.map((d) => (
            <option key={d.secs} value={d.secs}>
              {d.label}
            </option>
          ))}
        </select>
      </label>
      <div className="flex gap-1.5">
        <button type="button" className={btnPrimary} disabled={busy} onClick={() => void send(state)} data-testid="sources-simulate-start">
          Simulate
        </button>
        {current && (
          <button type="button" className={btn} disabled={busy} onClick={() => void send('off')} data-testid="sources-simulate-off">
            Turn off
          </button>
        )}
        <button type="button" className={btn} onClick={onDone}>
          Cancel
        </button>
      </div>
      {error && (
        <p className={`text-[11px] ${danger}`} role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
