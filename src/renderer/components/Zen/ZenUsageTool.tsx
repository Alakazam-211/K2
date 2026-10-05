// Rosson 2026-10-04 — the app top bar's usage tool (the subscription usage
// chip and its menu, `UsageButton`) in Zen's top band, immediately left of
// the theme control: usage, theme, Zen toggle.
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

const CHIP = '[data-zen-root] [data-zen-usage] [data-testid="subscription-usage"]'
const MENU = '[data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"]'

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
`

export function ZenUsageTool(): React.JSX.Element {
  const overlay = useCallback((el: HTMLDivElement | null) => (el ? registerZenK2Overlay(el) : undefined), [])
  return (
    <div ref={overlay} data-zen-usage="" className="no-drag" style={{ position: 'relative', flexShrink: 0, zIndex: 20 }}>
      <style data-zen-usage-styles="">{ZEN_USAGE_CSS}</style>
      <UsageButton />
    </div>
  )
}
