// prd-zen-gardens-v1 G5 — the way into Zen: one toggle that turns THIS
// window's Zen on; the page underneath stays put. Shift-click starts it in
// safe mode (Z29). From Settings it closes Settings and enters Zen. It
// renders nothing where Zen doesn't exist: the web client, Windows until
// G-Win, Focus and ticket windows.
//
// 0.46.0 experiment (Rosson 2026-10-16, "gardens zen swirl as the sixth
// icon just before the settings icon"): it moved out of the top bar's
// right cluster into the nav bar's page row (`NavPageRow`), between
// Tickets and Settings, drawn like the row's other buttons (`pageTabClass`,
// 13px icon). The app menu and ⌃⌘Z / Ctrl+Alt+Z still enter Zen.
//
// This draws K2's regular (Styles) chrome, not Zen: it may use the Styles
// tokens. Nothing under it renders inside the Zen root. The way out of Zen
// is the Garden page's own Zen toggle, the app menu, or ⌃⌘Z. Zen hides the
// nav (the shell is parked), so the selected state only shows if a nav is
// on screen with Zen on; it follows `useZenShown`.
//
// Its icon is the ensō (`ZenIcon`, off state; the choice is
// `lib/zen/zen-icon.ts`, Rosson 2026-10-04). Pointing at it draws the
// stroke in once more; reduced motion keeps it still.

import { useState } from 'react'
import { enterZen, useZenShown } from '@/lib/zen/zen-view'
import { currentDesktopOs, zenAvailable } from '@/lib/zen/zen-platform'
import { ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'
import { ZEN_ICON_CHOICE } from '@/lib/zen/zen-icon'
import { ZenIcon } from '@/components/Zen/ZenIcon'
import { pageTabClass } from './PageTabs'

/** The button's accessible name in the nav row. */
export const ZEN_NAV_LABEL = 'Gardens'

/** Icon size: the nav row's (the cog's) 13px. */
export const ZEN_NAV_ICON_PX = 13

export function zenTopBarToggleTitle(): string {
  const chord = currentDesktopOs() === 'mac' ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other
  return `Gardens — Zen Mode (${chord}). Hold Shift for safe mode.`
}

function ZenEnterButton({ title }: { title: string }): React.JSX.Element {
  // Each hover replays the icon's draw-in.
  const [hovers, setHovers] = useState(0)
  const shown = useZenShown()
  return (
    <button
      type="button"
      aria-pressed={shown}
      aria-label={ZEN_NAV_LABEL}
      title={title}
      data-zen-enter=""
      onClick={(e) => enterZen({ safe: e.shiftKey })}
      onMouseEnter={() => setHovers((n) => n + 1)}
      className={`no-drag cursor-pointer ${pageTabClass(shown)}`}
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag',
      }}
    >
      <ZenIcon variant={ZEN_ICON_CHOICE} on={false} size={ZEN_NAV_ICON_PX} replay={hovers} />
    </button>
  )
}

export default function ZenTopBarToggle(): React.JSX.Element | null {
  if (!zenAvailable()) return null
  return <ZenEnterButton title={zenTopBarToggleTitle()} />
}
