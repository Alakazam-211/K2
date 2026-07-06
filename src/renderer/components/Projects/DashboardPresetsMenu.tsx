// Projects V1 §6.8.4 — the layout-presets menu: a stacked-squares
// button at the RIGHT end of the dashboards|Feedback tab row opening a
// small menu of preset thumbnails (pure CSS divs, no images). Picking
// one RE-TILES the open dashboard's existing panes in reading order
// into that shape (the tileIntoPreset rules; the mounted
// ProjectDashboard applies + saves through the dashboard-dnd preset
// registry). Owners/admins only — a resolved viewer-mode window sees
// no button (the window-mode idiom, same as all layout edits); the
// daemon's owner-or-admin save gate backstops.
//
// Close behavior = the house popover idiom (WorkspaceFilterDropdown):
// outside mousedown closes; capture-phase Esc closes BEFORE the
// page-level Esc-to-pane handler sees it.

import React, { useEffect, useRef, useState } from 'react'
import { useWindowModeStore } from '@/stores/window-mode'
import { requestPreset } from './dashboard-dnd'
import type { PresetShape } from './dashboard-layout'

/** Thumbnail geometry: rows of relative column widths — enough to draw
 *  every §6.8.4 shape as nested flex divs. */
const PRESETS: Array<{ shape: PresetShape; label: string; rows: number[][] }> = [
  { shape: 'single', label: 'Single', rows: [[1]] },
  { shape: 'cols2', label: '2 up', rows: [[1, 1]] },
  { shape: 'cols3', label: '3 up', rows: [[1, 1, 1]] },
  {
    shape: 'grid2x2',
    label: '2 × 2 grid',
    rows: [
      [1, 1],
      [1, 1],
    ],
  },
  {
    shape: 'mainStack',
    label: 'Main + stack',
    // Drawn specially below (the right column stacks two cells).
    rows: [],
  },
]

function PresetThumb({ rows, mainStack }: { rows: number[][]; mainStack?: boolean }): React.JSX.Element {
  const cell = 'bg-[var(--color-text-muted)]/45'
  if (mainStack) {
    return (
      <div className="flex gap-[2px] w-9 h-6 flex-shrink-0">
        <div className={`flex-1 ${cell}`} />
        <div className="flex-1 flex flex-col gap-[2px]">
          <div className={`flex-1 ${cell}`} />
          <div className={`flex-1 ${cell}`} />
        </div>
      </div>
    )
  }
  return (
    <div className="flex flex-col gap-[2px] w-9 h-6 flex-shrink-0">
      {rows.map((cols, i) => (
        <div key={i} className="flex-1 flex gap-[2px]">
          {cols.map((flex, j) => (
            <div key={j} className={cell} style={{ flex }} />
          ))}
        </div>
      ))}
    </div>
  )
}

export default function DashboardPresetsMenu(): React.JSX.Element | null {
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  // Outside click closes; capture-phase Esc closes the popover BEFORE
  // the dashboard's Esc-to-pane handler sees it.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  // §6.8.4 — viewers see no button at all.
  if (readOnly) return null

  return (
    <div ref={rootRef} className="relative flex-shrink-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className={`flex h-6 w-6 items-center justify-center transition-colors cursor-pointer ${
          open
            ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]'
            : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]'
        }`}
        title="Layout presets"
      >
        {/* Stacked squares. */}
        <svg
          width="14"
          height="14"
          viewBox="0 0 14 14"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="1" y="2" width="12" height="10" />
          <line x1="7" y1="2" x2="7" y2="12" />
          <line x1="7" y1="7" x2="13" y2="7" />
        </svg>
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-40 w-44 border border-[var(--color-border)] bg-[var(--color-bg-elevated)] shadow-xl py-1">
          {PRESETS.map((p) => (
            <button
              key={p.shape}
              type="button"
              onClick={() => {
                requestPreset(p.shape)
                setOpen(false)
              }}
              className="w-full flex items-center gap-2.5 px-2.5 py-1.5 text-left text-[11px] text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
            >
              <PresetThumb rows={p.rows} mainStack={p.shape === 'mainStack'} />
              <span className="truncate">{p.label}</span>
            </button>
          ))}
          <p className="px-2.5 pt-1 pb-0.5 text-[9px] leading-snug text-[var(--color-text-muted)] opacity-70">
            Re-tiles the current panes in reading order.
          </p>
        </div>
      )}
    </div>
  )
}
