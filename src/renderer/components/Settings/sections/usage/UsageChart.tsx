// Usage v2 Phase 2 — the day chart: one stacked series per harness for the
// chosen measure, a y-axis, hover totals, a legend, and a table view (B29,
// B31). Geometry is `usageChart` (pure); this file only draws it.

import React, { useMemo, useState } from 'react'
import type { UsageDay } from '../dailyTokenChart'
import {
  CHART_MEASURES,
  fmtExact,
  fmtMeasure,
  harnessColor,
  harnessLabel,
  usageChart,
  type ChartMeasure,
} from './usage-page'

export interface ChartSeries {
  harness: string
  days: UsageDay[] | null | undefined
}

export function UsageChart({
  series,
  slots,
  measure,
  onMeasure,
  error,
  loading,
}: {
  series: ChartSeries[]
  slots: string[]
  measure: ChartMeasure
  onMeasure: (m: ChartMeasure) => void
  /** Shown inside the chart box; the rest of the page stays (B23). */
  error: string | null
  loading: boolean
}): React.JSX.Element {
  const geo = useMemo(() => usageChart(series, measure, slots), [series, measure, slots])
  const [hover, setHover] = useState<number | null>(null)
  const col = hover !== null ? geo.columns[hover] : null
  const measureLabel = CHART_MEASURES.find((m) => m.id === measure)?.label ?? measure
  const first = slots[0] ?? ''
  const last = slots[slots.length - 1] ?? ''
  const empty = geo.columns.every((c) => c.total === 0)

  return (
    <div className="mb-4" data-testid="usage-chart">
      <div className="flex flex-wrap items-center gap-1 mb-2" role="radiogroup" aria-label="Chart measure">
        {CHART_MEASURES.map((m) => (
          <button
            key={m.id}
            type="button"
            role="radio"
            aria-checked={measure === m.id}
            data-testid={`usage-measure-${m.id}`}
            onClick={() => onMeasure(m.id)}
            className={`px-2 py-0.5 text-[11px] border ${
              measure === m.id
                ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {m.label}
          </button>
        ))}
      </div>
      {error ? (
        <p className="text-xs text-amber-400 border border-[var(--color-border)] p-3" data-testid="usage-chart-error" role="alert">
          The chart could not load: {error}
        </p>
      ) : (
        <div className="relative">
          <svg
            viewBox={`0 0 ${geo.width} ${geo.height}`}
            preserveAspectRatio="none"
            className="block w-full h-48"
            role="img"
            aria-label={`${measureLabel} per day, ${first} to ${last}, by harness`}
            onMouseLeave={() => setHover(null)}
          >
            {geo.ticks.map((t) => (
              <line
                key={t.value}
                x1={geo.plotLeft}
                x2={geo.width}
                y1={t.y}
                y2={t.y}
                stroke="var(--color-border)"
                strokeWidth={1}
                vectorEffect="non-scaling-stroke"
              />
            ))}
            {geo.columns.map((c, i) => (
              <g key={c.day} data-day={c.day}>
                {c.segments.map((s) => (
                  <rect
                    key={s.harness}
                    x={s.x}
                    y={s.y}
                    width={s.width}
                    height={s.height}
                    fill={harnessColor(s.harness)}
                    data-harness={s.harness}
                  />
                ))}
                <rect
                  x={c.x}
                  y={geo.plotTop}
                  width={c.width}
                  height={geo.plotBottom - geo.plotTop}
                  fill="transparent"
                  data-testid={`usage-chart-hit-${c.day}`}
                  onMouseEnter={() => setHover(i)}
                />
              </g>
            ))}
          </svg>
          {/* y-axis labels as HTML so they don't stretch with the svg. */}
          <div className="pointer-events-none absolute inset-y-0 left-0" style={{ width: `${(geo.plotLeft / geo.width) * 100}%` }}>
            {geo.ticks.map((t) => (
              <span
                key={t.value}
                className="absolute right-1 -translate-y-1/2 text-[10px] tabular-nums text-[var(--color-text-muted)]"
                style={{ top: `${(t.y / geo.height) * 100}%` }}
              >
                {fmtMeasure(t.value, measure)}
              </span>
            ))}
          </div>
          {col ? (
            <div
              role="tooltip"
              data-testid="usage-chart-tooltip"
              className="pointer-events-none absolute top-0 z-10 -translate-x-1/2 border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2 py-1 text-[11px] shadow-lg"
              style={{ left: `${((col.x + col.width / 2) / geo.width) * 100}%` }}
            >
              <div className="text-[var(--color-text-primary)]">{col.day}</div>
              {col.segments.map((s) => (
                <div key={s.harness} className="flex items-center gap-1.5 text-[var(--color-text-secondary)] tabular-nums">
                  <span className="inline-block h-2 w-2" style={{ background: harnessColor(s.harness) }} />
                  {harnessLabel(s.harness)} {fmtMeasure(s.value, measure)}
                </div>
              ))}
              <div className="text-[var(--color-text-primary)] tabular-nums">
                Total {measure === 'cost' ? fmtMeasure(col.total, measure) : fmtExact(col.total)}
              </div>
            </div>
          ) : null}
          {loading ? (
            <p className="absolute inset-0 flex items-center justify-center text-[11px] text-[var(--color-text-muted)]">Loading…</p>
          ) : empty ? (
            <p className="absolute inset-0 flex items-center justify-center text-[11px] text-[var(--color-text-muted)]">
              Nothing in this range.
            </p>
          ) : null}
        </div>
      )}
      <div className="mt-1 flex justify-between text-[11px] text-[var(--color-text-muted)]" style={{ paddingLeft: `${(geo.plotLeft / geo.width) * 100}%` }}>
        <span>{first}</span>
        <span>{last}</span>
      </div>
      {geo.harnesses.length > 0 ? (
        <div className="mt-1 flex gap-3 text-[11px] text-[var(--color-text-muted)]" data-testid="usage-chart-legend">
          {geo.harnesses.map((h) => (
            <span key={h} className="inline-flex items-center gap-1">
              <span className="inline-block w-2 h-2" style={{ background: harnessColor(h) }} />
              {harnessLabel(h)}
            </span>
          ))}
        </div>
      ) : null}
      {!error && !empty ? (
        <details className="mt-1 text-[11px] text-[var(--color-text-muted)]">
          <summary className="cursor-pointer">Show as table</summary>
          <table className="mt-1 w-full border-collapse" data-testid="usage-chart-table">
            <thead>
              <tr className="text-left">
                <th className="py-0.5 pr-3 font-medium">Day</th>
                {geo.harnesses.map((h) => (
                  <th key={h} className="py-0.5 pr-3 font-medium text-right">
                    {harnessLabel(h)}
                  </th>
                ))}
                <th className="py-0.5 font-medium text-right">Total</th>
              </tr>
            </thead>
            <tbody>
              {geo.columns
                .filter((c) => c.total > 0)
                .map((c) => (
                  <tr key={c.day} className="border-t border-[var(--color-border)]">
                    <td className="py-0.5 pr-3">{c.day}</td>
                    {geo.harnesses.map((h) => (
                      <td key={h} className="py-0.5 pr-3 text-right tabular-nums">
                        {fmtMeasure(c.segments.find((s) => s.harness === h)?.value ?? 0, measure)}
                      </td>
                    ))}
                    <td className="py-0.5 text-right tabular-nums">{fmtMeasure(c.total, measure)}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </details>
      ) : null}
    </div>
  )
}
