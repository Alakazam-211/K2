// prd-zen-gardens-v1 G5 — the way into Zen: one square toggle at the end of
// the top bar's right cluster, right of the drawer toggles on Agents and
// Home (after the mode toggle on pages without them). Zen is a mode of the
// window, so it turns THIS window's Zen on; the page underneath stays put.
// Shift-click starts it in safe mode (Z29). In Settings' bar it closes
// Settings and enters Zen. It renders nothing where Zen doesn't exist: the
// web client, Windows until G-Win, Focus and ticket windows.
//
// This draws K2's regular (Styles) chrome, not Zen: it may use the Styles
// tokens. Nothing under it renders inside the Zen root. The way out of Zen
// is the Garden page's own Zen toggle, the app menu, or ⌃⌘Z.

import { enterZen } from '@/lib/zen/zen-view'
import { currentDesktopOs, zenAvailable } from '@/lib/zen/zen-platform'
import { ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'

export function zenTopBarToggleTitle(): string {
  const chord = currentDesktopOs() === 'mac' ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other
  return `Zen Mode (${chord}). Hold Shift for safe mode.`
}

export default function ZenTopBarToggle(): React.JSX.Element | null {
  if (!zenAvailable()) return null
  const title = zenTopBarToggleTitle()
  return (
    <button
      type="button"
      aria-pressed={false}
      aria-label="Zen Mode"
      title={title}
      data-zen-enter=""
      onClick={(e) => enterZen({ safe: e.shiftKey })}
      className="no-drag flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag',
      }}
    >
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden>
        <circle cx="8" cy="8" r="5.5" />
        <path d="M5.5 8c1.2-1.6 3.8-1.6 5 0" strokeLinecap="round" />
      </svg>
    </button>
  )
}
