// A dropdown menu that no ancestor can clip (Rosson 2026-10-04: "the
// options are cut-off because the agent section borders cuts it off").
//
// The menu is portalled out of its trigger's tree and placed with
// `position: fixed` against the trigger's rect, so `overflow: hidden` /
// `auto` on a card, a sidebar or a scroller never cuts it off:
//   - inside Zen it portals into the Zen root (`[data-zen-root]`), so it keeps
//     Zen's theme tokens (the root's CSS variables) and sits above every Zen
//     layer (the window-control cluster, the Add agent picker);
//   - elsewhere it portals into `document.body`, one layer above the
//     trigger's highest z-index ancestor (floor 400, like `SettingDropdown`).
// It opens below the trigger and flips upward when there isn't room below
// but there is more above (`prefer: 'up'` reverses that: Zen's bottom band
// opens toward the page). It lines up with the trigger's left edge, or its
// right edge with `align: 'end'`. It follows the trigger on scroll and resize, and
// closes on a mousedown outside both the trigger and the menu, or on Escape.
//
// Usage:
//   const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose })
//   <div ref={menu.anchorRef}>…trigger…</div>
//   {open && menu.portal(<div ref={menu.menuRef} style={menu.style}>…</div>)}

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'

/** Above every Zen layer (cluster 3, content 1, Add agent picker 30). */
export const ZEN_ANCHORED_MENU_LAYER = 40
/** Kept clear of the window's edges. */
export const ANCHORED_MENU_MARGIN = 8

export interface AnchoredMenuBox {
  placement: 'down' | 'up'
  top?: number
  bottom?: number
  left: number
  /** The trigger's width (`width: 'match'`), else unset. */
  width?: number
  minWidth: number
}

/** Where the menu goes for a trigger at `anchor` in a `viewport`-sized
 *  window, for a menu `menuHeight` tall and `menuWidth` wide (pure). */
export function anchoredMenuBox(
  anchor: { top: number; bottom: number; left: number; width: number },
  viewport: { width: number; height: number },
  menuHeight: number,
  opts: {
    gap: number
    width: 'match' | 'min'
    minWidth: number
    menuWidth: number
    /** The side it opens toward when both fit (default `down`). Zen opens
     *  toward the page: down from the top band, up from the bottom band
     *  (prd-zen-freeform-chrome FC18). It still flips when there's no room. */
    prefer?: 'down' | 'up'
    /** Line up with the trigger's left edge (`start`, default) or its
     *  right edge (`end`). */
    align?: 'start' | 'end'
  },
): AnchoredMenuBox {
  const m = ANCHORED_MENU_MARGIN
  const spaceBelow = viewport.height - m - (anchor.bottom + opts.gap)
  const spaceAbove = anchor.top - opts.gap - m
  const up =
    opts.prefer === 'up'
      ? !(menuHeight > spaceAbove && spaceBelow > spaceAbove)
      : menuHeight > spaceBelow && spaceAbove > spaceBelow
  const minWidth = Math.max(opts.minWidth, anchor.width)
  const shownWidth = opts.width === 'match' ? anchor.width : Math.max(minWidth, opts.menuWidth)
  // Keep the menu inside the window horizontally.
  const wanted = opts.align === 'end' ? anchor.left + anchor.width - shownWidth : anchor.left
  const left = Math.max(m, Math.min(wanted, viewport.width - m - shownWidth))
  return {
    placement: up ? 'up' : 'down',
    ...(up ? { bottom: viewport.height - anchor.top + opts.gap } : { top: anchor.bottom + opts.gap }),
    left,
    ...(opts.width === 'match' ? { width: anchor.width } : {}),
    minWidth: opts.width === 'match' ? anchor.width : minWidth,
  }
}

export interface AnchoredMenu<A extends HTMLElement> {
  /** The trigger (or the trigger's wrapper): the menu is placed against it. */
  anchorRef: React.MutableRefObject<A | null>
  /** The menu's outer element (outside clicks inside it don't close). */
  menuRef: React.RefCallback<HTMLDivElement>
  /** Fixed position, layer, width and inherited font size for the menu. */
  style: React.CSSProperties
  placement: 'down' | 'up'
  /** Renders the menu into the Zen root (in Zen) or `document.body`. */
  portal(node: React.ReactNode): React.ReactPortal | null
}

export function useAnchoredMenu<A extends HTMLElement = HTMLElement>({
  open,
  onClose,
  gap = 2,
  width = 'match',
  minWidth = 0,
  estimatedHeight = 240,
  prefer = 'down',
  align = 'start',
}: {
  open: boolean
  onClose(): void
  /** Pixels between the trigger and the menu. */
  gap?: number
  /** `match`: exactly the trigger's width; `min`: at least the trigger's
   *  (and `minWidth`), wider when its content is. */
  width?: 'match' | 'min'
  minWidth?: number
  /** Used for the flip until the menu can be measured. */
  estimatedHeight?: number
  /** The side it opens toward when both fit (it flips when there's no room). */
  prefer?: 'down' | 'up'
  /** Line up with the trigger's left (`start`) or right (`end`) edge. */
  align?: 'start' | 'end'
}): AnchoredMenu<A> {
  const anchorRef = useRef<A | null>(null)
  const menuEl = useRef<HTMLDivElement | null>(null)
  const [box, setBox] = useState<(AnchoredMenuBox & { zIndex: number; fontSize?: string }) | null>(null)
  const onCloseRef = useRef(onClose)
  onCloseRef.current = onClose

  const place = useCallback(() => {
    const anchor = anchorRef.current
    if (!anchor) return
    const rect = anchor.getBoundingClientRect()
    const menuRect = menuEl.current?.getBoundingClientRect()
    const height = menuRect && menuRect.height > 0 ? menuRect.height : estimatedHeight
    const next = anchoredMenuBox(rect, { width: window.innerWidth, height: window.innerHeight }, height, {
      gap,
      width,
      minWidth,
      menuWidth: menuRect?.width ?? 0,
      prefer,
      align,
    })
    const zIndex = anchor.closest('[data-zen-root]') ? ZEN_ANCHORED_MENU_LAYER : menuLayerForTrigger(anchor)
    const fontSize = getComputedStyle(anchor).fontSize || undefined
    const full = { ...next, zIndex, fontSize }
    setBox((prev) => (prev && JSON.stringify(prev) === JSON.stringify(full) ? prev : full))
  }, [gap, width, minWidth, estimatedHeight, prefer, align])

  useLayoutEffect(() => {
    if (!open) {
      setBox(null)
      return
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    const ro =
      typeof ResizeObserver === 'function'
        ? new ResizeObserver(() => place())
        : null
    if (ro && anchorRef.current) ro.observe(anchorRef.current)
    if (ro && menuEl.current) ro.observe(menuEl.current)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      ro?.disconnect()
    }
  }, [open, place])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      const t = e.target
      if (!(t instanceof Node)) return
      if (anchorRef.current?.contains(t) || menuEl.current?.contains(t)) return
      onCloseRef.current()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onCloseRef.current()
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  // Refs attach before layout effects run, so the first placement already
  // measures the menu; later size changes come through the ResizeObserver.
  const menuRef = useCallback((el: HTMLDivElement | null) => {
    menuEl.current = el
  }, [])

  // Not placed yet (first commit): in the DOM so it can be measured and
  // focused, but not seen or hit.
  const style: React.CSSProperties = box
    ? {
        position: 'fixed',
        top: box.top,
        bottom: box.bottom,
        left: box.left,
        width: box.width,
        minWidth: box.minWidth,
        zIndex: box.zIndex,
        fontSize: box.fontSize,
      }
    : { position: 'fixed', top: 0, left: 0, opacity: 0, pointerEvents: 'none' }

  const portal = (node: React.ReactNode): React.ReactPortal | null => {
    const anchor = anchorRef.current
    if (!anchor) return null
    const zen = anchor.closest('[data-zen-root]')
    return createPortal(node, zen instanceof HTMLElement ? zen : document.body)
  }

  return { anchorRef, menuRef, style, placement: box?.placement ?? prefer, portal }
}
