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

//
// Its icon is `ZenIcon` (off state), chosen in `lib/zen/zen-icon.ts`. While
// `ZEN_ICON_PREVIEW` is on, this renders all three icon candidates side by
// side in a dashed group, each a real Zen toggle (TEMPORARY, Rosson
// 2026-10-04 is picking one).

import { enterZen } from '@/lib/zen/zen-view'
import { currentDesktopOs, zenAvailable } from '@/lib/zen/zen-platform'
import { ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'
import { zenToggleIcons, type ZenIconOption } from '@/lib/zen/zen-icon'
import { ZenIcon } from '@/components/Zen/ZenIcon'

export function zenTopBarToggleTitle(): string {
  const chord = currentDesktopOs() === 'mac' ? ZEN_CHORD_LABEL.mac : ZEN_CHORD_LABEL.other
  return `Zen Mode (${chord}). Hold Shift for safe mode.`
}

function ZenEnterButton({ option, title }: { option: ZenIconOption; title: string }): React.JSX.Element {
  return (
    <button
      type="button"
      aria-pressed={false}
      aria-label="Zen Mode"
      title={title}
      data-zen-enter=""
      data-zen-icon-option={option.variant}
      onClick={(e) => enterZen({ safe: e.shiftKey })}
      className="no-drag flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag',
      }}
    >
      <ZenIcon variant={option.variant} on={false} size={16} />
    </button>
  )
}

export default function ZenTopBarToggle(): React.JSX.Element | null {
  if (!zenAvailable()) return null
  const title = zenTopBarToggleTitle()
  const icons = zenToggleIcons()
  if (icons.length === 1) return <ZenEnterButton option={icons[0]} title={title} />
  // TEMPORARY icon comparison (ZEN_ICON_PREVIEW): one dashed group.
  return (
    <div
      role="group"
      aria-label="Zen icon preview (temporary): pick one"
      data-zen-icon-preview=""
      className="no-drag flex items-center gap-0.5 border border-dashed border-[var(--color-accent)]"
    >
      {icons.map((o) => (
        <ZenEnterButton key={o.variant} option={o} title={`${o.label}. ${title}`} />
      ))}
    </div>
  )
}
