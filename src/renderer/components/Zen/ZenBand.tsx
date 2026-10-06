// Rosson 2026-10-06 and prd-zen-freeform-chrome FC43 — the clip a band's
// (or column edge's) CONTENT widgets sit in. A Garden file places a widget
// in a band with `[[widget]] slot = "top"` (or `"bottom"`), the way it
// places one in a column (`docs/zen-contract.md`). `ZenBands` draws each
// row from `page.placement` and hands every run of content widgets (the nav
// rail as a row, today) to one `ZenBandSlot`.
//
// One component for every band and edge. Which kinds fit a band is the
// daemon schema's one allowlist (`BAND_WIDGET_KINDS`); this file keeps none.
//
// The slot shrinks first (min-width 0, clipped), so a band widget never
// pushes the required controls (Garden switcher, Zen toggle), a menu
// button or K2's extras off-screen, and it never eats the band's empty
// drag space, which keeps its own minimum width (`ZEN_DRAG_MIN_WIDTH_PX`).

/** Where a row draws a run of its content widgets. */
export function ZenBandSlot({ slot, children }: { slot: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div
      data-zen-band={slot}
      className="no-drag flex min-w-0 items-center overflow-hidden"
      // Shrinks before anything else in the row; a little room so the
      // widgets' glass edge and badges aren't clipped.
      style={{ flex: '0 1 auto', gap: 8, padding: '4px 2px', alignSelf: 'center' }}
    >
      {children}
    </div>
  )
}
