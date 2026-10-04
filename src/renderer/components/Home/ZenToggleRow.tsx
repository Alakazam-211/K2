// prd-zen-mode-v1 (Rosson 2026-10-04 answer 2) — how you enter Zen: a Zen
// toggle in its own row on the regular Home page, just above Add Agent and
// the collapse button. It starts the CURRENT Home's Zen page; Zen on/off is
// stored per Home and shared by every window (`k2.zen.homes.v1`). Holding
// Shift while toggling enters safe mode (Z29). Desktop only.
//
// This file draws K2's regular (Styles) chrome, not Zen: it may use the
// Styles tokens. Nothing under it renders inside the Zen root.

import { enterZen, useSelectedHomeZenOn } from '@/lib/zen/zen-view'
import { zenAvailable, currentDesktopOs } from '@/lib/zen/zen-platform'
import { ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'

function chordLabel(): string {
  return currentDesktopOs() === 'mac' ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other
}

export function ZenToggleRow(): React.JSX.Element | null {
  const on = useSelectedHomeZenOn()
  if (!zenAvailable()) return null
  return (
    <div data-zen-toggle-row="">
      <button
        type="button"
        role="switch"
        aria-checked={on}
        className="no-drag flex w-full items-center justify-between gap-2 px-3 py-1.5 text-xs bg-[var(--color-bg-surface)] transition-colors text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
        title={`Zen Mode for this Home (${chordLabel()}). Hold Shift for safe mode.`}
        onClick={(e) => enterZen({ safe: e.shiftKey })}
        data-zen-enter=""
      >
        <span>Zen Mode</span>
        <span
          aria-hidden
          className="relative inline-block h-3.5 w-6 border border-[var(--color-border)]"
          style={{ background: on ? 'var(--color-accent)' : 'transparent' }}
        >
          <span
            className="absolute top-[1px] h-2.5 w-2.5 bg-[var(--color-text-secondary)]"
            style={{ left: on ? 11 : 1 }}
          />
        </span>
      </button>
    </div>
  )
}

/** The collapsed rail's version: one square button above Add Agent. */
export function ZenToggleRailButton(): React.JSX.Element | null {
  if (!zenAvailable()) return null
  return (
    <button
      type="button"
      className="no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors"
      title={`Zen Mode (${chordLabel()}). Hold Shift for safe mode.`}
      aria-label="Zen Mode"
      onClick={(e) => enterZen({ safe: e.shiftKey })}
      data-zen-enter-rail=""
    >
      <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden>
        <circle cx="8" cy="8" r="5.5" />
        <path d="M5.5 8c1.2-1.6 3.8-1.6 5 0" strokeLinecap="round" />
      </svg>
    </button>
  )
}
