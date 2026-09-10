import { create } from 'zustand'

export interface ContextMenuItemDef {
  id: string
  label: string
  type?: string
  enabled?: boolean
  /** When set, render a checkbox in front of the label. */
  checked?: boolean
  /** Small pill next to the label (e.g. "coming soon" for disabled stubs). */
  badge?: string
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
    const item = items.find((i) => i.id === id)
    if (item && item.type !== 'separator' && item.enabled !== false) {
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
