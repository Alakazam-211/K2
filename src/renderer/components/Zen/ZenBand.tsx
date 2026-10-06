// Rosson 2026-10-06 — a page band's widget slot. A Garden file places a
// widget in a band with `[[widget]] slot = "top"`, the way it places one in
// a column (`docs/zen-contract.md`). The page (`ZenPage`) draws each band
// widget with its own bridge and hands the nodes, by slot, to the template
// through `ZenBandContext`; the template puts `<ZenBandSlot slot="…" />`
// where that band's widgets go (the top band: immediately right of the
// Garden switcher, before the drag region).
//
// One component for every band: a later bottom band, corner or menu slot is
// a new `slot` value and another `<ZenBandSlot>` in the template, not new
// code here. Which kinds fit a band is the daemon schema's one allowlist
// (`BAND_WIDGET_KINDS`); this file keeps none.
//
// The slot shrinks first (min-width 0, clipped), so a band widget never
// pushes the template's required controls (Garden switcher, Zen toggle) or
// K2's top-right items off-screen, and it never covers the drag region,
// which keeps its own minimum width (`ZEN_DRAG_MIN_WIDTH_PX`).

import { createContext, useContext } from 'react'

/** The page's band widgets, drawn, by slot (`top` → nodes). */
export const ZenBandContext = createContext<ReadonlyMap<string, React.ReactNode>>(new Map())

/** Where a template draws a band's widgets. Nothing (no wrapper at all)
 *  when the Garden puts none there, so a default page is unchanged. */
export function ZenBandSlot({ slot }: { slot: string }): React.JSX.Element | null {
  const nodes = useContext(ZenBandContext).get(slot)
  if (nodes === undefined || nodes === null) return null
  return (
    <div
      data-zen-band={slot}
      className="no-drag flex min-w-0 items-center overflow-hidden"
      // Shrinks before anything else in the band; a little room so the
      // widgets' glass edge and badges aren't clipped.
      style={{ flex: '0 1 auto', gap: 8, padding: '4px 2px', alignSelf: 'center' }}
    >
      {nodes}
    </div>
  )
}
