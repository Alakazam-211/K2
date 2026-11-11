// Projects V1 §6.8.4 — the layout-presets menu: "Layout" in the project
// bar (ProjectBar.tsx) opening a small menu of preset thumbnails (pure
// CSS divs, no images). Picking one RE-TILES the open dashboard's
// existing panes in reading order into that shape (the tileIntoPreset
// rules; the mounted ProjectDashboard applies + saves through the
// dashboard-dnd preset registry). Owners/admins only — the bar leaves it
// out for a Member login (the daemon's owner-or-admin save gate) and a
// resolved viewer-mode window sees no button (the window-mode idiom).
//
// The menu is the bar's shared panel (bar-menu.tsx): Frost in Glass, a
// cut solid box in Terminal; arrows move, Esc closes and gives focus back.

import React, { useState } from 'react'
import { Button } from '@/components/ui'
import { useWindowModeStore } from '@/stores/window-mode'
import { BarMenuItem, BarMenuPanel, Chevron, useBarMenu } from './bar-menu'
import { requestPreset } from './dashboard-dnd'
import type { PresetShape } from './dashboard-layout'

/** Thumbnail geometry: rows of relative column widths — enough to draw
 *  every §6.8.4 shape as nested flex divs. */
const PRESETS: Array<{ shape: PresetShape; label: string; rows: number[][] }> = [
  { shape: 'single', label: 'One tile', rows: [[1]] },
  { shape: 'cols2', label: 'Two side by side', rows: [[1, 1]] },
  { shape: 'cols3', label: 'Three side by side', rows: [[1, 1, 1]] },
  {
    shape: 'grid2x2',
    label: 'Grid of four',
    rows: [
      [1, 1],
      [1, 1],
    ],
  },
  {
    shape: 'mainStack',
    label: 'One large, two stacked',
    // Drawn specially below (the right column stacks two cells).
    rows: [],
  },
]

function PresetThumb({ rows, mainStack }: { rows: number[][]; mainStack?: boolean }): React.JSX.Element {
  const cell = 'bg-[var(--color-text-secondary)]/45'
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

export default function DashboardPresetsMenu({ showWord = true }: { showWord?: boolean }): React.JSX.Element | null {
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const [open, setOpen] = useState(false)
  const menu = useBarMenu(open, setOpen, { minWidth: 200, estimatedHeight: 220 })

  // §6.8.4 — viewers see no button at all.
  if (readOnly) return null

  return (
    <>
      <Button
        ref={menu.anchorRef}
        variant="quiet"
        className="flex items-center gap-1.5 flex-shrink-0"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={showWord ? undefined : 'Layout'}
        title="Lay out this dashboard’s tiles"
        onClick={() => setOpen(!open)}
        data-layout-menu=""
      >
        {/* Two columns, the right one split. */}
        <svg
          width="14"
          height="14"
          viewBox="0 0 14 14"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          className="block flex-shrink-0"
        >
          <rect x="1" y="2" width="12" height="10" />
          <line x1="7" y1="2" x2="7" y2="12" />
          <line x1="7" y1="7" x2="13" y2="7" />
        </svg>
        {showWord && <span>Layout</span>}
        <Chevron open={open} />
      </Button>

      {open && (
        <BarMenuPanel menu={menu} label="Layout" onClose={() => setOpen(false)} testId="layout-menu">
          {PRESETS.map((p) => (
            <BarMenuItem
              key={p.shape}
              onSelect={() => {
                requestPreset(p.shape)
                setOpen(false)
                menu.anchorRef.current?.focus({ preventScroll: true })
              }}
            >
              <PresetThumb rows={p.rows} mainStack={p.shape === 'mainStack'} />
              <span className="truncate">{p.label}</span>
            </BarMenuItem>
          ))}
          <p className="px-3 pt-1.5 pb-1 mt-1 border-t border-[var(--color-border)] type-hint text-[var(--color-text-secondary)] max-w-[15rem]">
            Puts this dashboard’s tiles into that shape, in reading order.
          </p>
        </BarMenuPanel>
      )}
    </>
  )
}
