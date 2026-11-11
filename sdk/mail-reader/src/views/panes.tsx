// The reader's columns (K2's own; not from OCmail): how wide each is, the
// handles that resize them, and when the reader stacks them.
//
// At NARROW_BELOW (840px of the reader's own width, the width other K2 split
// pages fold at) and above: three columns, mailboxes · list · conversation.
// The first two are resizable between their minimum and maximum; the
// conversation keeps at least MESSAGE_MIN, and when the window shrinks the
// list gives way first, then the mailbox column. Below it: one column at a
// time, mailboxes → list → conversation, each with Back.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'

export const NARROW_BELOW = 840
export const SIDEBAR = { min: 184, max: 320, initial: 232 } as const
export const LIST = { min: 260, max: 520, initial: 340 } as const
export const MESSAGE_MIN = 360

const STORE_KEY = 'mail-reader.panes.v1'

export interface PaneWidths {
  sidebar: number
  list: number
}

const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n))

/** Widths that fit `total`: each within its own range, and the conversation
 *  at least MESSAGE_MIN (the list gives way first). */
export function fitPanes(want: PaneWidths, total: number): PaneWidths {
  let sidebar = clamp(want.sidebar, SIDEBAR.min, SIDEBAR.max)
  let list = clamp(want.list, LIST.min, LIST.max)
  if (total > 0) {
    let over = sidebar + list + MESSAGE_MIN - total
    if (over > 0) {
      const give = Math.min(over, list - LIST.min)
      list -= give
      over -= give
    }
    if (over > 0) sidebar -= Math.min(over, sidebar - SIDEBAR.min)
  }
  return { sidebar: Math.round(sidebar), list: Math.round(list) }
}

function readStored(): PaneWidths {
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem(STORE_KEY) : null
    const v = raw ? (JSON.parse(raw) as Partial<PaneWidths>) : null
    if (v && typeof v.sidebar === 'number' && typeof v.list === 'number') return { sidebar: v.sidebar, list: v.list }
  } catch {
    /* no storage: the defaults */
  }
  return { sidebar: SIDEBAR.initial, list: LIST.initial }
}

function writeStored(w: PaneWidths): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(w))
  } catch {
    /* no storage: this session only */
  }
}

/** The reader's width (0 before it is measured, or with no ResizeObserver). */
export function useElementWidth<T extends HTMLElement>(): [React.RefObject<T | null>, number] {
  const ref = useRef<T | null>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    setWidth(el.getBoundingClientRect().width)
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width
      if (typeof w === 'number') setWidth(w)
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, width]
}

/** The two resizable widths, kept per viewer (browser storage). */
export function usePaneWidths(total: number): [PaneWidths, (next: PaneWidths) => void] {
  const [want, setWant] = useState<PaneWidths>(readStored)
  const set = useCallback((next: PaneWidths) => {
    setWant(next)
    writeStored(next)
  }, [])
  return [fitPanes(want, total), set]
}

/** A column's right edge you can drag (or arrow) to resize it. */
export function PaneHandle({
  label,
  value,
  min,
  max,
  initial,
  onChange,
  testId,
}: {
  label: string
  value: number
  min: number
  max: number
  /** Where a double-click puts it back. */
  initial: number
  onChange: (next: number) => void
  testId?: string
}): React.JSX.Element {
  const drag = useRef<{ x: number; start: number } | null>(null)
  const [active, setActive] = useState(false)
  useEffect(() => {
    if (!active) return
    const prev = document.body.style.cursor
    document.body.style.cursor = 'col-resize'
    return () => {
      document.body.style.cursor = prev
    }
  }, [active])
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      data-mr="pane-handle"
      data-testid={testId}
      data-active={active ? 'true' : undefined}
      title={label}
      onPointerDown={(e) => {
        e.preventDefault()
        ;(e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId)
        drag.current = { x: e.clientX, start: value }
        setActive(true)
      }}
      onPointerMove={(e) => {
        if (!drag.current) return
        onChange(clamp(drag.current.start + e.clientX - drag.current.x, min, max))
      }}
      onPointerUp={() => {
        drag.current = null
        setActive(false)
      }}
      onPointerCancel={() => {
        drag.current = null
        setActive(false)
      }}
      onDoubleClick={() => onChange(clamp(initial, min, max))}
      onKeyDown={(e) => {
        const step = e.shiftKey ? 48 : 16
        if (e.key === 'ArrowLeft') onChange(clamp(value - step, min, max))
        else if (e.key === 'ArrowRight') onChange(clamp(value + step, min, max))
        else return
        e.preventDefault()
      }}
      className="group relative z-10 -mx-[3px] w-[7px] flex-shrink-0 cursor-col-resize no-drag outline-none flex justify-center"
    >
      <span
        aria-hidden="true"
        className={`w-px h-full transition-colors ${active ? 'bg-[var(--color-accent)]' : 'bg-transparent group-hover:bg-[var(--color-accent)] group-focus-visible:bg-[var(--color-accent)]'}`}
      />
    </div>
  )
}
