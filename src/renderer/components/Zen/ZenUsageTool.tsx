// Rosson 2026-10-04 — the app top bar's usage tool (the subscription usage
// chip and its menu, `UsageButton`) in Zen's top band, immediately left of
// the theme control: usage, theme, Zen toggle.
//
// The same component, so the numbers, the followed room and the refresh
// are the top bar's. Zen only restyles it: a liquid glass pill the size of
// the theme control, and a frosted menu, in Zen tokens (the component's
// Styles variables are Zen tokens under the root's shield). The menu opens
// below, right-aligned, over the page: the wrapper is a stacking layer
// above the page (like the theme picker) and a K2 overlay, so an open menu
// never counts as hiding the page's controls. No portal: it stays inside
// the Zen root, where its Zen colours apply.

import { useCallback } from 'react'
import UsageButton from '@/components/Timer/UsageButton'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'

const GLASS = 'color-mix(in srgb, var(--zen-surface) 62%, transparent)'
const GLASS_RAISED = 'color-mix(in srgb, var(--zen-surface-raised) 78%, transparent)'
const EDGE = 'color-mix(in srgb, var(--zen-border) 55%, color-mix(in srgb, var(--zen-text) 14%, transparent))'
const BLUR = 'blur(20px) saturate(1.5)'

export const ZEN_USAGE_CSS = `
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage"] {
  height: 26px;
  padding: 0 10px;
  border-radius: 13px;
  border: 1px solid ${EDGE};
  background: ${GLASS};
  -webkit-backdrop-filter: ${BLUR};
  backdrop-filter: ${BLUR};
  box-shadow: inset 0 1px 0 color-mix(in srgb, white 20%, transparent);
  color: var(--zen-text-muted);
  font-size: 12px;
}
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage"]:hover,
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage"][aria-expanded="true"] {
  color: var(--zen-text);
  background: ${GLASS_RAISED};
}
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"] {
  margin-top: 6px;
  padding: 10px 12px;
  border: 1px solid ${EDGE};
  border-radius: var(--zen-radius);
  background: ${GLASS_RAISED};
  -webkit-backdrop-filter: ${BLUR};
  backdrop-filter: ${BLUR};
  box-shadow: inset 0 1px 0 color-mix(in srgb, white 18%, transparent), 0 12px 32px color-mix(in srgb, black 16%, transparent);
  color: var(--zen-text);
}
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"] [role="progressbar"] {
  border-radius: 999px;
  background: color-mix(in srgb, var(--zen-text) 10%, transparent);
}
[data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"] [role="progressbar"] > div { border-radius: 999px; }
@media (prefers-reduced-transparency: reduce) {
  [data-zen-root] [data-zen-usage] [data-testid="subscription-usage"],
  [data-zen-root] [data-zen-usage] [data-testid="subscription-usage-menu"] {
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
