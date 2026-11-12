// Usage v2 Phase 2 — the day chart: one stacked series per harness for the
// chosen total, a y-axis, hover totals, a legend, and a table view (B29,
// B31). Geometry is `usageChart` (pure); this file only draws it, at the
// card's real width so columns and text never stretch.
//
// Drawn with tokens only: series are the theme's chart colours (each at
// least 3:1 on the card), gridlines the hairline border, axis text the
// muted text colour. The hover card is floating chrome (frost in Glass).

import React, { useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { UsageDay } from '../dailyTokenChart'
import { Button, floatMaterial } from '@/components/ui'
import { fmtDay, fmtExact, fmtMeasure, harnessColor, harnessLabel, usageChart, MEASURE_LABEL, type ChartMeasure } from './usage-page'

export interface ChartSeries {
  harness: string
  days: UsageDay[] | null | undefined
}

/** A swatch beside a name: identity is never colour alone (the name says it). */
export function SeriesKey({ harness }: { harness: string }): React.JSX.Element {
  return <span aria-hidden="true" className="inline-block h-2.5 w-2.5 flex-shrink-0 rounded-[var(--radius-status)]" style={{ background: harnessColor(harness) }} />
}

export function ChartLegend({ harnesses }: { harnesses: string[] }): React.JSX.Element | null {
  if (harnesses.length < 2) return null
  return (
    <div className="flex items-center gap-3 type-hint text-[var(--color-text-secondary)]" data-testid="usage-chart-legend">
      {harnesses.map((h) => (
        <span key={h} className="inline-flex items-center gap-1.5">
          <SeriesKey harness={h} />
          {harnessLabel(h)}
        </span>
      ))}
    </div>
  )
}

/** Room one day label needs on the axis, gap included ("Nov 12" in the
 *  hint type), in CSS px. */
export const AXIS_LABEL_SLOT_PX = 56

/** Day labels: every day for a week or so, else about five on an even
 *  step, always the first and the last. `room` caps how many fit the axis
 *  (a narrow card, the app zoomed in): the middle ones go first, then the
 *  first; the last day always shows. */
export function axisDays(n: number, room = Infinity): number[] {
  if (n <= 0) return []
  const most = Math.min(n <= 8 ? n : 5, Math.max(1, Math.floor(room)))
  if (most >= n) return Array.from({ length: n }, (_, i) => i)
  if (most === 1) return [n - 1]
  const step = Math.ceil((n - 1) / (most - 1))
  // A label close before the last one is left out: one within half a step,
  // and, on a measured axis, one less than a label's room from it.
  const last = Number.isFinite(room) ? Math.min(n - 1 - step / 2, (n - 1) * (1 - 1 / room) + 1e-9) : n - 1 - step / 2
  const out: number[] = []
  for (let i = 0; i < last; i += step) out.push(i)
  out.push(n - 1)
  return out
}

export function UsageChart({
  series,
  slots,
  measure,
  loading,
}: {
  series: ChartSeries[]
  slots: string[]
  measure: ChartMeasure
  loading: boolean
}): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = box.current
    if (!el) return
    // clientWidth is CSS px, like the SVG's width and the tooltip's offset;
    // a rect is on-screen px, which the app zoom scales (lib/app-zoom).
    const read = (): void => setWidth(el.clientWidth)
    read()
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(read)
    ro?.observe(el)
    return () => ro?.disconnect()
  }, [])
  const geo = useMemo(() => usageChart(series, measure, slots, width || undefined), [series, measure, slots, width])
  const [hover, setHover] = useState<number | null>(null)
  const [table, setTable] = useState(false)
  const col = hover !== null ? geo.columns[hover] : null
  const label = MEASURE_LABEL[measure]
  const first = slots[0] ?? ''
  const last = slots[slots.length - 1] ?? ''
  const empty = geo.columns.every((c) => c.total === 0)
  const fmtTotal = (v: number): string => (measure === 'cost' ? fmtMeasure(v, measure) : fmtExact(v))

  return (
    <div className="px-4 pt-4 pb-3" data-testid="usage-chart">
      <div ref={box} className="relative" style={{ height: geo.height }} onMouseLeave={() => setHover(null)}>
        {width > 0 && !loading ? (
          <svg
            width={geo.width}
            height={geo.height}
            className="block overflow-visible"
            role="img"
            aria-label={`${label} per day, ${fmtDay(first)} to ${fmtDay(last)}, by provider`}
          >
            {geo.ticks.map((t, i) => (
              <line
                key={t.value}
                x1={geo.plotLeft}
                x2={geo.width}
                y1={Math.round(t.y) + 0.5}
                y2={Math.round(t.y) + 0.5}
                stroke={i === 0 ? 'var(--color-control-edge)' : 'var(--color-border)'}
                strokeWidth={1}
                shapeRendering="crispEdges"
              />
            ))}
            {geo.columns.map((c, i) => (
              <g key={c.day} data-day={c.day}>
                {hover === i ? (
                  <rect x={c.x} y={geo.plotTop} width={c.width} height={geo.plotBottom - geo.plotTop} fill="var(--color-wash-1)" />
                ) : null}
                {c.segments.map((s) => (
                  <rect key={s.harness} x={s.x} y={s.y} width={s.width} height={s.height} fill={harnessColor(s.harness)} data-harness={s.harness} />
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
        ) : null}
        {/* The y-axis labels, as text so they stay crisp and themed. */}
        {!empty && !loading
          ? geo.ticks.map((t) => (
              <span
                key={t.value}
                aria-hidden="true"
                className="pointer-events-none absolute left-0 -translate-y-1/2 type-hint tabular-nums text-[var(--color-text-muted)] text-right"
                style={{ top: t.y, width: geo.plotLeft - 8 }}
              >
                {fmtMeasure(t.value, measure)}
              </span>
            ))
          : null}
        {col && col.total > 0 ? (
          <div
            role="tooltip"
            data-testid="usage-chart-tooltip"
            {...floatMaterial('popover', 'frost')}
            data-k2-chamfer="surface"
            className="pointer-events-none absolute top-0 z-10 min-w-[10rem] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] rounded-[var(--radius-box)] px-3 py-2 type-hint"
            style={
              col.x + col.width / 2 > geo.width / 2
                ? { right: geo.width - col.x + 8 }
                : { left: col.x + col.width + 8 }
            }
          >
            <div className="text-[var(--color-text-primary)] mb-1">{fmtDay(col.day, true)}</div>
            {col.segments
              .slice()
              .reverse()
              .map((s) => (
                <div key={s.harness} className="flex items-center gap-2 text-[var(--color-text-secondary)] tabular-nums">
                  <SeriesKey harness={s.harness} />
                  <span className="flex-1">{harnessLabel(s.harness)}</span>
                  <span>{fmtTotal(s.value)}</span>
                </div>
              ))}
            {col.segments.length > 1 ? (
              <div className="flex justify-between gap-2 mt-1 pt-1 border-t border-[var(--color-border)] text-[var(--color-text-primary)] tabular-nums">
                <span>Total</span>
                <span>{fmtTotal(col.total)}</span>
              </div>
            ) : null}
          </div>
        ) : null}
        {loading ? (
          <p role="status" className="absolute inset-0 flex items-center justify-center type-hint text-[var(--color-text-secondary)]">
            Loading usage…
          </p>
        ) : empty ? (
          <p className="absolute inset-0 flex items-center justify-center type-hint text-[var(--color-text-secondary)]">
            No {label.toLowerCase()} on any day in this range.
          </p>
        ) : null}
      </div>
      <div className="relative mt-1.5 h-4 type-hint tabular-nums text-[var(--color-text-muted)]" aria-hidden="true">
        {width > 0 && !loading
          ? axisDays(geo.columns.length, (geo.width - geo.plotLeft) / AXIS_LABEL_SLOT_PX).map((i, k, shown) => {
              const c = geo.columns[i]
              const mid = c.x + c.width / 2
              // A long axis, or one with labels left out, pins its first and
              // last labels to the plot's edges, so none sticks out of the card.
              const pinned = geo.columns.length > 8 || shown.length < geo.columns.length
              const end = k === shown.length - 1 && pinned
              const start = k === 0 && i === 0 && pinned && shown.length > 1
              return (
                <span
                  key={c.day}
                  className="absolute top-0 whitespace-nowrap"
                  style={end ? { right: 0 } : start ? { left: c.x } : { left: mid, transform: 'translateX(-50%)' }}
                >
                  {fmtDay(c.day)}
                </span>
              )
            })
          : null}
      </div>
      {!empty && !loading ? (
        <div className="mt-3 flex justify-end">
          <Button variant="quiet" aria-expanded={table} data-testid="usage-chart-table-toggle" onClick={() => setTable((v) => !v)}>
            {table ? 'Hide the numbers' : 'Show the numbers'}
          </Button>
        </div>
      ) : null}
      {table && !empty ? (
        <table className="mt-2 w-full border-collapse type-hint tabular-nums" data-testid="usage-chart-table">
          <thead>
            <tr className="text-left text-[var(--color-text-secondary)]">
              <th className="py-1 pr-3 font-medium">Day</th>
              {geo.harnesses.map((h) => (
                <th key={h} className="py-1 pr-3 font-medium text-right">
                  {harnessLabel(h)}
                </th>
              ))}
              <th className="py-1 font-medium text-right">Total</th>
            </tr>
          </thead>
          <tbody className="text-[var(--color-text-primary)]">
            {geo.columns
              .filter((c) => c.total > 0)
              .map((c) => (
                <tr key={c.day} className="border-t border-[var(--color-border)]">
                  <td className="py-1 pr-3">{fmtDay(c.day, true)}</td>
                  {geo.harnesses.map((h) => (
                    <td key={h} className="py-1 pr-3 text-right">
                      {fmtTotal(c.segments.find((s) => s.harness === h)?.value ?? 0)}
                    </td>
                  ))}
                  <td className="py-1 text-right">{fmtTotal(c.total)}</td>
                </tr>
              ))}
          </tbody>
        </table>
      ) : null}
    </div>
  )
}

/** The page's one retry control, the same everywhere a part fails. */
export function RetryButton({ onClick }: { onClick: () => void }): React.JSX.Element {
  return (
    <Button className="flex-shrink-0" onClick={onClick}>
      Try again
    </Button>
  )
}
