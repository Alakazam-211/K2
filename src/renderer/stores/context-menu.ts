import type { ReactNode } from 'react'
import { create } from 'zustand'

export interface ContextMenuItemDef {
  id: string
  label: string
  /** `separator`; `heading` (small muted group title) and `note` (muted
   *  wrapping text) are not selectable. */
  type?: string
  /** A "▸" row that opens these rows beside it (hover or click). Picking
   *  one resolves the menu with that row's id. */
  submenu?: ContextMenuItemDef[]
  enabled?: boolean
  /** When set, render a checkbox in front of the label. */
  checked?: boolean
  /** Small pill next to the label (e.g. "coming soon" for disabled stubs). */
  badge?: string
  /** Leading icon. Omitted rows stay text-only. */
  icon?: ReactNode
  /** Trailing shortcut hint (e.g. "⌘T"). Not a badge. Hidden while `hint` is set. */
  shortcut?: string
  /** Plain trailing words (e.g. "open in sandbox"). Not a key combo. */
  hint?: string
}

/** A row a click or Enter can pick: not a separator, heading or note, not
 *  disabled, and not a submenu parent (that opens its rows instead). */
export function isSelectableItem(item: ContextMenuItemDef): boolean {
  return (
    item.type !== 'separator' &&
    item.type !== 'heading' &&
    item.type !== 'note' &&
    item.enabled !== false &&
    !item.submenu
  )
}

/**
 * CSS `zoom` on <html> (applyK2SOZoom): clientX/Y and getBoundingClientRect
 * are post-zoom; position:fixed left/top are pre-zoom. Divide by zoom.
 * WKWebView may want the inverse — keep `/` to match column-resize tests.
 */
export function clientToCssPx(clientPx: number): number {
  const z = typeof window !== 'undefined' ? window.__k2soZoom ?? 1 : 1
  return z > 0 ? clientPx / z : clientPx
}

interface ContextMenuState {
  isOpen: boolean
  x: number
  y: number
  items: ContextMenuItemDef[]
  onSelect: ((id: string | null) => void) | null
  focusedIndex: number

  show: (x: number, y: number, items: ContextMenuItemDef[]) => Promise<string | null>
  /** Swap rows on an open menu. Leaves `isOpen` and `onSelect` alone. */
  replaceItems: (items: ContextMenuItemDef[]) => void
  close: () => void
  selectItem: (id: string) => void
  setFocusedIndex: (index: number) => void
}

export const useContextMenuStore = create<ContextMenuState>((set, get) => ({
  isOpen: false,
  x: 0,
  y: 0,
  items: [],
  onSelect: null,
  focusedIndex: -1,

  show: (x, y, items) => {
    return new Promise<string | null>((resolve) => {
      // Close any existing menu first
      const current = get()
      if (current.isOpen && current.onSelect) {
        current.onSelect(null)
      }

      set({
        isOpen: true,
        x: clientToCssPx(x),
        y: clientToCssPx(y),
        items,
        onSelect: resolve,
        focusedIndex: -1
      })
    })
  },

  replaceItems: (items) => {
    if (!get().isOpen) return
    set({ items })
  },

  close: () => {
    const { onSelect } = get()
    if (onSelect) {
      onSelect(null)
    }
    set({
      isOpen: false,
      items: [],
      onSelect: null,
      focusedIndex: -1
    })
  },

  selectItem: (id) => {
    const { onSelect, items } = get()
    const item = items.find((i) => i.id === id) ?? items.flatMap((i) => i.submenu ?? []).find((i) => i.id === id)
    if (item && isSelectableItem(item)) {
      set({
        isOpen: false,
        items: [],
        onSelect: null,
        focusedIndex: -1
      })
      if (onSelect) {
        onSelect(id)
      }
    }
  },

  setFocusedIndex: (index) => set({ focusedIndex: index })
}))
