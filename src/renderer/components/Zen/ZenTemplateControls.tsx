// prd-zen-mode-v1 Z27 and prd-zen-gardens-v1 G24, G58 — what a template's
// own controls share. The controls themselves (Garden switcher, drag strip,
// Zen toggle) are ONE component set, `widgets/ZenTextingControls`,
// registered for both built-in templates and drawn in safe mode too. They
// are drawn by the TEMPLATE, not by K2's shell: they bind through the
// bridge exactly as a v2 user page would, so the same check covers them.
//
// K2's own top-right items (the usage tool, then the theme control) are
// handed to the template through `ZenK2TopRightContext`, so the template
// places them immediately left of its Zen toggle (Rosson 2026-10-04: usage,
// theme, Zen toggle). They stay K2's: drawn by the Zen root with its state,
// registered as K2 overlays, absent in safe mode.

import { createContext, useCallback, useContext, useRef } from 'react'
import type { ZenBindKind } from '@/lib/zen/zen-controls'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'

/** Ref callback that binds the element as `kind` (React 19 ref cleanup
 *  unbinds it). A `garden-option` passes its Garden id. */
export function useZenBind(
  bridge: ZenWidgetBridge,
  kind: ZenBindKind,
  gardenId?: string,
): (el: HTMLElement | null) => (() => void) | undefined {
  return useCallback(
    (el: HTMLElement | null) => {
      if (!el) return undefined
      return bridge.controls.bind(kind, el, gardenId)
    },
    [bridge, kind, gardenId],
  )
}

/** Height of the template's top band (CSS px). */
export const TEXTING_BAR_HEIGHT_PX = 44

/** K2's items for the template's top-right cluster (usage, then theme). */
export const ZenK2TopRightContext = createContext<React.ReactNode>(null)

/** Where a template draws K2's top-right items: immediately left of its
 *  Zen toggle. */
export function ZenK2TopRightItems(): React.JSX.Element | null {
  const items = useContext(ZenK2TopRightContext)
  return items ? <>{items}</> : null
}

/** An Add agent button's click: open (or close) K2's Add agent picker
 *  above `el` through the bridge (`agents.add`), for the calling Agents
 *  widget's Home. A failure is loud in the console; the page keeps
 *  working. */
export function useZenAddAgentClick(bridge: ZenWidgetBridge): {
  ref: React.RefObject<HTMLButtonElement | null>
  onClick(): void
} {
  const ref = useRef<HTMLButtonElement | null>(null)
  const onClick = useCallback(() => {
    void Promise.resolve()
      .then(() => bridge.call('agents.add', { anchor: ref.current, toggle: true }))
      .catch((err: unknown) => console.warn('[zen] Add agent failed:', err))
  }, [bridge])
  return { ref, onClick }
}
