// The tab strip's pin (Rosson 2026-10-16): any tab can be pinned, from the
// tab's right-click menu (Pin Tab / Unpin Tab). Nothing about pinning shows
// on hover (Rosson 2026-10-16, 0.46.0): hovering a tab only swaps its
// status slot for × (– on a heartbeat / API tab), pinned or not. At rest a
// pinned tab shows a small static pin where the live / reaped square would
// be. See `lib/pinned-tabs.ts` for ordering.

import { PinIcon } from '@/components/icons/K2Icons'

const PIN_PX = 12

/** A pinned tab's resting mark in its status slot. Static: never a button,
 *  never shown on hover (hover shows the close glyph instead). */
export function PinnedTabMark(): React.JSX.Element {
  return (
    <span data-tab-pinned-glyph="" className="flex text-[var(--color-accent)]">
      <PinIcon size={PIN_PX} />
    </span>
  )
}
