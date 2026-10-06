// Rosson 2026-10-04 — the app top bar's usage tool (the subscription usage
// chip and its menu, `UsageButton`), the `usage` chrome widget: by default
// in Zen's top band, immediately left of the theme control (usage, theme,
// Zen toggle). A Garden file may move it, tuck it in a menu or leave it
// out (prd-zen-freeform-chrome FC6).
//
// The same component, so the numbers, the followed room and the refresh
// are the top bar's. Zen only restyles it: the shared Zen glass tile
// (`zen-glass.ts`), a pill the size of the theme control, and a frosted
// menu, in Zen tokens (the component's Styles variables are Zen tokens
// under the root's shield). The menu opens
// below, right-aligned, over the page: the wrapper is a stacking layer
// above the page (like the theme picker) and a K2 overlay, so an open menu
// never counts as hiding the page's controls. No portal: it stays inside
// the Zen root, where its Zen colours apply.

import { useCallback } from 'react'
import UsageButton from '@/components/Timer/UsageButton'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import { zenGlassRule } from '@/lib/zen/zen-glass'
import type { ZenChromePlace } from './ZenTemplateControls'

const CHIP = '[data-zen-root] [data-zen-usage] [data-testid="subscription-usage"]'
const MENU = '[data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"]'
/** FC51: the menu opens up from the bottom band or a bottom edge. */
const UP = '[data-zen-root] [data-zen-usage][data-zen-usage-opens="up"] [data-testid="subscription-usage-menu"]'
/** FC51: a start-aligned chip's menu lines up with its left edge. */
const START = '[data-zen-root] [data-zen-usage][data-zen-usage-align="start"] [data-testid="subscription-usage-menu"]'
/** Inside a K2 menu's Usage panel: in flow, no popover. */
const INLINE = '[data-zen-root] [data-zen-usage][data-zen-usage-inline] [data-testid="subscription-usage-menu"]'

/** The usage tool's default place: the top band's end group. */
const ZEN_USAGE_TOP_PLACE: ZenChromePlace = { row: 'top', opens: 'down', align: 'end' }

/** The chip is a shared Zen glass tile (`zen-glass.ts`, tokens on the Zen
 *  root); the menu is the raised glass. */
export const ZEN_USAGE_CSS = `
${zenGlassRule([CHIP])}
${CHIP} {
  height: 26px;
  padding: 0 10px;
  border-radius: 13px;
  color: var(--zen-text-muted);
  font-size: 12px;
}
${CHIP}:hover,
${CHIP}[aria-expanded="true"] {
  color: var(--zen-text);
  background: var(--zen-glass-raised);
}
${MENU} {
  margin-top: 6px;
  padding: 10px 12px;
  border: 1px solid var(--zen-glass-edge);
  border-radius: var(--zen-radius);
  background: var(--zen-glass-raised);
  -webkit-backdrop-filter: var(--zen-glass-blur);
  backdrop-filter: var(--zen-glass-blur);
  box-shadow: var(--zen-glass-sheen), 0 12px 32px color-mix(in srgb, black 16%, transparent);
  color: var(--zen-text);
}
${MENU} [role="progressbar"] {
  border-radius: 999px;
  background: color-mix(in srgb, var(--zen-text) 10%, transparent);
}
${MENU} [role="progressbar"] > div { border-radius: 999px; }
@media (prefers-reduced-transparency: reduce) {
  ${CHIP}:hover,
  ${CHIP}[aria-expanded="true"],
  ${MENU} {
    background: var(--zen-surface-raised);
    -webkit-backdrop-filter: none;
    backdrop-filter: none;
  }
}
${UP} {
  top: auto;
  bottom: 100%;
  margin-top: 0;
  margin-bottom: 6px;
}
${START} {
  left: 0;
  right: auto;
}
${INLINE} {
  position: static;
  margin-top: 6px;
  box-shadow: none;
}
`

/**
 * prd-zen-freeform-chrome FC51: the usage menu is the app's `UsageButton`
 * (`absolute right-0 top-full`), so Zen flips it with its own stylesheet
 * and leaves the top-bar component alone: up from the bottom band or a
 * bottom edge, left-aligned when the chip is start-aligned, and in flow
 * (no popover) inside a K2 menu's Usage panel (`inline`).
 */
export function ZenUsageTool({
  place = ZEN_USAGE_TOP_PLACE,
  inline = false,
}: {
  place?: ZenChromePlace
  inline?: boolean
}): React.JSX.Element {
  const overlay = useCallback((el: HTMLDivElement | null) => (el ? registerZenK2Overlay(el) : undefined), [])
  return (
    <div
      ref={overlay}
      data-zen-usage=""
      data-zen-usage-opens={place.opens}
      data-zen-usage-align={place.align}
      data-zen-usage-inline={inline ? '' : undefined}
      className="no-drag"
      style={{ position: 'relative', flexShrink: 0, zIndex: 20 }}
    >
      <style data-zen-usage-styles="">{ZEN_USAGE_CSS}</style>
      <UsageButton />
    </div>
  )
}
