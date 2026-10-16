// Usage v2 (Q4, P8) — one subscription window as a T3-style bar: % used,
// the reset countdown, a pace mark (where even spending would be now),
// amber at 75% and red at 90%, and "reset — re-checking" once the window's
// reset time has passed. Shared by the top-bar menu and Settings →
// Language Models → Subs / Tokens so both read the same.

import {
  formatResetsIn,
  limitTone,
  limitToneColor,
  paceMark,
  paceText,
  windowPercent,
  windowReset,
  type UsageWindow,
} from '@/lib/subscription-usage'

/** The text line above the bar: "Session 42% · 3h", "Weekly · reset — re-checking". */
export function limitBarLabel(w: UsageWindow, nowMs: number): string {
  if (windowReset(w, nowMs)) return `${w.label} · reset — re-checking`
  const until = formatResetsIn(w.resetsAt, nowMs)
  return `${w.label} ${windowPercent(w, nowMs)}%${until ? ` · ${until}` : ''}`
}

/** The hover text: "Session 42% used · refreshes in 3h · on pace (40% of the window gone)". */
export function limitBarTitle(w: UsageWindow, nowMs: number): string | undefined {
  if (windowReset(w, nowMs)) return `${w.label} reset; K2 is checking the new numbers`
  const until = formatResetsIn(w.resetsAt, nowMs)
  const pace = paceMark(w, nowMs)
  if (!until && !pace) return undefined
  const parts = [`${w.label} ${windowPercent(w, nowMs)}% used`]
  if (until) parts.push(`refreshes ${until === 'now' ? 'now' : `in ${until}`}`)
  if (pace) parts.push(`${paceText(pace)} (${Math.round(pace.elapsedPct)}% of the window gone)`)
  return parts.join(' · ')
}

export function LimitBar({
  window: w,
  nowMs,
  labelTestId,
  dense = false,
}: {
  window: UsageWindow
  nowMs: number
  /** `data-testid` of the label line. */
  labelTestId?: string
  /** Settings rows: smaller type. */
  dense?: boolean
}): React.JSX.Element {
  const reset = windowReset(w, nowMs)
  const pct = windowPercent(w, nowMs)
  const tone = limitTone(pct)
  const pace = paceMark(w, nowMs)
  const label = limitBarLabel(w, nowMs)
  const title = limitBarTitle(w, nowMs)
  return (
    <div className="min-w-0" data-limit-tone={tone} data-limit-pace={pace?.status}>
      <div
        className={`${dense ? 'text-[9px]' : 'text-[11px]'} text-[var(--color-text-secondary)] truncate`}
        data-testid={labelTestId}
        title={title}
      >
        {label}
        {pace && pace.status !== 'on' ? (
          <span className="text-[var(--color-text-muted)]"> · {paceText(pace)}</span>
        ) : null}
      </div>
      <div
        role="progressbar"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`${w.label} used`}
        aria-valuetext={
          reset ? 'reset, re-checking' : `${pct}% used${pace ? `, ${paceText(pace)}` : ''}`
        }
        className={`relative mt-0.5 w-full overflow-visible bg-[var(--color-border)] ${dense ? 'h-1' : 'h-1.5'}`}
      >
        <div className="h-full" style={{ width: `${pct}%`, backgroundColor: limitToneColor(tone) }} />
        {pace ? (
          <div
            data-testid="limit-pace-mark"
            aria-hidden="true"
            className="absolute -top-0.5 -bottom-0.5 w-0.5 bg-[var(--color-text-primary)]"
            style={{ left: `calc(${pace.elapsedPct}% - 1px)` }}
            title={`Even pace: ${Math.round(pace.elapsedPct)}%`}
          />
        ) : null}
      </div>
    </div>
  )
}
