// prd-zen-mode-v1 Z27, prd-zen-gardens-v1 G24, G58 and
// prd-zen-freeform-chrome FC1, FC43, FC50 — what K2's controls share.
//
// K2's controls (the Garden switcher, the Zen toggle, the usage tool, the
// theme control, menus) are chrome widgets: the page places them from data
// (`page.placement`), in a band, at a column edge or inside a menu, and
// `ZenBands` draws each one through the chrome registry by kind
// (`zen-registry.tsx`). They bind through the bridge exactly as a v2 user
// page would, so the same check covers them.
//
// Each item learns where it sits from `ZenChromePlaceContext` (which way
// its dropdown opens, which side it lines up with). K2's own extras (the
// usage tool, the theme control) come from the Zen root through
// `ZenK2ChromeContext`, one entry per kind (FC50): drawn with the root's
// state, registered as K2 overlays, absent in safe mode.

import { createContext, useCallback, useEffect, useRef } from 'react'
import type { ZenBindKind } from '@/lib/zen/zen-controls'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAlign, ZenThemeEntry } from '@/lib/zen/zen-page'

/** Ref callback that binds the element as `kind` (React 19 ref cleanup
 *  unbinds it). A `garden-option` passes its Garden id, a `zen-menu` its
 *  menu id. */
export function useZenBind(
  bridge: ZenWidgetBridge,
  kind: ZenBindKind,
  ref?: string,
): (el: HTMLElement | null) => (() => void) | undefined {
  return useCallback(
    (el: HTMLElement | null) => {
      if (!el) return undefined
      return bridge.controls.bind(kind, el, ref)
    },
    [bridge, kind, ref],
  )
}

/** A ref callback that registers the element as a K2 overlay while it is
 *  mounted (FC18, FC26 rule 6: an open K2 menu never covers a control). */
export function useZenK2Overlay(): (el: HTMLElement | null) => void {
  const off = useRef<(() => void) | null>(null)
  useEffect(() => () => off.current?.(), [])
  return useCallback((el: HTMLElement | null) => {
    off.current?.()
    off.current = el ? registerZenK2Overlay(el) : null
  }, [])
}

/** Height of a band's controls row (CSS px). */
export const TEXTING_BAR_HEIGHT_PX = 44

/** Where a chrome item sits (FC18): which row, which way its dropdown
 *  opens (toward the page), and which side it lines up with. */
export interface ZenChromePlace {
  row: 'top' | 'bottom' | 'edge'
  opens: 'down' | 'up'
  align: ZenAlign
}

export const ZEN_TOP_PLACE: ZenChromePlace = Object.freeze({ row: 'top', opens: 'down', align: 'start' }) as ZenChromePlace

export const ZenChromePlaceContext = createContext<ZenChromePlace>(ZEN_TOP_PLACE)

/** FC27: the row is too narrow even with the extras hidden, so the Garden
 *  switcher's name truncates (down to 6 rem). */
export const ZenRowCompactContext = createContext(false)

/** The theme control's state, from the Zen root. */
export interface ZenK2Theme {
  themes: readonly ZenThemeEntry[]
  active: string | null
  onPick(name: string): void
  error: string | null
}

/** K2's own extras, per kind (FC50). Off / null in safe mode. */
export interface ZenK2Chrome {
  /** The usage tool (the top bar's subscription usage chip and menu). */
  usage: boolean
  theme: ZenK2Theme | null
}

export const ZenK2ChromeContext = createContext<ZenK2Chrome>({ usage: false, theme: null })

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
